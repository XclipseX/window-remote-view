//! Small helpers: icon, URL opening, local IP, QR payload, autostart.

use crate::config::Mode;

pub const ADB_PORT: u16 = 47810;
pub const LAN_PORT: u16 = 47800;

pub fn port_for(mode: Mode) -> u16 {
    match mode {
        Mode::Usb => 0,
        Mode::Lan => LAN_PORT,
        Mode::Adb => ADB_PORT,
    }
}

/// Payload shown in the QR code: {pin, host, port, mode}.
pub fn qr_payload(pin: &str, host: &str, port: u16, mode: Mode) -> String {
    serde_json::json!({ "pin": pin, "host": host, "port": port, "mode": mode.name() }).to_string()
}

/// Best-effort LAN IPv4 (no packet is sent by a UDP "connect").
pub fn local_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("192.0.2.1:9")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".into())
}

pub fn open_url(url: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd")
            .args(["/c", "start", "", url])
            .creation_flags(0x0800_0000)
            .spawn();
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

/// Programmatic app icon: blue rounded square with a white "screen" and a play wedge.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let inside_round = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    for yi in 0..size {
        for xi in 0..size {
            let (x, y) = (xi as f32 + 0.5, yi as f32 + 0.5);
            let i = ((yi * size + xi) * 4) as usize;
            if !inside_round(x, y, 0.0, 0.0, s, s, s * 0.22) {
                continue;
            }
            let t = y / s;
            let mut c = [
                (40.0 + 30.0 * t) as u8,
                (110.0 - 20.0 * t) as u8,
                (240.0 - 30.0 * t) as u8,
                255,
            ];
            // screen outline
            let outer = inside_round(x, y, s * 0.16, s * 0.24, s * 0.84, s * 0.66, s * 0.06);
            let inner = inside_round(x, y, s * 0.22, s * 0.30, s * 0.78, s * 0.60, s * 0.03);
            if outer && !inner {
                c = [255, 255, 255, 255];
            }
            // stand
            if (s * 0.40..s * 0.60).contains(&x) && (s * 0.66..s * 0.74).contains(&y)
                || (s * 0.30..s * 0.70).contains(&x) && (s * 0.74..s * 0.80).contains(&y)
            {
                c = [255, 255, 255, 255];
            }
            // play wedge
            let (px0, py0) = (s * 0.43, s * 0.36);
            let py1 = s * 0.54;
            let tipx = s * 0.60;
            if x >= px0 && x <= tipx {
                let f = (x - px0) / (tipx - px0);
                let mid = (py0 + py1) / 2.0;
                let half = (py1 - py0) / 2.0 * (1.0 - f);
                if (y - mid).abs() <= half {
                    c = [255, 255, 255, 255];
                }
            }
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    px
}

#[cfg(windows)]
pub fn autostart_supported() -> bool {
    true
}
#[cfg(not(windows))]
pub fn autostart_supported() -> bool {
    false
}

#[cfg(windows)]
pub fn set_autostart(enable: bool) -> Result<(), String> {
    use winreg::{enums::*, RegKey};
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(r"Software\Microsoft\Windows\CurrentVersion\Run")
        .map_err(|e| e.to_string())?
        .0;
    if enable {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value("RemoteView", &format!("\"{}\" --minimized", exe.display()))
            .map_err(|e| e.to_string())
    } else {
        match key.delete_value("RemoteView") {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}
#[cfg(not(windows))]
pub fn set_autostart(_enable: bool) -> Result<(), String> {
    Err("Chỉ hỗ trợ trên Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_payload_is_json() {
        assert_eq!(port_for(Mode::Lan), 47800);
        let p = qr_payload("123456", "192.168.1.5", LAN_PORT, Mode::Lan);
        let v: serde_json::Value = serde_json::from_str(&p).unwrap();
        assert_eq!(v["pin"], "123456");
        assert_eq!(v["port"], 47800);
        assert_eq!(v["mode"], "lan");
    }

    #[test]
    fn icon_has_expected_size() {
        let i = icon_rgba(64);
        assert_eq!(i.len(), 64 * 64 * 4);
        assert_eq!(i[3], 0); // transparent corner
        assert!(i.chunks(4).any(|p| p == [255, 255, 255, 255]));
    }
}
