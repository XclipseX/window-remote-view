use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use rv_crypto::{derive_psk, HandshakeKind, Initiator, Keypair, Psk};
use rv_proto::{codec_flags, ControlMessage, InputEvent, Packet, VideoPacket};
use rv_transport::fec::{FrameReassembler, Shard};

use crate::channel::Channel;
use crate::{hs_packet, hs_parse, DatagramIo, Result, SessionError, PROLOGUE};

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
    Control(ControlMessage),
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
}

impl<I: DatagramIo> ClientSession<I> {
    pub fn new(io: I, keypair: Keypair, otp: &str, host_id: &[u8]) -> Result<Self> {
        Ok(Self {
            io,
            keypair,
            psk: derive_psk(otp, host_id)?,
            state: State::Idle,
            events: VecDeque::new(),
            reasm: FrameReassembler::new(Duration::from_millis(250)),
            meta: HashMap::new(),
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

    /// Send handshake message 1 (call again to retransmit).
    pub fn start(&mut self) -> Result<()> {
        let mut i = Initiator::new(HandshakeKind::XxPsk3, &self.keypair, None, Some(&self.psk), PROLOGUE)?;
        let m1 = i.write_message(&[])?;
        self.io.send(&hs_packet(1, 0, &m1));
        self.state = State::WaitMsg2(Box::new(i));
        Ok(())
    }

    /// Drop stale partially received frames; call periodically.
    pub fn tick(&mut self, now: Instant) {
        self.reasm.evict_expired(now);
        if self.reasm.pending_frames() == 0 {
            self.meta.clear();
        }
    }

    pub fn handle_datagram(&mut self, data: &[u8]) -> Result<()> {
        self.handle_datagram_at(data, Instant::now())
    }

    pub fn handle_datagram_at(&mut self, data: &[u8], now: Instant) -> Result<()> {
        if data.first() == Some(&crate::HS_MAGIC) {
            return self.handle_handshake(data);
        }
        let State::Established(ch) = &mut self.state else {
            return Err(SessionError::NotEstablished);
        };
        match ch.open(data)? {
            Packet::Video(vp) => self.on_video(vp, now),
            Packet::Control(c) => self.events.push_back(ClientEvent::Control(c.message)),
            _ => {}
        }
        Ok(())
    }

    fn on_video(&mut self, vp: VideoPacket, now: Instant) {
        let k = u16::from(vp.data_k);
        let m = u16::from(vp.parity_m);
        let (id, ts, flags) = (vp.frame_id, vp.capture_ts_us, vp.codec_flags);
        self.meta.entry(id).or_insert((ts, flags));
        // frame_len = padded size; the real length is the 4-byte prefix.
        let shard = Shard {
            frame_id: id,
            k,
            m,
            index: u16::from(vp.shard_idx),
            frame_len: u32::from(k) * vp.payload.len() as u32,
            data: vp.payload,
        };
        if let Some(buf) = self.reasm.push(shard, now) {
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
            self.events.push_back(ClientEvent::Frame(Frame {
                frame_id: id,
                capture_ts_us: ts,
                keyframe: flags & codec_flags::FLAG_KEYFRAME != 0,
                codec: flags & codec_flags::CODEC_MASK,
                data: buf[4..4 + len].to_vec(),
            }));
        }
    }

    fn handle_handshake(&mut self, data: &[u8]) -> Result<()> {
        let (step, sid, msg) = hs_parse(data)?;
        if step != 2 {
            return Err(SessionError::Handshake("unexpected step"));
        }
        let State::WaitMsg2(i) = std::mem::replace(&mut self.state, State::Idle) else {
            return Err(SessionError::Handshake("unexpected msg2"));
        };
        let mut i = *i;
        let payload = i.read_message(msg)?;
        if payload.as_slice() != sid.to_be_bytes() {
            return Err(SessionError::SessionId);
        }
        let m3 = i.write_message(&[])?;
        self.io.send(&hs_packet(3, sid, &m3));
        let session = i.finish()?;
        let host_static = session.remote_static().ok_or(SessionError::Handshake("no host key"))?;
        self.state = State::Established(Box::new(Channel::new(session, sid)));
        self.events.push_back(ClientEvent::Established { host_static });
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
        Ok(())
    }

    pub fn send_control(&mut self, msg: ControlMessage) -> Result<()> {
        let dg = self.channel()?.control(msg)?;
        self.io.send(&dg);
        Ok(())
    }

    pub fn request_idr(&mut self) -> Result<()> {
        self.send_control(ControlMessage::RequestIdr)
    }
}
