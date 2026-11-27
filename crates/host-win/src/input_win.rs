//! Windows input injection.

use std::collections::BTreeMap;

use rv_proto::{phase, InputEvent, TouchPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::Pointer::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::input::{norm_to_px, px_to_virtual_norm, InputInjector};
use crate::{HostError, Rect, Result};

const MAX_CONTACTS: u32 = 10;

pub struct WinInjector {
    display: Rect,
    touch_dev: Option<HSYNTHETICPOINTERDEVICE>,
    pen_dev: Option<HSYNTHETICPOINTERDEVICE>,
    /// Active touch contacts: id -> last pixel position.
    contacts: BTreeMap<u8, (i32, i32)>,
    pen_down: bool,
    /// Active finger id when touch falls back to mouse events via `SendInput`.
    fallback_touch_id: Option<u8>,
}

// The synthetic device handles are plain handles, used from one thread.
unsafe impl Send for WinInjector {}

fn create_pointer_devices() -> (Option<HSYNTHETICPOINTERDEVICE>, Option<HSYNTHETICPOINTERDEVICE>) {
    unsafe {
        let touch_dev = match CreateSyntheticPointerDevice(PT_TOUCH, MAX_CONTACTS, POINTER_FEEDBACK_DEFAULT) {
            Ok(d) => Some(d),
            Err(e) => {
                crate::control::log(
                    crate::control::Level::Warn,
                    &format!("input: CreateSyntheticPointerDevice(PT_TOUCH) failed: {e}; touch will fall back to mouse"),
                );
                None
            }
        };
        let pen_dev = match CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT) {
            Ok(d) => Some(d),
            Err(e) => {
                crate::control::log(
                    crate::control::Level::Warn,
                    &format!("input: CreateSyntheticPointerDevice(PT_PEN) failed: {e}; pen disabled on this desktop"),
                );
                None
            }
        };
        (touch_dev, pen_dev)
    }
}

impl WinInjector {
    /// `display` is the desktop rect of the captured (virtual) display.
    /// Synthetic touch/pen device failures are logged and degraded gracefully so
    /// mouse, keyboard, and `SendSAS` always remain available (including on Winlogon).
    pub fn new(display: Rect) -> Result<Self> {
        let (touch_dev, pen_dev) = create_pointer_devices();
        Ok(Self {
            display,
            touch_dev,
            pen_dev,
            contacts: BTreeMap::new(),
            pen_down: false,
            fallback_touch_id: None,
        })
    }

    /// Follows the live desktop rect of the captured output (resolution / position changes).
    pub fn set_display(&mut self, display: Rect) {
        self.display = display;
    }

    /// Destroys old synthetic pointer devices, clears active touch/pen state, and
    /// recreates the devices on the newly attached desktop (ignoring failure -> `None`).
    pub fn on_desktop_changed(&mut self) {
        unsafe {
            if let Some(d) = self.touch_dev.take() {
                DestroySyntheticPointerDevice(d);
            }
            if let Some(d) = self.pen_dev.take() {
                DestroySyntheticPointerDevice(d);
            }
        }
        self.contacts.clear();
        self.pen_down = false;
        self.fallback_touch_id = None;
        let (touch_dev, pen_dev) = create_pointer_devices();
        self.touch_dev = touch_dev;
        self.pen_dev = pen_dev;
    }

    fn map(&self, x: u16, y: u16) -> (i32, i32) {
        (
            norm_to_px(x, self.display.left, self.display.width()),
            norm_to_px(y, self.display.top, self.display.height()),
        )
    }

    fn send_inputs(&self, inputs: &[INPUT]) -> Result<()> {
        if inputs.is_empty() {
            return Ok(());
        }
        let n = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
        if n as usize != inputs.len() {
            return Err(HostError::Os(format!("SendInput injected {n}/{}", inputs.len())));
        }
        Ok(())
    }

    fn mouse(&self, flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data as u32,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn mouse_abs(&self, x: u16, y: u16, extra: MOUSE_EVENT_FLAGS) -> INPUT {
        let (px, py) = self.map(x, y);
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        self.mouse(
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | extra,
            px_to_virtual_norm(px, vx, vw),
            px_to_virtual_norm(py, vy, vh),
            0,
        )
    }

    fn key(&self, vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// Graceful single-finger touch -> mouse fallback via `SendInput` for desktops
    /// where synthetic pointer injection is unavailable (e.g. Winlogon lock/PIN screen).
    fn inject_touch_as_mouse(&mut self, points: &[TouchPoint]) -> Result<()> {
        let chosen = match self.fallback_touch_id {
            Some(active_id) => points.iter().find(|p| p.id == active_id),
            None => points.iter().find(|p| p.phase == phase::DOWN).or_else(|| points.first()),
        };
        let Some(p) = chosen else {
            return Ok(());
        };
        match p.phase {
            phase::DOWN => {
                self.fallback_touch_id = Some(p.id);
                let move_in = self.mouse_abs(p.x, p.y, MOUSE_EVENT_FLAGS(0));
                let down_in = self.mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0);
                self.send_inputs(&[move_in, down_in])
            }
            phase::MOVE => {
                let move_in = self.mouse_abs(p.x, p.y, MOUSE_EVENT_FLAGS(0));
                self.send_inputs(&[move_in])
            }
            phase::UP | phase::CANCEL => {
                if self.fallback_touch_id == Some(p.id) {
                    self.fallback_touch_id = None;
                    let move_in = self.mouse_abs(p.x, p.y, MOUSE_EVENT_FLAGS(0));
                    let up_in = self.mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0);
                    self.send_inputs(&[move_in, up_in])
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }

    fn inject_touch(&mut self, points: &[TouchPoint]) -> Result<()> {
        if points.is_empty() {
            return Ok(());
        }
        let Some(touch_dev) = self.touch_dev else {
            return self.inject_touch_as_mouse(points);
        };
        let mut infos: Vec<POINTER_TYPE_INFO> = Vec::new();
        // Contacts that continue but are absent from this event: no-move update.
        let mentioned: Vec<u8> = points.iter().map(|p| p.id).collect();
        let carried: Vec<(u8, (i32, i32))> = self
            .contacts
            .iter()
            .filter(|(id, _)| !mentioned.contains(id))
            .map(|(id, pos)| (*id, *pos))
            .collect();
        for (id, (x, y)) in carried {
            infos.push(touch_info(id, POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT, x, y, 512));
        }
        for p in points {
            let (x, y) = self.map(p.x, p.y);
            let pressure = ((p.pressure as u32 * 1024) / 65535).clamp(1, 1024);
            let flags = match p.phase {
                phase::DOWN => {
                    POINTER_FLAG_NEW | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_DOWN
                }
                phase::MOVE => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                phase::CANCEL => POINTER_FLAG_UP | POINTER_FLAG_CANCELED,
                _ => POINTER_FLAG_UP,
            };
            infos.push(touch_info(p.id, flags, x, y, pressure));
            if p.phase == phase::UP || p.phase == phase::CANCEL {
                self.contacts.remove(&p.id);
            } else {
                self.contacts.insert(p.id, (x, y));
            }
        }
        infos.truncate(MAX_CONTACTS as usize);
        if unsafe { InjectSyntheticPointerInput(touch_dev, &infos) }.is_err() {
            self.contacts.clear();
            return self.inject_touch_as_mouse(points);
        }
        self.fallback_touch_id = None;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn inject_pen(&mut self, x: u16, y: u16, pressure: u16, tilt_x: i8, tilt_y: i8, buttons: u8, ph: u8) -> Result<()> {
        let Some(pen_dev) = self.pen_dev else {
            return Ok(());
        };
        let (px, py) = self.map(x, y);
        let mut flags = match ph {
            phase::DOWN => {
                self.pen_down = true;
                POINTER_FLAG_NEW | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT | POINTER_FLAG_DOWN
            }
            phase::MOVE if self.pen_down => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            phase::MOVE => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE, // hover
            _ => {
                self.pen_down = false;
                POINTER_FLAG_UP
            }
        };
        if ph == phase::CANCEL {
            flags |= POINTER_FLAG_CANCELED;
        }
        let mut pen = POINTER_PEN_INFO::default();
        pen.pointerInfo = POINTER_INFO {
            pointerType: PT_PEN,
            pointerId: 0,
            pointerFlags: flags,
            ptPixelLocation: POINT { x: px, y: py },
            ..Default::default()
        };
        pen.penMask = PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_TILT_Y;
        pen.pressure = (pressure as u32 * 1024 / 65535).min(1024);
        pen.tiltX = i32::from(tilt_x).clamp(-90, 90);
        pen.tiltY = i32::from(tilt_y).clamp(-90, 90);
        // bit0 barrel, bit1 eraser (ASSUMPTION about rv-proto's `buttons` layout).
        if buttons & 1 != 0 {
            pen.penFlags |= PEN_FLAG_BARREL;
        }
        if buttons & 2 != 0 {
            pen.penFlags |= PEN_FLAG_ERASER;
        }
        let info = POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 { penInfo: pen },
        };
        unsafe { InjectSyntheticPointerInput(pen_dev, &[info])? };
        Ok(())
    }

    fn inject_text(&self, s: &str) -> Result<()> {
        let mut v = Vec::new();
        let shift_scan = unsafe {
            let sc = MapVirtualKeyW(u32::from(VK_LSHIFT.0), MAPVK_VK_TO_VSC) as u16;
            if sc != 0 { sc } else { 0x2A }
        };
        for ch in s.chars() {
            let scancode_mapping: Option<(u16, bool)> = match ch {
                '\n' => {
                    let sc = unsafe { MapVirtualKeyW(u32::from(VK_RETURN.0), MAPVK_VK_TO_VSC) as u16 };
                    (sc != 0).then_some((sc, false))
                }
                '\t' => {
                    let sc = unsafe { MapVirtualKeyW(u32::from(VK_TAB.0), MAPVK_VK_TO_VSC) as u16 };
                    (sc != 0).then_some((sc, false))
                }
                '\x20'..='\x7E' => {
                    let res = unsafe { VkKeyScanW(ch as u16) };
                    if res != -1 {
                        let vk = (res & 0xFF) as u16;
                        let mods = ((res >> 8) & 0xFF) as u8;
                        if mods & !0x01 == 0 {
                            let sc = unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) as u16 };
                            (sc != 0).then_some((sc, (mods & 0x01) != 0))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };
            if let Some((scan, need_shift)) = scancode_mapping {
                if need_shift {
                    v.push(self.key(0, shift_scan, KEYEVENTF_SCANCODE));
                }
                v.push(self.key(0, scan, KEYEVENTF_SCANCODE));
                v.push(self.key(0, scan, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP));
                if need_shift {
                    v.push(self.key(0, shift_scan, KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP));
                }
            } else {
                let mut buf = [0u16; 2];
                for u in ch.encode_utf16(&mut buf).iter().copied() {
                    v.push(self.key(0, u, KEYEVENTF_UNICODE));
                    v.push(self.key(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
            }
        }
        self.send_inputs(&v)
    }
}

fn touch_info(id: u8, flags: POINTER_FLAGS, x: i32, y: i32, pressure: u32) -> POINTER_TYPE_INFO {
    let mut t = POINTER_TOUCH_INFO::default();
    t.pointerInfo = POINTER_INFO {
        pointerType: PT_TOUCH,
        pointerId: u32::from(id),
        pointerFlags: flags,
        ptPixelLocation: POINT { x, y },
        ..Default::default()
    };
    t.touchMask = TOUCH_MASK_CONTACTAREA | TOUCH_MASK_ORIENTATION | TOUCH_MASK_PRESSURE;
    t.rcContact = windows::Win32::Foundation::RECT {
        left: x - 2,
        top: y - 2,
        right: x + 2,
        bottom: y + 2,
    };
    t.orientation = 90;
    t.pressure = pressure;
    POINTER_TYPE_INFO {
        r#type: PT_TOUCH,
        Anonymous: POINTER_TYPE_INFO_0 { touchInfo: t },
    }
}

impl Drop for WinInjector {
    fn drop(&mut self) {
        unsafe {
            if let Some(d) = self.touch_dev.take() {
                DestroySyntheticPointerDevice(d);
            }
            if let Some(d) = self.pen_dev.take() {
                DestroySyntheticPointerDevice(d);
            }
        }
    }
}

impl InputInjector for WinInjector {
    fn inject(&mut self, ev: &InputEvent) -> Result<()> {
        match ev {
            InputEvent::MouseRel { dx, dy } => {
                self.send_inputs(&[self.mouse(MOUSEEVENTF_MOVE, i32::from(*dx), i32::from(*dy), 0)])
            }
            InputEvent::MouseAbs { x, y } => {
                self.send_inputs(&[self.mouse_abs(*x, *y, MOUSE_EVENT_FLAGS(0))])
            }
            InputEvent::MouseButton { button, down } => {
                // 0 left, 1 right, 2 middle, 3 X1, 4 X2 (ASSUMPTION about rv-proto numbering).
                let (f, data) = match (button, down) {
                    (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
                    (0, false) => (MOUSEEVENTF_LEFTUP, 0),
                    (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
                    (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
                    (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
                    (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
                    (3, true) => (MOUSEEVENTF_XDOWN, 1),
                    (3, false) => (MOUSEEVENTF_XUP, 1),
                    (4, true) => (MOUSEEVENTF_XDOWN, 2),
                    (4, false) => (MOUSEEVENTF_XUP, 2),
                    _ => return Ok(()),
                };
                self.send_inputs(&[self.mouse(f, 0, 0, data)])
            }
            InputEvent::Wheel { dx, dy } => {
                // Units assumed to be WHEEL_DELTA-style (120 = one notch).
                let mut v = Vec::new();
                if *dy != 0 {
                    v.push(self.mouse(MOUSEEVENTF_WHEEL, 0, 0, i32::from(*dy)));
                }
                if *dx != 0 {
                    v.push(self.mouse(MOUSEEVENTF_HWHEEL, 0, 0, i32::from(*dx)));
                }
                self.send_inputs(&v)
            }
            InputEvent::Key { scancode, extended, down } => {
                let mut f = KEYEVENTF_SCANCODE;
                if *extended {
                    f |= KEYEVENTF_EXTENDEDKEY;
                }
                if !*down {
                    f |= KEYEVENTF_KEYUP;
                }
                self.send_inputs(&[self.key(0, *scancode, f)])
            }
            InputEvent::Text(s) => self.inject_text(s),
            InputEvent::Touch { points } => self.inject_touch(points),
            InputEvent::Pen { x, y, pressure, tilt_x, tilt_y, buttons, phase } => {
                self.inject_pen(*x, *y, *pressure, *tilt_x, *tilt_y, *buttons, *phase)
            }
            InputEvent::SecureAttention => send_sas(),
        }
    }

    fn on_desktop_changed(&mut self) {
        WinInjector::on_desktop_changed(self);
    }
}

type SendSasFn = unsafe extern "system" fn(windows::core::BOOL);

fn resolve_send_sas() -> std::result::Result<SendSasFn, String> {
    use std::sync::OnceLock;

    static SEND_SAS: OnceLock<std::result::Result<SendSasFn, String>> = OnceLock::new();
    SEND_SAS
        .get_or_init(|| {
            const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
            #[link(name = "kernel32")]
            extern "system" {
                fn LoadLibraryExW(
                    lp_lib_file_name: *const u16,
                    h_file: *mut core::ffi::c_void,
                    dw_flags: u32,
                ) -> *mut core::ffi::c_void;
                fn GetProcAddress(
                    h_module: *mut core::ffi::c_void,
                    lp_proc_name: *const u8,
                ) -> *const core::ffi::c_void;
                fn FreeLibrary(h_module: *mut core::ffi::c_void) -> i32;
            }
            // SAFETY: Load `sas.dll` strictly from System32 and resolve `SendSAS`.
            // On success the library remains loaded for the process lifetime so the cached
            // function pointer stays valid; on lookup failure the handle is freed.
            unsafe {
                let module = LoadLibraryExW(
                    windows::core::w!("sas.dll").as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_LIBRARY_SEARCH_SYSTEM32,
                );
                if module.is_null() {
                    let err = windows::core::Error::from_thread();
                    return Err(format!("LoadLibraryExW(sas.dll): {err}"));
                }
                let proc = GetProcAddress(module, windows::core::s!("SendSAS").as_ptr());
                if proc.is_null() {
                    let err = windows::core::Error::from_thread();
                    FreeLibrary(module);
                    return Err(format!("GetProcAddress(SendSAS): {err}"));
                }
                Ok(std::mem::transmute::<*const core::ffi::c_void, SendSasFn>(proc))
            }
        })
        .clone()
}

/// Sends a Software Secure Attention Sequence (Ctrl+Alt+Del) via `sas.dll!SendSAS(FALSE)`.
/// Requires running as LocalSystem (`RemoteViewSvc`) with the `SoftwareSASGeneration`
/// policy enabled.
fn send_sas() -> Result<()> {
    if !crate::session_state::running_as_system() {
        return Err(HostError::Unsupported(
            "Ctrl+Alt+Del cần dịch vụ RemoteViewSvc (SYSTEM)",
        ));
    }
    let f = resolve_send_sas().map_err(HostError::Os)?;
    // SAFETY: `f` is the resolved `VOID WINAPI SendSAS(BOOL AsUser)` entry point in `sas.dll`.
    // Passing `BOOL(0)` (`asUser = FALSE`) is the documented LocalSystem service mode.
    unsafe { f(windows::core::BOOL(0)) };
    crate::control::log(crate::control::Level::Info, "input: SendSAS(FALSE) invoked");
    Ok(())
}
