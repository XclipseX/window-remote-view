//! SudoVDA virtual display driver (SudoMaker, MIT; the IddCx driver Apollo ships).
//!
//! RemoteView creates its *own* monitor with it, so the host shows a real desktop even when the
//! laptop panel is broken/off and no other virtual display (TeamViewer, spacedesk) exists.
//!
//! Contract (verified against `Common/Include/sudovda-ioctl.h`, SudoVDA commit a4b09fa, and
//! Apollo `third-party/sudovda/sudovda.h`):
//! * device interface GUID `{e5bcc234-1e0c-418a-a0d4-ef8b7501414d}`, hardware id
//!   `root\sudomaker\sudovda`, display class `{4d36e968-e325-11ce-bfc1-08002be10318}`;
//! * IOCTLs = `CTL_CODE(FILE_DEVICE_UNKNOWN, fn, METHOD_BUFFERED, FILE_ANY_ACCESS)` with
//!   fn ADD 0x800, REMOVE 0x801, SET_RENDER_ADAPTER 0x802, GET_WATCHDOG 0x803, PING 0x888,
//!   GET_PROTOCOL_VERSION 0x8FF;
//! * ADD in = `{u32 w, u32 h, u32 hz, GUID monitor, char name[14], char serial[14]}` (56 B),
//!   ADD out = `{LUID adapter, u32 target_id}` (12 B);
//! * the driver's watchdog (default 3 s) removes every virtual monitor when pings stop, so a
//!   crashed RemoteView never leaves a stray monitor behind.

/// `CTL_CODE(FILE_DEVICE_UNKNOWN=0x22, func, METHOD_BUFFERED=0, FILE_ANY_ACCESS=0)`.
pub const fn ctl_code(func: u32) -> u32 {
    (0x22 << 16) | (func << 2)
}
pub const IOCTL_ADD: u32 = ctl_code(0x800);
pub const IOCTL_REMOVE: u32 = ctl_code(0x801);
pub const IOCTL_GET_WATCHDOG: u32 = ctl_code(0x803);
pub const IOCTL_PING: u32 = ctl_code(0x888);
pub const IOCTL_GET_VERSION: u32 = ctl_code(0x8FF);

pub const HARDWARE_ID: &str = r"Root\SudoMaker\SudoVDA";

/// Fixed monitor identity so Windows remembers scale/position for "the RemoteView display".
pub const MONITOR_GUID: u128 = 0x5256_5645_5701_4e9a_9d6b_5245_4d4f_5445;

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub fn driver_present() -> bool {
    false
}

/// Non-Windows placeholder (never constructed).
#[cfg(not(windows))]
pub struct SudoVdaMonitor {
    pub gdi_name: String,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use crate::vdisplay::DisplayMode;
    use crate::control::{self, Level};
    macro_rules! info {
        ($($a:tt)*) => { control::log(Level::Info, &format!($($a)*)) };
    }
    macro_rules! warn {
        ($($a:tt)*) => { control::log(Level::Warn, &format!($($a)*)) };
    }
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows::core::{GUID, PCWSTR};
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Devices::Display::*;
    use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LUID};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::DeviceIoControl;

    const IFACE: GUID = GUID::from_u128(0xe5bcc234_1e0c_418a_a0d4_ef8b7501414d);
    const DISPLAY_CLASS: GUID = GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318);

    #[repr(C)]
    struct AddParams {
        width: u32,
        height: u32,
        refresh: u32,
        guid: GUID,
        name: [u8; 14],
        serial: [u8; 14],
    }
    #[repr(C)]
    #[derive(Default)]
    struct AddOut {
        luid: LUID,
        target_id: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct WatchdogOut {
        timeout: u32,
        countdown: u32,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn wstr(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    /// Device paths of present SudoVDA interfaces.
    fn interface_paths() -> Vec<Vec<u16>> {
        let mut out = Vec::new();
        // SAFETY: standard SetupDi enumeration; the detail buffer is u32-aligned and sized as
        // the API asked; the set is destroyed before returning.
        unsafe {
            let Ok(set) = SetupDiGetClassDevsW(Some(&IFACE), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE) else {
                return out;
            };
            let mut i = 0;
            loop {
                let mut d = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInterfaces(set, None, &IFACE, i, &mut d).is_err() {
                    break;
                }
                i += 1;
                let mut need = 0u32;
                let _ = SetupDiGetDeviceInterfaceDetailW(set, &d, None, 0, Some(&mut need), None);
                if need < 8 {
                    continue;
                }
                let mut buf = vec![0u32; (need as usize).div_ceil(4)];
                let det = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*det).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(set, &d, Some(det), need, None, None).is_ok() {
                    let p = std::ptr::addr_of!((*det).DevicePath) as *const u16;
                    let n = (0..).take_while(|&k| *p.add(k) != 0).count();
                    let mut v = std::slice::from_raw_parts(p, n).to_vec();
                    v.push(0);
                    out.push(v);
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
        }
        out
    }

    /// True when the SudoVDA driver is installed and its device is running.
    pub fn driver_present() -> bool {
        !interface_paths().is_empty()
    }

    fn open_device() -> Result<HANDLE, String> {
        let paths = interface_paths();
        if paths.is_empty() {
            return Err("chưa cài driver màn hình ảo SudoVDA".into());
        }
        let mut last = String::new();
        for p in paths {
            // SAFETY: `p` is a NUL-terminated device path.
            match unsafe {
                CreateFileW(
                    PCWSTR(p.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            } {
                Ok(h) if !h.is_invalid() => return Ok(h),
                Ok(_) => last = "invalid handle".into(),
                Err(e) => last = e.to_string(),
            }
        }
        Err(format!("không mở được thiết bị SudoVDA: {last}"))
    }

    fn ioctl<I, O>(h: HANDLE, code: u32, inp: Option<&I>, out: Option<&mut O>) -> windows::core::Result<()> {
        let mut got = 0u32;
        let out_len = if out.is_some() { std::mem::size_of::<O>() as u32 } else { 0 };
        // SAFETY: buffers are plain repr(C) values valid for the call; synchronous handle.
        unsafe {
            DeviceIoControl(
                h,
                code,
                inp.map(|i| i as *const I as *const _),
                if inp.is_some() { std::mem::size_of::<I>() as u32 } else { 0 },
                out.map(|o| o as *mut O as *mut _),
                out_len,
                Some(&mut got),
                None,
            )
        }
    }

    #[derive(Clone, Copy)]
    struct SendHandle(HANDLE);
    // SAFETY: a kernel handle value may be used from any thread.
    unsafe impl Send for SendHandle {}
    unsafe impl Sync for SendHandle {}

    /// A RemoteView-owned virtual monitor. Dropping it removes the monitor.
    pub struct SudoVdaMonitor {
        h: SendHandle,
        stop: Arc<AtomicBool>,
        ping: Option<std::thread::JoinHandle<()>>,
        /// GDI name (`\\.\DISPLAYn`) of the new monitor.
        pub gdi_name: String,
    }

    impl SudoVdaMonitor {
        /// Adds a monitor with `mode`, keeps it alive with watchdog pings and waits until
        /// Windows has attached it to the desktop.
        pub fn create(mode: DisplayMode) -> Result<Self, String> {
            let h = open_device()?;
            let mut ver = [0u8; 4];
            if ioctl::<(), [u8; 4]>(h, IOCTL_GET_VERSION, None, Some(&mut ver)).is_ok() {
                info!("sudovda: protocol {}.{}.{}{}", ver[0], ver[1], ver[2], if ver[3] != 0 { " (test)" } else { "" });
            }
            let mut wd = WatchdogOut::default();
            let timeout = match ioctl::<(), WatchdogOut>(h, IOCTL_GET_WATCHDOG, None, Some(&mut wd)) {
                Ok(()) => wd.timeout,
                Err(_) => 3,
            };
            let guid = GUID::from_u128(MONITOR_GUID);
            // Remove a leftover monitor with our GUID (previous crash within the watchdog window).
            let _ = ioctl::<GUID, ()>(h, IOCTL_REMOVE, Some(&guid), None);
            let mut p = AddParams {
                width: mode.width,
                height: mode.height,
                refresh: mode.refresh_hz.max(30),
                guid,
                name: [0; 14],
                serial: [0; 14],
            };
            p.name[..10].copy_from_slice(b"RemoteView");
            p.serial[..8].copy_from_slice(b"RV000001");
            let mut out = AddOut::default();
            if let Err(e) = ioctl(h, IOCTL_ADD, Some(&p), Some(&mut out)) {
                // SAFETY: handle from CreateFileW, not used afterwards.
                unsafe {
                    let _ = CloseHandle(h);
                }
                return Err(format!("SudoVDA không tạo được màn hình {}x{}@{}: {e}", p.width, p.height, p.refresh));
            }
            info!(
                "sudovda: added {}x{}@{} target {} adapter {:08x}:{:08x} (watchdog {}s)",
                p.width, p.height, p.refresh, out.target_id, out.luid.HighPart, out.luid.LowPart, timeout
            );
            let stop = Arc::new(AtomicBool::new(false));
            let ping = {
                let (stop, sh) = (stop.clone(), SendHandle(h));
                let every = Duration::from_millis(if timeout == 0 { 1000 } else { (timeout as u64 * 1000 / 3).clamp(200, 1000) });
                std::thread::Builder::new()
                    .name("sudovda-ping".into())
                    .spawn(move || {
                        let mut fails = 0u32;
                        while !stop.load(Ordering::Relaxed) {
                            let sh = sh;
                            match ioctl::<(), ()>(sh.0, IOCTL_PING, None, None) {
                                Ok(()) => fails = 0,
                                Err(e) => {
                                    fails += 1;
                                    if fails == 1 || fails % 30 == 0 {
                                        warn!("sudovda: ping failed ({fails}x): {e}");
                                    }
                                }
                            }
                            std::thread::sleep(every);
                        }
                    })
                    .ok()
            };
            let mut me = SudoVdaMonitor { h: SendHandle(h), stop, ping, gdi_name: String::new() };
            // Windows attaches the new monitor asynchronously.
            let t0 = Instant::now();
            while t0.elapsed() < Duration::from_secs(10) {
                if let Some(n) = gdi_name_for(out.luid, out.target_id) {
                    me.gdi_name = n;
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if me.gdi_name.is_empty() {
                return Err("SudoVDA đã thêm màn hình nhưng Windows chưa bật nó sau 10 giây".into());
            }
            info!("sudovda: monitor is {} after {} ms", me.gdi_name, t0.elapsed().as_millis());
            Ok(me)
        }
    }

    impl Drop for SudoVdaMonitor {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(t) = self.ping.take() {
                let _ = t.join();
            }
            let guid = GUID::from_u128(MONITOR_GUID);
            match ioctl::<GUID, ()>(self.h.0, IOCTL_REMOVE, Some(&guid), None) {
                Ok(()) => info!("sudovda: removed {}", self.gdi_name),
                Err(e) => warn!("sudovda: remove failed: {e} (watchdog will remove it)"),
            }
            // SAFETY: handle owned by this struct.
            unsafe {
                let _ = CloseHandle(self.h.0);
            }
        }
    }

    /// GDI source name of the active path driving `target` on adapter `luid`.
    fn gdi_name_for(luid: LUID, target: u32) -> Option<String> {
        // SAFETY: standard QueryDisplayConfig usage with buffers sized by the API.
        unsafe {
            let (mut np, mut nm) = (0u32, 0u32);
            if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).0 != 0 {
                return None;
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).0 != 0 {
                return None;
            }
            paths.truncate(np as usize);
            for p in &paths {
                let a = p.targetInfo.adapterId;
                if a.LowPart != luid.LowPart || a.HighPart != luid.HighPart || p.targetInfo.id != target {
                    continue;
                }
                let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
                src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
                src.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
                src.header.adapterId = p.sourceInfo.adapterId;
                src.header.id = p.sourceInfo.id;
                if DisplayConfigGetDeviceInfo(&mut src.header) == 0 {
                    return Some(wstr(&src.viewGdiDeviceName));
                }
            }
        }
        None
    }

    /// Makes `gdi_name` the primary display (taskbar, new windows, sign-in screen) by moving
    /// it to (0,0) and shifting every other attached display by the same offset.
    pub fn make_primary(gdi_name: &str) -> Result<(), String> {
        // SAFETY: EnumDisplay*/ChangeDisplaySettingsEx with properly sized structs.
        unsafe {
            let cur = |name: &[u16]| -> Option<DEVMODEW> {
                let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
                EnumDisplaySettingsW(PCWSTR(name.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool().then_some(dm)
            };
            let target = wide(gdi_name);
            let tdm = cur(&target).ok_or("không đọc được vị trí màn hình ảo")?;
            let origin = tdm.Anonymous1.Anonymous2.dmPosition;
            let mut i = 0;
            loop {
                let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
                if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0).as_bool() {
                    break;
                }
                i += 1;
                if dd.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == DISPLAY_DEVICE_STATE_FLAGS(0) {
                    continue;
                }
                let name = wstr(&dd.DeviceName);
                let wname = wide(&name);
                let Some(mut dm) = cur(&wname) else { continue };
                let is_target = name.eq_ignore_ascii_case(gdi_name);
                let pos = dm.Anonymous1.Anonymous2.dmPosition;
                dm.Anonymous1.Anonymous2.dmPosition.x = if is_target { 0 } else { pos.x - origin.x };
                dm.Anonymous1.Anonymous2.dmPosition.y = if is_target { 0 } else { pos.y - origin.y };
                dm.dmFields = DM_POSITION;
                let mut flags = CDS_UPDATEREGISTRY | CDS_NORESET;
                if is_target {
                    flags |= CDS_SET_PRIMARY;
                }
                let r = ChangeDisplaySettingsExW(PCWSTR(wname.as_ptr()), Some(&dm), None, flags, None);
                if r != DISP_CHANGE_SUCCESSFUL {
                    warn!("make_primary: {name}: DISP_CHANGE {}", r.0);
                }
            }
            let r = ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None);
            if r != DISP_CHANGE_SUCCESSFUL {
                return Err(format!("ChangeDisplaySettingsEx apply: {}", r.0));
            }
        }
        info!("sudovda: {gdi_name} is now the primary display");
        Ok(())
    }

    // ---------------------------------------------------------------- driver install (admin)

    fn device_node_exists() -> bool {
        // SAFETY: SetupDi enumeration of the display class; buffers sized explicitly.
        unsafe {
            let Ok(set) = SetupDiGetClassDevsW(Some(&DISPLAY_CLASS), PCWSTR::null(), None, SETUP_DI_GET_CLASS_DEVS_FLAGS(0)) else {
                return false;
            };
            let mut found = false;
            let mut i = 0;
            loop {
                let mut d = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInfo(set, i, &mut d).is_err() {
                    break;
                }
                i += 1;
                let mut buf = [0u8; 1024];
                if SetupDiGetDeviceRegistryPropertyW(set, &d, SPDRP_HARDWAREID, None, Some(&mut buf), None).is_ok() {
                    let w: Vec<u16> = buf.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    if String::from_utf16_lossy(&w).to_ascii_lowercase().contains("sudomaker\\sudovda") {
                        found = true;
                        break;
                    }
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(set);
            found
        }
    }

    /// Installs the driver from `dir` (SudoVDA.inf, sudovda.cat, SudoVDA.dll, sudovda.cer).
    /// Must run elevated. Mirrors Apollo's install.bat (certutil + nefcon) with SetupAPI.
    pub fn install(dir: &std::path::Path) -> Result<(), String> {
        let inf = dir.join("SudoVDA.inf");
        let cer = dir.join("sudovda.cer");
        for f in [&inf, &cer, &dir.join("SudoVDA.dll"), &dir.join("sudovda.cat")] {
            if !f.exists() {
                return Err(format!("thiếu {}", f.display()));
            }
        }
        for store in ["root", "TrustedPublisher"] {
            let st = std::process::Command::new("certutil")
                .args(["-addstore", "-f", store])
                .arg(&cer)
                .status()
                .map_err(|e| format!("certutil: {e}"))?;
            if !st.success() {
                return Err(format!("certutil -addstore {store} lỗi {st}"));
            }
        }
        let hwid = wide(HARDWARE_ID);
        // SAFETY: devcon-style root device creation; all buffers outlive the calls.
        unsafe {
            if !device_node_exists() {
                let set = SetupDiCreateDeviceInfoList(Some(&DISPLAY_CLASS), None).map_err(|e| format!("CreateDeviceInfoList: {e}"))?;
                let mut d = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                let cls = wide("Display");
                let r = (|| -> Result<(), String> {
                    SetupDiCreateDeviceInfoW(set, PCWSTR(cls.as_ptr()), &DISPLAY_CLASS, PCWSTR::null(), None, DICD_GENERATE_ID, Some(&mut d))
                        .map_err(|e| format!("CreateDeviceInfo: {e}"))?;
                    // REG_MULTI_SZ: id\0\0
                    let mut ms: Vec<u16> = HARDWARE_ID.encode_utf16().collect();
                    ms.extend([0, 0]);
                    let bytes: Vec<u8> = ms.iter().flat_map(|c| c.to_le_bytes()).collect();
                    SetupDiSetDeviceRegistryPropertyW(set, &mut d, SPDRP_HARDWAREID, Some(&bytes)).map_err(|e| format!("SetHardwareId: {e}"))?;
                    SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set, Some(&d)).map_err(|e| format!("RegisterDevice: {e}"))?;
                    Ok(())
                })();
                let _ = SetupDiDestroyDeviceInfoList(set);
                r?;
            }
            let infw = wide(&inf.to_string_lossy());
            let mut reboot = windows::core::BOOL(0);
            UpdateDriverForPlugAndPlayDevicesW(None, PCWSTR(hwid.as_ptr()), PCWSTR(infw.as_ptr()), INSTALLFLAG_FORCE, Some(&mut reboot))
                .map_err(|e| format!("UpdateDriverForPlugAndPlayDevices: {e}"))?;
            if reboot.as_bool() {
                warn!("sudovda: Windows yêu cầu khởi động lại");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ioctl_codes_match_header() {
        // CTL_CODE(0x22, 0x800, 0, 0) == 0x222000
        assert_eq!(IOCTL_ADD, 0x0022_2000);
        assert_eq!(IOCTL_REMOVE, 0x0022_2004);
        assert_eq!(IOCTL_GET_WATCHDOG, 0x0022_200C);
        assert_eq!(IOCTL_PING, 0x0022_2220);
        assert_eq!(IOCTL_GET_VERSION, 0x0022_23FC);
    }
}
