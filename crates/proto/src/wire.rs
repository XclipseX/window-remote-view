//! Bounds-checked big-endian reader and small write helpers.

use crate::error::{ProtoError, Result};

pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let available = self.remaining();
        if n > available {
            return Err(ProtoError::Truncated {
                needed: n,
                available,
            });
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn i8(&mut self) -> Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u48(&mut self) -> Result<u64> {
        let b = self.take(6)?;
        Ok(u64::from_be_bytes([0, 0, b[0], b[1], b[2], b[3], b[4], b[5]]))
    }

    pub fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ProtoError::InvalidValue("bool must be 0 or 1")),
        }
    }

    /// Fails if unread bytes remain (strict payload parsing).
    pub fn finish(&self) -> Result<()> {
        if self.remaining() != 0 {
            return Err(ProtoError::LengthMismatch {
                expected: self.pos,
                actual: self.pos + self.remaining(),
            });
        }
        Ok(())
    }
}

pub(crate) fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_i16(buf: &mut Vec<u8>, v: i16) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_be_bytes());
}

pub(crate) fn check_max(what: &'static str, len: usize, max: usize) -> Result<()> {
    if len > max {
        Err(ProtoError::TooLarge { what, len, max })
    } else {
        Ok(())
    }
}

/// Runs `f`; on error rolls `buf` back to its original length.
pub(crate) fn atomic<F>(buf: &mut Vec<u8>, f: F) -> Result<()>
where
    F: FnOnce(&mut Vec<u8>) -> Result<()>,
{
    let start = buf.len();
    let r = f(buf);
    if r.is_err() {
        buf.truncate(start);
    }
    r
}
