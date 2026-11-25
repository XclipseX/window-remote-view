//! Desktop capture: DXGI Desktop Duplication on Windows, stub elsewhere.

use crate::{Rect, Result};
#[cfg(not(windows))]
use crate::HostError;

/// One monitor as seen by DXGI.
#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub adapter_index: u32,
    pub output_index: u32,
    pub name: String,
    pub desktop_rect: Rect,
    pub attached: bool,
}

/// A captured desktop frame.
///
/// On Windows it normally wraps a D3D11 texture (BGRA8) so a GPU encoder can
/// consume it with no copy; `read_bgra` does a staging-texture readback for
/// CPU encoders. [`CapturedFrame::from_bgra`] makes a CPU-only frame (tests,
/// non-Windows).
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    /// Number of dirty rectangles DXGI reported for this frame.
    pub dirty_rects: u32,
    /// `AccumulatedFrames` from DXGI_OUTDUPL_FRAME_INFO (always >= 1 for
    /// frames returned by the duplicator).
    pub accumulated_frames: u32,
    pub(crate) data: FrameData,
}

pub(crate) enum FrameData {
    Cpu(Vec<u8>),
    #[cfg(windows)]
    Gpu(crate::capture_win::GpuFrame),
}

impl CapturedFrame {
    /// Tightly packed BGRA8 (stride = width*4).
    pub fn from_bgra(width: u32, height: u32, bgra: Vec<u8>) -> Self {
        assert_eq!(bgra.len(), (width * height * 4) as usize);
        Self {
            width,
            height,
            dirty_rects: 1,
            accumulated_frames: 1,
            data: FrameData::Cpu(bgra),
        }
    }

    /// Copies pixels into `out` as tightly packed BGRA8 (`width*height*4`).
    pub fn read_bgra(&self, out: &mut Vec<u8>) -> Result<()> {
        out.clear();
        match &self.data {
            FrameData::Cpu(v) => {
                out.extend_from_slice(v);
                Ok(())
            }
            #[cfg(windows)]
            FrameData::Gpu(g) => g.read_bgra(self.width, self.height, out),
        }
    }

    /// The D3D11 texture backing this frame, if any (Windows only).
    #[cfg(windows)]
    pub fn texture(&self) -> Option<&windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> {
        match &self.data {
            FrameData::Gpu(g) => Some(&g.texture),
            _ => None,
        }
    }
}

#[cfg(windows)]
pub use crate::capture_win::{list_outputs, DesktopDuplicator};

#[cfg(not(windows))]
pub fn list_outputs() -> Result<Vec<OutputInfo>> {
    Ok(Vec::new())
}

/// Non-Windows stub.
#[cfg(not(windows))]
pub struct DesktopDuplicator;

#[cfg(not(windows))]
impl DesktopDuplicator {
    pub fn new(_adapter: u32, _output: u32) -> Result<Self> {
        Err(HostError::Unsupported("DXGI Desktop Duplication needs Windows"))
    }
    pub fn desktop_rect(&self) -> Rect {
        Rect::default()
    }
    pub fn size(&self) -> (u32, u32) {
        (0, 0)
    }
    pub fn acquire(&mut self, _timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        Err(HostError::Unsupported("DXGI Desktop Duplication needs Windows"))
    }
}
