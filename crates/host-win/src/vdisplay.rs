//! Virtual (IddCx) display control.
//!
//! HONEST STATUS: nothing in this file talks to a real IddCx driver yet.
//!
//! Candidates for the driver:
//! * SudoVDA (SudoMaker, used by Apollo/Sunshine forks): a user-mode IddCx
//!   driver that exposes a device interface and IOCTLs to add/remove a virtual
//!   monitor with a given width/height/refresh and GUID. I do NOT remember the
//!   interface GUID / IOCTL codes / struct layouts reliably, so they are not
//!   invented here. Take them from its `sudovda-ioctl.h`.
//! * VirtualDrivers "Virtual-Display-Driver" (MikeTheTech): modes come from a
//!   `vdd_settings.xml` and it listens on a named pipe for reload commands;
//!   monitor count is static per settings. Pipe name/protocol unverified.
//!
//! Until one is wired, [`ExistingOutput`] simply picks an already-attached
//! DXGI output (e.g. one created by hand with the driver's control app, or a
//! physical secondary monitor) and reports its rect so capture/input line up.

use crate::capture::{list_outputs, OutputInfo};
use crate::{HostError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

impl DisplayMode {
    /// Galaxy Tab S10+ native panel at its 120 Hz mode.
    pub const TAB_S10_PLUS: DisplayMode = DisplayMode {
        width: 2800,
        height: 1752,
        refresh_hz: 120,
    };

    /// Parses `WxH@Hz` (Hz optional, default 60).
    pub fn parse(s: &str) -> Option<Self> {
        let (wh, hz) = match s.split_once('@') {
            Some((a, b)) => (a, b.parse().ok()?),
            None => (s, 60),
        };
        let (w, h) = wh.split_once(['x', 'X'])?;
        Some(Self {
            width: w.parse().ok()?,
            height: h.parse().ok()?,
            refresh_hz: hz,
        })
    }
}

/// Where the virtual display ended up.
#[derive(Debug, Clone)]
pub struct VirtualMonitor {
    pub output: OutputInfo,
}

pub trait VirtualDisplay {
    /// Create (or select) a monitor with `mode`.
    fn create(&mut self, mode: DisplayMode) -> Result<VirtualMonitor>;
    /// Remove it again (no-op for fallbacks).
    fn remove(&mut self) -> Result<()>;
}

/// Driver-backed implementation. TODO: implement against SudoVDA's IOCTLs
/// (open its device interface with SetupDi*, DeviceIoControl ADD_VIRTUAL_DISPLAY
/// with {width,height,refresh,guid}, keep the handle open, send the watchdog
/// ping it requires, REMOVE on drop).
pub struct IddCxDisplay;

impl VirtualDisplay for IddCxDisplay {
    fn create(&mut self, _mode: DisplayMode) -> Result<VirtualMonitor> {
        Err(HostError::Unsupported(
            "IddCx virtual display driver control not implemented (see vdisplay.rs)",
        ))
    }
    fn remove(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Fallback: use an existing output. `index` None = last enumerated attached
/// output that is not output 0 of adapter 0 (assumed primary), else the first.
pub struct ExistingOutput {
    pub index: Option<usize>,
}

impl VirtualDisplay for ExistingOutput {
    fn create(&mut self, _mode: DisplayMode) -> Result<VirtualMonitor> {
        let outs: Vec<_> = list_outputs()?.into_iter().filter(|o| o.attached).collect();
        let pick = match self.index {
            Some(i) => outs.get(i).cloned(),
            None => outs
                .iter()
                .rev()
                .find(|o| !(o.adapter_index == 0 && o.output_index == 0))
                .or(outs.first())
                .cloned(),
        };
        pick.map(|output| VirtualMonitor { output })
            .ok_or_else(|| HostError::Other("no attached DXGI output found".into()))
    }
    fn remove(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_mode() {
        assert_eq!(DisplayMode::parse("2800x1752@120"), Some(DisplayMode::TAB_S10_PLUS));
        assert_eq!(DisplayMode::parse("1920x1080").unwrap().refresh_hz, 60);
        assert!(DisplayMode::parse("junk").is_none());
    }
}
