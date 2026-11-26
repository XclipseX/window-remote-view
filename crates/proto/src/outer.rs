//! Outer 12-byte frame header.

use crate::error::{ProtoError, Result};
use crate::wire::{atomic, put_u32, Reader};

/// Protocol version carried in the high nibble of `version_flags`.
pub const PROTOCOL_VERSION: u8 = 2;
/// Size of the outer header in bytes.
pub const OUTER_HEADER_LEN: usize = 12;
/// Nonce counter is 48 bits wide.
pub const NONCE_MAX: u64 = (1u64 << 48) - 1;

/// Outer packet type (byte 5 of the outer header).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PacketType {
    Video = 0x01,
    Audio = 0x02,
    Input = 0x03,
    Control = 0x04,
}

impl PacketType {
    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0x01 => Ok(Self::Video),
            0x02 => Ok(Self::Audio),
            0x03 => Ok(Self::Input),
            0x04 => Ok(Self::Control),
            other => Err(ProtoError::UnknownPacketType(other)),
        }
    }
}

/// Outer frame header. The payload that follows is encrypted by another crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OuterHeader {
    /// Low 4 bits of `version_flags` (0..=15).
    pub flags: u8,
    pub session_id: u32,
    pub packet_type: PacketType,
    /// 48-bit AEAD nonce counter.
    pub nonce_counter: u64,
}

impl OuterHeader {
    pub fn new(session_id: u32, packet_type: PacketType, nonce_counter: u64) -> Self {
        Self {
            flags: 0,
            session_id,
            packet_type,
            nonce_counter,
        }
    }

    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        if self.flags > 0x0F {
            return Err(ProtoError::InvalidValue("flags must fit in 4 bits"));
        }
        if self.nonce_counter > NONCE_MAX {
            return Err(ProtoError::InvalidValue("nonce_counter exceeds 48 bits"));
        }
        atomic(buf, |buf| {
            buf.push((PROTOCOL_VERSION << 4) | self.flags);
            put_u32(buf, self.session_id);
            buf.push(self.packet_type as u8);
            buf.extend_from_slice(&self.nonce_counter.to_be_bytes()[2..]);
            Ok(())
        })
    }

    /// Returns the header and bytes consumed (always 12).
    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let vf = r.u8()?;
        let version = vf >> 4;
        if version != PROTOCOL_VERSION {
            return Err(ProtoError::UnsupportedVersion(version));
        }
        let session_id = r.u32()?;
        let packet_type = PacketType::from_u8(r.u8()?)?;
        let nonce_counter = r.u48()?;
        Ok((
            Self {
                flags: vf & 0x0F,
                session_id,
                packet_type,
                nonce_counter,
            },
            r.pos(),
        ))
    }
}
