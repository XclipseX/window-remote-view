use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rv_crypto::{derive_psk, HandshakeKind, Initiator, Keypair, Psk};
use rv_proto::{codec_flags, AudioPacket, ControlMessage, ControlPacket, InputEvent, Packet, VideoPacket};
use rv_transport::fec::{FrameReassembler, Shard};

use crate::channel::Channel;
use crate::{
    hs_packet, hs_parse, DatagramIo, Result, SessionError, IDLE_TIMEOUT, KEEPALIVE_INTERVAL,
    PING_INTERVAL, PROLOGUE,
};

/// Ignore RTT samples larger than 10 seconds as outliers.
const MAX_RTT_SAMPLE_US: u64 = 10_000_000;
/// EWMA smoothing factor for RTT samples (first sample is taken exact).
const RTT_EWMA_ALPHA: f64 = 0.25;
/// Sliding window of recent frame outcomes used for `loss_permille()`.
const LOSS_WINDOW: usize = 100;

/// A fully reassembled video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_id: u32,
    pub capture_ts_us: u32,
    pub keyframe: bool,
    pub codec: u8,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Established { host_static: [u8; 32] },
    Frame(Frame),
    Audio(AudioPacket),
    Control(ControlMessage),
    /// No authenticated packet for [`crate::IDLE_TIMEOUT`] (detected in `tick`),
    /// or `start()` was called again. Call `start()` to reconnect.
    Disconnected,
}

enum State {
    Idle,
    WaitMsg2(Box<Initiator>),
    Established(Box<Channel>),
}

/// Client (Noise initiator, video receiver).
pub struct ClientSession<I: DatagramIo> {
    io: I,
    keypair: Keypair,
    psk: Psk,
    state: State,
    events: VecDeque<ClientEvent>,
    reasm: FrameReassembler,
    meta: HashMap<u32, (u32, u8)>,
    epoch: Instant,
    last_rx: Instant,
    last_tx: Instant,
    last_ping: Instant,
    rtt_ewma_us: Option<f64>,
    last_evicted: u64,
    frame_outcomes: VecDeque<bool>,
}

impl<I: DatagramIo> ClientSession<I> {
    pub fn new(io: I, keypair: Keypair, otp: &str, host_id: &[u8]) -> Result<Self> {
        let now = Instant::now();
        Ok(Self {
            io,
            keypair,
            psk: derive_psk(otp, host_id)?,
            state: State::Idle,
            events: VecDeque::new(),
            reasm: FrameReassembler::new(Duration::from_millis(250)),
            meta: HashMap::new(),
            epoch: now,
            last_rx: now,
            last_tx: now,
            last_ping: now,
            rtt_ewma_us: None,
            last_evicted: 0,
            frame_outcomes: VecDeque::with_capacity(LOSS_WINDOW),
        })
    }

    pub fn is_established(&self) -> bool {
        matches!(self.state, State::Established(_))
    }

    pub fn io_mut(&mut self) -> &mut I {
        &mut self.io
    }

    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// Frames dropped because shards never completed in time.
    pub fn evicted_frames(&self) -> u64 {
        self.reasm.evicted_frames()
    }

    /// Smoothed RTT estimate in microseconds from Ping/Pong probes, or `None`
    /// until the first valid `Pong` is received.
    pub fn rtt_us(&self) -> Option<u32> {
        self.rtt_ewma_us.map(|v| v.round() as u32)
    }

    /// Observed frame loss rate in permille (`0..=1000`) over recent frames.
    pub fn loss_permille(&self) -> u16 {
        let total = self.frame_outcomes.len();
        if total == 0 {
            return 0;
        }
        let lost = self.frame_outcomes.iter().filter(|&&ok| !ok).count();
        ((lost * 1000) / total).min(1000) as u16
    }

    /// Send handshake message 1 (call again to retransmit).
    pub fn start(&mut self) -> Result<()> {
        if self.is_established() {
            self.drop_session();
        }
        let mut i = Initiator::new(
            HandshakeKind::XxPsk3,
            &self.keypair,
            None,
            Some(&self.psk),
            PROLOGUE,
        )?;
        let m1 = i.write_message(&[])?;
        self.io.send(&hs_packet(1, 0, &m1));
        self.state = State::WaitMsg2(Box::new(i));
        Ok(())
    }

    fn drop_session(&mut self) {
        self.state = State::Idle;
        self.reasm = FrameReassembler::new(Duration::from_millis(250));
        self.meta.clear();
        self.rtt_ewma_us = None;
        self.last_evicted = 0;
        self.frame_outcomes.clear();
        self.events.push_back(ClientEvent::Disconnected);
    }

    fn record_frame_outcome(&mut self, completed: bool) {
        if self.frame_outcomes.len() == LOSS_WINDOW {
            self.frame_outcomes.pop_front();
        }
        self.frame_outcomes.push_back(completed);
    }

    fn sync_evicted(&mut self) {
        let evicted = self.reasm.evicted_frames();
        let delta = evicted.saturating_sub(self.last_evicted);
        self.last_evicted = evicted;
        for _ in 0..delta.min(LOSS_WINDOW as u64) {
            self.record_frame_outcome(false);
        }
    }

    fn on_pong(&mut self, ts_us: u64, now: Instant) {
        let now_us = now.saturating_duration_since(self.epoch).as_micros() as u64;
        let sample = now_us.saturating_sub(ts_us);
        if sample > MAX_RTT_SAMPLE_US {
            return;
        }
        let sample_f = sample as f64;
        self.rtt_ewma_us = Some(match self.rtt_ewma_us {
            None => sample_f,
            Some(prev) => prev * (1.0 - RTT_EWMA_ALPHA) + sample_f * RTT_EWMA_ALPHA,
        });
    }

    /// Drop stale partially received frames, send periodic RTT pings and
    /// keepalives, and detect a dead host ([`ClientEvent::Disconnected`] after
    /// [`IDLE_TIMEOUT`] of silence); call every ~100 ms.
    pub fn tick(&mut self, now: Instant) {
        if self.is_established() {
            if now.saturating_duration_since(self.last_rx) >= IDLE_TIMEOUT {
                self.drop_session();
                return;
            }
            if now.saturating_duration_since(self.last_ping) >= PING_INTERVAL {
                let ts_us = now.saturating_duration_since(self.epoch).as_micros() as u64;
                if self
                    .send_control_at(ControlMessage::Ping { ts_us }, now)
                    .is_ok()
                {
                    self.last_ping = now;
                }
            } else if now.saturating_duration_since(self.last_tx) >= KEEPALIVE_INTERVAL {
                let _ = self.send_control_at(ControlMessage::Keepalive, now);
            }
        }
        self.reasm.evict_expired(now);
        self.sync_evicted();
        if self.reasm.pending_frames() == 0 {
            self.meta.clear();
        }
    }

    pub fn handle_datagram(&mut self, data: &[u8]) -> Result<()> {
        self.handle_datagram_at(data, Instant::now())
    }

    pub fn handle_datagram_at(&mut self, data: &[u8], now: Instant) -> Result<()> {
        if data.first() == Some(&crate::HS_MAGIC) {
            return self.handle_handshake(data, now);
        }
        let State::Established(ch) = &mut self.state else {
            return Err(SessionError::NotEstablished);
        };
        let pkt = ch.open(data)?;
        self.last_rx = now;
        match pkt {
            Packet::Video(vp) => self.on_video(vp, now),
            Packet::Control(ControlPacket {
                message: ControlMessage::Keepalive,
                ..
            }) => {}
            Packet::Control(ControlPacket {
                message: ControlMessage::Pong { ts_us },
                ..
            }) => self.on_pong(ts_us, now),
            Packet::Control(c) => self.events.push_back(ClientEvent::Control(c.message)),
            Packet::Audio(a) => self.events.push_back(ClientEvent::Audio(a)),
            _ => {}
        }
        Ok(())
    }

    fn on_video(&mut self, vp: VideoPacket, now: Instant) {
        let (k, m) = (vp.data_k, vp.parity_m);
        let (id, ts, flags) = (vp.frame_id, vp.capture_ts_us, vp.codec_flags);
        self.meta.entry(id).or_insert((ts, flags));
        // frame_len = padded size; the real length is the 4-byte prefix.
        let shard = Shard {
            frame_id: id,
            k,
            m,
            block: vp.block_idx,
            blocks: vp.block_count,
            index: vp.shard_idx,
            // Unknown here: the real length is the 4-byte prefix in the data.
            frame_len: u32::MAX,
            data: vp.payload,
        };
        let frame = self.reasm.push(shard, now);
        self.sync_evicted();
        if let Some(buf) = frame {
            let (ts, flags) = self.meta.remove(&id).unwrap_or((ts, flags));
            if self.meta.len() > 512 {
                self.meta.clear();
            }
            if buf.len() < 4 {
                return;
            }
            let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
            if len > buf.len() - 4 {
                return;
            }
            self.record_frame_outcome(true);
            self.events.push_back(ClientEvent::Frame(Frame {
                frame_id: id,
                capture_ts_us: ts,
                keyframe: flags & codec_flags::FLAG_KEYFRAME != 0,
                codec: flags & codec_flags::CODEC_MASK,
                data: buf[4..4 + len].to_vec(),
            }));
        }
    }

    fn handle_handshake(&mut self, data: &[u8], now: Instant) -> Result<()> {
        let (step, sid, msg) = hs_parse(data)?;
        if step != 2 {
            return Err(SessionError::Handshake("unexpected step"));
        }
        // A stray msg2 (e.g. answer to someone else's msg1) must not kill a live session.
        if !matches!(self.state, State::WaitMsg2(_)) {
            return Err(SessionError::Handshake("unexpected msg2"));
        }
        let State::WaitMsg2(i) = std::mem::replace(&mut self.state, State::Idle) else {
            unreachable!()
        };
        let mut i = *i;
        let payload = i.read_message(msg)?;
        if payload.as_slice() != sid.to_be_bytes() {
            return Err(SessionError::SessionId);
        }
        let m3 = i.write_message(&[])?;
        self.io.send(&hs_packet(3, sid, &m3));
        let session = i.finish()?;
        let host_static = session
            .remote_static()
            .ok_or(SessionError::Handshake("no host key"))?;
        self.state = State::Established(Box::new(Channel::new(session, sid)));
        self.last_rx = now;
        self.last_tx = now;
        self.last_ping = now;
        self.rtt_ewma_us = None;
        self.last_evicted = 0;
        self.frame_outcomes.clear();
        self.events
            .push_back(ClientEvent::Established { host_static });
        Ok(())
    }

    fn channel(&mut self) -> Result<&mut Channel> {
        match &mut self.state {
            State::Established(c) => Ok(c),
            _ => Err(SessionError::NotEstablished),
        }
    }

    pub fn send_input(&mut self, event: InputEvent) -> Result<()> {
        let dg = self.channel()?.input(event)?;
        self.io.send(&dg);
        self.last_tx = Instant::now();
        Ok(())
    }

    pub fn send_control(&mut self, msg: ControlMessage) -> Result<()> {
        self.send_control_at(msg, Instant::now())
    }

    fn send_control_at(&mut self, msg: ControlMessage, now: Instant) -> Result<()> {
        let dg = self.channel()?.control(msg)?;
        self.io.send(&dg);
        self.last_tx = now;
        Ok(())
    }

    pub fn request_idr(&mut self) -> Result<()> {
        self.send_control(ControlMessage::RequestIdr)
    }
}
