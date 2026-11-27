//! LAN UDP transport and beacon discovery for the Android client core.
//!
//! * [`parse_target`]: parses `"ip:port"`, bare `"ip"` (default port `47800`),
//!   and IPv6 (`"[::1]:47800"`, `"[::1]"`, `"::1"`).
//! * [`UdpLinkIo`] + [`spawn_udp_session`]: real UDP session I/O paired with a
//!   background `"rv-udp-recv"` reader thread.
//! * [`DiscoveryHandle`]: background `"rv-lan-disc"` listener decoding
//!   [`rv_transport::discovery::Announce`] beacons with deduplication.

use std::collections::HashMap;
use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use rv_session::DatagramIo;
use rv_transport::discovery::{Announce, DISCOVERY_PORT, MULTICAST_GROUP};
use socket2::{Domain, Protocol, Socket, Type};

use crate::glue::{EventSink, HelloConfig};
use crate::worker::{self, Command, WorkerHandle};

/// Default host UDP port when a target string omits `:port`.
pub const DEFAULT_LAN_PORT: u16 = DISCOVERY_PORT;

/// Read timeout on the LAN session UDP socket so shutdown stops cleanly.
const UDP_RECV_TIMEOUT: Duration = Duration::from_millis(100);
/// Best-effort kernel receive buffer size for high-bitrate video bursts.
const UDP_RCVBUF_BYTES: usize = 4 * 1024 * 1024;
/// Maximum UDP datagram payload size.
const MAX_UDP_DATAGRAM: usize = 65_536;
/// Read timeout on the LAN discovery socket so `nativeStopDiscovery` exits cleanly.
const DISCOVERY_RECV_TIMEOUT: Duration = Duration::from_millis(250);

/// Parse a LAN target string (`"ip:port"`, bare `"ip"`, `"[ipv6]:port"`,
/// `"[ipv6]"`, or bare `"ipv6"`). Bare addresses default to port `47800`.
pub fn parse_target(raw: &str) -> Result<SocketAddr, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("empty target address".into());
    }
    if let Ok(addr) = s.parse::<SocketAddr>() {
        if addr.port() == 0 {
            return Err(format!("invalid target port 0 in `{s}`"));
        }
        return Ok(addr);
    }
    if let Ok(ip) = s.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_LAN_PORT));
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
        if let Ok(ip) = inner.parse::<Ipv6Addr>() {
            return Ok(SocketAddr::new(IpAddr::V6(ip), DEFAULT_LAN_PORT));
        }
    }
    Err(format!("invalid LAN target `{s}` (expected ip or ip:port)"))
}

/// `DatagramIo` sending over a connected `std::net::UdpSocket`.
pub struct UdpLinkIo {
    sock: UdpSocket,
}

impl UdpLinkIo {
    pub fn new(sock: UdpSocket) -> Self {
        Self { sock }
    }
}

impl DatagramIo for UdpLinkIo {
    fn send(&mut self, buf: &[u8]) {
        if let Err(e) = self.sock.send(buf) {
            log::debug!("udp send failed: {e}");
        }
    }
}

/// Owns the background `"rv-udp-recv"` thread for a LAN session.
pub struct UdpRecvGuard {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl UdpRecvGuard {
    /// Signal the receiver thread to stop without blocking.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for UdpRecvGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn bind_connected_udp(peer: SocketAddr) -> io::Result<UdpSocket> {
    let (domain, bind_addr): (Domain, SocketAddr) = if peer.is_ipv6() {
        (Domain::IPV6, SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 0, 0, 0)))
    } else {
        (Domain::IPV4, SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)))
    };
    let s = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = s.set_recv_buffer_size(UDP_RCVBUF_BYTES);
    s.bind(&bind_addr.into())?;
    let sock: UdpSocket = s.into();
    sock.set_read_timeout(Some(UDP_RECV_TIMEOUT))?;
    sock.connect(peer)?;
    Ok(sock)
}

/// Create a connected UDP socket, start the session worker, and spawn the
/// background `"rv-udp-recv"` thread that forwards incoming packets to it.
pub fn spawn_udp_session<S>(
    target: &str,
    cfg: HelloConfig,
    sink: S,
) -> Result<(WorkerHandle, UdpRecvGuard), String>
where
    S: EventSink + Send + 'static,
{
    let peer = parse_target(target)?;
    let sock = bind_connected_udp(peer).map_err(|e| format!("udp connect {peer}: {e}"))?;
    let recv_sock = sock.try_clone().map_err(|e| format!("udp clone: {e}"))?;

    let worker = worker::spawn(UdpLinkIo::new(sock), sink, cfg)?;
    let tx = worker.sender();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let join = std::thread::Builder::new()
        .name("rv-udp-recv".into())
        .spawn(move || {
            let mut buf = vec![0u8; MAX_UDP_DATAGRAM];
            while !stop_flag.load(Ordering::Relaxed) {
                match recv_sock.recv(&mut buf) {
                    Ok(0) => continue,
                    Ok(n) => {
                        if tx.send(Command::Datagram(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            ErrorKind::WouldBlock
                                | ErrorKind::TimedOut
                                | ErrorKind::Interrupted
                                | ErrorKind::ConnectionRefused
                                | ErrorKind::ConnectionReset
                        ) =>
                    {
                        continue;
                    }
                    Err(e) => {
                        if !stop_flag.load(Ordering::Relaxed) {
                            log::warn!("rv-udp-recv exited on socket error: {e}");
                        }
                        break;
                    }
                }
            }
        })
        .map_err(|e| format!("spawn rv-udp-recv: {e}"))?;

    Ok((worker, UdpRecvGuard { stop, join: Some(join) }))
}

fn bind_discovery_socket(port: u16) -> io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_reuse_address(true)?;
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    {
        let _ = s.set_reuse_port(true);
    }
    s.bind(&SocketAddr::from(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port)).into())?;
    // Multicast is best-effort: broadcast beacons still work if joining fails.
    if let Err(e) = s.join_multicast_v4(&MULTICAST_GROUP, &Ipv4Addr::UNSPECIFIED) {
        log::debug!("lan discovery multicast join failed: {e}");
    }
    let sock: UdpSocket = s.into();
    sock.set_read_timeout(Some(DISCOVERY_RECV_TIMEOUT))?;
    Ok(sock)
}

/// Background LAN beacon listener (`"rv-lan-disc"`).
pub struct DiscoveryHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    local_addr: SocketAddr,
}

impl DiscoveryHandle {
    /// Start listening on [`DISCOVERY_PORT`] (`47800`).
    pub fn start<F>(on_discovered: F) -> io::Result<Self>
    where
        F: FnMut(&str, &str) + Send + 'static,
    {
        Self::start_on_port(DISCOVERY_PORT, on_discovered)
    }

    /// Start listening on `port` (pass `0` in tests for an ephemeral port).
    pub fn start_on_port<F>(port: u16, mut on_discovered: F) -> io::Result<Self>
    where
        F: FnMut(&str, &str) + Send + 'static,
    {
        let sock = bind_discovery_socket(port)?;
        let local_addr = sock.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);

        let join = std::thread::Builder::new()
            .name("rv-lan-disc".into())
            .spawn(move || {
                let mut seen: HashMap<[u8; 16], (String, String)> = HashMap::new();
                let mut buf = [0u8; 512];
                while !stop_flag.load(Ordering::Relaxed) {
                    match sock.recv_from(&mut buf) {
                        Ok((n, from)) => {
                            if let Ok(announce) = Announce::decode(&buf[..n]) {
                                let addr = SocketAddr::new(from.ip(), announce.port).to_string();
                                let changed = seen
                                    .get(&announce.host_id)
                                    .map(|(n, a)| n != &announce.name || a != &addr)
                                    .unwrap_or(true);
                                if changed {
                                    on_discovered(&announce.name, &addr);
                                    seen.insert(announce.host_id, (announce.name, addr));
                                }
                            }
                        }
                        Err(e)
                            if matches!(
                                e.kind(),
                                ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                            ) =>
                        {
                            continue;
                        }
                        Err(e) => {
                            if !stop_flag.load(Ordering::Relaxed) {
                                log::warn!("rv-lan-disc stopped on error: {e}");
                            }
                            break;
                        }
                    }
                }
            })?;

        Ok(Self { stop, join: Some(join), local_addr })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for DiscoveryHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glue::{state, Outgoing, UiInput};
    use rv_crypto::Keypair;
    use rv_proto::ControlMessage;
    use rv_session::{HostEvent, HostSession, USB_HOST_ID};
    use std::sync::Mutex;
    use std::time::Instant;

    #[test]
    fn parse_target_variants() {
        assert_eq!(parse_target("192.168.1.42:47900").unwrap(), "192.168.1.42:47900".parse::<SocketAddr>().unwrap());
        assert_eq!(parse_target(" 192.168.1.42 ").unwrap(), "192.168.1.42:47800".parse::<SocketAddr>().unwrap());
        assert_eq!(parse_target("[::1]:47901").unwrap(), "[::1]:47901".parse::<SocketAddr>().unwrap());
        assert_eq!(parse_target("[::1]").unwrap(), "[::1]:47800".parse::<SocketAddr>().unwrap());
        assert_eq!(parse_target("::1").unwrap(), "[::1]:47800".parse::<SocketAddr>().unwrap());
        assert!(parse_target("").is_err());
        assert!(parse_target("   ").is_err());
        assert!(parse_target("192.168.1.1:0").is_err());
        assert!(parse_target("not-a-host").is_err());
    }

    #[derive(Clone, Default)]
    struct Rec(Arc<Mutex<Vec<Outgoing>>>);
    impl EventSink for Rec {
        fn video(&mut self, d: &[u8], p: i64, f: i32) {
            self.0.lock().unwrap().push(Outgoing::Video { data: d.to_vec(), pts_us: p, flags: f });
        }
        fn audio(&mut self, d: &[u8], p: i64, c: i32) {
            self.0.lock().unwrap().push(Outgoing::Audio { data: d.to_vec(), pts_us: p, codec: c });
        }
        fn state(&mut self, s: i32, m: Option<&str>) {
            self.0.lock().unwrap().push(Outgoing::State { state: s, message: m.map(str::to_string) });
        }
        fn stats(&mut self, _: f32, _: i32, _: f32) {}
    }

    struct HostUdpIo {
        sock: UdpSocket,
        peer: Arc<Mutex<Option<SocketAddr>>>,
    }
    impl DatagramIo for HostUdpIo {
        fn send(&mut self, buf: &[u8]) {
            if let Some(p) = *self.peer.lock().unwrap() {
                let _ = self.sock.send_to(buf, p);
            }
        }
    }

    #[test]
    fn udp_loopback_end_to_end_with_host_session() {
        let host_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        host_sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let host_addr = host_sock.local_addr().unwrap();
        let peer_slot = Arc::new(Mutex::new(None));

        let mut host = HostSession::new(
            HostUdpIo { sock: host_sock.try_clone().unwrap(), peer: Arc::clone(&peer_slot) },
            Keypair::generate().unwrap(),
            "654321",
            USB_HOST_ID,
        )
        .unwrap();

        let rec = Rec::default();
        let cfg = HelloConfig {
            otp: "654321".into(),
            width: 1920,
            height: 1080,
            refresh_hz: 60,
            codecs: rv_proto::codec_mask::H264,
            device_name: "Pixel".into(),
        };
        let (mut worker, recv_guard) = spawn_udp_session(&host_addr.to_string(), cfg, rec.clone()).unwrap();

        let frame: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let mut got_hello = false;
        let mut got_touch = false;
        let mut buf = vec![0u8; MAX_UDP_DATAGRAM];
        let deadline = Instant::now() + Duration::from_secs(15);

        let client_got_video = |rec: &Rec| {
            let g = rec.0.lock().unwrap();
            g.iter().any(|o| matches!(o, Outgoing::Video { .. }))
                && g.iter().any(|o| matches!(o, Outgoing::State { state: state::DISPLAY_CONFIG, .. }))
        };

        while Instant::now() < deadline && !(client_got_video(&rec) && got_hello && got_touch) {
            if let Ok((n, from)) = host_sock.recv_from(&mut buf) {
                *peer_slot.lock().unwrap() = Some(from);
                let _ = host.handle_datagram(&buf[..n]);
            }
            while let Some(ev) = host.poll_event() {
                match ev {
                    HostEvent::Established { .. } => {
                        host.send_control(ControlMessage::DisplayConfigChange { w: 1920, h: 1080, fps: 60 })
                            .unwrap();
                        host.send_video_frame(1, 5000, &frame, true).unwrap();
                        worker.send(Command::Input(UiInput::Touch {
                            id: 1,
                            x: 500,
                            y: 600,
                            phase: 0,
                            pressure: 512,
                        }));
                    }
                    HostEvent::Control(ControlMessage::Hello { max_w, max_h, max_fps, .. }) => {
                        assert_eq!((max_w, max_h, max_fps), (1920, 1080, 60));
                        got_hello = true;
                    }
                    HostEvent::Input { event, .. } => {
                        assert!(matches!(event, rv_proto::InputEvent::Touch { .. }));
                        got_touch = true;
                    }
                    _ => {}
                }
            }
        }

        assert!(got_hello && got_touch && client_got_video(&rec));
        recv_guard.stop();
        worker.shutdown();
        drop(recv_guard);

        let g = rec.0.lock().unwrap();
        assert!(g.contains(&Outgoing::Video { data: frame, pts_us: 5000, flags: 1 }));
        assert_eq!(g.last(), Some(&Outgoing::State { state: state::CLOSED, message: None }));
    }

    #[test]
    fn discovery_receives_and_deduplicates_beacons() {
        let found: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let found_cb = Arc::clone(&found);
        let mut disc = DiscoveryHandle::start_on_port(0, move |name, addr| {
            found_cb.lock().unwrap().push((name.to_string(), addr.to_string()));
        })
        .unwrap();

        let target = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), disc.local_addr().port());
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();

        let b1 = Announce { host_id: [1; 16], name: "WORKSTATION".into(), port: 47800 }.encode().unwrap();
        let b1_updated = Announce { host_id: [1; 16], name: "WORKSTATION".into(), port: 47801 }.encode().unwrap();
        let b2 = Announce { host_id: [2; 16], name: "LAPTOP".into(), port: 47900 }.encode().unwrap();

        // Send duplicate b1, garbage, updated b1, and distinct b2.
        sender.send_to(&b1, target).unwrap();
        sender.send_to(&b1, target).unwrap();
        sender.send_to(b"garbage-packet", target).unwrap();
        sender.send_to(&b1_updated, target).unwrap();
        sender.send_to(&b2, target).unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && found.lock().unwrap().len() < 3 {
            std::thread::sleep(Duration::from_millis(10));
        }

        disc.stop();
        let items = found.lock().unwrap().clone();
        assert_eq!(
            items,
            vec![
                ("WORKSTATION".to_string(), "127.0.0.1:47800".to_string()),
                ("WORKSTATION".to_string(), "127.0.0.1:47801".to_string()),
                ("LAPTOP".to_string(), "127.0.0.1:47900".to_string()),
            ]
        );
    }
}
