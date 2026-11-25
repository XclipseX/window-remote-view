//! LAN discovery: the host periodically announces itself via UDP broadcast and
//! multicast; clients listen and keep a peer list.
//!
//! TODO: add mDNS/DNS-SD (`_remotedesk._udp.local.`) so Android `NsdManager` and
//! Windows `DnsServiceRegister` can find hosts without this custom beacon.

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use thiserror::Error;
use tokio::net::UdpSocket;

pub const DISCOVERY_PORT: u16 = 47800;
pub const MULTICAST_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 77, 77);
pub const MAGIC: [u8; 4] = *b"RVD1";
pub const MAX_NAME_LEN: usize = 64;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DiscoveryError {
    #[error("bad magic or truncated packet")]
    Malformed,
    #[error("host name too long")]
    NameTooLong,
}

/// Beacon payload: `magic(4) | host_id(16) | port(2 BE) | name_len(1) | name(utf8)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announce {
    pub host_id: [u8; 16],
    pub name: String,
    /// Port the host listens on for the media/session transport.
    pub port: u16,
}

impl Announce {
    pub fn encode(&self) -> Result<Vec<u8>, DiscoveryError> {
        let name = self.name.as_bytes();
        if name.len() > MAX_NAME_LEN {
            return Err(DiscoveryError::NameTooLong);
        }
        let mut v = Vec::with_capacity(23 + name.len());
        v.extend_from_slice(&MAGIC);
        v.extend_from_slice(&self.host_id);
        v.extend_from_slice(&self.port.to_be_bytes());
        v.push(name.len() as u8);
        v.extend_from_slice(name);
        Ok(v)
    }

    pub fn decode(b: &[u8]) -> Result<Announce, DiscoveryError> {
        if b.len() < 23 || b[..4] != MAGIC {
            return Err(DiscoveryError::Malformed);
        }
        let n = b[22] as usize;
        if n > MAX_NAME_LEN || b.len() < 23 + n {
            return Err(DiscoveryError::Malformed);
        }
        let name = std::str::from_utf8(&b[23..23 + n]).map_err(|_| DiscoveryError::Malformed)?;
        let mut host_id = [0u8; 16];
        host_id.copy_from_slice(&b[4..20]);
        Ok(Announce { host_id, name: name.to_string(), port: u16::from_be_bytes([b[20], b[21]]) })
    }
}

#[derive(Debug, Clone)]
pub struct Peer {
    pub announce: Announce,
    /// Source IP of the beacon combined with the announced port.
    pub addr: SocketAddr,
    pub last_seen: Instant,
}

/// Peer list built from received beacons.
#[derive(Debug, Default)]
pub struct PeerList {
    peers: HashMap<[u8; 16], Peer>,
}

impl PeerList {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a received datagram; returns true if it was a valid beacon.
    pub fn handle_packet(&mut self, data: &[u8], from: SocketAddr, now: Instant) -> bool {
        match Announce::decode(data) {
            Ok(a) => {
                let addr = SocketAddr::new(from.ip(), a.port);
                self.peers.insert(a.host_id, Peer { announce: a, addr, last_seen: now });
                true
            }
            Err(_) => false,
        }
    }

    pub fn prune(&mut self, now: Instant, ttl: Duration) {
        self.peers.retain(|_, p| now.saturating_duration_since(p.last_seen) <= ttl);
    }

    pub fn peers(&self) -> Vec<Peer> {
        let mut v: Vec<_> = self.peers.values().cloned().collect();
        v.sort_by(|a, b| a.announce.name.cmp(&b.announce.name));
        v
    }
}

/// Bind a reusable listening socket on `port` joined to the multicast group.
pub fn bind_listener(port: u16) -> io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    s.set_reuse_port(true)?;
    s.set_nonblocking(true)?;
    s.bind(&SocketAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).into())?;
    // Multicast is best-effort: broadcast still works if the join fails.
    if let Err(e) = s.join_multicast_v4(&MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED) {
        tracing::debug!("multicast join failed: {e}");
    }
    UdpSocket::from_std(s.into())
}

/// Host side: send beacons forever every `interval` to broadcast + multicast.
pub async fn run_beacon(announce: Announce, interval: Duration, port: u16) -> io::Result<()> {
    let payload = announce.encode().map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
    sock.set_broadcast(true)?;
    let targets = [
        SocketAddr::from((Ipv4Addr::BROADCAST, port)),
        SocketAddr::from((MULTICAST_GROUP, port)),
    ];
    let mut tick = tokio::time::interval(interval);
    loop {
        tick.tick().await;
        for t in targets {
            if let Err(e) = sock.send_to(&payload, t).await {
                tracing::debug!("beacon to {t} failed: {e}");
            }
        }
    }
}

/// Client side: receive beacons into `list` until the socket errors.
pub async fn run_listener(
    sock: &UdpSocket,
    list: &std::sync::Mutex<PeerList>,
) -> io::Result<()> {
    let mut buf = [0u8; 256];
    loop {
        let (n, from) = sock.recv_from(&mut buf).await?;
        list.lock().unwrap().handle_packet(&buf[..n], from, Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann() -> Announce {
        Announce { host_id: [7; 16], name: "DESKTOP-ÄB".into(), port: 47900 }
    }

    #[test]
    fn roundtrip() {
        let b = ann().encode().unwrap();
        assert_eq!(Announce::decode(&b).unwrap(), ann());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Announce::decode(b"nope").is_err());
        let mut b = ann().encode().unwrap();
        b[0] = b'X';
        assert!(Announce::decode(&b).is_err());
        let mut b = ann().encode().unwrap();
        b.truncate(b.len() - 2);
        assert!(Announce::decode(&b).is_err());
        let long = Announce { name: "x".repeat(65), ..ann() };
        assert_eq!(long.encode(), Err(DiscoveryError::NameTooLong));
    }

    #[test]
    fn peer_list_updates_and_prunes() {
        let mut l = PeerList::new();
        let t = Instant::now();
        let from: SocketAddr = "192.168.1.5:5555".parse().unwrap();
        assert!(l.handle_packet(&ann().encode().unwrap(), from, t));
        assert!(!l.handle_packet(b"junk", from, t));
        assert!(l.handle_packet(&ann().encode().unwrap(), from, t));
        let p = l.peers();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].addr, "192.168.1.5:47900".parse().unwrap());
        l.prune(t + Duration::from_secs(10), Duration::from_secs(5));
        assert!(l.peers().is_empty());
    }
}
