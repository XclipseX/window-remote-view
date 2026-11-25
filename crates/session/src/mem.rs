//! In-memory datagram pipe with optional random loss, for tests and examples.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::DatagramIo;

/// Shared FIFO of datagrams.
pub type Queue = Arc<Mutex<VecDeque<Vec<u8>>>>;

/// Sending half; datagrams are dropped with probability `loss` (data packets
/// only, handshake datagrams starting with `HS_MAGIC` are never dropped).
pub struct MemIo {
    q: Queue,
    loss: f64,
    rng: u64,
    pub sent: u64,
    pub dropped: u64,
}

impl MemIo {
    pub fn new(q: Queue, loss: f64, seed: u64) -> Self {
        Self {
            q,
            loss,
            rng: seed | 1,
            sent: 0,
            dropped: 0,
        }
    }

    fn next_f64(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let v = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (v >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl DatagramIo for MemIo {
    fn send(&mut self, buf: &[u8]) {
        self.sent += 1;
        if buf.first() != Some(&crate::HS_MAGIC) && self.next_f64() < self.loss {
            self.dropped += 1;
            return;
        }
        self.q.lock().unwrap().push_back(buf.to_vec());
    }
}

pub fn queue() -> Queue {
    Arc::new(Mutex::new(VecDeque::new()))
}

pub fn drain(q: &Queue) -> Vec<Vec<u8>> {
    q.lock().unwrap().drain(..).collect()
}
