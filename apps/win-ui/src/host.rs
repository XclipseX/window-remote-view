//! Host engine supervision. The engine is the same executable re-launched as
//! `RemoteView.exe --host-worker <rv-host args>` (see `main.rs`), speaking the
//! JSON-lines contract of `crates/host-win/CONTROL_PROTOCOL.md`; a crash or
//! stuck capture therefore cannot take the GUI down.

use crate::control::{parse_event, HostCmd, HostEvent, LogLevel};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Messages delivered to the UI thread.
#[derive(Debug)]
pub enum UiMsg {
    /// Virtual display driver install finished (result, start sharing afterwards).
    Vdd((crate::driver::DriverResult, bool)),
    Event(HostEvent),
    /// Process ended. `restart_in` is Some when an auto-restart is scheduled.
    Exited { code: Option<i32>, restart_in: Option<Duration> },
    SpawnFailed(String),
    Stopped,
    /// Device scan done by the GUI itself while no host engine is running.
    Scan(Vec<crate::control::DeviceItem>),
    /// A WinUSB install attempt finished.
    Driver { vid: u16, pid: u16, result: crate::driver::DriverResult },
}

/// The `--host-worker` marker: everything after it is `rv-host` arguments.
pub const WORKER_FLAG: &str = "--host-worker";

/// Executable + leading arguments that start the host engine: this very exe in
/// worker mode, or a custom `rv-host` (debug) when `custom` is set.
pub fn host_command(custom: Option<&str>) -> (PathBuf, Vec<String>) {
    if let Some(p) = custom.filter(|s| !s.trim().is_empty()) {
        return (PathBuf::from(p), Vec::new());
    }
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("RemoteView.exe"));
    (me, vec![WORKER_FLAG.to_string()])
}

/// Where the host engine's command lines go (child stdin or the service pipe).
type CmdWriter = Box<dyn Write + Send>;

pub struct HostProcess {
    stdin: Arc<Mutex<Option<CmdWriter>>>,
    stop: Arc<AtomicBool>,
    detach_only: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl HostProcess {
    /// Starts a supervisor thread that runs and restarts the host until `stop()` or `detach()`.
    pub fn start(
        exe: PathBuf,
        args: Vec<String>,
        tx: Sender<UiMsg>,
        repaint: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let stdin = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let detach_only = Arc::new(AtomicBool::new(false));
        let (s2, st2, dt2) = (stdin.clone(), stop.clone(), detach_only.clone());
        let repaint = Arc::new(repaint);
        let thread = thread::spawn(move || supervise(exe, args, tx, s2, st2, dt2, repaint));
        Self { stdin, stop, detach_only, thread: Some(thread) }
    }

    pub fn send(&self, cmd: &HostCmd) {
        if let Ok(mut g) = self.stdin.lock() {
            if let Some(w) = g.as_mut() {
                let _ = w.write_all(cmd.to_line().as_bytes());
                let _ = w.flush();
            }
        }
    }

    /// Graceful stop (sends `stop`, kills after a grace period). Non-blocking.
    pub fn stop(&self) {
        self.detach_only.store(false, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        self.send(&HostCmd::Stop);
    }

    /// Detaches from the always-on `RemoteViewSvc` pipe WITHOUT stopping the daemon worker
    /// (closing the GUI window keeps the tablet display alive). For a non-service child process
    /// this still stops the child cleanly.
    pub fn detach(&self) {
        self.detach_only.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        if !self.detach_only.load(Ordering::SeqCst) {
            self.stop();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join(); // supervisor detaches immediately or kills child after ~2 s
        }
    }
}

fn spawn_child(exe: &PathBuf, args: &[String]) -> std::io::Result<Child> {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.spawn()
}

/// One running host engine, wherever it lives: a child process of the GUI, or a worker started
/// by the RemoteView service (as SYSTEM) behind the named pipe.
struct Link {
    stdout: Box<dyn Read + Send>,
    stderr: Option<Box<dyn Read + Send>>,
    stdin: CmdWriter,
    end: LinkEnd,
}

enum LinkEnd {
    Child { child: Child, _job: Option<crate::job::ChildJob> },
    Pipe { close: Arc<AtomicBool>, done: Arc<AtomicBool> },
}

impl LinkEnd {
    /// `Some(exit code)` once ended (`Some(None)` = ended, code unknown).
    fn try_wait(&mut self) -> Option<Option<i32>> {
        match self {
            LinkEnd::Child { child, .. } => match child.try_wait() {
                Ok(Some(st)) => Some(st.code()),
                Ok(None) => None,
                Err(_) => Some(None),
            },
            LinkEnd::Pipe { done, .. } => done.load(Ordering::SeqCst).then_some(None),
        }
    }

    /// Hard stop after the grace period (or immediate pipe close on detach).
    fn kill(&mut self) -> Option<i32> {
        match self {
            LinkEnd::Child { child, .. } => {
                let _ = child.kill();
                child.wait().ok().and_then(|s| s.code())
            }
            LinkEnd::Pipe { close, done } => {
                close.store(true, Ordering::SeqCst);
                let t = Instant::now();
                while !done.load(Ordering::SeqCst) && t.elapsed() < Duration::from_secs(1) {
                    thread::sleep(Duration::from_millis(10));
                }
                None
            }
        }
    }
}

/// Whether `args` start the engine as this exe's own worker (as opposed to a custom `rv-host`).
fn is_self_worker(args: &[String]) -> bool {
    args.first().map(String::as_str) == Some(WORKER_FLAG)
}

enum ServiceAttempt {
    Connected(Link),
    NotInstalled,
    TemporarilyUnavailable,
}

/// Connects to the always-on RemoteView service (lock-screen capable, runs the engine as SYSTEM).
fn connect_service(worker_args: &[String]) -> ServiceAttempt {
    let mut c = match crate::service::connect() {
        Ok(c) => c,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound || e.kind() == std::io::ErrorKind::Unsupported {
                return if crate::service::service_installed() {
                    ServiceAttempt::TemporarilyUnavailable
                } else {
                    ServiceAttempt::NotInstalled
                };
            }
            rv_host_win::logfile::write_line("warn", &format!("host: service pipe busy/unusable ({e})"));
            return ServiceAttempt::TemporarilyUnavailable;
        }
    };
    let line = rv_host_win::service_proto::attach_line(worker_args);
    if c.writer.write_all(line.as_bytes()).is_err() {
        c.close.store(true, Ordering::SeqCst);
        return ServiceAttempt::TemporarilyUnavailable;
    }
    ServiceAttempt::Connected(Link {
        stdout: Box::new(c.reader),
        stderr: None,
        stdin: Box::new(c.writer),
        end: LinkEnd::Pipe { close: c.close, done: c.done },
    })
}

/// Starts the engine: attaches to the always-on service when installed, retrying briefly if
/// the service is restarting so we never spawn a conflicting non-SYSTEM child process.
fn open_link(exe: &PathBuf, args: &[String], allow_service: bool, stop: &AtomicBool) -> std::io::Result<(Link, &'static str)> {
    if allow_service && is_self_worker(args) {
        for attempt in 0..8 {
            match connect_service(&args[1..]) {
                ServiceAttempt::Connected(l) => {
                    return Ok((l, "host: dịch vụ RemoteView (SYSTEM) qua pipe — chạy ngay từ màn hình khóa"));
                }
                ServiceAttempt::NotInstalled => break,
                ServiceAttempt::TemporarilyUnavailable => {
                    if stop.load(Ordering::SeqCst) {
                        return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "stopped"));
                    }
                    if attempt < 7 {
                        thread::sleep(Duration::from_millis(500));
                    }
                }
            }
        }
    }
    let mut child = spawn_child(exe, args)?;
    let job = crate::job::ChildJob::new();
    if let Some(ref j) = job {
        let _ = j.assign_child(&child);
    }
    let link = Link {
        stdout: Box::new(child.stdout.take().expect("piped stdout")),
        stderr: child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>),
        stdin: Box::new(child.stdin.take().expect("piped stdin")),
        end: LinkEnd::Child { child, _job: job },
    };
    Ok((link, "host: tiến trình con của app (không xử lý được màn hình khóa; cài dịch vụ để có)"))
}

fn supervise(
    exe: PathBuf,
    args: Vec<String>,
    tx: Sender<UiMsg>,
    stdin_slot: Arc<Mutex<Option<CmdWriter>>>,
    stop: Arc<AtomicBool>,
    detach_only: Arc<AtomicBool>,
    repaint: Arc<dyn Fn() + Send + Sync>,
) {
    let send = |m: UiMsg| {
        let _ = tx.send(m);
        repaint();
    };
    let mut backoff = Duration::from_secs(1);
    // The service path is new and untested on many machines: if it dies quickly twice in a row,
    // fall back to the plain child process for the rest of this run so sharing still works.
    let mut svc_quick_fails = 0u32;
    let mut allow_service = true;
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let (mut link, transport) = match open_link(&exe, &args, allow_service, &stop) {
            Ok(l) => l,
            Err(e) => {
                if stop.load(Ordering::SeqCst) {
                    send(UiMsg::Stopped);
                    return;
                }
                send(UiMsg::SpawnFailed(format!("{}: {e}", exe.display())));
                // Missing executable will not fix itself: do not loop.
                return;
            }
        };
        if cfg!(windows) {
            send(UiMsg::Event(HostEvent::Log { level: LogLevel::Info, msg: transport.into() }));
        }
        if let Ok(mut g) = stdin_slot.lock() {
            *g = Some(link.stdin);
        }
        let mut readers = Vec::new();
        {
            let (tx, rp) = (tx.clone(), repaint.clone());
            let out = link.stdout;
            readers.push(thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let msg = match parse_event(&line) {
                        Some(ev) => UiMsg::Event(ev),
                        None if !line.trim().is_empty() => UiMsg::Event(HostEvent::Log {
                            level: LogLevel::Info,
                            msg: line,
                        }),
                        None => continue,
                    };
                    let _ = tx.send(msg);
                    rp();
                }
            }));
        }
        if let Some(err) = link.stderr.take() {
            let (tx, rp) = (tx.clone(), repaint.clone());
            readers.push(thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = tx.send(UiMsg::Event(HostEvent::Log { level: LogLevel::Warn, msg: line }));
                    rp();
                }
            }));
        }

        let mut stop_at: Option<Instant> = None;
        let status = loop {
            if let Some(code) = link.end.try_wait() {
                break code;
            }
            if stop.load(Ordering::SeqCst) {
                if detach_only.load(Ordering::SeqCst) && matches!(link.end, LinkEnd::Pipe { .. }) {
                    // Close the pipe immediately without sending `stop` so the service worker keeps streaming.
                    break link.end.kill();
                }
                if detach_only.load(Ordering::SeqCst) && matches!(link.end, LinkEnd::Child { .. }) {
                    // Non-service child process: still send stop cleanly on GUI exit.
                    if stop_at.is_none() {
                        if let Ok(mut g) = stdin_slot.lock() {
                            if let Some(w) = g.as_mut() {
                                let _ = w.write_all(HostCmd::Stop.to_line().as_bytes());
                                let _ = w.flush();
                            }
                        }
                    }
                }
                let t = *stop_at.get_or_insert_with(Instant::now);
                let grace = if matches!(link.end, LinkEnd::Pipe { .. }) {
                    Duration::from_secs(4)
                } else {
                    Duration::from_secs(2)
                };
                if t.elapsed() > grace {
                    break link.end.kill();
                }
            }
            thread::sleep(Duration::from_millis(50));
        };
        if let Ok(mut g) = stdin_slot.lock() {
            *g = None;
        }
        // A pipe link only ends when its I/O thread did, which also ends the reader.
        for r in readers {
            let _ = r.join();
        }
        let code = status;
        if stop.load(Ordering::SeqCst) {
            send(UiMsg::Stopped);
            return;
        }
        if started.elapsed() > Duration::from_secs(15) {
            backoff = Duration::from_secs(1);
            svc_quick_fails = 0;
        } else if allow_service && matches!(link.end, LinkEnd::Pipe { .. }) {
            svc_quick_fails += 1;
            if svc_quick_fails >= 2 {
                allow_service = false;
                send(UiMsg::Event(HostEvent::Log {
                    level: LogLevel::Warn,
                    msg: "host: dịch vụ RemoteView lỗi liên tục, chuyển sang chạy tiến trình con (không xem được màn hình khóa)".into(),
                }));
            }
        }
        send(UiMsg::Exited { code, restart_in: Some(backoff) });
        // interruptible sleep
        let until = Instant::now() + backoff;
        while Instant::now() < until && !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(Duration::from_secs(15));
    }
    send(UiMsg::Stopped);
}

pub fn new_channel() -> (Sender<UiMsg>, Receiver<UiMsg>) {
    channel()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn runs_script_and_parses() {
        let (tx, rx) = new_channel();
        let p = HostProcess::start(
            PathBuf::from("sh"),
            vec![
                "-c".into(),
                r#"echo '{"event":"status","state":"waiting","link":"usb","peer":null,"otp":"111111","message":"x"}'; read l; echo "$l" >&2; sleep 1"#.into(),
            ],
            tx,
            || {},
        );
        let m = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(m, UiMsg::Event(HostEvent::Status(_))));
        p.send(&HostCmd::NewOtp);
        let mut got = false;
        while let Ok(m) = rx.recv_timeout(Duration::from_secs(3)) {
            if let UiMsg::Event(HostEvent::Log { msg, .. }) = m {
                if msg.contains("new_otp") {
                    got = true;
                    break;
                }
            }
        }
        assert!(got);
        drop(p);
    }

    #[test]
    fn host_command_is_self_worker() {
        let (exe, pre) = host_command(None);
        assert_eq!(pre, vec![WORKER_FLAG.to_string()]);
        assert!(!exe.as_os_str().is_empty());
        let (exe, pre) = host_command(Some("C:/x/rv-host.exe"));
        assert_eq!(exe, PathBuf::from("C:/x/rv-host.exe"));
        assert!(pre.is_empty());
    }

    #[test]
    fn missing_exe_reports() {
        let (tx, rx) = new_channel();
        let _p = HostProcess::start(PathBuf::from("/nonexistent/rv-host"), vec![], tx, || {});
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), UiMsg::SpawnFailed(_)));
    }
}
