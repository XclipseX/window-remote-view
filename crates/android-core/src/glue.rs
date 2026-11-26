//! Platform-neutral glue: hello parsing, Kotlin input -> `InputEvent`, and
//! `ClientEvent` -> callbacks for Kotlin. No JNI in here so it is unit-tested
//! on the host.

use rv_proto::{
    codec_flags, codec_mask, phase, ControlMessage, InputEvent, TouchPoint, MAX_TEXT_BYTES,
};
use rv_session::ClientEvent;

/// `NativeBridge.onConnectionStateFromNative` state codes.
pub mod state {
    pub const CONNECTING: i32 = 0;
    /// Noise handshake finished (host may still reject a wrong PIN silently).
    pub const ESTABLISHED: i32 = 1;
    /// Link/session closed normally.
    pub const CLOSED: i32 = 2;
    /// Fatal; `message` has the reason (e.g. bad PIN, no data).
    pub const FAILED: i32 = 3;
    /// Host told us the display config; message = "WxH@fps".
    pub const DISPLAY_CONFIG: i32 = 10;
    /// Host chose the stream parameters; message = "<codec> WxH@fps <kbps>kbps"
    /// with codec one of `h264`, `hevc`, `av1` (e.g. "hevc 2800x1752@120 80000kbps").
    pub const STREAM_CONFIG: i32 = 11;
    /// Host went silent for 5 s (or restarted). The worker keeps retrying the
    /// handshake by itself; `ESTABLISHED` follows when the host is back.
    pub const DISCONNECTED: i32 = 4;
}

/// Video flag bits understood by `NativeBridge` (`FLAG_KEYFRAME`, `FLAG_CONFIG`).
pub const FLAG_KEYFRAME: i32 = 1;

/// Audio codec ids for `onAudioFromNative`: 0 = PCM s16le, 1 = Opus.
pub const AUDIO_CODEC_OPUS: i32 = 1;

/// Parsed `helloJson` from `DisplayCaps.helloJson` (+ `otp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloConfig {
    pub otp: String,
    pub width: u16,
    pub height: u16,
    pub refresh_hz: u16,
    /// `codec_mask` bits from the optional `"codecs": ["hevc","h264","av1"]`
    /// field; defaults to H.264 only.
    pub codecs: u16,
}

/// Parse codec names ("h264"/"avc", "hevc"/"h265", "av1") into a `codec_mask`.
pub fn parse_codec_mask(v: Option<&serde_json::Value>) -> u16 {
    let mut mask = 0;
    if let Some(arr) = v.and_then(|x| x.as_array()) {
        for c in arr.iter().filter_map(|c| c.as_str()) {
            mask |= match c.trim().to_ascii_lowercase().as_str() {
                "h264" | "avc" => codec_mask::H264,
                "hevc" | "h265" => codec_mask::H265,
                "av1" => codec_mask::AV1,
                _ => 0,
            };
        }
    }
    if mask == 0 {
        codec_mask::H264
    } else {
        mask
    }
}

fn codec_name(id: u8) -> &'static str {
    match id {
        codec_flags::CODEC_H265 => "hevc",
        codec_flags::CODEC_AV1 => "av1",
        _ => "h264",
    }
}

impl HelloConfig {
    pub fn parse(json: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("hello json: {e}"))?;
        let num = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0).min(u64::from(u16::MAX)) as u16;
        Ok(Self {
            otp: v.get("otp").and_then(|x| x.as_str()).unwrap_or("").trim().to_string(),
            width: num("display_width"),
            height: num("display_height"),
            refresh_hz: num("display_refresh_hz"),
            codecs: parse_codec_mask(v.get("codecs")),
        })
    }

    /// The control Hello sent right after the handshake.
    pub fn hello_message(&self) -> ControlMessage {
        ControlMessage::Hello {
            proto_version: rv_proto::PROTOCOL_VERSION,
            codecs: self.codecs,
            max_w: self.width,
            max_h: self.height,
            max_fps: self.refresh_hz,
        }
    }
}

/// Input as declared by the `nativeSend*` JNI functions.
#[derive(Debug, Clone, PartialEq)]
pub enum UiInput {
    Key { scan_code: i32, extended: bool, down: bool },
    Text(String),
    MouseAbs { nx: f32, ny: f32 },
    MouseRel { dx: i32, dy: i32 },
    MouseButton { button: i32, down: bool },
    Wheel { horizontal: i32, vertical: i32 },
    Pen {
        nx: f32,
        ny: f32,
        pressure: i32,
        tilt_x: i32,
        tilt_y: i32,
        in_range: bool,
        in_contact: bool,
        barrel: bool,
        eraser: bool,
    },
    Sas,
    Touch { id: i32, x: i32, y: i32, phase: i32, pressure: i32 },
}

/// Kotlin pressure is 0..1024; the wire (and host) expects 0..65535.
fn scale_pressure(p: i32) -> u16 {
    (p.clamp(0, 1024) as u32 * 64).min(65535) as u16
}

fn norm(v: f32) -> u16 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 65535.0).round() as u16
}

fn i16c(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// Stateful translator (pen needs contact history to derive DOWN/MOVE/UP).
#[derive(Default)]
pub struct InputTranslator {
    pen_down: bool,
}

impl InputTranslator {
    pub fn translate(&mut self, input: UiInput) -> Vec<InputEvent> {
        match input {
            UiInput::Key { scan_code, extended, down } => vec![InputEvent::Key {
                scancode: scan_code.clamp(0, 0xFFFF) as u16,
                extended,
                down,
            }],
            UiInput::Text(s) => chunk_text(&s).into_iter().map(InputEvent::Text).collect(),
            UiInput::MouseAbs { nx, ny } => vec![InputEvent::MouseAbs { x: norm(nx), y: norm(ny) }],
            UiInput::MouseRel { dx, dy } => vec![InputEvent::MouseRel { dx: i16c(dx), dy: i16c(dy) }],
            // Kotlin ordinals LEFT, RIGHT, MIDDLE, X1, X2 == wire numbering.
            UiInput::MouseButton { button, down } => vec![InputEvent::MouseButton {
                button: button.clamp(0, 4) as u8,
                down,
            }],
            UiInput::Wheel { horizontal, vertical } => vec![InputEvent::Wheel {
                dx: i16c(horizontal),
                dy: i16c(vertical),
            }],
            UiInput::Sas => vec![InputEvent::SecureAttention],
            UiInput::Touch { id, x, y, phase: ph, pressure } => vec![InputEvent::Touch {
                points: vec![TouchPoint {
                    id: id.clamp(0, 255) as u8,
                    phase: ph.clamp(0, 3) as u8,
                    x: x.clamp(0, 65535) as u16,
                    y: y.clamp(0, 65535) as u16,
                    pressure: scale_pressure(pressure),
                }],
            }],
            UiInput::Pen { nx, ny, pressure, tilt_x, tilt_y, in_range, in_contact, barrel, eraser } => {
                let ph = match (in_contact, self.pen_down) {
                    (true, false) => phase::DOWN,
                    (true, true) => phase::MOVE,
                    (false, true) => phase::UP,
                    (false, false) if in_range => phase::MOVE, // hover
                    (false, false) => return Vec::new(),
                };
                self.pen_down = in_contact;
                vec![InputEvent::Pen {
                    x: norm(nx),
                    y: norm(ny),
                    pressure: if in_contact { scale_pressure(pressure) } else { 0 },
                    tilt_x: tilt_x.clamp(-90, 90) as i8,
                    tilt_y: tilt_y.clamp(-90, 90) as i8,
                    buttons: u8::from(barrel) | (u8::from(eraser) << 1),
                    phase: ph,
                }]
            }
        }
    }
}

fn chunk_text(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if cur.len() + ch.len_utf8() > MAX_TEXT_BYTES {
            out.push(std::mem::take(&mut cur));
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What gets pushed up to Kotlin.
#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Video { data: Vec<u8>, pts_us: i64, flags: i32 },
    Audio { data: Vec<u8>, pts_us: i64, codec: i32 },
    State { state: i32, message: Option<String> },
}

/// Receiver of [`Outgoing`] (JNI callbacks in production, a recorder in tests).
pub trait EventSink {
    fn video(&mut self, data: &[u8], pts_us: i64, flags: i32);
    fn audio(&mut self, data: &[u8], pts_us: i64, codec: i32);
    fn state(&mut self, state: i32, message: Option<&str>);
    fn stats(&mut self, rtt_ms: f32, bitrate_kbps: i32, loss_percent: f32);
}

pub fn deliver(sink: &mut dyn EventSink, o: &Outgoing) {
    match o {
        Outgoing::Video { data, pts_us, flags } => sink.video(data, *pts_us, *flags),
        Outgoing::Audio { data, pts_us, codec } => sink.audio(data, *pts_us, *codec),
        Outgoing::State { state, message } => sink.state(*state, message.as_deref()),
    }
}

/// Translate one session event. Frames are Annex-B already (host encoder output).
pub fn translate_event(ev: ClientEvent) -> Vec<Outgoing> {
    match ev {
        ClientEvent::Established { .. } => vec![Outgoing::State {
            state: state::ESTABLISHED,
            message: None,
        }],
        ClientEvent::Frame(f) => {
            let mut flags = 0;
            if f.keyframe {
                flags |= FLAG_KEYFRAME;
            }
            debug_assert!(matches!(
                f.codec,
                codec_flags::CODEC_H264 | codec_flags::CODEC_H265 | codec_flags::CODEC_AV1
            ) || f.codec == 0);
            vec![Outgoing::Video {
                data: f.data,
                pts_us: i64::from(f.capture_ts_us),
                flags,
            }]
        }
        ClientEvent::Audio(a) => vec![Outgoing::Audio {
            data: a.payload,
            pts_us: i64::from(a.capture_ts_us),
            codec: AUDIO_CODEC_OPUS,
        }],
        ClientEvent::Control(ControlMessage::DisplayConfigChange { w, h, fps }) => vec![Outgoing::State {
            state: state::DISPLAY_CONFIG,
            message: Some(format!("{w}x{h}@{fps}")),
        }],
        ClientEvent::Control(ControlMessage::StreamConfig { codec, width, height, fps, bitrate_kbps }) => {
            vec![Outgoing::State {
                state: state::STREAM_CONFIG,
                message: Some(format!("{} {width}x{height}@{fps} {bitrate_kbps}kbps", codec_name(codec))),
            }]
        }
        ClientEvent::Control(_) => Vec::new(),
        ClientEvent::Disconnected => vec![Outgoing::State {
            state: state::DISCONNECTED,
            message: None,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rv_proto::AudioPacket;
    use rv_session::Frame;

    #[test]
    fn hello_parses_kotlin_json() {
        let j = r#"{"mode":"usb_display","display_width":2800,"display_height":1752,
            "display_refresh_hz":120,"otp":" 123456 ","touch":{"max_points":10}}"#;
        let c = HelloConfig::parse(j).unwrap();
        assert_eq!(
            c,
            HelloConfig { otp: "123456".into(), width: 2800, height: 1752, refresh_hz: 120, codecs: codec_mask::H264 }
        );
        assert_eq!(
            c.hello_message(),
            ControlMessage::Hello {
                proto_version: rv_proto::PROTOCOL_VERSION,
                codecs: codec_mask::H264,
                max_w: 2800,
                max_h: 1752,
                max_fps: 120
            }
        );
        let c = HelloConfig::parse(r#"{"otp":"1","codecs":["hevc","H264","bogus"]}"#).unwrap();
        assert_eq!(c.codecs, codec_mask::H264 | codec_mask::H265);
        let c = HelloConfig::parse(r#"{"otp":"1","codecs":["av1"]}"#).unwrap();
        assert_eq!(c.codecs, codec_mask::AV1);
        assert_eq!(HelloConfig::parse(r#"{"codecs":[]}"#).unwrap().codecs, codec_mask::H264);
        assert!(HelloConfig::parse("nope").is_err());
    }

    #[test]
    fn touch_and_pressure_scaling() {
        let mut t = InputTranslator::default();
        let e = t.translate(UiInput::Touch { id: 3, x: 70000, y: -5, phase: 1, pressure: 1024 });
        assert_eq!(
            e,
            vec![InputEvent::Touch {
                points: vec![TouchPoint { id: 3, phase: 1, x: 65535, y: 0, pressure: 65535 }]
            }]
        );
    }

    #[test]
    fn mouse_and_keys() {
        let mut t = InputTranslator::default();
        assert_eq!(
            t.translate(UiInput::MouseAbs { nx: 0.5, ny: 2.0 }),
            vec![InputEvent::MouseAbs { x: 32768, y: 65535 }]
        );
        assert_eq!(
            t.translate(UiInput::MouseRel { dx: 100000, dy: -100000 }),
            vec![InputEvent::MouseRel { dx: i16::MAX, dy: i16::MIN }]
        );
        assert_eq!(
            t.translate(UiInput::Key { scan_code: 0x1D, extended: true, down: true }),
            vec![InputEvent::Key { scancode: 0x1D, extended: true, down: true }]
        );
        assert_eq!(t.translate(UiInput::Sas), vec![InputEvent::SecureAttention]);
        assert_eq!(
            t.translate(UiInput::MouseButton { button: 2, down: true }),
            vec![InputEvent::MouseButton { button: 2, down: true }]
        );
    }

    #[test]
    fn long_text_is_chunked_on_char_boundaries() {
        let s = "ệ".repeat(1000); // 3 bytes each
        let mut t = InputTranslator::default();
        let ev = t.translate(UiInput::Text(s.clone()));
        assert!(ev.len() >= 3);
        let mut joined = String::new();
        for e in ev {
            let InputEvent::Text(c) = e else { panic!() };
            assert!(c.len() <= MAX_TEXT_BYTES);
            joined.push_str(&c);
        }
        assert_eq!(joined, s);
    }

    fn pen(contact: bool, range: bool, p: i32) -> UiInput {
        UiInput::Pen {
            nx: 0.25, ny: 0.75, pressure: p, tilt_x: 120, tilt_y: -10,
            in_range: range, in_contact: contact, barrel: true, eraser: false,
        }
    }

    #[test]
    fn pen_phases_derived_from_contact() {
        let mut t = InputTranslator::default();
        let ph = |v: Vec<InputEvent>| match v.as_slice() {
            [InputEvent::Pen { phase, .. }] => Some(*phase),
            [] => None,
            _ => panic!(),
        };
        assert_eq!(ph(t.translate(pen(false, true, 0))), Some(phase::MOVE)); // hover
        assert_eq!(ph(t.translate(pen(true, true, 512))), Some(phase::DOWN));
        assert_eq!(ph(t.translate(pen(true, true, 600))), Some(phase::MOVE));
        assert_eq!(ph(t.translate(pen(false, true, 0))), Some(phase::UP));
        assert_eq!(ph(t.translate(pen(false, false, 0))), None); // left range
        let e = t.translate(pen(true, true, 1024));
        let InputEvent::Pen { x, y, pressure, tilt_x, tilt_y, buttons, .. } = e[0].clone() else { panic!() };
        assert_eq!((x, y, pressure, tilt_x, tilt_y, buttons), (16384, 49151, 65535, 90, -10, 1));
    }

    #[test]
    fn events_translate() {
        let f = ClientEvent::Frame(Frame {
            frame_id: 1,
            capture_ts_us: 42,
            keyframe: true,
            codec: codec_flags::CODEC_H264,
            data: vec![0, 0, 0, 1, 0x65],
        });
        assert_eq!(
            translate_event(f),
            vec![Outgoing::Video { data: vec![0, 0, 0, 1, 0x65], pts_us: 42, flags: FLAG_KEYFRAME }]
        );
        let a = ClientEvent::Audio(AudioPacket {
            seq_num: 1, capture_ts_us: 7, duration_ms: 10, channels_rate: 0x21, payload: vec![1, 2],
        });
        assert_eq!(translate_event(a), vec![Outgoing::Audio { data: vec![1, 2], pts_us: 7, codec: 1 }]);
        assert_eq!(
            translate_event(ClientEvent::Control(ControlMessage::DisplayConfigChange { w: 2800, h: 1752, fps: 120 })),
            vec![Outgoing::State { state: state::DISPLAY_CONFIG, message: Some("2800x1752@120".into()) }]
        );
        assert!(translate_event(ClientEvent::Control(ControlMessage::RequestIdr)).is_empty());
        assert_eq!(
            translate_event(ClientEvent::Control(ControlMessage::StreamConfig {
                codec: codec_flags::CODEC_H265,
                width: 2800,
                height: 1752,
                fps: 120,
                bitrate_kbps: 80000
            })),
            vec![Outgoing::State { state: state::STREAM_CONFIG, message: Some("hevc 2800x1752@120 80000kbps".into()) }]
        );
        assert_eq!(
            translate_event(ClientEvent::Disconnected),
            vec![Outgoing::State { state: state::DISCONNECTED, message: None }]
        );
        assert_eq!(
            translate_event(ClientEvent::Established { host_static: [0; 32] }),
            vec![Outgoing::State { state: state::ESTABLISHED, message: None }]
        );
    }
}
