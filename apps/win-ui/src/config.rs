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
    Mirror,
    Extend,
    #[default]
    /// RemoteView's own virtual monitor (SudoVDA), made the main display.
    Virtual,
}
impl Display {
    pub fn arg(self) -> &'static str {
        match self {
            Display::Mirror => "mirror",
            Display::Extend => "extend",
            Display::Virtual => "virtual",
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
    /// GDI name of the display to share (`\\.\DISPLAY1`); None = automatic (the laptop's own panel).
    pub monitor: Option<String>,
    pub autostart: bool,
    /// Start sharing as soon as the app opens (after the driver/service auto-install), so plugging
    /// the cable is enough.
    pub auto_share: bool,
    pub minimize_to_tray: bool,
    /// The user declined the automatic virtual display driver install (UAC) once.
    pub vdd_auto_declined: bool,
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
            fps: Preset::Auto.fps(),
            quality: Preset::Auto.quality(),
            codec: Codec::Auto,
            encoder: Encoder::Auto,
            display: Display::Virtual,
            monitor: None,
            autostart: false,
            auto_share: true,
            minimize_to_tray: false,
            vdd_auto_declined: false,
            auto_update_check: true,
            host_path: None,
        }
    }
}

/// One-click quality presets shown on the Home page. Everything else lives under "Nâng cao".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Auto,
    Smooth,
    Sharp,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Preset::Auto, Preset::Smooth, Preset::Sharp];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Auto => "Tự động (khuyên dùng)",
            Preset::Smooth => "Mượt nhất",
            Preset::Sharp => "Nét nhất",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
            Preset::Auto => "Theo độ phân giải máy bảng, tối đa 120 FPS (tự giảm nếu bộ mã hóa không kịp), chất lượng 85%.",
            Preset::Smooth => "Tối đa FPS, chất lượng 70% để luôn mượt.",
            Preset::Sharp => "Chất lượng 100%, 60 FPS: ảnh nét nhất.",
        }
    }
    /// Target frame rate (the host caps it to what the encoder can sustain).
    pub fn fps(self) -> u32 {
        match self {
            Preset::Auto | Preset::Smooth => 120,
            Preset::Sharp => 60,
        }
    }
    pub fn quality(self) -> u8 {
        match self {
            Preset::Auto => 85,
            Preset::Smooth => 70,
            Preset::Sharp => 100,
        }
    }
    /// Sets resolution (follow the device), fps, quality, codec and encoder; leaves the rest.
    pub fn apply(self, c: &mut Config) {
        c.resolution = None;
        c.fps = self.fps();
        c.quality = self.quality();
        c.codec = Codec::Auto;
        c.encoder = Encoder::Auto;
    }
    /// Which preset the current settings correspond to (None = custom, set under "Nâng cao").
    pub fn detect(c: &Config) -> Option<Preset> {
        Self::ALL.into_iter().find(|p| {
            c.resolution.is_none() && c.fps == p.fps() && c.quality == p.quality() && c.codec == Codec::Auto && c.encoder == Encoder::Auto
        })
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
        ]);
        if let Some(m) = self.monitor.as_deref().filter(|m| !m.trim().is_empty()) {
            a.extend(["--monitor".into(), m.to_string()]);
        }
        a.push("--json-status".into());
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
        assert_eq!(p.quality, 85);
    }

    #[test]
    fn defaults_and_old_config_back_compat() {
        let d = Config::default();
        assert!(d.auto_share);
        assert!(!d.autostart);
        assert_eq!(d.display, Display::Virtual);
        assert_eq!(Preset::detect(&d), Some(Preset::Auto));
        // A config.json written before `auto_share` / presets existed still loads.
        let old = r#"{"mode":"usb","resolution":null,"fps":60,"quality":80,"codec":"auto","encoder":"auto","display":"virtual","monitor":null,"autostart":true,"minimize_to_tray":false,"vdd_auto_declined":false,"auto_update_check":true,"host_path":null}"#;
        let c: Config = serde_json::from_str(old).unwrap();
        assert!(c.auto_share);
        assert!(c.autostart);
        assert_eq!((c.fps, c.quality), (60, 80));
        assert_eq!(Preset::detect(&c), None); // custom
        // Explicit opt-out survives a round trip.
        let off: Config = serde_json::from_str(r#"{"auto_share":false}"#).unwrap();
        assert!(!off.auto_share);
    }

    #[test]
    fn presets_apply_and_detect() {
        let mut c = Config { resolution: Some((1280, 720)), codec: Codec::H264, ..Default::default() };
        assert_eq!(Preset::detect(&c), None);
        Preset::Sharp.apply(&mut c);
        assert_eq!((c.fps, c.quality, c.resolution, c.codec), (60, 100, None, Codec::Auto));
        assert_eq!(Preset::detect(&c), Some(Preset::Sharp));
        Preset::Smooth.apply(&mut c);
        assert_eq!((c.fps, c.quality), (120, 70));
        assert_eq!(Preset::detect(&c), Some(Preset::Smooth));
        Preset::Auto.apply(&mut c);
        assert_eq!((c.fps, c.quality), (120, 85));
        assert!(c.host_args().windows(2).any(|w| w == ["--fps", "120"]));
    }

    #[test]
    fn args() {
        let c = Config { resolution: Some((1920, 1080)), mode: Mode::Lan, ..Default::default() };
        let a = c.host_args();
        assert_eq!(a[0], "--lan");
        assert!(a.windows(2).any(|w| w == ["--width", "1920"]));
        assert_eq!(a.last().unwrap(), "--json-status");
        assert!(!a.contains(&"--monitor".to_string()));
        let c = Config { monitor: Some(r"\\.\DISPLAY1".into()), ..Default::default() };
        let a = c.host_args();
        assert!(a.windows(2).any(|w| w == ["--monitor", r"\\.\DISPLAY1"]));
        assert_eq!(a.last().unwrap(), "--json-status");
    }
}
