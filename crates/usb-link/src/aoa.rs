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
//! so step 1 fails with `NotSupported`/`Access` until the user installs WinUSB
//! (e.g. Zadig) for the normal-mode device and again for the 18D1:2D00/2D01
//! accessory device. A signed INF shipped with the host installer would
//! automate that. On Linux/macOS no driver work is needed (udev rule on Linux).

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

impl Default for AccessoryInfo {
    fn default() -> Self {
        Self {
            manufacturer: "RemoteView".into(),
            model: "RemoteViewHost".into(),
            description: "RemoteView second display host".into(),
            version: "1.0".into(),
            uri: "https://example.invalid/remoteview".into(),
            serial: "0001".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub vid: u16,
    pub pid: u16,
    pub bus: u8,
    pub address: u8,
}

fn is_accessory(vid: u16, pid: u16) -> bool {
    vid == ACCESSORY_VID && (pid == ACCESSORY_PID || pid == ACCESSORY_ADB_PID)
}

/// Lists attached USB devices that look like Android (or all, if `scan_all`).
pub fn list_candidates(scan_all: bool) -> Result<Vec<Candidate>> {
    let ctx = Context::new()?;
    let mut out = Vec::new();
    for d in ctx.devices()?.iter() {
        let Ok(desc) = d.device_descriptor() else { continue };
        // Class 9 = hub; never poke those.
        if desc.class_code() == 9 {
            continue;
        }
        if scan_all || ANDROID_VIDS.contains(&desc.vendor_id()) {
            out.push(Candidate {
                vid: desc.vendor_id(),
                pid: desc.product_id(),
                bus: d.bus_number(),
                address: d.address(),
            });
        }
    }
    Ok(out)
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
}

impl AoaDevice {
    /// Finds an Android device, switches it to accessory mode if needed and
    /// opens it. Returns `NoDevice` when nothing suitable is attached.
    pub fn connect(info: &AccessoryInfo, scan_all: bool, timeout: Duration) -> Result<Self> {
        let ctx = Context::new()?;
        // Already in accessory mode (e.g. app restarted)?
        if let Some(dev) = find_accessory(&ctx)? {
            return Self::open(dev);
        }
        let mut last_err = LinkError::NoDevice;
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
            return Self::wait_reenumerate(&ctx, timeout);
        }
        Err(last_err)
    }

    fn wait_reenumerate(ctx: &Context, timeout: Duration) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        // Give the device a moment to detach before polling.
        std::thread::sleep(Duration::from_millis(500));
        loop {
            if let Some(dev) = find_accessory(ctx)? {
                // Retry open briefly: node may exist before it is usable.
                match Self::open(dev) {
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

    fn open(dev: Device<Context>) -> Result<Self> {
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
        Ok(Self {
            handle: Arc::new(handle),
            ep_in,
            ep_out,
            iface,
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
        // Short timeout so the reader thread notices a dropped link promptly.
        self.h
            .read_bulk(self.ep, buf, Duration::from_millis(500))
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
