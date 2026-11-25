use thiserror::Error;

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("noise error: {0}")]
    Noise(#[from] snow::Error),
    #[error("handshake not finished")]
    HandshakeNotFinished,
    #[error("handshake already finished")]
    HandshakeFinished,
    #[error("missing remote static key for this handshake kind")]
    MissingRemoteKey,
    #[error("missing PSK for this handshake kind")]
    MissingPsk,
    #[error("invalid OTP: must be 6-9 ASCII digits")]
    InvalidOtp,
    #[error("key derivation failed")]
    Kdf,
    #[error("nonce exceeds 48-bit range")]
    NonceOutOfRange,
    #[error("replayed or too-old nonce")]
    Replay,
    #[error("authentication failed")]
    AuthFailed,
    #[error("packet too short")]
    TooShort,
}
