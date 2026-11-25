//! Video encoders.

use crate::capture::CapturedFrame;
use crate::{HostError, Result};

pub trait VideoEncoder {
    /// Encode one frame to an Annex-B bitstream. May return an empty Vec if
    /// the encoder chose to skip the frame.
    fn encode(&mut self, frame: &CapturedFrame, force_idr: bool) -> Result<Vec<u8>>;
    /// `rv_proto::codec_flags::CODEC_*` of the produced stream.
    fn codec(&self) -> u8;
}

/// BGRA8 -> planar I420 (BT.709 limited range), `w`/`h` must be even.
/// OpenH264 consumes I420 (not NV12), hence planar output. A future AMF
/// encoder wants NV12 (or the D3D11 texture directly, which is the point).
pub fn bgra_to_i420(bgra: &[u8], w: usize, h: usize, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
    debug_assert!(w % 2 == 0 && h % 2 == 0);
    debug_assert_eq!(bgra.len(), w * h * 4);
    let cw = w / 2;
    for row in 0..h {
        for col in 0..w {
            let p = (row * w + col) * 4;
            let (b, g, r) = (bgra[p] as i32, bgra[p + 1] as i32, bgra[p + 2] as i32);
            // BT.709 limited: Y = 16 + (0.1826R + 0.6142G + 0.0620B)
            let yy = 16 + ((47 * r + 157 * g + 16 * b + 128) >> 8);
            y[row * w + col] = yy.clamp(0, 255) as u8;
        }
    }
    for row in 0..h / 2 {
        for col in 0..cw {
            let (mut sb, mut sg, mut sr) = (0i32, 0i32, 0i32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = ((row * 2 + dy) * w + col * 2 + dx) * 4;
                sb += bgra[p] as i32;
                sg += bgra[p + 1] as i32;
                sr += bgra[p + 2] as i32;
            }
            let (b, g, r) = (sb / 4, sg / 4, sr / 4);
            // Cb = 128 + (-0.1006R - 0.3386G + 0.4392B), Cr = 128 + (0.4392R - 0.3989G - 0.0403B)
            let cb = 128 + ((-26 * r - 87 * g + 112 * b + 128) >> 8);
            let cr = 128 + ((112 * r - 102 * g - 10 * b + 128) >> 8);
            u[row * cw + col] = cb.clamp(0, 255) as u8;
            v[row * cw + col] = cr.clamp(0, 255) as u8;
        }
    }
}

struct I420 {
    w: usize,
    h: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}

impl openh264::formats::YUVSource for I420 {
    fn dimensions(&self) -> (usize, usize) {
        (self.w, self.h)
    }
    fn strides(&self) -> (usize, usize, usize) {
        (self.w, self.w / 2, self.w / 2)
    }
    fn y(&self) -> &[u8] {
        &self.y
    }
    fn u(&self) -> &[u8] {
        &self.u
    }
    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Software H.264 fallback (Cisco OpenH264, built from source by the crate).
///
/// Works without any GPU so the pipeline runs end to end, but a CPU encoder
/// will not reach 2800x1752@120: expect tens of ms per frame at that size.
/// Also note H.264 level limits (level 5.2 caps at ~2.07M macroblocks/s,
/// below 2800x1752x120).
pub struct OpenH264Encoder {
    enc: Option<openh264::encoder::Encoder>,
    dims: (usize, usize),
    fps: f32,
    bitrate_bps: u32,
    bgra: Vec<u8>,
    yuv: I420,
}

impl OpenH264Encoder {
    pub fn new(fps: u32, bitrate_bps: u32) -> Self {
        Self {
            enc: None,
            dims: (0, 0),
            fps: fps as f32,
            bitrate_bps,
            bgra: Vec::new(),
            yuv: I420 { w: 0, h: 0, y: vec![], u: vec![], v: vec![] },
        }
    }

    fn rebuild(&mut self, w: usize, h: usize) -> Result<()> {
        use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, RateControlMode, UsageType};
        use openh264::OpenH264API;
        let cfg = EncoderConfig::new()
            .bitrate(BitRate::from_bps(self.bitrate_bps))
            .max_frame_rate(FrameRate::from_hz(self.fps))
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(false);
        self.enc = Some(
            Encoder::with_api_config(OpenH264API::from_source(), cfg)
                .map_err(|e| HostError::Encoder(e.to_string()))?,
        );
        self.dims = (w, h);
        self.yuv = I420 {
            w,
            h,
            y: vec![0; w * h],
            u: vec![0; w * h / 4],
            v: vec![0; w * h / 4],
        };
        Ok(())
    }
}

impl VideoEncoder for OpenH264Encoder {
    fn encode(&mut self, frame: &CapturedFrame, force_idr: bool) -> Result<Vec<u8>> {
        let (fw, fh) = (frame.width as usize, frame.height as usize);
        let (w, h) = (fw & !1, fh & !1); // crop to even
        if w == 0 || h == 0 {
            return Err(HostError::Encoder("empty frame".into()));
        }
        if self.enc.is_none() || self.dims != (w, h) {
            self.rebuild(w, h)?;
        }
        let mut raw = std::mem::take(&mut self.bgra);
        frame.read_bgra(&mut raw)?;
        let cropped;
        let src: &[u8] = if (w, h) == (fw, fh) {
            &raw
        } else {
            let mut c = Vec::with_capacity(w * h * 4);
            for r in 0..h {
                c.extend_from_slice(&raw[r * fw * 4..(r * fw + w) * 4]);
            }
            cropped = c;
            &cropped
        };
        let I420 { y, u, v, .. } = &mut self.yuv;
        bgra_to_i420(src, w, h, y, u, v);
        self.bgra = raw;
        let enc = self.enc.as_mut().expect("encoder built");
        if force_idr {
            enc.force_intra_frame();
        }
        let bs = enc.encode(&self.yuv).map_err(|e| HostError::Encoder(e.to_string()))?;
        Ok(bs.to_vec())
    }

    fn codec(&self) -> u8 {
        rv_proto::codec_flags::CODEC_H264
    }
}

/// AMD AMF hardware encoder: NOT IMPLEMENTED.
///
/// Plan (native AMF SDK, `amf.dll` loaded at runtime, no Rust binding assumed):
/// * component `AMFVideoEncoder_HEVC`, `USAGE = ULTRA_LOW_LATENCY`
/// * no B-frames, `QUALITY_PRESET = SPEED`, GOP = infinite + on-demand IDR
///   (`FORCE_PICTURE_TYPE = IDR`)
/// * `RATE_CONTROL_METHOD = CBR`, HRD enforced, `VBV_BUFFER_SIZE` = 1.5x the
///   per-frame budget at target bitrate (ie. bitrate/fps*1.5)
/// * input: wrap the captured `ID3D11Texture2D` via `AMFContext::CreateSurfaceFromDX11Native`
///   after a BGRA->NV12 video-processor/compute pass (no CPU readback).
pub struct AmfEncoder;

impl VideoEncoder for AmfEncoder {
    fn encode(&mut self, _frame: &CapturedFrame, _force_idr: bool) -> Result<Vec<u8>> {
        Err(HostError::Unsupported("AMF encoder not implemented"))
    }
    fn codec(&self) -> u8 {
        rv_proto::codec_flags::CODEC_H265
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_conversion_known_points() {
        // 2x2 white / black / mid-grey
        for (px, ey) in [([255u8, 255, 255, 255], 235i32), ([0, 0, 0, 255], 16), ([128, 128, 128, 255], 126)] {
            let bgra: Vec<u8> = px.iter().cycle().take(16).copied().collect();
            let (mut y, mut u, mut v) = ([0u8; 4], [0u8; 1], [0u8; 1]);
            bgra_to_i420(&bgra, 2, 2, &mut y, &mut u, &mut v);
            assert!((y[0] as i32 - ey).abs() <= 2, "y={} expected {}", y[0], ey);
            assert!((u[0] as i32 - 128).abs() <= 2 && (v[0] as i32 - 128).abs() <= 2);
        }
    }

    #[test]
    fn openh264_produces_idr_annexb() {
        let (w, h) = (128u32, 64u32);
        let mut px = Vec::new();
        for i in 0..w * h {
            px.extend_from_slice(&[(i % 256) as u8, 40, 200, 255]);
        }
        let f = CapturedFrame::from_bgra(w, h, px);
        let mut e = OpenH264Encoder::new(30, 2_000_000);
        let out = e.encode(&f, true).unwrap();
        assert!(out.len() > 16, "got {} bytes", out.len());
        assert_eq!(&out[..4], &[0, 0, 0, 1]);
    }
}
