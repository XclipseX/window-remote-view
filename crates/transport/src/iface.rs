//! Local network interface enumeration and classification, plus per-link tuning profiles.
//!
//! Classification is heuristic (interface name, then address ranges). USB tethering is
//! treated as the top-priority LAN path: it is a point-to-point wired link to the phone.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::path::PathKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IfaceKind {
    Wifi,
    Ethernet,
    UsbTether,
    Cellular,
    Loopback,
    Other,
}

/// Address scope, mostly relevant for IPv6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddrClass {
    Loopback,
    LinkLocal,
    /// RFC 1918 / ULA (fc00::/7).
    Private,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalIface {
    pub name: String,
    pub ip: IpAddr,
    pub kind: IfaceKind,
    pub class: AddrClass,
    /// OS interface index (IPv6 scope id for link-local addresses), when known.
    pub index: Option<u32>,
}

impl LocalIface {
    /// Lower is better. USB tether first, then wired, Wi-Fi, other, cellular, loopback.
    pub fn priority(&self) -> u8 {
        kind_priority(self.kind)
    }

    /// The race tier of a path over this interface (everything local is a LAN path).
    pub fn path_kind(&self) -> PathKind {
        PathKind::LanDirect
    }
}

pub fn kind_priority(k: IfaceKind) -> u8 {
    match k {
        IfaceKind::UsbTether => 0,
        IfaceKind::Ethernet => 1,
        IfaceKind::Wifi => 2,
        IfaceKind::Other => 3,
        IfaceKind::Cellular => 4,
        IfaceKind::Loopback => 5,
    }
}

pub fn addr_class(ip: IpAddr) -> AddrClass {
    match ip {
        IpAddr::V4(v) => {
            if v.is_loopback() {
                AddrClass::Loopback
            } else if v.is_link_local() {
                AddrClass::LinkLocal
            } else if v.is_private() {
                AddrClass::Private
            } else {
                AddrClass::Global
            }
        }
        IpAddr::V6(v) => {
            let s0 = v.segments()[0];
            if v.is_loopback() {
                AddrClass::Loopback
            } else if s0 & 0xffc0 == 0xfe80 {
                AddrClass::LinkLocal
            } else if s0 & 0xfe00 == 0xfc00 {
                AddrClass::Private
            } else {
                AddrClass::Global
            }
        }
    }
}

fn in_v4_24(ip: Ipv4Addr, a: u8, b: u8, c: u8) -> bool {
    ip.octets()[..3] == [a, b, c]
}

/// Classify one interface from its name (Linux/Android/macOS names or Windows friendly
/// names/descriptions) and address.
pub fn classify(name: &str, ip: IpAddr) -> IfaceKind {
    let n = name.to_ascii_lowercase();
    let has = |pats: &[&str]| pats.iter().any(|p| n.contains(p));
    let starts = |pats: &[&str]| pats.iter().any(|p| n.starts_with(p));

    if ip.is_loopback() || starts(&["lo"]) && n.len() <= 3 || n.contains("loopback") {
        return IfaceKind::Loopback;
    }
    // Android USB tethering: rndis0 / usb0; Linux USB NICs get MAC-based enx* names;
    // Windows shows "Remote NDIS based Internet Sharing Device" or "Android".
    if starts(&["rndis", "usb", "enx"]) || has(&["remote ndis", "rndis", "android", "usb tether"]) {
        return IfaceKind::UsbTether;
    }
    if starts(&["rmnet", "ccmni", "wwan", "pdp", "ppp"]) || has(&["cellular", "mobile broadband", "mobile data"]) {
        return IfaceKind::Cellular;
    }
    if starts(&["wlan", "wl", "wifi", "ath", "p2p-wlan"]) || has(&["wi-fi", "wifi", "wireless", "wlan", "802.11"]) {
        return IfaceKind::Wifi;
    }
    // Name is unspecific or wired-looking: let well-known subnets decide.
    if let IpAddr::V4(v4) = ip {
        if in_v4_24(v4, 192, 168, 42) {
            return IfaceKind::UsbTether; // Android USB tethering default subnet
        }
        // 192.168.49.x: Wi-Fi Direct group; 192.168.43.x: Android hotspot.
        if in_v4_24(v4, 192, 168, 49) || in_v4_24(v4, 192, 168, 43) {
            return IfaceKind::Wifi;
        }
    }
    if starts(&["eth", "en", "em", "eno", "ens", "enp", "bond", "br"]) || has(&["ethernet", "lan"]) {
        return IfaceKind::Ethernet;
    }
    IfaceKind::Other
}

/// Build `LocalIface`s from `(name, ip, index)` triples, best interface first.
/// Pure so it can be tested without touching the OS.
pub fn classify_all(raw: &[(String, IpAddr, Option<u32>)]) -> Vec<LocalIface> {
    let mut v: Vec<LocalIface> = raw
        .iter()
        .map(|(name, ip, index)| LocalIface {
            name: name.clone(),
            ip: *ip,
            kind: classify(name, *ip),
            class: addr_class(*ip),
            index: *index,
        })
        .collect();
    // Stable sort keeps OS order among equals; within a kind prefer IPv6-global-free
    // ordering: private/link-local LAN addresses before global ones.
    v.sort_by_key(|i| (i.priority(), i.ip.is_ipv6() as u8));
    v
}

/// Enumerate all local interface addresses (IPv4 + IPv6 incl. link-local), best first.
pub fn enumerate() -> std::io::Result<Vec<LocalIface>> {
    let raw: Vec<(String, IpAddr, Option<u32>)> =
        if_addrs::get_if_addrs()?.into_iter().map(|i| (i.name.clone(), i.ip(), i.index)).collect();
    Ok(classify_all(&raw))
}

/// Interfaces usable for a LAN race (non-loopback, non-cellular), best first.
pub fn lan_capable(ifaces: &[LocalIface]) -> Vec<&LocalIface> {
    ifaces.iter().filter(|i| !matches!(i.kind, IfaceKind::Loopback | IfaceKind::Cellular)).collect()
}

/// True if a USB tether link is present (caller should race it first).
pub fn has_usb_tether(ifaces: &[LocalIface]) -> bool {
    ifaces.iter().any(|i| i.kind == IfaceKind::UsbTether)
}

/// Recommended stream parameters for a link tier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkProfile {
    pub bitrate_mbps: (u32, u32),
    pub fec_percent: (u8, u8),
    pub jitter_ms: (u32, u32),
    pub fps: u32,
}

/// Research table: LAN/USB 50-150 Mbps, FEC 0-5 %, jitter 0-2 ms; P2P 15-25 Mbps,
/// FEC 10-15 %, jitter 4-8 ms; Relay 8-15 Mbps, FEC 15-20 %, jitter 8-15 ms.
pub fn link_profile(kind: PathKind) -> LinkProfile {
    match kind {
        PathKind::LanDirect => LinkProfile { bitrate_mbps: (50, 150), fec_percent: (0, 5), jitter_ms: (0, 2), fps: 120 },
        PathKind::P2p => LinkProfile { bitrate_mbps: (15, 25), fec_percent: (10, 15), jitter_ms: (4, 8), fps: 60 },
        PathKind::Relay => LinkProfile { bitrate_mbps: (8, 15), fec_percent: (15, 20), jitter_ms: (8, 15), fps: 30 },
    }
}

/// Profile for a local interface; cellular LAN-ish links are treated like P2P.
pub fn link_profile_for_iface(kind: IfaceKind) -> LinkProfile {
    match kind {
        IfaceKind::Cellular => link_profile(PathKind::P2p),
        _ => link_profile(PathKind::LanDirect),
    }
}

/// Keep `Ipv6Addr` import used for doc/tests helpers.
pub fn is_v6_link_local(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn classifies_names() {
        assert_eq!(classify("lo", v4("127.0.0.1")), IfaceKind::Loopback);
        assert_eq!(classify("rndis0", v4("192.168.42.129")), IfaceKind::UsbTether);
        assert_eq!(classify("enx0a1b2c3d4e5f", v4("10.0.0.2")), IfaceKind::UsbTether);
        assert_eq!(classify("Ethernet 3 (Remote NDIS based Internet Sharing Device)", v4("10.1.1.1")), IfaceKind::UsbTether);
        assert_eq!(classify("wlan0", v4("192.168.1.5")), IfaceKind::Wifi);
        assert_eq!(classify("Wi-Fi", v4("10.0.0.5")), IfaceKind::Wifi);
        assert_eq!(classify("rmnet_data0", v4("100.64.1.1")), IfaceKind::Cellular);
        assert_eq!(classify("eth0", v4("192.168.1.9")), IfaceKind::Ethernet);
        assert_eq!(classify("enp3s0", v4("10.0.0.9")), IfaceKind::Ethernet);
        assert_eq!(classify("tun7", v4("10.8.0.2")), IfaceKind::Other);
    }

    #[test]
    fn classifies_by_subnet_when_name_is_generic() {
        assert_eq!(classify("Ethernet 5", v4("192.168.42.100")), IfaceKind::UsbTether);
        assert_eq!(classify("eth1", v4("192.168.42.7")), IfaceKind::UsbTether);
        assert_eq!(classify("p2p0x", v4("192.168.49.1")), IfaceKind::Wifi);
        assert_eq!(classify("net1", v4("192.168.43.20")), IfaceKind::Wifi);
        // an explicit Wi-Fi name wins over the subnet
        assert_eq!(classify("wlan0", v4("192.168.42.5")), IfaceKind::Wifi);
    }

    #[test]
    fn ipv6_classes() {
        assert_eq!(addr_class("fe80::1".parse().unwrap()), AddrClass::LinkLocal);
        assert_eq!(addr_class("fd00::1".parse().unwrap()), AddrClass::Private);
        assert_eq!(addr_class("2001:db8::1".parse().unwrap()), AddrClass::Global);
        assert_eq!(addr_class("::1".parse().unwrap()), AddrClass::Loopback);
        assert_eq!(classify("wlan0", "fe80::abcd".parse().unwrap()), IfaceKind::Wifi);
        assert!(is_v6_link_local(&"fe80::1".parse().unwrap()));
    }

    #[test]
    fn usb_tether_is_top_priority() {
        let raw = vec![
            ("wlan0".to_string(), v4("192.168.1.5"), Some(3)),
            ("lo".to_string(), v4("127.0.0.1"), Some(1)),
            ("eth0".to_string(), v4("10.0.0.2"), Some(2)),
            ("rndis0".to_string(), v4("192.168.42.129"), Some(4)),
            ("rmnet0".to_string(), v4("100.64.0.9"), None),
        ];
        let l = classify_all(&raw);
        assert_eq!(l[0].kind, IfaceKind::UsbTether);
        assert_eq!(l[1].kind, IfaceKind::Ethernet);
        assert_eq!(l[2].kind, IfaceKind::Wifi);
        assert_eq!(l.last().unwrap().kind, IfaceKind::Loopback);
        assert!(has_usb_tether(&l));
        assert_eq!(lan_capable(&l).len(), 3);
    }

    #[test]
    fn profiles_match_research_table() {
        let l = link_profile(PathKind::LanDirect);
        assert_eq!((l.bitrate_mbps, l.fec_percent, l.jitter_ms), ((50, 150), (0, 5), (0, 2)));
        let p = link_profile(PathKind::P2p);
        assert_eq!((p.bitrate_mbps, p.fec_percent, p.jitter_ms), ((15, 25), (10, 15), (4, 8)));
        let r = link_profile(PathKind::Relay);
        assert_eq!((r.bitrate_mbps, r.fec_percent, r.jitter_ms), ((8, 15), (15, 20), (8, 15)));
        assert_eq!(link_profile_for_iface(IfaceKind::UsbTether), l);
    }

    #[test]
    fn enumerate_runs_and_finds_loopback() {
        let l = enumerate().unwrap();
        assert!(l.iter().any(|i| i.kind == IfaceKind::Loopback));
    }
}
