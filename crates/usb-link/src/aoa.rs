//! Host side of Android Open Accessory (AOA) over libusb (`rusb`).
//!
//! Sequence (per source.android.com/docs/core/interaction/accessories/aoa):
//! 1. For an Android device send GET_PROTOCOL (51); need >= 1.
//! 2. Send identification strings (52, index 0..5) then START (53).
//! 3. Device drops off the bus and re-enumerates as vendor 0x18D1, product
//!    0x2D00 (accessory) or 0x2D01 (accessory + adb).
//! 4. Open it, claim the interface, use its bulk IN/OUT endpoints.
//!
//! UNVERIFIED against real hardware. Windows caveat: libusb needs the WinUSB
//! driver bound to the device. The phone in normal (MTP) mode usually is not,
//! so step 1 fails with `NotSupported`/`Access` until WinUSB is bound to the
//! device. RemoteView.exe installs it automatically (libwdi, see
//! `apps/win-ui/src/driver.rs`) for the 18D1:2D00/2D01 accessory device. On Linux/macOS no driver work is needed (udev rule on Linux).

use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rusb::{Context, Device, DeviceHandle, Direction, Recipient, RequestType, UsbContext};

use crate::{LinkError, Result};

pub const ACCESSORY_VID: u16 = 0x18D1;
pub const ACCESSORY_PID: u16 = 0x2D00;
pub const ACCESSORY_ADB_PID: u16 = 0x2D01;

const GET_PROTOCOL: u8 = 51;
const SEND_STRING: u8 = 52;
const START: u8 = 53;

/// Vendor IDs commonly seen on Android devices.
pub const ANDROID_VIDS: &[u16] = &[
    0x04E8, // Samsung
    0x18D1, // Google
    0x2717, // Xiaomi
    0x12D1, // Huawei
    0x22B8, // Motorola
    0x2A70, // OnePlus
    0x1004, // LG
    0x0FCE, // Sony
    0x0BB4, // HTC
    0x0B05, // ASUS
    0x22D9, // OPPO
    0x17EF, // Lenovo
];

/// Identification strings; the Android app's accessory_filter.xml must match
/// manufacturer + model (and the app matches version for its own use).
#[derive(Debug, Clone)]
pub struct AccessoryInfo {
    pub manufacturer: String,
    pub model: String,
    pub description: String,
    pub version: String,
    pub uri: String,
    pub serial: String,
}

/// Shared contract with the Android app (accessory_filter.xml + pairing).
pub const AOA_MANUFACTURER: &str = "RemoteView";
pub const AOA_MODEL: &str = "RemoteViewHost";
pub const AOA_VERSION: &str = "2";
pub const AOA_URI: &str = "https://github.com/XclipseX/window-remote-view/releases/tag/latest-build";

impl AccessoryInfo {
    /// `computer_name` becomes the description; `token` (8 digits) is sent as
    /// the AOA serial and doubles as the session OTP for the USB link.
    pub fn new(computer_name: &str, token: &str) -> Self {
        Self {
            manufacturer: AOA_MANUFACTURER.into(),
            model: AOA_MODEL.into(),
            description: computer_name.chars().take(100).collect(),
            version: AOA_VERSION.into(),
            uri: AOA_URI.into(),
            serial: token.into(),
        }
    }
}

impl Default for AccessoryInfo {
    fn default() -> Self {
        Self::new("RemoteView second display host", "00000000")
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub vid: u16,
    pub pid: u16,
    pub bus: u8,
    pub address: u8,
    /// USB string descriptors (only readable when libusb can open the device).
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    /// libusb could open the device (WinUSB/udev permissions OK).
    pub can_open: bool,
}

impl Candidate {
    pub fn is_accessory(&self) -> bool {
        is_accessory(self.vid, self.pid)
    }
}

fn is_accessory(vid: u16, pid: u16) -> bool {
    vid == ACCESSORY_VID && (pid == ACCESSORY_PID || pid == ACCESSORY_ADB_PID)
}

/// Lists attached USB devices that look like Android (or all, if `scan_all`).
pub fn list_candidates(scan_all: bool) -> Result<Vec<Candidate>> {
    list_candidates_opts(scan_all, true)
}

/// Classifies a `Device::open()` failure during candidate discovery:
/// - `None`: device vanished mid-scan (`NoDevice`), skip stale entry.
/// - `Some(true)`: driver is bound (`Access`/`Busy` — on Windows WinUSB opens are
///   exclusive, so an active host worker causes `Access`/`Busy`, not `NotSupported`).
/// - `Some(false)`: driver missing or unusable (`NotSupported`, etc.).
fn classify_open_error(err: rusb::Error) -> Option<bool> {
    match err {
        rusb::Error::NoDevice => None,
        rusb::Error::Access | rusb::Error::Busy => Some(true),
        _ => Some(false),
    }
}

/// Like [`list_candidates`]. With `probe_accessory = false` devices already in
/// accessory mode are not opened (they may be in use by a live link; a second
/// open could fail or disturb it) and are reported with `can_open = true`.
pub fn list_candidates_opts(scan_all: bool, probe_accessory: bool) -> Result<Vec<Candidate>> {
    let _g = usb_lock();
    let ctx = Context::new()?;
    let mut out = Vec::new();
    for d in ctx.devices()?.iter() {
        let Ok(desc) = d.device_descriptor() else { continue };
        // Class 9 = hub; never poke those.
        if desc.class_code() == 9 {
            continue;
        }
        if scan_all || ANDROID_VIDS.contains(&desc.vendor_id()) {
            let (manufacturer, product, can_open);
            if !probe_accessory && is_accessory(desc.vendor_id(), desc.product_id()) {
                (manufacturer, product, can_open) = (None, None, true);
            } else {
                match d.open() {
                    Ok(h) => {
                        can_open = true;
                        manufacturer = h.read_manufacturer_string_ascii(&desc).ok();
                        product = h.read_product_string_ascii(&desc).ok();
                    }
                    Err(e) => match classify_open_error(e) {
                        Some(usable) => {
                            (manufacturer, product, can_open) = (None, None, usable);
                        }
                        None => continue,
                    },
                }
            }
            out.push(Candidate {
                vid: desc.vendor_id(),
                pid: desc.product_id(),
                bus: d.bus_number(),
                address: d.address(),
                manufacturer,
                product,
                can_open,
            });
        }
    }
    Ok(out)
}

static USB_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serialises device opens inside this process (scanner thread vs. connect).
fn usb_lock() -> std::sync::MutexGuard<'static, ()> {
    USB_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn get_protocol(h: &DeviceHandle<Context>) -> Result<u16> {
    let rt = rusb::request_type(Direction::In, RequestType::Vendor, Recipient::Device);
    let mut b = [0u8; 2];
    let n = h.read_control(rt, GET_PROTOCOL, 0, 0, &mut b, Duration::from_secs(1))?;
    if n != 2 {
        return Err(LinkError::AoaUnsupported(0));
    }
    Ok(u16::from_le_bytes(b))
}

fn send_identity(h: &DeviceHandle<Context>, info: &AccessoryInfo) -> Result<()> {
    let rt = rusb::request_type(Direction::Out, RequestType::Vendor, Recipient::Device);
    let strings = [
        &info.manufacturer,
        &info.model,
        &info.description,
        &info.version,
        &info.uri,
        &info.serial,
    ];
    for (i, s) in strings.iter().enumerate() {
        let mut data = s.as_bytes().to_vec();
        data.push(0); // NUL terminated
        h.write_control(rt, SEND_STRING, 0, i as u16, &data, Duration::from_secs(1))?;
    }
    h.write_control(rt, START, 0, 0, &[], Duration::from_secs(1))?;
    Ok(())
}

/// Opened accessory with claimed interface and bulk endpoints.
pub struct AoaDevice {
    handle: Arc<DeviceHandle<Context>>,
    ep_in: u8,
    ep_out: u8,
    iface: u8,
    bus: u8,
    address: u8,
}

/// Where an opened accessory lives (for link-stats diagnostics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeviceLocation {
    pub bus: u8,
    pub address: u8,
    pub iface: u8,
    pub ep_in: u8,
    pub ep_out: u8,
}

impl AoaDevice {
    pub fn location(&self) -> DeviceLocation {
        DeviceLocation { bus: self.bus, address: self.address, iface: self.iface, ep_in: self.ep_in, ep_out: self.ep_out }
    }

    /// Finds an Android device, switches it to accessory mode if needed and
    /// opens it. Returns `NoDevice` when nothing suitable is attached.
    ///
    /// A device already in accessory mode is simply opened: the pairing token
    /// (AOA serial) is persistent per Windows user, so an accessory left from
    /// an earlier run still matches. There is deliberately NO reset/re-switch
    /// logic: it could make the host read from a different device instance
    /// than the one Android opened.
    pub fn connect(info: &AccessoryInfo, scan_all: bool, timeout: Duration) -> Result<Self> {
        if let Some(dev) = Self::try_open_existing_accessory(info)? {
            return Ok(dev);
        }
        Self::switch_normal_device(info, scan_all, timeout)
    }

    /// If a device is already in accessory mode (`18D1:2D00/2D01`), open it.
    /// Retries transient `Access`/`Busy`/`NoDevice`/`Io`/`Pipe` errors for up
    /// to ~1.5 s (every 150 ms) without holding `USB_LOCK` across sleeps to
    /// survive brief PnP or previous-handle teardown windows.
    fn try_open_existing_accessory(info: &AccessoryInfo) -> Result<Option<Self>> {
        let initial = {
            let _g = usb_lock();
            let ctx = Context::new()?;
            find_accessory(&ctx)?.map(|dev| Self::open(dev, Some(&info.serial)))
        };
        let mut last_err = match initial {
            None => return Ok(None),
            Some(Ok(d)) => return Ok(Some(d)),
            Some(Err(e)) if is_transient_open_err(&e) => e,
            Some(Err(e)) => return Err(e),
        };
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(150));
            let attempt = {
                let _g = usb_lock();
                let ctx = Context::new()?;
                find_accessory(&ctx)?.map(|dev| Self::open(dev, Some(&info.serial)))
            };
            if let Some(res) = attempt {
                match res {
                    Ok(d) => return Ok(Some(d)),
                    Err(e) if is_transient_open_err(&e) => last_err = e,
                    Err(e) => return Err(e),
                }
            }
        }
        Err(last_err)
    }

    fn switch_normal_device(info: &AccessoryInfo, scan_all: bool, timeout: Duration) -> Result<Self> {
        let switched = {
            let _g = usb_lock();
            let ctx = Context::new()?;
            let mut last_err = LinkError::NoDevice;
            let mut did_switch = false;
            for d in ctx.devices()?.iter() {
                let Ok(desc) = d.device_descriptor() else { continue };
                if desc.class_code() == 9
                    || !(scan_all || ANDROID_VIDS.contains(&desc.vendor_id()))
                {
                    continue;
                }
                let h = match d.open() {
                    Ok(h) => h,
                    Err(e) => {
                        last_err = e.into();
                        continue;
                    }
                };
                match get_protocol(&h) {
                    Ok(p) if p >= 1 => {}
                    Ok(p) => {
                        last_err = LinkError::AoaUnsupported(p);
                        continue;
                    }
                    Err(e) => {
                        last_err = e;
                        continue;
                    }
                }
                if let Err(e) = send_identity(&h, info) {
                    last_err = e;
                    continue;
                }
                drop(h);
                did_switch = true;
                break;
            }
            if did_switch {
                Ok(())
            } else {
                Err(last_err)
            }
        };
        switched?;
        Self::wait_reenumerate(timeout, &info.serial)
    }

    fn wait_reenumerate(timeout: Duration, token: &str) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        // Give the device a moment to detach before polling (without holding USB_LOCK).
        std::thread::sleep(Duration::from_millis(500));
        loop {
            let attempt = {
                let _g = usb_lock();
                // Fresh libusb Context per poll so Windows PnP arrival of 18D1:2D00/2D01 is seen.
                let ctx = Context::new()?;
                find_accessory(&ctx)?.map(|dev| Self::open(dev, Some(token)))
            };
            if let Some(res) = attempt {
                // Retry open briefly: node may exist before it is usable.
                match res {
                    Ok(d) => return Ok(d),
                    Err(e) if Instant::now() >= deadline => return Err(e),
                    Err(_) => {}
                }
            }
            if Instant::now() >= deadline {
                return Err(LinkError::ReenumerateTimeout);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn open(dev: Device<Context>, _token: Option<&str>) -> Result<Self> {
        let cfg = dev.active_config_descriptor()?;
        let mut found = None;
        'outer: for iface in cfg.interfaces() {
            for alt in iface.descriptors() {
                let (mut i, mut o) = (None, None);
                for ep in alt.endpoint_descriptors() {
                    if ep.transfer_type() != rusb::TransferType::Bulk {
                        continue;
                    }
                    match ep.direction() {
                        Direction::In => i = i.or(Some(ep.address())),
                        Direction::Out => o = o.or(Some(ep.address())),
                    }
                }
                if let (Some(i), Some(o)) = (i, o) {
                    // Prefer the vendor-specific accessory interface (class 0xFF);
                    // with 2D01 the adb interface is class 0xFF subclass 0x42, skip it.
                    if alt.class_code() == 0xFF && alt.sub_class_code() == 0x42 {
                        continue;
                    }
                    found = Some((alt.interface_number(), i, o));
                    break 'outer;
                }
            }
        }
        let (iface, ep_in, ep_out) = found.ok_or(LinkError::NoEndpoints)?;
        let handle = dev.open()?;
        let _ = handle.set_auto_detach_kernel_driver(true); // unsupported on Windows
        handle.claim_interface(iface)?;
        // Clear any stale halt/toggle state left by an earlier session.
        let _ = handle.clear_halt(ep_in);
        let _ = handle.clear_halt(ep_out);
        let (bus, address) = (dev.bus_number(), dev.address());
        // Do NOT compare the USB device descriptor's `iSerialNumber` against `_token`.
        // AOA `SEND_STRING(5)` only sets the accessory serial visible to Android's
        // `UsbAccessory.getSerial()`; the USB descriptor's `iSerialNumber` remains the
        // device's hardware serial (e.g. `R52Y106D7TR`), which `adb` needs on `18D1:2D01`.
        Ok(Self {
            handle: Arc::new(handle),
            ep_in,
            ep_out,
            iface,
            bus,
            address,
        })
    }

    /// Splits into blocking `Read` / `Write` halves (bulk transfers).
    pub fn into_halves(self) -> (BulkReader, BulkWriter) {
        let guard = Arc::new(Release {
            handle: self.handle.clone(),
            iface: self.iface,
        });
        (
            BulkReader {
                h: self.handle.clone(),
                ep: self.ep_in,
                _g: guard.clone(),
            },
            BulkWriter {
                h: self.handle,
                ep: self.ep_out,
                _g: guard,
            },
        )
    }
}

fn is_transient_open_err(err: &LinkError) -> bool {
    matches!(
        err,
        LinkError::Usb(
            rusb::Error::Access
                | rusb::Error::Busy
                | rusb::Error::NoDevice
                | rusb::Error::Io
                | rusb::Error::Pipe
        )
    )
}

fn find_accessory(ctx: &Context) -> Result<Option<Device<Context>>> {
    for d in ctx.devices()?.iter() {
        if let Ok(desc) = d.device_descriptor() {
            if is_accessory(desc.vendor_id(), desc.product_id()) {
                return Ok(Some(d));
            }
        }
    }
    Ok(None)
}

struct Release {
    handle: Arc<DeviceHandle<Context>>,
    iface: u8,
}
impl Drop for Release {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.iface);
    }
}

pub struct BulkReader {
    h: Arc<DeviceHandle<Context>>,
    ep: u8,
    _g: Arc<Release>,
}

pub struct BulkWriter {
    h: Arc<DeviceHandle<Context>>,
    ep: u8,
    _g: Arc<Release>,
}

fn to_io(e: rusb::Error) -> std::io::Error {
    use std::io::ErrorKind::*;
    let kind = match e {
        rusb::Error::Timeout => TimedOut,
        rusb::Error::Interrupted => Interrupted,
        rusb::Error::NoDevice | rusb::Error::Io | rusb::Error::Pipe => BrokenPipe,
        _ => Other,
    };
    std::io::Error::new(kind, e)
}

impl Read for BulkReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // Short timeout so UsbDatagramLink::drop joins promptly without racing reconnect.
        self.h
            .read_bulk(self.ep, buf, Duration::from_millis(150))
            .map_err(to_io)
    }
}

impl Write for BulkWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // A timeout here may mean a partial transfer; libusb reports how much
        // was sent only via the Ok path, so treat Timeout as a hard error.
        // 5 s is generous: it only fires if the tablet stops reading.
        self.h
            .write_bulk(self.ep, buf, Duration::from_secs(5))
            .map_err(to_io)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_open_error_distinguishes_driver_missing_from_in_use() {
        assert_eq!(classify_open_error(rusb::Error::NotSupported), Some(false));
        assert_eq!(classify_open_error(rusb::Error::Access), Some(true));
        assert_eq!(classify_open_error(rusb::Error::Busy), Some(true));
        assert_eq!(classify_open_error(rusb::Error::NoDevice), None);
    }

    #[test]
    fn transient_open_errors_match_expected_variants() {
        assert!(is_transient_open_err(&LinkError::Usb(rusb::Error::Access)));
        assert!(is_transient_open_err(&LinkError::Usb(rusb::Error::Busy)));
        assert!(is_transient_open_err(&LinkError::Usb(rusb::Error::NoDevice)));
        assert!(is_transient_open_err(&LinkError::Usb(rusb::Error::Io)));
        assert!(is_transient_open_err(&LinkError::Usb(rusb::Error::Pipe)));
        assert!(!is_transient_open_err(&LinkError::Usb(rusb::Error::NotSupported)));
        assert!(!is_transient_open_err(&LinkError::NoEndpoints));
    }
}
