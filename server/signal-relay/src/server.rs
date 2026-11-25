//! Server state machine (pure: datagram in -> datagrams out) plus the tokio UDP loop.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

use crate::proto::{ErrorCode, Id, Msg};

#[derive(Debug, Clone)]
pub struct Config {
    /// Registration lifetime without refresh (hosts refresh every 20 s).
    pub register_ttl: Duration,
    /// Relay session lifetime without traffic.
    pub session_idle: Duration,
    pub max_registrations: usize,
    pub max_sessions: usize,
    /// Per-session forwarding cap in bits per second (0 = unlimited).
    pub max_bps: u64,
}

/// Default per-session cap: 25 Mbps.
pub const DEFAULT_MAX_BPS: u64 = 25_000_000;

impl Default for Config {
    fn default() -> Self {
        Self {
            register_ttl: Duration::from_secs(60),
            session_idle: Duration::from_secs(60),
            max_registrations: 100_000,
            max_sessions: 10_000,
            max_bps: DEFAULT_MAX_BPS,
        }
    }
}

/// Token bucket in bytes; burst is 100 ms worth of traffic (at least one full packet).
struct TokenBucket {
    rate: f64, // bytes per second, 0 = unlimited
    burst: f64,
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    fn new(bps: u64, now: Instant) -> Self {
        let rate = bps as f64 / 8.0;
        let burst = (rate / 10.0).max(4096.0);
        Self { rate, burst, tokens: burst, last: now }
    }

    fn allow(&mut self, bytes: usize, now: Instant) -> bool {
        if self.rate <= 0.0 {
            return true;
        }
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.rate).min(self.burst);
        let need = bytes as f64;
        if self.tokens >= need {
            self.tokens -= need;
            true
        } else {
            false
        }
    }
}

struct Session {
    a: SocketAddr,
    b: Option<SocketAddr>,
    last: Instant,
    bucket: TokenBucket,
}

#[derive(Default)]
struct Inner {
    hosts: HashMap<Id, (SocketAddr, Instant)>,
    sessions: HashMap<Id, Session>,
}

/// Datagrams to send in response: `(destination, bytes)`.
pub type Outbox = Vec<(SocketAddr, Vec<u8>)>;

pub struct State {
    cfg: Config,
    inner: Mutex<Inner>,
    /// Peers connected over TCP: datagrams addressed to them are written to their stream.
    tcp_peers: Mutex<HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>>,
}

impl State {
    pub fn new(cfg: Config) -> Self {
        Self { cfg, inner: Mutex::new(Inner::default()), tcp_peers: Mutex::new(HashMap::new()) }
    }

    pub fn registered_hosts(&self) -> usize {
        self.inner.lock().unwrap().hosts.len()
    }

    pub fn sessions(&self) -> usize {
        self.inner.lock().unwrap().sessions.len()
    }

    /// Remove expired registrations and idle relay sessions.
    pub fn sweep(&self, now: Instant) {
        let mut g = self.inner.lock().unwrap();
        let ttl = self.cfg.register_ttl;
        g.hosts.retain(|_, (_, t)| now.saturating_duration_since(*t) < ttl);
        let idle = self.cfg.session_idle;
        g.sessions.retain(|_, s| now.saturating_duration_since(s.last) < idle);
    }

    /// Send `out` to its destinations: TCP-connected peers through their stream, the rest
    /// via `udp`.
    pub async fn deliver(&self, out: Outbox, udp: &UdpSocket) {
        for (to, bytes) in out {
            let tx = self.tcp_peers.lock().unwrap().get(&to).cloned();
            match tx {
                Some(tx) => {
                    let _ = tx.try_send(bytes); // slow TCP peer: drop rather than block others
                }
                None => {
                    let _ = udp.send_to(&bytes, to).await;
                }
            }
        }
    }

    pub fn handle(&self, from: SocketAddr, data: &[u8], now: Instant) -> Outbox {
        // rv-transport path probes: answer so relay/signal can be a race candidate.
        if let Some(ack) = probe_reply(data) {
            return vec![(from, ack.to_vec())];
        }
        let Ok(msg) = Msg::decode(data) else { return Vec::new() };
        let mut g = self.inner.lock().unwrap();
        match msg {
            Msg::Register { host_id } => {
                if !g.hosts.contains_key(&host_id) && g.hosts.len() >= self.cfg.max_registrations {
                    return vec![err(from, ErrorCode::Full)];
                }
                g.hosts.insert(host_id, (from, now));
                vec![(from, Msg::RegisterAck { observed: from }.encode())]
            }
            Msg::Lookup { host_id } => {
                let host = g
                    .hosts
                    .get(&host_id)
                    .filter(|(_, t)| now.saturating_duration_since(*t) < self.cfg.register_ttl)
                    .map(|(a, _)| *a);
                let mut out = vec![(from, Msg::LookupResp { host_id, addr: host }.encode())];
                if let Some(h) = host {
                    out.push((h, Msg::PeerNotify { client: from }.encode()));
                }
                out
            }
            Msg::RelayJoin { token } => {
                if !g.sessions.contains_key(&token) && g.sessions.len() >= self.cfg.max_sessions {
                    return vec![err(from, ErrorCode::Full)];
                }
                let s = g.sessions.entry(token).or_insert(Session { a: from, b: None, last: now, bucket: TokenBucket::new(self.cfg.max_bps, now) });
                s.last = now;
                if s.a != from && s.b.is_none() {
                    s.b = Some(from);
                }
                if s.a != from && s.b != Some(from) {
                    return vec![err(from, ErrorCode::Full)]; // third party
                }
                let paired = s.b.is_some();
                let ack = |to| (to, Msg::RelayJoinAck { token, paired }.encode());
                let mut out = vec![ack(from)];
                if paired {
                    // Tell the first joiner too so both sides learn the session is live.
                    let other = if s.a == from { s.b.unwrap() } else { s.a };
                    out.push(ack(other));
                }
                out
            }
            Msg::RelayData { token, payload } => {
                let Some(s) = g.sessions.get_mut(&token) else {
                    return vec![err(from, ErrorCode::NotPaired)];
                };
                let to = match (s.a == from, s.b) {
                    (true, Some(b)) => b,
                    (false, Some(b)) if b == from => s.a,
                    _ => return vec![err(from, ErrorCode::NotPaired)],
                };
                s.last = now;
                if !s.bucket.allow(payload.len(), now) {
                    return Vec::new(); // over the per-session cap: drop (video tolerates loss)
                }
                // Forwarded blindly: payload is opaque (end-to-end encrypted by peers).
                vec![(to, Msg::RelayData { token, payload }.encode())]
            }
            // Server-originated messages are ignored if received.
            _ => Vec::new(),
        }
    }
}

fn err(to: SocketAddr, code: ErrorCode) -> (SocketAddr, Vec<u8>) {
    (to, Msg::Error { code }.encode())
}

/// Same probe format as `rv-transport::path` (duplicated to avoid a crate dependency).
fn probe_reply(data: &[u8]) -> Option<[u8; 13]> {
    if data.len() == 13 && &data[..4] == b"RVP1" && data[4] == 1 {
        let mut p = [0u8; 13];
        p.copy_from_slice(data);
        p[4] = 2;
        Some(p)
    } else {
        None
    }
}

/// Run the UDP loop until the socket fails. Spawns the expiry sweeper.
pub async fn serve(sock: Arc<UdpSocket>, state: Arc<State>) -> io::Result<()> {
    let st = state.clone();
    let sweeper = tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(5));
        loop {
            t.tick().await;
            st.sweep(Instant::now());
        }
    });
    let mut buf = vec![0u8; 2048];
    let res = loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, from)) => {
                let out = state.handle(from, &buf[..n], Instant::now());
                state.deliver(out, &sock).await;
            }
            // Windows reports ICMP port-unreachable as recv errors; keep serving.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(e) => break Err(e),
        }
    };
    sweeper.abort();
    res
}

/// Accept TCP clients (u16 big-endian length-prefixed datagrams) and bridge them into the
/// same session table as UDP peers. `udp` is used to reach UDP-side peers of a session.
pub async fn serve_tcp(listener: TcpListener, state: Arc<State>, udp: Arc<UdpSocket>) -> io::Result<()> {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) if e.kind() == io::ErrorKind::ConnectionAborted => continue,
            Err(e) => return Err(e),
        };
        let _ = stream.set_nodelay(true);
        let (state, udp) = (state.clone(), udp.clone());
        tokio::spawn(async move {
            let (mut rd, mut wr) = stream.into_split();
            let (tx, mut rx) = mpsc::channel::<Vec<u8>>(1024);
            state.tcp_peers.lock().unwrap().insert(peer, tx);
            let writer = tokio::spawn(async move {
                while let Some(d) = rx.recv().await {
                    let mut f = Vec::with_capacity(2 + d.len());
                    f.extend_from_slice(&(d.len() as u16).to_be_bytes());
                    f.extend_from_slice(&d);
                    if wr.write_all(&f).await.is_err() {
                        break;
                    }
                }
            });
            let mut len = [0u8; 2];
            let mut buf = Vec::new();
            // Idle TCP connections are closed so dead peers do not pin resources.
            let idle = state.cfg.session_idle;
            loop {
                let r = tokio::time::timeout(idle, async {
                    rd.read_exact(&mut len).await?;
                    buf.resize(u16::from_be_bytes(len) as usize, 0);
                    rd.read_exact(&mut buf).await
                })
                .await;
                if !matches!(r, Ok(Ok(_))) {
                    break;
                }
                let out = state.handle(peer, &buf, Instant::now());
                state.deliver(out, &udp).await;
            }
            state.tcp_peers.lock().unwrap().remove(&peer);
            writer.abort();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn registration_expires() {
        let st = State::new(Config::default());
        let t = Instant::now();
        st.handle(a("1.1.1.1:1"), &Msg::Register { host_id: [1; 16] }.encode(), t);
        assert_eq!(st.registered_hosts(), 1);
        st.sweep(t + Duration::from_secs(30));
        assert_eq!(st.registered_hosts(), 1);
        // lookup after TTL without sweep also misses
        let out = st.handle(a("2.2.2.2:2"), &Msg::Lookup { host_id: [1; 16] }.encode(), t + Duration::from_secs(61));
        assert_eq!(out.len(), 1);
        st.sweep(t + Duration::from_secs(61));
        assert_eq!(st.registered_hosts(), 0);
    }

    #[test]
    fn relay_rejects_third_party() {
        let st = State::new(Config::default());
        let t = Instant::now();
        let j = Msg::RelayJoin { token: [5; 16] }.encode();
        st.handle(a("1.1.1.1:1"), &j, t);
        st.handle(a("2.2.2.2:2"), &j, t);
        let out = st.handle(a("3.3.3.3:3"), &j, t);
        assert_eq!(Msg::decode(&out[0].1).unwrap(), Msg::Error { code: ErrorCode::Full });
        let d = Msg::RelayData { token: [5; 16], payload: vec![1] }.encode();
        let out = st.handle(a("3.3.3.3:3"), &d, t);
        assert_eq!(Msg::decode(&out[0].1).unwrap(), Msg::Error { code: ErrorCode::NotPaired });
    }

    #[test]
    fn bandwidth_cap_drops_excess_and_refills() {
        let cfg = Config { max_bps: 8_000_000, ..Config::default() }; // 1 MB/s, burst 100 KB
        let st = State::new(cfg);
        let t = Instant::now();
        let (x, y) = (a("1.1.1.1:1"), a("2.2.2.2:2"));
        let j = Msg::RelayJoin { token: [9; 16] }.encode();
        st.handle(x, &j, t);
        st.handle(y, &j, t);
        let d = Msg::RelayData { token: [9; 16], payload: vec![0; 1000] }.encode();
        let forwarded = (0..300).filter(|_| !st.handle(x, &d, t).is_empty()).count();
        // burst of ~100 KB at the same instant: roughly 100 packets pass, rest dropped
        assert!((90..=110).contains(&forwarded), "{forwarded}");
        // 50 ms later ~50 KB of tokens are back
        let later = t + Duration::from_millis(50);
        let again = (0..100).filter(|_| !st.handle(x, &d, later).is_empty()).count();
        assert!((40..=60).contains(&again), "{again}");
    }

    #[test]
    fn zero_cap_means_unlimited() {
        let st = State::new(Config { max_bps: 0, ..Config::default() });
        let t = Instant::now();
        let (x, y) = (a("1.1.1.1:1"), a("2.2.2.2:2"));
        let j = Msg::RelayJoin { token: [9; 16] }.encode();
        st.handle(x, &j, t);
        st.handle(y, &j, t);
        let d = Msg::RelayData { token: [9; 16], payload: vec![0; 1400] }.encode();
        assert!((0..2000).all(|_| !st.handle(x, &d, t).is_empty()));
    }
}
