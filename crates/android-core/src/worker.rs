//! Worker thread owning the `ClientSession`. Everything session-related runs
//! here; other threads talk to it over a channel.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rv_crypto::Keypair;
use rv_proto::ControlMessage;
use rv_session::{ClientSession, DatagramIo, USB_HOST_ID};

use crate::glue::{deliver, log_chunks, state, translate_event, EventSink, HelloConfig, InputTranslator, UiInput};

pub enum Command {
    Datagram(Vec<u8>),
    Input(UiInput),
    Control(ControlMessage),
    /// Set quality (1..=100) and optional resolution/FPS (0 = keep current).
    SetStreamQuality { quality: u8, width: u16, height: u16, fps: u16 },
    /// Set max FPS (1..=240) while preserving the worker's current quality level.
    SetMaxFps(u16),
    /// Report latest hardware MediaCodec decode time in microseconds.
    DecodeUs(u32),
    /// Upload a log file to the host as `LogChunk`s (paced by the worker loop).
    UploadLog { name: String, data: Vec<u8> },
    Shutdown,
}

/// Default quality level matching `StreamSettings.qualityPercent`.
const DEFAULT_QUALITY: u8 = 85;
/// Maximum number of additional queued commands drained per loop pass after `recv_timeout`.
const MAX_CMD_BATCH: usize = 64;
/// Re-send handshake msg1 if msg2 has not arrived.
const HANDSHAKE_RETRY: Duration = Duration::from_secs(2);
/// Log chunks sent per worker loop pass (~2 ms): paces an upload to a few MB/s.
const LOG_CHUNKS_PER_PASS: usize = 8;
/// After the handshake the host immediately sends DisplayConfigChange; silence
/// means the host rejected msg3 (wrong PIN) or went away.
const POST_HANDSHAKE_SILENCE: Duration = Duration::from_secs(4);
const TICK: Duration = Duration::from_millis(100);
/// Upper bound on flushing queued log chunks when the session is shutting down.
const LOG_FLUSH_ON_SHUTDOWN: Duration = Duration::from_millis(2500);

pub struct WorkerHandle {
    tx: Sender<Command>,
    decode_us: Arc<AtomicU32>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn send(&self, c: Command) -> bool {
        self.tx.send(c).is_ok()
    }

    /// Clone the underlying command channel sender (used by background UDP readers).
    pub fn sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    /// Lock-free update of the latest video decode duration in microseconds,
    /// reported to the host in periodic `FeedbackStats`.
    pub fn report_decode_us(&self, decode_us: u32) {
        self.decode_us.store(decode_us, Ordering::Relaxed);
    }

    /// Stop the worker and wait for it.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start a session worker. Fails synchronously on a bad OTP / key generation.
pub fn spawn<I, S>(io: I, mut sink: S, cfg: HelloConfig) -> Result<WorkerHandle, String>
where
    I: DatagramIo + Send + 'static,
    S: EventSink + Send + 'static,
{
    let kp = Keypair::generate().map_err(|e| e.to_string())?;
    let client = ClientSession::new(io, kp, &cfg.otp, USB_HOST_ID).map_err(|e| format!("invalid PIN: {e}"))?;
    let (tx, rx) = mpsc::channel();
    let decode_us = Arc::new(AtomicU32::new(0));
    let decode_us_worker = Arc::clone(&decode_us);
    let join = std::thread::Builder::new()
        .name("rv-session".into())
        .spawn(move || {
            run(client, &mut sink, &cfg, rx, decode_us_worker);
            sink.state(state::CLOSED, None);
        })
        .map_err(|e| e.to_string())?;
    Ok(WorkerHandle { tx, decode_us, join: Some(join) })
}

struct LoopState {
    translator: InputTranslator,
    rx_bytes: u64,
    heard_since_established: bool,
    current_quality: u8,
    upload: VecDeque<ControlMessage>,
}

/// Process a single command; returns `true` when the worker should shut down.
fn handle_cmd<I: DatagramIo>(
    cmd: Command,
    client: &mut ClientSession<I>,
    st: &mut LoopState,
    decode_us: &AtomicU32,
) -> bool {
    match cmd {
        Command::Datagram(d) => {
            st.rx_bytes += d.len() as u64;
            let was_established = client.is_established();
            if let Err(e) = client.handle_datagram(&d) {
                log::debug!("datagram dropped: {e}");
            } else if was_established {
                st.heard_since_established = true;
            }
        }
        Command::Input(i) => {
            for ev in st.translator.translate(i) {
                if let Err(e) = client.send_input(ev) {
                    log::debug!("input dropped: {e}");
                }
            }
        }
        Command::Control(c) => {
            if let Err(e) = client.send_control(c) {
                log::debug!("control dropped: {e}");
            }
        }
        Command::SetStreamQuality { quality, width, height, fps } => {
            let quality = quality.clamp(1, 100);
            st.current_quality = quality;
            if let Err(e) = client.send_control(ControlMessage::QualitySliderUpdate {
                quality,
                width,
                height,
                fps,
            }) {
                log::debug!("quality update dropped: {e}");
            }
        }
        Command::SetMaxFps(fps) => {
            let fps = fps.clamp(1, 240);
            if let Err(e) = client.send_control(ControlMessage::QualitySliderUpdate {
                quality: st.current_quality,
                width: 0,
                height: 0,
                fps,
            }) {
                log::debug!("max fps update dropped: {e}");
            }
        }
        Command::DecodeUs(us) => {
            decode_us.store(us, Ordering::Relaxed);
        }
        Command::UploadLog { name, data } => st.upload.extend(log_chunks(&name, &data)),
        Command::Shutdown => return true,
    }
    false
}

fn run<I: DatagramIo, S: EventSink>(
    mut client: ClientSession<I>,
    sink: &mut S,
    cfg: &HelloConfig,
    rx: Receiver<Command>,
    decode_us: Arc<AtomicU32>,
) {
    sink.state(state::CONNECTING, None);
    if let Err(e) = client.start() {
        sink.state(state::FAILED, Some(&format!("handshake start: {e}")));
        return;
    }
    let mut last_start = Instant::now();
    let mut established_at: Option<Instant> = None;
    let mut failed = false;
    let mut last_tick = Instant::now();
    let mut last_stats = Instant::now();
    let mut st = LoopState {
        translator: InputTranslator::default(),
        rx_bytes: 0,
        heard_since_established: false,
        current_quality: DEFAULT_QUALITY,
        upload: VecDeque::new(),
    };

    loop {
        let mut shutdown = false;
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(cmd) => {
                if handle_cmd(cmd, &mut client, &mut st, &decode_us) {
                    shutdown = true;
                } else {
                    for _ in 0..MAX_CMD_BATCH {
                        match rx.try_recv() {
                            Ok(cmd) => {
                                if handle_cmd(cmd, &mut client, &mut st, &decode_us) {
                                    shutdown = true;
                                    break;
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                shutdown = true;
                                break;
                            }
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => shutdown = true,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if shutdown {
            break;
        }

        for _ in 0..LOG_CHUNKS_PER_PASS {
            let Some(c) = st.upload.pop_front() else { break };
            if let Err(e) = client.send_control(c) {
                log::warn!("log upload aborted: {e}");
                st.upload.clear();
            }
        }

        while let Some(ev) = client.poll_event() {
            match &ev {
                rv_session::ClientEvent::Established { .. } => {
                    established_at = Some(Instant::now());
                    if let Err(e) = client.send_control(cfg.hello_message()) {
                        log::warn!("hello failed: {e}");
                    }
                    if let Some(info) = cfg.device_info_message() {
                        if let Err(e) = client.send_control(info) {
                            log::warn!("device info failed: {e}");
                        }
                    }
                }
                rv_session::ClientEvent::Disconnected => {
                    // Host vanished/restarted: go back to handshaking (retried by the tick below).
                    established_at = None;
                    st.heard_since_established = false;
                    failed = false;
                    last_start = Instant::now();
                    let _ = client.start();
                }
                rv_session::ClientEvent::Frame(_)
                | rv_session::ClientEvent::Audio(_)
                | rv_session::ClientEvent::Control(_) => {
                    if established_at.is_some() {
                        st.heard_since_established = true;
                    }
                }
            }
            for o in translate_event(ev) {
                deliver(sink, &o);
            }
        }

        let now = Instant::now();
        if now.duration_since(last_tick) >= TICK {
            last_tick = now;
            client.tick(now);
            if established_at.is_none() && now.duration_since(last_start) >= HANDSHAKE_RETRY {
                last_start = now;
                let _ = client.start();
            }
            if let Some(t) = established_at {
                if !st.heard_since_established && !failed && now.duration_since(t) >= POST_HANDSHAKE_SILENCE {
                    failed = true;
                    sink.state(state::FAILED, Some("no data from host after handshake: wrong PIN?"));
                }
            }
        }
        if now.duration_since(last_stats) >= Duration::from_secs(1) {
            let secs = now.duration_since(last_stats).as_secs_f32();
            last_stats = now;
            let kbps = (st.rx_bytes as f32 * 8.0 / 1000.0 / secs) as i32;
            st.rx_bytes = 0;
            if client.is_established() {
                let rtt_us = client.rtt_us().unwrap_or(0);
                let loss_permille = client.loss_permille();
                let latest_decode_us = decode_us.load(Ordering::Relaxed);
                let recv_kbps = kbps.max(0) as u32;
                if let Err(e) = client.send_control(ControlMessage::FeedbackStats {
                    rtt_us,
                    loss_permille,
                    recv_kbps,
                    decode_us: latest_decode_us,
                }) {
                    log::debug!("feedback stats dropped: {e}");
                }
                sink.stats(rtt_us as f32 / 1000.0, kbps, loss_permille as f32 / 10.0);
            } else {
                sink.stats(0.0, kbps, 0.0);
            }
        }
    }

    // Shutdown must not cut a log upload short: flush what is still queued (bounded wait, lightly paced).
    let deadline = Instant::now() + LOG_FLUSH_ON_SHUTDOWN;
    while !st.upload.is_empty() && Instant::now() < deadline {
        for _ in 0..LOG_CHUNKS_PER_PASS {
            let Some(c) = st.upload.pop_front() else { break };
            if let Err(e) = client.send_control(c) {
                log::warn!("log flush aborted: {e}");
                st.upload.clear();
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::{kbps_to_quality, Outgoing};
    use rv_session::mem::{drain, queue, MemIo};
    use rv_session::{HostEvent, HostSession};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Rec {
        events: Arc<Mutex<Vec<Outgoing>>>,
        stats: Arc<Mutex<Vec<(f32, i32, f32)>>>,
    }
    impl EventSink for Rec {
        fn video(&mut self, d: &[u8], p: i64, f: i32) {
            self.events.lock().unwrap().push(Outgoing::Video { data: d.to_vec(), pts_us: p, flags: f });
        }
        fn audio(&mut self, d: &[u8], p: i64, c: i32) {
            self.events.lock().unwrap().push(Outgoing::Audio { data: d.to_vec(), pts_us: p, codec: c });
        }
        fn state(&mut self, s: i32, m: Option<&str>) {
            self.events.lock().unwrap().push(Outgoing::State { state: s, message: m.map(str::to_string) });
        }
        fn stats(&mut self, rtt_ms: f32, bitrate_kbps: i32, loss_percent: f32) {
            self.stats.lock().unwrap().push((rtt_ms, bitrate_kbps, loss_percent));
        }
        fn cursor_shape(&mut self, w: i32, h: i32, hot_x: i32, hot_y: i32, rgba: &[u8]) {
            self.events.lock().unwrap().push(Outgoing::CursorShape { w, h, hot_x, hot_y, rgba: rgba.to_vec() });
        }
        fn cursor_pos(&mut self, x: i32, y: i32, visible: bool) {
            self.events.lock().unwrap().push(Outgoing::CursorPos { x, y, visible });
        }
    }

    fn cfg(otp: &str) -> HelloConfig {
        HelloConfig { otp: otp.into(), width: 1280, height: 800, refresh_hz: 60, codecs: rv_proto::codec_mask::H264, device_name: String::new() }
    }

    #[test]
    fn rejects_bad_otp() {
        let rec = Rec::default();
        let io = MemIo::new(queue(), 0.0, 1);
        assert!(spawn(io, rec, cfg("12")).is_err());
    }

    #[test]
    fn end_to_end_with_host_session() {
        let (to_host, to_client) = (queue(), queue());
        let mut host = HostSession::new(
            MemIo::new(to_client.clone(), 0.0, 1),
            Keypair::generate().unwrap(),
            "654321",
            USB_HOST_ID,
        )
        .unwrap();
        let rec = Rec::default();
        let mut w = spawn(MemIo::new(to_host.clone(), 0.0, 2), rec.clone(), cfg("654321")).unwrap();

        let frame: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let mut sent_video = false;
        let mut got_hello = false;
        let mut got_touch = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        let done = |rec: &Rec| {
            let g = rec.events.lock().unwrap();
            g.iter().any(|o| matches!(o, Outgoing::Video { .. }))
                && g.iter().any(|o| matches!(o, Outgoing::Audio { .. }))
        };
        while Instant::now() < deadline && !(done(&rec) && got_hello && got_touch) {
            for d in drain(&to_host) {
                host.handle_datagram(&d).unwrap();
            }
            while let Some(ev) = host.poll_event() {
                match ev {
                    HostEvent::Established { .. } => {
                        host.send_control(ControlMessage::DisplayConfigChange { w: 1280, h: 800, fps: 60 }).unwrap();
                        host.send_video_frame(1, 1000, &frame, true).unwrap();
                        host.send_audio(0, 2000, 10, 0x21, vec![9; 40]).unwrap();
                        sent_video = true;
                        w.send(Command::Input(UiInput::Touch { id: 1, x: 100, y: 200, phase: 0, pressure: 512 }));
                    }
                    HostEvent::Control(ControlMessage::Hello { max_w, max_h, max_fps, .. }) => {
                        assert_eq!((max_w, max_h, max_fps), (1280, 800, 60));
                        got_hello = true;
                    }
                    HostEvent::Input { event, .. } => {
                        assert!(matches!(event, rv_proto::InputEvent::Touch { .. }));
                        got_touch = true;
                    }
                    _ => {}
                }
            }
            for d in drain(&to_client) {
                w.send(Command::Datagram(d));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(sent_video && got_hello && got_touch, "hello={got_hello} touch={got_touch}");
        w.shutdown();
        let g = rec.events.lock().unwrap();
        assert!(g.contains(&Outgoing::Video { data: frame, pts_us: 1000, flags: 1 }));
        assert!(g.contains(&Outgoing::Audio { data: vec![9; 40], pts_us: 2000, codec: 1 }));
        assert!(g.iter().any(|o| matches!(o, Outgoing::State { state: 10, .. })));
        assert_eq!(g.last(), Some(&Outgoing::State { state: state::CLOSED, message: None }));
    }

    #[test]
    fn cursor_down_and_log_upload_up() {
        let (to_host, to_client) = (queue(), queue());
        let mut host = HostSession::new(
            MemIo::new(to_client.clone(), 0.0, 1),
            Keypair::generate().unwrap(),
            "654321",
            USB_HOST_ID,
        )
        .unwrap();
        let rec = Rec::default();
        let mut w = spawn(MemIo::new(to_host.clone(), 0.0, 2), rec.clone(), cfg("654321")).unwrap();
        let log: Vec<u8> = (0..3000u32).map(|i| (i % 200) as u8).collect();
        let mut got: Vec<u8> = Vec::new();
        let mut got_last = false;
        let mut sent = false;
        let deadline = Instant::now() + Duration::from_secs(20);
        let seen = |rec: &Rec| {
            let g = rec.events.lock().unwrap();
            g.contains(&Outgoing::CursorShape { w: 1, h: 1, hot_x: 0, hot_y: 0, rgba: vec![1, 2, 3, 4] })
                && g.contains(&Outgoing::CursorPos { x: 65535, y: 0, visible: true })
        };
        while Instant::now() < deadline && !(seen(&rec) && got_last) {
            for d in drain(&to_host) {
                host.handle_datagram(&d).unwrap();
            }
            while let Some(ev) = host.poll_event() {
                match ev {
                    HostEvent::Established { .. } => {
                        host.send_control(ControlMessage::CursorShape { w: 1, h: 1, hot_x: 0, hot_y: 0, rgba: vec![1, 2, 3, 4] }).unwrap();
                        host.send_control(ControlMessage::CursorPos { x: 65535, y: 0, visible: true }).unwrap();
                        w.send(Command::UploadLog { name: "tab.log".into(), data: log.clone() });
                        sent = true;
                    }
                    HostEvent::Control(ControlMessage::LogChunk { name, seq, last, data }) => {
                        assert_eq!(name, "tab.log");
                        assert_eq!(seq as usize * crate::glue::LOG_CHUNK_BYTES, got.len());
                        got.extend_from_slice(&data);
                        got_last = last;
                    }
                    _ => {}
                }
            }
            for d in drain(&to_client) {
                w.send(Command::Datagram(d));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        w.shutdown();
        assert!(sent && seen(&rec), "cursor events not delivered");
        assert!(got_last && got == log, "log upload incomplete: {} of {} bytes", got.len(), log.len());
    }

    #[test]
    fn request_log_and_quality_fps_bitrate_commands() {
        let (to_host, to_client) = (queue(), queue());
        let mut host = HostSession::new(
            MemIo::new(to_client.clone(), 0.0, 1),
            Keypair::generate().unwrap(),
            "654321",
            USB_HOST_ID,
        )
        .unwrap();
        let rec = Rec::default();
        let mut w = spawn(MemIo::new(to_host.clone(), 0.0, 2), rec.clone(), cfg("654321")).unwrap();

        let mut received_updates: Vec<ControlMessage> = Vec::new();
        let mut sent_cmds = false;
        let expected_bitrate_q = kbps_to_quality(40_000);
        let deadline = Instant::now() + Duration::from_secs(15);

        let got_log_req = |rec: &Rec| {
            rec.events.lock().unwrap().contains(&Outgoing::State { state: state::LOG_REQUEST, message: None })
        };

        while Instant::now() < deadline && !(got_log_req(&rec) && received_updates.len() == 4) {
            for d in drain(&to_host) {
                host.handle_datagram(&d).unwrap();
            }
            while let Some(ev) = host.poll_event() {
                match ev {
                    HostEvent::Established { .. } => {
                        host.send_control(ControlMessage::RequestLog).unwrap();
                    }
                    HostEvent::Control(ControlMessage::Hello { .. }) if !sent_cmds => {
                        sent_cmds = true;
                        // 1. SetMaxFps before quality update -> uses default quality (85)
                        w.send(Command::SetMaxFps(60));
                        // 2. SetStreamQuality updates current quality to 50
                        w.send(Command::SetStreamQuality { quality: 50, width: 1920, height: 1080, fps: 90 });
                        // 3. Subsequent SetMaxFps preserves updated quality (50)
                        w.send(Command::SetMaxFps(120));
                        // 4. Bitrate mapping -> SetStreamQuality
                        w.send(Command::SetStreamQuality {
                            quality: expected_bitrate_q,
                            width: 0,
                            height: 0,
                            fps: 0,
                        });
                    }
                    HostEvent::Control(m @ ControlMessage::QualitySliderUpdate { .. }) => {
                        received_updates.push(m);
                    }
                    _ => {}
                }
            }
            for d in drain(&to_client) {
                w.send(Command::Datagram(d));
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        w.shutdown();
        assert!(got_log_req(&rec), "state::LOG_REQUEST (12) not emitted");
        assert_eq!(
            received_updates,
            vec![
                ControlMessage::QualitySliderUpdate { quality: 85, width: 0, height: 0, fps: 60 },
                ControlMessage::QualitySliderUpdate { quality: 50, width: 1920, height: 1080, fps: 90 },
                ControlMessage::QualitySliderUpdate { quality: 50, width: 0, height: 0, fps: 120 },
                ControlMessage::QualitySliderUpdate {
                    quality: expected_bitrate_q,
                    width: 0,
                    height: 0,
                    fps: 0,
                },
            ]
        );
    }

    #[test]
    fn feedback_stats_emitted_with_real_rtt_decode_us_and_kbps() {
        let (to_host, to_client) = (queue(), queue());
        let mut host = HostSession::new(
            MemIo::new(to_client.clone(), 0.0, 1),
            Keypair::generate().unwrap(),
            "654321",
            USB_HOST_ID,
        )
        .unwrap();
        let rec = Rec::default();
        let mut w = spawn(MemIo::new(to_host.clone(), 0.0, 2), rec.clone(), cfg("654321")).unwrap();
        w.report_decode_us(4200);

        let payload: Vec<u8> = vec![0xAB; 2500];
        let mut established = false;
        let mut frame_id: u32 = 1;
        let mut last_frame_sent = Instant::now();
        let mut got_feedback: Option<(u32, u16, u32, u32)> = None;
        let deadline = Instant::now() + Duration::from_secs(6);

        while Instant::now() < deadline && got_feedback.is_none() {
            for d in drain(&to_host) {
                host.handle_datagram(&d).unwrap();
            }
            while let Some(ev) = host.poll_event() {
                match ev {
                    HostEvent::Established { .. } => {
                        established = true;
                        host.send_control(ControlMessage::DisplayConfigChange { w: 1280, h: 800, fps: 60 }).unwrap();
                    }
                    HostEvent::Control(ControlMessage::FeedbackStats {
                        rtt_us,
                        loss_permille,
                        recv_kbps,
                        decode_us,
                    }) if rtt_us > 0 && recv_kbps > 0 && decode_us == 4200 => {
                        got_feedback = Some((rtt_us, loss_permille, recv_kbps, decode_us));
                    }
                    _ => {}
                }
            }
            if established && last_frame_sent.elapsed() >= Duration::from_millis(100) {
                last_frame_sent = Instant::now();
                host.send_video_frame(frame_id, frame_id * 16_000, &payload, frame_id == 1).unwrap();
                frame_id += 1;
            }
            let out = drain(&to_client);
            if !out.is_empty() {
                // Simulate ~6 ms link delay so Ping/Pong yields a measurable non-zero RTT.
                if established {
                    std::thread::sleep(Duration::from_millis(6));
                }
                for d in out {
                    w.send(Command::Datagram(d));
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        w.shutdown();
        let (rtt_us, loss_permille, recv_kbps, decode_us) =
            got_feedback.expect("expected FeedbackStats with non-zero rtt_us, recv_kbps, and decode_us=4200");
        assert!(rtt_us >= 5_000, "expected rtt_us >= 5000, got {rtt_us}");
        assert_eq!(loss_permille, 0);
        assert!(recv_kbps > 0);
        assert_eq!(decode_us, 4200);

        let stats = rec.stats.lock().unwrap();
        assert!(
            stats.iter().any(|&(rtt_ms, kbps, _loss)| rtt_ms >= 5.0 && kbps > 0),
            "expected sink.stats to receive real RTT and kbps, got {stats:?}"
        );
    }
}
