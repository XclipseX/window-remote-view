//! DXGI Desktop Duplication (Windows 8+).

use windows::core::{Interface, PCWSTR};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::capture::{pick_output_index, CapturedFrame, FrameData, OutputInfo};
use crate::{HostError, Rect, Result};

fn from_rect(r: RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

fn wstr(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Display-config view of one active target: which GDI source it feeds, its
/// monitor name and connector technology.
struct TargetInfo {
    gdi_name: String,
    friendly: String,
    tech: i32,
}

fn display_targets() -> Vec<TargetInfo> {
    let mut out = Vec::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).0 != 0 {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).0 != 0 {
            return out;
        }
        paths.truncate(np as usize);
        for p in &paths {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            let (friendly, tech) = if DisplayConfigGetDeviceInfo(&mut tgt.header) == 0 {
                (wstr(&tgt.monitorFriendlyDeviceName), tgt.outputTechnology.0)
            } else {
                (String::new(), p.targetInfo.outputTechnology.0)
            };
            out.push(TargetInfo { gdi_name: wstr(&src.viewGdiDeviceName), friendly, tech });
        }
    }
    out
}

/// `EnumDisplayDevices`: (GDI name, adapter string, is primary).
fn gdi_devices() -> Vec<(String, String, bool)> {
    let mut v = Vec::new();
    unsafe {
        let mut i = 0;
        loop {
            let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0).as_bool() {
                break;
            }
            v.push((wstr(&dd.DeviceName), wstr(&dd.DeviceString), dd.StateFlags.0 & DISPLAY_DEVICE_PRIMARY_DEVICE.0 != 0));
            i += 1;
        }
    }
    v
}

/// One GPU adapter, for the startup log.
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_mb: u64,
    pub software: bool,
}

pub fn list_adapters() -> Vec<AdapterInfo> {
    let mut v = Vec::new();
    unsafe {
        let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return v };
        let mut i = 0;
        while let Ok(a) = factory.EnumAdapters1(i) {
            if let Ok(d) = a.GetDesc1() {
                v.push(AdapterInfo {
                    name: wstr(&d.Description),
                    vendor_id: d.VendorId,
                    device_id: d.DeviceId,
                    dedicated_mb: (d.DedicatedVideoMemory as u64) >> 20,
                    software: d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0,
                });
            }
            i += 1;
        }
    }
    v
}

/// One enumerated output with the COM objects needed to duplicate it.
struct Found {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    info: OutputInfo,
}

/// Fresh enumeration of every adapter and every output. Ordinals are only
/// valid for this snapshot (adapters/outputs come and go: spacedesk, IddCx,
/// hybrid GPUs), so nothing here may be cached across re-inits.
fn enumerate_all() -> Result<Vec<Found>> {
    let mut v = Vec::new();
    let targets = display_targets();
    let gdi = gdi_devices();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            let adapter_name = adapter.GetDesc1().map(|d| wstr(&d.Description)).unwrap_or_default();
            let mut oi = 0;
            while let Ok(out) = adapter.EnumOutputs(oi) {
                if let Ok(d) = out.GetDesc() {
                    let name = wstr(&d.DeviceName);
                    let target = targets.iter().find(|t| t.gdi_name.eq_ignore_ascii_case(&name));
                    let g = gdi.iter().find(|g| g.0.eq_ignore_ascii_case(&name));
                    let info = OutputInfo {
                        adapter_index: ai,
                        output_index: oi,
                        adapter_name: adapter_name.clone(),
                        desktop_rect: from_rect(d.DesktopCoordinates),
                        attached: d.AttachedToDesktop.as_bool(),
                        friendly_name: target.map(|t| t.friendly.clone()).unwrap_or_default(),
                        device_string: g.map(|g| g.1.clone()).unwrap_or_default(),
                        primary: g.is_some_and(|g| g.2),
                        technology: target.map(|t| t.tech),
                        name,
                    };
                    v.push(Found { adapter: adapter.clone(), output: out, info });
                }
                oi += 1;
            }
            ai += 1;
        }
    }
    Ok(v)
}

pub fn list_outputs() -> Result<Vec<OutputInfo>> {
    Ok(enumerate_all()?.into_iter().map(|f| f.info).collect())
}

/// GPU-side frame: a texture we own (copy of the duplication surface), so the
/// duplication frame can be released immediately. The same texture object is
/// reused for every frame: the consumer must finish with a frame (encode or
/// read back) before the next `acquire`.
pub struct GpuFrame {
    pub texture: ID3D11Texture2D,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    /// CPU-readable staging texture shared by all frames of one duplicator and
    /// reused while (width, height, format) are unchanged.
    staging: StagingCache,
}

type StagingCache = std::sync::Arc<std::sync::Mutex<Option<(u32, u32, windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT, ID3D11Texture2D)>>>;

impl GpuFrame {
    pub fn read_bgra(&self, w: u32, h: u32, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            self.texture.GetDesc(&mut desc);
            let mut cache = self.staging.lock().unwrap_or_else(|p| p.into_inner());
            let reuse = matches!(&*cache, Some((cw, ch, cf, _)) if (*cw, *ch, *cf) == (desc.Width, desc.Height, desc.Format));
            if !reuse {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.BindFlags = 0;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.MiscFlags = 0;
                let mut t = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut t))?;
                let t = t.ok_or_else(|| HostError::Os("CreateTexture2D returned null".into()))?;
                *cache = Some((desc.Width, desc.Height, desc.Format, t));
            }
            let staging = cache.as_ref().map(|c| c.3.clone()).expect("staging texture");
            self.ctx.CopyResource(&staging, &self.texture);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let row = (w * 4) as usize;
            out.clear();
            out.reserve(row * h as usize);
            for y in 0..h as usize {
                let p = (m.pData as *const u8).add(y * m.RowPitch as usize);
                out.extend_from_slice(std::slice::from_raw_parts(p, row));
            }
            self.ctx.Unmap(&staging, 0);
        }
        Ok(())
    }
}

pub struct DesktopDuplicator {
    name: String,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    rect: Rect,
    size: (u32, u32),
    copy: Option<ID3D11Texture2D>,
    staging: StagingCache,
    rects_buf: Vec<RECT>,
}

impl DesktopDuplicator {
    /// (Re)initialises duplication from scratch: new factory, full adapter and
    /// output enumeration, then picks the output named `prefer` if it is still
    /// attached, else the primary one, else the first attached one. The D3D11
    /// device is created on the chosen output's own adapter.
    pub fn open(prefer: Option<&str>) -> Result<Self> {
        let mut all = enumerate_all()?;
        let infos: Vec<OutputInfo> = all.iter().map(|f| f.info.clone()).collect();
        let idx = pick_output_index(&infos, prefer)
            .ok_or_else(|| HostError::Os("no attached DXGI output".into()))?;
        let Found { adapter, output, info } = all.swap_remove(idx);
        crate::control::log(
            crate::control::Level::Info,
            &format!(
                "capture: DXGI adapter '{}' output {} {:?} (adapter #{} output #{})",
                info.adapter_name, info.name, info.desktop_rect, info.adapter_index, info.output_index
            ),
        );
        unsafe {
            let output1: IDXGIOutput1 = output.cast()?;
            let base: IDXGIAdapter = adapter.cast()?;
            let mut device = None;
            let mut ctx = None;
            D3D11CreateDevice(
                &base,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )?;
            let device = device.ok_or_else(|| HostError::Os("no D3D11 device".into()))?;
            let ctx = ctx.ok_or_else(|| HostError::Os("no D3D11 context".into()))?;
            let dup = output1.DuplicateOutput(&device)?;
            let rect = info.desktop_rect;
            Ok(Self {
                name: info.name,
                device,
                ctx,
                dup,
                size: (rect.width() as u32, rect.height() as u32),
                rect,
                copy: None,
                staging: Default::default(),
                rects_buf: vec![RECT::default(); 64],
            })
        }
    }

    /// GDI device name of the duplicated output (e.g. `\\.\DISPLAY1`).
    pub fn device_name(&self) -> &str {
        &self.name
    }

    /// Output rectangle in virtual-desktop coordinates (for input mapping).
    pub fn desktop_rect(&self) -> Rect {
        self.rect
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    fn dirty_count(&mut self) -> u32 {
        let mut need = 0u32;
        let cap = (self.rects_buf.len() * std::mem::size_of::<RECT>()) as u32;
        let r = unsafe { self.dup.GetFrameDirtyRects(cap, self.rects_buf.as_mut_ptr(), &mut need) };
        match r {
            Ok(()) => need / std::mem::size_of::<RECT>() as u32,
            // Buffer too small: `need` is the required size, which is the count we want.
            Err(e) if e.code() == DXGI_ERROR_MORE_DATA => {
                let n = need as usize / std::mem::size_of::<RECT>();
                self.rects_buf.resize(n.max(self.rects_buf.len() * 2), RECT::default());
                n as u32
            }
            Err(_) => 0,
        }
    }

    /// Waits up to `timeout_ms` for a new frame. `Ok(None)` means nothing new
    /// to encode: timeout, a cursor-only update (`AccumulatedFrames == 0`), or
    /// Errors (including `DXGI_ERROR_ACCESS_LOST`) are returned to the caller, which
    /// re-opens from scratch with a full re-enumeration.
    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(e.into()),
            }
            if info.AccumulatedFrames == 0 {
                // Static desktop: only pointer movement/shape changed.
                let _ = self.dup.ReleaseFrame();
                return Ok(None);
            }
            let dirty = if info.TotalMetadataBufferSize > 0 { self.dirty_count() } else { 0 };
            let tex: ID3D11Texture2D = res
                .ok_or_else(|| HostError::Os("null desktop resource".into()))?
                .cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            if self.copy.is_none() || self.size != (desc.Width, desc.Height) {
                let mut d = desc;
                d.Usage = D3D11_USAGE_DEFAULT;
                d.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
                d.CPUAccessFlags = 0;
                d.MiscFlags = 0;
                let mut t = None;
                self.device.CreateTexture2D(&d, None, Some(&mut t))?;
                self.copy = t;
                self.size = (desc.Width, desc.Height);
            }
            let copy = self.copy.clone().expect("copy texture");
            self.ctx.CopyResource(&copy, &tex);
            self.dup.ReleaseFrame()?;
            Ok(Some(CapturedFrame {
                width: desc.Width,
                height: desc.Height,
                dirty_rects: dirty,
                accumulated_frames: info.AccumulatedFrames,
                data: FrameData::Gpu(GpuFrame {
                    texture: copy,
                    device: self.device.clone(),
                    ctx: self.ctx.clone(),
                    staging: self.staging.clone(),
                }),
            }))
        }
    }
}

/// GDI fallback capture: BitBlt from the screen DC into a top-down 32-bit DIB
/// section. Slower than DXGI and misses some protected content, but works
/// where duplication is unavailable (virtual-display drivers, RDP, GPU resets).
pub struct GdiCapture {
    rect: Rect,
    screen: HDC,
    mem: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    w: u32,
    h: u32,
}

impl GdiCapture {
    pub fn new(rect: Rect) -> Result<Self> {
        let (w, h) = (rect.width().max(0) as u32, rect.height().max(0) as u32);
        if w == 0 || h == 0 {
            return Err(HostError::Os("GDI capture: empty rect".into()));
        }
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(HostError::Os("GetDC(NULL) failed".into()));
            }
            let mem = CreateCompatibleDC(Some(screen));
            if mem.is_invalid() {
                ReleaseDC(None, screen);
                return Err(HostError::Os("CreateCompatibleDC failed".into()));
            }
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32); // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    let _ = DeleteDC(mem);
                    ReleaseDC(None, screen);
                    return Err(HostError::Os("CreateDIBSection failed".into()));
                }
            };
            let old = SelectObject(mem, bmp.into());
            Ok(Self { rect, screen, mem, bmp, old, bits: bits as *mut u8, w, h })
        }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn grab(&mut self) -> Result<CapturedFrame> {
        unsafe {
            BitBlt(
                self.mem,
                0,
                0,
                self.w as i32,
                self.h as i32,
                Some(self.screen),
                self.rect.left,
                self.rect.top,
                SRCCOPY | CAPTUREBLT,
            )
            .map_err(|e| HostError::Os(format!("BitBlt: {e}")))?;
            let n = (self.w * self.h * 4) as usize;
            let mut v = std::slice::from_raw_parts(self.bits, n).to_vec();
            for px in v.chunks_exact_mut(4) {
                px[3] = 255; // GDI leaves alpha undefined
            }
            Ok(CapturedFrame::from_bgra(self.w, self.h, v))
        }
    }
}

impl Drop for GdiCapture {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.mem, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.mem);
            ReleaseDC(None, self.screen);
        }
    }
}
