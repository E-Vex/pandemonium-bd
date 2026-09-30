//! The canonical little-endian byte codec for the replay format (plan §6.5).
//!
//! Replays deliberately do not use serde: the sim-side snapshot rule ("our own
//! canonical byte encoding", plan §6.4) extends to the replay file, so the byte
//! form is fully ours, deterministic across platforms, and checksummed. This
//! module is the tiny reader/writer that makes the format explicit — every field
//! written or read is visible in [`crate::ReplayFile::encode`] and
//! [`crate::ReplayFile::decode`].

/// Appends canonical little-endian bytes to a buffer.
#[derive(Debug, Default)]
pub(crate) struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// A writer over a fresh buffer.
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Appends one byte.
    pub(crate) fn write_u8(&mut self, n: u8) {
        self.buf.push(n);
    }

    /// Appends a `u16` in little-endian.
    pub(crate) fn write_u16(&mut self, n: u16) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    /// Appends a `u32` in little-endian.
    pub(crate) fn write_u32(&mut self, n: u32) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    /// Appends a `u64` in little-endian.
    pub(crate) fn write_u64(&mut self, n: u64) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    /// Appends an `i32` in little-endian (its bit pattern).
    pub(crate) fn write_i32(&mut self, n: i32) {
        self.buf.extend_from_slice(&n.to_le_bytes());
    }

    /// Appends a `bool` as `0`/`1`.
    pub(crate) fn write_bool(&mut self, n: bool) {
        self.buf.push(u8::from(n));
    }

    /// Consumes the writer and yields the buffer.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// Reads canonical little-endian bytes. Every read is bounds-checked; failure is
/// a plain static reason string (the caller wraps it into a typed error).
#[derive(Debug)]
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

/// Why a read failed — always "truncated" in some form.
pub(crate) type ReadError = &'static str;

impl<'a> Reader<'a> {
    /// A reader positioned at the start of `bytes`.
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ReadError> {
        if self.pos + len > self.bytes.len() {
            return Err("unexpected end of replay data");
        }
        let slice = &self.bytes[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    /// Reads one byte.
    pub(crate) fn read_u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a `u16`.
    pub(crate) fn read_u16(&mut self) -> Result<u16, ReadError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Reads a `u32`.
    pub(crate) fn read_u32(&mut self) -> Result<u32, ReadError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads a `u64`.
    pub(crate) fn read_u64(&mut self) -> Result<u64, ReadError> {
        let b = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_le_bytes(arr))
    }

    /// Reads an `i32`.
    pub(crate) fn read_i32(&mut self) -> Result<i32, ReadError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Reads a `bool` (`0`/`1`); any other byte is a format error.
    pub(crate) fn read_bool(&mut self) -> Result<bool, ReadError> {
        match self.read_u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("boolean field is neither 0 nor 1"),
        }
    }

    /// True once every byte has been consumed.
    pub(crate) fn is_empty(&self) -> bool {
        self.pos == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_width() {
        let mut w = Writer::new();
        w.write_u8(0xAB);
        w.write_u16(0xC0DE);
        w.write_u32(0xDEAD_BEEF);
        w.write_u64(0x0123_4567_89AB_CDEF);
        w.write_i32(-1);
        w.write_bool(true);
        w.write_bool(false);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u8().unwrap(), 0xAB);
        assert_eq!(r.read_u16().unwrap(), 0xC0DE);
        assert_eq!(r.read_u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(r.read_u64().unwrap(), 0x0123_4567_89AB_CDEF);
        assert_eq!(r.read_i32().unwrap(), -1);
        assert!(r.read_bool().unwrap());
        assert!(!r.read_bool().unwrap());
        assert!(r.is_empty());
    }

    #[test]
    fn truncation_is_detected_at_every_width() {
        let mut r = Reader::new(&[0x01]);
        assert!(r.read_u16().is_err());
        let mut r = Reader::new(&[0x01, 0x02, 0x03]);
        assert!(r.read_u32().is_err());
        let mut r = Reader::new(&[2]);
        assert_eq!(
            r.read_bool().unwrap_err(),
            "boolean field is neither 0 nor 1"
        );
    }

    #[test]
    fn little_endian_is_explicit() {
        let mut w = Writer::new();
        w.write_u32(1);
        assert_eq!(w.into_bytes(), [1, 0, 0, 0]);
    }
}
