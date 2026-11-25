//! rv-transport: UDP transport building blocks for the remote-view stack.
//!
//! * [`fec`]       adaptive Reed-Solomon FEC + frame reassembly
//! * [`discovery`] LAN beacon discovery (broadcast + multicast)
//! * [`pacer`]     priority send queue (input/control before video)
//! * [`path`]      connection path kinds and happy-eyeballs racing

pub mod discovery;
pub mod fec;
pub mod pacer;
pub mod path;
