//! End-to-end encryption primitives: Noise handshakes (XXpsk3 / XX / IK),
//! stateless UDP transport sealing with anti-replay, OTP-derived PSK and
//! a brute-force limiter.

mod error;
mod handshake;
mod keys;
mod limiter;
mod psk;
mod replay;
mod session;

pub use error::CryptoError;
pub use handshake::{HandshakeKind, Initiator, Responder, Handshake};
pub use keys::Keypair;
pub use limiter::AttemptLimiter;
pub use psk::{derive_psk, Psk};
pub use replay::ReplayWindow;
pub use session::{Session, MAX_NONCE, SEAL_OVERHEAD};
