//! Priority send queue: input/control packets always leave before video shards.
//! Video is additionally rate-limited by a token bucket and bounded in depth
//! (oldest video packets are dropped when the queue overflows).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Input and control traffic: never delayed by the pacer, always first.
    Control,
    /// Video shards: paced.
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub priority: Priority,
    pub data: Vec<u8>,
}

pub struct Pacer {
    control: VecDeque<Packet>,
    video: VecDeque<Packet>,
    max_video: usize,
    rate_bytes_per_sec: f64,
    burst: f64,
    tokens: f64,
    last: Instant,
    dropped_video: u64,
}

impl Pacer {
    /// `rate_bps` is the video pacing rate in bits/s; `max_video` bounds queue depth in packets.
    pub fn new(rate_bps: u64, max_video: usize, now: Instant) -> Self {
        let rate = (rate_bps as f64 / 8.0).max(1.0);
        let burst = (rate * 0.005).max(2400.0); // ~5 ms or two MTUs
        Self {
            control: VecDeque::new(),
            video: VecDeque::new(),
            max_video,
            rate_bytes_per_sec: rate,
            burst,
            tokens: burst,
            last: now,
            dropped_video: 0,
        }
    }

    pub fn set_rate_bps(&mut self, rate_bps: u64) {
        self.rate_bytes_per_sec = (rate_bps as f64 / 8.0).max(1.0);
    }

    pub fn push(&mut self, priority: Priority, data: Vec<u8>) {
        let p = Packet { priority, data };
        match priority {
            Priority::Control => self.control.push_back(p),
            Priority::Video => {
                if self.video.len() >= self.max_video {
                    self.video.pop_front();
                    self.dropped_video += 1;
                }
                self.video.push_back(p);
            }
        }
    }

    pub fn dropped_video(&self) -> u64 {
        self.dropped_video
    }

    pub fn len(&self) -> usize {
        self.control.len() + self.video.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now.max(self.last);
        self.tokens = (self.tokens + dt * self.rate_bytes_per_sec).min(self.burst);
    }

    /// Next packet allowed to be sent at `now`: control first (unpaced), then video if tokens allow.
    pub fn pop_ready(&mut self, now: Instant) -> Option<Packet> {
        self.refill(now);
        if let Some(p) = self.control.pop_front() {
            self.tokens -= p.data.len() as f64;
            return Some(p);
        }
        let need = self.video.front()?.data.len() as f64;
        if self.tokens >= need.min(self.burst) {
            self.tokens -= need;
            return self.video.pop_front();
        }
        None
    }

    /// How long until the next video packet may be sent (None if nothing queued / ready now).
    pub fn wait_time(&mut self, now: Instant) -> Option<Duration> {
        self.refill(now);
        if !self.control.is_empty() {
            return Some(Duration::ZERO);
        }
        let need = self.video.front()?.data.len() as f64;
        let missing = need.min(self.burst) - self.tokens;
        if missing <= 0.0 {
            Some(Duration::ZERO)
        } else {
            Some(Duration::from_secs_f64(missing / self.rate_bytes_per_sec))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_before_video_regardless_of_arrival() {
        let t = Instant::now();
        let mut p = Pacer::new(100_000_000, 100, t);
        p.push(Priority::Video, vec![1; 100]);
        p.push(Priority::Video, vec![2; 100]);
        p.push(Priority::Control, vec![9; 10]);
        p.push(Priority::Control, vec![8; 10]);
        let order: Vec<u8> = std::iter::from_fn(|| p.pop_ready(t)).map(|x| x.data[0]).collect();
        assert_eq!(order, vec![9, 8, 1, 2]);
    }

    #[test]
    fn video_is_paced_but_control_is_not() {
        let t = Instant::now();
        let mut p = Pacer::new(8_000_000, 100, t); // 1 MB/s
        for _ in 0..10 {
            p.push(Priority::Video, vec![0; 1200]);
        }
        let mut sent = 0;
        while p.pop_ready(t).is_some() {
            sent += 1;
        }
        assert!((1..10).contains(&sent));
        assert!(p.wait_time(t).unwrap() > Duration::ZERO);
        p.push(Priority::Control, vec![7]);
        assert_eq!(p.pop_ready(t).unwrap().priority, Priority::Control);
        // after time passes, more video flows
        assert!(p.pop_ready(t + Duration::from_millis(10)).is_some());
    }

    #[test]
    fn overflow_drops_oldest_video() {
        let t = Instant::now();
        let mut p = Pacer::new(1_000_000_000, 2, t);
        for i in 0..4u8 {
            p.push(Priority::Video, vec![i]);
        }
        assert_eq!(p.dropped_video(), 2);
        assert_eq!(p.pop_ready(t).unwrap().data, vec![2]);
    }
}
