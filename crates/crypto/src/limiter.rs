use std::collections::HashMap;
use std::hash::Hash;

/// Per-peer exponential backoff after failed attempts. Time is passed in
/// (milliseconds, any monotonic origin) so the logic is pure.
///
/// Policy: first `free_attempts` (3) failures are free; the next failure locks
/// for `base_ms` (30 s), doubling each further failure up to `max_ms`.
#[derive(Debug, Clone)]
pub struct AttemptLimiter<K: Hash + Eq> {
    entries: HashMap<K, Entry>,
    free_attempts: u32,
    base_ms: u64,
    max_ms: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct Entry {
    failures: u32,
    locked_until: u64,
}

impl<K: Hash + Eq> Default for AttemptLimiter<K> {
    fn default() -> Self {
        Self::new(2, 30_000, 3_600_000)
    }
}

impl<K: Hash + Eq> AttemptLimiter<K> {
    /// `free_failures`: failures tolerated before any lock (lock starts on failure free_failures+1).
    pub fn new(free_failures: u32, base_ms: u64, max_ms: u64) -> Self {
        Self { entries: HashMap::new(), free_attempts: free_failures, base_ms, max_ms }
    }

    /// Ok if an attempt is allowed at `now_ms`, else Err(ms remaining).
    pub fn check(&self, peer: &K, now_ms: u64) -> Result<(), u64> {
        match self.entries.get(peer) {
            Some(e) if e.locked_until > now_ms => Err(e.locked_until - now_ms),
            _ => Ok(()),
        }
    }

    pub fn record_failure(&mut self, peer: K, now_ms: u64) {
        let e = self.entries.entry(peer).or_default();
        e.failures = e.failures.saturating_add(1);
        if e.failures > self.free_attempts {
            let exp = (e.failures - self.free_attempts - 1).min(32);
            let d = self.base_ms.saturating_mul(1u64 << exp).min(self.max_ms);
            e.locked_until = now_ms.saturating_add(d);
        }
    }

    pub fn record_success(&mut self, peer: &K) {
        self.entries.remove(peer);
    }

    pub fn failures(&self, peer: &K) -> u32 {
        self.entries.get(peer).map_or(0, |e| e.failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff() {
        let mut l: AttemptLimiter<String> = AttemptLimiter::default();
        let p = "phone".to_string();
        l.record_failure(p.clone(), 0);
        l.record_failure(p.clone(), 0);
        assert!(l.check(&p, 0).is_ok());
        l.record_failure(p.clone(), 1000);
        assert_eq!(l.check(&p, 1000), Err(30_000));
        assert!(l.check(&p, 31_000).is_ok());
        l.record_failure(p.clone(), 31_000);
        assert_eq!(l.check(&p, 31_000), Err(60_000));
        l.record_failure(p.clone(), 100_000);
        assert_eq!(l.check(&p, 100_000), Err(120_000));
        assert!(l.check(&"other".to_string(), 100_000).is_ok());
        l.record_success(&p);
        assert!(l.check(&p, 100_001).is_ok());
        for _ in 0..100 {
            l.record_failure(p.clone(), 0);
        }
        assert_eq!(l.check(&p, 0), Err(3_600_000));
    }
}
