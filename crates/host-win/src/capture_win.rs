//! DXGI Desktop Duplication (Windows 8+).

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::capture::{CapturedFrame, FrameData, OutputInfo};
use crate::{HostError, Rect, Result};

fn from_rect(r: RECT) -> Rect {
    Rect {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    }
}

pub fn list_outputs() -> Result<Vec<OutputInfo>> {
    let mut v = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut ai = 0;
        while let Ok(adapter) = factory.EnumAdapters1(ai) {
            let mut oi = 0;
            while let Ok(out) = adapter.EnumOutputs(oi) {
                let d = out.GetDesc()?;
                let len = d.DeviceName.iter().position(|&c| c == 0).unwrap_or(d.DeviceName.len());
                v.push(OutputInfo {
                    adapter_index: ai,
                    output_index: oi,
                    name: String::from_utf16_lossy(&d.DeviceName[..len]),
                    desktop_rect: from_rect(d.DesktopCoordinates),
                    attached: d.AttachedToDesktop.as_bool(),
                });
                oi += 1;
            }
            ai += 1;
        }
    }
    Ok(v)
}

/// GPU-side frame: a texture we own (copy of the duplication surface), so the
/// duplication frame can be released immediately. The same texture object is
/// reused for every frame: the consumer must finish with a frame (encode or
/// read back) before the next `acquire`.
pub struct GpuFrame {
    pub texture: ID3D11Texture2D,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
}

impl GpuFrame {
    pub fn read_bgra(&self, w: u32, h: u32, out: &mut Vec<u8>) -> Result<()> {
        unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            self.texture.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            desc.MiscFlags = 0;
            let mut staging = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or_else(|| HostError::Os("CreateTexture2D returned null".into()))?;
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
    adapter_index: u32,
    output_index: u32,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    rect: Rect,
    size: (u32, u32),
    copy: Option<ID3D11Texture2D>,
    rects_buf: Vec<RECT>,
}

impl DesktopDuplicator {
    pub fn new(adapter_index: u32, output_index: u32) -> Result<Self> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let adapter = factory.EnumAdapters1(adapter_index)?;
            let output = adapter.EnumOutputs(output_index)?;
            let desc = output.GetDesc()?;
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
            let rect = from_rect(desc.DesktopCoordinates);
            Ok(Self {
                adapter_index,
                output_index,
                device,
                ctx,
                dup,
                size: (rect.width() as u32, rect.height() as u32),
                rect,
                copy: None,
                rects_buf: vec![RECT::default(); 64],
            })
        }
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
    /// the duplication was lost and recreated (e.g. mode switch, UAC desktop).
    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                    *self = Self::new(self.adapter_index, self.output_index)?;
                    return Ok(None);
                }
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
                }),
            }))
        }
    }
}
