//! Established encrypted channel: nonce counter per direction, seal/open.

use rv_crypto::Session;
use rv_proto::{
    ControlMessage, ControlPacket, InputEvent, InputPacket, OuterHeader, Packet, NONCE_MAX,
    OUTER_HEADER_LEN,
};

use crate::{Result, SessionError};

pub(crate) struct Channel {
    session: Session,
    session_id: u32,
    tx_nonce: u64,
    ctrl_seq: u16,
    input_seq: u16,
}

impl Channel {
    pub fn new(session: Session, session_id: u32) -> Self {
        Self { session, session_id, tx_nonce: 0, ctrl_seq: 0, input_seq: 0 }
    }


    pub fn seal(&mut self, packet: &Packet) -> Result<Vec<u8>> {
        if self.tx_nonce > NONCE_MAX {
            return Err(SessionError::NonceExhausted);
        }
        let nonce = self.tx_nonce;
        let mut out = Vec::with_capacity(OUTER_HEADER_LEN + 1300);
        OuterHeader::new(self.session_id, packet.packet_type(), nonce).encode(&mut out)?;
        let mut inner = Vec::with_capacity(1200);
        packet.encode(&mut inner)?;
        let ct = self.session.seal(nonce, &out[..OUTER_HEADER_LEN], &inner)?;
        self.tx_nonce += 1;
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&mut self, datagram: &[u8]) -> Result<Packet> {
        let (hdr, n) = OuterHeader::decode(datagram)?;
        if hdr.session_id != self.session_id {
            return Err(SessionError::SessionId);
        }
        let plain = self.session.open(hdr.nonce_counter, &datagram[..n], &datagram[n..])?;
        let (pkt, _) = Packet::decode(hdr.packet_type, &plain)?;
        Ok(pkt)
    }

    pub fn control(&mut self, message: ControlMessage) -> Result<Vec<u8>> {
        let ctrl_seq = self.ctrl_seq;
        let out = self.seal(&Packet::Control(ControlPacket { ctrl_seq, message }))?;
        self.ctrl_seq = self.ctrl_seq.wrapping_add(1);
        Ok(out)
    }

    pub fn input(&mut self, event: InputEvent) -> Result<Vec<u8>> {
        let input_seq = self.input_seq;
        let out = self.seal(&Packet::Input(InputPacket { input_seq, flags: 0, event }))?;
        self.input_seq = self.input_seq.wrapping_add(1);
        Ok(out)
    }
}

pub(crate) fn random_session_id() -> Result<u32> {
    loop {
        let mut b = [0u8; 4];
        getrandom::getrandom(&mut b).map_err(|_| SessionError::Random)?;
        let id = u32::from_be_bytes(b);
        if id != 0 {
            return Ok(id);
        }
    }
}
