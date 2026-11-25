//! `rv-proto`: binary wire protocol v1 (all integers big-endian).
//!
//! Layout on the wire: `OuterHeader` (12 B, plaintext) followed by an
//! AEAD-encrypted inner packet (see [`Packet`]). This crate only encodes and
//! decodes headers and plaintext inner packets; encryption lives elsewhere.
//! All decoders are bounds-checked and never panic on malformed input.

mod control;
mod error;
mod input;
mod media;
mod outer;
mod packet;
mod wire;

#[cfg(test)]
mod tests;

pub use control::*;
pub use error::{ProtoError, Result};
pub use input::*;
pub use media::*;
pub use outer::*;
pub use packet::Packet;
