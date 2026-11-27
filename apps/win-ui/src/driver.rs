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
        run_elevated(&me, &format!("{INSTALL_ALL_FLAG} \"{}\"", dir.display()), None, false)
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
            Ok(me) => run_elevated(&me, &format!("{INSTALL_ALL_FLAG} -"), None, false),
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

/// Writable working directory for `wdi-simple.exe` (extracted exe + CWD).
fn wdi_extract_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("RemoteView")
        .join("driver")
}

/// Writable destination directory passed via `--dest` to `wdi-simple.exe` so
/// libwdi never tries to extract driver files into `C:\Windows\System32`.
fn wdi_driver_dir() -> PathBuf {
    wdi_extract_dir().join("usb_driver")
}

/// Arguments for wdi-simple (pure; unit-tested).
///
/// 18D1:2D01 (accessory + adb) is a composite device: WinUSB must bind to the
/// accessory interface only (AOA interface 0; adb is interface 1), so it needs
/// `--iid 0`. 18D1:2D00 (accessory only) is a single-interface device, so
/// `--iid` must be left out there (libwdi would look for a non-existent MI_00).
pub fn wdi_args(vid: u16, pid: u16) -> String {
    wdi_args_with_dest(vid, pid, &wdi_driver_dir())
}

fn wdi_args_with_dest(vid: u16, pid: u16, dest: &std::path::Path) -> String {
    let iid = if (vid, pid) == (0x18D1, 0x2D01) { " --iid 0" } else { "" };
    format!(
        "--vid 0x{vid:04X} --pid 0x{pid:04X}{iid} --type 0 --name \"RemoteView USB\" --dest \"{}\"",
        dest.display()
    )
}

/// Writes `bytes` to `path` only when the existing file is missing or differs,
/// avoiding `ERROR_SHARING_VIOLATION` if a prior run of `wdi-simple.exe` is
/// still releasing its file handle.
#[cfg(any(feature = "embed-wdi", test))]
fn write_if_changed(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == bytes {
            return Ok(false);
        }
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

/// Decodes a `libwdi` `enum wdi_error` return value from `wdi-simple.exe`.
#[cfg(any(windows, test))]
fn wdi_error_name(code: i32) -> Option<&'static str> {
    match code {
        -1 => Some("WDI_ERROR_IO"),
        -2 => Some("WDI_ERROR_INVALID_PARAM"),
        -3 => Some("WDI_ERROR_ACCESS"),
        -4 => Some("WDI_ERROR_NO_DEVICE"),
        -5 => Some("WDI_ERROR_NOT_FOUND"),
        -6 => Some("WDI_ERROR_BUSY"),
        -7 => Some("WDI_ERROR_TIMEOUT"),
        -8 => Some("WDI_ERROR_OVERFLOW"),
        -9 => Some("WDI_ERROR_PENDING_INSTALLATION"),
        -10 => Some("WDI_ERROR_INTERRUPTED"),
        -11 => Some("WDI_ERROR_RESOURCE"),
        -12 => Some("WDI_ERROR_NOT_SUPPORTED"),
        -13 => Some("WDI_ERROR_EXISTS"),
        -14 => Some("WDI_ERROR_USER_CANCEL"),
        -15 => Some("WDI_ERROR_NEEDS_ADMIN"),
        -16 => Some("WDI_ERROR_WOW64"),
        -17 => Some("WDI_ERROR_INF_SYNTAX"),
        -18 => Some("WDI_ERROR_CAT_MISSING"),
        -19 => Some("WDI_ERROR_UNSIGNED"),
        -99 => Some("WDI_ERROR_OTHER"),
        _ => None,
    }
}

/// Formats a non-zero `wdi-simple.exe` exit code as signed `i32` with its
/// `WDI_ERROR_*` label when recognized.
#[cfg(any(windows, test))]
fn format_wdi_exit_code(code: u32) -> String {
    let signed = code as i32;
    match wdi_error_name(signed) {
        Some(name) => format!("trình cài driver thoát với mã {signed} ({name})"),
        None => format!("trình cài driver thoát với mã {signed}"),
    }
}

/// Where wdi-simple comes from: embedded bytes (extracted) or a sibling file.
fn locate_wdi() -> Result<PathBuf, String> {
    #[cfg(feature = "embed-wdi")]
    {
        let dir = wdi_extract_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("không tạo được thư mục cài driver: {e}"))?;
        let p = dir.join("wdi-simple.exe");
        write_if_changed(&p, WDI_SIMPLE)
            .map_err(|e| format!("không giải nén được wdi-simple.exe: {e}"))?;
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
        let work_dir = wdi_extract_dir();
        if let Err(e) = std::fs::create_dir_all(wdi_driver_dir()) {
            return DriverResult::Failed(format!("không tạo được thư mục cài driver: {e}"));
        }
        match locate_wdi() {
            Ok(exe) => run_elevated(&exe, &wdi_args(vid, pid), Some(&work_dir), true),
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
fn run_elevated(
    exe: &std::path::Path,
    params: &str,
    work_dir: Option<&std::path::Path>,
    is_wdi: bool,
) -> DriverResult {
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
    let dir_wide = work_dir.map(|d| wide(d.as_os_str()));
    let lp_directory = dir_wide
        .as_ref()
        .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args.as_ptr()),
        lpDirectory: lp_directory,
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: `info` and the wide strings it points to (`verb`, `file`, `args`,
    // `dir_wide`) outlive the call; the process handle returned by
    // SEE_MASK_NOCLOSEPROCESS is closed below.
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
        } else if is_wdi {
            DriverResult::Failed(format_wdi_exit_code(code))
        } else {
            DriverResult::Failed(format!("trình cài driver thoát với mã {}", code as i32))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_match_contract() {
        let dest = wdi_driver_dir();
        assert_eq!(
            wdi_args(0x18D1, 0x2D00),
            format!(
                "--vid 0x18D1 --pid 0x2D00 --type 0 --name \"RemoteView USB\" --dest \"{}\"",
                dest.display()
            )
        );
        assert_eq!(
            wdi_args(0x18D1, 0x2D01),
            format!(
                "--vid 0x18D1 --pid 0x2D01 --iid 0 --type 0 --name \"RemoteView USB\" --dest \"{}\"",
                dest.display()
            )
        );
    }

    #[test]
    fn wdi_dir_is_scoped() {
        let d = wdi_extract_dir();
        assert!(d.ends_with(std::path::Path::new("RemoteView").join("driver")));
        assert_eq!(wdi_driver_dir(), d.join("usb_driver"));
    }

    #[test]
    fn format_wdi_exit_code_decodes_signed_errors() {
        assert_eq!(
            format_wdi_exit_code((-11i32) as u32),
            "trình cài driver thoát với mã -11 (WDI_ERROR_RESOURCE)"
        );
        assert_eq!(
            format_wdi_exit_code((-3i32) as u32),
            "trình cài driver thoát với mã -3 (WDI_ERROR_ACCESS)"
        );
        assert_eq!(
            format_wdi_exit_code((-6i32) as u32),
            "trình cài driver thoát với mã -6 (WDI_ERROR_BUSY)"
        );
        assert_eq!(
            format_wdi_exit_code((-13i32) as u32),
            "trình cài driver thoát với mã -13 (WDI_ERROR_EXISTS)"
        );
        assert_eq!(
            format_wdi_exit_code((-14i32) as u32),
            "trình cài driver thoát với mã -14 (WDI_ERROR_USER_CANCEL)"
        );
        assert_eq!(
            format_wdi_exit_code(42),
            "trình cài driver thoát với mã 42"
        );
    }

    #[test]
    fn write_if_changed_skips_identical_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "rv-driver-test-{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("dummy.bin");
        let _ = std::fs::remove_file(&p);

        assert!(write_if_changed(&p, b"abc").unwrap());
        assert!(!write_if_changed(&p, b"abc").unwrap());
        assert!(write_if_changed(&p, b"abcd").unwrap());
        assert_eq!(std::fs::read(&p).unwrap(), b"abcd");

        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }

    #[cfg(not(windows))]
    #[test]
    fn not_windows_fails_cleanly() {
        assert!(matches!(install_winusb(0x18D1, 0x2D00), DriverResult::Failed(_)));
    }
}
