//! ADB fallback: `adb reverse` + TCP.
//!
//! The tablet app connects to its own `127.0.0.1:47810`; adb tunnels that to
//! the host, where [`listen_and_accept`] is waiting. Requires USB debugging
//! enabled on the tablet and `adb` on PATH.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::{LinkError, Result, UsbDatagramLink, ADB_PORT};

fn run_adb(args: &[&str]) -> Result<String> {
    let out = Command::new("adb")
        .args(args)
        .output()
        .map_err(|e| LinkError::Adb(format!("cannot run adb: {e}")))?;
    if !out.status.success() {
        return Err(LinkError::Adb(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// True if `adb` runs and lists at least one device in state `device`.
pub fn device_present() -> bool {
    run_adb(&["devices"])
        .map(|s| s.lines().skip(1).any(|l| l.trim().ends_with("device")))
        .unwrap_or(false)
}

/// `adb reverse tcp:PORT tcp:PORT`.
pub fn reverse(port: u16) -> Result<()> {
    let spec = format!("tcp:{port}");
    run_adb(&["reverse", &spec, &spec]).map(|_| ())
}

pub fn remove_reverse(port: u16) -> Result<()> {
    run_adb(&["reverse", "--remove", &format!("tcp:{port}")]).map(|_| ())
}

/// Listen on 127.0.0.1:`port` and accept one connection within `timeout`.
pub fn listen_and_accept(port: u16, timeout: Duration) -> Result<TcpStream> {
    let l = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))?;
    l.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        match l.accept() {
            Ok((s, _)) => {
                s.set_nonblocking(false)?;
                return Ok(s);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(LinkError::Io(std::io::ErrorKind::TimedOut.into()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Full fallback: set up the reverse tunnel, wait for the tablet to connect.
pub fn connect(timeout: Duration) -> Result<UsbDatagramLink> {
    reverse(ADB_PORT)?;
    UsbDatagramLink::from_tcp(listen_and_accept(ADB_PORT, timeout)?)
}
