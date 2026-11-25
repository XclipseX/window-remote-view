//! Video (0x01) and audio (0x02) inner packets.

use crate::error::{ProtoError, Result};
use crate::wire::{atomic, check_max, put_u16, put_u32, Reader};

/// Max bytes of one video shard payload (avoids IP fragmentation).
pub const MAX_SHARD_PAYLOAD: usize = 1150;
pub const VIDEO_HEADER_LEN: usize = 16;
/// Max opus payload per audio packet.
pub const MAX_AUDIO_PAYLOAD: usize = 1200;
pub const AUDIO_HEADER_LEN: usize = 10;

/// `codec_flags` bits.
pub mod codec_flags {
    pub const CODEC_MASK: u8 = 0x0F;
    pub const CODEC_H264: u8 = 0x01;
    pub const CODEC_H265: u8 = 0x02;
    pub const CODEC_AV1: u8 = 0x03;
    pub const FLAG_KEYFRAME: u8 = 0x10;
    pub const FLAG_PARITY: u8 = 0x20;
    pub const FLAG_LAST_FRAME_SHARD: u8 = 0x40;
}

/// One video shard (data or RS-FEC parity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPacket {
    pub frame_id: u32,
    pub capture_ts_us: u32,
    pub codec_flags: u8,
    pub shard_idx: u8,
    pub data_k: u8,
    pub parity_m: u8,
    pub payload: Vec<u8>,
}

impl VideoPacket {
    fn validate(&self) -> Result<()> {
        check_max("video payload", self.payload.len(), MAX_SHARD_PAYLOAD)?;
        validate_shard(self.shard_idx, self.data_k, self.parity_m)
    }

    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        self.validate()?;
        atomic(buf, |buf| {
            put_u32(buf, self.frame_id);
            put_u32(buf, self.capture_ts_us);
            buf.extend_from_slice(&[self.codec_flags, self.shard_idx, self.data_k, self.parity_m]);
            put_u16(buf, self.payload.len() as u16);
            put_u16(buf, 0);
            buf.extend_from_slice(&self.payload);
            Ok(())
        })
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let frame_id = r.u32()?;
        let capture_ts_us = r.u32()?;
        let codec_flags = r.u8()?;
        let shard_idx = r.u8()?;
        let data_k = r.u8()?;
        let parity_m = r.u8()?;
        let len = r.u16()? as usize;
        let _reserved = r.u16()?;
        check_max("video payload", len, MAX_SHARD_PAYLOAD)?;
        validate_shard(shard_idx, data_k, parity_m)?;
        let payload = r.take(len)?.to_vec();
        Ok((
            Self {
                frame_id,
                capture_ts_us,
                codec_flags,
                shard_idx,
                data_k,
                parity_m,
                payload,
            },
            r.pos(),
        ))
    }
}

fn validate_shard(idx: u8, k: u8, m: u8) -> Result<()> {
    if k == 0 {
        return Err(ProtoError::InvalidValue("data_k must be >= 1"));
    }
    if u16::from(idx) >= u16::from(k) + u16::from(m) {
        return Err(ProtoError::InvalidValue("shard_idx >= data_k + parity_m"));
    }
    Ok(())
}

/// One audio frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPacket {
    pub seq_num: u16,
    pub capture_ts_us: u32,
    pub duration_ms: u8,
    pub channels_rate: u8,
    pub payload: Vec<u8>,
}

impl AudioPacket {
    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        check_max("audio payload", self.payload.len(), MAX_AUDIO_PAYLOAD)?;
        atomic(buf, |buf| {
            put_u16(buf, self.seq_num);
            put_u32(buf, self.capture_ts_us);
            buf.extend_from_slice(&[self.duration_ms, self.channels_rate]);
            put_u16(buf, self.payload.len() as u16);
            buf.extend_from_slice(&self.payload);
            Ok(())
        })
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let seq_num = r.u16()?;
        let capture_ts_us = r.u32()?;
        let duration_ms = r.u8()?;
        let channels_rate = r.u8()?;
        let len = r.u16()? as usize;
        check_max("audio payload", len, MAX_AUDIO_PAYLOAD)?;
        let payload = r.take(len)?.to_vec();
        Ok((
            Self {
                seq_num,
                capture_ts_us,
                duration_ms,
                channels_rate,
                payload,
            },
            r.pos(),
        ))
    }
}
