//! Input injection from rv-proto `InputEvent`s.
//!
//! Windows: touch/pen via `InjectSyntheticPointerInput` (PT_TOUCH up to 10
//! contacts, PT_PEN), mouse/keyboard via `SendInput` (incl. KEYEVENTF_UNICODE).
//! Coordinates in events are normalised 0..=65535 over the captured display
//! and mapped onto that display's desktop rect.

use rv_proto::InputEvent;

#[cfg(not(windows))]
use crate::{HostError, Rect};
use crate::Result;

/// Maps a normalised 0..=65535 coordinate onto `origin + [0, size)`.
pub fn norm_to_px(v: u16, origin: i32, size: i32) -> i32 {
    if size <= 0 {
        return origin;
    }
    origin + ((v as i64 * (size as i64 - 1) + 32767) / 65535) as i32
}

/// Pixel -> 0..=65535 over a virtual-desktop span (for MOUSEEVENTF_VIRTUALDESK).
///
/// Windows maps the absolute value back as `n * size / 65536` (floor or round, depending on
/// version), so the exact inverse is `ceil(px * 65536 / size)`: both conventions then land on
/// `px`. The older `px * 65535 / (size - 1)` could land one pixel short on large spans.
pub fn px_to_virtual_norm(px: i32, vorigin: i32, vsize: i32) -> i32 {
    if vsize <= 1 {
        return 0;
    }
    let rel = ((px - vorigin) as i64).clamp(0, vsize as i64 - 1);
    ((rel * 65536 + vsize as i64 - 1) / vsize as i64).min(65535) as i32
}

/// UTF-16 code units to inject for a text event.
pub fn text_units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub trait InputInjector {
    fn inject(&mut self, ev: &InputEvent) -> Result<()>;
}

#[cfg(windows)]
pub use crate::input_win::WinInjector;

/// Non-Windows stub: records nothing, fails loudly.
#[cfg(not(windows))]
pub struct WinInjector;

#[cfg(not(windows))]
impl WinInjector {
    pub fn new(_display: Rect) -> Result<Self> {
        Err(HostError::Unsupported("input injection needs Windows"))
    }
    pub fn set_display(&mut self, _display: Rect) {}
}

#[cfg(not(windows))]
impl InputInjector for WinInjector {
    fn inject(&mut self, _ev: &InputEvent) -> Result<()> {
        Err(HostError::Unsupported("input injection needs Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping() {
        assert_eq!(norm_to_px(0, 100, 2800), 100);
        assert_eq!(norm_to_px(65535, 100, 2800), 100 + 2799);
        let mid = norm_to_px(32768, 0, 1001);
        assert!((mid - 500).abs() <= 1);
        assert_eq!(px_to_virtual_norm(0, 0, 4000), 0);
        assert_eq!(px_to_virtual_norm(3999, 0, 4000), 65520); // 65520 * 4000 / 65536 = 3999.0
        assert_eq!(text_units("a\u{1F600}").len(), 3);
    }

    #[test]
    fn virtual_norm_round_trips_to_the_same_pixel() {
        for size in [800, 1366, 1920, 2560, 2800, 3840, 7680] {
            for px in 0..size {
                let n = px_to_virtual_norm(px, 0, size) as i64;
                assert!((0..=65535).contains(&n));
                let floor = n * size as i64 / 65536;
                let round = (n * size as i64 + 32768) / 65536;
                assert_eq!(floor, px as i64, "floor size={size} px={px}");
                assert_eq!(round, px as i64, "round size={size} px={px}");
            }
        }
        // negative virtual origin (monitor left of the primary)
        assert_eq!(px_to_virtual_norm(-1920, -1920, 3840), 0);
    }
}
