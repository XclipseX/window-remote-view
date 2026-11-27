//! Keeps the PC awake while sharing and detects a locked / secure desktop.
//!
//! When Windows is locked (or UAC / Ctrl+Alt+Del is showing) the input desktop
//! is `Winlogon`, a user-mode process can neither capture it (DXGI access lost,
//! `BitBlt: Access is denied`) nor inject input into it. The host then keeps
//! the last frame flowing and tells the user to unlock the laptop.

/// Message shown to the user (GUI status) while the desktop is locked.
pub const LOCKED_MESSAGE: &str = "Laptop đang khóa màn hình — hãy mở khóa laptop";

/// `true` when the input desktop named `name` is not the interactive one.
pub fn is_locked_desktop_name(name: &str) -> bool {
    !name.eq_ignore_ascii_case("default")
}

/// Whether the input desktop is the secure / lock desktop right now.
/// Failure to open the input desktop at all (access denied on Winlogon) counts as locked.
#[cfg(windows)]
pub fn desktop_locked() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::StationsAndDesktops::*;
    unsafe {
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else {
            return true;
        };
        let mut buf = [0u16; 64];
        let mut needed = 0u32;
        let r = GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            std::mem::size_of_val(&buf) as u32,
            Some(&mut needed),
        );
        let _ = CloseDesktop(desk);
        match r {
            Ok(()) => {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                is_locked_desktop_name(&String::from_utf16_lossy(&buf[..n]))
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
pub fn desktop_locked() -> bool {
    false
}

/// Name of the desktop `desk` refers to (`Default`, `Winlogon`, ...).
#[cfg(windows)]
fn desktop_name(desk: windows::Win32::System::StationsAndDesktops::HDESK) -> Option<String> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::StationsAndDesktops::*;
    let mut buf = [0u16; 64];
    let mut needed = 0u32;
    // SAFETY: `buf` / `needed` outlive the call; `desk` is a live desktop handle.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desk.0),
            UOI_NAME,
            Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
            std::mem::size_of_val(&buf) as u32,
            Some(&mut needed),
        )
        .ok()?;
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// `true` when this process runs as LocalSystem (token user SID `S-1-5-18`), i.e. when it can
/// open and attach to the Winlogon (lock / sign-in / UAC) desktop.
#[cfg(windows)]
pub fn running_as_system() -> bool {
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    static SYSTEM: OnceLock<bool> = OnceLock::new();
    *SYSTEM.get_or_init(|| unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_err() {
            return false;
        }
        let mut need = 0u32;
        let _ = GetTokenInformation(tok, TokenUser, None, 0, &mut need); // size query: fails by design
        let mut buf = vec![0u8; need.max(64) as usize];
        let ok = GetTokenInformation(tok, TokenUser, Some(buf.as_mut_ptr() as *mut core::ffi::c_void), buf.len() as u32, &mut need)
            .is_ok()
            // SAFETY: on success `buf` starts with a TOKEN_USER whose SID points into `buf`.
            && IsWellKnownSid((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid, WinLocalSystemSid).as_bool();
        let _ = CloseHandle(tok);
        ok
    })
}

#[cfg(not(windows))]
pub fn running_as_system() -> bool {
    false
}

#[cfg(windows)]
thread_local! {
    /// Desktop attached to the calling thread. The handle stays open: `CloseDesktop` fails while
    /// a thread uses it, and a closed handle must not stay attached.
    static ATTACHED: std::cell::RefCell<Option<(String, windows::Win32::System::StationsAndDesktops::HDESK)>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(windows)]
fn desktop_warn_gate() -> &'static std::sync::Mutex<crate::logfile::WarnGate> {
    use std::sync::{Mutex, OnceLock};
    static GATE: OnceLock<Mutex<crate::logfile::WarnGate>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(crate::logfile::WarnGate::new()))
}

#[cfg(windows)]
fn warn_desktop_err(msg: String) {
    if let Ok(mut g) = desktop_warn_gate().lock() {
        if g.should_log(&msg) {
            crate::control::log(crate::control::Level::Warn, &msg);
        }
    }
}

#[cfg(windows)]
fn clear_desktop_err() {
    if let Ok(mut g) = desktop_warn_gate().lock() {
        let _ = g.recovered();
    }
}

#[cfg(windows)]
fn open_input_desktop_handle(
) -> windows::core::Result<windows::Win32::System::StationsAndDesktops::HDESK> {
    use windows::Win32::System::StationsAndDesktops::*;
    let explicit = DESKTOP_ACCESS_FLAGS(
        DESKTOP_READOBJECTS.0
            | DESKTOP_WRITEOBJECTS.0
            | DESKTOP_CREATEWINDOW.0
            | DESKTOP_SWITCHDESKTOP.0
            | DESKTOP_HOOKCONTROL.0
            | DESKTOP_ENUMERATE.0,
    );
    const MAXIMUM_ALLOWED: DESKTOP_ACCESS_FLAGS = DESKTOP_ACCESS_FLAGS(0x0200_0000);
    // SAFETY: plain Win32 call returning a desktop handle owned by the caller.
    unsafe {
        OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, explicit)
            .or_else(|_| OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, MAXIMUM_ALLOWED))
    }
}

#[cfg(windows)]
fn follow_input_desktop_inner(force: bool) -> Option<String> {
    use windows::Win32::System::StationsAndDesktops::*;
    // SAFETY: plain Win32 calls; handles are owned by the thread-local until replaced.
    unsafe {
        let desk = match open_input_desktop_handle() {
            Ok(d) => d,
            Err(e) => {
                warn_desktop_err(format!("desktop: OpenInputDesktop failed: {e}"));
                return None;
            }
        };
        let Some(name) = desktop_name(desk) else {
            let _ = CloseDesktop(desk);
            return None;
        };
        ATTACHED.with(|a| {
            let mut a = a.borrow_mut();
            if !force && a.as_ref().is_some_and(|(n, _)| *n == name) {
                let _ = CloseDesktop(desk); // already attached to this desktop
                clear_desktop_err();
                return Some(name);
            }
            if let Err(e) = SetThreadDesktop(desk) {
                warn_desktop_err(format!("desktop: SetThreadDesktop({name}) failed: {e}"));
                let _ = CloseDesktop(desk);
                return None;
            }
            if let Some((_, old)) = a.replace((name.clone(), desk)) {
                let _ = CloseDesktop(old);
            }
            clear_desktop_err();
            Some(name)
        })
    }
}

/// Attaches the calling thread to the current *input* desktop (`OpenInputDesktop` +
/// `SetThreadDesktop`) and returns its name (`Default`, `Winlogon`, `Screen-saver`, ...).
/// Needed so DXGI duplication, GDI, cursor reads and `SendInput` of this thread see / reach the
/// desktop the user is looking at, including the lock screen. Only works from LocalSystem (a
/// normal user is denied on Winlogon) and when the thread owns no windows / hooks; returns
/// `None` on failure (the thread then stays where it was). Cheap when the desktop did not change.
#[cfg(windows)]
pub fn follow_input_desktop() -> Option<String> {
    follow_input_desktop_inner(false)
}

#[cfg(not(windows))]
pub fn follow_input_desktop() -> Option<String> {
    None
}

/// Like [`follow_input_desktop`], but always calls `SetThreadDesktop` even when
/// the input desktop name matches the previously attached one (handles recreated
/// `"Default"` desktops after logoff/logon).
#[cfg(windows)]
pub fn force_follow_input_desktop() -> Option<String> {
    follow_input_desktop_inner(true)
}

#[cfg(not(windows))]
pub fn force_follow_input_desktop() -> Option<String> {
    None
}

/// Per-thread helper that keeps a worker thread attached to the input desktop **when running
/// as SYSTEM** (a no-op otherwise, so behaviour for a normal user is unchanged).
pub struct DesktopFollower {
    tag: &'static str,
    log: bool,
    system: bool,
    every: std::time::Duration,
    last_check: Option<std::time::Instant>,
    current: Option<String>,
}

impl DesktopFollower {
    /// `every`: minimum time between checks. `log`: log desktop switches (one thread only).
    pub fn new(tag: &'static str, every: std::time::Duration, log: bool) -> Self {
        Self { tag, log, system: running_as_system(), every, last_check: None, current: None }
    }

    /// Whether following is active (running as SYSTEM).
    pub fn active(&self) -> bool {
        self.system
    }

    /// Name of the currently attached input desktop, if known.
    pub fn current_name(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Like [`poll`](Self::poll) but ignores the rate limit (call right before re-opening capture).
    pub fn poll_now(&mut self) -> bool {
        self.last_check = None;
        self.poll()
    }

    /// Unconditionally re-attaches the calling thread to the current input desktop
    /// (even if its name is unchanged, e.g. a recreated `"Default"` desktop after
    /// logoff/logon). Updates `self.current`, logs transitions, and returns `true`
    /// when re-attachment succeeded.
    pub fn force_poll(&mut self) -> bool {
        if !self.system {
            return false;
        }
        self.last_check = Some(std::time::Instant::now());
        let Some(name) = force_follow_input_desktop() else { return false };
        let changed = self.current.as_deref() != Some(name.as_str());
        if self.log {
            if changed {
                let from = self.current.as_deref().unwrap_or("?");
                crate::control::log(
                    crate::control::Level::Info,
                    &format!("desktop: {from} -> {name} ({})", self.tag),
                );
            } else {
                crate::control::log(
                    crate::control::Level::Info,
                    &format!("desktop: re-attached to {name} ({})", self.tag),
                );
            }
        }
        self.current = Some(name);
        true
    }

    /// Re-attaches the thread when the input desktop changed (rate-limited). Returns `true`
    /// when the thread moved to a different desktop since the previous call.
    pub fn poll(&mut self) -> bool {
        if !self.system {
            return false;
        }
        if self.last_check.is_some_and(|t| t.elapsed() < self.every) {
            return false;
        }
        self.last_check = Some(std::time::Instant::now());
        let Some(name) = follow_input_desktop() else { return false };
        if self.current.as_deref() == Some(name.as_str()) {
            return false;
        }
        if self.log {
            let from = self.current.as_deref().unwrap_or("?");
            crate::control::log(crate::control::Level::Info, &format!("desktop: {from} -> {name} ({})", self.tag));
        }
        self.current = Some(name);
        true
    }
}

/// Asks Windows not to sleep, turn the display off or start the screen saver
/// (`SetThreadExecutionState`, applies to the calling thread). `on = false` releases it.
#[cfg(windows)]
pub fn set_keep_awake(on: bool) {
    use windows::Win32::System::Power::*;
    unsafe {
        let flags = if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
        SetThreadExecutionState(flags);
    }
}

#[cfg(not(windows))]
pub fn set_keep_awake(_on: bool) {}

/// A zero-distance mouse move: resets the idle timers that drive the
/// screen-saver / "lock after N minutes" policies, invisible to the user.
#[cfg(windows)]
pub fn nudge_idle() {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    unsafe {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { dx: 0, dy: 0, mouseData: 0, dwFlags: MOUSEEVENTF_MOVE, time: 0, dwExtraInfo: 0 },
            },
        };
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(not(windows))]
pub fn nudge_idle() {}

/// Per-monitor DPI awareness so DXGI / cursor / injection coordinates are all physical pixels.
/// Returns a one-line description for the log (awareness 2 = per-monitor; system DPI 96 = 100%).
#[cfg(windows)]
pub fn set_dpi_aware() -> String {
    use windows::Win32::UI::HiDpi::*;
    unsafe {
        // Fails harmlessly when the GUI/manifest already set it.
        let r = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let aw = GetAwarenessFromDpiAwarenessContext(GetThreadDpiAwarenessContext());
        format!("dpi: set_per_monitor_v2={} awareness={} system_dpi={}", r.is_ok(), aw.0, GetDpiForSystem())
    }
}

#[cfg(not(windows))]
pub fn set_dpi_aware() -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_names() {
        assert!(!is_locked_desktop_name("Default"));
        assert!(!is_locked_desktop_name("default"));
        assert!(is_locked_desktop_name("Winlogon"));
        assert!(is_locked_desktop_name("Screen-saver"));
        assert!(is_locked_desktop_name(""));
    }
}
