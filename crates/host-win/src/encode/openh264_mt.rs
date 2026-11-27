//! Multi-slice, multi-threaded OpenH264 software H.264 encoder.
//!
//! The high-level `openh264::encoder::Encoder` wrapper hard-codes
//! `SM_SINGLE_SLICE` (`uiSliceNum = 1`), which causes OpenH264's
//! `InitSliceSettings` to clamp `iMultipleThreadIdc` down to 1 thread even when
//! `num_threads(N)` is configured.
//!
//! [`MtOpenH264`] talks directly to the `openh264-sys2` C API (`ISVCEncoder`
//! vtable + `SEncParamExt`) with `SM_FIXEDSLCNUM_SLICE`, enabling OpenH264's
//! parallel slice-encoding thread pool and live bitrate updates via
//! `SetOption(ENCODER_OPTION_BITRATE, ...)`.

use std::os::raw::{c_int, c_longlong, c_void};
use std::ptr;

use openh264_sys2::{
    API as _, CONSTANT_ID, DEBLOCKING_IDC_0, DEBLOCKING_IDC_2, ENCODER_OPTION,
    ENCODER_OPTION_BITRATE, ENCODER_OPTION_DATAFORMAT, ENCODER_OPTION_MAX_BITRATE,
    ENCODER_OPTION_TRACE_LEVEL, ISVCEncoder, ISVCEncoderVtbl, LOW_COMPLEXITY, RC_BITRATE_MODE,
    SBitrateInfo, SCREEN_CONTENT_REAL_TIME, SEncParamExt, SFrameBSInfo, SM_FIXEDSLCNUM_SLICE,
    SM_SINGLE_SLICE, SPATIAL_LAYER_0, SPATIAL_LAYER_ALL, SSourcePicture, UNSPECIFIED_BIT_RATE,
    WELS_LOG_ERROR, cmResultSuccess, videoFormatI420, videoFrameTypeInvalid, videoFrameTypeSkip,
};

use crate::{HostError, Result};

/// Minimum macroblocks per frame required by OpenH264 (`MIN_NUM_MB_PER_SLICE`)
/// before multi-slice mode is accepted.
const OPENH264_MIN_MB_FOR_MULTI_SLICE: u32 = 48;

/// Cached function pointers extracted from an `ISVCEncoder` vtable.
struct EncoderVtbl {
    initialize_ext:
        unsafe extern "C" fn(arg1: *mut ISVCEncoder, p_param: *const SEncParamExt) -> c_int,
    get_default_params:
        unsafe extern "C" fn(arg1: *mut ISVCEncoder, p_param: *mut SEncParamExt) -> c_int,
    uninitialize: unsafe extern "C" fn(arg1: *mut ISVCEncoder) -> c_int,
    encode_frame: unsafe extern "C" fn(
        arg1: *mut ISVCEncoder,
        kp_src_pic: *const SSourcePicture,
        p_bs_info: *mut SFrameBSInfo,
    ) -> c_int,
    force_intra_frame: unsafe extern "C" fn(arg1: *mut ISVCEncoder, b_idr: bool) -> c_int,
    set_option: unsafe extern "C" fn(
        arg1: *mut ISVCEncoder,
        e_option_id: ENCODER_OPTION,
        p_option: *mut c_void,
    ) -> c_int,
}

/// Multi-slice, multi-threaded Cisco OpenH264 encoder using `openh264-sys2`.
pub struct MtOpenH264 {
    api: openh264_sys2::DynamicAPI,
    encoder_ptr: *mut ISVCEncoder,
    vtbl: EncoderVtbl,
    initialized: bool,
    width: u32,
    height: u32,
    threads: u16,
    bs_info: SFrameBSInfo,
}

// SAFETY: `MtOpenH264` owns its `ISVCEncoder` instance exclusively and every
// mutating operation requires `&mut self`. OpenH264 encoder handles have no
// thread-affinity requirement and may be moved between threads.
unsafe impl Send for MtOpenH264 {}

impl MtOpenH264 {
    /// Creates and initializes a multi-slice, multi-threaded OpenH264 encoder
    /// configured for real-time screen content at `width x height`.
    ///
    /// `width` and `height` must be non-zero even numbers within OpenH264's
    /// level 5.2 resolution limit (3840x2160 horizontal or 2160x3840 vertical).
    pub fn new(width: u32, height: u32, fps: u32, bitrate_bps: u32) -> Result<Self> {
        if width == 0 || height == 0 || (width & 1) != 0 || (height & 1) != 0 {
            return Err(HostError::Encoder(format!(
                "openh264_mt: dimensions must be non-zero and even, got {width}x{height}"
            )));
        }
        let max_dim = width.max(height);
        let min_dim = width.min(height);
        if max_dim > 3840 || min_dim > 2160 {
            return Err(HostError::Encoder(format!(
                "openh264_mt: resolution {width}x{height} exceeds OpenH264 max 3840x2160"
            )));
        }
        if bitrate_bps == 0 {
            return Err(HostError::Encoder(
                "openh264_mt: bitrate_bps must be > 0".into(),
            ));
        }

        let requested_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(2, 8) as u16;

        let api = openh264_sys2::DynamicAPI::from_source();
        let mut encoder_ptr: *mut ISVCEncoder = ptr::null_mut();

        // SAFETY: `encoder_ptr` is a valid out-pointer for `WelsCreateSVCEncoder`.
        let create_rc = unsafe { api.WelsCreateSVCEncoder(&mut encoder_ptr) };
        if create_rc != cmResultSuccess || encoder_ptr.is_null() {
            return Err(HostError::Encoder(format!(
                "openh264_mt: WelsCreateSVCEncoder failed (rc={create_rc})"
            )));
        }

        // SAFETY: `encoder_ptr` is non-null after `WelsCreateSVCEncoder` succeeded;
        // verify the vtable pointer is also non-null before dereferencing.
        let vtbl_ptr: *const ISVCEncoderVtbl = unsafe { *encoder_ptr };
        if vtbl_ptr.is_null() {
            unsafe { api.WelsDestroySVCEncoder(encoder_ptr) };
            return Err(HostError::Encoder(
                "openh264_mt: null ISVCEncoderVtbl".into(),
            ));
        }

        let vtbl = match unsafe { extract_vtbl(&*vtbl_ptr) } {
            Ok(v) => v,
            Err(e) => {
                unsafe { api.WelsDestroySVCEncoder(encoder_ptr) };
                return Err(e);
            }
        };

        // Construct `Self` immediately so `Drop` always cleans up `encoder_ptr`
        // if any subsequent setup call returns an error.
        let mut enc = Self {
            api,
            encoder_ptr,
            vtbl,
            initialized: false,
            width,
            height,
            threads: requested_threads,
            bs_info: SFrameBSInfo::default(),
        };

        // Silence noisy info/warning logs before `InitializeExt` runs.
        // OpenH264 permits `ENCODER_OPTION_TRACE_LEVEL` prior to initialization.
        let mut trace_level: c_int = WELS_LOG_ERROR;
        let rc = unsafe {
            (enc.vtbl.set_option)(
                enc.encoder_ptr,
                ENCODER_OPTION_TRACE_LEVEL,
                ptr::addr_of_mut!(trace_level).cast(),
            )
        };
        if rc != cmResultSuccess {
            return Err(HostError::Encoder(format!(
                "openh264_mt: SetOption(TRACE_LEVEL) failed (rc={rc})"
            )));
        }

        let mut params = SEncParamExt::default();
        let rc = unsafe { (enc.vtbl.get_default_params)(enc.encoder_ptr, &mut params) };
        if rc != cmResultSuccess {
            return Err(HostError::Encoder(format!(
                "openh264_mt: GetDefaultParams failed (rc={rc})"
            )));
        }

        let target_fps = fps.max(1) as f32;
        let target_bitrate = bitrate_bps.min(i32::MAX as u32) as c_int;

        let (slice_mode, slice_num, effective_threads) =
            compute_slice_layout(width, height, requested_threads);
        enc.threads = effective_threads;

        params.iUsageType = SCREEN_CONTENT_REAL_TIME;
        params.iPicWidth = width as c_int;
        params.iPicHeight = height as c_int;
        params.fMaxFrameRate = target_fps;
        params.iTargetBitrate = target_bitrate;
        params.iMaxBitrate = UNSPECIFIED_BIT_RATE as c_int;
        params.iRCMode = RC_BITRATE_MODE;
        params.iComplexityMode = LOW_COMPLEXITY;
        params.bEnableFrameSkip = true;
        params.bEnableAdaptiveQuant = false;
        params.bEnableBackgroundDetection = false;
        params.bEnableSceneChangeDetect = true;
        params.bEnableDenoise = false;
        params.iMaxQp = 40;
        params.iMinQp = 12;
        params.uiIntraPeriod = 0;
        params.iSpatialLayerNum = 1;
        params.iTemporalLayerNum = 1;
        params.eSpsPpsIdStrategy = CONSTANT_ID;
        params.iMultipleThreadIdc = effective_threads;
        params.iLoopFilterDisableIdc = if slice_num > 1 {
            DEBLOCKING_IDC_2
        } else {
            DEBLOCKING_IDC_0
        };

        let layer0 = &mut params.sSpatialLayers[0];
        layer0.iVideoWidth = width as c_int;
        layer0.iVideoHeight = height as c_int;
        layer0.fFrameRate = target_fps;
        layer0.iSpatialBitrate = target_bitrate;
        layer0.iMaxSpatialBitrate = UNSPECIFIED_BIT_RATE as c_int;
        layer0.sSliceArgument.uiSliceMode = slice_mode;
        layer0.sSliceArgument.uiSliceNum = slice_num;

        let rc = unsafe { (enc.vtbl.initialize_ext)(enc.encoder_ptr, &params) };
        if rc != cmResultSuccess {
            return Err(HostError::Encoder(format!(
                "openh264_mt: InitializeExt({width}x{height}, fps={fps}, bps={bitrate_bps}, slices={slice_num}) failed (rc={rc})"
            )));
        }
        enc.initialized = true;

        let mut data_format: c_int = videoFormatI420;
        let rc = unsafe {
            (enc.vtbl.set_option)(
                enc.encoder_ptr,
                ENCODER_OPTION_DATAFORMAT,
                ptr::addr_of_mut!(data_format).cast(),
            )
        };
        if rc != cmResultSuccess {
            return Err(HostError::Encoder(format!(
                "openh264_mt: SetOption(DATAFORMAT) failed (rc={rc})"
            )));
        }

        Ok(enc)
    }

    /// Encodes a single I420 frame into an Annex-B H.264 bitstream.
    ///
    /// All NAL units across all layers and slices in `SFrameBSInfo` are
    /// concatenated in order. Returns an empty `Vec` when OpenH264 skips the
    /// frame (`videoFrameTypeSkip`) under rate control.
    pub fn encode_i420(
        &mut self,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        timestamp_ms: i64,
        force_idr: bool,
    ) -> Result<Vec<u8>> {
        let w = self.width as usize;
        let h = self.height as usize;
        let y_len = w * h;
        let uv_len = (w / 2) * (h / 2);
        if y.len() < y_len || u.len() < uv_len || v.len() < uv_len {
            return Err(HostError::Encoder(format!(
                "openh264_mt: I420 plane too small for {}x{} (got y={}, u={}, v={}, need y>={}, uv>={})",
                self.width,
                self.height,
                y.len(),
                u.len(),
                v.len(),
                y_len,
                uv_len
            )));
        }

        if force_idr {
            let rc = unsafe { (self.vtbl.force_intra_frame)(self.encoder_ptr, true) };
            if rc != cmResultSuccess {
                return Err(HostError::Encoder(format!(
                    "openh264_mt: ForceIntraFrame failed (rc={rc})"
                )));
            }
        }

        let src = SSourcePicture {
            iColorFormat: videoFormatI420,
            iStride: [self.width as c_int, (self.width / 2) as c_int, (self.width / 2) as c_int, 0],
            pData: [
                // OpenH264 reads source planes without mutating them; the C ABI
                // header declares `unsigned char* pData[4]` without `const`.
                y.as_ptr().cast_mut(),
                u.as_ptr().cast_mut(),
                v.as_ptr().cast_mut(),
                ptr::null_mut(),
            ],
            iPicWidth: self.width as c_int,
            iPicHeight: self.height as c_int,
            uiTimeStamp: timestamp_ms as c_longlong,
            bPsnrY: false,
            bPsnrU: false,
            bPsnrV: false,
        };

        self.bs_info = SFrameBSInfo::default();
        let rc = unsafe { (self.vtbl.encode_frame)(self.encoder_ptr, &src, &mut self.bs_info) };
        if rc != cmResultSuccess {
            return Err(HostError::Encoder(format!(
                "openh264_mt: EncodeFrame failed (rc={rc})"
            )));
        }

        if self.bs_info.eFrameType == videoFrameTypeSkip {
            return Ok(Vec::new());
        }
        if self.bs_info.eFrameType == videoFrameTypeInvalid {
            return Err(HostError::Encoder(
                "openh264_mt: EncodeFrame produced videoFrameTypeInvalid".into(),
            ));
        }

        let cap = self.bs_info.iFrameSizeInBytes.max(0) as usize;
        let mut out = Vec::with_capacity(cap);
        let layer_count = (self.bs_info.iLayerNum.max(0) as usize).min(self.bs_info.sLayerInfo.len());

        for layer in self.bs_info.sLayerInfo.iter().take(layer_count) {
            if layer.pBsBuf.is_null() || layer.pNalLengthInByte.is_null() || layer.iNalCount <= 0 {
                continue;
            }
            let nal_count = layer.iNalCount as usize;
            let mut offset = 0usize;
            for nal_idx in 0..nal_count {
                // SAFETY: OpenH264 populates `pNalLengthInByte[0..iNalCount]`
                // and `pBsBuf` with contiguous Annex-B NAL units for this layer.
                let nal_len = unsafe { *layer.pNalLengthInByte.add(nal_idx) };
                if nal_len <= 0 {
                    continue;
                }
                let len = nal_len as usize;
                let nal = unsafe { std::slice::from_raw_parts(layer.pBsBuf.add(offset), len) };
                out.extend_from_slice(nal);
                offset += len;
            }
        }

        Ok(out)
    }

    /// Updates the encoder's target bitrate live without re-initializing.
    ///
    /// Returns `Ok(true)` when applied live, or `Ok(false)` if OpenH264
    /// rejected the live update (caller may rebuild if needed).
    pub fn set_bitrate(&mut self, bps: u32) -> Result<bool> {
        if bps == 0 {
            return Err(HostError::Encoder(
                "openh264_mt: bitrate_bps must be > 0".into(),
            ));
        }
        let target = bps.min(i32::MAX as u32) as c_int;
        let mut info = SBitrateInfo {
            iLayer: SPATIAL_LAYER_ALL,
            iBitrate: target,
        };
        let rc = unsafe {
            (self.vtbl.set_option)(
                self.encoder_ptr,
                ENCODER_OPTION_BITRATE,
                ptr::addr_of_mut!(info).cast(),
            )
        };
        if rc == cmResultSuccess {
            return Ok(true);
        }

        // If `target` exceeded the layer's current max bitrate (derived from
        // the auto-selected H.264 level at init), raise `MAX_BITRATE` on
        // layer 0 (which also lets OpenH264 auto-promote level) and retry once.
        let max_target = target.saturating_add(target / 2);
        let mut max_info = SBitrateInfo {
            iLayer: SPATIAL_LAYER_0,
            iBitrate: max_target,
        };
        let max_rc = unsafe {
            (self.vtbl.set_option)(
                self.encoder_ptr,
                ENCODER_OPTION_MAX_BITRATE,
                ptr::addr_of_mut!(max_info).cast(),
            )
        };
        if max_rc == cmResultSuccess {
            let mut retry_info = SBitrateInfo {
                iLayer: SPATIAL_LAYER_ALL,
                iBitrate: target,
            };
            let retry_rc = unsafe {
                (self.vtbl.set_option)(
                    self.encoder_ptr,
                    ENCODER_OPTION_BITRATE,
                    ptr::addr_of_mut!(retry_info).cast(),
                )
            };
            if retry_rc == cmResultSuccess {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Returns the configured multi-slice / worker thread count for this encoder.
    pub fn threads(&self) -> u16 {
        self.threads
    }
}

impl Drop for MtOpenH264 {
    fn drop(&mut self) {
        unsafe {
            if self.initialized {
                let _ = (self.vtbl.uninitialize)(self.encoder_ptr);
                self.initialized = false;
            }
            if !self.encoder_ptr.is_null() {
                self.api.WelsDestroySVCEncoder(self.encoder_ptr);
                self.encoder_ptr = ptr::null_mut();
            }
        }
    }
}

fn extract_vtbl(vtbl: &ISVCEncoderVtbl) -> Result<EncoderVtbl> {
    let missing = |name: &str| HostError::Encoder(format!("openh264_mt: vtable missing {name}"));
    Ok(EncoderVtbl {
        initialize_ext: vtbl.InitializeExt.ok_or_else(|| missing("InitializeExt"))?,
        get_default_params: vtbl
            .GetDefaultParams
            .ok_or_else(|| missing("GetDefaultParams"))?,
        uninitialize: vtbl.Uninitialize.ok_or_else(|| missing("Uninitialize"))?,
        encode_frame: vtbl.EncodeFrame.ok_or_else(|| missing("EncodeFrame"))?,
        force_intra_frame: vtbl
            .ForceIntraFrame
            .ok_or_else(|| missing("ForceIntraFrame"))?,
        set_option: vtbl.SetOption.ok_or_else(|| missing("SetOption"))?,
    })
}

/// Computes `(uiSliceMode, uiSliceNum, iMultipleThreadIdc)` compatible with
/// OpenH264's GOM rate-control slice validator (`SliceArgumentValidationFixedSliceMode`).
///
/// Under `RC_BITRATE_MODE`, OpenH264 requires at least one GOM row group per
/// slice (2 MB rows when `mb_width <= 30`, 4 MB rows when `mb_width > 30`) and
/// fails with `cmInitParaError` if `SM_FIXEDSLCNUM_SLICE` is requested on a
/// frame too short to fit at least 2 GOMs. For normal desktop resolutions
/// (>= 320x256 / 1280x720 / 1920x1080) this uses `SM_FIXEDSLCNUM_SLICE` with
/// the full `requested_threads` count.
fn compute_slice_layout(width: u32, height: u32, requested_threads: u16) -> (c_int, u32, u16) {
    let mb_w = width.div_ceil(16);
    let mb_h = height.div_ceil(16);
    let mb_total = mb_w.saturating_mul(mb_h);
    let gom_rows = if mb_w <= 30 { 2 } else { 4 };
    let max_gom_slices = mb_h / gom_rows;

    if mb_total <= OPENH264_MIN_MB_FOR_MULTI_SLICE || max_gom_slices < 2 {
        (SM_SINGLE_SLICE, 1, 1)
    } else {
        let slices = (requested_threads as u32).min(max_gom_slices).max(2);
        (SM_FIXEDSLCNUM_SLICE, slices, slices as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits an Annex-B byte stream into H.264 NAL unit types (`nal_byte & 0x1F`).
    fn parse_h264_nal_types(annexb: &[u8]) -> Vec<u8> {
        let mut nal_types = Vec::new();
        let mut i = 0;
        while i + 3 < annexb.len() {
            let start_code_len = if i + 4 <= annexb.len()
                && annexb[i] == 0
                && annexb[i + 1] == 0
                && annexb[i + 2] == 0
                && annexb[i + 3] == 1
            {
                4
            } else if annexb[i] == 0 && annexb[i + 1] == 0 && annexb[i + 2] == 1 {
                3
            } else {
                i += 1;
                continue;
            };
            let hdr_idx = i + start_code_len;
            if hdr_idx < annexb.len() {
                nal_types.push(annexb[hdr_idx] & 0x1F);
            }
            i = hdr_idx + 1;
        }
        nal_types
    }

    /// Generates a realistic desktop-like I420 frame (solid background panels
    /// plus a moving highlight bar driven by `seed`) so screen-content RC does
    /// not blow its VBV buffer on synthetic high-frequency noise.
    fn make_i420_pattern(width: u32, height: u32, seed: u8) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let w = width as usize;
        let h = height as usize;
        let mut y = vec![32u8; w * h];
        let bar_y = ((seed as usize * 37) % h.saturating_sub(16).max(1)).min(h.saturating_sub(1));
        let bar_h = 32.min(h - bar_y);
        for row in 0..h {
            let luma = if row >= bar_y && row < bar_y + bar_h {
                220u8
            } else if row < h / 2 {
                48u8
            } else {
                80u8
            };
            y[row * w..(row + 1) * w].fill(luma);
        }
        let cw = w / 2;
        let ch = h / 2;
        let u = vec![128u8.wrapping_add(seed.wrapping_mul(3)); cw * ch];
        let v = vec![128u8.wrapping_sub(seed.wrapping_mul(5)); cw * ch];
        (y, u, v)
    }

    #[test]
    fn encodes_multi_slice_idr_and_p_frames() {
        let (w, h) = (1280u32, 720u32);
        let mut enc = MtOpenH264::new(w, h, 60, 8_000_000).expect("init MtOpenH264");
        assert!(enc.threads() >= 2, "expected >= 2 threads, got {}", enc.threads());

        let (y0, u0, v0) = make_i420_pattern(w, h, 1);
        let idr = enc
            .encode_i420(&y0, &u0, &v0, 0, true)
            .expect("encode IDR");
        assert!(idr.len() > 32, "IDR too small: {} bytes", idr.len());
        assert_eq!(&idr[..4], &[0, 0, 0, 1], "must start with Annex-B start code");

        let idr_nals = parse_h264_nal_types(&idr);
        assert!(idr_nals.contains(&7), "IDR missing SPS (7): {idr_nals:?}");
        assert!(idr_nals.contains(&8), "IDR missing PPS (8): {idr_nals:?}");
        let idr_slice_count = idr_nals.iter().filter(|&&t| t == 5).count();
        assert!(
            idr_slice_count >= 2,
            "expected multi-slice IDR (>= 2 type-5 NALs), got {idr_slice_count}: {idr_nals:?}"
        );

        // Next frame without force_idr should produce non-IDR (P-frame) slices.
        let (y1, u1, v1) = make_i420_pattern(w, h, 2);
        let p_frame = enc
            .encode_i420(&y1, &u1, &v1, 16, false)
            .expect("encode P-frame");
        assert!(!p_frame.is_empty(), "P-frame should not be empty");
        let p_nals = parse_h264_nal_types(&p_frame);
        let p_slice_count = p_nals.iter().filter(|&&t| t == 1).count();
        assert!(
            p_slice_count >= 2,
            "expected multi-slice P-frame (>= 2 type-1 NALs), got {p_slice_count}: {p_nals:?}"
        );
        assert!(
            !p_nals.contains(&5),
            "P-frame unexpectedly contained IDR slice: {p_nals:?}"
        );

        // On-demand IDR via force_idr = true.
        let (y2, u2, v2) = make_i420_pattern(w, h, 3);
        let idr2 = enc
            .encode_i420(&y2, &u2, &v2, 33, true)
            .expect("encode forced IDR");
        let idr2_nals = parse_h264_nal_types(&idr2);
        assert!(
            idr2_nals.iter().filter(|&&t| t == 5).count() >= 2,
            "forced IDR should emit multi-slice IDR NALs: {idr2_nals:?}"
        );
    }

    #[test]
    fn skipped_frame_returns_empty_vec() {
        // Tiny bitrate with high-frequency noise forces OpenH264's VBV to skip
        // a subsequent frame (`videoFrameTypeSkip`), which must map to `Ok(vec![])`.
        let (w, h) = (640u32, 360u32);
        let mut enc = MtOpenH264::new(w, h, 60, 100_000).expect("init low-bitrate");
        let mut y = vec![0u8; (w * h) as usize];
        let u = vec![128u8; (w * h / 4) as usize];
        let v = vec![128u8; (w * h / 4) as usize];
        let mut saw_skip = false;
        for idx in 0..10u32 {
            for (i, px) in y.iter_mut().enumerate() {
                *px = ((i.wrapping_mul(31) ^ (idx as usize * 97)) & 0xFF) as u8;
            }
            let bs = enc
                .encode_i420(&y, &u, &v, (idx * 16) as i64, idx == 0)
                .expect("encode frame");
            if idx > 0 && bs.is_empty() {
                saw_skip = true;
                break;
            }
        }
        assert!(saw_skip, "expected at least one VBV-skipped frame");
    }

    #[test]
    fn set_bitrate_updates_live_up_and_down() {
        let (w, h) = (640u32, 360u32);
        let mut enc = MtOpenH264::new(w, h, 30, 2_000_000).expect("init MtOpenH264");
        let (y, u, v) = make_i420_pattern(w, h, 4);

        let f0 = enc.encode_i420(&y, &u, &v, 0, true).expect("IDR");
        assert!(!f0.is_empty());

        assert!(enc.set_bitrate(4_000_000).expect("raise bitrate"));
        let (y2, u2, v2) = make_i420_pattern(w, h, 5);
        let f1 = enc.encode_i420(&y2, &u2, &v2, 33, false).expect("P after raise");
        assert!(!f1.is_empty());

        assert!(enc.set_bitrate(500_000).expect("lower bitrate"));
        let (y3, u3, v3) = make_i420_pattern(w, h, 6);
        let f2 = enc.encode_i420(&y3, &u3, &v3, 66, false).expect("P after lower");
        assert!(!f2.is_empty());

        // Raise above typical 360p level cap to verify MAX_BITRATE bump path.
        assert!(enc.set_bitrate(20_000_000).expect("raise bitrate high"));
        let (y4, u4, v4) = make_i420_pattern(w, h, 7);
        let f3 = enc.encode_i420(&y4, &u4, &v4, 100, false).expect("P after high raise");
        assert!(!f3.is_empty());

        assert!(enc.set_bitrate(0).is_err(), "zero bitrate must error");
    }

    #[test]
    fn handles_small_resolution_fallback_cleanly() {
        let (w, h) = (64u32, 64u32);
        let mut enc = MtOpenH264::new(w, h, 30, 1_000_000).expect("init 64x64");
        assert_eq!(enc.threads(), 1);
        let (y, u, v) = make_i420_pattern(w, h, 9);
        let idr = enc.encode_i420(&y, &u, &v, 0, true).expect("encode 64x64 IDR");
        let nals = parse_h264_nal_types(&idr);
        assert!(nals.contains(&7) && nals.contains(&8) && nals.contains(&5), "{nals:?}");
    }

    #[test]
    fn rejects_invalid_inputs_safely() {
        assert!(MtOpenH264::new(0, 720, 60, 4_000_000).is_err());
        assert!(MtOpenH264::new(641, 360, 60, 4_000_000).is_err());
        assert!(MtOpenH264::new(640, 361, 60, 4_000_000).is_err());
        assert!(MtOpenH264::new(4096, 2160, 60, 4_000_000).is_err());
        assert!(MtOpenH264::new(640, 360, 60, 0).is_err());

        let mut enc = MtOpenH264::new(64, 64, 30, 1_000_000).unwrap();
        assert!(enc.encode_i420(&[0u8; 10], &[0u8; 1024], &[0u8; 1024], 0, true).is_err());
        assert!(enc.encode_i420(&[0u8; 4096], &[0u8; 10], &[0u8; 1024], 0, true).is_err());
        assert!(enc.encode_i420(&[0u8; 4096], &[0u8; 1024], &[0u8; 10], 0, true).is_err());
    }

    #[test]
    fn mt_openh264_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<MtOpenH264>();
    }
}
