//! Input (0x03) inner packet and typed events.

use crate::error::{ProtoError, Result};
use crate::wire::{atomic, check_max, put_i16, put_u16, Reader};

pub const INPUT_HEADER_LEN: usize = 6;
pub const MAX_TOUCH_POINTS: usize = 10;
/// Max UTF-8 bytes in a `Text` event.
pub const MAX_TEXT_BYTES: usize = 1024;

pub mod sub_type {
    pub const MOUSE_REL: u8 = 0x01;
    pub const MOUSE_ABS: u8 = 0x02;
    pub const MOUSE_BUTTON: u8 = 0x03;
    pub const WHEEL: u8 = 0x04;
    pub const KEY: u8 = 0x05;
    pub const TEXT: u8 = 0x06;
    pub const TOUCH: u8 = 0x07;
    pub const PEN: u8 = 0x08;
    pub const SECURE_ATTENTION: u8 = 0x09;
}

/// Touch/pen phase values (informational; not validated).
pub mod phase {
    pub const DOWN: u8 = 0;
    pub const MOVE: u8 = 1;
    pub const UP: u8 = 2;
    pub const CANCEL: u8 = 3;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchPoint {
    pub id: u8,
    pub phase: u8,
    pub x: u16,
    pub y: u16,
    pub pressure: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    MouseRel { dx: i16, dy: i16 },
    /// Normalized 0..=65535 across the captured display.
    MouseAbs { x: u16, y: u16 },
    MouseButton { button: u8, down: bool },
    Wheel { dx: i16, dy: i16 },
    Key { scancode: u16, extended: bool, down: bool },
    Text(String),
    Touch { points: Vec<TouchPoint> },
    Pen {
        x: u16,
        y: u16,
        pressure: u16,
        tilt_x: i8,
        tilt_y: i8,
        buttons: u8,
        phase: u8,
    },
    /// Ctrl+Alt+Del.
    SecureAttention,
}

const KEY_EXTENDED: u8 = 0x01;
const KEY_DOWN: u8 = 0x02;

impl InputEvent {
    pub fn sub_type(&self) -> u8 {
        use sub_type::*;
        match self {
            Self::MouseRel { .. } => MOUSE_REL,
            Self::MouseAbs { .. } => MOUSE_ABS,
            Self::MouseButton { .. } => MOUSE_BUTTON,
            Self::Wheel { .. } => WHEEL,
            Self::Key { .. } => KEY,
            Self::Text(_) => TEXT,
            Self::Touch { .. } => TOUCH,
            Self::Pen { .. } => PEN,
            Self::SecureAttention => SECURE_ATTENTION,
        }
    }

    /// Appends the event payload (without the input header).
    pub fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<()> {
        atomic(buf, |buf| {
            match self {
                Self::MouseRel { dx, dy } | Self::Wheel { dx, dy } => {
                    put_i16(buf, *dx);
                    put_i16(buf, *dy);
                }
                Self::MouseAbs { x, y } => {
                    put_u16(buf, *x);
                    put_u16(buf, *y);
                }
                Self::MouseButton { button, down } => {
                    buf.extend_from_slice(&[*button, u8::from(*down)]);
                }
                Self::Key {
                    scancode,
                    extended,
                    down,
                } => {
                    put_u16(buf, *scancode);
                    buf.push(
                        if *extended { KEY_EXTENDED } else { 0 } | if *down { KEY_DOWN } else { 0 },
                    );
                }
                Self::Text(s) => {
                    check_max("text", s.len(), MAX_TEXT_BYTES)?;
                    buf.extend_from_slice(s.as_bytes());
                }
                Self::Touch { points } => {
                    check_max("touch points", points.len(), MAX_TOUCH_POINTS)?;
                    buf.push(points.len() as u8);
                    for p in points {
                        buf.extend_from_slice(&[p.id, p.phase]);
                        put_u16(buf, p.x);
                        put_u16(buf, p.y);
                        put_u16(buf, p.pressure);
                    }
                }
                Self::Pen {
                    x,
                    y,
                    pressure,
                    tilt_x,
                    tilt_y,
                    buttons,
                    phase,
                } => {
                    put_u16(buf, *x);
                    put_u16(buf, *y);
                    put_u16(buf, *pressure);
                    buf.extend_from_slice(&[*tilt_x as u8, *tilt_y as u8, *buttons, *phase]);
                }
                Self::SecureAttention => {}
            }
            Ok(())
        })
    }

    /// Decodes a complete payload for `sub_type`; trailing bytes are rejected.
    pub fn decode_payload(sub_type: u8, payload: &[u8]) -> Result<Self> {
        use sub_type::*;
        let mut r = Reader::new(payload);
        let ev = match sub_type {
            MOUSE_REL => Self::MouseRel {
                dx: r.i16()?,
                dy: r.i16()?,
            },
            MOUSE_ABS => Self::MouseAbs {
                x: r.u16()?,
                y: r.u16()?,
            },
            MOUSE_BUTTON => Self::MouseButton {
                button: r.u8()?,
                down: r.bool()?,
            },
            WHEEL => Self::Wheel {
                dx: r.i16()?,
                dy: r.i16()?,
            },
            KEY => {
                let scancode = r.u16()?;
                let f = r.u8()?;
                if f & !(KEY_EXTENDED | KEY_DOWN) != 0 {
                    return Err(ProtoError::InvalidValue("unknown key flag bits"));
                }
                Self::Key {
                    scancode,
                    extended: f & KEY_EXTENDED != 0,
                    down: f & KEY_DOWN != 0,
                }
            }
            TEXT => {
                check_max("text", payload.len(), MAX_TEXT_BYTES)?;
                let b = r.take(payload.len())?;
                let s = core::str::from_utf8(b).map_err(|_| ProtoError::InvalidUtf8)?;
                Self::Text(s.to_owned())
            }
            TOUCH => {
                let n = r.u8()? as usize;
                check_max("touch points", n, MAX_TOUCH_POINTS)?;
                let mut points = Vec::with_capacity(n);
                for _ in 0..n {
                    points.push(TouchPoint {
                        id: r.u8()?,
                        phase: r.u8()?,
                        x: r.u16()?,
                        y: r.u16()?,
                        pressure: r.u16()?,
                    });
                }
                Self::Touch { points }
            }
            PEN => Self::Pen {
                x: r.u16()?,
                y: r.u16()?,
                pressure: r.u16()?,
                tilt_x: r.i8()?,
                tilt_y: r.i8()?,
                buttons: r.u8()?,
                phase: r.u8()?,
            },
            SECURE_ATTENTION => Self::SecureAttention,
            other => return Err(ProtoError::UnknownSubType(other)),
        };
        r.finish()?;
        Ok(ev)
    }
}

/// Input packet: header + one event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputPacket {
    pub input_seq: u16,
    pub flags: u8,
    pub event: InputEvent,
}

impl InputPacket {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        atomic(buf, |buf| {
            let start = buf.len();
            put_u16(buf, self.input_seq);
            buf.extend_from_slice(&[self.event.sub_type(), self.flags, 0, 0]);
            self.event.encode_payload(buf)?;
            let len = buf.len() - start - INPUT_HEADER_LEN;
            buf[start + 4..start + 6].copy_from_slice(&(len as u16).to_be_bytes());
            Ok(())
        })
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let input_seq = r.u16()?;
        let sub = r.u8()?;
        let flags = r.u8()?;
        let len = r.u16()? as usize;
        let payload = r.take(len)?;
        let event = InputEvent::decode_payload(sub, payload)?;
        Ok((
            Self {
                input_seq,
                flags,
                event,
            },
            r.pos(),
        ))
    }
}
