use crate::{CryptoError, ReplayWindow};
use sha2::{Digest, Sha256};
use snow::StatelessTransportState;

/// Largest usable nonce (48-bit).
pub const MAX_NONCE: u64 = (1 << 48) - 1;
const TAG: usize = 16;
const AAD_DIGEST: usize = 16;
/// Bytes added by `seal` to the plaintext.
pub const SEAL_OVERHEAD: usize = TAG + AAD_DIGEST;

/// Established transport session using explicit nonces (UDP-safe).
///
/// Noise transport messages carry no AAD, so the AAD (outer header) is bound
/// by prepending a 16-byte SHA-256 digest of it to the plaintext *inside* the
/// AEAD; `open` verifies it after decryption. Header tampering is rejected.
pub struct Session {
    state: StatelessTransportState,
    replay: ReplayWindow,
    remote_static: Option<[u8; 32]>,
    handshake_hash: Vec<u8>,
}

impl Session {
    pub(crate) fn new(state: StatelessTransportState, remote_static: Option<[u8; 32]>, handshake_hash: Vec<u8>) -> Self {
        Self { state, replay: ReplayWindow::new(), remote_static, handshake_hash }
    }

    /// Peer's authenticated static public key (TOFU pinning / fingerprint).
    pub fn remote_static(&self) -> Option<[u8; 32]> {
        self.remote_static
    }

    /// Channel-binding hash of the handshake (both sides get the same value).
    pub fn handshake_hash(&self) -> &[u8] {
        &self.handshake_hash
    }

    pub fn seal(&self, nonce_counter: u64, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if nonce_counter > MAX_NONCE {
            return Err(CryptoError::NonceOutOfRange);
        }
        let mut buf = Vec::with_capacity(AAD_DIGEST + plaintext.len());
        buf.extend_from_slice(&aad_digest(aad));
        buf.extend_from_slice(plaintext);
        let mut out = vec![0u8; buf.len() + TAG];
        let n = self.state.write_message(nonce_counter, &buf, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    pub fn open(&mut self, nonce_counter: u64, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if nonce_counter > MAX_NONCE {
            return Err(CryptoError::NonceOutOfRange);
        }
        if ciphertext.len() < SEAL_OVERHEAD {
            return Err(CryptoError::TooShort);
        }
        if !self.replay.check(nonce_counter) {
            return Err(CryptoError::Replay);
        }
        let mut buf = vec![0u8; ciphertext.len()];
        let n = self
            .state
            .read_message(nonce_counter, ciphertext, &mut buf)
            .map_err(|_| CryptoError::AuthFailed)?;
        buf.truncate(n);
        let expect = aad_digest(aad);
        let mut diff = 0u8;
        for (a, b) in buf[..AAD_DIGEST].iter().zip(expect.iter()) {
            diff |= a ^ b;
        }
        if diff != 0 {
            return Err(CryptoError::AuthFailed);
        }
        self.replay.update(nonce_counter);
        Ok(buf.split_off(AAD_DIGEST))
    }
}

fn aad_digest(aad: &[u8]) -> [u8; AAD_DIGEST] {
    let d = Sha256::digest(aad);
    let mut o = [0u8; AAD_DIGEST];
    o.copy_from_slice(&d[..AAD_DIGEST]);
    o
}
