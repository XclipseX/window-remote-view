//! Windows input injection.

use std::collections::BTreeMap;

use rv_proto::{phase, InputEvent, TouchPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Input::Pointer::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::input::{norm_to_px, px_to_virtual_norm, text_units, InputInjector};
use crate::{HostError, Rect, Result};

const MAX_CONTACTS: u32 = 10;

pub struct WinInjector {
    display: Rect,
    touch_dev: HSYNTHETICPOINTERDEVICE,
    pen_dev: HSYNTHETICPOINTERDEVICE,
    /// Active touch contacts: id -> last pixel position.
    contacts: BTreeMap<u8, (i32, i32)>,
    pen_down: bool,
}

// The synthetic device handles are plain handles, used from one thread.
unsafe impl Send for WinInjector {}

impl WinInjector {
    /// `display` is the desktop rect of the captured (virtual) display.
    pub fn new(display: Rect) -> Result<Self> {
        unsafe {
            let touch_dev = CreateSyntheticPointerDevice(PT_TOUCH, MAX_CONTACTS, POINTER_FEEDBACK_DEFAULT)?;
            let pen_dev = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)?;
            Ok(Self {
                display,
                touch_dev,
                pen_dev,
                contacts: BTreeMap::new(),
                pen_down: false,
            })
        }
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

    fn inject_touch(&mut self, points: &[TouchPoint]) -> Result<()> {
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
        unsafe { InjectSyntheticPointerInput(self.touch_dev, &infos)? };
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn inject_pen(&mut self, x: u16, y: u16, pressure: u16, tilt_x: i8, tilt_y: i8, buttons: u8, ph: u8) -> Result<()> {
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
        unsafe { InjectSyntheticPointerInput(self.pen_dev, &[info])? };
        Ok(())
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
            DestroySyntheticPointerDevice(self.touch_dev);
            DestroySyntheticPointerDevice(self.pen_dev);
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
                let (px, py) = self.map(*x, *y);
                let (vx, vy, vw, vh) = unsafe {
                    (
                        GetSystemMetrics(SM_XVIRTUALSCREEN),
                        GetSystemMetrics(SM_YVIRTUALSCREEN),
                        GetSystemMetrics(SM_CXVIRTUALSCREEN),
                        GetSystemMetrics(SM_CYVIRTUALSCREEN),
                    )
                };
                self.send_inputs(&[self.mouse(
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    px_to_virtual_norm(px, vx, vw),
                    px_to_virtual_norm(py, vy, vh),
                    0,
                )])
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
            InputEvent::Text(s) => {
                let mut v = Vec::new();
                for u in text_units(s) {
                    v.push(self.key(0, u, KEYEVENTF_UNICODE));
                    v.push(self.key(0, u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
                }
                self.send_inputs(&v)
            }
            InputEvent::Touch { points } => self.inject_touch(points),
            InputEvent::Pen { x, y, pressure, tilt_x, tilt_y, buttons, phase } => {
                self.inject_pen(*x, *y, *pressure, *tilt_x, *tilt_y, *buttons, *phase)
            }
            InputEvent::SecureAttention => Err(HostError::Unsupported(
                "Ctrl+Alt+Del cannot be sent via SendInput; needs SendSAS (sas.dll) from a service",
            )),
        }
    }
}
