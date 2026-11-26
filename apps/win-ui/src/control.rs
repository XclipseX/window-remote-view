//! Protocol between the UI and `rv-host --json-status` (JSON lines).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum State {
    #[default]
    Starting,
    Waiting,
    Connecting,
    Connected,
    Streaming,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    Usb,
    Lan,
    Adb,
    #[default]
    None,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StatusEvent {
    pub state: State,
    #[serde(default)]
    pub link: Link,
    #[serde(default)]
    pub peer: Option<String>,
    #[serde(default)]
    pub otp: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct StatsEvent {
    #[serde(default)]
    pub fps: f32,
    #[serde(default)]
    pub bitrate_kbps: u32,
    #[serde(default)]
    pub encode_ms: f32,
    #[serde(default)]
    pub rtt_ms: Option<f32>,
    #[serde(default)]
    pub encoder: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeviceItem {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub serial: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Events emitted by the host on stdout.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum HostEvent {
    Status(StatusEvent),
    Stats(StatsEvent),
    Devices { items: Vec<DeviceItem> },
    Log { level: LogLevel, msg: String },
}

/// Commands sent to the host on stdin.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum HostCmd {
    SetQuality { quality: u8 },
    SetFps { fps: u32 },
    SetResolution { width: u32, height: u32 },
    RequestIdr,
    NewOtp,
    Stop,
}

impl HostCmd {
    pub fn to_line(&self) -> String {
        let mut s = serde_json::to_string(self).unwrap_or_default();
        s.push('\n');
        s
    }
}

pub fn parse_event(line: &str) -> Option<HostEvent> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status() {
        let l = r#"{"event":"status","state":"waiting","link":"usb","peer":null,"otp":"123456","message":"Chờ"}"#;
        match parse_event(l) {
            Some(HostEvent::Status(s)) => {
                assert_eq!(s.state, State::Waiting);
                assert_eq!(s.link, Link::Usb);
                assert_eq!(s.peer, None);
                assert_eq!(s.otp.as_deref(), Some("123456"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn parse_stats() {
        let l = r#"{"event":"stats","fps":59.8,"bitrate_kbps":25000,"encode_ms":3.2,"rtt_ms":null,"encoder":"NVENC","width":2560,"height":1600}"#;
        match parse_event(l) {
            Some(HostEvent::Stats(s)) => {
                assert_eq!(s.bitrate_kbps, 25000);
                assert_eq!(s.rtt_ms, None);
                assert_eq!(s.width, 2560);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn parse_devices_and_log() {
        let l = r#"{"event":"devices","items":[{"kind":"usb","name":"SM-X920","serial":"R1"}]}"#;
        assert!(matches!(parse_event(l), Some(HostEvent::Devices { items }) if items.len() == 1));
        let l = r#"{"event":"log","level":"warn","msg":"x"}"#;
        assert!(matches!(parse_event(l), Some(HostEvent::Log { level: LogLevel::Warn, .. })));
        assert!(parse_event("garbage").is_none());
        assert!(parse_event(r#"{"event":"future"}"#).is_none());
    }

    #[test]
    fn cmd_lines() {
        assert_eq!(HostCmd::SetQuality { quality: 80 }.to_line(), "{\"cmd\":\"set_quality\",\"quality\":80}\n");
        assert_eq!(HostCmd::RequestIdr.to_line(), "{\"cmd\":\"request_idr\"}\n");
        assert_eq!(HostCmd::NewOtp.to_line(), "{\"cmd\":\"new_otp\"}\n");
        assert_eq!(
            HostCmd::SetResolution { width: 1920, height: 1080 }.to_line(),
            "{\"cmd\":\"set_resolution\",\"width\":1920,\"height\":1080}\n"
        );
    }
}
