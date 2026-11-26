//! Worker thread owning the `ClientSession`. Everything session-related runs
//! here; other threads talk to it over a channel.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rv_crypto::Keypair;
use rv_proto::ControlMessage;
use rv_session::{ClientSession, DatagramIo, USB_HOST_ID};

use crate::glue::{deliver, state, translate_event, EventSink, HelloConfig, InputTranslator, UiInput};

pub enum Command {
    Datagram(Vec<u8>),
    Input(UiInput),
    Control(ControlMessage),
    Shutdown,
}

/// Re-send handshake msg1 if msg2 has not arrived.
const HANDSHAKE_RETRY: Duration = Duration::from_secs(2);
/// After the handshake the host immediately sends DisplayConfigChange; silence
/// means the host rejected msg3 (wrong PIN) or went away.
const POST_HANDSHAKE_SILENCE: Duration = Duration::from_secs(4);
const TICK: Duration = Duration::from_millis(100);

pub struct WorkerHandle {
    tx: Sender<Command>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    pub fn send(&self, c: Command) -> bool {
        self.tx.send(c).is_ok()
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
    let join = std::thread::Builder::new()
        .name("rv-session".into())
        .spawn(move || {
            run(client, &mut sink, &cfg, rx);
            sink.state(state::CLOSED, None);
        })
        .map_err(|e| e.to_string())?;
    Ok(WorkerHandle { tx, join: Some(join) })
}

fn run<I: DatagramIo, S: EventSink>(
    mut client: ClientSession<I>,
    sink: &mut S,
    cfg: &HelloConfig,
    rx: Receiver<Command>,
) {
    let mut translator = InputTranslator::default();
    sink.state(state::CONNECTING, None);
    if let Err(e) = client.start() {
        sink.state(state::FAILED, Some(&format!("handshake start: {e}")));
        return;
    }
    let mut last_start = Instant::now();
    let mut established_at: Option<Instant> = None;
    let mut heard_since_established = false;
    let mut failed = false;
    let mut last_tick = Instant::now();
    let mut last_stats = Instant::now();
    let mut rx_bytes: u64 = 0;

    loop {
        match rx.recv_timeout(Duration::from_millis(2)) {
            Ok(Command::Datagram(d)) => {
                rx_bytes += d.len() as u64;
                if let Err(e) = client.handle_datagram(&d) {
                    log::debug!("datagram dropped: {e}");
                } else if established_at.is_some() {
                    heard_since_established = true;
                }
            }
            Ok(Command::Input(i)) => {
                for ev in translator.translate(i) {
                    if let Err(e) = client.send_input(ev) {
                        log::debug!("input dropped: {e}");
                    }
                }
            }
            Ok(Command::Control(c)) => {
                if let Err(e) = client.send_control(c) {
                    log::debug!("control dropped: {e}");
                }
            }
            Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }

        while let Some(ev) = client.poll_event() {
            if matches!(ev, rv_session::ClientEvent::Established { .. }) {
                established_at = Some(Instant::now());
                if let Err(e) = client.send_control(cfg.hello_message()) {
                    log::warn!("hello failed: {e}");
                }
            }
            if matches!(ev, rv_session::ClientEvent::Disconnected) {
                // Host vanished/restarted: go back to handshaking (retried by the tick below).
                established_at = None;
                heard_since_established = false;
                failed = false;
                last_start = Instant::now();
                let _ = client.start();
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
                if !heard_since_established && !failed && now.duration_since(t) >= POST_HANDSHAKE_SILENCE {
                    failed = true;
                    sink.state(state::FAILED, Some("no data from host after handshake: wrong PIN?"));
                }
            }
        }
        if now.duration_since(last_stats) >= Duration::from_secs(1) {
            let secs = now.duration_since(last_stats).as_secs_f32();
            last_stats = now;
            let kbps = (rx_bytes as f32 * 8.0 / 1000.0 / secs) as i32;
            rx_bytes = 0;
            sink.stats(0.0, kbps, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::Outgoing;
    use rv_session::mem::{drain, queue, MemIo};
    use rv_session::{HostEvent, HostSession};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Rec(Arc<Mutex<Vec<Outgoing>>>);
    impl EventSink for Rec {
        fn video(&mut self, d: &[u8], p: i64, f: i32) {
            self.0.lock().unwrap().push(Outgoing::Video { data: d.to_vec(), pts_us: p, flags: f });
        }
        fn audio(&mut self, d: &[u8], p: i64, c: i32) {
            self.0.lock().unwrap().push(Outgoing::Audio { data: d.to_vec(), pts_us: p, codec: c });
        }
        fn state(&mut self, s: i32, m: Option<&str>) {
            self.0.lock().unwrap().push(Outgoing::State { state: s, message: m.map(str::to_string) });
        }
        fn stats(&mut self, _: f32, _: i32, _: f32) {}
    }

    fn cfg(otp: &str) -> HelloConfig {
        HelloConfig { otp: otp.into(), width: 1280, height: 800, refresh_hz: 60, codecs: rv_proto::codec_mask::H264 }
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
            let g = rec.0.lock().unwrap();
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
        let g = rec.0.lock().unwrap();
        assert!(g.contains(&Outgoing::Video { data: frame, pts_us: 1000, flags: 1 }));
        assert!(g.contains(&Outgoing::Audio { data: vec![9; 40], pts_us: 2000, codec: 1 }));
        assert!(g.iter().any(|o| matches!(o, Outgoing::State { state: 10, .. })));
        assert_eq!(g.last(), Some(&Outgoing::State { state: state::CLOSED, message: None }));
    }
}
