//! Machine-readable control channel for the GUI (see CONTROL_PROTOCOL.md).
//!
//! Pure data/serialisation logic, no platform dependencies, fully unit-tested.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

static JSON_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_json_mode(on: bool) {
    JSON_MODE.store(on, Ordering::Relaxed);
}

pub fn json_mode() -> bool {
    JSON_MODE.load(Ordering::Relaxed)
}

/// Writes one JSON line to stdout (only used in `--json-status` mode).
pub fn emit(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

/// Logs to the log file (when [`crate::logfile::init`] ran). In JSON mode it
/// is a `log` event on stdout (the GUI shows and files it), otherwise a stderr line.
pub fn log(level: Level, msg: &str) {
    crate::logfile::write_line(level.as_str(), msg);
    if json_mode() {
        emit(&log_json(level, msg));
    } else {
        eprintln!("[{}] {msg}", level.as_str());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Starting,
    Waiting,
    Connecting,
    Connected,
    Streaming,
    Error,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Starting => "starting",
            State::Waiting => "waiting",
            State::Connecting => "connecting",
            State::Connected => "connected",
            State::Streaming => "streaming",
            State::Error => "error",
        }
    }
}

/// Commands accepted on stdin, one JSON object per line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    SetQuality(u8),
    SetFps(u32),
    SetResolution { width: u32, height: u32 },
    RequestIdr,
    NewOtp,
    /// Retry the USB link now (clears the LAN fallback backoff), for the
    /// device card the user clicked. `serial` is the item's `serial` (may be empty).
    Connect { serial: String },
    Stop,
}

fn get_u64(v: &Value, key: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("field `{key}` missing or not a non-negative integer"))
}

pub fn parse_command(line: &str) -> Result<Command, String> {
    let v: Value = serde_json::from_str(line.trim()).map_err(|e| format!("bad json: {e}"))?;
    let cmd = v
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or("field `cmd` missing")?;
    match cmd {
        "set_quality" => {
            let q = get_u64(&v, "quality")?;
            if !(1..=100).contains(&q) {
                return Err("quality must be in 1..=100".into());
            }
            Ok(Command::SetQuality(q as u8))
        }
        "set_fps" => {
            let f = get_u64(&v, "fps")?;
            if !(1..=240).contains(&f) {
                return Err("fps must be in 1..=240".into());
            }
            Ok(Command::SetFps(f as u32))
        }
        "set_resolution" => {
            let (w, h) = (get_u64(&v, "width")?, get_u64(&v, "height")?);
            if !(16..=16384).contains(&w) || !(16..=16384).contains(&h) {
                return Err("width/height must be in 16..=16384".into());
            }
            Ok(Command::SetResolution { width: w as u32, height: h as u32 })
        }
        "request_idr" => Ok(Command::RequestIdr),
        "new_otp" => Ok(Command::NewOtp),
        "connect" => Ok(Command::Connect {
            serial: v.get("serial").and_then(Value::as_str).unwrap_or("").to_string(),
        }),
        "stop" => Ok(Command::Stop),
        other => Err(format!("unknown cmd `{other}`")),
    }
}

pub fn status_json(state: State, link: &str, peer: Option<&str>, otp: &str, message: &str) -> Value {
    json!({
        "event": "status",
        "state": state.as_str(),
        "link": link,
        "peer": peer,
        "otp": otp,
        "message": message,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stats {
    pub fps: f64,
    pub bitrate_kbps: u64,
    pub encode_ms: f64,
    pub rtt_ms: Option<f64>,
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    /// Frame-time metrics of frames actually sent (see `frame_stats`).
    pub frame: crate::frame_stats::FrameSummary,
}

pub fn stats_json(s: &Stats) -> Value {
    json!({
        "event": "stats",
        "fps": round1(s.fps),
        "bitrate_kbps": s.bitrate_kbps,
        "encode_ms": round2(s.encode_ms),
        "rtt_ms": s.rtt_ms.map(round1),
        "encoder": s.encoder,
        "width": s.width,
        "height": s.height,
        "fps_now": round1(s.frame.fps_now),
        "fps_avg": round1(s.frame.fps_avg),
        "fps_low1": round1(s.frame.fps_low1),
        "frame_ms_mean": round2(s.frame.frame_ms_mean),
        "frame_ms_std": round2(s.frame.frame_ms_std),
        "frame_ms_var": round2(s.frame.frame_ms_var),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceItem {
    pub kind: String,
    /// Friendly name when known (DeviceInfo / USB string), else "USB vvvv:pppp".
    pub name: String,
    pub serial: String,
    pub vid: u16,
    pub pid: u16,
    /// Already in Android Open Accessory mode (18D1:2D00/2D01).
    pub accessory: bool,
    /// libusb can open the device (false on Windows = WinUSB driver missing).
    pub driver_ok: bool,
    /// This device is the one currently linked to the session.
    pub connected: bool,
}

pub fn devices_json(items: &[DeviceItem]) -> Value {
    json!({
        "event": "devices",
        "items": items.iter().map(|d| json!({
            "kind": d.kind, "name": d.name, "serial": d.serial,
            "vid": d.vid, "pid": d.pid, "accessory": d.accessory,
            "driver_ok": d.driver_ok, "connected": d.connected,
        })).collect::<Vec<_>>(),
    })
}

/// Random 8-digit numeric pairing token for the USB link (AOA serial = session OTP).
pub fn random_token() -> String {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("rng");
    format!("{:08}", u64::from_le_bytes(b) % 100_000_000)
}

fn token_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("RemoteView").join("usb_token"))
}

fn valid_token(t: &str) -> bool {
    t.len() == 8 && t.bytes().all(|b| b.is_ascii_digit())
}

/// Persistent per-user 8-digit USB pairing token (`%APPDATA%\RemoteView\usb_token`),
/// generated once. An accessory left in accessory mode by an earlier run keeps
/// the same AOA serial, so it still matches the session OTP. Falls back to a
/// random in-memory token if the file cannot be written.
pub fn persistent_token() -> String {
    let path = token_path();
    if let Some(p) = &path {
        if let Ok(s) = std::fs::read_to_string(p) {
            let t = s.trim();
            if valid_token(t) {
                return t.to_string();
            }
        }
    }
    let t = random_token();
    if let Some(p) = &path {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, &t);
    }
    t
}

/// Windows computer name (shown to the tablet as the accessory description).
pub fn computer_name() -> String {
    for k in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(k) {
            let v = v.trim();
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    "Windows PC".into()
}

pub fn log_json(level: Level, msg: &str) -> Value {
    json!({"event": "log", "level": level.as_str(), "msg": msg})
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Random 6-digit one-time password.
pub fn random_otp() -> String {
    let mut b = [0u8; 8];
    getrandom::getrandom(&mut b).expect("rng");
    format!("{:06}", u64::from_le_bytes(b) % 1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_commands() {
        assert_eq!(parse_command(r#"{"cmd":"set_quality","quality":80}"#), Ok(Command::SetQuality(80)));
        assert_eq!(parse_command(r#"{"cmd":"set_fps","fps":60}"#), Ok(Command::SetFps(60)));
        assert_eq!(
            parse_command(r#" {"cmd":"set_resolution","width":1920,"height":1080} "#),
            Ok(Command::SetResolution { width: 1920, height: 1080 })
        );
        assert_eq!(parse_command(r#"{"cmd":"request_idr"}"#), Ok(Command::RequestIdr));
        assert_eq!(parse_command(r#"{"cmd":"new_otp"}"#), Ok(Command::NewOtp));
        assert_eq!(parse_command(r#"{"cmd":"stop"}"#), Ok(Command::Stop));
        assert_eq!(
            parse_command(r#"{"cmd":"connect","serial":"1-5"}"#),
            Ok(Command::Connect { serial: "1-5".into() })
        );
        assert_eq!(parse_command(r#"{"cmd":"connect"}"#), Ok(Command::Connect { serial: String::new() }));
    }

    #[test]
    fn rejects_bad_commands() {
        for bad in [
            "",
            "not json",
            r#"{"cmd":"set_quality","quality":0}"#,
            r#"{"cmd":"set_quality","quality":101}"#,
            r#"{"cmd":"set_quality"}"#,
            r#"{"cmd":"set_fps","fps":0}"#,
            r#"{"cmd":"set_resolution","width":1920}"#,
            r#"{"cmd":"set_resolution","width":4,"height":1080}"#,
            r#"{"cmd":"explode"}"#,
            r#"{"quality":3}"#,
        ] {
            assert!(parse_command(bad).is_err(), "{bad:?} should fail");
        }
    }

    #[test]
    fn status_shape() {
        let v = status_json(State::Waiting, "usb", None, "123456", "Đang chờ");
        assert_eq!(v["event"], "status");
        assert_eq!(v["state"], "waiting");
        assert_eq!(v["link"], "usb");
        assert!(v["peer"].is_null());
        assert_eq!(v["otp"], "123456");
        let line = v.to_string();
        assert!(!line.contains('\n'));
        let v = status_json(State::Streaming, "lan", Some("10.0.0.2:5000"), "000001", "x");
        assert_eq!(v["peer"], "10.0.0.2:5000");
    }

    #[test]
    fn stats_shape() {
        let s = Stats {
            fps: 59.96,
            bitrate_kbps: 14000,
            encode_ms: 2.345,
            rtt_ms: None,
            encoder: "mf-hevc-hw".into(),
            width: 2800,
            height: 1752,
            frame: crate::frame_stats::FrameSummary {
                fps_now: 59.96,
                fps_avg: 58.04,
                fps_low1: 41.234,
                frame_ms_mean: 17.226,
                frame_ms_std: 3.456,
                frame_ms_var: 11.944,
            },
        };
        let v = stats_json(&s);
        assert_eq!(v["fps_now"], 60.0);
        assert_eq!(v["fps_avg"], 58.0);
        assert_eq!(v["fps_low1"], 41.2);
        assert_eq!(v["frame_ms_mean"], 17.23);
        assert_eq!(v["frame_ms_std"], 3.46);
        assert_eq!(v["frame_ms_var"], 11.94);
        assert_eq!(v["event"], "stats");
        assert_eq!(v["fps"], 60.0);
        assert_eq!(v["encode_ms"], 2.35);
        assert!(v["rtt_ms"].is_null());
        assert_eq!(v["encoder"], "mf-hevc-hw");
        let v = stats_json(&Stats { rtt_ms: Some(2.54), ..s });
        assert_eq!(v["rtt_ms"], 2.5);
    }

    #[test]
    fn devices_and_log_shape() {
        let v = devices_json(&[DeviceItem {
            kind: "usb".into(),
            name: "Samsung".into(),
            serial: "R5".into(),
            vid: 0x18d1,
            pid: 0x2d00,
            accessory: true,
            driver_ok: false,
            connected: false,
        }]);
        assert_eq!(v["items"][0]["kind"], "usb");
        assert_eq!(v["items"][0]["pid"], 0x2d00);
        assert_eq!(v["items"][0]["driver_ok"], false);
        assert_eq!(v["items"][0]["serial"], "R5");
        assert_eq!(devices_json(&[])["items"].as_array().unwrap().len(), 0);
        let v = log_json(Level::Warn, "hi \"there\"\n");
        assert_eq!(v["level"], "warn");
        assert!(!v.to_string().contains('\n'));
    }

    #[test]
    fn token_is_eight_digits() {
        for _ in 0..50 {
            let t = random_token();
            assert_eq!(t.len(), 8);
            assert!(t.bytes().all(|b| b.is_ascii_digit()));
        }
        assert!(!computer_name().is_empty());
    }

    #[test]
    fn otp_is_six_digits() {
        for _ in 0..50 {
            let o = random_otp();
            assert_eq!(o.len(), 6);
            assert!(o.bytes().all(|b| b.is_ascii_digit()));
        }
    }
}
