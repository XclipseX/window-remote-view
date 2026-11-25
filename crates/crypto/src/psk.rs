use crate::CryptoError;
use argon2::{Algorithm, Argon2, Params, Version};

pub type Psk = [u8; 32];

/// Derive a 32-byte Noise PSK from a 6-9 digit OTP using Argon2id.
/// `host_id` is used as the salt (must be >= 8 bytes).
pub fn derive_psk(otp: &str, host_id: &[u8]) -> Result<Psk, CryptoError> {
    if !(6..=9).contains(&otp.len()) || !otp.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CryptoError::InvalidOtp);
    }
    if host_id.len() < 8 {
        return Err(CryptoError::Kdf);
    }
    #[cfg(test)]
    let params = Params::new(8, 1, 1, Some(32)).map_err(|_| CryptoError::Kdf)?;
    #[cfg(not(test))]
    let params = Params::new(19 * 1024, 2, 1, Some(32)).map_err(|_| CryptoError::Kdf)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon.hash_password_into(otp.as_bytes(), host_id, &mut out).map_err(|_| CryptoError::Kdf)?;
    Ok(out)
}
