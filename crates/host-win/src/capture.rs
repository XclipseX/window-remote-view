//! Desktop capture: DXGI Desktop Duplication on Windows, stub elsewhere.

use std::time::{Duration, Instant};

use crate::{Rect, Result};
#[cfg(not(windows))]
use crate::HostError;

/// One monitor as seen by DXGI.
#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub adapter_index: u32,
    pub output_index: u32,
    pub name: String,
    /// DXGI adapter description (GPU name) the output hangs off.
    pub adapter_name: String,
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

/// Index of the output to capture: the one named `prefer` if still attached,
/// else the primary (its rect contains the origin 0,0), else the first
/// attached output. Pure so it can be unit-tested.
pub fn pick_output_index(outs: &[OutputInfo], prefer: Option<&str>) -> Option<usize> {
    if let Some(n) = prefer {
        if let Some(i) = outs.iter().position(|o| o.attached && o.name.eq_ignore_ascii_case(n)) {
            return Some(i);
        }
    }
    let primary = |o: &OutputInfo| {
        let r = o.desktop_rect;
        r.left <= 0 && r.top <= 0 && r.right > 0 && r.bottom > 0
    };
    outs.iter()
        .position(|o| o.attached && primary(o))
        .or_else(|| outs.iter().position(|o| o.attached))
}

/// Convenience over [`pick_output_index`].
pub fn pick_output<'a>(outs: &'a [OutputInfo], prefer: Option<&str>) -> Option<&'a OutputInfo> {
    pick_output_index(outs, prefer).map(|i| &outs[i])
}

/// After this long without a working DXGI duplication, frames come from GDI.
const GDI_AFTER: Duration = Duration::from_secs(3);

/// Self-healing capture: DXGI Desktop Duplication, re-opened from a full
/// re-enumeration (never cached ordinals) with backoff whenever it fails, and
/// a GDI BitBlt fallback so frames keep flowing while DXGI is unavailable.
pub struct Capturer {
    prefer: Option<String>,
    dup: Option<DesktopDuplicator>,
    #[cfg(windows)]
    gdi: Option<crate::capture_win::GdiCapture>,
    fail_since: Option<Instant>,
    retry_at: Instant,
    backoff: Duration,
    fails: u32,
    #[cfg(windows)]
    last_gdi: Instant,
    rect: Rect,
}

impl Capturer {
    pub fn new(prefer: Option<String>, rect: Rect) -> Self {
        Self {
            prefer,
            dup: None,
            #[cfg(windows)]
            gdi: None,
            fail_since: None,
            retry_at: Instant::now(),
            backoff: Duration::from_millis(50),
            fails: 0,
            #[cfg(windows)]
            last_gdi: Instant::now(),
            rect,
        }
    }

    /// Output rectangle in virtual-desktop coordinates of the current source.
    pub fn desktop_rect(&self) -> Rect {
        self.rect
    }

    fn log(level: crate::control::Level, m: String) {
        crate::control::log(level, &m);
    }

    /// Same contract as [`DesktopDuplicator::acquire`] but never returns an
    /// error: failures are handled internally.
    pub fn acquire(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        use crate::control::Level;
        if let Some(d) = self.dup.as_mut() {
            match d.acquire(timeout_ms) {
                Ok(f) => {
                    self.fail_since = None;
                    self.fails = 0;
                    self.backoff = Duration::from_millis(50);
                    return Ok(f);
                }
                Err(e) => {
                    Self::log(Level::Warn, format!("capture: DXGI error: {e}; re-enumerating outputs"));
                    self.prefer = Some(d.device_name().to_string());
                    self.dup = None;
                    self.fail_since.get_or_insert_with(Instant::now);
                    self.retry_at = Instant::now() + self.backoff;
                }
            }
        }
        if Instant::now() >= self.retry_at {
            match DesktopDuplicator::open(self.prefer.as_deref()) {
                Ok(d) => {
                    self.rect = d.desktop_rect();
                    self.prefer = Some(d.device_name().to_string());
                    #[cfg(windows)]
                    if self.gdi.take().is_some() {
                        Self::log(Level::Info, "capture: DXGI duplication is back; leaving GDI fallback".into());
                    }
                    self.dup = Some(d);
                    self.fail_since = None;
                    return Ok(None);
                }
                Err(e) => {
                    self.fails += 1;
                    if self.fails <= 3 || self.fails.is_multiple_of(20) {
                        Self::log(Level::Warn, format!("capture: DXGI init failed ({} times): {e}", self.fails));
                    }
                    self.fail_since.get_or_insert_with(Instant::now);
                    let cap = if self.fail_since.is_some_and(|t| t.elapsed() > GDI_AFTER) { 2000 } else { 500 };
                    self.backoff = (self.backoff * 2).min(Duration::from_millis(cap));
                    self.retry_at = Instant::now() + self.backoff;
                }
            }
        }
        // DXGI is down.
        #[cfg(windows)]
        if self.fail_since.is_some_and(|t| t.elapsed() > GDI_AFTER) {
            return self.acquire_gdi(timeout_ms);
        }
        std::thread::sleep(Duration::from_millis(timeout_ms.min(8) as u64));
        Ok(None)
    }

    #[cfg(windows)]
    fn acquire_gdi(&mut self, timeout_ms: u32) -> Result<Option<CapturedFrame>> {
        use crate::control::Level;
        // Throttle to ~60 fps.
        let min = Duration::from_millis(16);
        let since = self.last_gdi.elapsed();
        if since < min {
            std::thread::sleep((min - since).min(Duration::from_millis(timeout_ms as u64)));
            return Ok(None);
        }
        self.last_gdi = Instant::now();
        // Follow the monitor's current rect (mode changes) via a fresh enumeration.
        let want = list_outputs()
            .ok()
            .and_then(|o| pick_output(&o, self.prefer.as_deref()).cloned());
        if let Some(o) = &want {
            self.prefer = Some(o.name.clone());
            self.rect = o.desktop_rect;
        }
        if self.gdi.as_ref().is_none_or(|g| g.rect() != self.rect) {
            match crate::capture_win::GdiCapture::new(self.rect) {
                Ok(g) => {
                    Self::log(
                        Level::Warn,
                        format!(
                            "capture: DXGI unavailable for >3 s; using GDI BitBlt fallback on {} {:?}",
                            self.prefer.as_deref().unwrap_or("primary"),
                            self.rect
                        ),
                    );
                    self.gdi = Some(g);
                }
                Err(e) => {
                    Self::log(Level::Warn, format!("capture: GDI init failed: {e}"));
                    self.gdi = None;
                    std::thread::sleep(Duration::from_millis(200));
                    return Ok(None);
                }
            }
        }
        match self.gdi.as_mut().map(|g| g.grab()) {
            Some(Ok(f)) => Ok(Some(f)),
            Some(Err(e)) => {
                Self::log(Level::Warn, format!("capture: GDI grab failed: {e}"));
                self.gdi = None;
                Ok(None)
            }
            None => Ok(None),
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
    pub fn open(_prefer: Option<&str>) -> Result<Self> {
        Err(HostError::Unsupported("DXGI Desktop Duplication needs Windows"))
    }
    pub fn device_name(&self) -> &str {
        ""
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

#[cfg(test)]
mod tests {
    use super::*;

    fn o(name: &str, r: (i32, i32, i32, i32), attached: bool) -> OutputInfo {
        OutputInfo {
            adapter_index: 0,
            output_index: 0,
            name: name.into(),
            adapter_name: "gpu".into(),
            desktop_rect: Rect { left: r.0, top: r.1, right: r.2, bottom: r.3 },
            attached,
        }
    }

    #[test]
    fn picks_preferred_then_primary_then_first() {
        let outs = vec![
            o("\\\\.\\DISPLAY7", (1920, 0, 3840, 1080), true),
            o("\\\\.\\DISPLAY1", (0, 0, 1920, 1080), true),
            o("\\\\.\\DISPLAY2", (3840, 0, 5760, 1080), false),
        ];
        assert_eq!(pick_output_index(&outs, Some("\\\\.\\display7")), Some(0));
        assert_eq!(pick_output_index(&outs, Some("\\\\.\\DISPLAY2")), Some(1)); // detached -> primary
        assert_eq!(pick_output_index(&outs, Some("gone")), Some(1));
        assert_eq!(pick_output_index(&outs, None), Some(1));
        let no_primary = vec![o("a", (1920, 0, 3840, 1080), false), o("b", (1920, 0, 3840, 1080), true)];
        assert_eq!(pick_output_index(&no_primary, None), Some(1));
        assert_eq!(pick_output_index(&[], None), None);
    }
}
