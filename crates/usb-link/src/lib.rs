//! `rv-usb-link`: carry rv-session datagrams over a USB cable.
//!
//! Primary path: Android Open Accessory (AOA v1/v2) over raw USB bulk
//! endpoints ([`aoa`]). Fallback: `adb reverse tcp:47810 tcp:47810` plus a TCP
//! socket ([`adb`]). Both carry the same stream framing ([`framing`]):
//! `u32 BE length | payload`, payload <= 64 KiB, so the encrypted rv-session
//! runs unchanged on top ([`UsbDatagramLink`] implements `DatagramIo`).
//!
//! Everything touching hardware returns `Err` (never panics) when no device
//! is present. Only the pure framing logic is unit-tested; the AOA code path
//! has NOT been exercised against a real tablet.

pub mod adb;
pub mod aoa;
pub mod framing;
mod link;

pub use link::UsbDatagramLink;

#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("usb: {0}")]
    Usb(#[from] rusb::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("no Android device found")]
    NoDevice,
    #[error("device does not support Android Open Accessory (protocol {0})")]
    AoaUnsupported(u16),
    #[error("accessory did not re-enumerate within the timeout")]
    ReenumerateTimeout,
    #[error("no bulk IN/OUT endpoint pair found on accessory interface")]
    NoEndpoints,
    #[error("adb: {0}")]
    Adb(String),
    #[error("frame too large: {0} bytes (max {1})")]
    FrameTooLarge(usize, usize),
    #[error("link closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, LinkError>;

/// TCP port used for the ADB-reverse fallback (host listens, tablet connects
/// to its own `127.0.0.1:47810`).
pub const ADB_PORT: u16 = 47810;
