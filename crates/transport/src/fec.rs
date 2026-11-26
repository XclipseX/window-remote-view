//! Adaptive Reed-Solomon FEC over frame shards.
//!
//! A frame is split into equally sized data shards (<= [`MAX_SHARD_PAYLOAD`]
//! bytes, even length as required by `reed-solomon-simd`), grouped into FEC
//! blocks of <= [`MAX_BLOCK_DATA_SHARDS`] data shards. Each block gets its own
//! `m` parity shards and any `k` of a block's `k + m` shards recover that block.
//! See docs/vault/decisions/ADR-0002-large-frames.md.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Maximum payload bytes per shard (keeps packets < 1200 bytes with header).
pub const MAX_SHARD_PAYLOAD: usize = 1150;
/// Serialized shard header length.
pub const HEADER_LEN: usize = 16;
/// Loss rate below which FEC is disabled (LAN option).
pub const LAN_LOSS_THRESHOLD: f32 = 0.001;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FecError {
    #[error("frame too large: needs {0} shards")]
    FrameTooLarge(usize),
    #[error("malformed shard packet")]
    Malformed,
    #[error("reed-solomon error: {0}")]
    Codec(String),
}

/// Number of parity shards for `k` data shards at the observed `loss` rate (0.0..=1.0).
/// Returns 0 when loss is below [`LAN_LOSS_THRESHOLD`].
pub fn parity_count(k: usize, loss: f32) -> usize {
    if k == 0 || loss.is_nan() || loss < LAN_LOSS_THRESHOLD {
        return 0;
    }
    let ratio = (loss * 2.0 + 0.05).clamp(0.05, 0.5);
    ((k as f32) * ratio).ceil() as usize
}

/// Max data shards per FEC block. Recovery is per block, so a lost shard only
/// needs parity from its own block and a block decodes as soon as it has `k`
/// shards (small RS problems keep decode latency low).
pub const MAX_BLOCK_DATA_SHARDS: usize = 64;
/// Max blocks per frame (`block`/`blocks` are u8): 255 * 64 * 1150 ~ 18.7 MB.
pub const MAX_BLOCKS: usize = 255;

/// One wire shard: header + payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    pub frame_id: u32,
    /// Block index within the frame.
    pub block: u8,
    /// Total blocks in the frame.
    pub blocks: u8,
    /// Data shard count of this block.
    pub k: u16,
    /// Parity shard count of this block.
    pub m: u16,
    /// 0..k are data shards, k..k+m parity (within the block).
    pub index: u16,
    /// Original frame length in bytes.
    pub frame_len: u32,
    pub data: Vec<u8>,
}

impl Shard {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + self.data.len());
        v.extend_from_slice(&self.frame_id.to_be_bytes());
        v.push(self.block);
        v.push(self.blocks);
        v.extend_from_slice(&self.k.to_be_bytes());
        v.extend_from_slice(&self.m.to_be_bytes());
        v.extend_from_slice(&self.index.to_be_bytes());
        v.extend_from_slice(&self.frame_len.to_be_bytes());
        v.extend_from_slice(&self.data);
        v
    }

    pub fn from_bytes(b: &[u8]) -> Result<Shard, FecError> {
        if b.len() <= HEADER_LEN {
            return Err(FecError::Malformed);
        }
        let u16at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
        let u32at = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let s = Shard {
            frame_id: u32at(0),
            block: b[4],
            blocks: b[5],
            k: u16at(6),
            m: u16at(8),
            index: u16at(10),
            frame_len: u32at(12),
            data: b[HEADER_LEN..].to_vec(),
        };
        s.validate()?;
        Ok(s)
    }

    /// Structural validity (also applied to shards built from another wire format).
    pub fn validate(&self) -> Result<(), FecError> {
        if self.k == 0
            || self.k as usize > MAX_BLOCK_DATA_SHARDS
            || self.m as usize > MAX_BLOCK_DATA_SHARDS
            || self.blocks == 0
            || self.block >= self.blocks
            || self.index >= self.k.saturating_add(self.m)
            || self.data.is_empty()
            || !self.data.len().is_multiple_of(2)
        {
            return Err(FecError::Malformed);
        }
        Ok(())
    }
}

/// Split `frame` into FEC blocks of at most [`MAX_BLOCK_DATA_SHARDS`] data
/// shards each, add parity per block according to `loss`, and return the wire
/// shards interleaved across blocks (shard 0 of every block, then shard 1, ...)
/// so a loss burst is spread over blocks instead of exhausting one block.
pub fn encode_frame(frame_id: u32, frame: &[u8], loss: f32) -> Result<Vec<Shard>, FecError> {
    let est = frame.len().div_ceil(MAX_SHARD_PAYLOAD).max(1);
    if est > MAX_BLOCKS * MAX_BLOCK_DATA_SHARDS {
        return Err(FecError::FrameTooLarge(est));
    }
    // Equal shard size across the whole frame, rounded up to even.
    let size = (frame.len().div_ceil(est).max(2) + 1) & !1;
    let mut data: Vec<Vec<u8>> = frame
        .chunks(size)
        .map(|c| {
            let mut v = c.to_vec();
            v.resize(size, 0);
            v
        })
        .collect();
    if data.is_empty() {
        data.push(vec![0; size]);
    }
    let total = data.len();
    let blocks = total.div_ceil(MAX_BLOCK_DATA_SHARDS);
    let per_block = total.div_ceil(blocks);
    let mut per: Vec<Vec<Shard>> = Vec::with_capacity(blocks);
    let mut it = data.into_iter();
    for b in 0..blocks {
        let chunk: Vec<Vec<u8>> = it.by_ref().take(per_block).collect();
        let k = chunk.len();
        let m = parity_count(k, loss);
        let parity = if m > 0 {
            reed_solomon_simd::encode(k, m, &chunk).map_err(|e| FecError::Codec(e.to_string()))?
        } else {
            Vec::new()
        };
        per.push(
            chunk
                .into_iter()
                .chain(parity)
                .enumerate()
                .map(|(i, d)| Shard {
                    frame_id,
                    block: b as u8,
                    blocks: blocks as u8,
                    k: k as u16,
                    m: m as u16,
                    index: i as u16,
                    frame_len: frame.len() as u32,
                    data: d,
                })
                .collect(),
        );
    }
    let longest = per.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = Vec::with_capacity(per.iter().map(Vec::len).sum());
    let mut iters: Vec<_> = per.into_iter().map(Vec::into_iter).collect();
    for _ in 0..longest {
        for it in iters.iter_mut() {
            if let Some(s) = it.next() {
                out.push(s);
            }
        }
    }
    Ok(out)
}

struct BlockState {
    k: u16,
    m: u16,
    shards: HashMap<u16, Vec<u8>>,
    /// Recovered concatenated data shards of this block.
    done: Option<Vec<u8>>,
}

struct Pending {
    blocks: u8,
    frame_len: u32,
    shard_len: usize,
    first_seen: Instant,
    states: HashMap<u8, BlockState>,
    blocks_done: usize,
}

/// Collects shards and yields frames once every block has `k` of `k + m` shards.
pub struct FrameReassembler {
    timeout: Duration,
    pending: HashMap<u32, Pending>,
    done: VecDeque<u32>,
    evicted: u64,
}

const DONE_MEMORY: usize = 512;

impl FrameReassembler {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout, pending: HashMap::new(), done: VecDeque::new(), evicted: 0 }
    }

    /// Frames dropped by timeout so far.
    pub fn evicted_frames(&self) -> u64 {
        self.evicted
    }

    pub fn pending_frames(&self) -> usize {
        self.pending.len()
    }

    /// Push a shard (arrival time `now`). Returns the frame when it becomes complete.
    pub fn push(&mut self, shard: Shard, now: Instant) -> Option<Vec<u8>> {
        self.evict_expired(now);
        if shard.validate().is_err() || self.done.contains(&shard.frame_id) {
            return None;
        }
        let id = shard.frame_id;
        let p = self.pending.entry(id).or_insert_with(|| Pending {
            blocks: shard.blocks,
            frame_len: shard.frame_len,
            shard_len: shard.data.len(),
            first_seen: now,
            states: HashMap::new(),
            blocks_done: 0,
        });
        if p.blocks != shard.blocks || p.frame_len != shard.frame_len || p.shard_len != shard.data.len() {
            return None; // inconsistent with first shard; ignore
        }
        let st = p.states.entry(shard.block).or_insert_with(|| BlockState {
            k: shard.k,
            m: shard.m,
            shards: HashMap::new(),
            done: None,
        });
        if st.k != shard.k || st.m != shard.m || st.done.is_some() {
            return None;
        }
        st.shards.insert(shard.index, shard.data);
        if st.shards.len() >= st.k as usize {
            st.done = Self::recover_block(st);
            if st.done.is_some() {
                st.shards = HashMap::new();
                p.blocks_done += 1;
            }
        }
        if p.blocks_done < p.blocks as usize {
            return None;
        }
        let p = self.pending.remove(&id)?;
        let mut out = Vec::new();
        for b in 0..p.blocks {
            out.extend_from_slice(p.states.get(&b)?.done.as_ref()?);
        }
        out.truncate(p.frame_len as usize);
        self.done.push_back(id);
        if self.done.len() > DONE_MEMORY {
            self.done.pop_front();
        }
        Some(out)
    }

    fn recover_block(st: &BlockState) -> Option<Vec<u8>> {
        let k = st.k as usize;
        let mut out = Vec::new();
        if (0..st.k).all(|i| st.shards.contains_key(&i)) {
            for i in 0..st.k {
                out.extend_from_slice(&st.shards[&i]);
            }
        } else {
            let originals = st.shards.iter().filter(|(i, _)| **i < st.k).map(|(i, d)| (*i as usize, d.as_slice()));
            let recovery = st
                .shards
                .iter()
                .filter(|(i, _)| **i >= st.k)
                .map(|(i, d)| ((*i - st.k) as usize, d.as_slice()));
            let restored = reed_solomon_simd::decode(k, st.m as usize, originals, recovery).ok()?;
            for i in 0..k {
                match st.shards.get(&(i as u16)) {
                    Some(d) => out.extend_from_slice(d),
                    None => out.extend_from_slice(restored.get(&i)?),
                }
            }
        }
        Some(out)
    }

    /// Drop incomplete frames older than the timeout. Returns how many were evicted.
    pub fn evict_expired(&mut self, now: Instant) -> usize {
        let timeout = self.timeout;
        let before = self.pending.len();
        self.pending.retain(|_, p| now.saturating_duration_since(p.first_seen) < timeout);
        let n = before - self.pending.len();
        self.evicted += n as u64;
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn parity_policy() {
        assert_eq!(parity_count(10, 0.0), 0);
        assert_eq!(parity_count(10, 0.0005), 0);
        assert_eq!(parity_count(10, 0.01), 1); // 0.07*10 -> 1
        assert_eq!(parity_count(100, 0.5), 50); // clamped to 0.5
        assert!(parity_count(20, 0.05) >= 3);
    }

    #[test]
    fn no_parity_roundtrip() {
        let f = frame(5000);
        let shards = encode_frame(1, &f, 0.0).unwrap();
        assert!(shards.iter().all(|s| s.m == 0 && s.data.len() <= MAX_SHARD_PAYLOAD));
        let mut r = FrameReassembler::new(Duration::from_millis(100));
        let now = Instant::now();
        let mut out = None;
        for s in shards {
            out = r.push(Shard::from_bytes(&s.to_bytes()).unwrap(), now).or(out);
        }
        assert_eq!(out.unwrap(), f);
    }

    #[test]
    fn recovers_with_dropped_shards() {
        let f = frame(40_000);
        let shards = encode_frame(7, &f, 0.1).unwrap();
        let m = shards[0].m as usize;
        assert!(m >= 2);
        // Drop m shards: first m data shards (worst case, forces decode).
        let mut r = FrameReassembler::new(Duration::from_secs(1));
        let now = Instant::now();
        let mut out = None;
        for s in shards.into_iter().skip(m) {
            out = r.push(s, now).or(out);
        }
        assert_eq!(out.unwrap(), f);
    }

    #[test]
    fn fails_with_too_many_dropped_and_times_out() {
        let f = frame(10_000);
        let shards = encode_frame(9, &f, 0.1).unwrap();
        let m = shards[0].m as usize;
        let mut r = FrameReassembler::new(Duration::from_millis(50));
        let t0 = Instant::now();
        for s in shards.into_iter().skip(m + 1) {
            assert!(r.push(s, t0).is_none());
        }
        assert_eq!(r.pending_frames(), 1);
        assert_eq!(r.evict_expired(t0 + Duration::from_millis(10)), 0);
        assert_eq!(r.evict_expired(t0 + Duration::from_millis(60)), 1);
        assert_eq!(r.pending_frames(), 0);
        assert_eq!(r.evicted_frames(), 1);
    }

    #[test]
    fn duplicates_after_completion_ignored_and_tiny_frames() {
        let f = frame(3);
        let shards = encode_frame(2, &f, 0.2).unwrap();
        let mut r = FrameReassembler::new(Duration::from_secs(1));
        let now = Instant::now();
        let mut got = 0;
        for s in shards.iter().chain(shards.iter()) {
            if let Some(out) = r.push(s.clone(), now) {
                assert_eq!(out, f);
                got += 1;
            }
        }
        assert_eq!(got, 1);
        assert_eq!(encode_frame(3, &[], 0.0).unwrap().len(), 1);
    }

    #[test]
    fn large_frame_3mb_5pct_loss() {
        let f = frame(3 * 1024 * 1024);
        let shards = encode_frame(11, &f, 0.05).unwrap();
        assert!(shards[0].blocks > 1);
        // deterministic pseudo-random 5% loss
        let mut x: u32 = 12345;
        let mut r = FrameReassembler::new(Duration::from_secs(1));
        let now = Instant::now();
        let mut out = None;
        let mut dropped = 0;
        for s in shards {
            x = x.wrapping_mul(1664525).wrapping_add(1013904223);
            if (x >> 8) % 100 < 5 {
                dropped += 1;
                continue;
            }
            out = r.push(Shard::from_bytes(&s.to_bytes()).unwrap(), now).or(out);
        }
        assert!(dropped > 100);
        assert_eq!(out.unwrap(), f);
    }

    #[test]
    fn frame_8mb_and_too_large() {
        let f = frame(8 * 1024 * 1024);
        let shards = encode_frame(1, &f, 0.0).unwrap();
        let mut r = FrameReassembler::new(Duration::from_secs(1));
        let now = Instant::now();
        let mut out = None;
        for s in shards {
            out = r.push(s, now).or(out);
        }
        assert_eq!(out.unwrap(), f);
        let huge = vec![0u8; MAX_BLOCKS * MAX_BLOCK_DATA_SHARDS * MAX_SHARD_PAYLOAD + 1];
        assert!(matches!(encode_frame(1, &huge, 0.0), Err(FecError::FrameTooLarge(_))));
    }

    #[test]
    fn malformed_rejected() {
        assert_eq!(Shard::from_bytes(&[0; 5]), Err(FecError::Malformed));
    }
}
