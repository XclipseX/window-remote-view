//! Media Foundation hardware video encoder (HEVC / H.264).
//!
//! Works with any vendor's hardware encoder MFT (AMD's AMF-backed MFT, Intel
//! QSV, NVIDIA NVENC). Hardware MFTs are *asynchronous*: they raise
//! `METransformNeedInput` / `METransformHaveOutput` on `IMFMediaEventGenerator`.
//! Because the caller (video thread) already runs one frame at a time, the
//! event queue is simply pumped inside [`MfEncoder::encode`] instead of using
//! a callback object.
//!
//! Input path:
//! * GPU: capture texture (BGRA) -> `ID3D11VideoProcessor` (also scales and
//!   does BT.709 limited-range conversion) -> NV12 texture -> DXGI surface
//!   buffer -> MFT (device shared via `IMFDXGIDeviceManager`).
//! * CPU fallback (no D3D support in the MFT, or video processor failure):
//!   readback + [`bgra_to_nv12`] into a memory buffer.
//!
//! NOT VERIFIED ON REAL HARDWARE: this was written against the documented MF
//! contracts and only compile-checked (cross target x86_64-pc-windows-gnu).

use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::Foundation::RECT;

use super::{bgra_to_nv12, has_sps, scale_bgra_nearest, EncoderConfig, EncoderKind, VideoEncoder};
use crate::capture::CapturedFrame;
use crate::{HostError, Result};

const POOL: usize = 8;

fn err(ctx: &str, e: windows::core::Error) -> HostError {
    HostError::Encoder(format!("{ctx}: {e}"))
}

/// VT_UI4 VARIANT (the windows crate has no `From<u32>` for it).
#[allow(non_snake_case)]
fn VARIANT_u32(v: u32) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_UI4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { ulVal: v },
            }),
        },
    }
}

#[allow(non_snake_case)]
fn VARIANT_bool(v: bool) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_BOOL,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { boolVal: VARIANT_BOOL(if v { -1 } else { 0 }) },
            }),
        },
    }
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

// --------------------------------------------------------------------------
// GPU colour conversion / scaling
// --------------------------------------------------------------------------

struct D3dPath {
    device: ID3D11Device,
    #[allow(dead_code)]
    ctx: ID3D11DeviceContext,
    vdev: ID3D11VideoDevice,
    vctx: ID3D11VideoContext,
    venum: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    pool: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
    in_size: (u32, u32),
    out_size: (u32, u32),
    in_view: Option<(*mut std::ffi::c_void, ID3D11VideoProcessorInputView)>,
}

impl D3dPath {
    fn new(device: &ID3D11Device, in_size: (u32, u32), out_size: (u32, u32), fps: u32) -> Result<Self> {
        unsafe {
            let ctx = device.GetImmediateContext().map_err(|e| err("GetImmediateContext", e))?;
            let vdev: ID3D11VideoDevice = device.cast().map_err(|e| err("ID3D11VideoDevice", e))?;
            let vctx: ID3D11VideoContext = ctx.cast().map_err(|e| err("ID3D11VideoContext", e))?;
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                InputWidth: in_size.0,
                InputHeight: in_size.1,
                OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
                OutputWidth: out_size.0,
                OutputHeight: out_size.1,
                Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
            };
            let venum = vdev
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(|e| err("CreateVideoProcessorEnumerator", e))?;
            let vp = vdev.CreateVideoProcessor(&venum, 0).map_err(|e| err("CreateVideoProcessor", e))?;

            // Input: full-range RGB. Output: BT.709, 16-235.
            let in_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: 0 };
            let out_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: (1 << 2) | (1 << 4) };
            vctx.VideoProcessorSetStreamColorSpace(&vp, 0, &in_cs);
            vctx.VideoProcessorSetOutputColorSpace(&vp, &out_cs);
            let r = RECT { left: 0, top: 0, right: out_size.0 as i32, bottom: out_size.1 as i32 };
            vctx.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&r));
            vctx.VideoProcessorSetOutputTargetRect(&vp, true, Some(&r));
            vctx.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);

            let mut pool = Vec::with_capacity(POOL);
            for _ in 0..POOL {
                let td = D3D11_TEXTURE2D_DESC {
                    Width: out_size.0,
                    Height: out_size.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut tex = None;
                device.CreateTexture2D(&td, None, Some(&mut tex)).map_err(|e| err("NV12 CreateTexture2D", e))?;
                let tex = tex.ok_or_else(|| HostError::Encoder("NV12 texture null".into()))?;
                let vd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                    ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                    },
                };
                let mut view = None;
                vdev.CreateVideoProcessorOutputView(&tex, &venum, &vd, Some(&mut view))
                    .map_err(|e| err("CreateVideoProcessorOutputView", e))?;
                let view = view.ok_or_else(|| HostError::Encoder("output view null".into()))?;
                pool.push((tex, view));
            }
            Ok(Self {
                device: device.clone(),
                ctx,
                vdev,
                vctx,
                venum,
                vp,
                pool,
                next: 0,
                in_size,
                out_size,
                in_view: None,
            })
        }
    }

    /// BGRA texture -> next NV12 pool texture (scaled to `out_size`).
    fn convert(&mut self, src: &ID3D11Texture2D) -> Result<ID3D11Texture2D> {
        unsafe {
            let raw = src.as_raw();
            let need_view = !matches!(&self.in_view, Some((p, _)) if *p == raw);
            if need_view {
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
                    .map_err(|e| err("CreateVideoProcessorInputView", e))?;
                self.in_view = Some((raw, view.ok_or_else(|| HostError::Encoder("input view null".into()))?));
            }
            let (_, in_view) = self.in_view.as_ref().unwrap();
            let (tex, out_view) = &self.pool[self.next];
            self.next = (self.next + 1) % POOL;
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
            let r = self.vctx.VideoProcessorBlt(&self.vp, out_view, 0, &[stream.clone()]);
            drop(ManuallyDrop::into_inner(stream.pInputSurface));
            r.map_err(|e| err("VideoProcessorBlt", e))?;
            Ok(tex.clone())
        }
    }
}

// --------------------------------------------------------------------------
// encoder
// --------------------------------------------------------------------------

pub struct MfEncoder {
    kind: EncoderKind,
    cfg: EncoderConfig,
    transform: IMFTransform,
    activate: IMFActivate,
    events: Option<IMFMediaEventGenerator>,
    codec_api: Option<ICodecAPI>,
    _mgr: Option<IMFDXGIDeviceManager>,
    d3d: Option<D3dPath>,
    device: Option<ID3D11Device>,
    is_async: bool,
    need_input: u32,
    pending: VecDeque<Vec<u8>>,
    frame_no: u64,
    out_provides: bool,
    out_size: u32,
    seq_header: Vec<u8>,
    friendly: String,
    cpu_bgra: Vec<u8>,
    cpu_scaled: Vec<u8>,
    cpu_nv12: Vec<u8>,
}

impl MfEncoder {
    pub fn describe(&self) -> String {
        format!(
            "{}, {}, {}x{}@{} {} kbps",
            self.friendly,
            if self.d3d.is_some() { "gpu convert" } else { "cpu convert" },
            self.cfg.width,
            self.cfg.height,
            self.cfg.fps,
            self.cfg.bitrate_bps / 1000
        )
    }

    pub fn new(kind: EncoderKind, cfg: EncoderConfig, sample: &CapturedFrame) -> Result<Self> {
        let subtype = match kind {
            EncoderKind::MfHevcHw => MFVideoFormat_HEVC,
            EncoderKind::MfH264Hw => MFVideoFormat_H264,
            EncoderKind::OpenH264 => return Err(HostError::Encoder("not an MF kind".into())),
        };
        if cfg.width < 16 || cfg.height < 16 {
            return Err(HostError::Encoder("encoder size must be set".into()));
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| err("MFStartup", e))?;
        }
        let r = Self::init_any(kind, subtype, cfg, sample);
        if r.is_err() {
            unsafe {
                let _ = MFShutdown();
            }
        }
        r
    }

    fn init_any(kind: EncoderKind, subtype: GUID, cfg: EncoderConfig, sample: &CapturedFrame) -> Result<Self> {
        let activates = enum_hw_encoders(subtype)?;
        if activates.is_empty() {
            return Err(HostError::Encoder("no hardware MFT for this codec".into()));
        }
        let mut last = HostError::Encoder("no MFT".into());
        for act in activates {
            match Self::init_one(kind, subtype, cfg, sample, act) {
                Ok(e) => return Ok(e),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn init_one(
        kind: EncoderKind,
        subtype: GUID,
        cfg: EncoderConfig,
        sample: &CapturedFrame,
        activate: IMFActivate,
    ) -> Result<Self> {
        unsafe {
            let friendly = {
                let mut buf = [0u16; 256];
                let mut n = 0u32;
                if activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut n)).is_ok() {
                    String::from_utf16_lossy(&buf[..n as usize])
                } else {
                    "unknown MFT".into()
                }
            };
            let transform: IMFTransform = activate.ActivateObject().map_err(|e| err(&friendly, e))?;
            let mut is_async = false;
            if let Ok(attrs) = transform.GetAttributes() {
                is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
                if is_async {
                    attrs
                        .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                        .map_err(|e| err("async unlock", e))?;
                }
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            let events = if is_async { transform.cast::<IMFMediaEventGenerator>().ok() } else { None };
            if is_async && events.is_none() {
                return Err(HostError::Encoder("async MFT without event generator".into()));
            }

            // D3D11 device sharing + video processor (GPU path); any failure
            // falls back to CPU buffers.
            let mut mgr = None;
            let mut d3d = None;
            let mut device = None;
            if let Some(tex) = sample.texture() {
                if let Ok(dev) = tex.GetDevice() {
                    if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
                        let _ = mt.SetMultithreadProtected(true);
                    }
                    let mut token = 0u32;
                    let mut m = None;
                    if MFCreateDXGIDeviceManager(&mut token, &mut m).is_ok() {
                        if let Some(m) = m {
                            if m.ResetDevice(&dev, token).is_ok()
                                && transform
                                    .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, m.as_raw() as usize)
                                    .is_ok()
                            {
                                match D3dPath::new(&dev, (sample.width, sample.height), (cfg.width, cfg.height), cfg.fps) {
                                    Ok(p) => {
                                        d3d = Some(p);
                                        mgr = Some(m);
                                    }
                                    Err(_) => {
                                        let _ = transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
                                    }
                                }
                            }
                        }
                    }
                    device = Some(dev);
                }
            }

            let codec_api = transform.cast::<ICodecAPI>().ok();
            if let Some(api) = &codec_api {
                // Best effort: not every MFT supports every property.
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &VARIANT_bool(true));
                let _ = api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &VARIANT_u32(0)); // CBR
                let _ = api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT_u32(cfg.bitrate_bps));
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &VARIANT_u32(cfg.fps.max(1) * 3600));
                let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &VARIANT_u32(0));
            }

            // Output type first, then input type (encoder requirement).
            let out_t = make_type(subtype, &cfg, true)?;
            transform
                .SetOutputType(0, &out_t, 0)
                .map_err(|e| err(&format!("{friendly}: SetOutputType"), e))?;
            let in_t = make_type(MFVideoFormat_NV12, &cfg, false)?;
            transform
                .SetInputType(0, &in_t, 0)
                .map_err(|e| err(&format!("{friendly}: SetInputType(NV12)"), e))?;

            let _ = transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| err("BEGIN_STREAMING", e))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| err("START_OF_STREAM", e))?;

            let info = transform.GetOutputStreamInfo(0).map_err(|e| err("GetOutputStreamInfo", e))?;
            let out_provides = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;

            let mut enc = Self {
                kind,
                cfg,
                transform,
                activate,
                events,
                codec_api,
                _mgr: mgr,
                d3d,
                device,
                is_async,
                need_input: 0,
                pending: VecDeque::new(),
                frame_no: 0,
                out_provides,
                out_size: info.cbSize.max(1 << 20),
                seq_header: Vec::new(),
                friendly,
                cpu_bgra: Vec::new(),
                cpu_scaled: Vec::new(),
                cpu_nv12: Vec::new(),
            };
            enc.refresh_seq_header();
            Ok(enc)
        }
    }

    fn refresh_seq_header(&mut self) {
        unsafe {
            if let Ok(t) = self.transform.GetOutputCurrentType(0) {
                if let Ok(n) = t.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) {
                    let mut v = vec![0u8; n as usize];
                    if t.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut v, None).is_ok() {
                        self.seq_header = v;
                    }
                }
            }
        }
    }

    fn set_u32(&self, guid: &GUID, v: u32) -> bool {
        match &self.codec_api {
            Some(api) => unsafe { api.SetValue(guid, &VARIANT_u32(v)).is_ok() },
            None => false,
        }
    }

    // ---- event pump -------------------------------------------------------

    /// Waits up to `deadline` for one MFT event. Returns its type.
    fn next_event(&self, deadline: Instant) -> Result<Option<u32>> {
        let gen = self.events.as_ref().expect("async only");
        loop {
            match unsafe { gen.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(ev) => return unsafe { ev.GetType() }.map(Some).map_err(|e| err("event type", e)),
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_micros(150));
                }
                Err(e) => return Err(err("GetEvent", e)),
            }
        }
    }

    fn on_event(&mut self, ev: u32) -> Result<()> {
        if ev == METransformNeedInput.0 as u32 {
            self.need_input += 1;
        } else if ev == METransformHaveOutput.0 as u32 {
            self.pull_output()?;
        } else if ev == MEError.0 as u32 {
            return Err(HostError::Encoder("MFT raised MEError".into()));
        }
        Ok(())
    }

    fn pull_output(&mut self) -> Result<()> {
        unsafe {
            let own = if self.out_provides {
                None
            } else {
                let s = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
                let b = MFCreateMemoryBuffer(self.out_size).map_err(|e| err("MFCreateMemoryBuffer", e))?;
                s.AddBuffer(&b).map_err(|e| err("AddBuffer", e))?;
                Some(s)
            };
            let mut buf = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(own),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let r = self.transform.ProcessOutput(0, &mut buf, &mut status);
            let sample = ManuallyDrop::take(&mut buf[0].pSample);
            ManuallyDrop::drop(&mut buf[0].pEvents);
            match r {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    // Renegotiate the output type with the first one offered.
                    let t = self.transform.GetOutputAvailableType(0, 0).map_err(|e| err("stream change", e))?;
                    self.transform.SetOutputType(0, &t, 0).map_err(|e| err("stream change set", e))?;
                    self.refresh_seq_header();
                    return Ok(());
                }
                Err(e) => return Err(err("ProcessOutput", e)),
            }
            let Some(sample) = sample else { return Ok(()) };
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let mb = sample.ConvertToContiguousBuffer().map_err(|e| err("ConvertToContiguousBuffer", e))?;
            let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
            mb.Lock(&mut p, None, Some(&mut len)).map_err(|e| err("Lock", e))?;
            let mut data = Vec::with_capacity(len as usize + self.seq_header.len());
            data.extend_from_slice(std::slice::from_raw_parts(p, len as usize));
            let _ = mb.Unlock();
            let hevc = self.kind == EncoderKind::MfHevcHw;
            if key && !self.seq_header.is_empty() && !has_sps(&data, hevc) {
                let mut with = self.seq_header.clone();
                with.extend_from_slice(&data);
                data = with;
            }
            if !data.is_empty() {
                self.pending.push_back(data);
            }
        }
        Ok(())
    }

    // ---- input ------------------------------------------------------------

    fn make_sample(&mut self, frame: &CapturedFrame) -> Result<IMFSample> {
        let (w, h) = (self.cfg.width, self.cfg.height);
        let ts = (self.frame_no * 10_000_000 / self.cfg.fps.max(1) as u64) as i64;
        let dur = (10_000_000 / self.cfg.fps.max(1) as u64) as i64;
        let sample;
        unsafe {
            let mut done = None;
            if let (Some(d3d), Some(tex)) = (self.d3d.as_mut(), frame.texture()) {
                if d3d.in_size != (frame.width, frame.height) {
                    match D3dPath::new(&d3d.device, (frame.width, frame.height), d3d.out_size, self.cfg.fps) {
                        Ok(n) => *d3d = n,
                        Err(_) => {}
                    }
                }
                if d3d.in_size == (frame.width, frame.height) {
                    match d3d.convert(tex) {
                        Ok(nv12) => {
                            let b = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &nv12, 0, false)
                                .map_err(|e| err("MFCreateDXGISurfaceBuffer", e))?;
                            if let Ok(b2) = b.cast::<IMF2DBuffer>() {
                                if let Ok(n) = b2.GetContiguousLength() {
                                    let _ = b.SetCurrentLength(n);
                                }
                            }
                            let s = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
                            s.AddBuffer(&b).map_err(|e| err("AddBuffer", e))?;
                            done = Some(s);
                        }
                        Err(_) => {
                            // Permanently fall back to CPU conversion.
                            self.d3d = None;
                            let _ = self.transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
                        }
                    }
                }
            }
            sample = match done {
                Some(s) => s,
                None => {
                    frame.read_bgra(&mut self.cpu_bgra)?;
                    let (fw, fh) = (frame.width as usize, frame.height as usize);
                    let src: &[u8] = if (fw, fh) == (w as usize, h as usize) {
                        &self.cpu_bgra
                    } else {
                        scale_bgra_nearest(&self.cpu_bgra, fw, fh, w as usize, h as usize, &mut self.cpu_scaled);
                        &self.cpu_scaled
                    };
                    bgra_to_nv12(src, w as usize, h as usize, &mut self.cpu_nv12);
                    let n = self.cpu_nv12.len() as u32;
                    let b = MFCreateMemoryBuffer(n).map_err(|e| err("MFCreateMemoryBuffer", e))?;
                    let mut p = std::ptr::null_mut();
                    b.Lock(&mut p, None, None).map_err(|e| err("Lock", e))?;
                    std::ptr::copy_nonoverlapping(self.cpu_nv12.as_ptr(), p, n as usize);
                    let _ = b.Unlock();
                    b.SetCurrentLength(n).map_err(|e| err("SetCurrentLength", e))?;
                    let s = MFCreateSample().map_err(|e| err("MFCreateSample", e))?;
                    s.AddBuffer(&b).map_err(|e| err("AddBuffer", e))?;
                    s
                }
            };
            let _ = sample.SetSampleTime(ts);
            let _ = sample.SetSampleDuration(dur);
        }
        Ok(sample)
    }
}

impl VideoEncoder for MfEncoder {
    fn encode(&mut self, frame: &CapturedFrame, force_idr: bool) -> Result<Vec<u8>> {
        let _ = &self.device;
        // Anything already produced (late output from a previous call).
        if let Some(p) = self.pending.pop_front() {
            if !force_idr {
                return Ok(p);
            }
            self.pending.clear();
        }
        let sample = self.make_sample(frame)?;
        if force_idr {
            self.set_u32(&CODECAPI_AVEncVideoForceKeyFrame, 1);
        }
        unsafe {
            if self.is_async {
                let deadline = Instant::now() + Duration::from_millis(500);
                while self.need_input == 0 {
                    match self.next_event(deadline)? {
                        Some(ev) => self.on_event(ev)?,
                        None => return Err(HostError::Encoder("timeout waiting for METransformNeedInput".into())),
                    }
                }
                self.transform.ProcessInput(0, &sample, 0).map_err(|e| err("ProcessInput", e))?;
                self.need_input -= 1;
                self.frame_no += 1;
                // Low-latency encoders answer within a frame or two.
                let deadline = Instant::now() + Duration::from_millis(if self.frame_no < 3 { 1000 } else { 100 });
                while self.pending.is_empty() {
                    match self.next_event(deadline)? {
                        Some(ev) => self.on_event(ev)?,
                        None => break,
                    }
                }
            } else {
                self.transform.ProcessInput(0, &sample, 0).map_err(|e| err("ProcessInput", e))?;
                self.frame_no += 1;
                self.pull_output()?;
            }
        }
        Ok(self.pending.pop_front().unwrap_or_default())
    }

    fn codec(&self) -> u8 {
        self.kind.codec_flag()
    }

    fn kind(&self) -> EncoderKind {
        self.kind
    }

    fn set_bitrate(&mut self, bps: u32) -> Result<bool> {
        let ok = self.set_u32(&CODECAPI_AVEncCommonMeanBitRate, bps);
        if ok {
            self.cfg.bitrate_bps = bps;
        }
        Ok(ok)
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
            let _ = self.activate.ShutdownObject();
            let _ = MFShutdown();
        }
    }
}

// --------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------

fn enum_hw_encoders(subtype: GUID) -> Result<Vec<IMFActivate>> {
    unsafe {
        let out = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype };
        let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&out),
            &mut arr,
            &mut n,
        )
        .map_err(|e| err("MFTEnumEx", e))?;
        let mut v = Vec::new();
        if !arr.is_null() {
            for i in 0..n as usize {
                // Take ownership of each element (leaves None behind).
                if let Some(a) = (*arr.add(i)).take() {
                    v.push(a);
                }
            }
            CoTaskMemFree(Some(arr as *const _));
        }
        Ok(v)
    }
}

fn make_type(subtype: GUID, cfg: &EncoderConfig, output: bool) -> Result<IMFMediaType> {
    unsafe {
        let t = MFCreateMediaType().map_err(|e| err("MFCreateMediaType", e))?;
        let e = |x: windows::core::Result<()>| x.map_err(|e| err("media type", e));
        e(t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video))?;
        e(t.SetGUID(&MF_MT_SUBTYPE, &subtype))?;
        e(t.SetUINT64(&MF_MT_FRAME_SIZE, pack(cfg.width, cfg.height)))?;
        e(t.SetUINT64(&MF_MT_FRAME_RATE, pack(cfg.fps.max(1), 1)))?;
        e(t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)))?;
        e(t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32))?;
        if output {
            e(t.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_bps))?;
            if subtype == MFVideoFormat_H264 {
                e(t.SetUINT32(&MF_MT_MPEG2_PROFILE, 100))?; // eAVEncH264VProfile_High
            } else {
                e(t.SetUINT32(&MF_MT_MPEG2_PROFILE, 1))?; // eAVEncH265VProfile_Main_420_8
            }
        } else {
            e(t.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32))?;
            e(t.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32))?;
        }
        Ok(t)
    }
}
