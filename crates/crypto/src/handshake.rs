use crate::{CryptoError, Keypair, Psk, Session};
use snow::{Builder, HandshakeState};
use std::ops::{Deref, DerefMut};

pub(crate) const XX: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
const XXPSK3: &str = "Noise_XXpsk3_25519_ChaChaPoly_BLAKE2s";
const IK: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";

/// Which Noise pattern to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeKind {
    /// First pairing, no password.
    Xx,
    /// First pairing bound to a one-time password PSK (psk at message 3).
    XxPsk3,
    /// Reconnect to a known host static key (initiator needs `remote_static`).
    Ik,
}

impl HandshakeKind {
    fn pattern(self) -> &'static str {
        match self {
            Self::Xx => XX,
            Self::XxPsk3 => XXPSK3,
            Self::Ik => IK,
        }
    }
}

/// Message-in / message-out handshake driver shared by both roles.
pub struct Handshake {
    state: HandshakeState,
}

impl Handshake {
    fn build(kind: HandshakeKind, initiator: bool, local: &Keypair, remote_static: Option<&[u8; 32]>, psk: Option<&Psk>, prologue: &[u8]) -> Result<Self, CryptoError> {
        let params = kind.pattern().parse()?;
        let private = local.private_bytes();
        let mut b = Builder::new(params).local_private_key(&private)?.prologue(prologue)?;
        if kind == HandshakeKind::XxPsk3 {
            b = b.psk(3, psk.ok_or(CryptoError::MissingPsk)?)?;
        }
        if kind == HandshakeKind::Ik && initiator {
            b = b.remote_public_key(remote_static.ok_or(CryptoError::MissingRemoteKey)?)?;
        }
        let state = if initiator { b.build_initiator()? } else { b.build_responder()? };
        Ok(Self { state })
    }

    /// Produce the next outgoing handshake message (with optional payload).
    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut out = vec![0u8; 65535.min(payload.len() + 512)];
        let n = self.state.write_message(payload, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// Consume an incoming handshake message; returns its decrypted payload.
    pub fn read_message(&mut self, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let mut out = vec![0u8; msg.len()];
        let n = self.state.read_message(msg, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    /// True if it is this side's turn to write.
    pub fn is_my_turn(&self) -> bool {
        self.state.is_my_turn()
    }

    /// Remote static key learned so far (available after msg 2 for XX initiator, msg 3 for responder).
    pub fn remote_static(&self) -> Option<[u8; 32]> {
        self.state.get_remote_static().and_then(|s| s.try_into().ok())
    }

    /// Convert a finished handshake into a transport [`Session`].
    pub fn finish(self) -> Result<Session, CryptoError> {
        if !self.is_finished() {
            return Err(CryptoError::HandshakeNotFinished);
        }
        let remote = self.remote_static();
        let hash = self.state.get_handshake_hash().to_vec();
        let st = self.state.into_stateless_transport_mode()?;
        Ok(Session::new(st, remote, hash))
    }
}

macro_rules! role {
    ($name:ident, $init:expr, $doc:expr) => {
        #[doc = $doc]
        pub struct $name(Handshake);
        impl $name {
            pub fn new(kind: HandshakeKind, local: &Keypair, remote_static: Option<&[u8; 32]>, psk: Option<&Psk>, prologue: &[u8]) -> Result<Self, CryptoError> {
                Handshake::build(kind, $init, local, remote_static, psk, prologue).map(Self)
            }
        }
        impl Deref for $name {
            type Target = Handshake;
            fn deref(&self) -> &Handshake {
                &self.0
            }
        }
        impl DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Handshake {
                &mut self.0
            }
        }
        impl $name {
            /// Finish the handshake and obtain the transport session.
            pub fn finish(self) -> Result<Session, CryptoError> {
                self.0.finish()
            }
        }
    };
}

role!(Initiator, true, "Initiator side (Android client). For `Ik`, `remote_static` is the pinned host key.");
role!(Responder, false, "Responder side (Windows host).");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_psk;

    const PRO: &[u8] = b"rv-test-v1";

    fn run(mut i: Initiator, mut r: Responder) -> Result<(Session, Session), CryptoError> {
        let m1 = i.write_message(b"")?;
        r.read_message(&m1)?;
        let m2 = r.write_message(b"")?;
        i.read_message(&m2)?;
        if i.is_finished() {
            return Ok((i.finish()?, r.finish()?)); // IK
        }
        let m3 = i.write_message(b"hello")?;
        assert_eq!(r.read_message(&m3)?, b"hello");
        Ok((i.finish()?, r.finish()?))
    }

    fn pair() -> (Session, Session) {
        let (ik, hk) = (Keypair::generate().unwrap(), Keypair::generate().unwrap());
        let i = Initiator::new(HandshakeKind::Xx, &ik, None, None, PRO).unwrap();
        let r = Responder::new(HandshakeKind::Xx, &hk, None, None, PRO).unwrap();
        run(i, r).unwrap()
    }

    #[test]
    fn xx_psk3_success() {
        let (ik, hk) = (Keypair::generate().unwrap(), Keypair::generate().unwrap());
        let psk = derive_psk("123456", b"host-id-01").unwrap();
        let i = Initiator::new(HandshakeKind::XxPsk3, &ik, None, Some(&psk), PRO).unwrap();
        let r = Responder::new(HandshakeKind::XxPsk3, &hk, None, Some(&psk), PRO).unwrap();
        let (si, sr) = run(i, r).unwrap();
        assert_eq!(si.remote_static(), Some(hk.public_bytes()));
        assert_eq!(sr.remote_static(), Some(ik.public_bytes()));
        assert_eq!(si.handshake_hash(), sr.handshake_hash());
    }

    #[test]
    fn wrong_otp_fails() {
        let (ik, hk) = (Keypair::generate().unwrap(), Keypair::generate().unwrap());
        let p1 = derive_psk("123456", b"host-id-01").unwrap();
        let p2 = derive_psk("654321", b"host-id-01").unwrap();
        let i = Initiator::new(HandshakeKind::XxPsk3, &ik, None, Some(&p1), PRO).unwrap();
        let r = Responder::new(HandshakeKind::XxPsk3, &hk, None, Some(&p2), PRO).unwrap();
        assert!(run(i, r).is_err());
    }

    #[test]
    fn otp_validation() {
        assert!(derive_psk("12345", b"host-id-01").is_err());
        assert!(derive_psk("1234567890", b"host-id-01").is_err());
        assert!(derive_psk("12a456", b"host-id-01").is_err());
    }

    #[test]
    fn ik_reconnect() {
        let (ik, hk) = (Keypair::generate().unwrap(), Keypair::generate().unwrap());
        let hp = hk.public_bytes();
        assert!(Initiator::new(HandshakeKind::Ik, &ik, None, None, PRO).is_err());
        let i = Initiator::new(HandshakeKind::Ik, &ik, Some(&hp), None, PRO).unwrap();
        let r = Responder::new(HandshakeKind::Ik, &hk, None, None, PRO).unwrap();
        let (mut si, mut sr) = run(i, r).unwrap();
        assert_eq!(sr.remote_static(), Some(ik.public_bytes()));
        let c = si.seal(0, b"h", b"x").unwrap();
        assert_eq!(sr.open(0, b"h", &c).unwrap(), b"x");
        let c = sr.seal(0, b"h", b"y").unwrap();
        assert_eq!(si.open(0, b"h", &c).unwrap(), b"y");
    }

    #[test]
    fn keypair_roundtrip() {
        let k = Keypair::generate().unwrap();
        let k2 = Keypair::from_private(k.private_bytes());
        assert_eq!(k.public_bytes(), k2.public_bytes());
        assert_eq!(k.fingerprint().len(), 12);
        assert_eq!(k.fingerprint(), k2.fingerprint());
    }

    #[test]
    fn seal_open_roundtrip_and_tamper() {
        let (si, mut sr) = pair();
        let aad = [7u8; 12];
        let c = si.seal(1, &aad, b"video frame").unwrap();
        assert_eq!(c.len(), 11 + crate::SEAL_OVERHEAD);
        // tampered ciphertext
        let mut bad = c.clone();
        bad[3] ^= 1;
        assert!(sr.open(1, &aad, &bad).is_err());
        // tampered aad
        let mut aad2 = aad;
        aad2[0] ^= 1;
        assert!(sr.open(1, &aad2, &c).is_err());
        // wrong nonce
        assert!(sr.open(2, &aad, &c).is_err());
        // failures must not poison the window
        assert_eq!(sr.open(1, &aad, &c).unwrap(), b"video frame");
        assert!(si.seal(MAX_NONCE + 1, &aad, b"").is_err());
    }

    use crate::MAX_NONCE;

    #[test]
    fn replay_and_reorder() {
        let (si, mut sr) = pair();
        let c: Vec<_> = (0..5u64).map(|n| si.seal(n, b"", b"d").unwrap()).collect();
        assert!(sr.open(4, b"", &c[4]).is_ok());
        assert!(sr.open(2, b"", &c[2]).is_ok()); // out of order
        assert!(sr.open(4, b"", &c[4]).is_err()); // replay
        assert!(sr.open(2, b"", &c[2]).is_err());
        assert!(sr.open(0, b"", &c[0]).is_ok());
        let far = si.seal(1000, b"", b"d").unwrap();
        assert!(sr.open(1000, b"", &far).is_ok());
        assert!(sr.open(1, b"", &c[1]).is_err()); // too old
    }
}
