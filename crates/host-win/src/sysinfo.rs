//! Startup report for the log files: version, commit, OS build, GPUs, outputs.

use crate::capture::{list_adapters, list_outputs, pick_output_index, AdapterInfo};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Short git commit this build was made from (`unknown` outside git / CI).
pub const GIT_SHA: &str = env!("RV_GIT_SHA");

/// Windows version string (`cmd /c ver`), or the OS name elsewhere.
pub fn os_version() -> String {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Ok(o) = std::process::Command::new("cmd").args(["/c", "ver"]).creation_flags(0x0800_0000).output() {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn describe_adapter(i: usize, a: &AdapterInfo) -> String {
    format!(
        "GPU #{i}: {} (vendor 0x{:04x}, device 0x{:04x}, {} MB VRAM{})",
        a.name,
        a.vendor_id,
        a.device_id,
        a.dedicated_mb,
        if a.software { ", phần mềm" } else { "" }
    )
}

/// Lines to log once at startup (also useful from the GUI process).
pub fn header_lines(app: &str) -> Vec<String> {
    vec![
        format!("{app} v{VERSION} (commit {GIT_SHA})"),
        format!("OS: {}", os_version()),
        format!("Máy: {}", crate::control::computer_name()),
    ]
}

/// Header + every GPU adapter + every output (with the default pick marked).
pub fn startup_report(app: &str) -> Vec<String> {
    let mut v = header_lines(app);
    for (i, a) in list_adapters().iter().enumerate() {
        v.push(describe_adapter(i, a));
    }
    match list_outputs() {
        Ok(outs) => {
            let pick = pick_output_index(&outs, None);
            if outs.is_empty() {
                v.push("Không thấy màn hình nào qua DXGI".into());
            }
            for (i, o) in outs.iter().enumerate() {
                let mark = if pick == Some(i) { "  <== mặc định" } else { "" };
                v.push(format!("Output #{i}: {}{mark}", o.describe()));
            }
        }
        Err(e) => v.push(format!("Không liệt kê được màn hình: {e}")),
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_has_version_and_sha() {
        let h = header_lines("RemoteView");
        assert!(h[0].contains(VERSION) && h[0].contains("commit"));
        assert!(!GIT_SHA.is_empty());
        assert!(!os_version().is_empty());
        let a = AdapterInfo { name: "Intel".into(), vendor_id: 0x8086, device_id: 0xa7a0, dedicated_mb: 128, software: false };
        assert!(describe_adapter(0, &a).contains("0x8086"));
    }
}
