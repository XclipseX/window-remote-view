//! Persistent settings (JSON under the OS config dir / RemoteView).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Usb,
    Lan,
    Adb,
}

impl Mode {
    pub fn flag(self) -> &'static str {
        match self {
            Mode::Usb => "--usb",
            Mode::Lan => "--lan",
            Mode::Adb => "--adb",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Usb => "usb",
            Mode::Lan => "lan",
            Mode::Adb => "adb",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    #[default]
    Auto,
    Hevc,
    H264,
}
impl Codec {
    pub fn arg(self) -> &'static str {
        match self {
            Codec::Auto => "auto",
            Codec::Hevc => "hevc",
            Codec::H264 => "h264",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Encoder {
    #[default]
    Auto,
    Hw,
    Sw,
}
impl Encoder {
    pub fn arg(self) -> &'static str {
        match self {
            Encoder::Auto => "auto",
            Encoder::Hw => "hw",
            Encoder::Sw => "sw",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Display {
    #[default]
    Mirror,
    Extend,
}
impl Display {
    pub fn arg(self) -> &'static str {
        match self {
            Display::Mirror => "mirror",
            Display::Extend => "extend",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub mode: Mode,
    /// None = follow the receiving device.
    pub resolution: Option<(u32, u32)>,
    pub fps: u32,
    pub quality: u8,
    pub codec: Codec,
    pub encoder: Encoder,
    pub display: Display,
    pub autostart: bool,
    pub minimize_to_tray: bool,
    /// Check GitHub for a newer build on startup.
    pub auto_update_check: bool,
    /// Optional explicit path to rv-host executable.
    pub host_path: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Usb,
            resolution: None,
            fps: 60,
            quality: 80,
            codec: Codec::Auto,
            encoder: Encoder::Auto,
            display: Display::Mirror,
            autostart: false,
            minimize_to_tray: false,
            auto_update_check: true,
            host_path: None,
        }
    }
}

pub const RESOLUTIONS: &[(Option<(u32, u32)>, &str)] = &[
    (None, "Theo thiết bị nhận"),
    (Some((2800, 1752)), "2800 × 1752"),
    (Some((2560, 1600)), "2560 × 1600"),
    (Some((1920, 1200)), "1920 × 1200"),
    (Some((1920, 1080)), "1920 × 1080"),
    (Some((1280, 720)), "1280 × 720"),
];
pub const FPS_OPTIONS: &[u32] = &[30, 60, 90, 120, 144];

impl Config {
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("RemoteView").join("config.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = Self::path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    /// Command-line arguments for rv-host.
    pub fn host_args(&self) -> Vec<String> {
        let mut a = vec![self.mode.flag().to_string()];
        if let Some((w, h)) = self.resolution {
            a.extend(["--width".into(), w.to_string(), "--height".into(), h.to_string()]);
        }
        a.extend([
            "--fps".into(),
            self.fps.to_string(),
            "--quality".into(),
            self.quality.clamp(1, 100).to_string(),
            "--codec".into(),
            self.codec.arg().into(),
            "--encoder".into(),
            self.encoder.arg().into(),
            "--display".into(),
            self.display.arg().into(),
            "--json-status".into(),
        ]);
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&s).unwrap(), c);
        // partial / older files still load
        let p: Config = serde_json::from_str(r#"{"fps":120}"#).unwrap();
        assert_eq!(p.fps, 120);
        assert_eq!(p.quality, 80);
    }

    #[test]
    fn args() {
        let c = Config { resolution: Some((1920, 1080)), mode: Mode::Lan, ..Default::default() };
        let a = c.host_args();
        assert_eq!(a[0], "--lan");
        assert!(a.windows(2).any(|w| w == ["--width", "1920"]));
        assert_eq!(a.last().unwrap(), "--json-status");
    }
}
