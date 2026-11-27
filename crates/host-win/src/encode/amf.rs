//! Native AMD AMF DX11 hardware video encoder (`amfrt64.dll`) for HEVC and H.264.
//!
//! Uses pure FFI bindings in [`super::amf_ffi`] (zero AMF SDK or C/C++ build dependencies).
//! Input path (all on the capture `ID3D11Device` with multithread protection enabled):
//! - Primary: `ID3D11VideoProcessor` converts capture BGRA8 -> NV12 (BT.709 limited range)
//!   and scales to the target encode resolution into a 4-texture ring (`DXGI_FORMAT_NV12`,
//!   `RENDER_TARGET | SHADER_RESOURCE`, no keyed mutex), then wraps via
//!   `AMFContext::CreateSurfaceFromDX11Native`.
//! - Fallback (same-size only): if the video processor or NV12 surface wrapping fails and
//!   `sample.width == cfg.width && sample.height == cfg.height`, copies into a 4-texture
//!   BGRA ring (`SHADER_RESOURCE | RENDER_TARGET`) and feeds `AMF_SURFACE_BGRA`.

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::mem::ManuallyDrop;

#[cfg(windows)]
use windows::core::Interface;
#[cfg(windows)]
use windows::Win32::Foundation::RECT;
#[cfg(windows)]
use windows::Win32::Graphics::Direct3D11::*;
#[cfg(windows)]
use windows::Win32::Graphics::Dxgi::Common::*;

#[cfg(windows)]
use super::amf_ffi::*;
#[cfg(windows)]
use super::EncoderConfig;
#[cfg(windows)]
use crate::capture::CapturedFrame;
#[cfg(windows)]
use crate::{HostError, Result};

/// Hardware codec handled by [`AmfEncoder`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmfCodec {
    Hevc,
    H264,
}

impl AmfCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            AmfCodec::Hevc => "hevc",
            AmfCodec::H264 => "h264",
        }
    }
}

/// Computes a tight, low-latency VBV buffer size (in bits) that still leaves
/// enough headroom for crisp IDR keyframes without QP=51 overshoot.
#[cfg_attr(not(windows), allow(dead_code))]
fn vbv_buffer_bits(bps: u32, fps: u32) -> i64 {
    let per_sec = (bps as i64).max(200_000);
    let per_frame = per_sec / (fps.max(1) as i64);
    (per_sec / 4).max(per_frame * 4).max(512_000)
}

/// Periodic GOP length in frames (long enough to avoid periodic IDR spikes while
/// staying well within every AMD driver's accepted property range).
#[cfg_attr(not(windows), allow(dead_code))]
fn gop_frames_for_fps(fps: u32) -> i64 {
    (fps.max(1) as i64 * 10).clamp(60, 1200)
}

/// Caps a single HEVC Access Unit size (in bits) to 1.5x the average per-frame
/// budget so scene cuts cannot burst past the USB AOA receive ring and cause
/// decode/1% low spikes.
#[cfg_attr(not(windows), allow(dead_code))]
fn max_au_size_bits(bps: u32, fps: u32) -> i64 {
    let per_frame = (bps as i64).max(200_000) / (fps.max(1) as i64);
    (per_frame * 3 / 2).clamp(256_000, 100_000_000)
}

/// Extracts the first HEVC SPS NAL unit (type 33, up to 64 bytes) from an
/// Annex-B bitstream for one-time startup diagnostic logging.
#[cfg_attr(not(windows), allow(dead_code))]
fn extract_hevc_sps_hex(annex_b: &[u8]) -> Option<String> {
    let mut i = 0usize;
    let n = annex_b.len();
    while i + 4 < n {
        let sc = if annex_b[i] == 0 && annex_b[i + 1] == 0 && annex_b[i + 2] == 1 {
            3
        } else if i + 4 < n
            && annex_b[i] == 0
            && annex_b[i + 1] == 0
            && annex_b[i + 2] == 0
            && annex_b[i + 3] == 1
        {
            4
        } else {
            i += 1;
            continue;
        };
        let nal_start = i + sc;
        if nal_start >= n {
            break;
        }
        let nal_type = (annex_b[nal_start] >> 1) & 0x3F;
        if nal_type == 33 {
            let mut end = nal_start + 1;
            while end + 3 <= n {
                if annex_b[end] == 0
                    && annex_b[end + 1] == 0
                    && (annex_b[end + 2] == 1
                        || (end + 3 < n && annex_b[end + 2] == 0 && annex_b[end + 3] == 1))
                {
                    break;
                }
                end += 1;
            }
            let slice = &annex_b[nal_start..end.min(nal_start + 64)];
            let mut hex = String::with_capacity(slice.len() * 2);
            for b in slice {
                use std::fmt::Write;
                let _ = write!(&mut hex, "{b:02x}");
            }
            return Some(hex);
        }
        i = nal_start + 1;
    }
    None
}

#[cfg(windows)]
const RING_LEN: usize = 4;
#[cfg(windows)]
const QUERY_TIMEOUT_MS: i64 = 4;

#[cfg(windows)]
fn log(level: crate::control::Level, msg: &str) {
    crate::control::log(level, msg);
}

#[cfg(windows)]
fn d3d_err(ctx: &str, e: windows::core::Error) -> HostError {
    HostError::Encoder(format!("amf: {ctx}: {e}"))
}

#[cfg(windows)]
fn amf_err(ctx: &str, res: AmfResult) -> HostError {
    HostError::Encoder(format!("amf: {ctx}: {} ({res})", result_name(res)))
}

// --------------------------------------------------------------------------
// RAII wrappers for AMF COM-style interfaces
// --------------------------------------------------------------------------

#[cfg(windows)]
struct AmfContextHandle(*mut AMFContext);

#[cfg(windows)]
impl AmfContextHandle {
    fn create(factory: *mut AMFFactory) -> Result<Self> {
        unsafe {
            let mut ctx: *mut AMFContext = std::ptr::null_mut();
            let res = ((*(*factory).pVtbl).CreateContext)(factory, &mut ctx);
            if res != AMF_OK || ctx.is_null() {
                return Err(amf_err("Factory::CreateContext", res));
            }
            Ok(Self(ctx))
        }
    }

    #[inline]
    fn as_ptr(&self) -> *mut AMFContext {
        self.0
    }

    fn init_dx11(&self, dev_raw: *mut c_void, version: AMF_DX_VERSION) -> AmfResult {
        unsafe { ((*(*self.0).pVtbl).InitDX11)(self.0, dev_raw, version) }
    }

    fn create_surface_from_dx11_native(&self, tex_raw: *mut c_void) -> Result<AmfSurfaceHandle> {
        unsafe {
            let mut surf: *mut AMFSurface = std::ptr::null_mut();
            let res = ((*(*self.0).pVtbl).CreateSurfaceFromDX11Native)(
                self.0,
                tex_raw,
                &mut surf,
                std::ptr::null_mut(),
            );
            if res != AMF_OK || surf.is_null() {
                return Err(amf_err("Context::CreateSurfaceFromDX11Native", res));
            }
            Ok(AmfSurfaceHandle(surf))
        }
    }
}

#[cfg(windows)]
impl Drop for AmfContextHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = ((*(*self.0).pVtbl).Terminate)(self.0);
                let _ = ((*(*self.0).pVtbl).Release)(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
struct AmfComponentHandle(*mut AMFComponent);

#[cfg(windows)]
impl AmfComponentHandle {
    fn create(factory: *mut AMFFactory, ctx: &AmfContextHandle, component_id: &str) -> Result<Self> {
        unsafe {
            let id = wide_nul(component_id);
            let mut comp: *mut AMFComponent = std::ptr::null_mut();
            let res = ((*(*factory).pVtbl).CreateComponent)(factory, ctx.as_ptr(), id.as_ptr(), &mut comp);
            if res != AMF_OK || comp.is_null() {
                return Err(amf_err(&format!("Factory::CreateComponent({component_id})"), res));
            }
            Ok(Self(comp))
        }
    }

    fn set_property(&self, name: &str, value: AMFVariantStruct) -> AmfResult {
        unsafe {
            let w = wide_nul(name);
            ((*(*self.0).pVtbl).SetProperty)(self.0, w.as_ptr(), value)
        }
    }

    fn init(&self, format: AMF_SURFACE_FORMAT, width: i32, height: i32) -> Result<()> {
        unsafe {
            let res = ((*(*self.0).pVtbl).Init)(self.0, format, width, height);
            if res != AMF_OK {
                return Err(amf_err("Component::Init", res));
            }
            Ok(())
        }
    }

    fn submit_input(&self, data: *mut AMFData) -> AmfResult {
        unsafe { ((*(*self.0).pVtbl).SubmitInput)(self.0, data) }
    }

    fn query_output(&self) -> (AmfResult, Option<AmfDataHandle>) {
        unsafe {
            let mut data: *mut AMFData = std::ptr::null_mut();
            let res = ((*(*self.0).pVtbl).QueryOutput)(self.0, &mut data);
            if res == AMF_OK && !data.is_null() {
                (res, Some(AmfDataHandle(data)))
            } else {
                (res, None)
            }
        }
    }
}

#[cfg(windows)]
impl Drop for AmfComponentHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = ((*(*self.0).pVtbl).Terminate)(self.0);
                let _ = ((*(*self.0).pVtbl).Release)(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
struct AmfSurfaceHandle(*mut AMFSurface);

#[cfg(windows)]
impl AmfSurfaceHandle {
    #[inline]
    fn as_data_ptr(&self) -> *mut AMFData {
        self.0 as *mut AMFData
    }

    fn set_pts(&self, pts: amf_pts) {
        unsafe {
            ((*(*self.0).pVtbl).SetPts)(self.0, pts);
        }
    }

    fn set_duration(&self, duration: amf_pts) {
        unsafe {
            ((*(*self.0).pVtbl).SetDuration)(self.0, duration);
        }
    }

    fn set_property(&self, name: &str, value: AMFVariantStruct) -> AmfResult {
        unsafe {
            let w = wide_nul(name);
            ((*(*self.0).pVtbl).SetProperty)(self.0, w.as_ptr(), value)
        }
    }

    fn set_crop(&self, x: i32, y: i32, w: i32, h: i32) -> AmfResult {
        unsafe { ((*(*self.0).pVtbl).SetCrop)(self.0, x, y, w, h) }
    }
}

#[cfg(windows)]
impl Drop for AmfSurfaceHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = ((*(*self.0).pVtbl).Release)(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
struct AmfDataHandle(*mut AMFData);

#[cfg(windows)]
impl AmfDataHandle {
    fn query_buffer(&self) -> Result<AmfBufferHandle> {
        unsafe {
            let mut raw: *mut c_void = std::ptr::null_mut();
            let res = ((*(*self.0).pVtbl).QueryInterface)(self.0, &IID_AMF_BUFFER, &mut raw);
            if res != AMF_OK || raw.is_null() {
                return Err(amf_err("Data::QueryInterface(AMFBuffer)", res));
            }
            Ok(AmfBufferHandle(raw as *mut AMFBuffer))
        }
    }
}

#[cfg(windows)]
impl Drop for AmfDataHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = ((*(*self.0).pVtbl).Release)(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(windows)]
struct AmfBufferHandle(*mut AMFBuffer);

#[cfg(windows)]
impl AmfBufferHandle {
    fn copy_bytes(&self) -> Vec<u8> {
        unsafe {
            let len = ((*(*self.0).pVtbl).GetSize)(self.0);
            if len == 0 {
                return Vec::new();
            }
            let ptr = ((*(*self.0).pVtbl).GetNative)(self.0);
            if ptr.is_null() {
                return Vec::new();
            }
            std::slice::from_raw_parts(ptr as *const u8, len).to_vec()
        }
    }
}

#[cfg(windows)]
impl Drop for AmfBufferHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                let _ = ((*(*self.0).pVtbl).Release)(self.0);
                self.0 = std::ptr::null_mut();
            }
        }
    }
}

// --------------------------------------------------------------------------
// GPU input conversion / ring buffers (DX11 NV12 VideoProcessor & BGRA ring)
// --------------------------------------------------------------------------

#[cfg(windows)]
struct D3dNv12Ring {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    venum: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    ring: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    in_size: (u32, u32),
    out_size: (u32, u32),
    tex_size: (u32, u32),
    in_views: Vec<(*mut c_void, ID3D11VideoProcessorInputView)>,
}

#[cfg(windows)]
impl D3dNv12Ring {
    fn new(
        device: &ID3D11Device,
        in_size: (u32, u32),
        out_size: (u32, u32),
        fps: u32,
    ) -> Result<Self> {
        // VCN 4 hardware aligns NV12 surfaces to 64x16 (e.g. 2800x1752 -> 2816x1760).
        // Pre-allocating aligned textures + SetCrop avoids an internal AMF staging copy.
        let tex_size = ((out_size.0 + 63) & !63, (out_size.1 + 15) & !15);
        unsafe {
            let ctx = device
                .GetImmediateContext()
                .map_err(|e| d3d_err("GetImmediateContext", e))?;
            let vdev: ID3D11VideoDevice = device
                .cast()
                .map_err(|e| d3d_err("ID3D11VideoDevice", e))?;
            let vctx: ID3D11VideoContext = ctx
                .cast()
                .map_err(|e| d3d_err("ID3D11VideoContext", e))?;
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 },
                InputWidth: in_size.0,
                InputHeight: in_size.1,
                OutputFrameRate: DXGI_RATIONAL { Numerator: fps.max(1), Denominator: 1 },
                OutputWidth: tex_size.0,
                OutputHeight: tex_size.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let venum = vdev
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(|e| d3d_err("CreateVideoProcessorEnumerator", e))?;
            let vp = vdev
                .CreateVideoProcessor(&venum, 0)
                .map_err(|e| d3d_err("CreateVideoProcessor", e))?;

            // Input: full-range RGB (0..255). Output: BT.709 limited range (16..235).
            let in_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let out_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) };
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &in_cs);
            vctx.VideoProcessorSetOutputColorSpace(&vp, &out_cs);
            let dst_rect = RECT {
                left: 0,
                top: 0,
                right: out_size.0 as i32,
                bottom: out_size.1 as i32,
            };
            vctx.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&dst_rect));
            vctx.VideoProcessorSetOutputTargetRect(&vp, true, Some(&dst_rect));
            vctx.VideoProcessorSetStreamFrameFormat(
                &vp,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );

            let mut ring = Vec::with_capacity(RING_LEN);
            let mut with_srv = true;
            for _ in 0..RING_LEN {
                let mut td = D3D11_TEXTURE2D_DESC {
                    Width: tex_size.0,
                    Height: tex_size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                if with_srv {
                    if let Err(e) = device.CreateTexture2D(&td, None, Some(&mut tex)) {
                        log(
                            crate::control::Level::Warn,
                            &format!(
                                "amf: NV12 texture with RENDER_TARGET|SHADER_RESOURCE failed ({e}); using RENDER_TARGET only"
                            ),
                        );
                        with_srv = false;
                        tex = None;
                    }
                }
                if !with_srv {
                    td.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
                    device
                        .CreateTexture2D(&td, None, Some(&mut tex))
                        .map_err(|e| d3d_err("NV12 CreateTexture2D", e))?;
                }
                let tex = tex.ok_or_else(|| HostError::Encoder("amf: NV12 texture null".into()))?;
                let vd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut view = None;
                vdev.CreateVideoProcessorOutputView(&tex, &venum, &vd, Some(&mut view))
                    .map_err(|e| d3d_err("CreateVideoProcessorOutputView", e))?;
                let view =
                    view.ok_or_else(|| HostError::Encoder("amf: VP output view null".into()))?;
                ring.push((tex, view));
            }

            Ok(Self {
                device: device.clone(),
                ctx,
                vdev,
                vctx,
                venum,
                vp,
                ring,
                next: 0,
                in_size,
                out_size,
                tex_size,
                in_views: Vec::with_capacity(4),
            })
        }
    }

    fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        unsafe {
            let raw = src.as_raw();
            let view_idx = match self.in_views.iter().position(|(p, _)| *p == raw) {
                Some(i) => i,
                None => {
                    let vd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                        FourCC: 0,
                        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                            Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                        },
                    };
                    let mut view = None;
                    self.vdev
                        .CreateVideoProcessorInputView(src, &self.venum, &vd, Some(&mut view))
                        .map_err(|e| d3d_err("CreateVideoProcessorInputView", e))?;
                    let view =
                        view.ok_or_else(|| HostError::Encoder("amf: VP input view null".into()))?;
                    if self.in_views.len() >= 4 {
                        self.in_views.remove(0);
                    }
                    self.in_views.push((raw, view));
                    self.in_views.len() - 1
                }
            };
            let (_, in_view) = &self.in_views[view_idx];
            let (tex, out_view) = &self.ring[self.next];
            self.next = (self.next + 1) % self.ring.len();

            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(Some(in_view.clone())),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let res = self
                .vctx
                .VideoProcessorBlt(&self.vp, out_view, 0, std::slice::from_ref(&stream));
            drop(ManuallyDrop::into_inner(stream.pInputSurface));
            res.map_err(|e| d3d_err("VideoProcessorBlt", e))?;

            self.ctx.Flush();
            Ok(tex.clone())
        }
    }
}

#[cfg(windows)]
struct D3dBgraRing {
    ctx: ID3D11DeviceContext,
    ring: Vec<ID3D11Texture2D>,
    next: usize,
    size: (u32, u32),
}

#[cfg(windows)]
impl D3dBgraRing {
    fn new(device: &ID3D11Device, size: (u32, u32)) -> Result<Self> {
        unsafe {
            let ctx = device
                .GetImmediateContext()
                .map_err(|e| d3d_err("GetImmediateContext", e))?;
            let td = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut ring = Vec::with_capacity(RING_LEN);
            for _ in 0..RING_LEN {
                let mut tex = None;
                device
                    .CreateTexture2D(&td, None, Some(&mut tex))
                    .map_err(|e| d3d_err("BGRA CreateTexture2D", e))?;
                ring.push(tex.ok_or_else(|| HostError::Encoder("amf: BGRA texture null".into()))?);
            }
            Ok(Self { ctx, ring, next: 0, size })
        }
    }

    fn copy_from(&mut self, src: &ID3D11Texture2D) -> ID3D11Texture2D {
        unsafe {
            let dst = &self.ring[self.next];
            self.next = (self.next + 1) % self.ring.len();
            self.ctx.CopyResource(dst, src);
            self.ctx.Flush();
            dst.clone()
        }
    }
}

#[cfg(windows)]
enum InputPath {
    Nv12(D3dNv12Ring),
    Bgra(D3dBgraRing),
}

#[cfg(windows)]
impl InputPath {
    fn format_label(&self) -> &'static str {
        match self {
            InputPath::Nv12(_) => "nv12",
            InputPath::Bgra(_) => "bgra",
        }
    }

    fn needs_crop(&self) -> bool {
        match self {
            InputPath::Nv12(vp) => vp.tex_size != vp.out_size,
            InputPath::Bgra(_) => false,
        }
    }

    fn prepare(
        &mut self,
        src: &ID3D11Texture2D,
        in_w: u32,
        in_h: u32,
        fps: u32,
    ) -> Result<ID3D11Texture2D> {
        match self {
            InputPath::Nv12(vp) => {
                if vp.in_size != (in_w, in_h) {
                    *vp = D3dNv12Ring::new(&vp.device, (in_w, in_h), vp.out_size, fps)?;
                }
                vp.convert(src)
            }
            InputPath::Bgra(bgra) => {
                if bgra.size != (in_w, in_h) {
                    return Err(HostError::Encoder(format!(
                        "amf: BGRA input size changed from {}x{} to {}x{}",
                        bgra.size.0, bgra.size.1, in_w, in_h
                    )));
                }
                Ok(bgra.copy_from(src))
            }
        }
    }
}

// --------------------------------------------------------------------------
// Native AMD AMF DX11 Hardware Encoder
// --------------------------------------------------------------------------

/// Native AMD AMF DX11 hardware video encoder (`amfrt64.dll`).
///
/// Field declaration order enforces strict reverse teardown on drop:
/// 1. `encoder` (`AMFComponent::Terminate` + `Release`)
/// 2. `context` (`AMFContext::Terminate` + `Release`)
/// 3. `input` (D3D11 ring textures & video processor)
/// 4. `_device` (`ID3D11Device`)
#[cfg(windows)]
pub struct AmfEncoder {
    encoder: AmfComponentHandle,
    context: AmfContextHandle,
    input: InputPath,
    _device: ID3D11Device,
    codec: AmfCodec,
    cfg: EncoderConfig,
    runtime_version: String,
    frame_no: u64,
    logged_sps: bool,
}

#[cfg(windows)]
// SAFETY: `AmfEncoder` is owned exclusively by the single video thread (never shared
// concurrently across threads), its `ID3D11Device` has `SetMultithreadProtected(true)`
// enabled, and AMF objects are thread-safe to move between threads.
unsafe impl Send for AmfEncoder {}

#[cfg(windows)]
impl AmfEncoder {
    /// Initializes a native AMD AMF DX11 hardware encoder (`Hevc` or `H264`) sharing the
    /// D3D11 device from `sample`.
    pub fn new(codec: AmfCodec, mut cfg: EncoderConfig, sample: &CapturedFrame) -> Result<Self> {
        cfg.width &= !1;
        cfg.height &= !1;
        if cfg.width < 16 || cfg.height < 16 {
            return Err(HostError::Encoder("amf: encoder size must be >= 16x16".into()));
        }

        let lib = AmfLibrary::global().map_err(HostError::Encoder)?;
        let sample_tex = sample
            .texture()
            .ok_or_else(|| HostError::Encoder("amf: requires a D3D11 GPU frame".into()))?;

        let device: ID3D11Device = unsafe {
            sample_tex
                .GetDevice()
                .map_err(|e| d3d_err("ID3D11Texture2D::GetDevice", e))?
        };
        unsafe {
            if let Ok(mt) = device.cast::<ID3D11Multithread>() {
                let _ = mt.SetMultithreadProtected(true);
            }
        }

        let context = AmfContextHandle::create(lib.factory())?;
        let mut init_res = context.init_dx11(device.as_raw(), AMF_DX11_1);
        if init_res != AMF_OK {
            log(
                crate::control::Level::Warn,
                &format!(
                    "amf: InitDX11(DX11_1) returned {} ({init_res}); retrying DX11_0",
                    result_name(init_res)
                ),
            );
            init_res = context.init_dx11(device.as_raw(), AMF_DX11_0);
        }
        if init_res != AMF_OK {
            return Err(amf_err("Context::InitDX11", init_res));
        }

        // Build and validate the GPU input surface path before initializing the encoder component.
        let (input, surface_fmt) =
            Self::setup_input_path(&context, &device, sample_tex, (sample.width, sample.height), &cfg)?;

        let comp_id = match codec {
            AmfCodec::Hevc => AMF_COMPONENT_HEVC,
            AmfCodec::H264 => AMF_COMPONENT_AVC,
        };
        let encoder = AmfComponentHandle::create(lib.factory(), &context, comp_id)?;

        Self::configure_component(&encoder, codec, &cfg, surface_fmt)?;
        encoder.init(surface_fmt, cfg.width as i32, cfg.height as i32)?;

        let enc = Self {
            encoder,
            context,
            input,
            _device: device,
            codec,
            cfg,
            runtime_version: lib.version_string(),
            frame_no: 0,
            logged_sps: false,
        };
        log(
            crate::control::Level::Info,
            &format!("amf: initialized {} ({})", codec.as_str(), enc.describe()),
        );
        Ok(enc)
    }

    fn setup_input_path(
        context: &AmfContextHandle,
        device: &ID3D11Device,
        sample_tex: &ID3D11Texture2D,
        in_size: (u32, u32),
        cfg: &EncoderConfig,
    ) -> Result<(InputPath, AMF_SURFACE_FORMAT)> {
        let out_size = (cfg.width, cfg.height);
        let nv12_attempt = (|| -> Result<D3dNv12Ring> {
            let mut ring = D3dNv12Ring::new(device, in_size, out_size, cfg.fps)?;
            let first = ring.convert(sample_tex)?;
            let test_surf = context.create_surface_from_dx11_native(first.as_raw())?;
            drop(test_surf);
            Ok(ring)
        })();

        match nv12_attempt {
            Ok(ring) => return Ok((InputPath::Nv12(ring), AMF_SURFACE_NV12)),
            Err(nv12_err) => {
                if in_size != out_size {
                    return Err(nv12_err);
                }
                log(
                    crate::control::Level::Warn,
                    &format!("amf: NV12 VideoProcessor path failed ({nv12_err}); trying BGRA fallback"),
                );
                let mut bgra = D3dBgraRing::new(device, out_size)?;
                let first = bgra.copy_from(sample_tex);
                let test_surf = context.create_surface_from_dx11_native(first.as_raw())?;
                drop(test_surf);
                Ok((InputPath::Bgra(bgra), AMF_SURFACE_BGRA))
            }
        }
    }

    fn configure_component(
        encoder: &AmfComponentHandle,
        codec: AmfCodec,
        cfg: &EncoderConfig,
        surface_fmt: AMF_SURFACE_FORMAT,
    ) -> Result<()> {
        let fps = cfg.fps.max(1);
        let target_bps = (cfg.bitrate_bps as i64).max(100_000);
        let peak_bps = target_bps;
        let vbv_bits = vbv_buffer_bits(cfg.bitrate_bps, fps);
        let max_au_bits = max_au_size_bits(cfg.bitrate_bps, fps);
        let gop = gop_frames_for_fps(fps);
        let (w, h) = (cfg.width as i32, cfg.height as i32);

        let set_req = |name: &str, val: AMFVariantStruct| -> Result<()> {
            let res = encoder.set_property(name, val);
            if res != AMF_OK {
                return Err(amf_err(&format!("SetProperty({name})"), res));
            }
            Ok(())
        };
        let set_opt = |name: &str, val: AMFVariantStruct| {
            let res = encoder.set_property(name, val);
            if res != AMF_OK {
                log(
                    crate::control::Level::Warn,
                    &format!("amf: optional SetProperty({name}) returned {} ({res})", result_name(res)),
                );
            }
        };

        match codec {
            AmfCodec::Hevc => {
                // CRITICAL: HevcUsage MUST be set FIRST because AMF resets dependent presets on Usage change.
                if encoder.set_property(HEVC_USAGE, AMFVariantStruct::from_i64(HEVC_USAGE_ULTRA_LOW_LATENCY))
                    != AMF_OK
                {
                    log(
                        crate::control::Level::Warn,
                        "amf: HevcUsage ULTRA_LOW_LATENCY rejected; falling back to LOW_LATENCY",
                    );
                    set_req(HEVC_USAGE, AMFVariantStruct::from_i64(HEVC_USAGE_LOW_LATENCY))?;
                }
                set_opt(HEVC_LOW_LATENCY_MODE, AMFVariantStruct::from_bool(true));
                set_opt(HEVC_QUALITY_PRESET, AMFVariantStruct::from_i64(HEVC_QUALITY_PRESET_SPEED));
                set_req(HEVC_RATE_CONTROL_METHOD, AMFVariantStruct::from_i64(HEVC_RC_CBR))?;
                set_req(HEVC_TARGET_BITRATE, AMFVariantStruct::from_i64(target_bps))?;
                set_opt(HEVC_PEAK_BITRATE, AMFVariantStruct::from_i64(peak_bps));
                set_opt(HEVC_VBV_BUFFER_SIZE, AMFVariantStruct::from_i64(vbv_bits));
                set_opt(HEVC_MAX_AU_SIZE, AMFVariantStruct::from_i64(max_au_bits));
                set_req(HEVC_FRAME_RATE, AMFVariantStruct::from_rate(fps, 1))?;
                set_opt(HEVC_FRAME_SIZE, AMFVariantStruct::from_size(w, h));
                set_opt(HEVC_PROFILE, AMFVariantStruct::from_i64(HEVC_PROFILE_MAIN));
                set_opt(
                    HEVC_HEADER_INSERTION_MODE,
                    AMFVariantStruct::from_i64(HEVC_HEADER_INSERTION_IDR_ALIGNED),
                );
                set_opt(HEVC_MAX_NUM_REFRAMES, AMFVariantStruct::from_i64(1));
                set_opt(HEVC_MAX_LTR_FRAMES, AMFVariantStruct::from_i64(0));
                set_opt(HEVC_GOP_SIZE, AMFVariantStruct::from_i64(gop));
                // 0 = no periodic IDR spikes; initial IDR + client PLI handle recovery cleanly.
                set_opt(HEVC_GOPS_PER_IDR, AMFVariantStruct::from_i64(0));
                set_opt(HEVC_FILLER_DATA_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt("HevcRateControlSkipFrameEnable", AMFVariantStruct::from_bool(false));
                set_opt(HEVC_RATE_CONTROL_PREANALYSIS_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt(HEVC_PRE_ANALYSIS_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt(HEVC_HIGH_MOTION_QUALITY_BOOST_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt(HEVC_ENABLE_VBAQ, AMFVariantStruct::from_bool(false));
                set_opt(HEVC_QUERY_TIMEOUT, AMFVariantStruct::from_i64(QUERY_TIMEOUT_MS));
                if surface_fmt == AMF_SURFACE_NV12 {
                    set_opt(HEVC_NOMINAL_RANGE, AMFVariantStruct::from_i64(HEVC_NOMINAL_RANGE_STUDIO));
                }
            }
            AmfCodec::H264 => {
                // CRITICAL: Usage MUST be set FIRST because AMF resets dependent presets on Usage change.
                if encoder.set_property(AVC_USAGE, AMFVariantStruct::from_i64(AVC_USAGE_ULTRA_LOW_LATENCY))
                    != AMF_OK
                {
                    log(
                        crate::control::Level::Warn,
                        "amf: AVC Usage ULTRA_LOW_LATENCY rejected; falling back to LOW_LATENCY",
                    );
                    set_req(AVC_USAGE, AMFVariantStruct::from_i64(AVC_USAGE_LOW_LATENCY))?;
                }
                set_opt(AVC_LOW_LATENCY_MODE, AMFVariantStruct::from_bool(true));
                set_opt(AVC_QUALITY_PRESET, AMFVariantStruct::from_i64(AVC_QUALITY_PRESET_SPEED));
                set_req(AVC_RATE_CONTROL_METHOD, AMFVariantStruct::from_i64(AVC_RC_CBR))?;
                set_req(AVC_TARGET_BITRATE, AMFVariantStruct::from_i64(target_bps))?;
                set_opt(AVC_PEAK_BITRATE, AMFVariantStruct::from_i64(peak_bps));
                set_opt(AVC_VBV_BUFFER_SIZE, AMFVariantStruct::from_i64(vbv_bits));
                set_req(AVC_FRAME_RATE, AMFVariantStruct::from_rate(fps, 1))?;
                set_opt(AVC_FRAME_SIZE, AMFVariantStruct::from_size(w, h));
                set_opt(AVC_PROFILE, AMFVariantStruct::from_i64(AVC_PROFILE_HIGH));
                set_opt(AVC_MAX_CONSECUTIVE_BPICTURES, AMFVariantStruct::from_i64(0));
                set_opt(AVC_B_PICTURES_PATTERN, AMFVariantStruct::from_i64(0));
                set_opt(AVC_IDR_PERIOD, AMFVariantStruct::from_i64(gop));
                set_opt(AVC_FILLER_DATA_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt("RateControlSkipFrameEnable", AMFVariantStruct::from_bool(false));
                // Upstream VideoEncoderVCE.h defines RateControlPreanalysisEnable as amf_int64 (0=disabled);
                // fall back to bool(false) if an older runtime expects bool.
                if encoder.set_property(
                    AVC_RATE_CONTROL_PREANALYSIS_ENABLE,
                    AMFVariantStruct::from_i64(AVC_PREENCODE_DISABLED),
                ) != AMF_OK
                {
                    set_opt(AVC_RATE_CONTROL_PREANALYSIS_ENABLE, AMFVariantStruct::from_bool(false));
                }
                set_opt(AVC_PRE_ANALYSIS_ENABLE, AMFVariantStruct::from_bool(false));
                set_opt(AVC_ENABLE_VBAQ, AMFVariantStruct::from_bool(false));
                set_opt(AVC_QUERY_TIMEOUT, AMFVariantStruct::from_i64(QUERY_TIMEOUT_MS));
                set_opt(AVC_ENGINE_TYPE, AMFVariantStruct::from_i64(AMF_MEMORY_DX11 as i64));
            }
        }
        Ok(())
    }

    /// Polls `QueryOutput` up to `max_wait` when the driver returns `AMF_REPEAT`
    /// (some Radeon drivers do not block inside `QueryOutput` even when QueryTimeout is set).
    fn drain_one_output_wait(&self, max_wait: std::time::Duration) -> Result<Option<Vec<u8>>> {
        let start = std::time::Instant::now();
        let mut spins = 0u32;
        loop {
            let (res, maybe_data) = self.encoder.query_output();
            match (res, maybe_data) {
                (AMF_OK, Some(data)) => {
                    let buf = data.query_buffer()?;
                    return Ok(Some(buf.copy_bytes()));
                }
                (AMF_REPEAT, _) => {
                    if start.elapsed() >= max_wait {
                        return Ok(None);
                    }
                    spins += 1;
                    if spins <= 1 {
                        std::thread::yield_now();
                    } else {
                        std::thread::sleep(std::time::Duration::from_micros(500));
                    }
                }
                (AMF_EOF, _) | (AMF_OK, None) => return Ok(None),
                (other, _) => return Err(amf_err("Component::QueryOutput", other)),
            }
        }
    }

    /// Encodes one GPU frame into an Annex-B HEVC or H.264 bitstream.
    pub fn encode_frame(&mut self, frame: &CapturedFrame, force_idr: bool) -> Result<Vec<u8>> {
        let src_tex = frame
            .texture()
            .ok_or_else(|| HostError::Encoder("amf: frame has no D3D11 texture".into()))?;

        let ring_tex = self
            .input
            .prepare(src_tex, frame.width, frame.height, self.cfg.fps)?;
        let surface = self.context.create_surface_from_dx11_native(ring_tex.as_raw())?;
        if self.input.needs_crop() {
            let _ = surface.set_crop(0, 0, self.cfg.width as i32, self.cfg.height as i32);
        }

        let fps = self.cfg.fps.max(1) as u64;
        let pts_100ns = (self.frame_no * 10_000_000 / fps) as amf_pts;
        let dur_100ns = (10_000_000 / fps) as amf_pts;
        surface.set_pts(pts_100ns);
        surface.set_duration(dur_100ns);

        if force_idr || self.frame_no == 0 {
            match self.codec {
                AmfCodec::Hevc => {
                    let _ = surface.set_property(
                        HEVC_FORCE_PICTURE_TYPE,
                        AMFVariantStruct::from_i64(HEVC_PICTURE_TYPE_IDR),
                    );
                    let _ = surface.set_property(HEVC_INSERT_HEADER, AMFVariantStruct::from_bool(true));
                }
                AmfCodec::H264 => {
                    let _ = surface.set_property(
                        AVC_FORCE_PICTURE_TYPE,
                        AMFVariantStruct::from_i64(AVC_PICTURE_TYPE_IDR),
                    );
                    let _ = surface.set_property(AVC_INSERT_SPS, AMFVariantStruct::from_bool(true));
                    let _ = surface.set_property(AVC_INSERT_PPS, AMFVariantStruct::from_bool(true));
                }
            }
        }

        let mut drained: Option<Vec<u8>> = None;
        let mut submit_res = self.encoder.submit_input(surface.as_data_ptr());
        if submit_res == AMF_INPUT_FULL {
            drained = self.drain_one_output_wait(std::time::Duration::from_millis(10))?;
            submit_res = self.encoder.submit_input(surface.as_data_ptr());
        }
        // Release our wrapper ref immediately; AMF retains its own ref until HW encode completes.
        drop(surface);

        match submit_res {
            AMF_OK | AMF_NEED_MORE_INPUT => {
                self.frame_no += 1;
            }
            AMF_INPUT_FULL => {
                // Transient backpressure: return whatever we drained (if any) without failing the encoder.
                return Ok(if force_idr { Vec::new() } else { drained.unwrap_or_default() });
            }
            other => return Err(amf_err("Component::SubmitInput", other)),
        }

        let wait = if self.frame_no <= 3 {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_millis(25)
        };
        match self.drain_one_output_wait(wait)? {
            Some(bytes) => {
                if !self.logged_sps && self.codec == AmfCodec::Hevc {
                    if let Some(hex) = extract_hevc_sps_hex(&bytes) {
                        self.logged_sps = true;
                        log(crate::control::Level::Info, &format!("amf: hevc-sps={hex}"));
                    }
                }
                Ok(bytes)
            }
            None => Ok(if force_idr { Vec::new() } else { drained.unwrap_or_default() }),
        }
    }

    /// Dynamically updates target/peak bitrate and VBV buffer size without recreating the encoder.
    pub fn set_bitrate_live(&mut self, bps: u32) -> Result<bool> {
        let target_bps = (bps as i64).max(100_000);
        let vbv_bits = vbv_buffer_bits(bps, self.cfg.fps);
        let (k_target, k_peak, k_vbv) = match self.codec {
            AmfCodec::Hevc => (HEVC_TARGET_BITRATE, HEVC_PEAK_BITRATE, HEVC_VBV_BUFFER_SIZE),
            AmfCodec::H264 => (AVC_TARGET_BITRATE, AVC_PEAK_BITRATE, AVC_VBV_BUFFER_SIZE),
        };
        let r_target = self
            .encoder
            .set_property(k_target, AMFVariantStruct::from_i64(target_bps));
        let _ = self
            .encoder
            .set_property(k_peak, AMFVariantStruct::from_i64(target_bps));
        let _ = self
            .encoder
            .set_property(k_vbv, AMFVariantStruct::from_i64(vbv_bits));
        if self.codec == AmfCodec::Hevc {
            let max_au = max_au_size_bits(bps, self.cfg.fps);
            let _ = self
                .encoder
                .set_property(HEVC_MAX_AU_SIZE, AMFVariantStruct::from_i64(max_au));
        }

        if r_target == AMF_OK {
            self.cfg.bitrate_bps = bps;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Human-readable encoder summary for logs and diagnostics.
    pub fn describe(&self) -> String {
        format!(
            "amfrt64.dll {}, dx11 {}, {}x{}@{} {} kbps",
            self.runtime_version,
            self.input.format_label(),
            self.cfg.width,
            self.cfg.height,
            self.cfg.fps,
            self.cfg.bitrate_bps / 1000,
        )
    }

    pub fn codec(&self) -> AmfCodec {
        self.codec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vbv_and_gop_helpers_stay_bounded() {
        assert_eq!(vbv_buffer_bits(14_000_000, 120), 3_500_000);
        assert_eq!(vbv_buffer_bits(1_000_000, 60), 512_000);
        assert_eq!(max_au_size_bits(60_000_000, 120), 750_000);
        assert_eq!(max_au_size_bits(1_000_000, 120), 256_000);
        assert_eq!(gop_frames_for_fps(60), 600);
        assert_eq!(gop_frames_for_fps(120), 1200);
        assert_eq!(gop_frames_for_fps(240), 1200);
        assert_eq!(AmfCodec::Hevc.as_str(), "hevc");
        assert_eq!(AmfCodec::H264.as_str(), "h264");
    }

    #[test]
    fn extract_hevc_sps_hex_finds_nal_type_33() {
        // VPS (type 32: 0x40 0x01) + SPS (type 33: 0x42 0x01 0xaa 0xbb) + PPS (type 34: 0x44 0x01)
        let stream = [
            0, 0, 0, 1, 0x40, 0x01, 0x11,
            0, 0, 0, 1, 0x42, 0x01, 0xaa, 0xbb,
            0, 0, 1, 0x44, 0x01, 0x22,
        ];
        assert_eq!(extract_hevc_sps_hex(&stream).as_deref(), Some("4201aabb"));
        assert_eq!(extract_hevc_sps_hex(&[0, 0, 0, 1, 0x40, 0x01]), None);
    }
}
