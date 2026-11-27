//! `rv-host-win`: Windows host pieces for the USB second-display product.
//!
//! Everything Windows-specific is behind `cfg(windows)`; on other OSes the
//! same API exists as stubs that return [`HostError::Unsupported`], so the
//! workspace builds and the pure logic (colour conversion, Opus framing,
//! coordinate mapping) is unit-testable anywhere.

pub mod audio;
pub mod capture;
pub mod control;
pub mod cursor;
pub mod encode;
pub mod engine;
pub mod input;
pub mod logfile;
pub mod session_state;
pub mod sysinfo;
pub mod sudovda;
pub mod vdisplay;

#[cfg(windows)]
mod capture_win;
#[cfg(windows)]
mod audio_win;
#[cfg(windows)]
mod input_win;

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    #[error("not supported on this platform / not implemented: {0}")]
    Unsupported(&'static str),
    #[error("windows api: {0}")]
    Os(String),
    #[error("encoder: {0}")]
    Encoder(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, HostError>;

#[cfg(windows)]
impl From<windows::core::Error> for HostError {
    fn from(e: windows::core::Error) -> Self {
        HostError::Os(format!("{e}"))
    }
}

/// Integer rectangle in virtual-desktop pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
}
