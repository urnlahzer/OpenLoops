//! A tiny fixed-width/length-prefixed binary codec for this crate's own
//! logical record content.
//!
//! `openloops-persistence`'s [`openloops_persistence::store::Store`] only
//! ever sees opaque `ciphertext` bytes (`envelope_suite.minimum_clear_envelope_fields`
//! carries no logical schema); the logical record shape inside that
//! plaintext is each owning ADR's concern
//! (`contracts/persistence/protected-state-boundary.json`
//! `logical_schema_boundary`). This module gives `ingestion`/`ledger` one
//! small, dependency-free, round-trip-tested encoding so their plaintexts are
//! never an ad hoc `format!`/`Debug` string and never an unbounded/open
//! shape: every field is fixed-width or explicitly length-prefixed, mirroring
//! the style `openloops-persistence::digest::frame` already uses for its own
//! framing, and every decode call rejects trailing or truncated bytes.
//!
//! No `serde` (or any other new external crate) is used or needed: nothing
//! here does schema evolution, self-description, or reflection — it is a
//! closed, versioned-by-convention wire format private to this crate.

/// A rejected encode/decode call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodecError {
    /// Fewer bytes remained than the field being read requires.
    Truncated,
    /// A length-prefixed field would exceed `u32::MAX` bytes.
    TooLarge,
    /// Bytes remained after every expected field was read.
    TrailingBytes,
    /// A closed tag byte did not match any known variant.
    UnknownTag(u8),
}

/// Appends a length-prefixed byte string: `u32` big-endian length, then the
/// exact bytes. Every optional (`..._or_none`) field this crate encodes uses
/// an empty slice for `None`, so presence is exactly "length is zero" and no
/// separate tag byte is needed.
///
/// # Errors
///
/// Returns [`CodecError::TooLarge`] if `value.len()` does not fit in a `u32`.
pub fn push_bytes(buf: &mut Vec<u8>, value: &[u8]) -> Result<(), CodecError> {
    let len = u32::try_from(value.len()).map_err(|_| CodecError::TooLarge)?;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(value);
    Ok(())
}

/// Appends one closed tag/code byte.
pub fn push_u8(buf: &mut Vec<u8>, value: u8) {
    buf.push(value);
}

/// Appends a big-endian `u32`.
pub fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_be_bytes());
}

/// Appends a big-endian `i64` (used for `UnixSeconds`-shaped instants).
pub fn push_i64(buf: &mut Vec<u8>, value: i64) {
    buf.extend_from_slice(&value.to_be_bytes());
}

/// Appends an optional big-endian `i64`: a one-byte presence flag, then the
/// eight value bytes only when present. Unlike [`push_bytes`]'s
/// length-implies-presence encoding, a fixed-width field needs its own
/// explicit presence flag because `0i64` is itself a valid instant.
pub fn push_option_i64(buf: &mut Vec<u8>, value: Option<i64>) {
    match value {
        Some(v) => {
            push_u8(buf, 1);
            push_i64(buf, v);
        }
        None => push_u8(buf, 0),
    }
}

/// A cursor over an encoded byte string; every read fails closed on
/// truncation rather than panicking or reading uninitialized memory.
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// Wraps `bytes` for sequential decoding.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], CodecError> {
        let end = self.at.checked_add(len).ok_or(CodecError::Truncated)?;
        if end > self.bytes.len() {
            return Err(CodecError::Truncated);
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    /// Reads one closed tag/code byte.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if no byte remains.
    pub fn read_u8(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a big-endian `u32`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if fewer than four bytes remain.
    ///
    /// # Panics
    ///
    /// Never in practice: `self.take(4)` already returns exactly 4 bytes or
    /// an `Err`, so the `try_into` conversion to `[u8; 4]` cannot fail.
    pub fn read_u32(&mut self) -> Result<u32, CodecError> {
        let bytes: [u8; 4] = self.take(4)?.try_into().expect("exactly 4 bytes");
        Ok(u32::from_be_bytes(bytes))
    }

    /// Reads a big-endian `i64`.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if fewer than eight bytes remain.
    ///
    /// # Panics
    ///
    /// Never in practice; see [`Self::read_u32`]'s `# Panics` section.
    pub fn read_i64(&mut self) -> Result<i64, CodecError> {
        let bytes: [u8; 8] = self.take(8)?.try_into().expect("exactly 8 bytes");
        Ok(i64::from_be_bytes(bytes))
    }

    /// Reads an optional big-endian `i64` in [`push_option_i64`]'s shape.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] on a short read or
    /// [`CodecError::UnknownTag`] if the presence flag is neither `0` nor `1`.
    pub fn read_option_i64(&mut self) -> Result<Option<i64>, CodecError> {
        match self.read_u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.read_i64()?)),
            other => Err(CodecError::UnknownTag(other)),
        }
    }

    /// Reads a length-prefixed byte string in [`push_bytes`]'s shape.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if the declared length exceeds the
    /// remaining bytes.
    pub fn read_bytes(&mut self) -> Result<&'a [u8], CodecError> {
        let len = self.read_u32()? as usize;
        self.take(len)
    }

    /// Reads exactly 16 raw bytes (an opaque 128-bit identifier).
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if fewer than 16 bytes remain.
    ///
    /// # Panics
    ///
    /// Never in practice; see [`Self::read_u32`]'s `# Panics` section.
    pub fn read_array16(&mut self) -> Result<[u8; 16], CodecError> {
        Ok(self.take(16)?.try_into().expect("exactly 16 bytes"))
    }

    /// Reads exactly 32 raw bytes (a full HMAC-SHA-256 digest).
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::Truncated`] if fewer than 32 bytes remain.
    ///
    /// # Panics
    ///
    /// Never in practice; see [`Self::read_u32`]'s `# Panics` section.
    pub fn read_array32(&mut self) -> Result<[u8; 32], CodecError> {
        Ok(self.take(32)?.try_into().expect("exactly 32 bytes"))
    }

    /// Consumes the reader, rejecting any unread trailing bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CodecError::TrailingBytes`] if bytes remain unread. A closed
    /// wire format has no "extra field" concept to silently ignore; an
    /// unexpectedly long input is corruption, not forward compatibility.
    pub fn finish(self) -> Result<(), CodecError> {
        if self.at == self.bytes.len() {
            Ok(())
        } else {
            Err(CodecError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CodecError, Reader, push_bytes, push_i64, push_option_i64, push_u8, push_u32};

    #[test]
    fn round_trips_every_field_shape() {
        let mut buf = Vec::new();
        push_u8(&mut buf, 7);
        push_u32(&mut buf, 0xAABB_CCDD);
        push_i64(&mut buf, -12);
        push_option_i64(&mut buf, None);
        push_option_i64(&mut buf, Some(99));
        push_bytes(&mut buf, b"hello").unwrap();
        push_bytes(&mut buf, b"").unwrap();

        let mut reader = Reader::new(&buf);
        assert_eq!(reader.read_u8().unwrap(), 7);
        assert_eq!(reader.read_u32().unwrap(), 0xAABB_CCDD);
        assert_eq!(reader.read_i64().unwrap(), -12);
        assert_eq!(reader.read_option_i64().unwrap(), None);
        assert_eq!(reader.read_option_i64().unwrap(), Some(99));
        assert_eq!(reader.read_bytes().unwrap(), b"hello");
        assert_eq!(reader.read_bytes().unwrap(), b"");
        reader.finish().unwrap();
    }

    #[test]
    fn array_reads_round_trip() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[1u8; 16]);
        buf.extend_from_slice(&[2u8; 32]);
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.read_array16().unwrap(), [1u8; 16]);
        assert_eq!(reader.read_array32().unwrap(), [2u8; 32]);
        reader.finish().unwrap();
    }

    #[test]
    fn truncated_input_fails_closed_instead_of_panicking() {
        let buf = vec![1u8, 2, 3];
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.read_u32(), Err(CodecError::Truncated));
    }

    #[test]
    fn truncated_length_prefixed_field_fails_closed() {
        let mut buf = Vec::new();
        push_u32(&mut buf, 100); // claims 100 bytes follow; none do
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.read_bytes(), Err(CodecError::Truncated));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut buf = Vec::new();
        push_u8(&mut buf, 1);
        buf.push(0xFF); // unexpected trailing byte
        let mut reader = Reader::new(&buf);
        let _ = reader.read_u8().unwrap();
        assert_eq!(reader.finish(), Err(CodecError::TrailingBytes));
    }

    #[test]
    fn unknown_option_tag_is_rejected() {
        let buf = vec![2u8]; // neither 0 (None) nor 1 (Some)
        let mut reader = Reader::new(&buf);
        assert_eq!(reader.read_option_i64(), Err(CodecError::UnknownTag(2)));
    }
}
