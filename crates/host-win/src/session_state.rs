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
#[cfg(windows)]
pub fn set_dpi_aware() {
    use windows::Win32::UI::HiDpi::*;
    unsafe {
        // Fails harmlessly when the GUI/manifest already set it.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

#[cfg(not(windows))]
pub fn set_dpi_aware() {}

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
