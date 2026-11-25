//! Quality slider (1..=100) to bitrate / QP mapping.
//!
//! Formula from docs/research-1-ket-qua-gemini.md ("Agent 3"):
//! `B = 0.8 * (45/0.8)^(s^0.9) * (WH/1920*1080)^0.75 * (fps/60)^0.55 * k_codec`
//! with `s = (S-1)/99`, anchored at 1080p60 H.264 (0.8 Mbps at S=1, 45 Mbps at
//! S=100). Content (general, 1.0) and chroma (4:2:0, 1.0) factors are neutral.

/// Real-time codec efficiency classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
    Vp9,
    Hevc,
    Av1,
}

/// research-2: HEVC is the default codec.
pub const DEFAULT_CODEC: Codec = Codec::Hevc;
/// Default slider position; gives ~14 Mbps for HEVC 1080p60 (research-2 Internet range 12-25 Mbps).
pub const DEFAULT_QUALITY: u8 = 80;

impl Codec {
    pub fn efficiency(self) -> f64 {
        match self {
            Codec::H264 => 1.00,
            Codec::Vp9 => 0.72,
            Codec::Hevc => 0.65,
            Codec::Av1 => 0.52,
        }
    }
}

fn norm(quality: u8) -> f64 {
    (f64::from(quality.clamp(1, 100)) - 1.0) / 99.0
}

/// Target bitrate in bits per second.
pub fn slider_to_bitrate(quality: u8, width: u32, height: u32, fps: u32, codec: Codec) -> u64 {
    let s = norm(quality);
    let anchor_mbps = 0.8 * (45.0_f64 / 0.8).powf(s.powf(0.90));
    let res = ((f64::from(width) * f64::from(height)) / (1920.0 * 1080.0)).powf(0.75);
    let fr = (f64::from(fps.max(1)) / 60.0).powf(0.55);
    (anchor_mbps * res * fr * codec.efficiency() * 1_000_000.0).round() as u64
}

/// Target QP for CQP/VBR modes.
pub fn slider_to_qp(quality: u8, codec: Codec) -> u8 {
    let s = norm(quality).powf(0.85);
    match codec {
        Codec::H264 | Codec::Hevc => (44.0 - 28.0 * s).round() as u8,
        Codec::Vp9 | Codec::Av1 => (56.0 - 38.0 * s).round() as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors() {
        assert_eq!(slider_to_bitrate(1, 1920, 1080, 60, Codec::H264), 800_000);
        assert_eq!(
            slider_to_bitrate(100, 1920, 1080, 60, Codec::H264),
            45_000_000
        );
        assert_eq!(
            slider_to_bitrate(100, 1920, 1080, 60, Codec::Hevc),
            29_250_000
        );
        assert_eq!(slider_to_qp(1, Codec::Hevc), 44);
        assert_eq!(slider_to_qp(100, Codec::Hevc), 16);
        assert_eq!(slider_to_qp(100, Codec::Av1), 18);
    }

    #[test]
    fn monotonic_and_default() {
        let mut prev = 0;
        for q in 1..=100 {
            let b = slider_to_bitrate(q, 1920, 1080, 60, DEFAULT_CODEC);
            assert!(b > prev);
            prev = b;
        }
        let d = slider_to_bitrate(DEFAULT_QUALITY, 1920, 1080, 60, DEFAULT_CODEC);
        assert!((12_000_000..=25_000_000).contains(&d), "{d}");
        // more pixels / fps => more bits
        assert!(
            slider_to_bitrate(50, 2560, 1440, 60, Codec::Hevc)
                > slider_to_bitrate(50, 1920, 1080, 60, Codec::Hevc)
        );
        assert!(
            slider_to_bitrate(50, 1920, 1080, 120, Codec::Hevc)
                > slider_to_bitrate(50, 1920, 1080, 60, Codec::Hevc)
        );
        // out-of-range clamps
        assert_eq!(slider_to_bitrate(0, 1920, 1080, 60, Codec::H264), 800_000);
        assert_eq!(
            slider_to_bitrate(200, 1920, 1080, 60, Codec::H264),
            45_000_000
        );
    }
}
