//! Connection path selection: candidates are probed over one UDP socket with a
//! staggered start (happy-eyeballs: 0 / 150 / 600 ms); the first to ack wins.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    LanDirect,
    P2p,
    Relay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub kind: PathKind,
    pub addr: SocketAddr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Connected {
    pub kind: PathKind,
    pub addr: SocketAddr,
    pub rtt: Duration,
}

/// Default stagger for candidates 1, 2, 3...: 0 / 150 / 600 ms.
pub const DEFAULT_STAGGER_MS: [u64; 3] = [0, 150, 600];

const PROBE_MAGIC: [u8; 4] = *b"RVP1";
const KIND_PROBE: u8 = 1;
const KIND_ACK: u8 = 2;
const PROBE_LEN: usize = 13;

/// Build a probe packet carrying `nonce`.
pub fn encode_probe(nonce: u64) -> [u8; PROBE_LEN] {
    packet(KIND_PROBE, nonce)
}

fn packet(kind: u8, nonce: u64) -> [u8; PROBE_LEN] {
    let mut p = [0u8; PROBE_LEN];
    p[..4].copy_from_slice(&PROBE_MAGIC);
    p[4] = kind;
    p[5..].copy_from_slice(&nonce.to_be_bytes());
    p
}

/// If `data` is a probe, return the ack to send back. Responders (host, relay) call this.
pub fn probe_reply(data: &[u8]) -> Option<[u8; PROBE_LEN]> {
    if data.len() == PROBE_LEN && data[..4] == PROBE_MAGIC && data[4] == KIND_PROBE {
        let nonce = u64::from_be_bytes(data[5..].try_into().ok()?);
        Some(packet(KIND_ACK, nonce))
    } else {
        None
    }
}

fn parse_ack(data: &[u8]) -> Option<u64> {
    if data.len() == PROBE_LEN && data[..4] == PROBE_MAGIC && data[4] == KIND_ACK {
        Some(u64::from_be_bytes(data[5..].try_into().ok()?))
    } else {
        None
    }
}

fn fresh_nonce(salt: usize) -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(salt);
    h.finish()
}

/// Probe `candidates` in order with staggered starts (`stagger[i]`, last value reused
/// for extra candidates) and return the first that answers within `timeout`.
/// Started candidates are re-probed every `retry` interval.
pub async fn race_connect(
    sock: &UdpSocket,
    candidates: &[Candidate],
    stagger: &[u64],
    retry: Duration,
    timeout: Duration,
) -> io::Result<Connected> {
    let start = Instant::now();
    let nonces: Vec<u64> = (0..candidates.len()).map(fresh_nonce).collect();
    let by_nonce: HashMap<u64, usize> = nonces.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let starts: Vec<Duration> = (0..candidates.len())
        .map(|i| Duration::from_millis(*stagger.get(i).or(stagger.last()).unwrap_or(&0)))
        .collect();
    let mut last_sent: Vec<Option<Instant>> = vec![None; candidates.len()];
    let mut buf = [0u8; 64];
    let mut tick = tokio::time::interval(Duration::from_millis(10).min(retry));
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let now = Instant::now();
        for (i, c) in candidates.iter().enumerate() {
            if now.duration_since(start) < starts[i] {
                continue;
            }
            if last_sent[i].is_none_or(|t| now.duration_since(t) >= retry) {
                last_sent[i] = Some(now);
                let _ = sock.send_to(&encode_probe(nonces[i]), c.addr).await;
            }
        }
        tokio::select! {
            r = sock.recv_from(&mut buf) => {
                let (n, _from) = r?;
                if let Some(i) = parse_ack(&buf[..n]).and_then(|nc| by_nonce.get(&nc)) {
                    let c = candidates[*i];
                    let rtt = last_sent[*i].map_or(Duration::ZERO, |t| t.elapsed());
                    return Ok(Connected { kind: c.kind, addr: c.addr, rtt });
                }
            }
            _ = tick.tick() => {}
            _ = tokio::time::sleep_until(deadline) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no candidate answered"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn responder(delay: Duration) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = s.local_addr().unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 64];
            loop {
                let (n, from) = s.recv_from(&mut b).await.unwrap();
                if let Some(a) = probe_reply(&b[..n]) {
                    tokio::time::sleep(delay).await;
                    let _ = s.send_to(&a, from).await;
                }
            }
        });
        addr
    }

    #[tokio::test]
    async fn first_answering_candidate_wins() {
        let dead = UdpSocket::bind("127.0.0.1:0").await.unwrap(); // never answers
        let relay = responder(Duration::ZERO).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cands = [
            Candidate { kind: PathKind::LanDirect, addr: dead.local_addr().unwrap() },
            Candidate { kind: PathKind::Relay, addr: relay },
        ];
        let r = race_connect(&client, &cands, &DEFAULT_STAGGER_MS, Duration::from_millis(50), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(r.kind, PathKind::Relay);
    }

    #[tokio::test]
    async fn preferred_wins_when_both_answer() {
        let lan = responder(Duration::ZERO).await;
        let relay = responder(Duration::ZERO).await;
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cands = [
            Candidate { kind: PathKind::LanDirect, addr: lan },
            Candidate { kind: PathKind::Relay, addr: relay },
        ];
        let r = race_connect(&client, &cands, &DEFAULT_STAGGER_MS, Duration::from_millis(50), Duration::from_secs(2))
            .await
            .unwrap();
        assert_eq!(r.kind, PathKind::LanDirect);
    }

    #[tokio::test]
    async fn times_out() {
        let dead = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let cands = [Candidate { kind: PathKind::P2p, addr: dead.local_addr().unwrap() }];
        let e = race_connect(&client, &cands, &[0], Duration::from_millis(20), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    }
}
