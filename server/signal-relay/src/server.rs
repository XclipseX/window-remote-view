//! Server state machine (pure: datagram in -> datagrams out) plus the tokio UDP loop.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;

use crate::proto::{ErrorCode, Id, Msg};

#[derive(Debug, Clone)]
pub struct Config {
    /// Registration lifetime without refresh (hosts refresh every 20 s).
    pub register_ttl: Duration,
    /// Relay session lifetime without traffic.
    pub session_idle: Duration,
    pub max_registrations: usize,
    pub max_sessions: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            register_ttl: Duration::from_secs(60),
            session_idle: Duration::from_secs(60),
            max_registrations: 100_000,
            max_sessions: 10_000,
        }
    }
}

struct Session {
    a: SocketAddr,
    b: Option<SocketAddr>,
    last: Instant,
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
}

impl State {
    pub fn new(cfg: Config) -> Self {
        Self { cfg, inner: Mutex::new(Inner::default()) }
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
                let s = g.sessions.entry(token).or_insert(Session { a: from, b: None, last: now });
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
                for (to, bytes) in state.handle(from, &buf[..n], Instant::now()) {
                    let _ = sock.send_to(&bytes, to).await;
                }
            }
            // Windows reports ICMP port-unreachable as recv errors; keep serving.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(e) => break Err(e),
        }
    };
    sweeper.abort();
    res
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
}
