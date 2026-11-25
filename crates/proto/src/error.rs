//! Error type shared by all encoders/decoders.

/// Errors produced while encoding or decoding wire data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    #[error("truncated input: need {needed} bytes, have {available}")]
    Truncated { needed: usize, available: usize },
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("unknown packet type 0x{0:02x}")]
    UnknownPacketType(u8),
    #[error("unknown input sub_type 0x{0:02x}")]
    UnknownSubType(u8),
    #[error("unknown control op_code 0x{0:02x}")]
    UnknownOpCode(u8),
    #[error("{what} too large: {len} > {max}")]
    TooLarge {
        what: &'static str,
        len: usize,
        max: usize,
    },
    #[error("payload length mismatch: expected {expected}, got {actual}")]
    LengthMismatch { expected: usize, actual: usize },
    #[error("invalid value: {0}")]
    InvalidValue(&'static str),
    #[error("invalid utf-8")]
    InvalidUtf8,
}

pub type Result<T> = core::result::Result<T, ProtoError>;
