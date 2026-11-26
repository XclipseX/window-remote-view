use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rv_session::DatagramIo;

use crate::framing::{encode_record, RecordDecoder};
use crate::{LinkError, Result};

/// Live counters of a [`UsbDatagramLink`], shared so a monitor thread can
/// read them while the link is owned by a session (black-screen debugging).
#[derive(Debug, Default)]
pub struct LinkStats {
    /// Raw bytes read from the pipe (before record decoding).
    pub raw_in_bytes: AtomicU64,
    /// Raw bytes written to the pipe (including the 4-byte record headers).
    pub raw_out_bytes: AtomicU64,
    pub dg_in: AtomicU64,
    pub dg_out: AtomicU64,
    pub write_errors: AtomicU64,
    /// Why the reader thread ended / last write error, if any.
    pub last_error: Mutex<Option<String>>,
}

impl LinkStats {
    fn set_err(&self, m: String) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = Some(m);
        }
    }
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }
}

/// Datagram link over any reliable byte stream (USB bulk pipe or TCP).
///
/// A background thread reads the stream, decodes records and queues them;
/// `send` frames and writes synchronously. Implements [`DatagramIo`] so
/// `rv_session::HostSession<UsbDatagramLink>` works unchanged. Because the
/// underlying stream is reliable and ordered, FEC parity is wasted airtime
/// here; that tuning is left to the session layer (`set_loss(0.0)`).
pub struct UsbDatagramLink {
    writer: Box<dyn Write + Send>,
    rx: Receiver<Vec<u8>>,
    scratch: Vec<u8>,
    write_error: Option<std::io::Error>,
    stats: Arc<LinkStats>,
    _reader: JoinHandle<()>,
}

impl UsbDatagramLink {
    /// Build from separate read/write halves. `reader` must return
    /// `ErrorKind::TimedOut`/`WouldBlock`/`Interrupted` for "no data yet" and
    /// `Ok(0)` or another error for a closed stream.
    pub fn from_stream<R, W>(mut reader: R, writer: W) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let stats = Arc::new(LinkStats::default());
        let st = stats.clone();
        let handle = std::thread::Builder::new()
            .name("rv-usb-read".into())
            .spawn(move || {
                let mut dec = RecordDecoder::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            st.set_err("read: end of stream".into());
                            return;
                        }
                        Ok(n) => {
                            st.raw_in_bytes.fetch_add(n as u64, Ordering::Relaxed);
                            dec.push(&buf[..n]);
                            loop {
                                match dec.next_record() {
                                    Ok(Some(r)) => {
                                        st.dg_in.fetch_add(1, Ordering::Relaxed);
                                        if tx.send(r).is_err() {
                                            return;
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(e) => {
                                        // desynchronised
                                        st.set_err(format!("framing: {e}"));
                                        return;
                                    }
                                }
                            }
                        }
                        Err(e) => match e.kind() {
                            std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::Interrupted => continue,
                            _ => {
                                st.set_err(format!("read: {e}"));
                                return;
                            }
                        },
                    }
                }
            })
            .expect("spawn reader thread");
        Self {
            writer: Box::new(writer),
            rx,
            scratch: Vec::new(),
            write_error: None,
            stats,
            _reader: handle,
        }
    }

    /// Shared live counters.
    pub fn stats(&self) -> Arc<LinkStats> {
        self.stats.clone()
    }

    /// TCP link (ADB reverse fallback or LAN-over-TCP).
    pub fn from_tcp(stream: std::net::TcpStream) -> Result<Self> {
        stream.set_nodelay(true)?;
        let r = stream.try_clone()?;
        Ok(Self::from_stream(r, stream))
    }

    /// USB link over an opened AOA accessory.
    pub fn from_aoa(dev: crate::aoa::AoaDevice) -> Self {
        let (r, w) = dev.into_halves();
        Self::from_stream(r, w)
    }

    /// Blocking send with error reporting (`DatagramIo::send` swallows errors).
    pub fn try_send(&mut self, dg: &[u8]) -> Result<()> {
        if let Some(e) = &self.write_error {
            return Err(LinkError::Io(std::io::Error::new(e.kind(), e.to_string())));
        }
        self.scratch.clear();
        encode_record(dg, &mut self.scratch)?;
        let r = self
            .writer
            .write_all(&self.scratch)
            .and_then(|_| self.writer.flush());
        if let Err(e) = r {
            self.stats.write_errors.fetch_add(1, Ordering::Relaxed);
            self.stats.set_err(format!("write: {e}"));
            let err = std::io::Error::new(e.kind(), e.to_string());
            self.write_error = Some(e);
            return Err(LinkError::Io(err));
        }
        self.stats.dg_out.fetch_add(1, Ordering::Relaxed);
        self.stats.raw_out_bytes.fetch_add(self.scratch.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    pub fn try_recv(&self) -> Result<Option<Vec<u8>>> {
        match self.rx.try_recv() {
            Ok(v) => Ok(Some(v)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(LinkError::Closed),
        }
    }

    pub fn recv_timeout(&self, t: Duration) -> Result<Option<Vec<u8>>> {
        match self.rx.recv_timeout(t) {
            Ok(v) => Ok(Some(v)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(LinkError::Closed),
        }
    }

    /// True after a write failed (cable pulled, tablet app closed).
    pub fn is_broken(&self) -> bool {
        self.write_error.is_some()
    }
}

impl DatagramIo for UsbDatagramLink {
    fn send(&mut self, buf: &[u8]) {
        let _ = self.try_send(buf); // best effort; see is_broken()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn tcp_loopback_link() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let c = TcpStream::connect(addr).unwrap();
        let (s, _) = l.accept().unwrap();
        let mut a = UsbDatagramLink::from_tcp(c).unwrap();
        let b = UsbDatagramLink::from_tcp(s).unwrap();
        a.send(b"ping");
        a.send(&vec![1u8; 60000]);
        assert_eq!(b.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(), b"ping");
        assert_eq!(b.recv_timeout(Duration::from_secs(2)).unwrap().unwrap().len(), 60000);
    }
}
