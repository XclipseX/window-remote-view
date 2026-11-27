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

// SudoVDA virtual display driver (MIT, signed catalog + self-signed cert from the Apollo
// release), embedded by the `embed-vdd` feature (CI downloads it into assets/sudovda/).
#[cfg(feature = "embed-vdd")]
static VDD_FILES: &[(&str, &[u8])] = &[
    ("SudoVDA.inf", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sudovda/SudoVDA.inf"))),
    ("SudoVDA.dll", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sudovda/SudoVDA.dll"))),
    ("sudovda.cat", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sudovda/sudovda.cat"))),
    ("sudovda.cer", include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/sudovda/sudovda.cer"))),
];

/// Command-line flag of the elevated "install the virtual display driver" mode.
pub const INSTALL_VDD_FLAG: &str = "--install-vdd";

/// Folder holding the SudoVDA driver files: extracted from the exe, or `sudovda\` next to it.
fn locate_vdd() -> Result<PathBuf, String> {
    #[cfg(feature = "embed-vdd")]
    {
        let dir = std::env::temp_dir().join("RemoteView").join("sudovda");
        std::fs::create_dir_all(&dir).map_err(|e| format!("không tạo được {}: {e}", dir.display()))?;
        for (name, bytes) in VDD_FILES {
            std::fs::write(dir.join(name), bytes).map_err(|e| format!("không giải nén được {name}: {e}"))?;
        }
        Ok(dir)
    }
    #[cfg(not(feature = "embed-vdd"))]
    {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let d = me.parent().map(|d| d.join("sudovda")).unwrap_or_default();
        if d.join("SudoVDA.inf").exists() {
            Ok(d)
        } else {
            Err("bản build này không kèm driver màn hình ảo (thiếu thư mục sudovda)".into())
        }
    }
}

/// Whether this build can install the virtual display driver.
pub fn vdd_bundled() -> bool {
    cfg!(feature = "embed-vdd")
        || std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("sudovda").join("SudoVDA.inf").exists())).unwrap_or(false)
}

#[cfg(windows)]
use crate::service::INSTALL_ALL_FLAG;

/// Whether the RemoteView service (lock-screen support) is registered.
pub fn service_installed() -> bool {
    crate::service::service_installed()
}

/// Installs the SudoVDA driver **and** the RemoteView service with one UAC prompt: runs this exe
/// elevated with `--install-all <dir>`. Blocking.
pub fn install_vdd() -> DriverResult {
    #[cfg(windows)]
    {
        let dir = match locate_vdd() {
            Ok(d) => d,
            Err(e) => return DriverResult::Failed(e),
        };
        let me = match std::env::current_exe() {
            Ok(m) => m,
            Err(e) => return DriverResult::Failed(e.to_string()),
        };
        run_elevated(&me, &format!("{INSTALL_ALL_FLAG} \"{}\"", dir.display()))
    }
    #[cfg(not(windows))]
    {
        let _ = locate_vdd as fn() -> _;
        DriverResult::Failed("chỉ hỗ trợ cài driver trên Windows".into())
    }
}

/// Installs only the RemoteView service (driver already present): `--install-all -`. Blocking.
pub fn install_service() -> DriverResult {
    #[cfg(windows)]
    {
        match std::env::current_exe() {
            Ok(me) => run_elevated(&me, &format!("{INSTALL_ALL_FLAG} -")),
            Err(e) => DriverResult::Failed(e.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        DriverResult::Failed("chỉ hỗ trợ cài dịch vụ trên Windows".into())
    }
}

/// Result of one install attempt, delivered to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverResult {
    Installed,
    /// User declined the UAC prompt.
    Cancelled,
    Failed(String),
}

/// Arguments for wdi-simple (pure; unit-tested).
///
/// 18D1:2D01 (accessory + adb) is a composite device: WinUSB must bind to the
/// accessory interface only (AOA interface 0; adb is interface 1), so it needs
/// `--iid 0`. 18D1:2D00 (accessory only) is a single-interface device, so
/// `--iid` must be left out there (libwdi would look for a non-existent MI_00).
pub fn wdi_args(vid: u16, pid: u16) -> String {
    let iid = if (vid, pid) == (0x18D1, 0x2D01) { " --iid 0" } else { "" };
    format!("--vid 0x{vid:04X} --pid 0x{pid:04X}{iid} --type 0 --name \"RemoteView USB\"")
}

#[cfg(any(feature = "embed-wdi", test))]
fn wdi_extract_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("RemoteView")
        .join("driver")
}

/// Where wdi-simple comes from: embedded bytes (extracted) or a sibling file.
fn locate_wdi() -> Result<PathBuf, String> {
    #[cfg(feature = "embed-wdi")]
    {
        let dir = wdi_extract_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("không tạo được thư mục cài driver: {e}"))?;
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
    use windows::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess, WaitForSingleObject};
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
        if waited != WAIT_OBJECT_0 {
            let _ = TerminateProcess(h, 1);
            let _ = WaitForSingleObject(h, 2_000);
            let _ = CloseHandle(h);
            return DriverResult::Failed("cài driver quá thời gian chờ".into());
        }
        let mut code = 1u32;
        let got = GetExitCodeProcess(h, &mut code);
        let _ = CloseHandle(h);
        if got.is_ok() && code == 0 {
            DriverResult::Installed
        } else {
            DriverResult::Failed(format!("trình cài driver thoát với mã {code}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_match_contract() {
        assert_eq!(wdi_args(0x18D1, 0x2D00), "--vid 0x18D1 --pid 0x2D00 --type 0 --name \"RemoteView USB\"");
        assert_eq!(wdi_args(0x18D1, 0x2D01), "--vid 0x18D1 --pid 0x2D01 --iid 0 --type 0 --name \"RemoteView USB\"");
    }

    #[test]
    fn wdi_dir_is_scoped() {
        let d = wdi_extract_dir();
        assert!(d.ends_with(std::path::Path::new("RemoteView").join("driver")));
    }

    #[cfg(not(windows))]
    #[test]
    fn not_windows_fails_cleanly() {
        assert!(matches!(install_winusb(0x18D1, 0x2D00), DriverResult::Failed(_)));
    }
}
