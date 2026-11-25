//! Connection path selection: candidates are probed over one UDP socket with a
//! staggered start (happy-eyeballs: 0 / 150 / 600 ms); the first to ack wins.
//!
//! [`race_paths`] is the full policy from the design notes: LAN (USB, IPv6 link-local)
//! at t=0, P2P at t=150 ms (IPv6 50 ms earlier, 1.5 s timeout), relay pre-warmed at
//! t=500 ms and only chosen when P2P failed; a [`PathUpgrader`] then keeps retrying
//! better paths in the background.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathKind {
    /// Same-LAN path, including USB tethering and IPv6 link-local.
    LanDirect,
    P2p,
    Relay,
}

impl PathKind {
    /// Preference tier, lower is better.
    pub fn rank(self) -> u8 {
        match self {
            PathKind::LanDirect => 0,
            PathKind::P2p => 1,
            PathKind::Relay => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub kind: PathKind,
    pub addr: SocketAddr,
    /// Preference within a kind, lower is better (USB tether 0, Ethernet 1, Wi-Fi 2, ...).
    pub priority: u8,
}

impl Candidate {
    pub fn new(kind: PathKind, addr: SocketAddr) -> Self {
        Self { kind, addr, priority: 0 }
    }

    pub fn with_priority(mut self, priority: u8) -> Self {
        self.priority = priority;
        self
    }

    fn order(&self) -> (u8, u8) {
        (self.kind.rank(), self.priority)
    }
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

// ---------------------------------------------------------------------------
// Scheduled racing
// ---------------------------------------------------------------------------

/// Timing policy for [`race_paths`]. `Default` is the design schedule.
#[derive(Debug, Clone, Copy)]
pub struct RaceConfig {
    pub lan_start: Duration,
    pub p2p_start: Duration,
    /// IPv6 P2P candidates start this much before IPv4 ones.
    pub v6_lead: Duration,
    /// P2P gives up this long after `p2p_start`.
    pub p2p_timeout: Duration,
    /// Relay probing (pre-warm) starts here.
    pub relay_warm: Duration,
    /// Re-probe interval for started candidates.
    pub retry: Duration,
    /// After the first eligible ack, wait this long for a better-priority one.
    pub grace: Duration,
    /// Hard limit for the whole race.
    pub overall_timeout: Duration,
}

impl Default for RaceConfig {
    fn default() -> Self {
        Self {
            lan_start: Duration::ZERO,
            p2p_start: Duration::from_millis(150),
            v6_lead: Duration::from_millis(50),
            p2p_timeout: Duration::from_millis(1500),
            relay_warm: Duration::from_millis(500),
            retry: Duration::from_millis(100),
            grace: Duration::from_millis(15),
            overall_timeout: Duration::from_secs(5),
        }
    }
}

impl RaceConfig {
    /// Probe start offset of `c` from t=0.
    pub fn start_of(&self, c: &Candidate) -> Duration {
        match c.kind {
            PathKind::LanDirect => self.lan_start,
            PathKind::P2p if c.addr.is_ipv6() => self.p2p_start.saturating_sub(self.v6_lead),
            PathKind::P2p => self.p2p_start,
            PathKind::Relay => self.relay_warm,
        }
    }
}

/// Result of [`race_paths`].
pub struct RaceResult {
    pub winner: Connected,
    /// Background retrier for better paths than `winner` (None if nothing is better).
    pub upgrader: Option<PathUpgrader>,
}

/// Run the scheduled race on `sock` (should be dual-stack if IPv6 candidates are given).
/// Relay is selected only after P2P has timed out (or when there are no P2P candidates,
/// after the warm-up point) with nothing better answering.
pub async fn race_paths(
    sock: &Arc<UdpSocket>,
    candidates: &[Candidate],
    cfg: &RaceConfig,
) -> io::Result<RaceResult> {
    let start = Instant::now();
    let n = candidates.len();
    let nonces: Vec<u64> = (0..n).map(fresh_nonce).collect();
    let by_nonce: HashMap<u64, usize> = nonces.iter().enumerate().map(|(i, x)| (*x, i)).collect();
    let starts: Vec<Duration> = candidates.iter().map(|c| cfg.start_of(c)).collect();
    let has_p2p = candidates.iter().any(|c| c.kind == PathKind::P2p);
    let relay_allowed_at = if has_p2p { cfg.p2p_start + cfg.p2p_timeout } else { cfg.relay_warm };
    let p2p_end = cfg.p2p_start + cfg.p2p_timeout;

    let mut last_sent: Vec<Option<Instant>> = vec![None; n];
    let mut rtt: Vec<Option<Duration>> = vec![None; n];
    let mut first_ack: Option<Instant> = None;
    let mut buf = [0u8; 64];
    let mut tick = tokio::time::interval(Duration::from_millis(5).min(cfg.retry));
    let deadline = tokio::time::Instant::now() + cfg.overall_timeout;

    loop {
        let now = Instant::now();
        let el = now.duration_since(start);
        for (i, c) in candidates.iter().enumerate() {
            if el < starts[i] || (c.kind == PathKind::P2p && el >= p2p_end) || rtt[i].is_some() {
                continue;
            }
            if last_sent[i].is_none_or(|t| now.duration_since(t) >= cfg.retry) {
                last_sent[i] = Some(now);
                let _ = sock.send_to(&encode_probe(nonces[i]), c.addr).await;
            }
        }
        // Eligible acked candidates: relay only once P2P is over.
        let eligible = |i: usize| rtt[i].is_some() && (candidates[i].kind != PathKind::Relay || el >= relay_allowed_at);
        let best = (0..n).filter(|i| eligible(*i)).min_by_key(|i| (candidates[*i].order(), rtt[*i]));
        if let Some(b) = best {
            let c = &candidates[b];
            let perfect = c.kind == PathKind::LanDirect
                && !candidates.iter().any(|o| o.kind == PathKind::LanDirect && o.priority < c.priority);
            let waited = first_ack.is_some_and(|t| now.duration_since(t) >= cfg.grace);
            // A relay win has already waited out P2P; take it right away.
            if perfect || waited || c.kind == PathKind::Relay {
                let winner = Connected { kind: c.kind, addr: c.addr, rtt: rtt[b].unwrap_or_default() };
                let upgrader = PathUpgrader::spawn(sock.clone(), winner, candidates, cfg.retry.max(Duration::from_millis(500)));
                return Ok(RaceResult { winner, upgrader });
            }
        }
        tokio::select! {
            r = sock.recv_from(&mut buf) => {
                let (len, _from) = match r {
                    Ok(x) => x,
                    Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                    Err(e) => return Err(e),
                };
                if let Some(i) = parse_ack(&buf[..len]).and_then(|nc| by_nonce.get(&nc)).copied() {
                    if rtt[i].is_none() {
                        let t = last_sent[i].map_or(Duration::ZERO, |t| t.elapsed());
                        rtt[i] = Some(t);
                        // The grace clock runs for non-relay acks; relay acks are warm-up only.
                        if candidates[i].kind != PathKind::Relay {
                            first_ack.get_or_insert_with(Instant::now);
                        }
                    }
                }
            }
            _ = tick.tick() => {}
            _ = tokio::time::sleep_until(deadline) => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no path answered"));
            }
        }
    }
}

struct UpgraderState {
    current: Connected,
    current_order: (u8, u8),
    nonces: Vec<(u64, Candidate)>,
}

/// Background retrier: periodically probes paths better than the current one. It only
/// *sends* probes; the application's receive loop feeds incoming datagrams to
/// [`PathUpgrader::on_packet`], which reports an upgrade when a better path answers.
pub struct PathUpgrader {
    state: Arc<Mutex<UpgraderState>>,
    task: tokio::task::JoinHandle<()>,
}

impl PathUpgrader {
    /// Returns None when no candidate is better than `current`.
    pub fn spawn(sock: Arc<UdpSocket>, current: Connected, candidates: &[Candidate], interval: Duration) -> Option<Self> {
        let cur = candidates.iter().find(|c| c.addr == current.addr && c.kind == current.kind);
        let current_order = cur.map_or((current.kind.rank(), u8::MAX), |c| c.order());
        let nonces: Vec<(u64, Candidate)> = candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| c.order() < current_order)
            .map(|(i, c)| (fresh_nonce(i + 1000), *c))
            .collect();
        if nonces.is_empty() {
            return None;
        }
        let state = Arc::new(Mutex::new(UpgraderState { current, current_order, nonces }));
        let st = state.clone();
        let task = tokio::spawn(async move {
            let mut t = tokio::time::interval(interval);
            loop {
                t.tick().await;
                let targets: Vec<(u64, SocketAddr)> = {
                    let g = st.lock().unwrap();
                    g.nonces.iter().filter(|(_, c)| c.order() < g.current_order).map(|(nc, c)| (*nc, c.addr)).collect()
                };
                if targets.is_empty() {
                    break;
                }
                for (nc, addr) in targets {
                    let _ = sock.send_to(&encode_probe(nc), addr).await;
                }
            }
        });
        Some(Self { state, task })
    }

    /// Feed a received datagram. Returns the new path if it is an ack from a better one.
    pub fn on_packet(&self, data: &[u8]) -> Option<Connected> {
        let nonce = parse_ack(data)?;
        let mut g = self.state.lock().unwrap();
        let (_, c) = *g.nonces.iter().find(|(n, _)| *n == nonce)?;
        if c.order() >= g.current_order {
            return None;
        }
        let up = Connected { kind: c.kind, addr: c.addr, rtt: Duration::ZERO };
        g.current = up;
        g.current_order = c.order();
        Some(up)
    }

    pub fn current(&self) -> Connected {
        self.state.lock().unwrap().current
    }
}

impl Drop for PathUpgrader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// ---------------------------------------------------------------------------
// Session migration
// ---------------------------------------------------------------------------

/// Verdict of [`SessionPath::on_packet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketVerdict {
    /// Authentic packet from the active address.
    Active,
    /// Authentic packet from a new address, not yet enough to switch.
    Pending { seen: u32, needed: u32 },
    /// The active address was switched.
    Migrated { from: SocketAddr, to: SocketAddr },
    /// Dropped: wrong session id or failed authentication.
    Rejected,
}

/// Active remote address of one session, migrating on authenticated packets from a new
/// source (roaming Wi-Fi <-> cellular, NAT rebinding). Authentication is supplied by the
/// caller (and must reject replays). Anti-flap: a new address needs `required` valid
/// packets in a row, or one valid packet plus a probe response from that address.
#[derive(Debug)]
pub struct SessionPath {
    session_id: u32,
    active: SocketAddr,
    required: u32,
    pending_ttl: Duration,
    pending: Option<Pending>,
}

#[derive(Debug)]
struct Pending {
    addr: SocketAddr,
    count: u32,
    since: Instant,
    nonce: u64,
}

impl SessionPath {
    pub fn new(session_id: u32, active: SocketAddr) -> Self {
        Self { session_id, active, required: 2, pending_ttl: Duration::from_secs(2), pending: None }
    }

    pub fn with_required(mut self, n: u32) -> Self {
        self.required = n.max(1);
        self
    }

    pub fn session_id(&self) -> u32 {
        self.session_id
    }

    pub fn active(&self) -> SocketAddr {
        self.active
    }

    /// Process a datagram for `session_id` from `from`.
    pub fn on_packet(
        &mut self,
        session_id: u32,
        from: SocketAddr,
        data: &[u8],
        now: Instant,
        is_authentic: impl FnOnce(&[u8]) -> bool,
    ) -> PacketVerdict {
        if session_id != self.session_id || !is_authentic(data) {
            return PacketVerdict::Rejected;
        }
        if from == self.active {
            self.pending = None; // the old path is alive: any candidate switch restarts
            return PacketVerdict::Active;
        }
        let fresh = match &mut self.pending {
            Some(p) if p.addr == from && now.saturating_duration_since(p.since) < self.pending_ttl => {
                p.count += 1;
                false
            }
            _ => true,
        };
        if fresh {
            self.pending = Some(Pending { addr: from, count: 1, since: now, nonce: fresh_nonce(from.port() as usize) });
        }
        let count = self.pending.as_ref().map_or(0, |p| p.count);
        if count >= self.required {
            return self.migrate(from);
        }
        PacketVerdict::Pending { seen: count, needed: self.required }
    }

    /// Probe to send to the pending address (accelerates migration to one round trip).
    pub fn pending_probe(&self) -> Option<(SocketAddr, [u8; PROBE_LEN])> {
        self.pending.as_ref().map(|p| (p.addr, encode_probe(p.nonce)))
    }

    /// A probe ack arrived from `from`; migrates if it answers the pending probe.
    pub fn on_probe_ack(&mut self, from: SocketAddr, data: &[u8]) -> Option<PacketVerdict> {
        let nonce = parse_ack(data)?;
        let ok = self.pending.as_ref().is_some_and(|p| p.addr == from && p.nonce == nonce);
        ok.then(|| self.migrate(from))
    }

    fn migrate(&mut self, to: SocketAddr) -> PacketVerdict {
        let from = std::mem::replace(&mut self.active, to);
        self.pending = None;
        PacketVerdict::Migrated { from, to }
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
            Candidate::new(PathKind::LanDirect, dead.local_addr().unwrap()),
            Candidate::new(PathKind::Relay, relay),
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
            Candidate::new(PathKind::LanDirect, lan),
            Candidate::new(PathKind::Relay, relay),
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
        let cands = [Candidate::new(PathKind::P2p, dead.local_addr().unwrap())];
        let e = race_connect(&client, &cands, &[0], Duration::from_millis(20), Duration::from_millis(100))
            .await
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    }

    // ---- scheduled race ----

    use std::sync::atomic::{AtomicBool, Ordering};

    async fn gated_responder(on: Arc<AtomicBool>) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = s.local_addr().unwrap();
        tokio::spawn(async move {
            let mut b = [0u8; 64];
            loop {
                let (n, from) = s.recv_from(&mut b).await.unwrap();
                if on.load(Ordering::SeqCst) {
                    if let Some(a) = probe_reply(&b[..n]) {
                        let _ = s.send_to(&a, from).await;
                    }
                }
            }
        });
        addr
    }

    fn fast_cfg() -> RaceConfig {
        RaceConfig {
            lan_start: Duration::ZERO,
            p2p_start: Duration::from_millis(60),
            v6_lead: Duration::from_millis(20),
            p2p_timeout: Duration::from_millis(300),
            relay_warm: Duration::from_millis(120),
            retry: Duration::from_millis(20),
            grace: Duration::from_millis(15),
            overall_timeout: Duration::from_secs(3),
        }
    }

    #[test]
    fn default_schedule_matches_design() {
        let c = RaceConfig::default();
        let a: SocketAddr = "1.2.3.4:1".parse().unwrap();
        let a6: SocketAddr = "[2001:db8::1]:1".parse().unwrap();
        assert_eq!(c.start_of(&Candidate::new(PathKind::LanDirect, a)), Duration::ZERO);
        assert_eq!(c.start_of(&Candidate::new(PathKind::P2p, a)), Duration::from_millis(150));
        assert_eq!(c.start_of(&Candidate::new(PathKind::P2p, a6)), Duration::from_millis(100));
        assert_eq!(c.start_of(&Candidate::new(PathKind::Relay, a)), Duration::from_millis(500));
        assert_eq!(c.p2p_timeout, Duration::from_millis(1500));
    }

    #[tokio::test]
    async fn lan_beats_p2p_and_relay() {
        let on = Arc::new(AtomicBool::new(true));
        let (lan, p2p, relay) = (gated_responder(on.clone()).await, gated_responder(on.clone()).await, gated_responder(on).await);
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let cands = [
            Candidate::new(PathKind::Relay, relay),
            Candidate::new(PathKind::P2p, p2p),
            Candidate::new(PathKind::LanDirect, lan),
        ];
        let t = Instant::now();
        let r = race_paths(&sock, &cands, &fast_cfg()).await.unwrap();
        assert_eq!(r.winner.kind, PathKind::LanDirect);
        assert!(t.elapsed() < Duration::from_millis(50));
        assert!(r.upgrader.is_none());
    }

    #[tokio::test]
    async fn usb_priority_wins_among_lan() {
        let on = Arc::new(AtomicBool::new(true));
        let (wifi, usb) = (gated_responder(on.clone()).await, gated_responder(on).await);
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let cands = [
            Candidate::new(PathKind::LanDirect, wifi).with_priority(2),
            Candidate::new(PathKind::LanDirect, usb).with_priority(0),
        ];
        let r = race_paths(&sock, &cands, &fast_cfg()).await.unwrap();
        assert_eq!(r.winner.addr, usb);
    }

    #[tokio::test]
    async fn p2p_wins_over_ready_relay() {
        let on = Arc::new(AtomicBool::new(true));
        let (p2p, relay) = (gated_responder(on.clone()).await, gated_responder(on).await);
        let dead = gated_responder(Arc::new(AtomicBool::new(false))).await;
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let cands = [
            Candidate::new(PathKind::LanDirect, dead),
            Candidate::new(PathKind::Relay, relay),
            Candidate::new(PathKind::P2p, p2p),
        ];
        let r = race_paths(&sock, &cands, &fast_cfg()).await.unwrap();
        assert_eq!(r.winner.kind, PathKind::P2p);
        assert!(r.upgrader.is_some()); // LAN is still better
    }

    #[tokio::test]
    async fn relay_only_after_p2p_timeout_then_upgrade() {
        let relay = gated_responder(Arc::new(AtomicBool::new(true))).await;
        let p2p_on = Arc::new(AtomicBool::new(false));
        let p2p = gated_responder(p2p_on.clone()).await;
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let cands = [Candidate::new(PathKind::P2p, p2p), Candidate::new(PathKind::Relay, relay)];
        let cfg = fast_cfg();
        let t = Instant::now();
        let r = race_paths(&sock, &cands, &cfg).await.unwrap();
        // relay was warm at 120 ms but must wait for p2p_start + p2p_timeout = 360 ms
        assert!(t.elapsed() >= Duration::from_millis(350), "{:?}", t.elapsed());
        assert_eq!(r.winner.kind, PathKind::Relay);
        // the upgrader stopped nothing: P2P later becomes reachable -> upgrade reported
        let up = r.upgrader.expect("p2p is better than relay");
        p2p_on.store(true, Ordering::SeqCst);
        let mut buf = [0u8; 64];
        let got = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let (n, _) = sock.recv_from(&mut buf).await.unwrap();
                if let Some(c) = up.on_packet(&buf[..n]) {
                    return c;
                }
            }
        })
        .await
        .expect("upgrade");
        assert_eq!(got.kind, PathKind::P2p);
        assert_eq!(got.addr, p2p);
        assert_eq!(up.current().kind, PathKind::P2p);
    }

    #[tokio::test]
    async fn race_times_out_when_nothing_answers() {
        let dead = gated_responder(Arc::new(AtomicBool::new(false))).await;
        let sock = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let mut cfg = fast_cfg();
        cfg.overall_timeout = Duration::from_millis(150);
        let e = race_paths(&sock, &[Candidate::new(PathKind::LanDirect, dead)], &cfg).await.err().unwrap();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    }

    // ---- session migration ----

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn migrates_after_two_valid_packets_within_one_rtt() {
        let (a, b) = (sa("10.0.0.1:5000"), sa("203.0.113.9:6000"));
        let mut sp = SessionPath::new(7, a);
        let rtt = Duration::from_millis(40);
        let t0 = Instant::now();
        assert_eq!(sp.on_packet(7, a, b"x", t0, |_| true), PacketVerdict::Active);
        assert_eq!(sp.on_packet(7, b, b"x", t0, |_| true), PacketVerdict::Pending { seen: 1, needed: 2 });
        assert_eq!(sp.active(), a);
        // back-to-back packet 2 ms later: well under one RTT, no round trip needed
        let t1 = t0 + Duration::from_millis(2);
        assert_eq!(sp.on_packet(7, b, b"x", t1, |_| true), PacketVerdict::Migrated { from: a, to: b });
        assert!(t1 - t0 < rtt);
        assert_eq!(sp.active(), b);
    }

    #[test]
    fn probe_response_migrates_after_one_packet() {
        let (a, b) = (sa("10.0.0.1:5000"), sa("10.0.0.2:5001"));
        let mut sp = SessionPath::new(1, a);
        let t = Instant::now();
        sp.on_packet(1, b, b"x", t, |_| true);
        let (to, probe) = sp.pending_probe().unwrap();
        assert_eq!(to, b);
        let ack = probe_reply(&probe).unwrap();
        // wrong source is ignored
        assert!(sp.on_probe_ack(sa("1.1.1.1:1"), &ack).is_none());
        assert_eq!(sp.on_probe_ack(b, &ack), Some(PacketVerdict::Migrated { from: a, to: b }));
    }

    #[test]
    fn unauthentic_or_wrong_session_never_migrates() {
        let (a, b) = (sa("10.0.0.1:5000"), sa("10.0.0.2:5001"));
        let mut sp = SessionPath::new(1, a);
        let t = Instant::now();
        for _ in 0..5 {
            assert_eq!(sp.on_packet(1, b, b"x", t, |_| false), PacketVerdict::Rejected);
            assert_eq!(sp.on_packet(2, b, b"x", t, |_| true), PacketVerdict::Rejected);
        }
        assert_eq!(sp.active(), a);
        assert!(sp.pending_probe().is_none());
    }

    #[test]
    fn anti_flap_interleaved_and_expired() {
        let (a, b, c) = (sa("10.0.0.1:1"), sa("10.0.0.2:2"), sa("10.0.0.3:3"));
        let mut sp = SessionPath::new(1, a);
        let t = Instant::now();
        sp.on_packet(1, b, b"x", t, |_| true);
        sp.on_packet(1, a, b"x", t, |_| true); // old path still alive: resets
        assert!(matches!(sp.on_packet(1, b, b"x", t, |_| true), PacketVerdict::Pending { seen: 1, .. }));
        // a different new address restarts the count
        assert!(matches!(sp.on_packet(1, c, b"x", t, |_| true), PacketVerdict::Pending { seen: 1, .. }));
        // pending entry expires
        let late = t + Duration::from_secs(3);
        assert!(matches!(sp.on_packet(1, c, b"x", late, |_| true), PacketVerdict::Pending { seen: 1, .. }));
        assert_eq!(sp.active(), a);
    }
}
