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
    cap_software, create_encoder, is_blocked, on_hw_failure, CodecPref, EncoderConfig, EncoderKind, EncoderPref, HwFailAction,
    HwSupervisor, VideoEncoder,
};
use crate::input::{InputInjector, WinInjector};
use crate::vdisplay::{DisplayMode, ExistingOutput, IddCxDisplay, VirtualDisplay};
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
  --monitor NAME  display to share, e.g. \\.\DISPLAY1 (default: the laptop's own panel)
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

fn parse_args(argv: Vec<String>) -> Result<Args, String> {
    let mut a = Args {
        mode: Mode::Auto,
        mirror: false,
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
                "extend" => a.mirror = false,
                "mirror" => a.mirror = true,
                o => return Err(format!("--display: extend|mirror, got {o}")),
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

impl Transport {
    fn stream(l: UsbDatagramLink) -> Self {
        Self { kind: TransportKind::Stream(l), io: Arc::default() }
    }
    fn lan(h: LanHandle) -> Self {
        Self { kind: TransportKind::Lan(h), io: Arc::default() }
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

fn open_lan(bind: SocketAddr) -> Result<LanHandle, String> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async move {
            match UdpIo::bind(bind, None) {
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                }
                Ok((io, sock)) => {
                    let peer = io.peer_handle();
                    let _ = ready_tx.send(Ok(io));
                    let mut buf = vec![0u8; 2048];
                    while let Ok((n, from)) = sock.recv_from(&mut buf).await {
                        *peer.lock().unwrap() = Some(from);
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    });
    let io: UdpIo = ready_rx.recv().map_err(|e| e.to_string())??;
    info!("LAN: listening on {bind}");
    let peer = io.peer_handle();
    Ok(LanHandle { io: Arc::new(Mutex::new(io)), rx: Arc::new(Mutex::new(rx)), peer })
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
    Stats { fps: f64, kbps: u64, encode_ms: f64 },
    Fatal(String),
}

struct VideoPrefs {
    codec: CodecPref,
    encoder: EncoderPref,
}

/// A hardware encoder was declared broken: retry it once without D3D, else
/// switch to OpenH264 for the rest of the process.
fn hw_gave_up(kind: EncoderKind, uses_d3d: bool, reason: &str) {
    warn!("video: hardware encoder {} failed ({reason})", kind.name());
    match on_hw_failure(uses_d3d) {
        HwFailAction::RetryWithoutD3d => {
            info!("video: retrying the hardware encoder in system-memory mode (no D3D11 manager)")
        }
        HwFailAction::UseSoftware => {
            warn!("video: falling back to OpenH264 (software H.264) for the rest of the session")
        }
    }
}

/// Encode size for a capture of `nw x nh` when the client / GUI asked for at
/// most `rw x rh` (0 = no limit): the capture's own aspect ratio is kept (the
/// client letterboxes), the picture is never stretched or enlarged.
fn fit_size(nw: u32, nh: u32, rw: u32, rh: u32) -> (u32, u32) {
    if rw == 0 || rh == 0 || nw == 0 || nh == 0 || (rw >= nw && rh >= nh) {
        return (nw, nh);
    }
    let k = (f64::from(rw) / f64::from(nw)).min(f64::from(rh) / f64::from(nh));
    (((f64::from(nw) * k).round() as u32).max(2), ((f64::from(nh) * k).round() as u32).max(2))
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
    let mut cap = Capturer::new(Some(out_name), out_rect);
    let thread_start = Instant::now();
    let mut sup = HwSupervisor::new();
    let mut first_input: Option<Instant> = None;
    let mut force_sw = false;
    let mut enc: Option<Box<dyn VideoEncoder>> = None;
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
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let locked = sh.locked.load(Ordering::Relaxed);
        let acquired = if locked {
            // Lock screen / secure desktop: nothing can be captured. Keep the last frame flowing.
            std::thread::sleep(Duration::from_millis(8));
            None
        } else {
            cap.acquire(8).unwrap_or(None)
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
        if last_frame.is_none() && (locked || thread_start.elapsed() > Duration::from_millis(1500)) {
            // Nothing captured yet (locked at start, or capture failing): send black so the client does not time out.
            let (w, h) = ((r.width().max(2) as u32) & !1, (r.height().max(2) as u32) & !1);
            last_frame = Some(CapturedFrame::from_bgra(w, h, vec![0u8; (w * h * 4) as usize]));
            dirty = true;
        }
        let Some(frame) = last_frame.as_ref() else { continue };

        // Software path (forced, or every hardware encoder blocked): <=1920x1080, <=60 fps.
        let sw_mode = force_sw
            || prefs.encoder == EncoderPref::Sw
            || (is_blocked(EncoderKind::MfHevcHw) && is_blocked(EncoderKind::MfH264Hw));
        let fps = sh.fps.load(Ordering::Relaxed).max(1);
        let fps = if sw_mode { fps.min(crate::encode::SW_MAX_FPS) } else { fps };
        let min_interval = Duration::from_micros(1_000_000 / fps as u64);
        let want_idr = sh.idr.load(Ordering::Relaxed);
        let due = dirty && last_encode.elapsed() + Duration::from_micros(500) >= min_interval;
        let keepalive = last_sent.elapsed() > Duration::from_secs(1);
        if !(due || want_idr || keepalive) {
            continue;
        }

        // (Re)configure the encoder when settings or the capture size changed.
        let gen = sh.cfg_gen.load(Ordering::SeqCst);
        let (nw, nh) = (frame.width, frame.height);
        let rw = sh.width.load(Ordering::Relaxed);
        let rh = sh.height.load(Ordering::Relaxed);
        let (tw, th) = fit_size(nw, nh, rw, rh);
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
        let codec_mismatch = eff_codec == CodecPref::H264 && enc.as_ref().map_or(false, |e| e.kind() == EncoderKind::MfHevcHw);
        let geometry_changed = codec_mismatch || active.map_or(true, |a| (a.width, a.height, a.fps) != (tw, th, fps));
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
                        let _ = ev.send(VideoEvent::Active(*a));
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
        last_encode = Instant::now();
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
                        hw_gave_up(k, d3d, reason);
                        force_sw = is_blocked(EncoderKind::MfHevcHw) && is_blocked(EncoderKind::MfH264Hw);
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
                        hw_gave_up(k, e.uses_d3d(), reason);
                        force_sw = is_blocked(EncoderKind::MfHevcHw) && is_blocked(EncoderKind::MfH264Hw);
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
        if pcm.is_empty() || !sh.ready.load(Ordering::Relaxed) {
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
    active: Option<ActiveEnc>,
    rtt_us: Option<u32>,
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
                info!("resolution -> {width}x{height}");
            }
            Command::RequestIdr => sh.idr.store(true, Ordering::Relaxed),
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

    fn send_stream_config<I: DatagramIo>(&self, session: &mut HostSession<I>) {
        let Some(a) = self.active else { return };
        let _ = session.send_control(ControlMessage::DisplayConfigChange {
            w: a.width.min(u16::MAX as u32) as u16,
            h: a.height.min(u16::MAX as u32) as u16,
            fps: a.fps as u16,
        });
        let _ = session.send_control(ControlMessage::StreamConfig {
            codec: a.kind.codec_flag(),
            width: a.width.min(u16::MAX as u32) as u16,
            height: a.height.min(u16::MAX as u32) as u16,
            fps: a.fps as u16,
            bitrate_kbps: a.bitrate_bps / 1000,
        });
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
            self.lan = Some(open_lan(self.args.bind)?);
        }
        Ok((Transport::lan(self.lan.clone().unwrap()), "lan"))
    }
}

/// Clears the "USB link open" flag however `run_session` returns.
struct LinkGuard(Arc<Shared>);
impl Drop for LinkGuard {
    fn drop(&mut self) {
        self.0.usb_link_open.store(false, Ordering::Relaxed);
        self.0.dev_refresh.store(true, Ordering::Relaxed);
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
    let reliable = transport.reliable();
    let io_stats = transport.io.clone();
    let usb_stats = transport.usb_stats();
    let lan_peer = match &transport.kind {
        TransportKind::Lan(h) => {
            // Forget the previous client and any stale datagrams.
            *h.peer.lock().unwrap() = None;
            while h.rx.lock().unwrap().try_recv().is_ok() {}
            Some(h.peer.clone())
        }
        _ => None,
    };
    let session_otp = ctx.otp_for(link);
    let mut session =
        HostSession::new(transport, ctx.kp.clone(), &session_otp, rv_session::USB_HOST_ID).map_err(|e| e.to_string())?;
    session.set_codec(rv_proto::codec_flags::CODEC_H264);
    ctx.sh.usb_link_open.store(link != "lan", Ordering::Relaxed);
    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
    let _guard = LinkGuard(ctx.sh.clone());
    let mut last_dbg = Instant::now();
    let mut last_session_err = String::new();
    ctx.set_status(State::Waiting, link, None, waiting_message(link));

    let (mut established, mut streaming, mut connecting) = (false, false, false);
    let mut last_tick = Instant::now();
    let mut last_stats_emit = Instant::now();
    let mut last_stats: Option<(f64, u64, f64)> = None;
    let mut last_rx_peer_key = rv_session::PEER_UNKNOWN;

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

        match session.io_mut().recv(Duration::from_millis(2)) {
            Ok(Some(dg)) => {
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
            Ok(None) => {}
            Err(e) => {
                warn!("link lost: {e}");
                ctx.sh.ready.store(false, Ordering::Relaxed);
                return Ok(End::Reopen);
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
                    let peer = lan_peer
                        .as_ref()
                        .and_then(|p| p.lock().unwrap().map(|a| a.to_string()))
                        .unwrap_or_else(|| client_static[..6].iter().map(|b| format!("{b:02x}")).collect::<String>());
                    ctx.set_status(State::Connected, link, Some(peer), "Đã kết nối, đang chuẩn bị truyền hình ảnh");
                    ctx.send_stream_config(&mut session);
                    ctx.sh.idr.store(true, Ordering::Relaxed);
                    ctx.sh.ready.store(true, Ordering::Relaxed);
                }
                HostSessionEvent::Disconnected => {
                    info!("client disconnected; waiting for a new connection");
                    established = false;
                    streaming = false;
                    connecting = false;
                    ctx.sh.ready.store(false, Ordering::Relaxed);
                    ctx.sh.paused.store(false, Ordering::Relaxed);
                    ctx.rtt_us = None;
                    ctx.set_status(State::Waiting, link, None, "Thiết bị đã ngắt kết nối, đang chờ kết nối lại...");
                    *ctx.sh.peer_name.lock().unwrap() = None;
                    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
                    if session_otp != ctx.otp_for(link) {
                        return Ok(End::Reopen);
                    }
                }
                HostSessionEvent::Input { event, .. } => {
                    if let Some(inj) = ctx.injector.as_mut() {
                        if let Err(e) = inj.inject(&event) {
                            warn!("input: {e}");
                        }
                    }
                }
                HostSessionEvent::Control(ControlMessage::RequestIdr) => ctx.sh.idr.store(true, Ordering::Relaxed),
                HostSessionEvent::Control(ControlMessage::PauseVideo(p)) => {
                    ctx.sh.paused.store(p, Ordering::Relaxed);
                    if !p {
                        ctx.sh.idr.store(true, Ordering::Relaxed);
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
                HostSessionEvent::Control(ControlMessage::Hello { codecs, .. }) => {
                    let hevc = codecs & rv_proto::codec_mask::H265 != 0;
                    if ctx.sh.client_hevc.swap(hevc, Ordering::SeqCst) != hevc {
                        ctx.sh.bump();
                    }
                    info!("client Hello: codecs mask {codecs:#x} (hevc={hevc})");
                }
                HostSessionEvent::Control(ControlMessage::DeviceInfo { name }) => {
                    info!("client DeviceInfo: {name:?}");
                    *ctx.sh.peer_name.lock().unwrap() = Some(name.clone());
                    ctx.sh.dev_refresh.store(true, Ordering::Relaxed);
                    let (st, msg) = (ctx.state, ctx.message.clone());
                    ctx.set_status(st, link, Some(name), &msg);
                }
                HostSessionEvent::Control(ControlMessage::FeedbackStats { rtt_us, .. }) => ctx.rtt_us = Some(rtt_us),
                HostSessionEvent::Control(_) => {}
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
            while ctx.vrx.try_recv().is_ok() {}
            while ctx.arx.try_recv().is_ok() {}
            continue;
        }
        if reliable {
            session.set_loss(0.0); // ordered, lossless pipe: FEC parity is wasted bytes
        }
        loop {
            match ctx.vrx.try_recv() {
                Ok(v) => {
                    if let Err(e) = session.send_video_frame(v.frame_id, v.ts_us, &v.data, v.key) {
                        warn!("video send ({} B, key={}): {e}", v.data.len(), v.key);
                        ctx.sh.idr.store(true, Ordering::Relaxed);
                    } else if !streaming {
                        streaming = true;
                        let peer = ctx.peer.clone();
                        ctx.set_status(State::Streaming, link, peer, "Đang truyền màn hình");
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err("video thread stopped".into()),
            }
        }
        while let Ok(au) = ctx.arx.try_recv() {
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
                        encoder: a.kind.name().to_string(),
                        width: a.width,
                        height: a.height,
                    }));
                }
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
    let otp = a.otp.clone().unwrap_or_else(control::random_otp);
    let (cmdtx, cmdrx) = mpsc::channel::<Command>();
    let sh = Arc::new(Shared {
        ready: AtomicBool::new(false),
        idr: AtomicBool::new(true),
        paused: AtomicBool::new(false),
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
        usb_link_open: AtomicBool::new(false),
        peer_name: Mutex::new(None),
        dev_refresh: AtomicBool::new(false),
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

    let outputs = list_outputs().map_err(|e| e.to_string())?;
    if a.list {
        for o in &outputs {
            println!("adapter {} output {} {} {:?} attached={}", o.adapter_index, o.output_index, o.name, o.desktop_rect, o.attached);
        }
        let c = aoa::list_candidates(a.scan_all).unwrap_or_default();
        println!("usb android candidates: {c:?}; adb device: {}", adb::device_present());
        return Ok(());
    }

    // Pick the display to capture.
    let monitor = if a.mirror {
        // Primary output (not "first enumerated": virtual drivers such as spacedesk can sort first).
        pick_output(&outputs, None).cloned().ok_or("no attached output")?
    } else {
        let mut idd = IddCxDisplay;
        match idd.create(a.display_mode) {
            Ok(m) => m.output,
            Err(e) => {
                warn!("vdisplay: {e}; falling back to an existing output");
                ExistingOutput { index: a.output }.create(a.display_mode).map_err(|e| e.to_string())?.output
            }
        }
    };
    info!("capturing {} {:?} on '{}'", monitor.name, monitor.desktop_rect, monitor.adapter_name);

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
    let (vtx, vrx) = mpsc::sync_channel::<VideoOut>(3);
    let (atx, arx) = mpsc::sync_channel::<AudioOut>(64);
    let (evtx, evrx) = mpsc::channel::<VideoEvent>();
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
        active: None,
        rtt_us: None,
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
