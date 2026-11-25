//! Adaptive Reed-Solomon FEC over frame shards.
//!
//! A frame is split into `k` equally sized data shards (<= [`MAX_SHARD_PAYLOAD`]
//! bytes, even length as required by `reed-solomon-simd`), `m` parity shards are
//! added, and any `k` of the `k + m` shards recover the frame.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Maximum payload bytes per shard (keeps packets < 1200 bytes with header).
pub const MAX_SHARD_PAYLOAD: usize = 1150;
/// Serialized shard header length.
pub const HEADER_LEN: usize = 14;
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

/// One wire shard: header + payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    pub frame_id: u32,
    /// Data shard count.
    pub k: u16,
    /// Parity shard count.
    pub m: u16,
    /// 0..k are data shards, k..k+m parity.
    pub index: u16,
    /// Original frame length in bytes.
    pub frame_len: u32,
    pub data: Vec<u8>,
}

impl Shard {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(HEADER_LEN + self.data.len());
        v.extend_from_slice(&self.frame_id.to_be_bytes());
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
            k: u16at(4),
            m: u16at(6),
            index: u16at(8),
            frame_len: u32at(10),
            data: b[HEADER_LEN..].to_vec(),
        };
        if s.k == 0 || s.index >= s.k.saturating_add(s.m) || !s.data.len().is_multiple_of(2) {
            return Err(FecError::Malformed);
        }
        Ok(s)
    }
}

/// Split `frame` into data shards and add parity according to `loss`.
pub fn encode_frame(frame_id: u32, frame: &[u8], loss: f32) -> Result<Vec<Shard>, FecError> {
    let k = frame.len().div_ceil(MAX_SHARD_PAYLOAD).max(1);
    let m = parity_count(k, loss);
    if k + m > 60_000 {
        return Err(FecError::FrameTooLarge(k + m));
    }
    // Equal shard size, rounded up to even.
    let size = (frame.len().div_ceil(k).max(2) + 1) & !1;
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
    debug_assert_eq!(data.len(), k);
    let parity = if m > 0 {
        reed_solomon_simd::encode(k, m, &data).map_err(|e| FecError::Codec(e.to_string()))?
    } else {
        Vec::new()
    };
    let mk = |index: usize, data: Vec<u8>| Shard {
        frame_id,
        k: k as u16,
        m: m as u16,
        index: index as u16,
        frame_len: frame.len() as u32,
        data,
    };
    Ok(data
        .into_iter()
        .chain(parity)
        .enumerate()
        .map(|(i, d)| mk(i, d))
        .collect())
}

struct Pending {
    k: u16,
    m: u16,
    frame_len: u32,
    shard_len: usize,
    first_seen: Instant,
    shards: HashMap<u16, Vec<u8>>,
}

/// Collects shards and yields frames once any `k` of `k + m` shards arrived.
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
        if self.done.contains(&shard.frame_id) {
            return None;
        }
        let id = shard.frame_id;
        let p = self.pending.entry(id).or_insert_with(|| Pending {
            k: shard.k,
            m: shard.m,
            frame_len: shard.frame_len,
            shard_len: shard.data.len(),
            first_seen: now,
            shards: HashMap::new(),
        });
        if p.k != shard.k
            || p.m != shard.m
            || p.frame_len != shard.frame_len
            || p.shard_len != shard.data.len()
            || shard.index >= p.k + p.m
        {
            return None; // inconsistent with first shard; ignore
        }
        p.shards.insert(shard.index, shard.data);
        if p.shards.len() < p.k as usize {
            return None;
        }
        let p = self.pending.remove(&id)?;
        let frame = Self::recover(p)?;
        self.done.push_back(id);
        if self.done.len() > DONE_MEMORY {
            self.done.pop_front();
        }
        Some(frame)
    }

    fn recover(p: Pending) -> Option<Vec<u8>> {
        let k = p.k as usize;
        let mut out = Vec::with_capacity(k * p.shard_len);
        if (0..p.k).all(|i| p.shards.contains_key(&i)) {
            for i in 0..p.k {
                out.extend_from_slice(&p.shards[&i]);
            }
        } else {
            let originals = p.shards.iter().filter(|(i, _)| **i < p.k).map(|(i, d)| (*i as usize, d.as_slice()));
            let recovery = p
                .shards
                .iter()
                .filter(|(i, _)| **i >= p.k)
                .map(|(i, d)| ((*i - p.k) as usize, d.as_slice()));
            let restored = reed_solomon_simd::decode(k, p.m as usize, originals, recovery).ok()?;
            for i in 0..k {
                match p.shards.get(&(i as u16)) {
                    Some(d) => out.extend_from_slice(d),
                    None => out.extend_from_slice(restored.get(&i)?),
                }
            }
        }
        out.truncate(p.frame_len as usize);
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
    fn malformed_rejected() {
        assert_eq!(Shard::from_bytes(&[0; 5]), Err(FecError::Malformed));
    }
}
