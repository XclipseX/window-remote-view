/// 64-entry sliding anti-replay window.
#[derive(Debug, Default, Clone)]
pub struct ReplayWindow {
    highest: Option<u64>,
    /// bit i set => nonce (highest - i) already seen.
    bitmap: u64,
}

impl ReplayWindow {
    pub const SIZE: u64 = 64;

    pub fn new() -> Self {
        Self::default()
    }

    /// True if `n` would be accepted (not seen, not too old). Does not mutate.
    pub fn check(&self, n: u64) -> bool {
        match self.highest {
            None => true,
            Some(h) if n > h => true,
            Some(h) => {
                let d = h - n;
                d < Self::SIZE && self.bitmap & (1u64 << d) == 0
            }
        }
    }

    /// Mark `n` as seen. Call only after authentication succeeded.
    pub fn update(&mut self, n: u64) {
        match self.highest {
            None => {
                self.highest = Some(n);
                self.bitmap = 1;
            }
            Some(h) if n > h => {
                let shift = n - h;
                self.bitmap = if shift >= Self::SIZE { 0 } else { self.bitmap << shift };
                self.bitmap |= 1;
                self.highest = Some(n);
            }
            Some(h) => {
                let d = h - n;
                if d < Self::SIZE {
                    self.bitmap |= 1u64 << d;
                }
            }
        }
    }

    /// Check and update in one step; returns false if rejected.
    pub fn check_and_update(&mut self, n: u64) -> bool {
        if self.check(n) {
            self.update(n);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window() {
        let mut w = ReplayWindow::new();
        assert!(w.check_and_update(0));
        assert!(!w.check_and_update(0));
        assert!(w.check_and_update(10));
        assert!(w.check_and_update(5)); // out of order
        assert!(!w.check_and_update(5));
        assert!(w.check_and_update(200));
        assert!(!w.check_and_update(10)); // too old
        assert!(w.check_and_update(137)); // 200-63
        assert!(!w.check_and_update(136));
    }
}
