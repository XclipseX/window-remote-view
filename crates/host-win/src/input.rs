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
pub fn px_to_virtual_norm(px: i32, vorigin: i32, vsize: i32) -> i32 {
    if vsize <= 1 {
        return 0;
    }
    (((px - vorigin) as i64 * 65535 + (vsize as i64 - 1) / 2) / (vsize as i64 - 1)) as i32
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
        assert_eq!(px_to_virtual_norm(3999, 0, 4000), 65535);
        assert_eq!(text_units("a\u{1F600}").len(), 3);
    }
}
