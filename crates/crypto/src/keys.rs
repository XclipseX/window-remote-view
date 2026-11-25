use crate::CryptoError;
use sha2::{Digest, Sha256};

/// X25519 static keypair.
#[derive(Clone)]
pub struct Keypair {
    private: [u8; 32],
    public: [u8; 32],
}

impl std::fmt::Debug for Keypair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keypair").field("public", &hex(&self.public)).finish_non_exhaustive()
    }
}

impl Drop for Keypair {
    fn drop(&mut self) {
        // Best-effort wipe (volatile writes so they are not optimised away).
        for b in self.private.iter_mut() {
            unsafe { std::ptr::write_volatile(b, 0) };
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Keypair {
    pub fn generate() -> Result<Self, CryptoError> {
        let kp = snow::Builder::new(crate::handshake::XX.parse()?).generate_keypair()?;
        Ok(Self::from_private_bytes(kp.private.try_into().map_err(|_| CryptoError::Kdf)?, kp.public.try_into().map_err(|_| CryptoError::Kdf)?))
    }

    fn from_private_bytes(private: [u8; 32], public: [u8; 32]) -> Self {
        Self { private, public }
    }

    /// Rebuild a keypair from a stored 32-byte private key.
    pub fn from_private(private: [u8; 32]) -> Self {
        let public = x25519_public(&private);
        Self { private, public }
    }

    pub fn private_bytes(&self) -> [u8; 32] {
        self.private
    }

    pub fn public_bytes(&self) -> [u8; 32] {
        self.public
    }

    /// Short hex fingerprint of the public key (see [`fingerprint_of`]).
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.public)
    }
}

/// Fingerprint of any public key: first 6 bytes of SHA-256, as 12 lowercase hex chars.
pub fn fingerprint_of(public: &[u8; 32]) -> String {
    hex(&Sha256::digest(public)[..6])
}

/// X25519 base-point multiplication (via snow's resolver-independent path:
/// we derive it with a throwaway handshake-free DH using the RFC 7748 ladder).
fn x25519_public(private: &[u8; 32]) -> [u8; 32] {
    use snow::params::DHChoice;
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    let mut dh = DefaultResolver.resolve_dh(&DHChoice::Curve25519).expect("x25519 available");
    dh.set(private);
    let mut out = [0u8; 32];
    out.copy_from_slice(dh.pubkey());
    out
}
