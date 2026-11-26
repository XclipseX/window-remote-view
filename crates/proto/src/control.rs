//! Control (0x04) inner packet and typed messages.

use crate::error::{ProtoError, Result};
use crate::wire::{atomic, check_max, put_u16, put_u32, Reader};

pub const CONTROL_HEADER_LEN: usize = 6;
/// Largest payload expressible in the u16 `payload_len` field.
pub const MAX_CONTROL_PAYLOAD: usize = u16::MAX as usize;

pub mod op_code {
    pub const HELLO: u8 = 0x01;
    pub const DISPLAY_CONFIG_CHANGE: u8 = 0x02;
    pub const REQUEST_IDR: u8 = 0x03;
    pub const CURSOR_SHAPE: u8 = 0x04;
    pub const CLIPBOARD_SYNC: u8 = 0x05;
    pub const QUALITY_SLIDER_UPDATE: u8 = 0x06;
    pub const PAUSE_VIDEO: u8 = 0x07;
    pub const FEEDBACK_STATS: u8 = 0x08;
    pub const ACK: u8 = 0x09;
    pub const STREAM_CONFIG: u8 = 0x0A;
    pub const KEEPALIVE: u8 = 0x0B;
    pub const DEVICE_INFO: u8 = 0x0C;
    pub const CURSOR_POS: u8 = 0x0D;
    pub const LOG_CHUNK: u8 = 0x0E;
}

/// Codec capability bits for `Hello::codecs`.
pub mod codec_mask {
    pub const H264: u16 = 1 << 0;
    pub const H265: u16 = 1 << 1;
    pub const AV1: u16 = 1 << 2;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    Hello {
        proto_version: u8,
        codecs: u16,
        max_w: u16,
        max_h: u16,
        max_fps: u16,
    },
    DisplayConfigChange { w: u16, h: u16, fps: u16 },
    RequestIdr,
    /// `rgba.len()` must equal `w * h * 4`; whole payload must fit in u16.
    CursorShape {
        w: u16,
        h: u16,
        hot_x: u16,
        hot_y: u16,
        rgba: Vec<u8>,
    },
    ClipboardSync(String),
    /// `quality` in 1..=100.
    QualitySliderUpdate {
        quality: u8,
        width: u16,
        height: u16,
        fps: u16,
    },
    PauseVideo(bool),
    FeedbackStats {
        rtt_us: u32,
        loss_permille: u16,
        recv_kbps: u32,
        decode_us: u32,
    },
    Ack { ctrl_seq: u16 },
    /// Empty liveness ping, sent by both sides when otherwise idle (the
    /// session layer consumes it; it is never surfaced as an event).
    Keepalive,
    /// Host -> client: the codec/geometry actually chosen from the client's
    /// `Hello::codecs`. `codec` is a `codec_flags::CODEC_*` id (1=H264, 2=HEVC, 3=AV1).
    /// Sent after the session is established and again on every change.
    StreamConfig {
        codec: u8,
        width: u16,
        height: u16,
        fps: u16,
        bitrate_kbps: u32,
    },
    /// Client -> host, sent right after the handshake: the friendly device
    /// name (UTF-8, at most [`MAX_DEVICE_NAME`] bytes), e.g. "Galaxy Tab S10+ (SM-X826B)".
    DeviceInfo { name: String },
    /// Host -> client: pointer position, `x`/`y` normalised 0..=65535 over the
    /// captured frame; `visible` is false when the cursor is hidden or off the
    /// captured display.
    CursorPos { x: u16, y: u16, visible: bool },
    /// Client -> host: one piece of a log file. `name` (UTF-8, at most
    /// [`MAX_LOG_NAME`] bytes) identifies the file, `seq` counts chunks from 0,
    /// `last` marks the final chunk, `data` holds at most [`MAX_LOG_CHUNK`] bytes.
    LogChunk {
        name: String,
        seq: u32,
        last: bool,
        data: Vec<u8>,
    },
}

/// Longest accepted `DeviceInfo` name in bytes.
pub const MAX_DEVICE_NAME: usize = 255;
/// Longest accepted `LogChunk` name in bytes.
pub const MAX_LOG_NAME: usize = 64;
/// Largest `LogChunk` data in bytes.
pub const MAX_LOG_CHUNK: usize = 16 * 1024;

fn check_quality(q: u8) -> Result<()> {
    if (1..=100).contains(&q) {
        Ok(())
    } else {
        Err(ProtoError::InvalidValue("quality must be in 1..=100"))
    }
}

impl ControlMessage {
    pub fn op_code(&self) -> u8 {
        use op_code::*;
        match self {
            Self::Hello { .. } => HELLO,
            Self::DisplayConfigChange { .. } => DISPLAY_CONFIG_CHANGE,
            Self::RequestIdr => REQUEST_IDR,
            Self::CursorShape { .. } => CURSOR_SHAPE,
            Self::ClipboardSync(_) => CLIPBOARD_SYNC,
            Self::QualitySliderUpdate { .. } => QUALITY_SLIDER_UPDATE,
            Self::PauseVideo(_) => PAUSE_VIDEO,
            Self::FeedbackStats { .. } => FEEDBACK_STATS,
            Self::Ack { .. } => ACK,
            Self::StreamConfig { .. } => STREAM_CONFIG,
            Self::Keepalive => KEEPALIVE,
            Self::DeviceInfo { .. } => DEVICE_INFO,
            Self::CursorPos { .. } => CURSOR_POS,
            Self::LogChunk { .. } => LOG_CHUNK,
        }
    }

    /// Appends the message payload (without the control header).
    pub fn encode_payload(&self, buf: &mut Vec<u8>) -> Result<()> {
        atomic(buf, |buf| {
            let start = buf.len();
            match self {
                Self::Hello {
                    proto_version,
                    codecs,
                    max_w,
                    max_h,
                    max_fps,
                } => {
                    buf.push(*proto_version);
                    put_u16(buf, *codecs);
                    put_u16(buf, *max_w);
                    put_u16(buf, *max_h);
                    put_u16(buf, *max_fps);
                }
                Self::DisplayConfigChange { w, h, fps } => {
                    put_u16(buf, *w);
                    put_u16(buf, *h);
                    put_u16(buf, *fps);
                }
                Self::RequestIdr | Self::Keepalive => {}
                Self::CursorShape {
                    w,
                    h,
                    hot_x,
                    hot_y,
                    rgba,
                } => {
                    let expected = usize::from(*w) * usize::from(*h) * 4;
                    if rgba.len() != expected {
                        return Err(ProtoError::LengthMismatch {
                            expected,
                            actual: rgba.len(),
                        });
                    }
                    check_max("cursor payload", 8 + rgba.len(), MAX_CONTROL_PAYLOAD)?;
                    for v in [*w, *h, *hot_x, *hot_y] {
                        put_u16(buf, v);
                    }
                    buf.extend_from_slice(rgba);
                }
                Self::ClipboardSync(s) => {
                    check_max("clipboard", s.len(), MAX_CONTROL_PAYLOAD)?;
                    buf.extend_from_slice(s.as_bytes());
                }
                Self::QualitySliderUpdate {
                    quality,
                    width,
                    height,
                    fps,
                } => {
                    check_quality(*quality)?;
                    buf.push(*quality);
                    put_u16(buf, *width);
                    put_u16(buf, *height);
                    put_u16(buf, *fps);
                }
                Self::PauseVideo(p) => buf.push(u8::from(*p)),
                Self::FeedbackStats {
                    rtt_us,
                    loss_permille,
                    recv_kbps,
                    decode_us,
                } => {
                    put_u32(buf, *rtt_us);
                    put_u16(buf, *loss_permille);
                    put_u32(buf, *recv_kbps);
                    put_u32(buf, *decode_us);
                }
                Self::Ack { ctrl_seq } => put_u16(buf, *ctrl_seq),
                Self::StreamConfig {
                    codec,
                    width,
                    height,
                    fps,
                    bitrate_kbps,
                } => {
                    buf.push(*codec);
                    put_u16(buf, *width);
                    put_u16(buf, *height);
                    put_u16(buf, *fps);
                    put_u32(buf, *bitrate_kbps);
                }
                Self::DeviceInfo { name } => {
                    check_max("device name", name.len(), MAX_DEVICE_NAME)?;
                    buf.extend_from_slice(name.as_bytes());
                }
                Self::CursorPos { x, y, visible } => {
                    put_u16(buf, *x);
                    put_u16(buf, *y);
                    buf.push(u8::from(*visible));
                }
                Self::LogChunk { name, seq, last, data } => {
                    check_max("log name", name.len(), MAX_LOG_NAME)?;
                    check_max("log chunk", data.len(), MAX_LOG_CHUNK)?;
                    buf.push(name.len() as u8);
                    buf.extend_from_slice(name.as_bytes());
                    put_u32(buf, *seq);
                    buf.push(u8::from(*last));
                    buf.extend_from_slice(data);
                }
            }
            check_max("control payload", buf.len() - start, MAX_CONTROL_PAYLOAD)
        })
    }

    /// Decodes a complete payload for `op`; trailing bytes are rejected.
    pub fn decode_payload(op: u8, payload: &[u8]) -> Result<Self> {
        use op_code::*;
        let mut r = Reader::new(payload);
        let msg = match op {
            HELLO => Self::Hello {
                proto_version: r.u8()?,
                codecs: r.u16()?,
                max_w: r.u16()?,
                max_h: r.u16()?,
                max_fps: r.u16()?,
            },
            DISPLAY_CONFIG_CHANGE => Self::DisplayConfigChange {
                w: r.u16()?,
                h: r.u16()?,
                fps: r.u16()?,
            },
            REQUEST_IDR => Self::RequestIdr,
            CURSOR_SHAPE => {
                let w = r.u16()?;
                let h = r.u16()?;
                let hot_x = r.u16()?;
                let hot_y = r.u16()?;
                let expected = usize::from(w) * usize::from(h) * 4;
                if r.remaining() != expected {
                    return Err(ProtoError::LengthMismatch {
                        expected,
                        actual: r.remaining(),
                    });
                }
                Self::CursorShape {
                    w,
                    h,
                    hot_x,
                    hot_y,
                    rgba: r.take(expected)?.to_vec(),
                }
            }
            CLIPBOARD_SYNC => {
                let b = r.take(payload.len())?;
                let s = core::str::from_utf8(b).map_err(|_| ProtoError::InvalidUtf8)?;
                Self::ClipboardSync(s.to_owned())
            }
            QUALITY_SLIDER_UPDATE => {
                let quality = r.u8()?;
                check_quality(quality)?;
                Self::QualitySliderUpdate {
                    quality,
                    width: r.u16()?,
                    height: r.u16()?,
                    fps: r.u16()?,
                }
            }
            PAUSE_VIDEO => Self::PauseVideo(r.bool()?),
            FEEDBACK_STATS => Self::FeedbackStats {
                rtt_us: r.u32()?,
                loss_permille: r.u16()?,
                recv_kbps: r.u32()?,
                decode_us: r.u32()?,
            },
            ACK => Self::Ack { ctrl_seq: r.u16()? },
            KEEPALIVE => Self::Keepalive,
            STREAM_CONFIG => Self::StreamConfig {
                codec: r.u8()?,
                width: r.u16()?,
                height: r.u16()?,
                fps: r.u16()?,
                bitrate_kbps: r.u32()?,
            },
            DEVICE_INFO => {
                check_max("device name", payload.len(), MAX_DEVICE_NAME)?;
                let b = r.take(payload.len())?;
                let s = core::str::from_utf8(b).map_err(|_| ProtoError::InvalidUtf8)?;
                Self::DeviceInfo { name: s.to_owned() }
            }
            CURSOR_POS => Self::CursorPos {
                x: r.u16()?,
                y: r.u16()?,
                visible: r.bool()?,
            },
            LOG_CHUNK => {
                let n = usize::from(r.u8()?);
                check_max("log name", n, MAX_LOG_NAME)?;
                let name = core::str::from_utf8(r.take(n)?).map_err(|_| ProtoError::InvalidUtf8)?.to_owned();
                let seq = r.u32()?;
                let last = r.bool()?;
                check_max("log chunk", r.remaining(), MAX_LOG_CHUNK)?;
                let rem = r.remaining();
                Self::LogChunk { name, seq, last, data: r.take(rem)?.to_vec() }
            }
            other => return Err(ProtoError::UnknownOpCode(other)),
        };
        r.finish()?;
        Ok(msg)
    }
}

/// Control packet: header + one message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlPacket {
    pub ctrl_seq: u16,
    pub message: ControlMessage,
}

impl ControlPacket {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        atomic(buf, |buf| {
            let start = buf.len();
            put_u16(buf, self.ctrl_seq);
            buf.extend_from_slice(&[self.message.op_code(), 0, 0, 0]);
            self.message.encode_payload(buf)?;
            let len = buf.len() - start - CONTROL_HEADER_LEN;
            buf[start + 4..start + 6].copy_from_slice(&(len as u16).to_be_bytes());
            Ok(())
        })
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let ctrl_seq = r.u16()?;
        let op = r.u8()?;
        let _reserved = r.u8()?;
        let len = r.u16()? as usize;
        let payload = r.take(len)?;
        let message = ControlMessage::decode_payload(op, payload)?;
        Ok((Self { ctrl_seq, message }, r.pos()))
    }
}
