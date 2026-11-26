//! Video encoders.
//!
//! Selection order (see [`create_encoder`]): Media Foundation HEVC hardware
//! MFT -> Media Foundation H.264 hardware MFT -> OpenH264 software. The MF
//! path (`mf.rs`) is the primary one; [`AmfEncoder`] is only a stub kept for a
//! possible native AMF SDK path later.

use crate::capture::CapturedFrame;
use crate::{HostError, Result};

#[cfg(windows)]
pub mod mf;

/// Which concrete encoder is producing the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderKind {
    MfHevcHw,
    MfH264Hw,
    OpenH264,
}

impl EncoderKind {
    /// Stable name used in the JSON `stats.encoder` field.
    pub fn name(self) -> &'static str {
        match self {
            EncoderKind::MfHevcHw => "mf-hevc-hw",
            EncoderKind::MfH264Hw => "mf-h264-hw",
            EncoderKind::OpenH264 => "openh264",
        }
    }
    pub fn codec_flag(self) -> u8 {
        match self {
            EncoderKind::MfHevcHw => rv_proto::codec_flags::CODEC_H265,
            _ => rv_proto::codec_flags::CODEC_H264,
        }
    }
    pub fn slider_codec(self) -> rv_session::Codec {
        match self {
            EncoderKind::MfHevcHw => rv_session::Codec::Hevc,
            _ => rv_session::Codec::H264,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodecPref {
    Auto,
    Hevc,
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderPref {
    Auto,
    Hw,
    Sw,
}

/// Output stream parameters. `width`/`height` are the *encoded* size (even);
/// frames of another size are scaled (GPU video processor / CPU nearest).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
}

pub trait VideoEncoder {
    /// Encode one frame to an Annex-B bitstream. May return an empty Vec if
    /// the encoder chose to skip the frame.
    fn encode(&mut self, frame: &CapturedFrame, force_idr: bool) -> Result<Vec<u8>>;
    /// `rv_proto::codec_flags::CODEC_*` of the produced stream.
    fn codec(&self) -> u8;
    /// Which encoder this is.
    fn kind(&self) -> EncoderKind;
    /// Change the target bitrate without re-initialising. `Ok(true)` if
    /// applied live, `Ok(false)` if the caller must rebuild the encoder.
    fn set_bitrate(&mut self, _bps: u32) -> Result<bool> {
        Ok(false)
    }
}

/// Builds the best available encoder. `sample` is a real captured frame (its
/// D3D11 device is reused by the MF path). `bitrate_for` maps the chosen
/// codec to a bitrate (slider formula depends on the codec). `log` receives
/// human-readable progress/warnings.
/// Encoders that failed at runtime in this process (e.g. MFT that errors on every frame).
/// `create_encoder` skips them so a broken hardware path falls back instead of looping.
static BLOCKED: std::sync::Mutex<Vec<EncoderKind>> = std::sync::Mutex::new(Vec::new());

pub fn block_encoder(kind: EncoderKind) {
    if let Ok(mut b) = BLOCKED.lock() {
        if !b.contains(&kind) {
            b.push(kind);
        }
    }
}

pub fn is_blocked(kind: EncoderKind) -> bool {
    BLOCKED.lock().map(|b| b.contains(&kind)).unwrap_or(false)
}

pub fn create_encoder(
    codec: CodecPref,
    enc: EncoderPref,
    mut cfg: EncoderConfig,
    sample: &CapturedFrame,
    bitrate_for: &dyn Fn(rv_session::Codec) -> u32,
    log: &mut dyn FnMut(&str),
) -> Result<Box<dyn VideoEncoder>> {
    cfg.width &= !1;
    cfg.height &= !1;
    if enc != EncoderPref::Sw {
        #[cfg(windows)]
        {
            let mut kinds = Vec::new();
            if codec != CodecPref::H264 {
                kinds.push(EncoderKind::MfHevcHw);
            }
            {
                kinds.push(EncoderKind::MfH264Hw);
            }
            for k in kinds {
                if is_blocked(k) {
                    log(&format!("encoder {} skipped (failed earlier)", k.name()));
                    continue;
                }
                let c = EncoderConfig { bitrate_bps: bitrate_for(k.slider_codec()), ..cfg };
                match mf::MfEncoder::new(k, c, sample) {
                    Ok(e) => {
                        log(&format!("encoder: {} ({})", k.name(), e.describe()));
                        return Ok(Box::new(e));
                    }
                    Err(e) => log(&format!("encoder {} unavailable: {e}", k.name())),
                }
            }
        }
        if enc == EncoderPref::Hw {
            return Err(HostError::Encoder("no hardware encoder available (--encoder hw)".into()));
        }
    }
    let _ = sample;
    log("encoder: openh264 (software)");
    Ok(Box::new(OpenH264Encoder::with_config(OpenH264Encoder::cfg_from(
        EncoderConfig { bitrate_bps: bitrate_for(rv_session::Codec::H264), ..cfg },
    ))))
}

/// BGRA8 -> NV12 (BT.709 limited), `w`/`h` even. `out` is resized to w*h*3/2.
pub fn bgra_to_nv12(bgra: &[u8], w: usize, h: usize, out: &mut Vec<u8>) {
    debug_assert!(w % 2 == 0 && h % 2 == 0);
    debug_assert_eq!(bgra.len(), w * h * 4);
    out.resize(w * h * 3 / 2, 0);
    let (yp, uv) = out.split_at_mut(w * h);
    for row in 0..h {
        for col in 0..w {
            let p = (row * w + col) * 4;
            let (b, g, r) = (bgra[p] as i32, bgra[p + 1] as i32, bgra[p + 2] as i32);
            yp[row * w + col] = (16 + ((47 * r + 157 * g + 16 * b + 128) >> 8)).clamp(0, 255) as u8;
        }
    }
    for row in 0..h / 2 {
        for col in 0..w / 2 {
            let (mut sb, mut sg, mut sr) = (0i32, 0i32, 0i32);
            for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                let p = ((row * 2 + dy) * w + col * 2 + dx) * 4;
                sb += bgra[p] as i32;
                sg += bgra[p + 1] as i32;
                sr += bgra[p + 2] as i32;
            }
            let (b, g, r) = (sb / 4, sg / 4, sr / 4);
            uv[row * w + col * 2] = (128 + ((-26 * r - 87 * g + 112 * b + 128) >> 8)).clamp(0, 255) as u8;
            uv[row * w + col * 2 + 1] = (128 + ((112 * r - 102 * g - 10 * b + 128) >> 8)).clamp(0, 255) as u8;
        }
    }
}

/// Nearest-neighbour BGRA scale (CPU fallback only).
pub fn scale_bgra_nearest(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize, out: &mut Vec<u8>) {
    out.resize(dw * dh * 4, 0);
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            let (s, d) = ((sy * sw + sx) * 4, (y * dw + x) * 4);
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
}

/// True if an Annex-B buffer contains an SPS (H.264 type 7 / HEVC type 33).
pub fn has_sps(annexb: &[u8], hevc: bool) -> bool {
    let mut i = 0;
    while i + 3 < annexb.len() {
        if annexb[i] == 0 && annexb[i + 1] == 0 && annexb[i + 2] == 1 {
            let b = annexb[i + 3];
            let t = if hevc { (b >> 1) & 0x3f } else { b & 0x1f };
            if (hevc && t == 33) || (!hevc && t == 7) {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
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
    /// Fixed output size (0,0 = follow the frame size, cropped to even).
    target: (usize, usize),
    fps: f32,
    bitrate_bps: u32,
    bgra: Vec<u8>,
    scaled: Vec<u8>,
    yuv: I420,
}

impl OpenH264Encoder {
    pub fn new(fps: u32, bitrate_bps: u32) -> Self {
        Self::with_config(EncoderConfig { width: 0, height: 0, fps, bitrate_bps })
    }

    pub(crate) fn cfg_from(c: EncoderConfig) -> EncoderConfig {
        c
    }

    pub fn with_config(c: EncoderConfig) -> Self {
        Self {
            enc: None,
            dims: (0, 0),
            target: (c.width as usize, c.height as usize),
            fps: c.fps as f32,
            bitrate_bps: c.bitrate_bps,
            bgra: Vec::new(),
            scaled: Vec::new(),
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
        let (w, h) = if self.target.0 > 0 && self.target.1 > 0 {
            (self.target.0 & !1, self.target.1 & !1)
        } else {
            (fw & !1, fh & !1) // crop to even
        };
        if w == 0 || h == 0 {
            return Err(HostError::Encoder("empty frame".into()));
        }
        if self.enc.is_none() || self.dims != (w, h) {
            self.rebuild(w, h)?;
        }
        let mut raw = std::mem::take(&mut self.bgra);
        frame.read_bgra(&mut raw)?;
        let src: &[u8] = if (w, h) == (fw, fh) {
            &raw
        } else if w <= fw && h <= fh && self.target == (0, 0) {
            let mut c = std::mem::take(&mut self.scaled);
            c.clear();
            for r in 0..h {
                c.extend_from_slice(&raw[r * fw * 4..(r * fw + w) * 4]);
            }
            self.scaled = c;
            &self.scaled
        } else {
            scale_bgra_nearest(&raw, fw, fh, w, h, &mut self.scaled);
            &self.scaled
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
    fn kind(&self) -> EncoderKind {
        EncoderKind::OpenH264
    }
}

/// AMD AMF hardware encoder: NOT IMPLEMENTED. The Media Foundation path
/// (`mf.rs`, which uses AMD's AMF-backed MFT) is primary for now; this stub is
/// only a placeholder for a native AMF SDK path.
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
    fn kind(&self) -> EncoderKind {
        EncoderKind::MfHevcHw
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
    fn nv12_known_points() {
        let bgra: Vec<u8> = [255u8, 255, 255, 255].iter().cycle().take(16).copied().collect();
        let mut out = Vec::new();
        bgra_to_nv12(&bgra, 2, 2, &mut out);
        assert_eq!(out.len(), 6);
        assert!((out[0] as i32 - 235).abs() <= 2);
        assert!((out[4] as i32 - 128).abs() <= 2 && (out[5] as i32 - 128).abs() <= 2);
    }

    #[test]
    fn nearest_scale_halves() {
        let src: Vec<u8> = (0..16u8).flat_map(|i| [i, i, i, 255]).collect(); // 4x4
        let mut out = Vec::new();
        scale_bgra_nearest(&src, 4, 4, 2, 2, &mut out);
        assert_eq!(out.len(), 16);
        assert_eq!(out[0], 0);
        assert_eq!(out[4], 2);
        assert_eq!(out[8], 8);
    }

    #[test]
    fn sps_detection() {
        assert!(has_sps(&[0, 0, 0, 1, 0x67, 1, 2], false));
        assert!(!has_sps(&[0, 0, 0, 1, 0x65, 1, 2], false));
        assert!(has_sps(&[0, 0, 0, 1, 0x40, 1, 0, 0, 1, 0x42, 1], true));
        assert!(!has_sps(&[0, 0, 1, 0x26, 1], true));
        assert!(!has_sps(&[], true));
    }

    #[test]
    fn kind_names() {
        assert_eq!(EncoderKind::MfHevcHw.name(), "mf-hevc-hw");
        assert_eq!(EncoderKind::MfH264Hw.name(), "mf-h264-hw");
        assert_eq!(EncoderKind::OpenH264.name(), "openh264");
    }

    #[test]
    fn factory_falls_back_to_software() {
        let f = CapturedFrame::from_bgra(64, 64, vec![90u8; 64 * 64 * 4]);
        let mut logs = Vec::new();
        let cfg = EncoderConfig { width: 64, height: 64, fps: 30, bitrate_bps: 1_000_000 };
        let r = create_encoder(CodecPref::Auto, EncoderPref::Sw, cfg, &f, &|_| 1_000_000, &mut |m| logs.push(m.to_string()));
        let mut e = r.unwrap();
        assert_eq!(e.kind(), EncoderKind::OpenH264);
        assert!(e.encode(&f, true).unwrap().len() > 8);
        let hw = create_encoder(CodecPref::Auto, EncoderPref::Hw, cfg, &f, &|_| 1_000_000, &mut |_| {});
        assert!(hw.is_err());
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
