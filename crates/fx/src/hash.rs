//! FNV-1a 64-bit canonical hasher (plan §5.10, §6.4).
//!
//! The state hash is computed over a canonical byte encoding — little-endian, fixed
//! field order — through this hasher, never over `Debug` output or memory layout.
//! FNV-1a is chosen for M0 because it is trivially portable and obviously correct;
//! swapping to xxHash64 behind the same API is DEBT-001.

/// FNV-1a 64 offset basis.
const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 prime.
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64-bit hasher.
///
/// Feed canonical bytes with the `write_*` methods (integers are encoded
/// little-endian, matching plan §6.4's canonical form) and read the digest with
/// [`Fnv1a64::finish`]. Copyable and restartable at any point.
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    /// A fresh hasher at the offset basis.
    pub const fn new() -> Self {
        Self {
            state: OFFSET_BASIS,
        }
    }

    /// Absorbs a byte slice in order.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= u64::from(b);
            self.state = self.state.wrapping_mul(PRIME);
        }
    }

    /// Absorbs a `u8` (single byte, identity encoding).
    pub fn write_u8(&mut self, n: u8) {
        self.write_bytes(&[n]);
    }

    /// Absorbs a `u16`, little-endian.
    pub fn write_u16(&mut self, n: u16) {
        self.write_bytes(&n.to_le_bytes());
    }

    /// Absorbs a `u32`, little-endian.
    pub fn write_u32(&mut self, n: u32) {
        self.write_bytes(&n.to_le_bytes());
    }

    /// Absorbs a `u64`, little-endian.
    pub fn write_u64(&mut self, n: u64) {
        self.write_bytes(&n.to_le_bytes());
    }

    /// Absorbs an `i32` via its bit pattern, little-endian.
    pub fn write_i32(&mut self, n: i32) {
        self.write_bytes(&n.to_le_bytes());
    }

    /// Absorbs an `i64` via its bit pattern, little-endian.
    pub fn write_i64(&mut self, n: i64) {
        self.write_bytes(&n.to_le_bytes());
    }

    /// The digest of everything absorbed so far.
    pub fn finish(self) -> u64 {
        self.state
    }
}

impl Default for Fnv1a64 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot FNV-1a 64 hash of a byte slice.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h = Fnv1a64::new();
    h.write_bytes(bytes);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::{fnv1a64, Fnv1a64};

    #[test]
    fn golden_vectors_from_the_fnv_reference() {
        // Published FNV-1a 64 test vectors (isthe.com/chongo/tech/comp/fnv).
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn incremental_matches_one_shot() {
        let mut h = Fnv1a64::new();
        h.write_bytes(b"foo");
        h.write_bytes(b"bar");
        assert_eq!(h.finish(), fnv1a64(b"foobar"));
    }

    #[test]
    fn integer_writes_are_little_endian() {
        for n in [0u32, 1, 0xDEAD_BEEF, u32::MAX] {
            let mut a = Fnv1a64::new();
            a.write_u32(n);
            let mut b = Fnv1a64::new();
            b.write_bytes(&n.to_le_bytes());
            assert_eq!(a.finish(), b.finish());
        }
        for n in [0i64, -1, i64::MIN, i64::MAX] {
            let mut a = Fnv1a64::new();
            a.write_i64(n);
            let mut b = Fnv1a64::new();
            b.write_bytes(&n.to_le_bytes());
            assert_eq!(a.finish(), b.finish());
        }
    }

    #[test]
    fn changing_any_byte_changes_the_digest() {
        let a = fnv1a64(b"pandemonium");
        let b = fnv1a64(b"pandemoniuN");
        let c = fnv1a64(b"pandemoniu");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn default_is_a_fresh_hasher() {
        let h: Fnv1a64 = Default::default();
        assert_eq!(h.finish(), fnv1a64(b""));
    }
}
