//! `rv-host`: Windows host for the USB second display.
//!
//! Threads: video (capture+encode), audio (loopback+Opus), main (session,
//! link I/O, input). Frames cross threads as already-encoded bytes.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rv_crypto::Keypair;
use rv_host_win::audio::{LoopbackCapture, OpusFrameEncoder, CHANNELS_RATE, FRAME_MS};
use rv_host_win::capture::{list_outputs, DesktopDuplicator};
use rv_host_win::encode::{OpenH264Encoder, VideoEncoder};
use rv_host_win::input::{InputInjector, WinInjector};
use rv_host_win::vdisplay::{DisplayMode, ExistingOutput, IddCxDisplay, VirtualDisplay};
use rv_proto::ControlMessage;
use rv_session::udp::UdpIo;
use rv_session::{DatagramIo, HostEvent, HostSession};
use rv_usb_link::{adb, aoa, UsbDatagramLink};

const USAGE: &str = "rv-host [--usb | --adb | --lan] [--display extend|mirror] [--mode WxH@Hz]
        [--output N] [--fps N] [--bitrate-mbps N] [--bind ADDR:PORT] [--otp DIGITS]
        [--scan-all] [--list]
  --usb   AOA bulk link, falls back to `adb reverse` (default when a device is attached)
  --adb   only the adb-reverse TCP fallback
  --lan   UDP on --bind (default 0.0.0.0:47800)";

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
    fps: u32,
    bitrate_mbps: u32,
    bind: SocketAddr,
    otp: Option<String>,
    scan_all: bool,
    list: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        mode: Mode::Auto,
        mirror: false,
        display_mode: DisplayMode::TAB_S10_PLUS,
        output: None,
        fps: 60,
        bitrate_mbps: 40,
        bind: "0.0.0.0:47800".parse().unwrap(),
        otp: None,
        scan_all: false,
        list: false,
    };
    let mut it = std::env::args().skip(1);
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
                a.display_mode = DisplayMode::parse(&val("--mode")?).ok_or("bad --mode, want WxH@Hz")?
            }
            "--output" => a.output = Some(val("--output")?.parse().map_err(|_| "bad --output")?),
            "--fps" => a.fps = val("--fps")?.parse().map_err(|_| "bad --fps")?,
            "--bitrate-mbps" => a.bitrate_mbps = val("--bitrate-mbps")?.parse().map_err(|_| "bad bitrate")?,
            "--bind" => a.bind = val("--bind")?.parse().map_err(|_| "bad --bind")?,
            "--otp" => a.otp = Some(val("--otp")?),
            "--scan-all" => a.scan_all = true,
            "--list" => a.list = true,
            "-h" | "--help" => return Err(String::new()),
            o => return Err(format!("unknown argument {o}")),
        }
    }
    Ok(a)
}

/// Link abstraction so one session loop serves USB, adb and LAN.
enum Transport {
    Stream(UsbDatagramLink),
    Lan { io: UdpIo, rx: Receiver<Vec<u8>> },
}

impl DatagramIo for Transport {
    fn send(&mut self, buf: &[u8]) {
        match self {
            Transport::Stream(l) => l.send(buf),
            Transport::Lan { io, .. } => io.send(buf),
        }
    }
}

impl Transport {
    fn recv(&mut self, t: Duration) -> Result<Option<Vec<u8>>, String> {
        match self {
            Transport::Stream(l) => {
                if l.is_broken() {
                    return Err("link write failed".into());
                }
                l.recv_timeout(t).map_err(|e| e.to_string())
            }
            Transport::Lan { rx, .. } => match rx.recv_timeout(t) {
                Ok(v) => Ok(Some(v)),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
                Err(_) => Err("udp receiver stopped".into()),
            },
        }
    }
    fn reliable(&self) -> bool {
        matches!(self, Transport::Stream(_))
    }
}

fn open_lan(bind: SocketAddr) -> Result<Transport, String> {
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
    let io = ready_rx.recv().map_err(|e| e.to_string())??;
    eprintln!("LAN: listening on {bind}");
    Ok(Transport::Lan { io, rx })
}

fn open_usb(a: &Args) -> Result<Transport, String> {
    if a.mode != Mode::Adb {
        let info = aoa::AccessoryInfo::default();
        match aoa::AoaDevice::connect(&info, a.scan_all, Duration::from_secs(8)) {
            Ok(dev) => {
                eprintln!("USB: AOA accessory open");
                return Ok(Transport::Stream(UsbDatagramLink::from_aoa(dev)));
            }
            Err(e) => eprintln!("USB: AOA failed: {e}"),
        }
    }
    if adb::device_present() {
        eprintln!("USB: trying adb reverse tcp:{}", rv_usb_link::ADB_PORT);
        let l = adb::connect(Duration::from_secs(30)).map_err(|e| e.to_string())?;
        return Ok(Transport::Stream(l));
    }
    Err("no USB link: AOA failed and no adb device".into())
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

fn video_thread(
    out_idx: (u32, u32),
    fps: u32,
    bitrate_bps: u32,
    start: Instant,
    ready: Arc<AtomicBool>,
    idr: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    tx: mpsc::SyncSender<VideoOut>,
) {
    let mut dup = match DesktopDuplicator::new(out_idx.0, out_idx.1) {
        Ok(d) => d,
        Err(e) => return eprintln!("video: capture init failed: {e}"),
    };
    let mut enc = OpenH264Encoder::new(fps, bitrate_bps);
    let mut frame_id = 0u32;
    let mut last_sent = Instant::now();
    let mut last_frame = None;
    loop {
        if !ready.load(Ordering::Relaxed) || paused.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }
        let acquired = match dup.acquire(8) {
            Ok(f) => f,
            Err(e) => return eprintln!("video: capture error: {e}"),
        };
        // Static screen: DXGI gives nothing, so re-send the last frame about
        // once a second as keepalive and so late joiners / IDR requests work.
        let frame = match acquired {
            Some(f) => {
                last_frame = Some(f);
                last_frame.as_ref().unwrap()
            }
            None if idr.load(Ordering::Relaxed) || last_sent.elapsed() > Duration::from_secs(1) => {
                match last_frame.as_ref() {
                    Some(f) => f,
                    None => continue,
                }
            }
            None => continue,
        };
        let force = idr.swap(false, Ordering::Relaxed) || frame_id == 0;
        match enc.encode(frame, force) {
            Ok(data) if !data.is_empty() => {
                let out = VideoOut {
                    frame_id,
                    ts_us: start.elapsed().as_micros() as u32,
                    key: force,
                    data,
                };
                frame_id = frame_id.wrapping_add(1);
                last_sent = Instant::now();
                if tx.try_send(out).is_err() {
                    idr.store(true, Ordering::Relaxed); // dropped a delta: resync
                }
            }
            Ok(_) => {}
            Err(e) => eprintln!("video: encode error: {e}"),
        }
    }
}

fn audio_thread(start: Instant, ready: Arc<AtomicBool>, tx: mpsc::SyncSender<AudioOut>) {
    let mut cap = match LoopbackCapture::new() {
        Ok(c) => c,
        Err(e) => return eprintln!("audio: disabled ({e})"),
    };
    let mut enc = match OpusFrameEncoder::new(128_000) {
        Ok(e) => e,
        Err(e) => return eprintln!("audio: opus init failed: {e}"),
    };
    let (mut seq, mut pcm) = (0u16, Vec::new());
    loop {
        std::thread::sleep(Duration::from_millis(5));
        pcm.clear();
        if let Err(e) = cap.poll(&mut pcm) {
            return eprintln!("audio: capture error: {e}");
        }
        if pcm.is_empty() || !ready.load(Ordering::Relaxed) {
            continue;
        }
        match enc.push(&pcm) {
            Ok(pkts) => {
                for p in pkts {
                    let out = AudioOut {
                        seq,
                        ts_us: start.elapsed().as_micros() as u32,
                        payload: p,
                    };
                    seq = seq.wrapping_add(1);
                    let _ = tx.try_send(out); // audio: drop on overflow, never block
                }
            }
            Err(e) => eprintln!("audio: encode error: {e}"),
        }
    }
}

fn random_otp() -> String {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).expect("rng");
    format!("{:06}", u32::from_le_bytes(b) % 1_000_000)
}

fn run() -> Result<(), String> {
    let a = match parse_args() {
        Ok(a) => a,
        Err(m) => {
            if !m.is_empty() {
                eprintln!("error: {m}");
            }
            eprintln!("{USAGE}");
            return Err(String::new());
        }
    };

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
        outputs
            .iter()
            .find(|o| o.attached)
            .cloned()
            .ok_or("no attached output")?
    } else {
        let mut idd = IddCxDisplay;
        match idd.create(a.display_mode) {
            Ok(m) => m.output,
            Err(e) => {
                eprintln!("vdisplay: {e}; falling back to an existing output");
                ExistingOutput { index: a.output }
                    .create(a.display_mode)
                    .map_err(|e| e.to_string())?
                    .output
            }
        }
    };
    eprintln!("capturing {} {:?}", monitor.name, monitor.desktop_rect);

    // Probe capture + input early so we fail before waiting on a cable.
    let probe = DesktopDuplicator::new(monitor.adapter_index, monitor.output_index).map_err(|e| e.to_string())?;
    let (w, h) = probe.size();
    drop(probe);
    let mut injector = match WinInjector::new(monitor.desktop_rect) {
        Ok(i) => Some(i),
        Err(e) => {
            eprintln!("input: disabled ({e})");
            None
        }
    };

    let transport = match a.mode {
        Mode::Lan => open_lan(a.bind)?,
        Mode::Usb | Mode::Adb => open_usb(&a)?,
        Mode::Auto => {
            let have_usb = aoa::list_candidates(a.scan_all).map(|c| !c.is_empty()).unwrap_or(false)
                || adb::device_present();
            if have_usb {
                open_usb(&a)?
            } else {
                eprintln!("no Android device attached, falling back to --lan");
                open_lan(a.bind)?
            }
        }
    };
    let reliable = transport.reliable();

    let otp = a.otp.clone().unwrap_or_else(random_otp);
    let kp = Keypair::generate().map_err(|e| e.to_string())?;
    println!("OTP: {otp}   host key fingerprint: {}", kp.fingerprint());
    // Fixed id shared with the Android client (USB mode has no pre-handshake hello).
    let mut session = HostSession::new(transport, kp, &otp, rv_session::USB_HOST_ID).map_err(|e| e.to_string())?;
    session.set_codec(rv_proto::codec_flags::CODEC_H264);

    let start = Instant::now();
    let ready = Arc::new(AtomicBool::new(false));
    let idr = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));
    let (vtx, vrx) = mpsc::sync_channel::<VideoOut>(3);
    let (atx, arx) = mpsc::sync_channel::<AudioOut>(64);
    {
        let (ready, idr, paused) = (ready.clone(), idr.clone(), paused.clone());
        let bps = a.bitrate_mbps * 1_000_000;
        let (out, fps) = ((monitor.adapter_index, monitor.output_index), a.fps);
        std::thread::spawn(move || video_thread(out, fps, bps, start, ready, idr, paused, vtx));
    }
    {
        let ready = ready.clone();
        std::thread::spawn(move || audio_thread(start, ready, atx));
    }

    eprintln!("waiting for client ({w}x{h}) ...");
    loop {
        match session.io_mut().recv(Duration::from_millis(2))? {
            Some(dg) => {
                if let Err(e) = session.handle_datagram(&dg) {
                    eprintln!("session: {e}");
                }
            }
            None => {}
        }
        while let Some(ev) = session.poll_event() {
            match ev {
                HostEvent::Established { .. } => {
                    eprintln!("client established");
                    let _ = session.send_control(ControlMessage::DisplayConfigChange {
                        w: w as u16,
                        h: h as u16,
                        fps: a.fps as u16,
                    });
                    idr.store(true, Ordering::Relaxed);
                    ready.store(true, Ordering::Relaxed);
                }
                HostEvent::Input { event, .. } => {
                    if let Some(inj) = injector.as_mut() {
                        if let Err(e) = inj.inject(&event) {
                            eprintln!("input: {e}");
                        }
                    }
                }
                HostEvent::Control(ControlMessage::RequestIdr) => idr.store(true, Ordering::Relaxed),
                HostEvent::Control(ControlMessage::PauseVideo(p)) => {
                    paused.store(p, Ordering::Relaxed);
                    if !p {
                        idr.store(true, Ordering::Relaxed);
                    }
                }
                HostEvent::Control(ControlMessage::QualitySliderUpdate { .. }) => {
                    // TODO: rebuild encoder with slider_to_bitrate(); fixed bitrate for now.
                }
                HostEvent::Control(_) => {}
            }
        }
        if !session.is_established() {
            continue;
        }
        if reliable {
            session.set_loss(0.0); // ordered, lossless pipe: FEC parity is wasted bytes
        }
        loop {
            match vrx.try_recv() {
                Ok(v) => {
                    // NOTE: rv-session's FEC path caps a frame at roughly 290 KB
                    // (255 shards); big IDRs from the software encoder at
                    // 2800x1752 can exceed it. Not fixed here (proto/session scope).
                    if let Err(e) = session.send_video_frame(v.frame_id, v.ts_us, &v.data, v.key) {
                        eprintln!("video send ({} B, key={}): {e}", v.data.len(), v.key);
                        idr.store(true, Ordering::Relaxed);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return Err("video thread stopped".into()),
            }
        }
        while let Ok(au) = arx.try_recv() {
            let _ = session.send_audio(au.seq, au.ts_us, FRAME_MS, CHANNELS_RATE, au.payload);
        }
    }
}

fn main() {
    if let Err(m) = run() {
        if !m.is_empty() {
            eprintln!("error: {m}");
        }
        std::process::exit(1);
    }
}
