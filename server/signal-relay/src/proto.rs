//! Versioned binary wire format for the signal/relay server.
//!
//! Every message: `'R' 'S' | version(1) | type(1) | body`. Addresses are encoded as
//! `family(4|6) | ip(4|16) | port(2 BE)`. The relay never interprets relay payloads.
//! (The server additionally answers `rv-transport` path probes, see `server.rs`.)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use thiserror::Error;

pub const MAGIC: [u8; 2] = *b"RS";
pub const VERSION: u8 = 1;
pub const ID_LEN: usize = 16;
pub type Id = [u8; ID_LEN];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtoError {
    #[error("malformed message")]
    Malformed,
    #[error("unsupported version {0}")]
    BadVersion(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NotFound = 1,
    NotPaired = 2,
    Full = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    /// Host -> server: register/refresh `host_id` at the observed source address.
    Register { host_id: Id },
    /// Server -> host.
    RegisterAck { observed: SocketAddr },
    /// Client -> server: look up a host.
    Lookup { host_id: Id },
    /// Server -> client.
    LookupResp { host_id: Id, addr: Option<SocketAddr> },
    /// Server -> host: a client is looking for you; punch toward `client`.
    PeerNotify { client: SocketAddr },
    /// Either side -> server: join relay session `token` (pairs the first two joiners).
    RelayJoin { token: Id },
    /// Server -> joiner.
    RelayJoinAck { token: Id, paired: bool },
    /// Opaque datagram for the relay; the server forwards `payload` to the other endpoint.
    RelayData { token: Id, payload: Vec<u8> },
    /// Server -> sender.
    Error { code: ErrorCode },
}

const T_REGISTER: u8 = 0x01;
const T_LOOKUP: u8 = 0x02;
const T_RELAY_JOIN: u8 = 0x03;
const T_RELAY_DATA: u8 = 0x04;
const T_REGISTER_ACK: u8 = 0x81;
const T_LOOKUP_RESP: u8 = 0x82;
const T_PEER_NOTIFY: u8 = 0x83;
const T_RELAY_JOIN_ACK: u8 = 0x84;
const T_ERROR: u8 = 0xFF;

fn put_addr(v: &mut Vec<u8>, a: &SocketAddr) {
    match a.ip() {
        IpAddr::V4(ip) => {
            v.push(4);
            v.extend_from_slice(&ip.octets());
        }
        IpAddr::V6(ip) => {
            v.push(6);
            v.extend_from_slice(&ip.octets());
        }
    }
    v.extend_from_slice(&a.port().to_be_bytes());
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], ProtoError> {
        if self.0.len() < n {
            return Err(ProtoError::Malformed);
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> Result<u8, ProtoError> {
        Ok(self.take(1)?[0])
    }
    fn id(&mut self) -> Result<Id, ProtoError> {
        Ok(self.take(ID_LEN)?.try_into().unwrap())
    }
    fn addr(&mut self) -> Result<SocketAddr, ProtoError> {
        let ip = match self.u8()? {
            4 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(self.take(4)?).unwrap())),
            6 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(self.take(16)?).unwrap())),
            _ => return Err(ProtoError::Malformed),
        };
        let p = self.take(2)?;
        Ok(SocketAddr::new(ip, u16::from_be_bytes([p[0], p[1]])))
    }
    fn rest(self) -> &'a [u8] {
        self.0
    }
}

impl Msg {
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![MAGIC[0], MAGIC[1], VERSION];
        match self {
            Msg::Register { host_id } => {
                v.push(T_REGISTER);
                v.extend_from_slice(host_id);
            }
            Msg::RegisterAck { observed } => {
                v.push(T_REGISTER_ACK);
                put_addr(&mut v, observed);
            }
            Msg::Lookup { host_id } => {
                v.push(T_LOOKUP);
                v.extend_from_slice(host_id);
            }
            Msg::LookupResp { host_id, addr } => {
                v.push(T_LOOKUP_RESP);
                v.extend_from_slice(host_id);
                match addr {
                    Some(a) => {
                        v.push(1);
                        put_addr(&mut v, a);
                    }
                    None => v.push(0),
                }
            }
            Msg::PeerNotify { client } => {
                v.push(T_PEER_NOTIFY);
                put_addr(&mut v, client);
            }
            Msg::RelayJoin { token } => {
                v.push(T_RELAY_JOIN);
                v.extend_from_slice(token);
            }
            Msg::RelayJoinAck { token, paired } => {
                v.push(T_RELAY_JOIN_ACK);
                v.extend_from_slice(token);
                v.push(*paired as u8);
            }
            Msg::RelayData { token, payload } => {
                v.push(T_RELAY_DATA);
                v.extend_from_slice(token);
                v.extend_from_slice(payload);
            }
            Msg::Error { code } => {
                v.push(T_ERROR);
                v.push(*code as u8);
            }
        }
        v
    }

    pub fn decode(b: &[u8]) -> Result<Msg, ProtoError> {
        let mut r = Reader(b);
        if r.take(2)? != MAGIC {
            return Err(ProtoError::Malformed);
        }
        let ver = r.u8()?;
        if ver != VERSION {
            return Err(ProtoError::BadVersion(ver));
        }
        Ok(match r.u8()? {
            T_REGISTER => Msg::Register { host_id: r.id()? },
            T_REGISTER_ACK => Msg::RegisterAck { observed: r.addr()? },
            T_LOOKUP => Msg::Lookup { host_id: r.id()? },
            T_LOOKUP_RESP => {
                let host_id = r.id()?;
                let addr = if r.u8()? == 1 { Some(r.addr()?) } else { None };
                Msg::LookupResp { host_id, addr }
            }
            T_PEER_NOTIFY => Msg::PeerNotify { client: r.addr()? },
            T_RELAY_JOIN => Msg::RelayJoin { token: r.id()? },
            T_RELAY_JOIN_ACK => Msg::RelayJoinAck { token: r.id()?, paired: r.u8()? == 1 },
            T_RELAY_DATA => {
                let token = r.id()?;
                Msg::RelayData { token, payload: r.rest().to_vec() }
            }
            T_ERROR => Msg::Error {
                code: match r.u8()? {
                    1 => ErrorCode::NotFound,
                    2 => ErrorCode::NotPaired,
                    3 => ErrorCode::Full,
                    _ => return Err(ProtoError::Malformed),
                },
            },
            _ => return Err(ProtoError::Malformed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all() {
        let a4: SocketAddr = "1.2.3.4:5".parse().unwrap();
        let a6: SocketAddr = "[2001:db8::1]:443".parse().unwrap();
        let msgs = vec![
            Msg::Register { host_id: [1; 16] },
            Msg::RegisterAck { observed: a4 },
            Msg::Lookup { host_id: [2; 16] },
            Msg::LookupResp { host_id: [2; 16], addr: Some(a6) },
            Msg::LookupResp { host_id: [2; 16], addr: None },
            Msg::PeerNotify { client: a6 },
            Msg::RelayJoin { token: [3; 16] },
            Msg::RelayJoinAck { token: [3; 16], paired: true },
            Msg::RelayData { token: [3; 16], payload: vec![9, 8, 7] },
            Msg::Error { code: ErrorCode::NotPaired },
        ];
        for m in msgs {
            assert_eq!(Msg::decode(&m.encode()).unwrap(), m);
        }
    }

    #[test]
    fn rejects_bad() {
        assert!(Msg::decode(b"").is_err());
        assert!(Msg::decode(b"XX\x01\x01").is_err());
        assert_eq!(Msg::decode(&[b'R', b'S', 9, 1]), Err(ProtoError::BadVersion(9)));
        assert!(Msg::decode(&[b'R', b'S', 1, 1, 0]).is_err());
    }
}
