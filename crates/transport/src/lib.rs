//! rv-transport: UDP transport building blocks for the remote-view stack.
//!
//! * [`fec`]       adaptive Reed-Solomon FEC + frame reassembly
//! * [`discovery`] LAN beacon discovery (broadcast + multicast)
//! * [`iface`]     interface enumeration/classification + link profiles
//! * [`tcp`]       TCP/443 datagram tunnel (relay fallback)
//! * [`pacer`]     priority send queue (input/control before video)
//! * [`path`]      connection path kinds and happy-eyeballs racing

pub mod discovery;
pub mod fec;
pub mod iface;
pub mod pacer;
pub mod tcp;
pub mod path;
