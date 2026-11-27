//! RemoteView Windows service (`RemoteView.exe --service`, service name `RemoteViewSvc`).
//!
//! Why: only a LocalSystem process inside the user's console session may open the Winlogon desktop
//! (lock screen / sign-in / UAC), duplicate it and `SendInput` into it. The service therefore
//! launches *its own exe* as `--host-worker` with a SYSTEM token moved to the active console
//! session and relays the worker's JSON lines over the named pipe `\\.\pipe\RemoteViewSvc`
//! (protocol: `crates/host-win/CONTROL_PROTOCOL.md`, pure parts: `rv_host_win::service_proto`).
//!
//! This module also holds the pipe *client* used by the GUI (`host.rs`) and the elevated
//! install / uninstall helpers (`--install-all`, `--uninstall-service`).

/// `RemoteView.exe --service`: run as the Windows service.
pub const SERVICE_FLAG: &str = "--service";
/// `RemoteView.exe --install-all <dir|->`: elevated; install SudoVDA from `<dir>` (unless `-`),
/// create / update the service and start it.
pub const INSTALL_ALL_FLAG: &str = "--install-all";
/// `RemoteView.exe --uninstall-service`: elevated; stop and delete the service.
pub const UNINSTALL_SERVICE_FLAG: &str = "--uninstall-service";

/// Quotes one command-line argument for `CreateProcess` (MSVCRT rules).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Expected service binary path line for `exe`: `"<exe>" --service` (quoted when needed).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn expected_bin_path(exe: &std::path::Path) -> String {
    format!("{} {SERVICE_FLAG}", quote_arg(&exe.to_string_lossy()))
}

/// True when a service's `lpBinaryPathName` points to `exe` with `--service` (case-insensitive).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn bin_path_matches(configured: &str, exe: &std::path::Path) -> bool {
    let trimmed = configured.trim();
    if trimmed.eq_ignore_ascii_case(&expected_bin_path(exe)) {
        return true;
    }
    let exe_str = exe.to_string_lossy();
    let alt_quoted = format!("\"{exe_str}\" {SERVICE_FLAG}");
    let alt_unquoted = format!("{exe_str} {SERVICE_FLAG}");
    trimmed.eq_ignore_ascii_case(&alt_quoted) || trimmed.eq_ignore_ascii_case(&alt_unquoted)
}

#[cfg(windows)]
pub use imp::*;

#[cfg(not(windows))]
pub use stub::*;

#[cfg(not(windows))]
mod stub {
    use rv_host_win::service_proto::{ChanReader, ChanWriter};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    pub struct PipeConn {
        pub reader: ChanReader,
        pub writer: ChanWriter,
        pub close: Arc<AtomicBool>,
        pub done: Arc<AtomicBool>,
    }
    pub fn connect() -> std::io::Result<PipeConn> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    pub fn run_service() -> i32 {
        1
    }
    pub fn install_all_main(_dir: Option<String>) -> i32 {
        1
    }
    pub fn uninstall_main() -> i32 {
        1
    }
    #[allow(dead_code)]
    pub fn service_installed() -> bool {
        false
    }
    #[allow(dead_code)]
    pub fn service_bin_matches_current_exe() -> Option<bool> {
        None
    }
    #[allow(dead_code)]
    pub fn service_needs_repoint() -> bool {
        false
    }
}

#[cfg(windows)]
mod imp {
    use super::quote_arg;
    use rv_host_win::control::{status_json, State};
    use rv_host_win::logfile::write_line;
    use rv_host_win::service_proto::*;
    use std::ffi::c_void;
    use std::io::{self, BufRead, BufReader, Write};
    use std::os::windows::io::FromRawHandle;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::channel;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::*;
    use windows::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
    use windows::Win32::Security::*;
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
    use windows::Win32::System::Pipes::*;
    use windows::Win32::System::RemoteDesktop::{WTSGetActiveConsoleSessionId, WTSQueryUserToken};
    use windows::Win32::System::Services::*;
    use windows::Win32::System::Threading::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn code(e: &windows::core::Error) -> u32 {
        e.code().0 as u32 & 0xFFFF
    }

    fn log(level: &str, m: &str) {
        write_line(level, &format!("service: {m}"));
    }

    // ----------------------------------------------------------------------
    // pipe I/O (one thread per pipe end, polling: see `RawPipe`)
    // ----------------------------------------------------------------------

    struct WinPipe(HANDLE);
    // SAFETY: a pipe handle may be used from any thread; each WinPipe is used by one thread.
    unsafe impl Send for WinPipe {}

    fn pipe_err(e: windows::core::Error) -> io::Error {
        match code(&e) {
            109 | 232 | 233 | 1236 => io::ErrorKind::BrokenPipe.into(), // broken / closing / not connected / aborted
            _ => io::Error::other(e.to_string()),
        }
    }

    impl RawPipe for WinPipe {
        fn available(&mut self) -> io::Result<usize> {
            let mut avail = 0u32;
            // SAFETY: valid handle, out pointer lives for the call.
            unsafe { PeekNamedPipe(self.0, None, 0, None, Some(&mut avail), None) }.map_err(pipe_err)?;
            Ok(avail as usize)
        }
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let mut n = 0u32;
            // SAFETY: as above.
            unsafe { ReadFile(self.0, Some(buf), Some(&mut n), None) }.map_err(pipe_err)?;
            Ok(n as usize)
        }
        fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
            let mut off = 0;
            while off < data.len() {
                let mut n = 0u32;
                // SAFETY: as above.
                unsafe { WriteFile(self.0, Some(&data[off..]), Some(&mut n), None) }.map_err(pipe_err)?;
                if n == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                off += n as usize;
            }
            Ok(())
        }
    }

    // ----------------------------------------------------------------------
    // GUI side: pipe client
    // ----------------------------------------------------------------------

    /// A connected service pipe, seen through channels (the I/O thread owns the handle).
    pub struct PipeConn {
        pub reader: ChanReader,
        pub writer: ChanWriter,
        /// Set to drop the connection (the service then stops the worker).
        pub close: Arc<AtomicBool>,
        /// Set by the I/O thread when the pipe is gone.
        pub done: Arc<AtomicBool>,
    }

    /// Connects to the service pipe. `NotFound` = service not running; other errors = unusable
    /// (busy for ~2 s, access denied, ...): the caller falls back to a child process.
    pub fn connect() -> io::Result<PipeConn> {
        let name = wide(PIPE_NAME);
        let deadline = Instant::now() + Duration::from_secs(2);
        let h = loop {
            // SAFETY: `name` is NUL-terminated and outlives the call.
            let r = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };
            match r {
                Ok(h) => break h,
                Err(e) if code(&e) == ERROR_PIPE_BUSY.0 && Instant::now() < deadline => {
                    // SAFETY: as above.
                    let _ = unsafe { WaitNamedPipeW(PCWSTR(name.as_ptr()), 300) };
                }
                Err(e) if code(&e) == ERROR_FILE_NOT_FOUND.0 => return Err(io::ErrorKind::NotFound.into()),
                Err(e) => return Err(io::Error::other(e.to_string())),
            }
        };
        let (otx, orx) = channel::<Vec<u8>>();
        let (itx, irx) = channel::<Vec<u8>>();
        let (close, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (c2, d2) = (close.clone(), done.clone());
        let mut pipe = WinPipe(h);
        thread::spawn(move || {
            let _ = pump(&mut pipe, &orx, &itx, &c2);
            d2.store(true, Ordering::SeqCst);
            // SAFETY: we own the handle; nothing uses it after the pump ended.
            unsafe {
                let _ = CloseHandle(pipe.0);
            }
        });
        Ok(PipeConn { reader: ChanReader::new(irx), writer: ChanWriter(otx), close, done })
    }

    // ----------------------------------------------------------------------
    // service side
    // ----------------------------------------------------------------------

    static STATUS_HANDLE: AtomicUsize = AtomicUsize::new(0);
    static STOP: AtomicBool = AtomicBool::new(false);

    fn report(state: SERVICE_STATUS_CURRENT_STATE, accepted: u32, wait_hint: u32) {
        let h = SERVICE_STATUS_HANDLE(STATUS_HANDLE.load(Ordering::SeqCst) as *mut c_void);
        if h.is_invalid() {
            return;
        }
        let st = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: state,
            dwControlsAccepted: accepted,
            dwWin32ExitCode: 0,
            dwServiceSpecificExitCode: 0,
            dwCheckPoint: 0,
            dwWaitHint: wait_hint,
        };
        // SAFETY: valid status handle + struct.
        let _ = unsafe { SetServiceStatus(h, &st) };
    }

    /// Entry of `RemoteView.exe --service`: hands the thread to the service control manager.
    pub fn run_service() -> i32 {
        let _ = rv_host_win::logfile::init("remoteview-service");
        log("info", &format!("start (pid {})", std::process::id()));
        let mut name = wide(SERVICE_NAME);
        let table = [
            SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR(name.as_mut_ptr()), lpServiceProc: Some(service_main) },
            SERVICE_TABLE_ENTRYW { lpServiceName: PWSTR::null(), lpServiceProc: None },
        ];
        // SAFETY: the table is NULL-terminated and outlives the (blocking) call.
        match unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } {
            Ok(()) => 0,
            Err(e) => {
                log("error", &format!("StartServiceCtrlDispatcher: {e} (run it through the service manager, not by hand)"));
                1
            }
        }
    }

    unsafe extern "system" fn ctrl_handler(ctrl: u32, _ev: u32, _data: *mut c_void, _ctx: *mut c_void) -> u32 {
        match ctrl {
            SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
                log("info", "stop requested");
                STOP.store(true, Ordering::SeqCst);
                report(SERVICE_STOP_PENDING, 0, 10_000);
                wake_acceptor();
                0
            }
            SERVICE_CONTROL_INTERROGATE => 0,
            _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
        }
    }

    unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
        let name = wide(SERVICE_NAME);
        // SAFETY: valid NUL-terminated name, handler has the required signature.
        match unsafe { RegisterServiceCtrlHandlerExW(PCWSTR(name.as_ptr()), Some(ctrl_handler), None) } {
            Ok(h) => STATUS_HANDLE.store(h.0 as usize, Ordering::SeqCst),
            Err(e) => {
                log("error", &format!("RegisterServiceCtrlHandlerEx: {e}"));
                return;
            }
        }
        report(SERVICE_START_PENDING, 0, 5_000);
        report(SERVICE_RUNNING, SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN, 0);
        log("info", &format!("running, pipe {PIPE_NAME}"));
        serve();
        log("info", "stopped");
        report(SERVICE_STOPPED, 0, 0);
    }

    /// Connects (and drops) a dummy client so a blocked `ConnectNamedPipe` returns.
    fn wake_acceptor() {
        if let Ok(c) = connect() {
            c.close.store(true, Ordering::SeqCst);
        }
    }

    fn create_pipe() -> Result<HANDLE, String> {
        let sddl = wide(PIPE_SDDL);
        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: NUL-terminated SDDL; `sd` receives a LocalAlloc'ed descriptor (freed below).
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(PCWSTR(sddl.as_ptr()), SDDL_REVISION_1, &mut sd, None) }
            .map_err(|e| format!("pipe ACL: {e}"))?;
        let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.0, bInheritHandle: false.into() };
        let name = wide(PIPE_NAME);
        // One instance only (FIRST_PIPE_INSTANCE stops a squatter, max 1 = one client at a time).
        // SAFETY: valid name + attributes for the duration of the call.
        let h = unsafe {
            CreateNamedPipeW(
                PCWSTR(name.as_ptr()),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64 * 1024,
                64 * 1024,
                0,
                Some(&sa),
            )
        };
        let err = windows::core::Error::from_thread();
        // SAFETY: the descriptor was allocated by the call above and the pipe keeps its own copy.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
        if h.is_invalid() {
            return Err(format!("CreateNamedPipe: {err}"));
        }
        Ok(h)
    }

    fn serve() {
        let pipe = match create_pipe() {
            Ok(h) => h,
            Err(e) => {
                log("error", &e);
                return;
            }
        };
        while !STOP.load(Ordering::SeqCst) {
            // SAFETY: valid pipe handle; blocks until a client connects (or wake_acceptor).
            let r = unsafe { ConnectNamedPipe(pipe, None) };
            match r {
                Ok(()) => {}
                Err(e) if code(&e) == ERROR_PIPE_CONNECTED.0 => {}
                Err(e) => {
                    log("error", &format!("ConnectNamedPipe: {e}"));
                    thread::sleep(Duration::from_millis(500));
                    // SAFETY: valid handle.
                    unsafe {
                        let _ = DisconnectNamedPipe(pipe);
                    }
                    continue;
                }
            }
            if STOP.load(Ordering::SeqCst) {
                break;
            }
            log("info", "client connected");
            handle_client(pipe);
            // SAFETY: valid handle; the client end is closed / done.
            unsafe {
                let _ = DisconnectNamedPipe(pipe);
            }
            log("info", "client disconnected");
        }
        // SAFETY: we own the handle.
        unsafe {
            let _ = CloseHandle(pipe);
        }
    }

    /// Runs one client session on the already connected pipe.
    fn handle_client(pipe: HANDLE) {
        let (otx, orx) = channel::<Vec<u8>>(); // service -> client
        let (itx, irx) = channel::<Vec<u8>>(); // client -> service
        let stop_pump = Arc::new(AtomicBool::new(false));
        let pump_thread = {
            let (mut p, sp) = (WinPipe(pipe), stop_pump.clone());
            thread::spawn(move || {
                let r = pump(&mut p, &orx, &itx, &sp);
                if let Err(e) = r {
                    log("info", &format!("pipe closed: {e}"));
                }
            })
        };
        let fail = |otx: &std::sync::mpsc::Sender<Vec<u8>>, m: &str| {
            log("warn", m);
            let mut l = status_json(State::Error, "none", None, "", &format!("service: {m}")).to_string();
            l.push('\n');
            let _ = otx.send(l.into_bytes());
        };

        let first = read_first_line(&irx, SPAWN_TIMEOUT).and_then(|(line, rest)| parse_spawn_line(&line).map(|a| (a, rest)));
        let (args, rest) = match first {
            Ok(v) => v,
            Err(m) => {
                fail(&otx, &m);
                drop(otx);
                let _ = pump_thread.join(); // flushes the error line, then ends (outgoing closed)
                thread::sleep(Duration::from_millis(150)); // let the client read it before disconnecting
                return;
            }
        };
        log("info", &format!("spawn worker, args: {}", args.join(" ")));
        let mut worker = match spawn_worker(&args) {
            Ok(w) => w,
            Err(m) => {
                fail(&otx, &format!("không chạy được tiến trình host: {m}"));
                drop(otx);
                let _ = pump_thread.join();
                thread::sleep(Duration::from_millis(150));
                return;
            }
        };
        log("info", &format!("worker pid {} started in console session", worker.pid));

        // worker stdout -> client (every complete line); closing the sender ends the pump.
        let reader = {
            let out = worker.stdout.take().expect("stdout");
            thread::spawn(move || {
                let mut br = BufReader::new(out);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match br.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if otx.send(line.clone()).is_err() {
                                break;
                            }
                        }
                    }
                }
            })
        };

        // client -> worker stdin
        let mut stdin = worker.stdin.take().expect("stdin");
        let mut alive = stdin.write_all(&rest).and_then(|_| stdin.flush()).is_ok();
        while alive && !STOP.load(Ordering::SeqCst) {
            match irx.recv_timeout(Duration::from_millis(200)) {
                Ok(chunk) => alive = stdin.write_all(&chunk).and_then(|_| stdin.flush()).is_ok(),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => alive = worker.running(),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // client gone or worker output ended
            }
        }
        if worker.running() {
            log("info", "client left: asking the worker to stop");
            let _ = stdin.write_all(b"{\"cmd\":\"stop\"}\n");
            let _ = stdin.flush();
            if !worker.wait(STOP_GRACE) {
                log("warn", "worker did not stop in time: terminating");
                worker.kill();
            }
        }
        drop(stdin);
        let _ = reader.join();
        stop_pump.store(true, Ordering::SeqCst);
        let _ = pump_thread.join();
        log("info", &format!("worker exit code {:?}", worker.exit_code()));
    }

    struct Worker {
        process: HANDLE,
        pid: u32,
        stdin: Option<std::fs::File>,
        stdout: Option<std::fs::File>,
    }

    impl Worker {
        fn running(&self) -> bool {
            // SAFETY: valid process handle.
            unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT }
        }
        fn wait(&self, d: Duration) -> bool {
            // SAFETY: valid process handle.
            unsafe { WaitForSingleObject(self.process, d.as_millis() as u32) == WAIT_OBJECT_0 }
        }
        fn kill(&self) {
            // SAFETY: valid process handle.
            unsafe {
                let _ = TerminateProcess(self.process, 1);
                WaitForSingleObject(self.process, 2000);
            }
        }
        fn exit_code(&self) -> Option<u32> {
            let mut c = 0u32;
            // SAFETY: valid process handle.
            unsafe { GetExitCodeProcess(self.process, &mut c) }.ok().map(|_| c)
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            // SAFETY: we own the process handle.
            unsafe {
                let _ = CloseHandle(self.process);
            }
        }
    }

    /// Launches `<this exe> --host-worker <args>` as SYSTEM in the active console session.
    fn spawn_worker(args: &[String]) -> Result<Worker, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut cmd = quote_arg(&exe.to_string_lossy());
        cmd.push_str(" --host-worker");
        for a in args {
            cmd.push(' ');
            cmd.push_str(&quote_arg(a));
        }
        let mut cmdw = wide(&cmd);
        let mut desktop = wide("winsta0\\default");
        // SAFETY: plain Win32 calls; every handle opened here is closed on all paths below.
        unsafe {
            let session = WTSGetActiveConsoleSessionId();
            if session == 0xFFFF_FFFF {
                return Err("không có phiên console đang hoạt động".into());
            }
            let mut me = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT | TOKEN_ADJUST_SESSIONID, &mut me)
                .map_err(|e| format!("OpenProcessToken: {e}"))?;
            let mut primary = HANDLE::default();
            let r = DuplicateTokenEx(me, TOKEN_ALL_ACCESS, None, SecurityImpersonation, TokenPrimary, &mut primary);
            let _ = CloseHandle(me);
            r.map_err(|e| format!("DuplicateTokenEx: {e}"))?;
            if let Err(e) = SetTokenInformation(primary, TokenSessionId, &session as *const u32 as *const c_void, 4) {
                let _ = CloseHandle(primary);
                return Err(format!("SetTokenInformation(SessionId {session}): {e}"));
            }

            // Environment of the logged-on user (APPDATA / LOCALAPPDATA: same USB token + logs as
            // the user's own worker). Not available on the sign-in screen: fall back to SYSTEM's.
            let mut env: *mut c_void = std::ptr::null_mut();
            let mut user_tok = HANDLE::default();
            if WTSQueryUserToken(session, &mut user_tok).is_ok() {
                if CreateEnvironmentBlock(&mut env, Some(user_tok), false).is_err() {
                    env = std::ptr::null_mut();
                }
                let _ = CloseHandle(user_tok);
            }

            let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: true.into() };
            let (mut in_r, mut in_w, mut out_r, mut out_w) = (HANDLE::default(), HANDLE::default(), HANDLE::default(), HANDLE::default());
            let close_all = |hs: &[HANDLE]| {
                for h in hs {
                    if !h.is_invalid() {
                        let _ = CloseHandle(*h);
                    }
                }
            };
            let pipes = CreatePipe(&mut in_r, &mut in_w, Some(&sa), 0)
                .and_then(|_| CreatePipe(&mut out_r, &mut out_w, Some(&sa), 0))
                // the parent's ends must not leak into the child
                .and_then(|_| SetHandleInformation(in_w, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)))
                .and_then(|_| SetHandleInformation(out_r, HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0)));
            if let Err(e) = pipes {
                close_all(&[in_r, in_w, out_r, out_w, primary]);
                if !env.is_null() {
                    let _ = DestroyEnvironmentBlock(env);
                }
                return Err(format!("CreatePipe: {e}"));
            }

            let si = STARTUPINFOW {
                cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                lpDesktop: PWSTR(desktop.as_mut_ptr()),
                dwFlags: STARTF_USESTDHANDLES,
                hStdInput: in_r,
                hStdOutput: out_w,
                hStdError: out_w, // panics / stray text end up as (non-JSON) log lines in the GUI
                ..Default::default()
            };
            let mut pi = PROCESS_INFORMATION::default();
            let flags = CREATE_NO_WINDOW | if env.is_null() { PROCESS_CREATION_FLAGS(0) } else { CREATE_UNICODE_ENVIRONMENT };
            let r = CreateProcessAsUserW(
                Some(primary),
                PCWSTR::null(),
                Some(PWSTR(cmdw.as_mut_ptr())),
                None,
                None,
                true,
                flags,
                if env.is_null() { None } else { Some(env as *const c_void) },
                PCWSTR::null(),
                &si,
                &mut pi,
            );
            close_all(&[in_r, out_w, primary]);
            if !env.is_null() {
                let _ = DestroyEnvironmentBlock(env);
            }
            if let Err(e) = r {
                close_all(&[in_w, out_r]);
                return Err(format!("CreateProcessAsUser: {e}"));
            }
            let _ = CloseHandle(pi.hThread);
            Ok(Worker {
                process: pi.hProcess,
                pid: pi.dwProcessId,
                stdin: Some(std::fs::File::from_raw_handle(in_w.0)),
                stdout: Some(std::fs::File::from_raw_handle(out_r.0)),
            })
        }
    }

    // ----------------------------------------------------------------------
    // install / uninstall (elevated)
    // ----------------------------------------------------------------------

    struct Sc(SC_HANDLE);
    impl Drop for Sc {
        fn drop(&mut self) {
            // SAFETY: we own the handle.
            unsafe {
                let _ = CloseServiceHandle(self.0);
            }
        }
    }

    fn open_scm(access: u32) -> Result<Sc, String> {
        // SAFETY: null machine / database = local active database.
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), access) }.map(Sc).map_err(|e| format!("OpenSCManager: {e}"))
    }

    fn open_service(scm: &Sc, access: u32) -> windows::core::Result<Sc> {
        let name = wide(SERVICE_NAME);
        // SAFETY: NUL-terminated name.
        unsafe { OpenServiceW(scm.0, PCWSTR(name.as_ptr()), access) }.map(Sc)
    }

    /// Whether the service is registered (works without elevation).
    #[allow(dead_code)]
    pub fn service_installed() -> bool {
        open_scm(SC_MANAGER_CONNECT).ok().is_some_and(|scm| open_service(&scm, SERVICE_QUERY_STATUS).is_ok())
    }

    /// Queries `RemoteViewSvc`'s configured binary path (read-only `SERVICE_QUERY_CONFIG`,
    /// works without admin elevation). Returns `None` if the service is not installed or
    /// cannot be queried; `Some(true)` if it matches the running exe; `Some(false)` otherwise.
    #[allow(dead_code)]
    pub fn service_bin_matches_current_exe() -> Option<bool> {
        let exe = std::env::current_exe().ok()?;
        let scm = open_scm(SC_MANAGER_CONNECT).ok()?;
        let svc = open_service(&scm, SERVICE_QUERY_CONFIG).ok()?;
        let mut needed = 0u32;
        // First call asks for required buffer size (fails with ERROR_INSUFFICIENT_BUFFER).
        // SAFETY: valid handle; null buffer with size 0 queries the byte count.
        let _ = unsafe { QueryServiceConfigW(svc.0, None, 0, &mut needed) };
        if needed == 0 {
            return None;
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut buf = vec![0usize; words];
        let buf_bytes = (words * std::mem::size_of::<usize>()) as u32;
        let cfg_ptr = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        // SAFETY: `buf` is usize-aligned, at least `needed` bytes, and outlives the call.
        unsafe { QueryServiceConfigW(svc.0, Some(cfg_ptr), buf_bytes, &mut needed) }.ok()?;
        // SAFETY: `QueryServiceConfigW` succeeded and populated `lpBinaryPathName` inside `buf`.
        let bin_pwstr = unsafe { (*cfg_ptr).lpBinaryPathName };
        if bin_pwstr.is_null() {
            return Some(false);
        }
        // SAFETY: `lpBinaryPathName` is a NUL-terminated UTF-16 string within `buf`.
        let configured = unsafe { bin_pwstr.to_string().ok()? };
        Some(super::bin_path_matches(&configured, &exe))
    }

    /// `true` when `RemoteViewSvc` is installed AND points to a different exe path than current_exe.
    #[allow(dead_code)]
    pub fn service_needs_repoint() -> bool {
        service_bin_matches_current_exe() == Some(false)
    }

    /// Enables Windows Software SAS policy (`SoftwareSASGeneration = 1`) so `SendSAS(FALSE)`
    /// works for Ctrl+Alt+Del from the LocalSystem service. Non-fatal if policy is locked.
    fn enable_software_sas() {
        use winreg::{enums::*, RegKey};
        const SUBKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";
        const VALUE: &str = "SoftwareSASGeneration";
        let key = match RegKey::predef(HKEY_LOCAL_MACHINE).create_subkey(SUBKEY) {
            Ok((k, _)) => k,
            Err(e) => {
                log("warn", &format!("sas policy: cannot open {SUBKEY}: {e}"));
                return;
            }
        };
        let cur: u32 = key.get_value(VALUE).unwrap_or(0);
        if cur & 1 != 0 {
            log("info", &format!("sas policy: {VALUE} already {cur}"));
            return;
        }
        let target = cur | 1;
        match key.set_value(VALUE, &target) {
            Ok(()) => log("info", &format!("sas policy: set {VALUE} = {target}")),
            Err(e) => log("warn", &format!("sas policy: failed to set {VALUE}: {e}")),
        }
    }

    fn state_of(svc: &Sc) -> Option<SERVICE_STATUS_CURRENT_STATE> {
        let mut st = SERVICE_STATUS::default();
        // SAFETY: valid handle + out struct.
        unsafe { QueryServiceStatus(svc.0, &mut st) }.ok().map(|_| st.dwCurrentState)
    }

    fn stop_and_wait(svc: &Sc) {
        let mut st = SERVICE_STATUS::default();
        // SAFETY: valid handle + out struct.
        let _ = unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) };
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(15) && state_of(svc).is_some_and(|s| s != SERVICE_STOPPED) {
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// Creates or updates the service (auto start, LocalSystem, `"<this exe>" --service`) and starts it.
    fn install_service() -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let bin = wide(&super::expected_bin_path(&exe));
        let (name, display) = (wide(SERVICE_NAME), wide(SERVICE_DISPLAY));
        let scm = open_scm(SC_MANAGER_ALL_ACCESS)?;
        let svc = match open_service(&scm, SERVICE_ALL_ACCESS) {
            Ok(svc) => {
                // Already there (maybe an older exe path): stop, repoint, start again.
                if state_of(&svc).is_some_and(|s| s != SERVICE_STOPPED) {
                    stop_and_wait(&svc);
                }
                // SAFETY: valid handle; wide strings outlive the call; NULL = leave unchanged.
                unsafe {
                    ChangeServiceConfigW(
                        svc.0,
                        ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                        SERVICE_AUTO_START,
                        SERVICE_ERROR_NORMAL,
                        PCWSTR(bin.as_ptr()),
                        PCWSTR::null(),
                        None,
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR(display.as_ptr()),
                    )
                }
                .map_err(|e| format!("ChangeServiceConfig: {e}"))?;
                log("info", "install: service updated");
                svc
            }
            Err(e) if code(&e) == ERROR_SERVICE_DOES_NOT_EXIST.0 => {
                // SAFETY: as above; NULL start name = LocalSystem.
                let svc = unsafe {
                    CreateServiceW(
                        scm.0,
                        PCWSTR(name.as_ptr()),
                        PCWSTR(display.as_ptr()),
                        SERVICE_ALL_ACCESS,
                        SERVICE_WIN32_OWN_PROCESS,
                        SERVICE_AUTO_START,
                        SERVICE_ERROR_NORMAL,
                        PCWSTR(bin.as_ptr()),
                        PCWSTR::null(),
                        None,
                        PCWSTR::null(),
                        PCWSTR::null(),
                        PCWSTR::null(),
                    )
                }
                .map(Sc)
                .map_err(|e| format!("CreateService: {e}"))?;
                log("info", "install: service created");
                svc
            }
            Err(e) => return Err(format!("OpenService: {e}")),
        };
        let mut desc = wide("Cho phép RemoteView hiển thị và điều khiển cả màn hình khóa / đăng nhập / UAC của Windows.");
        let info = SERVICE_DESCRIPTIONW { lpDescription: PWSTR(desc.as_mut_ptr()) };
        // SAFETY: `info` outlives the call. Cosmetic: errors ignored.
        let _ = unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, Some(&info as *const _ as *const c_void)) };
        enable_software_sas();
        // SAFETY: valid handle, no start arguments.
        match unsafe { StartServiceW(svc.0, None) } {
            Ok(()) => {}
            Err(e) if code(&e) == ERROR_SERVICE_ALREADY_RUNNING.0 => {}
            Err(e) => return Err(format!("StartService: {e}")),
        }
        log("info", "install: service started");
        Ok(())
    }

    /// `--install-all <dir|->`: SudoVDA (unless `-`) + service. Exit code 0 only if both worked.
    pub fn install_all_main(dir: Option<String>) -> i32 {
        let _ = rv_host_win::logfile::init("remoteview-service");
        let mut ok = true;
        match dir.as_deref() {
            None => return 2,
            Some("-") => {}
            Some(d) => {
                log("info", &format!("install-all: installing SudoVDA from {d}"));
                match rv_host_win::sudovda::install(std::path::Path::new(d)) {
                    Ok(()) => log("info", "install-all: SudoVDA done"),
                    Err(e) => {
                        log("error", &format!("install-all: SudoVDA: {e}"));
                        ok = false;
                    }
                }
            }
        }
        match install_service() {
            Ok(()) => log("info", "install-all: service done"),
            Err(e) => {
                log("error", &format!("install-all: service: {e}"));
                ok = false;
            }
        }
        i32::from(!ok)
    }

    /// `--uninstall-service`: stop + delete the service.
    pub fn uninstall_main() -> i32 {
        let _ = rv_host_win::logfile::init("remoteview-service");
        let r = open_scm(SC_MANAGER_ALL_ACCESS).and_then(|scm| match open_service(&scm, SERVICE_ALL_ACCESS) {
            Ok(svc) => {
                if state_of(&svc).is_some_and(|s| s != SERVICE_STOPPED) {
                    stop_and_wait(&svc);
                }
                // SAFETY: valid handle.
                unsafe { DeleteService(svc.0) }.map_err(|e| format!("DeleteService: {e}"))
            }
            Err(e) if code(&e) == ERROR_SERVICE_DOES_NOT_EXIST.0 => Ok(()),
            Err(e) => Err(format!("OpenService: {e}")),
        });
        match r {
            Ok(()) => {
                log("info", "uninstall: done");
                0
            }
            Err(e) => {
                log("error", &format!("uninstall: {e}"));
                1
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{bin_path_matches, quote_arg};
    use std::path::Path;

    #[test]
    fn quoting() {
        assert_eq!(quote_arg("--fps"), "--fps");
        assert_eq!(quote_arg(r"\\.\DISPLAY1"), r"\\.\DISPLAY1");
        assert_eq!(quote_arg(r"C:\Program Files\RemoteView.exe"), r#""C:\Program Files\RemoteView.exe""#);
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
    }

    #[test]
    fn bin_path_matching() {
        let p = Path::new(r"C:\Tools\RemoteView.exe");
        assert!(bin_path_matches(r"C:\Tools\RemoteView.exe --service", p));
        assert!(bin_path_matches(r#""C:\Tools\RemoteView.exe" --service"#, p));
        assert!(bin_path_matches(r"c:\tools\remoteview.exe --service", p));
        assert!(!bin_path_matches(r"C:\Other\RemoteView.exe --service", p));

        let spaced = Path::new(r"C:\Program Files\RemoteView\RemoteView.exe");
        assert!(bin_path_matches(r#""C:\Program Files\RemoteView\RemoteView.exe" --service"#, spaced));
        assert!(!bin_path_matches(r#""C:\Old\RemoteView.exe" --service"#, spaced));
    }
}
