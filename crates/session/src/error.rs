use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("crypto: {0}")]
    Crypto(#[from] rv_crypto::CryptoError),
    #[error("proto: {0}")]
    Proto(#[from] rv_proto::ProtoError),
    #[error("fec: {0}")]
    Fec(#[from] rv_transport::fec::FecError),
    #[error("handshake failed: {0}")]
    Handshake(&'static str),
    #[error("session not established")]
    NotEstablished,
    #[error("session id mismatch")]
    SessionId,
    #[error("malformed datagram")]
    Malformed,
    #[error("frame too large for 255-shard limit ({0} data shards)")]
    FrameTooLarge(usize),
    #[error("nonce space exhausted")]
    NonceExhausted,
    #[error("random source failure")]
    Random,
}

pub type Result<T> = std::result::Result<T, SessionError>;
