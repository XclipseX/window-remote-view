//! Mouse cursor forwarding: shape (BGRA from `GetIconInfo` + `DrawIconEx`,
//! works for DXGI and GDI capture alike) and position normalised over the
//! captured frame. Pure helpers are unit-tested everywhere.

use std::time::{Duration, Instant};

use crate::Rect;

/// Largest cursor bitmap sent; bigger ones are scaled down.
pub const MAX_CURSOR_DIM: u32 = 64;
/// Position updates are sent at most this often (120 Hz).
pub const POS_INTERVAL: Duration = Duration::from_micros(8_333);
/// New cursor handles (animated cursors) are rendered at most this often.
pub const SHAPE_INTERVAL: Duration = Duration::from_millis(50);

/// A cursor change for the session loop to forward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorUpdate {
    Shape { w: u16, h: u16, hot_x: u16, hot_y: u16, rgba: Vec<u8> },
    Pos { x: u16, y: u16, visible: bool },
}

/// Pointer pixel position -> 0..=65535 over `rect`; the flag is false when
/// the pointer is outside the captured display.
pub fn norm_cursor(px: i32, py: i32, rect: Rect) -> (u16, u16, bool) {
    let (w, h) = (rect.width(), rect.height());
    if w <= 0 || h <= 0 {
        return (0, 0, false);
    }
    let inside = px >= rect.left && px < rect.right && py >= rect.top && py < rect.bottom;
    let n = |v: i32, origin: i32, size: i32| -> u16 {
        let d = (v - origin).clamp(0, size - 1) as i64;
        if size <= 1 {
            0
        } else {
            ((d * 65535 + (size as i64 - 1) / 2) / (size as i64 - 1)) as u16
        }
    };
    (n(px, rect.left, w), n(py, rect.top, h), inside)
}

/// Combines the same cursor drawn on black and on white (both BGRA, equal
/// length) into straight-alpha RGBA. Pixels that invert the background
/// (I-beam XOR cursors) become opaque black.
pub fn composite_alpha(on_black: &[u8], on_white: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(on_black.len());
    for (b, w) in on_black.chunks_exact(4).zip(on_white.chunks_exact(4)) {
        let diff: i32 = (0..3).map(|i| i32::from(w[i]) - i32::from(b[i])).sum::<i32>() / 3;
        if diff < 0 {
            out.extend_from_slice(&[0, 0, 0, 255]);
            continue;
        }
        let a = (255 - diff).clamp(0, 255);
        if a == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let un = |c: u8| ((i32::from(c) * 255 + a / 2) / a).clamp(0, 255) as u8;
        out.extend_from_slice(&[un(b[2]), un(b[1]), un(b[0]), a as u8]);
    }
    out
}

/// Nearest-neighbour downscale so that neither side exceeds `max`.
/// Returns `(rgba, new_w, new_h)`; unchanged when already small enough.
pub fn downscale_rgba(src: &[u8], w: u32, h: u32, max: u32) -> (Vec<u8>, u32, u32) {
    if w <= max && h <= max || w == 0 || h == 0 {
        return (src.to_vec(), w, h);
    }
    let k = f64::from(max) / f64::from(w.max(h));
    let nw = ((f64::from(w) * k).round() as u32).max(1);
    let nh = ((f64::from(h) * k).round() as u32).max(1);
    let mut out = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        let sy = (u64::from(y) * u64::from(h) / u64::from(nh)) as usize;
        for x in 0..nw {
            let sx = (u64::from(x) * u64::from(w) / u64::from(nw)) as usize;
            let i = (sy * w as usize + sx) * 4;
            out.extend_from_slice(&src[i..i + 4]);
        }
    }
    (out, nw, nh)
}

/// Decides what to forward given the latest raw pointer state.
pub struct CursorTracker {
    last_handle: usize,
    last_shape_at: Option<Instant>,
    last_pos: Option<(u16, u16, bool)>,
    last_pos_at: Option<Instant>,
}

impl Default for CursorTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorTracker {
    pub fn new() -> Self {
        Self { last_handle: 0, last_shape_at: None, last_pos: None, last_pos_at: None }
    }

    /// Forget everything, so the next client gets the shape and position again.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `true` when the cursor image for `handle` must be (re)rendered now.
    pub fn want_shape(&mut self, handle: usize, now: Instant) -> bool {
        if handle == 0 || handle == self.last_handle {
            return false;
        }
        if self.last_shape_at.is_some_and(|t| now.saturating_duration_since(t) < SHAPE_INTERVAL) {
            return false; // animated cursor: pick it up on a later poll
        }
        self.last_handle = handle;
        self.last_shape_at = Some(now);
        true
    }

    /// `Some` when the position/visibility changed and the rate limit allows a send.
    pub fn want_pos(&mut self, pos: (u16, u16, bool), now: Instant) -> Option<CursorUpdate> {
        if self.last_pos == Some(pos) {
            return None;
        }
        // Visibility flips are never delayed; plain movement is limited to 120 Hz.
        let flip = self.last_pos.map(|p| p.2) != Some(pos.2);
        if !flip && self.last_pos_at.is_some_and(|t| now.saturating_duration_since(t) < POS_INTERVAL) {
            return None;
        }
        self.last_pos = Some(pos);
        self.last_pos_at = Some(now);
        Some(CursorUpdate::Pos { x: pos.0, y: pos.1, visible: pos.2 })
    }
}

/// Raw pointer state read from the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawCursor {
    pub x: i32,
    pub y: i32,
    pub showing: bool,
    /// `HCURSOR` as an integer (0 = none).
    pub handle: usize,
}

#[cfg(windows)]
mod win {
    use super::*;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    pub fn read() -> Option<RawCursor> {
        unsafe {
            let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
            GetCursorInfo(&mut ci).ok()?;
            let POINT { x, y } = ci.ptScreenPos;
            Some(RawCursor { x, y, showing: ci.flags.0 & CURSOR_SHOWING.0 != 0, handle: ci.hCursor.0 as usize })
        }
    }

    /// Renders the cursor to straight-alpha RGBA: `(w, h, hot_x, hot_y, rgba)`.
    pub fn render(handle: usize) -> Option<(u32, u32, u32, u32, Vec<u8>)> {
        unsafe {
            let hcursor = HCURSOR(handle as *mut _);
            let mut ii = ICONINFO::default();
            GetIconInfo(HICON(hcursor.0), &mut ii).ok()?;
            let has_color = !ii.hbmColor.is_invalid();
            let src = if has_color { ii.hbmColor } else { ii.hbmMask };
            let mut bm = BITMAP::default();
            let got = GetObjectW(
                HGDIOBJ(src.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut BITMAP as *mut core::ffi::c_void),
            );
            let (hx, hy) = (ii.xHotspot, ii.yHotspot);
            if has_color {
                let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
            }
            if got == 0 {
                return None;
            }
            let w = bm.bmWidth.max(0) as u32;
            // A monochrome cursor stacks the AND and XOR masks in one bitmap.
            let h = (if has_color { bm.bmHeight } else { bm.bmHeight / 2 }).max(0) as u32;
            if w == 0 || h == 0 || w > 512 || h > 512 {
                return None;
            }

            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32);
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let result = match dib {
                Ok(dib) if !bits.is_null() && !mem.is_invalid() => {
                    let old = SelectObject(mem, dib.into());
                    let n = (w * h * 4) as usize;
                    let draw = |fill: u8| -> Vec<u8> {
                        std::ptr::write_bytes(bits as *mut u8, fill, n);
                        let _ = DrawIconEx(mem, 0, 0, HICON(hcursor.0), w as i32, h as i32, 0, None, DI_NORMAL);
                        let _ = GdiFlush();
                        std::slice::from_raw_parts(bits as *const u8, n).to_vec()
                    };
                    let on_black = draw(0x00);
                    let on_white = draw(0xFF);
                    SelectObject(mem, old);
                    let _ = DeleteObject(dib.into());
                    Some((w, h, hx, hy, composite_alpha(&on_black, &on_white)))
                }
                _ => None,
            };
            if !mem.is_invalid() {
                let _ = DeleteDC(mem);
            }
            ReleaseDC(None, screen);
            result
        }
    }
}

/// Polls the OS cursor; `None` when it cannot be read (e.g. secure desktop).
#[cfg(windows)]
pub fn read_raw() -> Option<RawCursor> {
    win::read()
}

#[cfg(not(windows))]
pub fn read_raw() -> Option<RawCursor> {
    None
}

/// Renders a cursor handle to RGBA, capped to [`MAX_CURSOR_DIM`] (hotspot scaled with it).
pub fn render_shape(handle: usize) -> Option<CursorUpdate> {
    #[cfg(windows)]
    {
        let (w, h, hx, hy, rgba) = win::render(handle)?;
        let (rgba, nw, nh) = downscale_rgba(&rgba, w, h, MAX_CURSOR_DIM);
        let sx = |v: u32| (u64::from(v) * u64::from(nw) / u64::from(w)).min(u64::from(nw.saturating_sub(1))) as u16;
        let sy = |v: u32| (u64::from(v) * u64::from(nh) / u64::from(h)).min(u64::from(nh.saturating_sub(1))) as u16;
        Some(CursorUpdate::Shape { w: nw as u16, h: nh as u16, hot_x: sx(hx), hot_y: sy(hy), rgba })
    }
    #[cfg(not(windows))]
    {
        let _ = handle;
        None
    }
}

/// One poll: what to forward for the cursor over the capture `rect`.
pub fn poll(tracker: &mut CursorTracker, rect: Rect, now: Instant) -> Vec<CursorUpdate> {
    let mut out = Vec::new();
    let Some(raw) = read_raw() else {
        // Cursor unreadable (secure desktop, no session): hide it once.
        let p = tracker.last_pos.map_or((0, 0), |p| (p.0, p.1));
        out.extend(tracker.want_pos((p.0, p.1, false), now));
        return out;
    };
    if raw.showing && tracker.want_shape(raw.handle, now) {
        if let Some(shape) = render_shape(raw.handle) {
            out.push(shape);
        } else {
            tracker.last_handle = 0; // retry later
        }
    }
    let (x, y, inside) = norm_cursor(raw.x, raw.y, rect);
    out.extend(tracker.want_pos((x, y, raw.showing && inside), now));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(l: i32, t: i32, r: i32, b: i32) -> Rect {
        Rect { left: l, top: t, right: r, bottom: b }
    }

    #[test]
    fn normalises_over_the_captured_display() {
        let r = rect(1920, 0, 4800, 1800);
        assert_eq!(norm_cursor(1920, 0, r), (0, 0, true));
        assert_eq!(norm_cursor(4799, 1799, r), (65535, 65535, true));
        let (x, y, inside) = norm_cursor(3360, 900, r);
        assert!(inside && (i32::from(x) - 32768).abs() <= 40 && (i32::from(y) - 32768).abs() <= 40);
        // off the display: clamped, flagged invisible
        assert_eq!(norm_cursor(100, 100, r).0, 0);
        assert!(!norm_cursor(100, 100, r).2);
        assert!(!norm_cursor(5000, 10, r).2);
        assert_eq!(norm_cursor(1, 1, rect(0, 0, 0, 0)), (0, 0, false));
    }

    #[test]
    fn alpha_from_black_and_white_renders() {
        // opaque red, fully transparent, half-transparent white, XOR (inverting) pixel
        let black = [0, 0, 255, 255, 0, 0, 0, 0, 128, 128, 128, 255, 255, 255, 255, 255];
        let white = [0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 255];
        let out = composite_alpha(&black, &white);
        assert_eq!(&out[0..4], &[255, 0, 0, 255]); // red, opaque
        assert_eq!(&out[4..8], &[0, 0, 0, 0]); // transparent
        assert_eq!(out[11], 128); // half alpha
        assert_eq!(&out[8..11], &[255, 255, 255]);
        assert_eq!(&out[12..16], &[0, 0, 0, 255]);
    }

    #[test]
    fn downscale_caps_size() {
        let src: Vec<u8> = (0..128 * 96).flat_map(|i| [(i % 251) as u8, 1, 2, 255]).collect();
        let (o, w, h) = downscale_rgba(&src, 128, 96, 64);
        assert_eq!((w, h), (64, 48));
        assert_eq!(o.len(), 64 * 48 * 4);
        let (o, w, h) = downscale_rgba(&src[..32 * 32 * 4], 32, 32, 64);
        assert_eq!((w, h, o.len()), (32, 32, 32 * 32 * 4));
        // payload of a capped cursor always fits one control packet
        const { assert!(8 + 64 * 64 * 4 <= rv_proto::MAX_CONTROL_PAYLOAD) };
    }

    #[test]
    fn tracker_rate_limits_and_resends() {
        let mut t = CursorTracker::new();
        let t0 = Instant::now();
        assert!(t.want_shape(7, t0));
        assert!(!t.want_shape(7, t0)); // unchanged
        assert!(!t.want_shape(8, t0 + Duration::from_millis(10))); // animated: too soon
        assert!(t.want_shape(8, t0 + Duration::from_millis(60)));
        assert!(!t.want_shape(0, t0 + Duration::from_secs(1)));

        let p = |x| (x, 5u16, true);
        assert_eq!(t.want_pos(p(1), t0), Some(CursorUpdate::Pos { x: 1, y: 5, visible: true }));
        assert_eq!(t.want_pos(p(1), t0 + Duration::from_millis(50)), None); // same
        assert_eq!(t.want_pos(p(2), t0 + Duration::from_millis(2)), None); // < 8.3 ms
        assert!(t.want_pos(p(2), t0 + Duration::from_millis(9)).is_some());
        // hiding is immediate
        assert!(t.want_pos((2, 5, false), t0 + Duration::from_millis(10)).is_some());
        t.reset();
        assert!(t.want_shape(8, t0));
        assert!(t.want_pos(p(2), t0).is_some());
    }
}
