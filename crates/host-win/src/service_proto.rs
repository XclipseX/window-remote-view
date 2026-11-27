//! Protocol pieces shared by the RemoteView Windows service (`RemoteView.exe --service`, runs the
//! host engine as LocalSystem so the lock screen can be captured / controlled) and its client
//! (the GUI). Everything here is platform independent and unit-tested; the Win32 side (service
//! control manager, named pipe, `CreateProcessAsUserW`) lives in `apps/win-ui/src/service.rs`.
//!
//! Wire format on the named pipe `\\.\pipe\RemoteViewSvc` (see `CONTROL_PROTOCOL.md`):
//! the client's first line is `{"op":"spawn","args":[...]}`; after that the pipe is a transparent
//! relay of the worker's stdout lines (service -> client) and stdin lines (client -> service).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

/// Windows service name (`sc query RemoteViewSvc`).
pub const SERVICE_NAME: &str = "RemoteViewSvc";
/// Name shown in `services.msc`.
pub const SERVICE_DISPLAY: &str = "RemoteView Service";
/// Named pipe the GUI connects to.
pub const PIPE_NAME: &str = r"\\.\pipe\RemoteViewSvc";
/// Pipe ACL: SYSTEM and Administrators full control, interactive users read/write only.
pub const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";
/// Longest accepted line from the client before the spawn request (bytes).
pub const MAX_SPAWN_LINE: usize = 16 * 1024;
/// How long the service waits for the spawn line after a client connects.
pub const SPAWN_TIMEOUT: Duration = Duration::from_secs(10);
/// Grace period after `{"cmd":"stop"}` before the worker is terminated.
pub const STOP_GRACE: Duration = Duration::from_secs(3);

/// Flags without a value that the service lets through.
const FLAGS: &[&str] = &["--usb", "--adb", "--lan", "--json-status"];
/// Flags with exactly one value.
const VALUE_FLAGS: &[&str] = &[
    "--display", "--mode", "--fps", "--width", "--height", "--quality", "--codec", "--encoder",
    "--bitrate-mbps", "--otp", "--monitor",
];

fn digits(v: &str, max_len: usize) -> bool {
    !v.is_empty() && v.len() <= max_len && v.bytes().all(|b| b.is_ascii_digit())
}

/// GDI output name such as `\\.\DISPLAY1` (never an arbitrary path).
fn gdi_name(v: &str) -> bool {
    v.strip_prefix(r"\\.\").is_some_and(|r| (1..=32).contains(&r.len()) && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
}

fn value_ok(flag: &str, v: &str) -> bool {
    match flag {
        "--display" => matches!(v, "extend" | "mirror" | "virtual"),
        "--codec" => matches!(v, "auto" | "hevc" | "h265" | "h264"),
        "--encoder" => matches!(v, "auto" | "hw" | "sw"),
        "--fps" | "--width" | "--height" | "--quality" | "--bitrate-mbps" => digits(v, 6),
        "--otp" => (6..=9).contains(&v.len()) && digits(v, 9),
        "--mode" => v.len() <= 24 && v.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'x' | b'X' | b'@')) && crate::vdisplay::DisplayMode::parse(v).is_some(),
        "--monitor" => gdi_name(v),
        _ => false,
    }
}

/// Allow-list check of the worker arguments the client asked for. Only engine flags with
/// well-formed values pass (no paths, no `--host-path`, no `--bind`, `--list`, `--scan-all`, ...);
/// the engine's own parser must accept them too. `--json-status` is always present in the result.
pub fn validate_worker_args(args: &[String]) -> Result<Vec<String>, String> {
    if args.len() > 40 {
        return Err("too many arguments".into());
    }
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if FLAGS.contains(&a.as_str()) {
            continue;
        }
        if VALUE_FLAGS.contains(&a.as_str()) {
            let v = it.next().ok_or_else(|| format!("{a} needs a value"))?;
            if !value_ok(a, v) {
                return Err(format!("bad value for {a}"));
            }
            continue;
        }
        return Err(format!("argument not allowed: {}", a.chars().take(40).collect::<String>()));
    }
    let mut out = args.to_vec();
    if !out.iter().any(|a| a == "--json-status") {
        out.push("--json-status".into());
    }
    crate::engine::check_args(&out)?;
    Ok(out)
}

/// The client's first line.
pub fn spawn_line(args: &[String]) -> String {
    let mut s = serde_json::json!({ "op": "spawn", "args": args }).to_string();
    s.push('\n');
    s
}

/// Parses + validates the first line from the client; returns the worker arguments.
pub fn parse_spawn_line(line: &str) -> Result<Vec<String>, String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).map_err(|e| format!("bad spawn line: {e}"))?;
    if v.get("op").and_then(|o| o.as_str()) != Some("spawn") {
        return Err("first line must be {\"op\":\"spawn\",\"args\":[...]}".into());
    }
    let arr = v.get("args").and_then(|a| a.as_array()).ok_or("spawn: args must be an array")?;
    let args: Vec<String> = arr
        .iter()
        .map(|x| x.as_str().map(str::to_string).ok_or("spawn: args must be strings"))
        .collect::<Result<_, _>>()?;
    validate_worker_args(&args)
}

/// Collects bytes from `rx` until the first `\n`. Returns the line (without the newline) and any
/// bytes received after it. Errors on timeout, disconnect or an over-long line.
pub fn read_first_line(rx: &Receiver<Vec<u8>>, timeout: Duration) -> Result<(String, Vec<u8>), String> {
    let deadline = Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            let rest = buf.split_off(i + 1);
            buf.pop();
            let line = String::from_utf8(buf).map_err(|_| "spawn line is not UTF-8".to_string())?;
            return Ok((line, rest));
        }
        if buf.len() > MAX_SPAWN_LINE {
            return Err("spawn line too long".into());
        }
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(chunk) => buf.extend_from_slice(&chunk),
            Err(RecvTimeoutError::Timeout) => return Err("timed out waiting for the spawn line".into()),
            Err(RecvTimeoutError::Disconnected) => return Err("client disconnected before the spawn line".into()),
        }
    }
}

// --------------------------------------------------------------------------
// pipe pump
// --------------------------------------------------------------------------

/// One end of a duplex byte pipe, driven from a single thread. Reads are only issued when
/// [`available`](Self::available) says there is data, so a thread never blocks in `read` while
/// it also has something to write (a synchronous Win32 pipe handle serialises read and write,
/// so a blocked read would stall the other direction).
pub trait RawPipe {
    /// Bytes ready to read now. `Err(BrokenPipe)` when the peer is gone.
    fn available(&mut self) -> io::Result<usize>;
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
}

/// Relays between a pipe and two in-process channels until the peer disconnects (`Err`), `stop`
/// is set, `incoming` has no receiver left, or `outgoing` has no sender left and is drained
/// (`Ok`; the peer should then be told by closing the pipe).
pub fn pump<P: RawPipe>(pipe: &mut P, outgoing: &Receiver<Vec<u8>>, incoming: &Sender<Vec<u8>>, stop: &AtomicBool) -> io::Result<()> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut busy = false;
        let n = pipe.available()?;
        if n > 0 {
            let want = n.min(buf.len());
            let got = pipe.read(&mut buf[..want])?;
            if got == 0 {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            if incoming.send(buf[..got].to_vec()).is_err() {
                return Ok(());
            }
            busy = true;
        }
        loop {
            match outgoing.try_recv() {
                Ok(data) => {
                    pipe.write_all(&data)?;
                    busy = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Ok(()),
            }
        }
        if !busy {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// `Read` over a channel of byte chunks (EOF when the sender is dropped).
pub struct ChanReader {
    rx: Receiver<Vec<u8>>,
    cur: Vec<u8>,
    pos: usize,
}

impl ChanReader {
    pub fn new(rx: Receiver<Vec<u8>>) -> Self {
        Self { rx, cur: Vec::new(), pos: 0 }
    }
}

impl io::Read for ChanReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos >= self.cur.len() {
            match self.rx.recv() {
                Ok(c) => (self.cur, self.pos) = (c, 0),
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.cur.len() - self.pos);
        out[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// `Write` into a channel of byte chunks (each `write` is one chunk).
pub struct ChanWriter(pub Sender<Vec<u8>>);

impl io::Write for ChanWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.send(data.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Write};
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Mutex};

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn gui_args_pass() {
        let a = v(&["--usb", "--width", "1920", "--height", "1080", "--fps", "60", "--quality", "80", "--codec", "auto", "--encoder", "auto", "--display", "virtual", "--monitor", r"\\.\DISPLAY2", "--json-status"]);
        assert_eq!(validate_worker_args(&a).unwrap(), a);
    }

    #[test]
    fn json_status_is_forced() {
        let out = validate_worker_args(&v(&["--lan"])).unwrap();
        assert_eq!(out, v(&["--lan", "--json-status"]));
    }

    #[test]
    fn rejects_dangerous_or_unknown() {
        for bad in [
            v(&["--host-path", r"C:\evil.exe"]),
            v(&["--host-worker"]),
            v(&["--bind", "0.0.0.0:1"]),
            v(&["--list"]),
            v(&["--scan-all"]),
            v(&["-h"]),
            v(&[r"C:\Windows\System32\cmd.exe"]),
            v(&["--monitor", r"C:\x"]),
            v(&["--monitor", r"\\.\..\evil"]),
            v(&["--monitor"]),
            v(&["--otp", "12ab56"]),
            v(&["--otp", "123"]),
            v(&["--fps", "-1"]),
            v(&["--codec", "x264"]),
            v(&["--display", "../etc"]),
            v(&["--mode", "1920x1080;calc"]),
            v(&["--output", "1"]),
        ] {
            assert!(validate_worker_args(&bad).is_err(), "should reject {bad:?}");
        }
        // values the engine itself rejects
        assert!(validate_worker_args(&v(&["--fps", "0"])).is_err());
        assert!(validate_worker_args(&v(&["--width", "100"])).is_err());
        assert!(validate_worker_args(&vec!["--usb".to_string(); 41]).is_err());
    }

    #[test]
    fn spawn_line_roundtrip_and_errors() {
        let args = v(&["--usb", "--fps", "30", "--json-status"]);
        let line = spawn_line(&args);
        assert!(line.ends_with('\n'));
        assert_eq!(parse_spawn_line(&line).unwrap(), args);
        assert_eq!(parse_spawn_line(r#"{"op":"spawn","args":["--lan"]}"#).unwrap(), v(&["--lan", "--json-status"]));
        assert!(parse_spawn_line("not json").is_err());
        assert!(parse_spawn_line(r#"{"op":"run","args":[]}"#).is_err());
        assert!(parse_spawn_line(r#"{"op":"spawn"}"#).is_err());
        assert!(parse_spawn_line(r#"{"op":"spawn","args":[1]}"#).is_err());
        assert!(parse_spawn_line(r#"{"op":"spawn","args":["--host-path","x"]}"#).is_err());
    }

    #[test]
    fn first_line_splits_chunks() {
        let (tx, rx) = channel();
        tx.send(b"{\"op\":\"sp".to_vec()).unwrap();
        tx.send(b"awn\"}\n{\"cmd\":\"stop\"}\n".to_vec()).unwrap();
        let (l, rest) = read_first_line(&rx, Duration::from_secs(1)).unwrap();
        assert_eq!(l, r#"{"op":"spawn"}"#);
        assert_eq!(rest, b"{\"cmd\":\"stop\"}\n");
        let (tx2, rx2) = channel::<Vec<u8>>();
        assert!(read_first_line(&rx2, Duration::from_millis(30)).is_err());
        drop(tx2);
        assert!(read_first_line(&rx2, Duration::from_millis(30)).is_err());
        let (tx3, rx3) = channel();
        tx3.send(vec![b'a'; MAX_SPAWN_LINE + 1]).unwrap();
        assert!(read_first_line(&rx3, Duration::from_millis(30)).is_err());
    }

    /// In-memory pipe end for the pump tests.
    struct Mem {
        inbox: Arc<Mutex<VecDeque<u8>>>,
        written: Arc<Mutex<Vec<u8>>>,
        closed: Arc<AtomicBool>,
    }
    impl RawPipe for Mem {
        fn available(&mut self) -> io::Result<usize> {
            let n = self.inbox.lock().unwrap().len();
            if n == 0 && self.closed.load(Ordering::SeqCst) {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            Ok(n)
        }
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut q = self.inbox.lock().unwrap();
            let n = buf.len().min(q.len());
            for b in buf.iter_mut().take(n) {
                *b = q.pop_front().unwrap();
            }
            Ok(n)
        }
        fn write_all(&mut self, d: &[u8]) -> io::Result<()> {
            self.written.lock().unwrap().extend_from_slice(d);
            Ok(())
        }
    }

    #[test]
    fn pump_relays_both_ways_and_ends_on_disconnect() {
        let inbox = Arc::new(Mutex::new(VecDeque::new()));
        let written = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let mut mem = Mem { inbox: inbox.clone(), written: written.clone(), closed: closed.clone() };
        let (otx, orx) = channel::<Vec<u8>>();
        let (itx, irx) = channel::<Vec<u8>>();
        let h = std::thread::spawn(move || pump(&mut mem, &orx, &itx, &AtomicBool::new(false)));
        inbox.lock().unwrap().extend(b"hello\nworld\n");
        let mut rd = BufReader::new(ChanReader::new(irx));
        let mut l = String::new();
        rd.read_line(&mut l).unwrap();
        assert_eq!(l, "hello\n");
        l.clear();
        rd.read_line(&mut l).unwrap();
        assert_eq!(l, "world\n");
        let mut w = ChanWriter(otx);
        w.write_all(b"{\"event\":1}\n").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(&*written.lock().unwrap(), b"{\"event\":1}\n");
        closed.store(true, Ordering::SeqCst);
        assert!(h.join().unwrap().is_err()); // broken pipe
        l.clear();
        assert_eq!(rd.read_line(&mut l).unwrap(), 0); // reader sees EOF
    }

    #[test]
    fn pump_returns_ok_when_outgoing_closes_after_drain() {
        let mut mem = Mem { inbox: Default::default(), written: Default::default(), closed: Default::default() };
        let written = mem.written.clone();
        let (otx, orx) = channel::<Vec<u8>>();
        let (itx, _irx) = channel::<Vec<u8>>();
        otx.send(b"bye\n".to_vec()).unwrap();
        drop(otx);
        assert!(pump(&mut mem, &orx, &itx, &AtomicBool::new(false)).is_ok());
        assert_eq!(&*written.lock().unwrap(), b"bye\n");
    }
}
