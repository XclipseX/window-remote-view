//! System-audio capture (WASAPI loopback) + Opus encoding.

use crate::{HostError, Result};

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
/// 10 ms @ 48 kHz.
pub const FRAME_SAMPLES: usize = 480;
pub const FRAME_MS: u8 = 10;
/// Value for `AudioPacket::channels_rate`. The proto crate does not document
/// this byte; 0x21 is what its own test uses, interpreted here as
/// (channels << 4) | rate_id with rate_id 1 = 48 kHz. ASSUMPTION: confirm
/// with the Android decoder before relying on it.
pub const CHANNELS_RATE: u8 = 0x21;

/// Opus encoder in restricted low-delay mode, 10 ms frames, interleaved f32.
pub struct OpusFrameEncoder {
    enc: opus::Encoder,
    pending: Vec<f32>,
    out: Vec<u8>,
}

impl OpusFrameEncoder {
    pub fn new(bitrate_bps: i32) -> Result<Self> {
        let mut enc = opus::Encoder::new(SAMPLE_RATE, opus::Channels::Stereo, opus::Application::LowDelay)
            .map_err(|e| HostError::Other(format!("opus: {e}")))?;
        enc.set_bitrate(opus::Bitrate::Bits(bitrate_bps))
            .map_err(|e| HostError::Other(format!("opus: {e}")))?;
        Ok(Self {
            enc,
            pending: Vec::new(),
            out: vec![0; 1200],
        })
    }

    /// Feed interleaved stereo f32 samples of any length; returns every
    /// complete 10 ms Opus packet now available.
    pub fn push(&mut self, interleaved: &[f32]) -> Result<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(interleaved);
        let mut packets = Vec::new();
        let n = FRAME_SAMPLES * CHANNELS;
        let mut off = 0;
        while self.pending.len() - off >= n {
            let len = self
                .enc
                .encode_float(&self.pending[off..off + n], &mut self.out)
                .map_err(|e| HostError::Other(format!("opus: {e}")))?;
            packets.push(self.out[..len].to_vec());
            off += n;
        }
        self.pending.drain(..off);
        Ok(packets)
    }
}

#[cfg(windows)]
pub use crate::audio_win::LoopbackCapture;

/// Non-Windows stub.
#[cfg(not(windows))]
pub struct LoopbackCapture;

#[cfg(not(windows))]
impl LoopbackCapture {
    pub fn new() -> Result<Self> {
        Err(HostError::Unsupported("WASAPI loopback needs Windows"))
    }
    /// Appends any available 48 kHz stereo f32 samples to `out`.
    pub fn poll(&mut self, _out: &mut Vec<f32>) -> Result<()> {
        Err(HostError::Unsupported("WASAPI loopback needs Windows"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_frames_chunking() {
        let mut e = OpusFrameEncoder::new(128_000).unwrap();
        let tone: Vec<f32> = (0..FRAME_SAMPLES * 2 * 5 + 100)
            .map(|i| ((i / 2) as f32 * 0.05).sin() * 0.3)
            .collect();
        let p = e.push(&tone).unwrap();
        assert_eq!(p.len(), 5);
        assert!(p.iter().all(|x| !x.is_empty() && x.len() <= 1200));
        // leftover 100 samples + 860 more => 960/960 = 1 frame
        let p = e.push(&vec![0.0; 860]).unwrap();
        assert_eq!(p.len(), 1);
    }
}
