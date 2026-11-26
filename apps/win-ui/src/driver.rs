//! Automatic WinUSB driver setup (replaces the manual Zadig steps).
//!
//! When an Android device (normal or accessory mode, 18D1:2D00/2D01) is present
//! but libusb cannot open it, `wdi-simple.exe` (built from libwdi, LGPL-3, the
//! library Zadig uses) installs WinUSB for exactly that VID:PID. It is embedded
//! into RemoteView.exe by the `embed-wdi` cargo feature (CI release builds) or
//! found next to the executable, extracted to %TEMP% and run elevated through
//! `ShellExecuteExW("runas")`.

use std::path::PathBuf;

#[cfg(feature = "embed-wdi")]
static WDI_SIMPLE: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/wdi-simple.exe"));

/// Result of one install attempt, delivered to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverResult {
    Installed,
    /// User declined the UAC prompt.
    Cancelled,
    Failed(String),
}

/// Arguments for wdi-simple (pure; unit-tested).
pub fn wdi_args(vid: u16, pid: u16) -> String {
    format!("--vid 0x{vid:04X} --pid 0x{pid:04X} --type 0 --name \"RemoteView USB\"")
}

/// Where wdi-simple comes from: embedded bytes (extracted) or a sibling file.
fn locate_wdi() -> Result<PathBuf, String> {
    #[cfg(feature = "embed-wdi")]
    {
        let dir = std::env::temp_dir().join("RemoteView");
        std::fs::create_dir_all(&dir).map_err(|e| format!("không tạo được thư mục tạm: {e}"))?;
        let p = dir.join("wdi-simple.exe");
        std::fs::write(&p, WDI_SIMPLE).map_err(|e| format!("không giải nén được wdi-simple.exe: {e}"))?;
        Ok(p)
    }
    #[cfg(not(feature = "embed-wdi"))]
    {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let p = me.parent().map(|d| d.join("wdi-simple.exe")).unwrap_or_default();
        if p.exists() {
            Ok(p)
        } else {
            Err("thiếu wdi-simple.exe (bản build này không kèm trình cài driver)".into())
        }
    }
}

/// Installs WinUSB for `vid:pid`. Blocking (UAC prompt + install); call from a thread.
pub fn install_winusb(vid: u16, pid: u16) -> DriverResult {
    #[cfg(windows)]
    {
        match locate_wdi() {
            Ok(exe) => run_elevated(&exe, &wdi_args(vid, pid)),
            Err(e) => DriverResult::Failed(e),
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (vid, pid, locate_wdi as fn() -> _);
        DriverResult::Failed("chỉ hỗ trợ cài driver trên Windows".into())
    }
}

#[cfg(windows)]
fn run_elevated(exe: &std::path::Path, params: &str) -> DriverResult {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let wide = |s: &std::ffi::OsStr| -> Vec<u16> { s.encode_wide().chain(Some(0)).collect() };
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let args = wide(params.as_ref());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the wide strings it points to outlive the call; the
    // process handle returned by SEE_MASK_NOCLOSEPROCESS is closed below.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            // ERROR_CANCELLED (1223) = user said no to UAC.
            return if e.code().0 as u32 & 0xFFFF == 1223 {
                DriverResult::Cancelled
            } else {
                DriverResult::Failed(format!("không chạy được trình cài driver: {e}"))
            };
        }
        let h = info.hProcess;
        if h.is_invalid() {
            return DriverResult::Failed("trình cài driver không trả về tiến trình".into());
        }
        // Installing + trusting the self-signed catalog can take a while.
        let waited = WaitForSingleObject(h, 180_000);
        let mut code = 1u32;
        let got = GetExitCodeProcess(h, &mut code);
        let _ = CloseHandle(h);
        if waited != WAIT_OBJECT_0 {
            return DriverResult::Failed("cài driver quá thời gian chờ".into());
        }
        if got.is_ok() && code == 0 {
            DriverResult::Installed
        } else {
            DriverResult::Failed(format!("wdi-simple thoát với mã {code}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_match_contract() {
        assert_eq!(wdi_args(0x18D1, 0x2D00), "--vid 0x18D1 --pid 0x2D00 --type 0 --name \"RemoteView USB\"");
        assert_eq!(wdi_args(0x18D1, 0x2D01), "--vid 0x18D1 --pid 0x2D01 --type 0 --name \"RemoteView USB\"");
    }

    #[cfg(not(windows))]
    #[test]
    fn not_windows_fails_cleanly() {
        assert!(matches!(install_winusb(0x18D1, 0x2D00), DriverResult::Failed(_)));
    }
}
