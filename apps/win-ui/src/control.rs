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
    pub decode_ms: Option<f64>,
    #[serde(default)]
    pub encoder: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Frame-time metrics of frames actually sent (absent = 0 on older hosts).
    #[serde(default)]
    pub fps_now: f32,
    #[serde(default)]
    pub fps_avg: f32,
    #[serde(default)]
    pub fps_low1: f32,
    #[serde(default)]
    pub frame_ms_mean: f32,
    #[serde(default)]
    pub frame_ms_std: f32,
    #[serde(default)]
    pub frame_ms_var: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeviceItem {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub serial: String,
    #[serde(default)]
    pub vid: u16,
    #[serde(default)]
    pub pid: u16,
    /// Already in Android Open Accessory mode (18D1:2D00/2D01).
    #[serde(default)]
    pub accessory: bool,
    /// libusb can open it; false on Windows means the WinUSB driver is missing.
    #[serde(default = "yes")]
    pub driver_ok: bool,
    #[serde(default)]
    pub connected: bool,
}

fn yes() -> bool {
    true
}

impl From<rv_host_win::control::DeviceItem> for DeviceItem {
    fn from(d: rv_host_win::control::DeviceItem) -> Self {
        Self {
            kind: d.kind,
            name: d.name,
            serial: d.serial,
            vid: d.vid,
            pid: d.pid,
            accessory: d.accessory,
            driver_ok: d.driver_ok,
            connected: d.connected,
        }
    }
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
    RequestLog,
    NewOtp,
    /// Retry the USB link now for this device card.
    Connect { serial: String },
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
                assert_eq!(s.decode_ms, None);
                assert_eq!(s.width, 2560);
                // Older hosts do not send the frame-time fields.
                assert_eq!(s.fps_low1, 0.0);
                assert_eq!(s.frame_ms_var, 0.0);
            }
            o => panic!("{o:?}"),
        }
        let l = r#"{"event":"stats","fps":60.0,"decode_ms":2.75,"fps_now":59.0,"fps_avg":58.5,"fps_low1":41.2,"frame_ms_mean":17.1,"frame_ms_std":3.4,"frame_ms_var":11.6}"#;
        match parse_event(l) {
            Some(HostEvent::Stats(s)) => {
                assert_eq!(s.decode_ms, Some(2.75));
                assert_eq!(s.fps_now, 59.0);
                assert_eq!(s.fps_avg, 58.5);
                assert_eq!(s.fps_low1, 41.2);
                assert_eq!(s.frame_ms_mean, 17.1);
                assert_eq!(s.frame_ms_std, 3.4);
                assert_eq!(s.frame_ms_var, 11.6);
            }
            o => panic!("{o:?}"),
        }
        let l = r#"{"event":"stats","fps":60.0,"decode_ms":null}"#;
        match parse_event(l) {
            Some(HostEvent::Stats(s)) => assert_eq!(s.decode_ms, None),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn parse_devices_and_log() {
        let l = r#"{"event":"devices","items":[{"kind":"usb","name":"SM-X920","serial":"R1"}]}"#;
        assert!(matches!(parse_event(l), Some(HostEvent::Devices { items }) if items.len() == 1 && items[0].driver_ok));
        let l = r#"{"event":"devices","items":[{"kind":"usb","name":"Galaxy Tab S10+ (SM-X826B)","serial":"1-5","vid":6353,"pid":11520,"accessory":true,"driver_ok":false,"connected":false}]}"#;
        match parse_event(l) {
            Some(HostEvent::Devices { items }) => {
                assert!(items[0].accessory && !items[0].driver_ok);
                assert_eq!(items[0].pid, 0x2d00);
            }
            o => panic!("{o:?}"),
        }
        let l = r#"{"event":"log","level":"warn","msg":"x"}"#;
        assert!(matches!(parse_event(l), Some(HostEvent::Log { level: LogLevel::Warn, .. })));
        assert!(parse_event("garbage").is_none());
        assert!(parse_event(r#"{"event":"future"}"#).is_none());
    }

    #[test]
    fn cmd_lines() {
        assert_eq!(HostCmd::SetQuality { quality: 80 }.to_line(), "{\"cmd\":\"set_quality\",\"quality\":80}\n");
        assert_eq!(HostCmd::RequestIdr.to_line(), "{\"cmd\":\"request_idr\"}\n");
        assert_eq!(HostCmd::RequestLog.to_line(), "{\"cmd\":\"request_log\"}\n");
        assert_eq!(HostCmd::NewOtp.to_line(), "{\"cmd\":\"new_otp\"}\n");
        assert_eq!(HostCmd::Connect { serial: "1-5".into() }.to_line(), "{\"cmd\":\"connect\",\"serial\":\"1-5\"}\n");
        assert_eq!(
            HostCmd::SetResolution { width: 1920, height: 1080 }.to_line(),
            "{\"cmd\":\"set_resolution\",\"width\":1920,\"height\":1080}\n"
        );
        assert_eq!(
            HostCmd::SetResolution { width: 0, height: 0 }.to_line(),
            "{\"cmd\":\"set_resolution\",\"width\":0,\"height\":0}\n"
        );
    }
}
