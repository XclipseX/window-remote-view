use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use rv_session::DatagramIo;

use crate::framing::{encode_record, RecordDecoder};
use crate::{LinkError, Result};

/// Size of the bulk read buffer (large so a whole record can arrive in one transfer).
const READ_BUF: usize = 64 * 1024;

/// Writer thread coalesces queued records into one bulk write up to roughly
/// this many bytes (32 KiB matches Android `f_accessory.c`'s `RX_REQ_MAX(2) *
/// BULK_BUFFER_SIZE(16384)` DMA ring to minimize syscalls without stalling xHCI).
const WRITE_BATCH: usize = 32 * 1024;
/// Max records queued for the writer thread; a full queue blocks `send`
/// (back-pressure, same as the old synchronous write).
const WRITE_QUEUE: usize = 1024;

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
    /// Opened accessory (bus/address/interface/endpoints); default for TCP.
    pub location: Mutex<crate::aoa::DeviceLocation>,
    /// Hex of the first bytes ever read (diagnostics), e.g. the client msg1.
    pub first_bytes: Mutex<Option<String>>,
}

impl LinkStats {
    fn set_err(&self, m: String) {
        if let Ok(mut g) = self.last_error.lock() {
            *g = Some(m);
        }
    }
    pub fn location(&self) -> crate::aoa::DeviceLocation {
        self.location.lock().map(|g| *g).unwrap_or_default()
    }
    pub fn first_bytes(&self) -> Option<String> {
        self.first_bytes.lock().ok().and_then(|g| g.clone())
    }
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|g| g.clone())
    }
}

/// Datagram link over any reliable byte stream (USB bulk pipe or TCP).
///
/// A background thread reads the stream, decodes records and queues them;
/// `send` frames the record and queues it for a writer thread, which coalesces
/// whatever is pending into few large bulk writes (a video frame is ~100+
/// small datagrams; one blocking USB transfer each is far too slow). Implements [`DatagramIo`] so
/// `rv_session::HostSession<UsbDatagramLink>` works unchanged. Because the
/// underlying stream is reliable and ordered, FEC parity is wasted airtime
/// here; that tuning is left to the session layer (`set_loss(0.0)`).
pub struct UsbDatagramLink {
    wtx: Option<SyncSender<Vec<u8>>>,
    rx: Receiver<Vec<u8>>,
    stop: Arc<AtomicBool>,
    broken: Arc<AtomicBool>,
    stats: Arc<LinkStats>,
    _reader: Option<JoinHandle<()>>,
    _writer: Option<JoinHandle<()>>,
}

impl UsbDatagramLink {
    /// Build from separate read/write halves. `reader` must return
    /// `ErrorKind::TimedOut`/`WouldBlock`/`Interrupted` for "no data yet" and
    /// `Ok(0)` or another error for a closed stream.
    pub fn from_stream<R, W>(reader: R, writer: W) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        Self::from_stream_opts(reader, writer, false)
    }

    fn from_stream_opts<R, W>(mut reader: R, writer: W, pad_usb_short_packet: bool) -> Self
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
    {
        let mut writer = writer;
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(LinkStats::default());
        let st = stats.clone();
        let stop_r = stop.clone();
        let handle = std::thread::Builder::new()
            .name("rv-usb-read".into())
            .spawn(move || {
                let mut dec = RecordDecoder::new();
                let mut buf = vec![0u8; READ_BUF];
                loop {
                    if stop_r.load(Ordering::Acquire) {
                        return;
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => {
                            st.set_err("read: end of stream".into());
                            return;
                        }
                        Ok(n) => {
                            if st.raw_in_bytes.fetch_add(n as u64, Ordering::Relaxed) == 0 {
                                let hex: String = buf[..n.min(16)].iter().map(|b| format!("{b:02x}")).collect();
                                if let Ok(mut g) = st.first_bytes.lock() {
                                    *g = Some(format!("{n}B:{hex}"));
                                }
                            }
                            dec.push(&buf[..n]);
                            loop {
                                match dec.next_record() {
                                    Ok(Some(r)) => {
                                        if pad_usb_short_packet && r.is_empty() {
                                            continue; // zero-length USB short-packet pad record
                                        }
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
                            | std::io::ErrorKind::Interrupted => {
                                if stop_r.load(Ordering::Acquire) {
                                    return;
                                }
                                continue;
                            }
                            _ => {
                                st.set_err(format!("read: {e}"));
                                return;
                            }
                        },
                    }
                }
            })
            .expect("spawn reader thread");
        let (wtx, wrx) = mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE);
        let broken = Arc::new(AtomicBool::new(false));
        let (st, br, stop_w) = (stats.clone(), broken.clone(), stop.clone());
        let whandle = std::thread::Builder::new()
            .name("rv-usb-write".into())
            .spawn(move || {
                let mut batch: Vec<u8> = Vec::with_capacity(WRITE_BATCH + crate::framing::MAX_RECORD + 8);
                let mut pending: Option<Vec<u8>> = None;
                loop {
                    if stop_w.load(Ordering::Acquire) {
                        return;
                    }
                    let first = match pending.take() {
                        Some(r) => r,
                        None => match wrx.recv() {
                            Ok(r) => r,
                            Err(_) => break,
                        },
                    };
                    if stop_w.load(Ordering::Acquire) {
                        return;
                    }
                    batch.clear();
                    batch.extend_from_slice(&first);
                    let mut recs = 1u64;
                    while batch.len() < WRITE_BATCH {
                        match wrx.try_recv() {
                            Ok(r) => {
                                if batch.len() + r.len() <= WRITE_BATCH {
                                    batch.extend_from_slice(&r);
                                    recs += 1;
                                } else {
                                    pending = Some(r);
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    // Avoid USB 2.0 (512B) / USB 3.x (1024B) short-packet stall in Android
                    // `f_accessory.c`: if a transfer is an exact multiple of 512 bytes, append a
                    // 4-byte zero-length pad record so `acc_read()` wakes up immediately.
                    if pad_usb_short_packet && !batch.is_empty() && batch.len().is_multiple_of(512) {
                        batch.extend_from_slice(&[0u8; 4]);
                    }
                    if let Err(e) = writer.write_all(&batch).and_then(|_| writer.flush()) {
                        st.write_errors.fetch_add(1, Ordering::Relaxed);
                        st.set_err(format!("write: {e}"));
                        br.store(true, Ordering::Release);
                        return; // dropping wrx makes later sends fail
                    }
                    st.dg_out.fetch_add(recs, Ordering::Relaxed);
                    st.raw_out_bytes.fetch_add(batch.len() as u64, Ordering::Relaxed);
                }
            })
            .expect("spawn writer thread");
        Self {
            wtx: Some(wtx),
            rx,
            stop,
            broken,
            stats,
            _reader: Some(handle),
            _writer: Some(whandle),
        }
    }

    /// Shared live counters.
    pub fn stats(&self) -> Arc<LinkStats> {
        self.stats.clone()
    }

    /// TCP link (ADB reverse fallback or LAN-over-TCP).
    pub fn from_tcp(stream: std::net::TcpStream) -> Result<Self> {
        stream.set_nodelay(true)?;
        // Short read timeout so `Drop` joins `rv-usb-read` promptly even if the peer stays open.
        stream.set_read_timeout(Some(Duration::from_millis(150)))?;
        let r = stream.try_clone()?;
        Ok(Self::from_stream(r, stream))
    }

    /// USB link over an opened AOA accessory.
    pub fn from_aoa(dev: crate::aoa::AoaDevice) -> Self {
        let loc = dev.location();
        let (r, w) = dev.into_halves();
        let l = Self::from_stream_opts(r, w, true);
        if let Ok(mut g) = l.stats.location.lock() {
            *g = loc;
        }
        l
    }

    /// Queues one datagram for the writer thread (blocks only when the queue
    /// is full). Reports a previous write failure or an oversized datagram.
    pub fn try_send(&mut self, dg: &[u8]) -> Result<()> {
        if self.is_broken() {
            return Err(LinkError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                self.stats.last_error().unwrap_or_else(|| "write failed".into()),
            )));
        }
        let mut rec = Vec::new();
        encode_record(dg, &mut rec)?;
        let Some(wtx) = self.wtx.as_ref() else {
            return Err(LinkError::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "writer stopped",
            )));
        };
        wtx.send(rec)
            .map_err(|_| LinkError::Io(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "writer stopped")))
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
        self.broken.load(Ordering::Acquire)
    }
}

impl Drop for UsbDatagramLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Dropping the sender wakes `wrx.recv()` so `rv-usb-write` exits promptly.
        drop(self.wtx.take());
        // Join both threads so `AoaDevice`'s `Release::drop()` has released the
        // WinUSB handle before any reconnect attempts `AoaDevice::connect()`.
        if let Some(h) = self._reader.take() {
            let _ = h.join();
        }
        if let Some(h) = self._writer.take() {
            let _ = h.join();
        }
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

    /// Writer that records the size of every write call (slow first call so the
    /// queue fills and later records get coalesced).
    struct Spy(Arc<Mutex<Vec<Vec<u8>>>>);
    impl Write for Spy {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            let mut g = self.0.lock().unwrap();
            if g.is_empty() {
                std::thread::sleep(Duration::from_millis(100));
            }
            g.push(b.to_vec());
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writes_are_coalesced_and_stream_identical() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut l = UsbDatagramLink::from_stream(Chunked { data: vec![], pos: 0, chunk: 1, tick: false }, Spy(log.clone()));
        let mut expect = Vec::new();
        for i in 0..200u32 {
            let dg = vec![i as u8; 1 + (i as usize % 50)];
            encode_record(&dg, &mut expect).unwrap();
            l.send(&dg);
        }
        let t0 = std::time::Instant::now();
        while l.stats().raw_out_bytes.load(Ordering::Relaxed) < expect.len() as u64 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let g = log.lock().unwrap();
        assert!(g.len() < 20, "expected coalescing, got {} writes", g.len());
        assert_eq!(g.concat(), expect);
        assert_eq!(l.stats().dg_out.load(Ordering::Relaxed), 200);
    }

    /// Reader that hands out the wire bytes in fixed small chunks, with
    /// timeouts in between (like a bulk pipe).
    struct Chunked {
        data: Vec<u8>,
        pos: usize,
        chunk: usize,
        tick: bool,
    }
    impl Read for Chunked {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.tick = !self.tick;
            if self.tick {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            if self.pos >= self.data.len() {
                std::thread::sleep(Duration::from_millis(20));
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let n = self.chunk.min(buf.len()).min(self.data.len() - self.pos);
            buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
            self.pos += n;
            Ok(n)
        }
    }

    #[test]
    fn split_chunks_over_mock_stream() {
        let mut wire = Vec::new();
        let recs: Vec<Vec<u8>> = vec![vec![1; 54], vec![], vec![9; 20_000], b"x".to_vec()];
        for r in &recs {
            encode_record(r, &mut wire).unwrap();
        }
        let total = wire.len() as u64;
        let l = UsbDatagramLink::from_stream(Chunked { data: wire, pos: 0, chunk: 7, tick: false }, std::io::sink());
        for r in &recs {
            assert_eq!(&l.recv_timeout(Duration::from_secs(10)).unwrap().unwrap(), r);
        }
        let st = l.stats();
        assert_eq!(st.raw_in_bytes.load(Ordering::Relaxed), total);
        assert_eq!(st.dg_in.load(Ordering::Relaxed), 4);
        assert!(st.first_bytes().unwrap().starts_with("7B:"));
    }

    #[test]
    fn aoa_pads_512_byte_multiples_and_skips_pad_on_read() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut l = UsbDatagramLink::from_stream_opts(
            Chunked { data: vec![], pos: 0, chunk: 1, tick: false },
            Spy(log.clone()),
            true,
        );
        // 508 bytes payload + 4 bytes length prefix = exact 512 bytes -> must pad to 516 bytes.
        let payload = vec![0xABu8; 508];
        l.send(&payload);
        let t0 = std::time::Instant::now();
        while l.stats().raw_out_bytes.load(Ordering::Relaxed) < 516 && t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let wire = log.lock().unwrap().concat();
        assert_eq!(wire.len(), 516);
        assert_eq!(&wire[512..516], &[0, 0, 0, 0]);

        // Reading that padded wire back with pad_usb_short_packet=true yields only the 1 real record.
        let r = UsbDatagramLink::from_stream_opts(
            Chunked { data: wire, pos: 0, chunk: 64, tick: false },
            std::io::sink(),
            true,
        );
        assert_eq!(r.recv_timeout(Duration::from_secs(5)).unwrap().unwrap(), payload);
        assert_eq!(r.recv_timeout(Duration::from_millis(80)).unwrap(), None);
    }

    #[test]
    fn drop_stops_and_joins_reader_on_timeout() {
        struct DropTrackerReader {
            dropped: Arc<AtomicBool>,
            reads: Arc<AtomicU64>,
        }
        impl Read for DropTrackerReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                self.reads.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
                Err(std::io::ErrorKind::TimedOut.into())
            }
        }
        impl Drop for DropTrackerReader {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let reader_dropped = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicU64::new(0));
        let link = UsbDatagramLink::from_stream(
            DropTrackerReader {
                dropped: reader_dropped.clone(),
                reads: reads.clone(),
            },
            std::io::sink(),
        );
        let wait_start = std::time::Instant::now();
        while reads.load(Ordering::Relaxed) == 0 && wait_start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(reads.load(Ordering::Relaxed) > 0, "reader thread did not start");

        let t0 = std::time::Instant::now();
        drop(link);
        let elapsed = t0.elapsed();

        assert!(
            reader_dropped.load(Ordering::Acquire),
            "reader was not dropped before UsbDatagramLink::drop returned"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "UsbDatagramLink::drop took too long: {elapsed:?}"
        );
    }
}
