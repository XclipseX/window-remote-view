//! Ties the outer packet type to the inner packet structs.

use crate::control::ControlPacket;
use crate::error::Result;
use crate::input::InputPacket;
use crate::media::{AudioPacket, VideoPacket};
use crate::outer::PacketType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Packet {
    Video(VideoPacket),
    Audio(AudioPacket),
    Input(InputPacket),
    Control(ControlPacket),
}

impl Packet {
    pub fn packet_type(&self) -> PacketType {
        match self {
            Self::Video(_) => PacketType::Video,
            Self::Audio(_) => PacketType::Audio,
            Self::Input(_) => PacketType::Input,
            Self::Control(_) => PacketType::Control,
        }
    }

    /// Appends the inner packet (header + payload), without the outer header.
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        match self {
            Self::Video(p) => p.encode(buf),
            Self::Audio(p) => p.encode(buf),
            Self::Input(p) => p.encode(buf),
            Self::Control(p) => p.encode(buf),
        }
    }

    /// Decodes an inner packet of the given outer type from decrypted bytes.
    /// Returns the packet and bytes consumed.
    pub fn decode(packet_type: PacketType, data: &[u8]) -> Result<(Self, usize)> {
        Ok(match packet_type {
            PacketType::Video => {
                let (p, n) = VideoPacket::decode(data)?;
                (Self::Video(p), n)
            }
            PacketType::Audio => {
                let (p, n) = AudioPacket::decode(data)?;
                (Self::Audio(p), n)
            }
            PacketType::Input => {
                let (p, n) = InputPacket::decode(data)?;
                (Self::Input(p), n)
            }
            PacketType::Control => {
                let (p, n) = ControlPacket::decode(data)?;
                (Self::Control(p), n)
            }
        })
    }
}
