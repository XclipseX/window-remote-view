use std::collections::VecDeque;
use std::time::{Duration, Instant};

use rv_crypto::{derive_psk, AttemptLimiter, HandshakeKind, Keypair, Psk, Responder};
use rv_proto::{codec_flags, AudioPacket, ControlMessage, InputEvent, Packet, VideoPacket};
use rv_transport::fec;

use crate::channel::{random_session_id, Channel};
use crate::{
    hs_packet, hs_parse, DatagramIo, Result, SessionError, HANDSHAKE_MIN_INTERVAL, HS_STEP_RESET,
    IDLE_TIMEOUT, KEEPALIVE_INTERVAL, PEER_UNKNOWN, PROLOGUE, USB_HANDSHAKE_MIN_INTERVAL,
    USB_IDLE_TIMEOUT,
};

/// Minimum interval between host-to-client reset hints on reliable links.
const RESET_HINT_MIN_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Handshake completed; carries the client's authenticated static key.
    Established {
        client_static: [u8; 32],
    },
    Input {
        seq: u16,
        event: InputEvent,
    },
    Control(ControlMessage),
}

/// Superset of [`HostEvent`] including link lifecycle. Use
/// [`HostSession::poll_session_event`]; the legacy [`HostSession::poll_event`]
/// returns the same stream minus `Disconnected` (so existing exhaustive
/// `match`es on `HostEvent` keep compiling).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostSessionEvent {
    Established {
        client_static: [u8; 32],
    },
    Input {
        seq: u16,
        event: InputEvent,
    },
    Control(ControlMessage),
    /// The client went silent for [`crate::IDLE_TIMEOUT`] or was replaced by a
    /// new handshake. Stop streaming; a new msg1 is accepted at any time.
    /// Emitted from `tick`, so the caller must call `HostSession::tick`.
    Disconnected,
}

enum State {
    Idle,
    Established(Box<Channel>),
}

/// Handshake in flight (msg1 seen, waiting for the OTP-bound msg3). The
/// established channel, if any, keeps running until msg3 verifies, so an
/// unauthenticated msg1 can never kick a live session.
struct Pending {
    responder: Box<Responder>,
    session_id: u32,
}

/// Host (Noise responder, video sender).
pub struct HostSession<I: DatagramIo> {
    io: I,
    keypair: Keypair,
    psk: Psk,
    session_id: u32,
    reliable: bool,
    state: State,
    pending: Option<Pending>,
    events: VecDeque<HostSessionEvent>,
    loss: f32,
    codec: u8,
    epoch: Instant,
    last_msg1: Option<Instant>,
    last_reset_hint: Option<Instant>,
    limiter: AttemptLimiter<u64>,
    last_rx: Instant,
    last_tx: Instant,
}

impl<I: DatagramIo> HostSession<I> {
    /// `otp` is the 6-9 digit one-time password shown to the user; `host_id`
    /// (>= 8 bytes) is the Argon2 salt and must be known to the client too.
    pub fn new(io: I, keypair: Keypair, otp: &str, host_id: &[u8]) -> Result<Self> {
        let now = Instant::now();
        Ok(Self {
            io,
            keypair,
            psk: derive_psk(otp, host_id)?,
            session_id: random_session_id()?,
            reliable: false,
            state: State::Idle,
            pending: None,
            events: VecDeque::new(),
            loss: 0.0,
            codec: codec_flags::CODEC_H265,
            epoch: now,
            last_msg1: None,
            last_reset_hint: None,
            limiter: AttemptLimiter::default(),
            last_rx: now,
            last_tx: now,
        })
    }

    /// Mark whether this session runs over a reliable point-to-point link
    /// (e.g. USB AOA / ADB) rather than untrusted LAN UDP. Enables fast
    /// [`USB_IDLE_TIMEOUT`], [`USB_HANDSHAKE_MIN_INTERVAL`], and host-to-client
    /// [`HS_STEP_RESET`] hints when stale encrypted data arrives while idle.
    pub fn set_reliable(&mut self, reliable: bool) {
        self.reliable = reliable;
    }

    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    pub fn is_established(&self) -> bool {
        matches!(self.state, State::Established(_))
    }

    pub fn io_mut(&mut self) -> &mut I {
        &mut self.io
    }

    /// Codec id placed in video packets (`codec_flags::CODEC_*`).
    pub fn set_codec(&mut self, codec: u8) {
        self.codec = codec & codec_flags::CODEC_MASK;
    }

    /// Loss estimate (0..1) used for FEC sizing; updated from client FeedbackStats.
    pub fn set_loss(&mut self, loss: f32) {
        self.loss = loss.clamp(0.0, 1.0);
    }

    /// Legacy event stream (no lifecycle events); see [`HostSessionEvent`].
    pub fn poll_event(&mut self) -> Option<HostEvent> {
        loop {
            match self.events.pop_front()? {
                HostSessionEvent::Established { client_static } => {
                    return Some(HostEvent::Established { client_static })
                }
                HostSessionEvent::Input { seq, event } => return Some(HostEvent::Input { seq, event }),
                HostSessionEvent::Control(c) => return Some(HostEvent::Control(c)),
                HostSessionEvent::Disconnected => {} // not representable; use poll_session_event
            }
        }
    }

    /// Full event stream including [`HostSessionEvent::Disconnected`].
    pub fn poll_session_event(&mut self) -> Option<HostSessionEvent> {
        self.events.pop_front()
    }

    /// Process one received datagram from an unknown peer.
    pub fn handle_datagram(&mut self, data: &[u8]) -> Result<()> {
        self.handle_datagram_from(PEER_UNKNOWN, data, Instant::now())
    }

    /// Like [`Self::handle_datagram`] with an explicit time (tests).
    pub fn handle_datagram_at(&mut self, data: &[u8], now: Instant) -> Result<()> {
        self.handle_datagram_from(PEER_UNKNOWN, data, now)
    }

    /// Process one received datagram. `peer` (see [`crate::peer_key`]) keys the
    /// OTP-failure limiter. Errors are per-datagram and non-fatal.
    pub fn handle_datagram_from(&mut self, peer: u64, data: &[u8], now: Instant) -> Result<()> {
        if data.first() == Some(&crate::HS_MAGIC) {
            return self.handle_handshake(peer, data, now);
        }
        let State::Established(ch) = &mut self.state else {
            if self.reliable
                && self
                    .last_reset_hint
                    .is_none_or(|t| now.saturating_duration_since(t) >= RESET_HINT_MIN_INTERVAL)
            {
                self.io.send(&hs_packet(HS_STEP_RESET, 0, &[]));
                self.last_reset_hint = Some(now);
            }
            return Err(SessionError::NotEstablished);
        };
        let pkt = ch.open(data)?;
        self.last_rx = now;
        match pkt {
            Packet::Input(p) => self.events.push_back(HostSessionEvent::Input {
                seq: p.input_seq,
                event: p.event,
            }),
            Packet::Control(c) => match c.message {
                ControlMessage::Keepalive => {}
                ControlMessage::Ping { ts_us } => {
                    if let Ok(dg) = ch.control(ControlMessage::Pong { ts_us }) {
                        self.io.send(&dg);
                        self.last_tx = now;
                    }
                }
                msg => {
                    if let ControlMessage::FeedbackStats { loss_permille, .. } = &msg {
                        self.loss = (f32::from(*loss_permille) / 1000.0).clamp(0.0, 1.0);
                    }
                    self.events.push_back(HostSessionEvent::Control(msg));
                }
            },
            _ => {}
        }
        Ok(())
    }

    /// Drive timers; call every ~100 ms. Sends keepalives while established and
    /// emits [`HostEvent::Disconnected`] (returning to the waiting state, ready
    /// for a new handshake) after [`IDLE_TIMEOUT`] (or [`USB_IDLE_TIMEOUT`] on
    /// reliable links) without an authenticated packet.
    pub fn tick(&mut self, now: Instant) {
        if self.is_established() {
            let idle_timeout = if self.reliable {
                USB_IDLE_TIMEOUT
            } else {
                IDLE_TIMEOUT
            };
            if now.saturating_duration_since(self.last_rx) >= idle_timeout {
                self.state = State::Idle;
                self.events.push_back(HostSessionEvent::Disconnected);
            } else if now.saturating_duration_since(self.last_tx) >= KEEPALIVE_INTERVAL {
                let _ = self.send_control(ControlMessage::Keepalive);
            }
        }
    }

    fn ms(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.epoch).as_millis() as u64
    }

    fn handle_handshake(&mut self, peer: u64, data: &[u8], now: Instant) -> Result<()> {
        let (step, sid, msg) = hs_parse(data)?;
        match step {
            1 => {
                let min_interval = if self.reliable {
                    USB_HANDSHAKE_MIN_INTERVAL
                } else {
                    HANDSHAKE_MIN_INTERVAL
                };
                if let Some(t) = self.last_msg1 {
                    if now.saturating_duration_since(t) < min_interval {
                        return Err(SessionError::RateLimited);
                    }
                }
                if let Err(left) = self.limiter.check(&peer, self.ms(now)) {
                    return Err(SessionError::Locked(left));
                }
                self.last_msg1 = Some(now);
                let mut r = Responder::new(
                    HandshakeKind::XxPsk3,
                    &self.keypair,
                    None,
                    Some(&self.psk),
                    PROLOGUE,
                )?;
                r.read_message(msg)?;
                let session_id = random_session_id()?;
                let m2 = r.write_message(&session_id.to_be_bytes())?;
                self.io.send(&hs_packet(2, session_id, &m2));
                self.pending = Some(Pending {
                    responder: Box::new(r),
                    session_id,
                });
                Ok(())
            }
            3 => {
                let ms = self.ms(now);
                if let Err(left) = self.limiter.check(&peer, ms) {
                    return Err(SessionError::Locked(left));
                }
                match &self.pending {
                    None => return Err(SessionError::Handshake("unexpected msg3")),
                    Some(p) if p.session_id != sid => return Err(SessionError::SessionId),
                    _ => {}
                }
                let p = self.pending.take().expect("checked above");
                let mut r = *p.responder;
                // Wrong OTP surfaces here: the PSK is mixed into msg3.
                if let Err(e) = r.read_message(msg) {
                    self.limiter.record_failure(peer, ms);
                    return Err(e.into());
                }
                let session = r.finish()?;
                let client_static = session
                    .remote_static()
                    .ok_or(SessionError::Handshake("no client key"))?;
                self.limiter.record_success(&peer);
                if self.is_established() {
                    self.events.push_back(HostSessionEvent::Disconnected);
                }
                self.session_id = p.session_id;
                self.state = State::Established(Box::new(Channel::new(session, p.session_id)));
                self.last_reset_hint = None;
                self.last_rx = now;
                self.last_tx = now;
                self.events
                    .push_back(HostSessionEvent::Established { client_static });
                Ok(())
            }
            _ => Err(SessionError::Handshake("unexpected step")),
        }
    }

    fn channel(&mut self) -> Result<&mut Channel> {
        match &mut self.state {
            State::Established(c) => Ok(c),
            _ => Err(SessionError::NotEstablished),
        }
    }

    /// FEC-encode `data` (one encoded video frame) and send all shards.
    /// Returns the number of datagrams sent.
    pub fn send_video_frame(
        &mut self,
        frame_id: u32,
        capture_ts_us: u32,
        data: &[u8],
        keyframe: bool,
    ) -> Result<usize> {
        let loss = self.loss;
        let codec = self.codec;
        // Prefix the real length: VideoPacket has no frame_len field, and
        // shards are zero-padded to equal size.
        let mut framed = Vec::with_capacity(data.len() + 4);
        framed.extend_from_slice(&(data.len() as u32).to_be_bytes());
        framed.extend_from_slice(data);
        // Frames are split into FEC blocks of <= 64 data shards (see ADR-0002).
        let shards = fec::encode_frame(frame_id, &framed, loss)?;
        let last = shards.len() - 1;
        let mut sent = 0;
        for (i, s) in shards.into_iter().enumerate() {
            let mut flags = codec;
            if keyframe {
                flags |= codec_flags::FLAG_KEYFRAME;
            }
            if s.index >= s.k {
                flags |= codec_flags::FLAG_PARITY;
            }
            if i == last {
                flags |= codec_flags::FLAG_LAST_FRAME_SHARD;
            }
            let vp = VideoPacket {
                frame_id,
                capture_ts_us,
                codec_flags: flags,
                block_idx: s.block,
                block_count: s.blocks,
                shard_idx: s.index,
                data_k: s.k,
                parity_m: s.m,
                payload: s.data,
            };
            let dg = self.channel()?.seal(&Packet::Video(vp))?;
            self.io.send(&dg);
            sent += 1;
        }
        self.last_tx = Instant::now();
        Ok(sent)
    }

    /// Seal and send one audio frame (e.g. a 10 ms Opus packet, <= 1200 bytes).
    pub fn send_audio(
        &mut self,
        seq_num: u16,
        capture_ts_us: u32,
        duration_ms: u8,
        channels_rate: u8,
        payload: Vec<u8>,
    ) -> Result<()> {
        let pkt = Packet::Audio(AudioPacket {
            seq_num,
            capture_ts_us,
            duration_ms,
            channels_rate,
            payload,
        });
        let dg = self.channel()?.seal(&pkt)?;
        self.io.send(&dg);
        self.last_tx = Instant::now();
        Ok(())
    }

    pub fn send_control(&mut self, msg: ControlMessage) -> Result<()> {
        let dg = self.channel()?.control(msg)?;
        self.io.send(&dg);
        self.last_tx = Instant::now();
        Ok(())
    }
}
