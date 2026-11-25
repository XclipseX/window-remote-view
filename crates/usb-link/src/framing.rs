//! Length-prefixed record framing over a byte stream.

use crate::{LinkError, Result};

/// Maximum payload of one record.
pub const MAX_RECORD: usize = 64 * 1024;

/// Appends `u32 BE len | payload` to `out`.
pub fn encode_record(payload: &[u8], out: &mut Vec<u8>) -> Result<()> {
    if payload.len() > MAX_RECORD {
        return Err(LinkError::FrameTooLarge(payload.len(), MAX_RECORD));
    }
    out.reserve(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    Ok(())
}

/// Incremental decoder: push arbitrary stream chunks, pop whole records.
#[derive(Default)]
pub struct RecordDecoder {
    buf: Vec<u8>,
    start: usize,
}

impl RecordDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Next complete record, `Ok(None)` if more bytes are needed. A length
    /// above [`MAX_RECORD`] is a framing error (stream is desynchronised).
    pub fn next_record(&mut self) -> Result<Option<Vec<u8>>> {
        let avail = &self.buf[self.start..];
        if avail.len() < 4 {
            self.compact();
            return Ok(None);
        }
        let len = u32::from_be_bytes([avail[0], avail[1], avail[2], avail[3]]) as usize;
        if len > MAX_RECORD {
            return Err(LinkError::FrameTooLarge(len, MAX_RECORD));
        }
        if avail.len() < 4 + len {
            self.compact();
            return Ok(None);
        }
        let rec = avail[4..4 + len].to_vec();
        self.start += 4 + len;
        Ok(Some(rec))
    }

    fn compact(&mut self) {
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_split_chunks() {
        let mut wire = Vec::new();
        encode_record(b"hello", &mut wire).unwrap();
        encode_record(b"", &mut wire).unwrap();
        encode_record(&vec![7u8; MAX_RECORD], &mut wire).unwrap();
        let mut d = RecordDecoder::new();
        let mut got = Vec::new();
        // Feed in awkward 3-byte chunks.
        for c in wire.chunks(3) {
            d.push(c);
            while let Some(r) = d.next_record().unwrap() {
                got.push(r);
            }
        }
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], b"hello");
        assert!(got[1].is_empty());
        assert_eq!(got[2].len(), MAX_RECORD);
    }

    #[test]
    fn rejects_oversize_encode() {
        let mut w = Vec::new();
        assert!(matches!(
            encode_record(&vec![0; MAX_RECORD + 1], &mut w),
            Err(LinkError::FrameTooLarge(..))
        ));
        assert!(w.is_empty());
    }

    #[test]
    fn rejects_oversize_decode() {
        let mut d = RecordDecoder::new();
        d.push(&((MAX_RECORD as u32 + 1).to_be_bytes()));
        assert!(d.next_record().is_err());
    }

    #[test]
    fn partial_header_waits() {
        let mut d = RecordDecoder::new();
        d.push(&[0, 0]);
        assert_eq!(d.next_record().unwrap(), None);
        d.push(&[0, 1, 9]);
        assert_eq!(d.next_record().unwrap(), Some(vec![9]));
    }
}
