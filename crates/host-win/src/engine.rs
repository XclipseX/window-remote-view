//! Host engine: Windows host for the USB second display (`rv-host`, and
//! `RemoteView.exe --host-worker`).
//!
//! Threads: video (capture+encode), audio (loopback+Opus), devices (USB/adb
//! polling), stdin (GUI commands, `--json-status` only), main (session, link
//! I/O, input). Frames cross threads as already-encoded bytes.
//!
//! The control contract for a GUI front-end is in `CONTROL_PROTOCOL.md`.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rv_crypto::Keypair;
use crate::audio::{LoopbackCapture, OpusFrameEncoder, CHANNELS_RATE, FRAME_MS};
use crate::capture::{list_outputs, pick_output, CapturedFrame, Capturer, DesktopDuplicator};
use crate::control::{self, Command, DeviceItem, Level, State, Stats};
use crate::cursor::{CursorTracker, CursorUpdate};
use crate::logfile::{LogAssembler, WarnGate};
use crate::encode::{
    cap_software, create_encoder, hw_exhausted, on_hw_failure, CodecPref, EncoderConfig, EncoderKind, EncoderPref, HwFailAction,
    HwSupervisor, VideoEncoder,
};
use crate::input::{InputInjector, WinInjector};
use crate::vdisplay::{DisplayMode, ExistingOutput, VirtualDisplay};
use rv_proto::ControlMessage;
use rv_session::udp::{SharedPeer, UdpIo};
use rv_session::{slider_to_bitrate, DatagramIo, HostSession, HostSessionEvent};
use rv_usb_link::{adb, aoa, UsbDatagramLink};

const USAGE: &str = "rv-host [--usb | --adb | --lan] [--otp N] [--width W --height H] [--fps F]
        [--quality 1-100] [--codec auto|hevc|h264] [--encoder auto|hw|sw]
        [--display mirror|extend] [--json-status]
        [--mode WxH@Hz] [--output N] [--monitor NAME] [--bitrate-mbps N] [--bind ADDR:PORT]
        [--scan-all] [--list]
  --usb   AOA bulk link, falls back to `adb reverse` (default when a device is attached)
  --adb   only the adb-reverse TCP fallback
  --lan   UDP on --bind (default 0.0.0.0:47800)
  --monitor NAME  display to share, e.g. \\\\.\\DISPLAY1 (default: the laptop's own panel)
  --json-status  machine-readable JSON lines on stdout / commands on stdin
                 (see CONTROL_PROTOCOL.md)";

macro_rules! info {
    ($($a:tt)*) => { control::log(Level::Info, &format!($($a)*)) };
}
macro_rules! warn {
    ($($a:tt)*) => { control::log(Level::Warn, &format!($($a)*)) };
}
macro_rules! error {
    ($($a:tt)*) => { control::log(Level::Error, &format!($($a)*)) };
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Auto,
    Usb,
    Adb,
    Lan,
}

struct Args {
    mode: Mode,
    mirror: bool,
    /// `--display virtual`: own SudoVDA monitor, made the primary display.
    virtual_primary: bool,
    display_mode: DisplayMode,
    output: Option<usize>,
    /// GDI device name of the output to share in mirror mode (None = auto: internal panel).
    monitor: Option<String>,
    fps: u32,
    width: u32,
    height: u32,
    quality: u8,
    codec: CodecPref,
    encoder: EncoderPref,
    json: bool,
    bitrate_mbps: Option<u32>,
    bind: SocketAddr,
    otp: Option<String>,
    scan_all: bool,
    list: bool,
}

/// Whether the engine's own parser accepts `argv` (used by the service to vet client requests).
pub fn check_args(argv: &[String]) -> Result<(), String> {
    parse_args(argv.to_vec()).map(|_| ())
}

fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut a = Args {
        mode: Mode::Auto,
        mirror: false,
        virtual_primary: false,
        display_mode: DisplayMode::TAB_S10_PLUS,
        output: None,
        monitor: None,
        fps: 60,
        width: 0,
        height: 0,
        quality: rv_session::DEFAULT_QUALITY,
        codec: CodecPref::Auto,
        encoder: EncoderPref::Auto,
        json: false,
        bitrate_mbps: None,
        bind: "0.0.0.0:47800".parse().unwrap(),
        otp: None,
        scan_all: false,
        list: false,
    };
    let mut explicit_mode = false;
    let mut it = argv.into_iter();
    while let Some(k) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match k.as_str() {
            "--usb" => a.mode = Mode::Usb,
            "--adb" => a.mode = Mode::Adb,
            "--lan" => a.mode = Mode::Lan,
            "--display" => match val("--display")?.as_str() {
                "extend" => (a.mirror, a.virtual_primary) = (false, false),
                "mirror" => (a.mirror, a.virtual_primary) = (true, false),
                "virtual" => (a.mirror, a.virtual_primary) = (false, true),
                o => return Err(format!("--display: extend|mirror|virtual, got {o}")),
            },
            "--mode" => {
                a.display_mode = DisplayMode::parse(&val("--mode")?).ok_or("bad --mode, want WxH@Hz")?;
                explicit_mode = true;
            }
            "--output" => a.output = Some(val("--output")?.parse().map_err(|_| "bad --output")?),
            "--monitor" => a.monitor = Some(val("--monitor")?).filter(|m| !m.trim().is_empty()),
            "--fps" => {
                a.fps = val("--fps")?.parse().map_err(|_| "bad --fps")?;
                if !(1..=240).contains(&a.fps) {
                    return Err("--fps must be 1..=240".into());
                }
            }
            "--width" => a.width = val("--width")?.parse().map_err(|_| "bad --width")?,
            "--height" => a.height = val("--height")?.parse().map_err(|_| "bad --height")?,
            "--quality" => {
                a.quality = val("--quality")?.parse().map_err(|_| "bad --quality")?;
                if !(1..=100).contains(&a.quality) {
                    return Err("--quality must be 1..=100".into());
                }
            }
            "--codec" => {
                a.codec = match val("--codec")?.as_str() {
                    "auto" => CodecPref::Auto,
                    "hevc" | "h265" => CodecPref::Hevc,
                    "h264" => CodecPref::H264,
                    o => return Err(format!("--codec: auto|hevc|h264, got {o}")),
                }
            }
            "--encoder" => {
                a.encoder = match val("--encoder")?.as_str() {
                    "auto" => EncoderPref::Auto,
                    "hw" => EncoderPref::Hw,
                    "sw" => EncoderPref::Sw,
                    o => return Err(format!("--encoder: auto|hw|sw, got {o}")),
                }
            }
            "--json-status" => a.json = true,
            "--bitrate-mbps" => a.bitrate_mbps = Some(val("--bitrate-mbps")?.parse().map_err(|_| "bad bitrate")?),
            "--bind" => a.bind = val("--bind")?.parse().map_err(|_| "bad --bind")?,
            "--otp" => {
                let o = val("--otp")?;
                if o.len() < 6 || o.len() > 9 || !o.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("--otp must be 6-9 digits".into());
                }
                a.otp = Some(o);
            }
            "--scan-all" => a.scan_all = true,
            "--list" => a.list = true,
            "-h" | "--help" => return Err(String::new()),
            o => return Err(format!("unknown argument {o}")),
        }
    }
    if (a.width == 0) != (a.height == 0) {
        return Err("--width and --height must be given together".into());
    }
    if a.width > 0 && !explicit_mode {
        a.display_mode.width = a.width;
        a.display_mode.height = a.height;
    }
    Ok(a)
}

// --------------------------------------------------------------------------
// transports
// --------------------------------------------------------------------------

/// Cloneable handle on the LAN socket so it survives session restarts.
#[derive(Clone)]
struct LanHandle {
    io: Arc<Mutex<UdpIo>>,
    rx: Arc<Mutex<Receiver<Vec<u8>>>>,
    peer: SharedPeer,
    established: Arc<AtomicBool>,
}

/// Truncates `s` to at most `max_bytes` UTF-8 bytes without splitting a char.
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Datagram / byte counters of one link (black-screen debugging).
#[derive(Default)]
struct IoStats {
    dg_in: AtomicU64,
    dg_out: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
}

enum TransportKind {
    Stream(UsbDatagramLink),
    Lan(LanHandle),
}

/// Link abstraction so one session loop serves USB, adb and LAN.
struct Transport {
    kind: TransportKind,
    io: Arc<IoStats>,
}

impl DatagramIo for Transport {
    fn send(&mut self, buf: &[u8]) {
        self.io.dg_out.fetch_add(1, Ordering::Relaxed);
        self.io.bytes_out.fetch_add(buf.len() as u64, Ordering::Relaxed);
        match &mut self.kind {
            TransportKind::Stream(l) => l.send(buf),
            TransportKind::Lan(h) => h.io.lock().unwrap().send(buf),
        }
    }
}

/// Keeps Windows system timer resolution at 1 ms for the host engine's lifetime
/// (without this, `sleep` / `recv_timeout` / `AcquireNextFrame` quantize to 15.625 ms,
/// capping streaming at ~58-64 FPS).
struct HighResTimerGuard {
    #[cfg(windows)]
    active: bool,
}

impl HighResTimerGuard {
    fn new() -> Self {
        #[cfg(windows)]
        {
            let r = unsafe { windows::Win32::Media::timeBeginPeriod(1) };
            if r == 0 {
                info!("timer: Windows timeBeginPeriod(1) enabled");
                Self { active: true }
            } else {
                warn!("timer: timeBeginPeriod(1) returned {r}");
                Self { active: false }
            }
        }
        #[cfg(not(windows))]
        {
            Self {}
        }
    }
}

impl Drop for HighResTimerGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.active {
            unsafe {
                let _ = windows::Win32::Media::timeEndPeriod(1);
            }
        }
    }
}

fn boost_thread_priority(name: &str) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};
        if let Err(e) = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL) {
            warn!("priority: SetThreadPriority({name}) failed: {e}");
        }
    }
    #[cfg(not(windows))]
    let _ = name;
}

impl Transport {
    fn stream(l: UsbDatagramLink) -> Self {
        Self { kind: TransportKind::Stream(l), io: Arc::default() }
    }
    fn lan(h: LanHandle) -> Self {
        Self { kind: TransportKind::Lan(h), io: Arc::default() }
    }
    fn try_recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        let r = match &mut self.kind {
            TransportKind::Stream(l) => {
                if l.is_broken() {
                    return Err(format!("link write failed ({})", l.stats().last_error().unwrap_or_default()));
                }
                l.try_recv().map_err(|e| format!("{e} ({})", l.stats().last_error().unwrap_or_default()))
            }
            TransportKind::Lan(h) => match h.rx.lock().unwrap().try_recv() {
                Ok(v) => Ok(Some(v)),
                Err(TryRecvError::Empty) => Ok(None),
                Err(TryRecvError::Disconnected) => Err("udp receiver stopped".into()),
            },
        };
        if let Ok(Some(v)) = &r {
            self.io.dg_in.fetch_add(1, Ordering::Relaxed);
            self.io.bytes_in.fetch_add(v.len() as u64, Ordering::Relaxed);
        }
        r
    }
    fn recv(&mut self, t: Duration) -> Result<Option<Vec<u8>>, String> {
        let r = match &mut self.kind {
            TransportKind::Stream(l) => {
                if l.is_broken() {
                    return Err(format!("link write failed ({})", l.stats().last_error().unwrap_or_default()));
                }
                l.recv_timeout(t).map_err(|e| format!("{e} ({})", l.stats().last_error().unwrap_or_default()))
            }
            TransportKind::Lan(h) => match h.rx.lock().unwrap().recv_timeout(t) {
                Ok(v) => Ok(Some(v)),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err("udp receiver stopped".into()),
            },
        };
        if let Ok(Some(v)) = &r {
            self.io.dg_in.fetch_add(1, Ordering::Relaxed);
            self.io.bytes_in.fetch_add(v.len() as u64, Ordering::Relaxed);
        }
        r
    }
    fn reliable(&self) -> bool {
        matches!(self.kind, TransportKind::Stream(_))
    }
    fn usb_stats(&self) -> Option<Arc<rv_usb_link::LinkStats>> {
        match &self.kind {
            TransportKind::Stream(l) => Some(l.stats()),
            TransportKind::Lan(_) => None,
        }
    }
}

fn open_lan(bind: SocketAddr, host_id: [u8; 16], sh: Arc<Shared>) -> Result<LanHandle, String> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let established = Arc::new(AtomicBool::new(false));
    let est_reader = established.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            match UdpIo::bind(bind, None) {
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
                Ok((io, sock)) => {
                    let local_port = sock.local_addr().map(|a| a.port()).unwrap_or(bind.port());
                    let peer = io.peer_handle();
                    let _ = ready_tx.send(Ok(io));

                    // Broadcast LAN discovery beacons while LAN mode is active.
                    let announce = rv_transport::discovery::Announce {
                        host_id,
                        name: truncate_utf8(&control::computer_name(), rv_transport::discovery::MAX_NAME_LEN),
                        port: local_port,
                    };
                    let beacon_sh = sh.clone();
                    let beacon_task = tokio::spawn(async move {
                        let beacon = rv_transport::discovery::run_beacon(
                            announce,
                            Duration::from_secs(1),
                            rv_transport::discovery::DISCOVERY_PORT,
                        );
                        tokio::pin!(beacon);
                        let mut check = tokio::time::interval(Duration::from_millis(250));
                        loop {
                            tokio::select! {
                                res = &mut beacon => {
                                    if let Err(e) = res {
                                        warn!("LAN discovery beacon stopped: {e}");
                                    }
                                    break;
                                }
                                _ = check.tick() => {
                                    if beacon_sh.stop.load(Ordering::Relaxed) {
                                        break;
                                    }
                                }
                            }
                        }
                    });

                    let mut buf = vec![0u8; 65536]; // control packets (cursor, Android logs) can exceed one MTU
                    loop {
                        let (n, from) = tokio::select! {
                            r = sock.recv_from(&mut buf) => match r {
                                Ok(v) => v,
                                Err(_) => break,
                            },
                            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                                if sh.stop.load(Ordering::Relaxed) {
                                    break;
                                }
                                continue;
                            }
                        };
                        // Ignore LAN discovery beacons (including our own broadcast on port 47800).
                        if n >= rv_transport::discovery::MAGIC.len()
                            && buf[..rv_transport::discovery::MAGIC.len()] == rv_transport::discovery::MAGIC
                        {
                            continue;
                        }
                        // Guard SharedPeer: while a session is established, do not let stray UDP
                        // packets from another endpoint overwrite the active peer.
                        {
                            let mut guard = peer.lock().unwrap();
                            if est_reader.load(Ordering::Relaxed) {
                                if let Some(active_peer) = *guard {
                                    if active_peer != from {
                                        continue;
                                    }
                                } else {
                                    *guard = Some(from);
                                }
                            } else {
                                *guard = Some(from);
                            }
                        }
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    beacon_task.abort();
                }
            }
        });
    });
    let io: UdpIo = ready_rx.recv().map_err(|e| e.to_string())??;
    info!("LAN: listening on {bind} (discovery beacon on UDP {})", rv_transport::discovery::DISCOVERY_PORT);
    let peer = io.peer_handle();
    Ok(LanHandle { io: Arc::new(Mutex::new(io)), rx: Arc::new(Mutex::new(rx)), peer, established })
}

fn open_usb(a: &Args, token: &str) -> Result<(Transport, &'static str), String> {
    if a.mode != Mode::Adb {
        let info = aoa::AccessoryInfo::new(&control::computer_name(), token);
        match aoa::AoaDevice::connect(&info, a.scan_all, Duration::from_secs(8)) {
            Ok(dev) => {
                let loc = dev.location();
                info!("USB: AOA accessory open bus={} addr={} iface={} ep_in=0x{:02x} ep_out=0x{:02x}", loc.bus, loc.address, loc.iface, loc.ep_in, loc.ep_out);
                info!("USB: AOA accessory open (identity {}/{} v{}, description \"{}\")", info.manufacturer, info.model, info.version, info.description);
                return Ok((Transport::stream(UsbDatagramLink::from_aoa(dev)), "usb"));
            }
            Err(e) => warn!("USB: AOA failed: {e}"),
        }
    }
    if adb::device_present() {
        info!("USB: trying adb reverse tcp:{}", rv_usb_link::ADB_PORT);
        let l = adb::connect(Duration::from_secs(30)).map_err(|e| e.to_string())?;
        return Ok((Transport::stream(l), "adb"));
    }
    Err("no USB link: AOA failed and no adb device".into())
}

// --------------------------------------------------------------------------
// shared state between threads
// --------------------------------------------------------------------------

struct Shared {
    ready: AtomicBool,
    idr: AtomicBool,
    paused: AtomicBool,
    /// Make the cursor thread forget its state: shape + position are re-sent (new session, IDR request).
    cursor_reset: AtomicBool,
    /// Pending `ControlMessage::RequestLog` to send to the connected Android client.
    log_request: AtomicBool,
    stop: AtomicBool,
    usb_present: AtomicBool,
    quality: AtomicU32,
    fps: AtomicU32,
    /// 0 = native capture size.
    width: AtomicU32,
    height: AtomicU32,
    /// 0 = derive from the quality slider.
    bitrate_override_bps: AtomicU32,
    /// Bumped whenever quality/fps/size/bitrate settings change.
    cfg_gen: AtomicU32,
    /// `codec_flags` of the live encoder (0 = none yet).
    codec_flag: AtomicU32,
    /// Client advertised HEVC decode in Hello (assumed until a Hello says otherwise).
    client_hevc: AtomicBool,
    /// Client panel / decoder hints from `Hello` (0 = unspecified).
    client_max_w: AtomicU32,
    client_max_h: AtomicU32,
    client_max_fps: AtomicU32,
    /// A USB/adb link is open right now (do not probe accessory devices).
    usb_link_open: AtomicBool,
    /// Friendly name from the client's `DeviceInfo`.
    peer_name: Mutex<Option<String>>,
    /// Ask the device poller to publish a fresh list now.
    dev_refresh: AtomicBool,
    /// The input desktop is the lock screen / a secure desktop (nothing can be captured or injected).
    locked: AtomicBool,
    /// Desktop rectangle of the display being captured (follows mode changes).
    cap_rect: Mutex<crate::Rect>,
}

impl Shared {
    fn bump(&self) {
        self.cfg_gen.fetch_add(1, Ordering::SeqCst);
    }
}

struct VideoOut {
    frame_id: u32,
    ts_us: u32,
    key: bool,
    data: Vec<u8>,
}

struct AudioOut {
    seq: u16,
    ts_us: u32,
    payload: Vec<u8>,
}

#[derive(Clone, Copy)]
struct ActiveEnc {
    kind: EncoderKind,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
}

enum VideoEvent {
    Active(ActiveEnc),
    Bitrate(u32),
    Stats { fps: f64, kbps: u64, encode_ms: f64 },
    Fatal(String),
}

struct VideoPrefs {
    codec: CodecPref,
    encoder: EncoderPref,
}

/// A hardware encoder was declared broken: retry it once without D3D, else
/// switch to OpenH264 for the rest of the process.
fn hw_gave_up(kind: EncoderKind, uses_d3d: bool, hevc_ok: bool, reason: &str) {
    warn!("video: hardware encoder {} failed ({reason})", kind.name());
    match on_hw_failure(kind, uses_d3d, hevc_ok) {
        HwFailAction::RetryWithoutD3d => {
            info!("video: {} will be retried in system-memory mode (no D3D11 manager) after the other encoder's D3D path", kind.name())
        }
        HwFailAction::NextEncoder => {
            info!("video: {} blocked; trying the next hardware encoder", kind.name())
        }
        HwFailAction::UseSoftware => {
            warn!("video: falling back to OpenH264 (software H.264) for the rest of the session")
        }
    }
}

/// Encode size for a capture of `nw x nh` when the client / GUI asked for at
/// most `rw x rh` (0 = no limit): the capture's own aspect ratio is kept (the
/// client letterboxes), the picture is never stretched or enlarged.
/// Near-identical aspect ratios (< 1% difference, e.g. 2800x1752 -> 2560x1600)
/// snap to the exact requested dimensions so hardware decoders receive clean
/// 16-px aligned resolutions instead of odd sizes like 2556x1600.
fn fit_size(nw: u32, nh: u32, rw: u32, rh: u32) -> (u32, u32) {
    if rw == 0 || rh == 0 || nw == 0 || nh == 0 || (rw >= nw && rh >= nh) {
        return (nw, nh);
    }
    if rw <= nw && rh <= nh {
        let ar_cap = f64::from(nw) / f64::from(nh);
        let ar_req = f64::from(rw) / f64::from(rh);
        if (ar_req / ar_cap - 1.0).abs() < 0.01 {
            return (rw, rh);
        }
    }
    let k = (f64::from(rw) / f64::from(nw)).min(f64::from(rh) / f64::from(nh));
    (((f64::from(nw) * k).round() as u32).max(2), ((f64::from(nh) * k).round() as u32).max(2))
}

fn is_gpu_frame(frame: &CapturedFrame) -> bool {
    #[cfg(windows)]
    {
        frame.texture().is_some()
    }
    #[cfg(not(windows))]
    {
        let _ = frame;
        false
    }
}

fn video_thread(
    out_name: String,
    out_rect: crate::Rect,
    prefs: VideoPrefs,
    start: Instant,
    sh: Arc<Shared>,
    tx: SyncSender<VideoOut>,
    ev: Sender<VideoEvent>,
) {
    boost_thread_priority("video");
    let mut cap = Capturer::new(Some(out_name), out_rect);
    let mut ready_since: Option<Instant> = None;
    let mut sup = HwSupervisor::new();
    let mut first_input: Option<Instant> = None;
    let mut force_sw = false;
    let mut enc: Option<Box<dyn VideoEncoder>> = None;
    let mut enc_from_gpu = false;
    let mut active: Option<ActiveEnc> = None;
    let mut seen_gen = u32::MAX;
    let mut fail_streak: u32 = 0;
    let mut empty_since: Option<Instant> = None;
    let mut frame_id = 0u32;
    let mut last_sent = Instant::now();
    let mut last_encode = Instant::now() - Duration::from_secs(1);
    let mut last_frame: Option<CapturedFrame> = None;
    let mut dirty = false;
    // stats window
    let (mut w_start, mut w_frames, mut w_bytes, mut w_enc) = (Instant::now(), 0u32, 0u64, Duration::ZERO);

    while !sh.stop.load(Ordering::Relaxed) {
        if !sh.ready.load(Ordering::Relaxed) || sh.paused.load(Ordering::Relaxed) {
            ready_since = None;
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let rs = *ready_since.get_or_insert_with(Instant::now);
        // Compute target cadence before acquire so wait timeout matches the target FPS.
        let sw_mode = force_sw
            || prefs.encoder == EncoderPref::Sw
            || hw_exhausted(sh.client_hevc.load(Ordering::Relaxed) && prefs.codec != CodecPref::H264);
        let mut fps = sh.fps.load(Ordering::Relaxed).max(1);
        let client_max_fps = sh.client_max_fps.load(Ordering::Relaxed);
        if client_max_fps > 0 {
            fps = fps.min(client_max_fps);
        }
        let fps = if sw_mode { fps.min(crate::encode::SW_MAX_FPS) } else { fps };
        let min_interval = Duration::from_micros(1_000_000 / fps as u64);

        let locked = sh.locked.load(Ordering::Relaxed);
        let acquired = if locked {
            // Lock screen / secure desktop: nothing can be captured. Keep the last frame flowing.
            std::thread::sleep(Duration::from_millis(8));
            None
        } else {
            // If we already have an unencoded dirty frame, poll non-blockingly for an even fresher one;
            // otherwise wait up to one frame interval (clamped to 2..=8 ms) for DWM to present.
            let wait_ms = if dirty { 0 } else { (1000 / fps).clamp(2, 8) };
            cap.acquire(wait_ms).unwrap_or(None)
        };
        if let Some(f) = acquired {
            last_frame = Some(f);
            dirty = true;
        }
        let r = cap.desktop_rect();
        if let Ok(mut g) = sh.cap_rect.lock() {
            if *g != r && r.width() > 0 && r.height() > 0 {
                *g = r;
            }
        }
        if last_frame.is_none() && (locked || rs.elapsed() > Duration::from_millis(1500)) {
            // Nothing captured yet (locked at start, or capture failing): send black so the client does not time out.
            let (w, h) = ((r.width().max(2) as u32) & !1, (r.height().max(2) as u32) & !1);
            last_frame = Some(CapturedFrame::from_bgra(w, h, vec![0u8; (w * h * 4) as usize]));
            dirty = true;
        }
        let Some(frame) = last_frame.as_ref() else { continue };

        let want_idr = sh.idr.load(Ordering::Relaxed);
        let tol = (min_interval / 4).min(Duration::from_millis(2));
        let elapsed = last_encode.elapsed();
        let due = dirty && elapsed + tol >= min_interval;
        let keepalive = last_sent.elapsed() > Duration::from_secs(1);
        if !(due || want_idr || keepalive) {
            if dirty {
                let rem = min_interval.saturating_sub(elapsed).min(Duration::from_millis(2));
                if !rem.is_zero() {
                    std::thread::sleep(rem);
                }
            }
            continue;
        }

        // (Re)configure the encoder when settings, capture size, or GPU-texture availability changed.
        let gen = sh.cfg_gen.load(Ordering::SeqCst);
        let (nw, nh) = (frame.width, frame.height);
        let rw = sh.width.load(Ordering::Relaxed);
        let rh = sh.height.load(Ordering::Relaxed);
        let (eff_rw, eff_rh) = if rw == 0 && rh == 0 {
            (sh.client_max_w.load(Ordering::Relaxed), sh.client_max_h.load(Ordering::Relaxed))
        } else {
            (rw, rh)
        };
        let (tw, th) = fit_size(nw, nh, eff_rw, eff_rh);
        let (tw, th) = (tw & !1, th & !1);
        let (tw, th) = if sw_mode { let c = cap_software(tw, th, fps); (c.0, c.1) } else { (tw, th) };
        let q = sh.quality.load(Ordering::Relaxed) as u8;
        let over = sh.bitrate_override_bps.load(Ordering::Relaxed);
        let bitrate_for = |c: rv_session::Codec| -> u32 {
            if over > 0 {
                over
            } else {
                slider_to_bitrate(q, tw, th, fps, c).clamp(300_000, 400_000_000) as u32
            }
        };
        let eff_codec = if prefs.codec == CodecPref::Auto && !sh.client_hevc.load(Ordering::Relaxed) {
            CodecPref::H264
        } else {
            prefs.codec
        };
        let codec_mismatch = eff_codec == CodecPref::H264
            && enc.as_ref().is_some_and(|e| e.codec() == rv_proto::codec_flags::CODEC_H265);
        let gpu_frame = is_gpu_frame(frame);
        let gpu_upgraded = enc.is_some() && gpu_frame && !enc_from_gpu && !sw_mode && prefs.encoder != EncoderPref::Sw;
        if gpu_upgraded {
            info!("video: DXGI GPU frame arrived; upgrading encoder from CPU init to GPU zero-copy");
        }
        let geometry_changed =
            codec_mismatch || gpu_upgraded || active.is_none_or(|a| (a.width, a.height, a.fps) != (tw, th, fps));
        if enc.is_none() || geometry_changed || gen != seen_gen {
            seen_gen = gen;
            let mut applied_live = false;
            if let (Some(e), Some(a)) = (enc.as_mut(), active.as_mut()) {
                if !geometry_changed {
                    let b = bitrate_for(e.kind().slider_codec());
                    if b == a.bitrate_bps {
                        applied_live = true;
                    } else if matches!(e.set_bitrate(b), Ok(true)) {
                        a.bitrate_bps = b;
                        applied_live = true;
                        info!("encoder: bitrate -> {} kbps (live)", b / 1000);
                        let _ = ev.send(VideoEvent::Bitrate(b));
                    }
                }
            }
            if !applied_live {
                drop(enc.take());
                let cfg = EncoderConfig { width: tw, height: th, fps, bitrate_bps: 0 };
                let mut log = |m: &str| info!("{m}");
                match create_encoder(eff_codec, if sw_mode { EncoderPref::Sw } else { prefs.encoder }, cfg, frame, &bitrate_for, &mut log) {
                    Ok(e) => {
                        let a = ActiveEnc {
                            kind: e.kind(),
                            width: tw,
                            height: th,
                            fps,
                            bitrate_bps: bitrate_for(e.kind().slider_codec()),
                        };
                        info!("encoder active: {} {}x{}@{} {} kbps", a.kind.name(), tw, th, fps, a.bitrate_bps / 1000);
                        sh.codec_flag.store(e.codec() as u32, Ordering::SeqCst);
                        enc_from_gpu = gpu_frame;
                        active = Some(a);
                        enc = Some(e);
                        empty_since = None;
                        sup = HwSupervisor::new();
                        first_input = None;
                        sh.idr.store(true, Ordering::Relaxed);
                        let _ = ev.send(VideoEvent::Active(a));
                    }
                    Err(e) => {
                        let _ = ev.send(VideoEvent::Fatal(format!("encoder init failed: {e}")));
                        return;
                    }
                }
            }
        }
        let Some(e) = enc.as_mut() else { continue };

        let force = sh.idr.swap(false, Ordering::Relaxed) || frame_id == 0;
        let t0 = Instant::now();
        let fi = *first_input.get_or_insert(t0);
        let r = e.encode(frame, force);
        let dt = t0.elapsed();
        dirty = false;
        // Pace from the start of the frame so the encode time is not added as an idle penalty.
        last_encode = t0;
        match r {
            Ok(data) if !data.is_empty() => {
                sup.on_output();
                fail_streak = 0;
                empty_since = None;
                w_frames += 1;
                w_bytes += data.len() as u64;
                w_enc += dt;
                let out = VideoOut { frame_id, ts_us: start.elapsed().as_micros() as u32, key: force, data };
                frame_id = frame_id.wrapping_add(1);
                last_sent = Instant::now();
                if tx.try_send(out).is_err() {
                    sh.idr.store(true, Ordering::Relaxed); // dropped a delta: resync
                }
            }
            Ok(_) => {
                if force {
                    sh.idr.store(true, Ordering::Relaxed);
                }
                let k = e.kind();
                let d3d = e.uses_d3d();
                if k != EncoderKind::OpenH264 {
                    // Hardware MFT silent for >1 s after its first input: give up on it.
                    if let Some(reason) = sup.on_empty(fi.elapsed()) {
                        hw_gave_up(k, d3d, sh.client_hevc.load(Ordering::Relaxed) && prefs.codec != CodecPref::H264, reason);
                        force_sw = hw_exhausted(sh.client_hevc.load(Ordering::Relaxed) && prefs.codec != CodecPref::H264);
                        sh.idr.store(true, Ordering::Relaxed);
                        empty_since = None;
                        enc = None;
                        active = None;
                    }
                } else {
                    // A software encoder that accepts input but never produces output for 2 s is broken.
                    let since = *empty_since.get_or_insert_with(Instant::now);
                    if since.elapsed() > Duration::from_secs(2) {
                        warn!("video: encoder {} produced no output for 2 s; rebuilding", k.name());
                        empty_since = None;
                        enc = None;
                        active = None;
                    }
                }
            }
            Err(err) => {
                warn!("video: encode error: {err}");
                fail_streak += 1;
                let k = e.kind();
                if k != EncoderKind::OpenH264 {
                    // Error before any output, or repeated errors: stop using this hardware path now.
                    let first = sup.on_error();
                    let reason = first.or(if fail_streak >= 3 { Some("3 consecutive errors") } else { None });
                    if let Some(reason) = reason {
                        hw_gave_up(k, e.uses_d3d(), sh.client_hevc.load(Ordering::Relaxed) && prefs.codec != CodecPref::H264, reason);
                        force_sw = hw_exhausted(sh.client_hevc.load(Ordering::Relaxed) && prefs.codec != CodecPref::H264);
                        fail_streak = 0;
                    }
                }
                sh.idr.store(true, Ordering::Relaxed);
                // A wedged encoder: rebuild it on the next frame.
                empty_since = None;
                enc = None;
                active = None;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let el = w_start.elapsed();
        if el >= Duration::from_secs(1) {
            let s = el.as_secs_f64();
            let _ = ev.send(VideoEvent::Stats {
                fps: w_frames as f64 / s,
                kbps: (w_bytes as f64 * 8.0 / 1000.0 / s) as u64,
                encode_ms: if w_frames > 0 { w_enc.as_secs_f64() * 1000.0 / w_frames as f64 } else { 0.0 },
            });
            (w_start, w_frames, w_bytes, w_enc) = (Instant::now(), 0, 0, Duration::ZERO);
        }
    }
}

fn audio_thread(start: Instant, sh: Arc<Shared>, tx: SyncSender<AudioOut>) {
    let mut cap = match LoopbackCapture::new() {
        Ok(c) => c,
        Err(e) => return warn!("audio: disabled ({e})"),
    };
    let mut enc = match OpusFrameEncoder::new(128_000) {
        Ok(e) => e,
        Err(e) => return warn!("audio: opus init failed: {e}"),
    };
    let (mut seq, mut pcm) = (0u16, Vec::new());
    while !sh.stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        pcm.clear();
        if let Err(e) = cap.poll(&mut pcm) {
            return warn!("audio: capture error: {e}");
        }
        // Drained above; drop it while no client is ready or the client paused (backgrounded).
        if pcm.is_empty() || !sh.ready.load(Ordering::Relaxed) || sh.paused.load(Ordering::Relaxed) {
            continue;
        }
        match enc.push(&pcm) {
            Ok(pkts) => {
                for p in pkts {
                    let out = AudioOut { seq, ts_us: start.elapsed().as_micros() as u32, payload: p };
                    seq = seq.wrapping_add(1);
                    let _ = tx.try_send(out); // audio: drop on overflow, never block
                }
            }
            Err(e) => warn!("audio: encode error: {e}"),
        }
    }
}

/// While a client is connected: keep Windows awake (no sleep / display off /
/// screen saver), nudge the idle timer, and publish whether the desktop is locked.
fn session_monitor_thread(sh: Arc<Shared>) {
    let mut awake = false;
    let mut last_nudge = Instant::now();
    // SYSTEM: this is the one thread that logs "desktop: Default -> Winlogon"; it also follows
    // the input desktop so the idle nudge below reaches it.
    let mut follower = crate::session_state::DesktopFollower::new("monitor", Duration::from_millis(250), true);
    let system = follower.active();
    while !sh.stop.load(Ordering::Relaxed) {
        follower.poll();
        let ready = sh.ready.load(Ordering::Relaxed);
        if ready != awake {
            awake = ready;
            crate::session_state::set_keep_awake(awake);
            info!("power: {}", if awake { "giữ máy và màn hình luôn bật khi đang chia sẻ" } else { "thôi giữ máy thức" });
        } else if awake && last_nudge.elapsed() >= Duration::from_secs(20) {
            crate::session_state::set_keep_awake(true); // re-assert
        }
        // As SYSTEM the lock screen is capturable and controllable: never treated as "locked".
        let locked = ready && !system && crate::session_state::desktop_locked();
        if sh.locked.swap(locked, Ordering::Relaxed) != locked {
            if locked {
                warn!("session: màn hình đang khóa / secure desktop; không thể ghi hình hay điều khiển cho tới khi mở khóa");
            } else {
                info!("session: đã mở khóa màn hình");
            }
        }
        if awake && !locked && last_nudge.elapsed() >= Duration::from_secs(30) {
            last_nudge = Instant::now();
            crate::session_state::nudge_idle();
        }
        std::thread::sleep(Duration::from_millis(if system { 250 } else { 500 }));
    }
    crate::session_state::set_keep_awake(false);
}

/// Polls the mouse cursor and forwards shape / position changes.
fn cursor_thread(sh: Arc<Shared>, tx: Sender<CursorUpdate>) {
    let mut tracker = CursorTracker::new();
    let mut last_rect = crate::Rect::default();
    let mut scale = 1.0f64;
    // SYSTEM: the cursor of the input desktop (lock screen) is only readable from that desktop.
    let mut follower = crate::session_state::DesktopFollower::new("cursor", Duration::from_millis(100), false);
    while !sh.stop.load(Ordering::Relaxed) {
        if follower.poll() {
            tracker.reset(); // shapes / handles differ per desktop
        }
        if !sh.ready.load(Ordering::Relaxed) {
            tracker.reset(); // the next client gets shape + position again
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if sh.paused.load(Ordering::Relaxed) {
            // Video is paused (client in background): nothing to draw the cursor on.
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        if sh.cursor_reset.swap(false, Ordering::Relaxed) {
            tracker.reset();
        }
        let rect = sh.cap_rect.lock().map(|g| *g).unwrap_or_default();
        if rect != last_rect {
            last_rect = rect;
            let ns = crate::cursor::monitor_dpi_scale(rect);
            if (ns - scale).abs() > 1e-6 {
                info!("cursor: DPI scale vs system DPI {scale:.2} -> {ns:.2}");
                scale = ns;
                tracker.reset(); // re-render the shape at the new scale
            }
        }
        for u in crate::cursor::poll(&mut tracker, rect, scale, Instant::now()) {
            if tx.send(u).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

// --------------------------------------------------------------------------
// device list + stdin helpers
// --------------------------------------------------------------------------

fn adb_serials() -> Vec<String> {
    let mut cmd = std::process::Command::new("adb");
    cmd.arg("devices");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let Ok(out) = cmd.output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut p = l.split_whitespace();
            match (p.next(), p.next()) {
                (Some(s), Some("device")) => Some(s.to_string()),
                _ => None,
            }
        })
        .collect()
}

fn vendor_name(vid: u16) -> &'static str {
    match vid {
        0x04E8 => "Samsung",
        0x18D1 => "Google",
        0x2717 => "Xiaomi",
        0x12D1 => "Huawei",
        0x22B8 => "Motorola",
        0x2A70 => "OnePlus",
        0x1004 => "LG",
        0x0FCE => "Sony",
        0x0BB4 => "HTC",
        0x0B05 => "ASUS",
        0x22D9 => "OPPO",
        0x17EF => "Lenovo",
        _ => "USB",
    }
}

/// Best available display name for a normal-mode candidate.
fn candidate_name(c: &aoa::Candidate) -> String {
    match (&c.manufacturer, &c.product) {
        (Some(m), Some(p)) if !p.starts_with(m.as_str()) => format!("{m} {p}"),
        (_, Some(p)) => p.clone(),
        (Some(m), None) => m.clone(),
        _ => format!("{} {:04x}:{:04x}", vendor_name(c.vid), c.vid, c.pid),
    }
}

/// `last_name` remembers the last normal-mode device name so the same device
/// keeps it after it re-enumerates as an accessory (18D1:2D00).
fn list_devices_with(scan_all: bool, link_open: bool, probe_accessory: bool, peer_name: Option<&str>, last_name: &mut Option<String>) -> Vec<DeviceItem> {
    let mut v = Vec::new();
    for c in aoa::list_candidates_opts(scan_all, probe_accessory).unwrap_or_default() {
        let acc = c.is_accessory();
        let name = if acc {
            peer_name.map(str::to_string).or_else(|| last_name.clone()).unwrap_or_else(|| "Thiết bị Android".to_string())
        } else {
            let n = candidate_name(&c);
            if c.can_open && c.product.is_some() {
                *last_name = Some(n.clone());
            }
            n
        };
        v.push(DeviceItem {
            kind: "usb".into(),
            name,
            serial: format!("{}-{}", c.bus, c.address),
            vid: c.vid,
            pid: c.pid,
            accessory: acc,
            driver_ok: c.can_open,
            connected: acc && link_open,
        });
    }
    for s in adb_serials() {
        v.push(DeviceItem { kind: "adb".into(), name: format!("adb {s}"), serial: s, vid: 0, pid: 0, accessory: false, driver_ok: true, connected: false });
    }
    v
}

/// One-shot device scan for the GUI. The GUI process must call this ONLY while
/// no host worker runs (it opens devices to read strings / test the driver);
/// while the worker runs the GUI uses the worker's `devices` events instead.
pub fn scan_devices(last_name: &mut Option<String>) -> Vec<DeviceItem> {
    list_devices_with(false, false, true, None, last_name)
}

fn list_devices(scan_all: bool, sh: &Shared, last_name: &mut Option<String>) -> Vec<DeviceItem> {
    let peer_name = sh.peer_name.lock().unwrap().clone();
    list_devices_with(scan_all, sh.usb_link_open.load(Ordering::Relaxed), false, peer_name.as_deref(), last_name)
}

fn devices_thread(scan_all: bool, sh: Arc<Shared>) {
    let mut last: Option<Vec<DeviceItem>> = None;
    let mut last_name: Option<String> = None;
    while !sh.stop.load(Ordering::Relaxed) {
        let items = list_devices(scan_all, &sh, &mut last_name);
        sh.usb_present.store(!items.is_empty(), Ordering::Relaxed);
        if last.as_ref() != Some(&items) {
            if control::json_mode() {
                control::emit(&control::devices_json(&items));
            }
            info!("devices: {}", if items.is_empty() { "none".to_string() } else { items.iter().map(|d| format!("{} [{:04x}:{:04x} driver_ok={}]", d.name, d.vid, d.pid, d.driver_ok)).collect::<Vec<_>>().join(", ") });
            last = Some(items);
        }
        for _ in 0..40 {
            if sh.stop.load(Ordering::Relaxed) || sh.dev_refresh.swap(false, Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn stdin_thread(tx: Sender<Command>, sh: Arc<Shared>) {
    use std::io::BufRead;
    let stop = |tx: &Sender<Command>| {
        sh.stop.store(true, Ordering::SeqCst);
        let _ = tx.send(Command::Stop);
        // Main may be blocked in a link open (up to ~30 s): do not hang the GUI.
        std::thread::sleep(Duration::from_secs(2));
        std::process::exit(0);
    };
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match control::parse_command(&line) {
            Ok(Command::Stop) => stop(&tx),
            Ok(c) => {
                let _ = tx.send(c);
            }
            Err(e) => warn!("stdin: {e}"),
        }
    }
    // GUI went away: do not linger as an orphan.
    stop(&tx);
}

// --------------------------------------------------------------------------
// host context + session loop
// --------------------------------------------------------------------------

enum End {
    Stop,
    Reopen,
}

struct Ctx {
    args: Args,
    sh: Arc<Shared>,
    kp: Keypair,
    /// LAN PIN (shown to the user).
    otp: String,
    /// Persistent per-user 8-digit pairing token for USB: AOA serial and session OTP.
    usb_token: String,
    lan: Option<LanHandle>,
    injector: Option<WinInjector>,
    vrx: Receiver<VideoOut>,
    arx: Receiver<AudioOut>,
    evrx: Receiver<VideoEvent>,
    cmdrx: Receiver<Command>,
    curx: Receiver<CursorUpdate>,
    active: Option<ActiveEnc>,
    rtt_us: Option<u32>,
    decode_us: Option<u32>,
    last_video_kbps: u64,
    // last status, so `new_otp` can re-emit it
    state: State,
    link: &'static str,
    peer: Option<String>,
    message: String,
    usb_backoff_until: Instant,
}

impl Ctx {
    /// Session OTP for a link: the pairing token over USB, the PIN otherwise.
    fn otp_for(&self, link: &str) -> String {
        if link == "usb" {
            self.usb_token.clone()
        } else {
            self.otp.clone()
        }
    }
}

/// What a command asks the session loop to do.
enum Action {
    None,
    Stop,
    NewOtp,
}

impl Ctx {
    fn set_status(&mut self, state: State, link: &'static str, peer: Option<String>, message: &str) {
        self.state = state;
        self.link = link;
        self.peer = peer;
        self.message = message.to_string();
        self.emit_status();
    }

    fn emit_status(&self) {
        info!("status: {} link={} peer={:?} otp={} - {}", self.state.as_str(), self.link, self.peer, self.otp, self.message);
        if control::json_mode() {
            control::emit(&control::status_json(self.state, self.link, self.peer.as_deref(), &self.otp, &self.message));
        }
    }

    fn apply(&mut self, cmd: Command) -> Action {
        let sh = &self.sh;
        match cmd {
            Command::SetQuality(q) => {
                sh.quality.store(q as u32, Ordering::Relaxed);
                sh.bitrate_override_bps.store(0, Ordering::Relaxed);
                sh.bump();
                info!("quality -> {q}");
            }
            Command::SetFps(f) => {
                sh.fps.store(f, Ordering::Relaxed);
                sh.bump();
                info!("fps -> {f}");
            }
            Command::SetResolution { width, height } => {
                sh.width.store(width, Ordering::Relaxed);
                sh.height.store(height, Ordering::Relaxed);
                sh.bump();
                if width == 0 && height == 0 {
                    info!("resolution -> native (theo thiết bị nhận)");
                } else {
                    info!("resolution -> {width}x{height}");
                }
            }
            Command::RequestIdr => {
                sh.idr.store(true, Ordering::Relaxed);
                sh.cursor_reset.store(true, Ordering::Relaxed);
            }
            Command::RequestLog => {
                sh.log_request.store(true, Ordering::SeqCst);
            }
            Command::NewOtp => {
                self.otp = control::random_otp();
                self.emit_status();
                return Action::NewOtp;
            }
            Command::Connect { serial } => {
                info!("connect requested (device {serial:?}): retrying USB now");
                self.usb_backoff_until = Instant::now();
                sh.dev_refresh.store(true, Ordering::Relaxed);
            }
            Command::Stop => {
                sh.stop.store(true, Ordering::SeqCst);
                return Action::Stop;
            }
        }
        Action::None
    }

    /// Drains GUI commands; `Some(End::Stop)` if stop was requested.
    fn poll_commands(&mut self) -> (bool, bool) {
        let (mut stop, mut otp) = (false, false);
        while let Ok(c) = self.cmdrx.try_recv() {
            match self.apply(c) {
                Action::Stop => stop = true,
                Action::NewOtp => otp = true,
                Action::None => {}
            }
        }
        (stop, otp)
    }

    fn send_stream_config_only<I: DatagramIo>(&self, session: &mut HostSession<I>) {
        let Some(a) = self.active else { return };
        let _ = session.send_control(ControlMessage::StreamConfig {
            codec: a.kind.codec_flag(),
            width: a.width.min(u16::MAX as u32) as u16,
            height: a.height.min(u16::MAX as u32) as u16,
            fps: a.fps as u16,
            bitrate_kbps: a.bitrate_bps / 1000,
        });
    }

    fn send_stream_config<I: DatagramIo>(&self, session: &mut HostSession<I>) {
        let Some(a) = self.active else { return };
        let _ = session.send_control(ControlMessage::DisplayConfigChange {
            w: a.width.min(u16::MAX as u32) as u16,
            h: a.height.min(u16::MAX as u32) as u16,
            fps: a.fps as u16,
        });
        self.send_stream_config_only(session);
    }

    /// Sleep that keeps servicing GUI commands. Returns true on stop.
    fn idle(&mut self, dur: Duration) -> bool {
        let end = Instant::now() + dur;
        while Instant::now() < end {
            let (stop, _) = self.poll_commands();
            if stop {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn open_transport(&mut self) -> Result<(Transport, &'static str), String> {
        let want_usb = match self.args.mode {
            Mode::Usb | Mode::Adb => true,
            Mode::Lan => false,
            Mode::Auto => {
                self.sh.usb_present.load(Ordering::Relaxed) && Instant::now() >= self.usb_backoff_until
            }
        };
        if want_usb {
            match open_usb(&self.args, &self.usb_token) {
                Ok(t) => return Ok(t),
                Err(e) if self.args.mode == Mode::Auto => {
                    warn!("{e}; falling back to LAN for a while");
                    self.usb_backoff_until = Instant::now() + Duration::from_secs(30);
                }
                Err(e) => return Err(e),
            }
        }
        if self.lan.is_none() {
            let mut host_id = [0u8; 16];
            host_id.copy_from_slice(&self.kp.public_bytes()[..16]);
            self.lan = Some(open_lan(self.args.bind, host_id, self.sh.clone())?);
        }
        Ok((Transport::lan(self.lan.clone().unwrap()), "lan"))
    }
}

/// Clears link-active flags however `run_session` returns.
struct LinkGuard {
    sh: Arc<Shared>,
    lan_established: Option<Arc<AtomicBool>>,
}
impl Drop for LinkGuard {
    fn drop(&mut self) {
        self.sh.usb_link_open.store(false, Ordering::Relaxed);
        self.sh.dev_refresh.store(true, Ordering::Relaxed);
        if let Some(est) = &self.lan_established {
            est.store(false, Ordering::SeqCst);
        }
    }
}

/// Writes a received Android log to the logs folder and notes it in the log.
fn save_android_log(done: &crate::logfile::FinishedLog) {
    match crate::logfile::save_android_log(&crate::logfile::logs_dir(), &done.name, &done.data) {
        Ok(p) => info!(
            "đã nhận log Android \"{}\" ({} byte{}) -> {}",
            done.name,
            done.data.len(),
            if done.missing > 0 { format!(", thiếu {} đoạn", done.missing) } else { String::new() },
            p.display()
        ),
        Err(e) => warn!("không lưu được log Android \"{}\": {e}", done.name),
    }
}

fn waiting_message(link: &str) -> &'static str {
    match link {
        "usb" | "adb" => "Đang chờ thiết bị Android qua USB...",
        "lan" => "Đang chờ thiết bị kết nối qua mạng LAN...",
        _ => "Đang chờ thiết bị kết nối...",
    }
}

fn run_session(ctx: &mut Ctx, transport: Transport, link: &'static str) -> Result<End, String> {
    boost_thread_priority("session");
    let reliable = transport.reliable();
    let io_stats = transport.io.clone();
    let usb_stats = transport.usb_stats();
    let (lan_peer, lan_established) = match &transport.kind {
        TransportKind::Lan(h) => {
            // Forget the previous client and any stale datagrams.
            h.established.store(false, Ordering::SeqCst);
            *h.peer.lock().unwrap() = None;
            while h.rx.lock().unwrap().try_recv().is_ok() {}
            (Some(h.peer.clone()), Some(h.established.clone()))
        }
        _ => (None, None),
    };
    let session_otp = ctx.otp_for(link);
    let mut session =
        HostSession::new(transport, ctx.kp.clone(), &session_otp, rv_session::USB_HOST_ID).map_err(|e| e.to_string())?;
    session.set_codec(rv_proto::codec_flags::CODEC_H264);
    ctx.sh.usb_link_open.store(link != "lan", Ordering::Relaxed);
    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
    let _guard = LinkGuard { sh: ctx.sh.clone(), lan_established: lan_established.clone() };
    let mut last_dbg = Instant::now();
    let mut last_session_err = String::new();
    ctx.set_status(State::Waiting, link, None, waiting_message(link));

    let (mut established, mut streaming, mut connecting) = (false, false, false);
    let mut last_tick = Instant::now();
    let mut last_stats_emit = Instant::now();
    let mut last_stats: Option<(f64, u64, f64)> = None;
    let stats_epoch = Instant::now();
    let mut frame_stats = crate::frame_stats::FrameStats::new();
    let mut last_rx_peer_key = rv_session::PEER_UNKNOWN;
    let mut locked_shown = false;
    let mut logs = LogAssembler::new();
    let mut input_gate = WarnGate::new();
    // SYSTEM: SendInput only reaches the desktop of the calling thread. This thread owns no
    // windows or hooks, so SetThreadDesktop works here (no dedicated injection thread needed).
    let mut input_follow = crate::session_state::DesktopFollower::new("input", Duration::from_millis(50), false);
    let mut last_log_flush = Instant::now();
    let mut last_activity = Instant::now();
    let mut pending_video: Option<VideoOut> = None;

    loop {
        let (stop, otp_changed) = ctx.poll_commands();
        if stop {
            ctx.sh.ready.store(false, Ordering::Relaxed);
            return Ok(End::Stop);
        }
        if otp_changed && !established && link != "usb" {
            // The PSK is bound at session creation: restart with the new OTP.
            return Ok(End::Reopen);
        }
        if ctx.args.mode == Mode::Auto
            && link == "lan"
            && !established
            && ctx.sh.usb_present.load(Ordering::Relaxed)
            && Instant::now() >= ctx.usb_backoff_until
        {
            info!("USB device appeared; switching from LAN to USB");
            return Ok(End::Reopen);
        }

        let mut got_rx = false;
        let max_rx_batch = if established { 64 } else { 1 };
        for _ in 0..max_rx_batch {
            let rx_res = if established {
                session.io_mut().try_recv()
            } else {
                session.io_mut().recv(Duration::from_millis(2))
            };
            match rx_res {
                Ok(Some(dg)) => {
                    got_rx = true;
                    if let Some(p) = &lan_peer {
                        if let Some(addr) = *p.lock().unwrap() {
                            last_rx_peer_key = rv_session::peer_key(&addr);
                        }
                    }
                    if !established && !connecting {
                        connecting = true;
                        let peer = lan_peer.as_ref().and_then(|p| p.lock().unwrap().map(|a| a.to_string()));
                        ctx.set_status(State::Connecting, link, peer, "Đang bắt tay bảo mật với thiết bị...");
                    }
                    if let Err(e) = session.handle_datagram_from(last_rx_peer_key, &dg, Instant::now()) {
                        match e {
                            rv_session::SessionError::RateLimited => {
                                warn!("handshake rate limited");
                            }
                            rv_session::SessionError::Locked(ms) => {
                                warn!("peer locked out after wrong OTP ({ms} ms left)");
                                if !established {
                                    let msg = format!("Sai mã OTP nhiều lần, thiết bị bị khóa tạm thời ({} giây)", ms / 1000 + 1);
                                    let (l, p) = (link, ctx.peer.clone());
                                    ctx.set_status(State::Waiting, l, p, &msg);
                                }
                            }
                            e => {
                                last_session_err = e.to_string();
                                warn!("session: {e}");
                            }
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!("link lost: {e}");
                    ctx.sh.ready.store(false, Ordering::Relaxed);
                    return Ok(End::Reopen);
                }
            }
        }
        if last_tick.elapsed() >= Duration::from_millis(100) {
            session.tick(Instant::now());
            last_tick = Instant::now();
        }
        if last_dbg.elapsed() >= Duration::from_secs(2) {
            last_dbg = Instant::now();
            let stage = if streaming {
                "streaming"
            } else if established {
                "established (waiting for first video frame)"
            } else if connecting {
                "handshake in progress (client msg1 seen, not yet established)"
            } else if io_stats.dg_in.load(Ordering::Relaxed) == 0 {
                "idle (no datagram received from client yet)"
            } else {
                "handshake (datagrams seen, not established)"
            };
            let raw = usb_stats
                .as_ref()
                .map(|u| {
                    let loc = u.location();
                    format!(
                        " usb_ep_in=0x{:02x} usb_ep_out=0x{:02x} usb_dev={}-{} usb_first_in={:?} usb_raw_in={}B usb_raw_out={}B write_errors={} pipe_err={:?}",
                        loc.ep_in,
                        loc.ep_out,
                        loc.bus,
                        loc.address,
                        u.first_bytes().unwrap_or_default(),
                        u.raw_in_bytes.load(Ordering::Relaxed),
                        u.raw_out_bytes.load(Ordering::Relaxed),
                        u.write_errors.load(Ordering::Relaxed),
                        u.last_error().unwrap_or_default()
                    )
                })
                .unwrap_or_default();
            info!(
                "link-stats link={link} stage={stage} in={}dg/{}B out={}dg/{}B{raw} last_session_err={:?}",
                io_stats.dg_in.load(Ordering::Relaxed),
                io_stats.bytes_in.load(Ordering::Relaxed),
                io_stats.dg_out.load(Ordering::Relaxed),
                io_stats.bytes_out.load(Ordering::Relaxed),
                last_session_err,
            );
        }

        while let Some(ev) = session.poll_session_event() {
            match ev {
                HostSessionEvent::Established { client_static } => {
                    established = true;
                    connecting = false;
                    streaming = false;
                    if let Some(est) = &lan_established {
                        est.store(true, Ordering::SeqCst);
                    }
                    let peer = lan_peer
                        .as_ref()
                        .and_then(|p| p.lock().unwrap().map(|a| a.to_string()))
                        .unwrap_or_else(|| client_static[..6].iter().map(|b| format!("{b:02x}")).collect::<String>());
                    ctx.set_status(State::Connected, link, Some(peer), "Đã kết nối, đang chuẩn bị truyền hình ảnh");
                    ctx.send_stream_config(&mut session);
                    ctx.sh.idr.store(true, Ordering::Relaxed);
                    ctx.sh.cursor_reset.store(true, Ordering::Relaxed);
                    ctx.sh.ready.store(true, Ordering::Relaxed);
                }
                HostSessionEvent::Disconnected => {
                    info!("client disconnected; waiting for a new connection");
                    established = false;
                    streaming = false;
                    connecting = false;
                    if let Some(est) = &lan_established {
                        est.store(false, Ordering::SeqCst);
                    }
                    ctx.sh.ready.store(false, Ordering::Relaxed);
                    ctx.sh.paused.store(false, Ordering::Relaxed);
                    ctx.rtt_us = None;
                    ctx.decode_us = None;
                    ctx.sh.client_max_w.store(0, Ordering::Relaxed);
                    ctx.sh.client_max_h.store(0, Ordering::Relaxed);
                    ctx.sh.client_max_fps.store(0, Ordering::Relaxed);
                    ctx.set_status(State::Waiting, link, None, "Thiết bị đã ngắt kết nối, đang chờ kết nối lại...");
                    *ctx.sh.peer_name.lock().unwrap() = None;
                    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
                    if session_otp != ctx.otp_for(link) {
                        return Ok(End::Reopen);
                    }
                }
                HostSessionEvent::Input { event, .. } => {
                    if ctx.sh.locked.load(Ordering::Relaxed) {
                        // Injection into the lock screen is refused by Windows; say so once, not per event.
                        if input_gate.should_log("locked") {
                            warn!("input: bỏ qua thao tác vì màn hình đang khóa");
                        }
                    } else if let Some(inj) = ctx.injector.as_mut() {
                        input_follow.poll();
                        // The captured output's rect follows mode changes: map onto the live one.
                        if let Ok(r) = ctx.sh.cap_rect.lock() {
                            if r.width() > 0 && r.height() > 0 {
                                inj.set_display(*r);
                            }
                        }
                        match inj.inject(&event) {
                            Ok(()) => {
                                if let Some(n) = input_gate.recovered() {
                                    info!("input: hoạt động trở lại (đã ẩn {n} cảnh báo lặp lại)");
                                }
                            }
                            Err(e) => {
                                if input_gate.should_log(&e.to_string()) {
                                    warn!("input: {e}");
                                }
                            }
                        }
                    }
                }
                HostSessionEvent::Control(ControlMessage::RequestIdr) => {
                    ctx.sh.idr.store(true, Ordering::Relaxed);
                    ctx.sh.cursor_reset.store(true, Ordering::Relaxed);
                }
                HostSessionEvent::Control(ControlMessage::PauseVideo(p)) => {
                    ctx.sh.paused.store(p, Ordering::Relaxed);
                    if !p {
                        ctx.sh.idr.store(true, Ordering::Relaxed);
                        ctx.sh.cursor_reset.store(true, Ordering::Relaxed);
                    }
                }
                HostSessionEvent::Control(ControlMessage::QualitySliderUpdate { quality, width, height, fps }) => {
                    let sh = &ctx.sh;
                    sh.quality.store(quality.clamp(1, 100) as u32, Ordering::Relaxed);
                    sh.bitrate_override_bps.store(0, Ordering::Relaxed);
                    if width > 0 && height > 0 {
                        sh.width.store(width as u32, Ordering::Relaxed);
                        sh.height.store(height as u32, Ordering::Relaxed);
                    }
                    if fps > 0 {
                        sh.fps.store((fps as u32).min(240), Ordering::Relaxed);
                    }
                    sh.bump();
                    info!("client slider: quality {quality}, {width}x{height}@{fps}");
                }
                HostSessionEvent::Control(ControlMessage::Hello { codecs, max_w, max_h, max_fps, .. }) => {
                    let hevc = codecs & rv_proto::codec_mask::H265 != 0;
                    let mut changed = ctx.sh.client_hevc.swap(hevc, Ordering::SeqCst) != hevc;
                    if ctx.sh.client_max_w.swap(max_w as u32, Ordering::Relaxed) != max_w as u32 {
                        changed = true;
                    }
                    if ctx.sh.client_max_h.swap(max_h as u32, Ordering::Relaxed) != max_h as u32 {
                        changed = true;
                    }
                    if ctx.sh.client_max_fps.swap(max_fps as u32, Ordering::Relaxed) != max_fps as u32 {
                        changed = true;
                    }
                    if changed {
                        ctx.sh.bump();
                    }
                    info!("client Hello: codecs mask {codecs:#x} (hevc={hevc}), max {max_w}x{max_h}@{max_fps}");
                }
                HostSessionEvent::Control(ControlMessage::DeviceInfo { name }) => {
                    info!("client DeviceInfo: {name:?}");
                    *ctx.sh.peer_name.lock().unwrap() = Some(name.clone());
                    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
                    let (st, msg) = (ctx.state, ctx.message.clone());
                    ctx.set_status(st, link, Some(name), &msg);
                }
                HostSessionEvent::Control(ControlMessage::FeedbackStats { rtt_us, decode_us, .. }) => {
                    ctx.rtt_us = Some(rtt_us);
                    ctx.decode_us = (decode_us > 0).then_some(decode_us);
                }
                HostSessionEvent::Control(ControlMessage::LogChunk { name, seq, last, data }) => {
                    if let Some(done) = logs.push(&name, seq, last, &data, Instant::now()) {
                        save_android_log(&done);
                    }
                }
                HostSessionEvent::Control(_) => {}
            }
        }

        // Lock-screen transitions: tell the user (GUI status) what to do.
        let locked = ctx.sh.locked.load(Ordering::Relaxed);
        if locked != locked_shown && established {
            locked_shown = locked;
            let (st, l, p) = (ctx.state, link, ctx.peer.clone());
            if locked {
                ctx.set_status(st, l, p, crate::session_state::LOCKED_MESSAGE);
            } else {
                ctx.set_status(st, l, p, if streaming { "Đang truyền màn hình" } else { "Đã kết nối, đang chuẩn bị truyền hình ảnh" });
            }
        }
        if last_log_flush.elapsed() >= Duration::from_secs(1) {
            last_log_flush = Instant::now();
            for done in logs.flush_stale(Instant::now(), Duration::from_secs(3)) {
                save_android_log(&done);
            }
        }

        // Video-thread notifications.
        loop {
            match ctx.evrx.try_recv() {
                Ok(VideoEvent::Active(a)) => {
                    ctx.active = Some(a);
                    if established {
                        ctx.send_stream_config(&mut session);
                    }
                }
                Ok(VideoEvent::Bitrate(bps)) => {
                    if let Some(a) = ctx.active.as_mut() {
                        a.bitrate_bps = bps;
                        if established {
                            ctx.send_stream_config_only(&mut session);
                        }
                    }
                }
                Ok(VideoEvent::Stats { fps, kbps, encode_ms }) => {
                    ctx.last_video_kbps = kbps;
                    last_stats = Some((fps, kbps, encode_ms));
                }
                Ok(VideoEvent::Fatal(m)) => return Err(m),
                Err(_) => break,
            }
        }
        if let Some(c) = ctx.sh.codec_flag.load(Ordering::SeqCst).try_into().ok().filter(|c: &u8| *c != 0) {
            session.set_codec(c);
        }

        if !established {
            // Drop anything queued while nobody was listening.
            pending_video = None;
            while ctx.vrx.try_recv().is_ok() {}
            while ctx.arx.try_recv().is_ok() {}
            while ctx.curx.try_recv().is_ok() {}
            continue;
        }
        if ctx.sh.log_request.swap(false, Ordering::SeqCst) {
            let _ = session.send_control(ControlMessage::RequestLog);
            info!("đã gửi yêu cầu lấy log từ thiết bị Android");
        }
        let mut did_work = got_rx;
        while let Ok(u) = ctx.curx.try_recv() {
            did_work = true;
            let msg = match u {
                CursorUpdate::Shape { w, h, hot_x, hot_y, rgba } => ControlMessage::CursorShape { w, h, hot_x, hot_y, rgba },
                CursorUpdate::Pos { x, y, visible } => ControlMessage::CursorPos { x, y, visible },
            };
            let _ = session.send_control(msg);
        }
        if reliable {
            session.set_loss(0.0); // ordered, lossless pipe: FEC parity is wasted bytes
        }
        loop {
            let next = if let Some(v) = pending_video.take() {
                Ok(v)
            } else {
                ctx.vrx.try_recv()
            };
            match next {
                Ok(v) => {
                    did_work = true;
                    if let Err(e) = session.send_video_frame(v.frame_id, v.ts_us, &v.data, v.key) {
                        warn!("video send ({} B, key={}): {e}", v.data.len(), v.key);
                        ctx.sh.idr.store(true, Ordering::Relaxed);
                    } else {
                        frame_stats.record(stats_epoch.elapsed().as_secs_f64());
                        if !streaming {
                            streaming = true;
                            let peer = ctx.peer.clone();
                            let msg = if ctx.sh.locked.load(Ordering::Relaxed) {
                                crate::session_state::LOCKED_MESSAGE
                            } else {
                                "Đang truyền màn hình"
                            };
                            ctx.set_status(State::Streaming, link, peer, msg);
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err("video thread stopped".into()),
            }
        }
        while let Ok(au) = ctx.arx.try_recv() {
            did_work = true;
            let _ = session.send_audio(au.seq, au.ts_us, FRAME_MS, CHANNELS_RATE, au.payload);
        }

        if streaming && last_stats_emit.elapsed() >= Duration::from_secs(1) {
            last_stats_emit = Instant::now();
            if let (Some((fps, kbps, encode_ms)), Some(a)) = (last_stats, ctx.active) {
                if control::json_mode() {
                    control::emit(&control::stats_json(&Stats {
                        fps,
                        bitrate_kbps: kbps,
                        encode_ms,
                        rtt_ms: ctx.rtt_us.map(|r| r as f64 / 1000.0),
                        decode_ms: ctx.decode_us.map(|u| u as f64 / 1000.0),
                        encoder: a.kind.name().to_string(),
                        width: a.width,
                        height: a.height,
                        frame: frame_stats.summary(stats_epoch.elapsed().as_secs_f64()),
                    }));
                }
            }
        }

        // Wait for the next encoded video frame (wakes immediately on send):
        // 1 ms when active for minimal input latency, backing off to 4 ms when the desktop
        // has been static for > 50 ms so idle CPU wakeups drop 4x.
        if did_work {
            last_activity = Instant::now();
        } else {
            let wait = if last_activity.elapsed() < Duration::from_millis(50) {
                Duration::from_millis(1)
            } else {
                Duration::from_millis(4)
            };
            match ctx.vrx.recv_timeout(wait) {
                Ok(v) => pending_video = Some(v),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("video thread stopped".into()),
            }
        }
    }
}

fn run(argv: Vec<String>) -> Result<(), String> {
    let a = match parse_args(argv) {
        Ok(a) => a,
        Err(m) => {
            if !m.is_empty() {
                eprintln!("error: {m}");
            }
            eprintln!("{USAGE}");
            return Err(String::new());
        }
    };
    control::set_json_mode(a.json);
    let _timer = HighResTimerGuard::new();
    let dpi_line = crate::session_state::set_dpi_aware();
    if crate::session_state::running_as_system() {
        info!("service: chạy dưới LocalSystem — theo dõi được desktop khóa/Winlogon (ghi hình + điều khiển khi khóa máy)");
    }
    if !a.json && !a.list {
        // Under the GUI the GUI owns the log file (it receives these lines as JSON events).
        if let Some(p) = crate::logfile::init("remoteview-host") {
            info!("log file: {}", p.display());
        }
    }
    let otp = a.otp.clone().unwrap_or_else(control::random_otp);
    let (cmdtx, cmdrx) = mpsc::channel::<Command>();
    let sh = Arc::new(Shared {
        ready: AtomicBool::new(false),
        idr: AtomicBool::new(true),
        paused: AtomicBool::new(false),
        cursor_reset: AtomicBool::new(true),
        log_request: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        usb_present: AtomicBool::new(false),
        quality: AtomicU32::new(a.quality as u32),
        fps: AtomicU32::new(a.fps),
        width: AtomicU32::new(a.width),
        height: AtomicU32::new(a.height),
        bitrate_override_bps: AtomicU32::new(a.bitrate_mbps.map_or(0, |m| m * 1_000_000)),
        cfg_gen: AtomicU32::new(0),
        codec_flag: AtomicU32::new(0),
        client_hevc: AtomicBool::new(true),
        client_max_w: AtomicU32::new(0),
        client_max_h: AtomicU32::new(0),
        client_max_fps: AtomicU32::new(0),
        usb_link_open: AtomicBool::new(false),
        peer_name: Mutex::new(None),
        dev_refresh: AtomicBool::new(false),
        locked: AtomicBool::new(false),
        cap_rect: Mutex::new(crate::Rect::default()),
    });
    let link0 = match a.mode {
        Mode::Usb => "usb",
        Mode::Adb => "adb",
        Mode::Lan => "lan",
        Mode::Auto => "none",
    };
    if a.json {
        control::emit(&control::status_json(State::Starting, link0, None, &otp, "Đang khởi động host..."));
    }

    for line in crate::sysinfo::startup_report("rv-host") {
        info!("{line}");
    }
    if !dpi_line.is_empty() {
        info!("{dpi_line}");
    }
    let outputs = list_outputs().map_err(|e| e.to_string())?;
    if a.list {
        for o in &outputs {
            println!("adapter {} output {}: {}", o.adapter_index, o.output_index, o.describe());
        }
        let c = aoa::list_candidates(a.scan_all).unwrap_or_default();
        println!("usb android candidates: {c:?}; adb device: {}", adb::device_present());
        return Ok(());
    }

    // Pick the display to capture.
    // A RemoteView-owned SudoVDA monitor is used when asked for (extend / virtual) or when the
    // laptop has no active display at all (broken panel, no TeamViewer/spacedesk monitor):
    // mirroring "nothing" would only ever give a black picture.
    let any_attached = outputs.iter().any(|o| o.attached);
    let want_vdd = !a.mirror || a.virtual_primary || !any_attached;
    let mut _vdd_guard: Option<crate::sudovda::SudoVdaMonitor> = None;
    let mut vdd_output = None;
    if want_vdd {
        match create_vdd(a.display_mode, a.virtual_primary || !any_attached) {
            Ok((guard, out)) => {
                _vdd_guard = Some(guard);
                vdd_output = Some(out);
            }
            Err(e) => {
                warn!("vdisplay: {e}");
                if a.json {
                    control::emit(&control::log_json(control::Level::Warn, &format!("Màn hình ảo: {e}")));
                }
            }
        }
    }
    let outputs = if vdd_output.is_some() { list_outputs().map_err(|e| e.to_string())? } else { outputs };
    let monitor = if let Some(m) = vdd_output {
        m
    } else if a.mirror {
        // The chosen output, else the laptop's own panel (never "first enumerated": virtual
        // drivers such as spacedesk can sort first and have a different aspect ratio).
        if let Some(m) = a.monitor.as_deref() {
            if !outputs.iter().any(|o| o.attached && o.name.eq_ignore_ascii_case(m)) {
                warn!("monitor {m} không còn kết nối; chọn tự động");
            }
        }
        pick_output(&outputs, a.monitor.as_deref()).cloned().ok_or("no attached output")?
    } else {
        ExistingOutput { index: a.output }.create(a.display_mode).map_err(|e| e.to_string())?.output
    };
    info!("capturing {} {:?} on '{}'", monitor.name, monitor.desktop_rect, monitor.adapter_name);
    info!("capture target: {}", monitor.describe());
    if let Ok(mut g) = sh.cap_rect.lock() {
        *g = monitor.desktop_rect;
    }

    // Probe capture + input early so we fail before waiting on a cable.
    // A failing probe is not fatal: the video thread re-enumerates, backs off and falls back to GDI.
    let (w, h) = match DesktopDuplicator::open(Some(&monitor.name)) {
        Ok(p) => p.size(),
        Err(e) => {
            warn!("capture: DXGI probe failed ({e}); the video thread will retry and fall back to GDI");
            (monitor.desktop_rect.width().max(0) as u32, monitor.desktop_rect.height().max(0) as u32)
        }
    };
    let injector = match WinInjector::new(monitor.desktop_rect) {
        Ok(i) => Some(i),
        Err(e) => {
            warn!("input: disabled ({e})");
            None
        }
    };

    let kp = Keypair::generate().map_err(|e| e.to_string())?;
    info!("host key fingerprint: {}", kp.fingerprint());
    if !a.json {
        println!("OTP: {otp}");
    }

    let start = Instant::now();
    let (vtx, vrx) = mpsc::sync_channel::<VideoOut>(4);
    let (atx, arx) = mpsc::sync_channel::<AudioOut>(64);
    let (evtx, evrx) = mpsc::channel::<VideoEvent>();
    let (curtx, curx) = mpsc::channel::<CursorUpdate>();
    {
        let sh = sh.clone();
        let (out_name, out_rect) = (monitor.name.clone(), monitor.desktop_rect);
        let prefs = VideoPrefs { codec: a.codec, encoder: a.encoder };
        std::thread::spawn(move || video_thread(out_name, out_rect, prefs, start, sh, vtx, evtx));
    }
    {
        let sh = sh.clone();
        std::thread::spawn(move || audio_thread(start, sh, atx));
    }
    {
        let sh = sh.clone();
        std::thread::spawn(move || session_monitor_thread(sh));
    }
    {
        let sh = sh.clone();
        std::thread::spawn(move || cursor_thread(sh, curtx));
    }
    {
        let (sh, scan) = (sh.clone(), a.scan_all);
        std::thread::spawn(move || devices_thread(scan, sh));
    }
    if a.json {
        let sh = sh.clone();
        std::thread::spawn(move || stdin_thread(cmdtx, sh));
    } else {
        drop(cmdtx);
    }

    let mut ctx = Ctx {
        args: a,
        sh,
        kp,
        otp,
        usb_token: control::persistent_token(),
        lan: None,
        injector,
        vrx,
        arx,
        evrx,
        cmdrx,
        curx,
        active: None,
        rtt_us: None,
        decode_us: None,
        last_video_kbps: 0,
        state: State::Starting,
        link: link0,
        peer: None,
        message: String::new(),
        usb_backoff_until: Instant::now(),
    };
    info!("capture size {w}x{h}");

    // Reconnect loop: never exits on a client disconnect, only on stop/fatal.
    loop {
        ctx.set_status(State::Waiting, link0, None, waiting_message(link0));
        let (transport, link) = match ctx.open_transport() {
            Ok(t) => t,
            Err(e) => {
                warn!("link: {e}; retrying");
                if ctx.idle(Duration::from_millis(1500)) {
                    return Ok(());
                }
                continue;
            }
        };
        match run_session(&mut ctx, transport, link)? {
            End::Stop => return Ok(()),
            End::Reopen => {
                if ctx.idle(Duration::from_millis(300)) {
                    return Ok(());
                }
            }
        }
    }
}

/// Runs the host with CLI-style arguments (without the program name). Returns
/// the process exit code; with `--json-status` it speaks the JSON contract of
/// `CONTROL_PROTOCOL.md` on stdout/stdin.
pub fn run_cli(argv: Vec<String>) -> i32 {
    match run(argv) {
        Ok(()) => 0,
        Err(m) => {
            if !m.is_empty() {
                error!("{m}");
                if control::json_mode() {
                    control::emit(&control::status_json(State::Error, "none", None, "", &m));
                }
            }
            1
        }
    }
}

/// Creates RemoteView's own SudoVDA monitor and returns it with its DXGI output.
#[cfg(windows)]
fn create_vdd(mode: DisplayMode, primary: bool) -> std::result::Result<(crate::sudovda::SudoVdaMonitor, crate::capture::OutputInfo), String> {
    let m = crate::sudovda::SudoVdaMonitor::create(mode)?;
    if primary {
        if let Err(e) = crate::sudovda::make_primary(&m.gdi_name) {
            warn!("vdisplay: không đặt được làm màn hình chính: {e}");
        }
    }
    // DXGI sees the new output a moment after the display config does.
    for _ in 0..25 {
        if let Ok(outs) = list_outputs() {
            if let Some(o) = outs.into_iter().find(|o| o.attached && o.name.eq_ignore_ascii_case(&m.gdi_name)) {
                info!("vdisplay: RemoteView virtual display {} {:?}", o.name, o.desktop_rect);
                return Ok((m, o));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!("màn hình ảo {} không xuất hiện trong DXGI", m.gdi_name))
}

#[cfg(not(windows))]
fn create_vdd(_mode: DisplayMode, _primary: bool) -> std::result::Result<(crate::sudovda::SudoVdaMonitor, crate::capture::OutputInfo), String> {
    Err("virtual display needs Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_the_capture_aspect_and_never_enlarges() {
        // no limit / limit larger than the capture: the capture size as is
        assert_eq!(fit_size(2880, 1800, 0, 0), (2880, 1800));
        assert_eq!(fit_size(1920, 1080, 2800, 1752), (1920, 1080));
        // 16:10 panel into a 16:9 request: scaled by the tighter side, not stretched
        assert_eq!(fit_size(2880, 1800, 1920, 1080), (1728, 1080));
        // tablet 2800x1752 vs panel 2880x1800: 16:10 stays ~16:10 (snaps cleanly to 2800x1752)
        let (w, h) = fit_size(2880, 1800, 2800, 1752);
        assert!(w <= 2800 && h <= 1752);
        assert!(((w as f64 / h as f64) - 1.6).abs() < 0.002);
        // 2800x1752 (~1.598) into 2560x1600 (1.600) snaps to 2560x1600 instead of odd 2556x1600
        assert_eq!(fit_size(2800, 1752, 2560, 1600), (2560, 1600));
        // only one side limited
        assert_eq!(fit_size(2000, 1000, 1000, 5000), (1000, 500));
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        assert_eq!(truncate_utf8("DESKTOP-PC", 64), "DESKTOP-PC");
        // 'á' is 2 bytes in UTF-8 (bytes 1..3); truncating at 2 must not split it.
        assert_eq!(truncate_utf8("Máy tính", 2), "M");
        assert_eq!(truncate_utf8("Máy tính", 4), "Máy");
        assert_eq!(truncate_utf8("abcdef", 3), "abc");
    }

    #[test]
    fn lan_ignores_discovery_magic_and_guards_established_peer() {
        let sh = Arc::new(Shared {
            ready: AtomicBool::new(false),
            idr: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cursor_reset: AtomicBool::new(false),
            log_request: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            usb_present: AtomicBool::new(false),
            quality: AtomicU32::new(80),
            fps: AtomicU32::new(60),
            width: AtomicU32::new(0),
            height: AtomicU32::new(0),
            bitrate_override_bps: AtomicU32::new(0),
            cfg_gen: AtomicU32::new(0),
            codec_flag: AtomicU32::new(0),
            client_hevc: AtomicBool::new(true),
            client_max_w: AtomicU32::new(0),
            client_max_h: AtomicU32::new(0),
            client_max_fps: AtomicU32::new(0),
            usb_link_open: AtomicBool::new(false),
            peer_name: Mutex::new(None),
            dev_refresh: AtomicBool::new(false),
            locked: AtomicBool::new(false),
            cap_rect: Mutex::new(crate::Rect::default()),
        });
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let bind_addr = probe.local_addr().unwrap();
        drop(probe);

        let lan = open_lan(bind_addr, [7u8; 16], sh.clone()).unwrap();
        let client_a = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let client_b = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr_a = client_a.local_addr().unwrap();

        // 1. Discovery beacon (RVD1...) must be ignored and must not set SharedPeer.
        let beacon = rv_transport::discovery::Announce {
            host_id: [1u8; 16],
            name: "other".into(),
            port: 47800,
        }
        .encode()
        .unwrap();
        client_b.send_to(&beacon, bind_addr).unwrap();
        assert!(lan.rx.lock().unwrap().recv_timeout(Duration::from_millis(80)).is_err());
        assert_eq!(*lan.peer.lock().unwrap(), None);

        // 2. Normal packet from client A sets SharedPeer and is delivered.
        client_a.send_to(b"hello-a", bind_addr).unwrap();
        let pkt = lan.rx.lock().unwrap().recv_timeout(Duration::from_millis(200)).unwrap();
        assert_eq!(pkt, b"hello-a");
        assert_eq!(*lan.peer.lock().unwrap(), Some(addr_a));

        // 3. Once established, stray packet from client B must NOT overwrite SharedPeer or be delivered.
        lan.established.store(true, Ordering::SeqCst);
        client_b.send_to(b"stray-b", bind_addr).unwrap();
        assert!(lan.rx.lock().unwrap().recv_timeout(Duration::from_millis(80)).is_err());
        assert_eq!(*lan.peer.lock().unwrap(), Some(addr_a));

        // 4. Packet from established client A still gets through.
        client_a.send_to(b"data-a", bind_addr).unwrap();
        let pkt2 = lan.rx.lock().unwrap().recv_timeout(Duration::from_millis(200)).unwrap();
        assert_eq!(pkt2, b"data-a");

        sh.stop.store(true, Ordering::SeqCst);
    }
}
