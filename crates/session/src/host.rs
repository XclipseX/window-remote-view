use std::collections::VecDeque;

use rv_crypto::{derive_psk, HandshakeKind, Keypair, Psk, Responder};
use rv_proto::{codec_flags, ControlMessage, InputEvent, Packet, VideoPacket};
use rv_transport::fec;

use crate::channel::{random_session_id, Channel};
use crate::{hs_packet, hs_parse, DatagramIo, Result, SessionError, PROLOGUE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// Handshake completed; carries the client's authenticated static key.
    Established { client_static: [u8; 32] },
    Input { seq: u16, event: InputEvent },
    Control(ControlMessage),
}

enum State {
    Idle,
    WaitMsg3(Box<Responder>),
    Established(Box<Channel>),
}

/// Host (Noise responder, video sender).
pub struct HostSession<I: DatagramIo> {
    io: I,
    keypair: Keypair,
    psk: Psk,
    session_id: u32,
    state: State,
    events: VecDeque<HostEvent>,
    loss: f32,
    codec: u8,
}

impl<I: DatagramIo> HostSession<I> {
    /// `otp` is the 6-9 digit one-time password shown to the user; `host_id`
    /// (>= 8 bytes) is the Argon2 salt and must be known to the client too.
    pub fn new(io: I, keypair: Keypair, otp: &str, host_id: &[u8]) -> Result<Self> {
        Ok(Self {
            io,
            keypair,
            psk: derive_psk(otp, host_id)?,
            session_id: random_session_id()?,
            state: State::Idle,
            events: VecDeque::new(),
            loss: 0.0,
            codec: codec_flags::CODEC_H265,
        })
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

    pub fn poll_event(&mut self) -> Option<HostEvent> {
        self.events.pop_front()
    }

    /// Process one received datagram. Errors are per-datagram and non-fatal.
    pub fn handle_datagram(&mut self, data: &[u8]) -> Result<()> {
        if data.first() == Some(&crate::HS_MAGIC) {
            return self.handle_handshake(data);
        }
        let State::Established(ch) = &mut self.state else {
            return Err(SessionError::NotEstablished);
        };
        match ch.open(data)? {
            Packet::Input(p) => self.events.push_back(HostEvent::Input { seq: p.input_seq, event: p.event }),
            Packet::Control(c) => {
                if let ControlMessage::FeedbackStats { loss_permille, .. } = &c.message {
                    self.loss = (f32::from(*loss_permille) / 1000.0).clamp(0.0, 1.0);
                }
                self.events.push_back(HostEvent::Control(c.message));
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_handshake(&mut self, data: &[u8]) -> Result<()> {
        let (step, sid, msg) = hs_parse(data)?;
        match step {
            1 => {
                if self.is_established() {
                    return Ok(()); // ignore stray/retransmitted msg1
                }
                let mut r = Responder::new(HandshakeKind::XxPsk3, &self.keypair, None, Some(&self.psk), PROLOGUE)?;
                r.read_message(msg)?;
                let m2 = r.write_message(&self.session_id.to_be_bytes())?;
                self.io.send(&hs_packet(2, self.session_id, &m2));
                self.state = State::WaitMsg3(Box::new(r));
                Ok(())
            }
            3 => {
                let State::WaitMsg3(r) = std::mem::replace(&mut self.state, State::Idle) else {
                    return Err(SessionError::Handshake("unexpected msg3"));
                };
                if sid != self.session_id {
                    return Err(SessionError::SessionId);
                }
                let mut r = *r;
                // Wrong OTP surfaces here: the PSK is mixed into msg3.
                r.read_message(msg)?;
                let session = r.finish()?;
                let client_static = session.remote_static().ok_or(SessionError::Handshake("no client key"))?;
                self.state = State::Established(Box::new(Channel::new(session, self.session_id)));
                self.events.push_back(HostEvent::Established { client_static });
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
    pub fn send_video_frame(&mut self, frame_id: u32, capture_ts_us: u32, data: &[u8], keyframe: bool) -> Result<usize> {
        let loss = self.loss;
        let codec = self.codec;
        // Prefix the real length: VideoPacket has no frame_len field, and
        // shards are zero-padded to equal size.
        let mut framed = Vec::with_capacity(data.len() + 4);
        framed.extend_from_slice(&(data.len() as u32).to_be_bytes());
        framed.extend_from_slice(data);
        let mut shards = fec::encode_frame(frame_id, &framed, loss)?;
        if shards.len() > 255 {
            // Too many shards for the u8 fields: drop parity, then give up.
            shards = fec::encode_frame(frame_id, &framed, 0.0)?;
            if shards.len() > 255 {
                return Err(SessionError::FrameTooLarge(shards.len()));
            }
        }
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
                shard_idx: s.index as u8,
                data_k: s.k as u8,
                parity_m: s.m as u8,
                payload: s.data,
            };
            let dg = self.channel()?.seal(&Packet::Video(vp))?;
            self.io.send(&dg);
            sent += 1;
        }
        Ok(sent)
    }

    pub fn send_control(&mut self, msg: ControlMessage) -> Result<()> {
        let dg = self.channel()?.control(msg)?;
        self.io.send(&dg);
        Ok(())
    }
}
