//! `rv-session`: glue proving the stack works end to end.
//!
//! rv-proto (wire format) + rv-crypto (Noise XXpsk3 handshake with OTP-derived
//! PSK, sealed packets with the 12-byte outer header as AAD, replay window) +
//! rv-transport FEC (Reed-Solomon shards, frame reassembly).
//!
//! # Handshake framing
//! rv-proto has no handshake packet type, so handshake datagrams use their own
//! tiny framing that cannot collide with data packets (data starts with
//! `0x2X`, version 2): `[0xA5, step, session_id u32 BE, noise_message...]`.
//! step 1 = initiator msg1 (session_id 0), step 2 = responder msg2 (host picks
//! the random session id; also carried as the authenticated Noise payload),
//! step 3 = initiator msg3 (carries the OTP-bound PSK proof).
//!
//! Sessions are sans-IO: feed received datagrams to `handle_datagram`, outgoing
//! datagrams go through [`DatagramIo`], results come from `poll_event`.

mod channel;
mod client;
mod error;
mod host;
pub mod mem;
mod slider;
pub mod udp;

pub use client::{ClientEvent, ClientSession, Frame};
pub use error::{Result, SessionError};
pub use host::{HostEvent, HostSession};
pub use slider::{slider_to_bitrate, slider_to_qp, Codec, DEFAULT_CODEC, DEFAULT_QUALITY};

/// Abstract datagram sink (UDP socket, in-memory pipe, ...). Best effort.
pub trait DatagramIo {
    fn send(&mut self, buf: &[u8]);
}

/// Fixed `host_id` (Argon2 salt for the OTP-derived PSK) used on the USB link.
/// Host and Android client must agree; there is no pre-handshake hello yet.
pub const USB_HOST_ID: &[u8] = b"rv-host-win-0001";

/// Silence (no authenticated packet) after which a session is declared dead.
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// Send a `Keepalive` when nothing else was sent for this long (from `tick`).
pub const KEEPALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// Host accepts at most one new handshake (msg1) per this interval.
pub const HANDSHAKE_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
/// Peer key used by [`HostSession::handle_datagram`] when the caller has no address.
pub const PEER_UNKNOWN: u64 = 0;

/// Stable-within-process key for a peer address (per-peer OTP attempt limiting).
pub fn peer_key(addr: &std::net::SocketAddr) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    addr.hash(&mut h);
    h.finish() | 1 // never PEER_UNKNOWN
}

/// First byte of every handshake datagram (data packets start with 0x1X).
pub const HS_MAGIC: u8 = 0xA5;
pub(crate) const HS_HEADER_LEN: usize = 6;
pub(crate) const PROLOGUE: &[u8] = b"rv-session-v1";

pub(crate) fn hs_packet(step: u8, session_id: u32, msg: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(HS_HEADER_LEN + msg.len());
    v.push(HS_MAGIC);
    v.push(step);
    v.extend_from_slice(&session_id.to_be_bytes());
    v.extend_from_slice(msg);
    v
}

/// Returns (step, session_id, noise message).
pub(crate) fn hs_parse(d: &[u8]) -> Result<(u8, u32, &[u8])> {
    if d.len() <= HS_HEADER_LEN || d[0] != HS_MAGIC {
        return Err(SessionError::Malformed);
    }
    Ok((
        d[1],
        u32::from_be_bytes([d[2], d[3], d[4], d[5]]),
        &d[HS_HEADER_LEN..],
    ))
}

#[cfg(test)]
mod tests;
