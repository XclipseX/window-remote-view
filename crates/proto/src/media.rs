//! Video (0x01) and audio (0x02) inner packets.

use crate::error::{ProtoError, Result};
use crate::wire::{atomic, check_max, put_u16, put_u32, Reader};

/// Max bytes of one video shard payload (avoids IP fragmentation).
pub const MAX_SHARD_PAYLOAD: usize = 1150;
/// Protocol v2 video header: frame_id 4, capture_ts 4, codec_flags 1,
/// block_idx 1, block_count 1, reserved 1, shard_idx 2, data_k 2, parity_m 2, len 2.
pub const VIDEO_HEADER_LEN: usize = 20;
/// Max total shards (data + parity) per FEC block (reed-solomon-simd limit).
pub const MAX_BLOCK_SHARDS: u32 = 32768;
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
    /// FEC block index within the frame (0..block_count).
    pub block_idx: u8,
    /// Number of FEC blocks in the frame (>= 1).
    pub block_count: u8,
    /// Shard index within the block: 0..data_k data, then parity.
    pub shard_idx: u16,
    pub data_k: u16,
    pub parity_m: u16,
    pub payload: Vec<u8>,
}

impl VideoPacket {
    fn validate(&self) -> Result<()> {
        check_max("video payload", self.payload.len(), MAX_SHARD_PAYLOAD)?;
        validate_shard(self.block_idx, self.block_count, self.shard_idx, self.data_k, self.parity_m)
    }

    pub fn encode(&self, buf: &mut Vec<u8>) -> Result<()> {
        self.validate()?;
        atomic(buf, |buf| {
            put_u32(buf, self.frame_id);
            put_u32(buf, self.capture_ts_us);
            buf.extend_from_slice(&[self.codec_flags, self.block_idx, self.block_count, 0]);
            put_u16(buf, self.shard_idx);
            put_u16(buf, self.data_k);
            put_u16(buf, self.parity_m);
            put_u16(buf, self.payload.len() as u16);
            buf.extend_from_slice(&self.payload);
            Ok(())
        })
    }

    pub fn decode(data: &[u8]) -> Result<(Self, usize)> {
        let mut r = Reader::new(data);
        let frame_id = r.u32()?;
        let capture_ts_us = r.u32()?;
        let codec_flags = r.u8()?;
        let block_idx = r.u8()?;
        let block_count = r.u8()?;
        let _reserved = r.u8()?;
        let shard_idx = r.u16()?;
        let data_k = r.u16()?;
        let parity_m = r.u16()?;
        let len = r.u16()? as usize;
        check_max("video payload", len, MAX_SHARD_PAYLOAD)?;
        validate_shard(block_idx, block_count, shard_idx, data_k, parity_m)?;
        let payload = r.take(len)?.to_vec();
        Ok((
            Self {
                frame_id,
                capture_ts_us,
                codec_flags,
                block_idx,
                block_count,
                shard_idx,
                data_k,
                parity_m,
                payload,
            },
            r.pos(),
        ))
    }
}

fn validate_shard(block_idx: u8, block_count: u8, idx: u16, k: u16, m: u16) -> Result<()> {
    if k == 0 {
        return Err(ProtoError::InvalidValue("data_k must be >= 1"));
    }
    if block_count == 0 || block_idx >= block_count {
        return Err(ProtoError::InvalidValue("block_idx >= block_count"));
    }
    if u32::from(k) + u32::from(m) > MAX_BLOCK_SHARDS {
        return Err(ProtoError::InvalidValue("data_k + parity_m too large"));
    }
    if u32::from(idx) >= u32::from(k) + u32::from(m) {
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
