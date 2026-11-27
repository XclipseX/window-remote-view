//! Frame-time metrics for frames actually SENT to the client (after encode).
//!
//! Pure and clock-free: callers pass a monotonic timestamp in seconds. The window keeps the last
//! `WINDOW_SECS` seconds, at most `MAX_FRAMES` inter-frame intervals.

use std::collections::VecDeque;

pub const WINDOW_SECS: f64 = 10.0;
pub const MAX_FRAMES: usize = 1200;

/// Summary published in the `stats` JSON event. All zero while no interval was seen yet.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameSummary {
    /// Frames sent during the last second.
    pub fps_now: f64,
    /// 1000 / mean frame time over the window.
    pub fps_avg: f64,
    /// 1000 / mean of the worst 1 % frame times (at least one sample).
    pub fps_low1: f64,
    pub frame_ms_mean: f64,
    pub frame_ms_std: f64,
    pub frame_ms_var: f64,
}

#[derive(Debug, Default)]
pub struct FrameStats {
    last_ts: Option<f64>,
    /// (timestamp of the later frame, interval in ms)
    items: VecDeque<(f64, f64)>,
}

impl FrameStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one sent frame at `now_s` (monotonic seconds).
    pub fn record(&mut self, now_s: f64) {
        if let Some(prev) = self.last_ts {
            let dt = ((now_s - prev) * 1000.0).max(0.0);
            self.items.push_back((now_s, dt));
            if self.items.len() > MAX_FRAMES {
                self.items.pop_front();
            }
        }
        self.last_ts = Some(now_s);
    }

    fn prune(&mut self, now_s: f64) {
        while self.items.front().is_some_and(|(t, _)| now_s - *t > WINDOW_SECS) {
            self.items.pop_front();
        }
    }

    pub fn summary(&mut self, now_s: f64) -> FrameSummary {
        self.prune(now_s);
        let n = self.items.len();
        if n == 0 {
            return FrameSummary::default();
        }
        let fps_now = self.items.iter().filter(|(t, _)| now_s - *t < 1.0 - 1e-9).count() as f64;
        let mean = self.items.iter().map(|(_, d)| d).sum::<f64>() / n as f64;
        let var = self.items.iter().map(|(_, d)| (d - mean).powi(2)).sum::<f64>() / n as f64;
        let mut sorted: Vec<f64> = self.items.iter().map(|(_, d)| *d).collect();
        sorted.sort_by(|a, b| b.total_cmp(a));
        let k = n.div_ceil(100).max(1);
        let worst = sorted[..k].iter().sum::<f64>() / k as f64;
        let inv = |ms: f64| if ms > 0.0 { 1000.0 / ms } else { 0.0 };
        FrameSummary {
            fps_now,
            fps_avg: inv(mean),
            fps_low1: inv(worst),
            frame_ms_mean: mean,
            frame_ms_std: var.sqrt(),
            frame_ms_var: var,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(intervals_ms: &[f64]) -> (FrameStats, f64) {
        let mut s = FrameStats::new();
        let mut t = 0.0;
        s.record(t);
        for d in intervals_ms {
            t += d / 1000.0;
            s.record(t);
        }
        (s, t)
    }

    fn close(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    #[test]
    fn empty_is_zero() {
        let mut s = FrameStats::new();
        assert_eq!(s.summary(0.0), FrameSummary::default());
        s.record(1.0); // a single frame has no interval yet
        assert_eq!(s.summary(1.0), FrameSummary::default());
    }

    #[test]
    fn steady_60fps() {
        let (mut s, t) = feed(&[16.0; 100]);
        let r = s.summary(t);
        close(r.fps_avg, 62.5);
        close(r.fps_low1, 62.5);
        close(r.frame_ms_mean, 16.0);
        close(r.frame_ms_var, 0.0);
        close(r.frame_ms_std, 0.0);
        assert!(r.fps_now >= 62.0 && r.fps_now <= 63.0);
    }

    #[test]
    fn known_set_variance_and_low1() {
        // 99 x 10 ms + 1 x 110 ms: mean 11, population variance = (99*1 + 99^2... computed below)
        let mut v = vec![10.0; 99];
        v.push(110.0);
        let (mut s, t) = feed(&v);
        let r = s.summary(t);
        close(r.frame_ms_mean, 11.0);
        let var = (99.0 * 1.0 + 99.0 * 99.0) / 100.0; // (10-11)^2 *99 + (110-11)^2
        close(r.frame_ms_var, var);
        close(r.frame_ms_std, var.sqrt());
        close(r.fps_avg, 1000.0 / 11.0);
        // worst 1% of 100 samples = the single 110 ms frame
        close(r.fps_low1, 1000.0 / 110.0);
    }

    #[test]
    fn low1_uses_ceil_of_one_percent() {
        // 200 samples -> worst 2: (50 + 30) / 2 = 40 ms -> 25 fps
        let mut v = vec![10.0; 198];
        v.push(50.0);
        v.push(30.0);
        let (mut s, t) = feed(&v);
        close(s.summary(t).fps_low1, 25.0);
    }

    #[test]
    fn window_is_bounded() {
        let (mut s, t) = feed(&vec![1.0; 3000]);
        s.summary(t);
        assert!(s.items.len() <= MAX_FRAMES);
        // After a long pause everything expires.
        assert_eq!(s.summary(t + 60.0), FrameSummary::default());
    }

    #[test]
    fn fps_now_counts_last_second() {
        let (mut s, t) = feed(&[100.0; 50]); // 10 fps for 5 s
        let r = s.summary(t);
        assert_eq!(r.fps_now, 10.0);
    }
}
