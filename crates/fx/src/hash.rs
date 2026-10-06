//! Canonical hashers (plan §5.10, §6.4).
//!
//! The state hash is computed over a canonical byte encoding — little-endian, fixed
//! field order — through a hasher from this module, never over `Debug` output or
//! memory layout.
//!
//! Two hashers live here:
//! - [`XxHash64`] — the canonical hasher since M10.1 (DEBT-001 repaid): the
//!   state hash, the fixture/content hashes, and the checkpoint digests embed
//!   its output. The algorithm is Yann Collet's XXH64 (the reference spec),
//!   implemented in-repo the way the project wrote PCG32: the dependency law
//!   forbids pulling `twox-hash`, so the ~80 lines are ours. No `unsafe`.
//! - [`Fnv1a64`] — the M0-era canonical hasher, retained for the replay *file*
//!   checksum (`crates/replay`'s trailing integrity word — a file-format
//!   concern, deliberately untouched by the M10.1 swap) and its own golden
//!   tests.
//!
//! Both share the same incremental call surface (`write_*`/`finish`), so the
//! M10.1 swap was an alias change at the canonical call sites: the byte
//! encodings the hasher consumes are unchanged — only the digest moved.

/// XXH64 prime 1.
const PRIME1: u64 = 0x9E37_79B1_85EB_CA87;
/// XXH64 prime 2.
const PRIME2: u64 = 0xC2B2_AE3D_27D4_EB4F;
/// XXH64 prime 3.
const PRIME3: u64 = 0x1656_67B1_9E37_79F9;
/// XXH64 prime 4.
const PRIME4: u64 = 0x85EB_CA77_C2B2_AE63;
/// XXH64 prime 5.
const PRIME5: u64 = 0x27D4_EB2F_1656_67C5;

/// The XXH64 accumulator round (spec step 2.2): `rotl64(acc + lane*P2, 31) * P1`.
#[inline]
fn round(acc: u64, lane: u64) -> u64 {
    acc.wrapping_add(lane.wrapping_mul(PRIME2))
        .rotate_left(31)
        .wrapping_mul(PRIME1)
}

/// The merge round applied to each accumulator at digest time (spec step 2.3).
#[inline]
fn merge_round(mut acc: u64, val: u64) -> u64 {
    acc ^= round(0, val);
    acc.wrapping_mul(PRIME1).wrapping_add(PRIME4)
}

/// The final avalanche (spec step 4).
#[inline]
fn avalanche(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(PRIME2);
    h ^= h >> 29;
    h = h.wrapping_mul(PRIME3);
    h ^= h >> 32;
    h
}

#[inline]
fn read_u64(bytes: &[u8]) -> u64 {
    let mut lane = [0u8; 8];
    lane.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(lane)
}

#[inline]
fn read_u32(bytes: &[u8]) -> u32 {
    let mut lane = [0u8; 4];
    lane.copy_from_slice(&bytes[..4]);
    u32::from_le_bytes(lane)
}

/// Incremental XXH64 hasher (the canonical hasher since M10.1, DEBT-001).
///
/// Feed canonical bytes with the `write_*` methods (integers are encoded
/// little-endian, matching plan §6.4's canonical form — the same call-site
/// behavior as [`Fnv1a64`]) and read the digest with [`XxHash64::finish`].
/// Copyable and restartable at any point; arbitrary write-chunking produces
/// the same digest as one shot (the streaming-state contract, pinned by
/// tests).
#[derive(Clone, Copy, Debug)]
pub struct XxHash64 {
    /// The seed (needed at digest time for inputs shorter than one stripe).
    seed: u64,
    /// Total bytes absorbed (the digest mixes the length in).
    total_len: u64,
    /// The four stripe accumulators, initialized from the seed.
    v1: u64,
    v2: u64,
    v3: u64,
    v4: u64,
    /// The carry buffer: whole 32-byte stripes are consumed immediately,
    /// the sub-stripe tail waits here (never more than 31 bytes).
    mem: [u8; 32],
    /// How many bytes of `mem` are live.
    memsize: u32,
}

impl XxHash64 {
    /// A fresh hasher at seed 0 (the canonical state/content hash seed).
    pub const fn new() -> Self {
        Self::seeded(0)
    }

    /// A fresh hasher at `seed` (XXH64 is seedable by design; the golden
    /// vectors pin at least one nonzero seed).
    pub const fn seeded(seed: u64) -> Self {
        Self {
            seed,
            total_len: 0,
            v1: seed.wrapping_add(PRIME1).wrapping_add(PRIME2),
            v2: seed.wrapping_add(PRIME2),
            v3: seed,
            v4: seed.wrapping_sub(PRIME1),
            mem: [0; 32],
            memsize: 0,
        }
    }

    /// Consumes one whole 32-byte stripe into the four accumulators.
    fn consume_stripe(&mut self, stripe: &[u8]) {
        self.v1 = round(self.v1, read_u64(&stripe[0..]));
        self.v2 = round(self.v2, read_u64(&stripe[8..]));
        self.v3 = round(self.v3, read_u64(&stripe[16..]));
        self.v4 = round(self.v4, read_u64(&stripe[24..]));
    }

    /// Absorbs a byte slice in order. Chunking is free: any sequence of
    /// `write_bytes` calls over the same byte stream yields the same digest
    /// as a single call (mirroring [`Fnv1a64::write_bytes`] call-site
    /// behavior).
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.total_len = self.total_len.wrapping_add(bytes.len() as u64);

        // Short writes that cannot complete a stripe just extend the tail.
        if (self.memsize as u64) + (bytes.len() as u64) < 32 {
            let start = self.memsize as usize;
            self.mem[start..start + bytes.len()].copy_from_slice(bytes);
            self.memsize += bytes.len() as u32;
            return;
        }

        let mut input = bytes;
        // Complete the buffered stripe first, so the accumulators advance on
        // whole stripes only.
        if self.memsize > 0 {
            let fill = 32 - self.memsize as usize;
            self.mem[32 - fill..].copy_from_slice(&input[..fill]);
            let stripe = self.mem;
            self.consume_stripe(&stripe);
            input = &input[fill..];
            self.memsize = 0;
        }
        // Whole stripes straight from the input.
        while input.len() >= 32 {
            self.consume_stripe(&input[..32]);
            input = &input[32..];
        }
        // Park the sub-stripe tail.
        self.mem[..input.len()].copy_from_slice(input);
        self.memsize = input.len() as u32;
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

    /// The digest of everything absorbed so far (XXH64 steps 3-4: merge the
    /// accumulators for long inputs or seed+PRIME5 for short ones, mix the
    /// total length, then process the buffered tail and avalanche).
    pub fn finish(self) -> u64 {
        let mut h = if self.total_len >= 32 {
            let merged = self
                .v1
                .rotate_left(1)
                .wrapping_add(self.v2.rotate_left(7))
                .wrapping_add(self.v3.rotate_left(12))
                .wrapping_add(self.v4.rotate_left(18));
            let merged = merge_round(merged, self.v1);
            let merged = merge_round(merged, self.v2);
            let merged = merge_round(merged, self.v3);
            merge_round(merged, self.v4)
        } else {
            self.seed.wrapping_add(PRIME5)
        };
        h = h.wrapping_add(self.total_len);

        // The buffered tail (at most 31 bytes): 8-byte lanes, then a 4-byte
        // lane, then single bytes.
        let mut pos = 0usize;
        let mut len = self.memsize as usize;
        while len >= 8 {
            h ^= round(0, read_u64(&self.mem[pos..]));
            h = h.rotate_left(27).wrapping_mul(PRIME1).wrapping_add(PRIME4);
            pos += 8;
            len -= 8;
        }
        if len >= 4 {
            h ^= u64::from(read_u32(&self.mem[pos..])).wrapping_mul(PRIME1);
            h = h.rotate_left(23).wrapping_mul(PRIME2).wrapping_add(PRIME3);
            pos += 4;
            len -= 4;
        }
        while len > 0 {
            h ^= u64::from(self.mem[pos]).wrapping_mul(PRIME5);
            h = h.rotate_left(11).wrapping_mul(PRIME1);
            pos += 1;
            len -= 1;
        }
        avalanche(h)
    }
}

impl Default for XxHash64 {
    fn default() -> Self {
        Self::new()
    }
}

/// One-shot XXH64 hash of a byte slice (seed 0).
pub fn xxhash64(bytes: &[u8]) -> u64 {
    let mut h = XxHash64::new();
    h.write_bytes(bytes);
    h.finish()
}

/// One-shot seeded XXH64 hash of a byte slice.
pub fn xxhash64_seeded(bytes: &[u8], seed: u64) -> u64 {
    let mut h = XxHash64::seeded(seed);
    h.write_bytes(bytes);
    h.finish()
}

// ---------------------------------------------------------------------------
// FNV-1a 64 — the M0-era canonical hasher (plan §5.10), retained for the
// replay file checksum (a file-format concern, not the canonical state hash).
// ---------------------------------------------------------------------------

/// FNV-1a 64 offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64 prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64-bit hasher.
///
/// Feed canonical bytes with the `write_*` methods (integers are encoded
/// little-endian, matching plan §6.4's canonical form) and read the digest
/// with [`Fnv1a64::finish`]. Copyable and restartable at any point.
#[derive(Clone, Copy, Debug)]
pub struct Fnv1a64 {
    state: u64,
}

impl Fnv1a64 {
    /// A fresh hasher at the offset basis.
    pub const fn new() -> Self {
        Self {
            state: FNV_OFFSET_BASIS,
        }
    }

    /// Absorbs a byte slice in order.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.state ^= u64::from(b);
            self.state = self.state.wrapping_mul(FNV_PRIME);
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
    use super::{fnv1a64, xxhash64, xxhash64_seeded, Fnv1a64, XxHash64};

    /// The input `0..len` byte sequence at a given length.
    fn sequence(len: usize) -> Vec<u8> {
        (0..len as u8).collect()
    }

    #[test]
    fn golden_vectors_from_the_reference() {
        // Reference XXH64 vectors over the `0..len` byte sequences — the
        // reference test shape (stripe boundaries 31/32/33 included), values
        // generated by the official xxhash Python binding wrapping Yann
        // Collet's reference implementation, plus the published
        // empty-input (0xEF46DB3751D8E999) and "abc" (0x44BC2CF5AD770999)
        // vectors as cross-checks below.
        let vectors: &[(usize, u64, u64)] = &[
            // (len, seed 0 digest, seed 1 digest)
            (0, 0xEF46_DB37_51D8_E999, 0xD5AF_BA13_36A3_BE4B),
            (1, 0xE934_A84A_DB05_2768, 0x7719_17C7_F6EE_2451),
            (14, 0x5CDA_8B69_BBFC_1D45, 0x9744_EC47_2BD1_5C83),
            (31, 0xC346_D2B5_9B4D_8EE1, 0xF031_031D_6597_7DFC),
            (32, 0xCBF5_9C51_16FF_32B4, 0xD74E_6766_CE9D_BA94),
            (33, 0x0C53_5D1A_CAFB_8EAD, 0xA371_825F_4210_FE99),
            (100, 0x6AC1_E580_3216_6597, 0x3D19_A3A2_098A_7023),
            (222, 0x88FF_0DAD_A4BF_A877, 0x2D04_D8C1_6E56_2039),
        ];
        for (len, want_zero, want_one) in vectors {
            let data = sequence(*len);
            assert_eq!(xxhash64(&data), *want_zero, "seed 0 at len {len}");
            assert_eq!(xxhash64_seeded(&data, 1), *want_one, "seed 1 at len {len}");
        }
        // The published "abc" vector (seed 0) cross-checks the reference.
        assert_eq!(xxhash64(b"abc"), 0x44BC_2CF5_AD77_0999);
    }

    #[test]
    fn incremental_matches_one_shot_at_every_split() {
        // The streaming-state contract: any chunking of the same byte stream
        // digests to the one-shot value. Covers the carry buffer's fill,
        // complete, and re-fill paths at every boundary.
        let data = sequence(222);
        for split in 0..=data.len() {
            let mut h = XxHash64::new();
            h.write_bytes(&data[..split]);
            h.write_bytes(&data[split..]);
            assert_eq!(h.finish(), xxhash64(&data), "split at {split}");
        }
    }

    #[test]
    fn incremental_matches_one_shot_chunked() {
        // Byte-at-a-time and ragged multi-chunk writes across the stripe
        // boundary, at both zero and nonzero seeds.
        let data = sequence(100);
        let mut h = XxHash64::new();
        for byte in &data {
            h.write_u8(*byte);
        }
        assert_eq!(h.finish(), xxhash64(&data));
        let mut h = XxHash64::seeded(1);
        for chunk in data.chunks(7) {
            h.write_bytes(chunk);
        }
        assert_eq!(h.finish(), xxhash64_seeded(&data, 1));
    }

    #[test]
    fn integer_writes_are_little_endian() {
        for n in [0u32, 1, 0xDEAD_BEEF, u32::MAX] {
            let mut a = XxHash64::new();
            a.write_u32(n);
            let mut b = XxHash64::new();
            b.write_bytes(&n.to_le_bytes());
            assert_eq!(a.finish(), b.finish());
        }
        for n in [0i64, -1, i64::MIN, i64::MAX] {
            let mut a = XxHash64::new();
            a.write_i64(n);
            let mut b = XxHash64::new();
            b.write_bytes(&n.to_le_bytes());
            assert_eq!(a.finish(), b.finish());
        }
    }

    #[test]
    fn changing_any_byte_changes_the_digest() {
        let a = xxhash64(b"pandemonium");
        let b = xxhash64(b"pandemoniuN");
        let c = xxhash64(b"pandemoniu");
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn default_is_a_fresh_hasher() {
        let h: XxHash64 = Default::default();
        assert_eq!(h.finish(), xxhash64(b""));
    }

    #[test]
    fn the_seed_matters() {
        let data = sequence(40);
        assert_ne!(xxhash64(&data), xxhash64_seeded(&data, 1));
        // Short inputs take the seed+PRIME5 branch — the seed must survive
        // even when no stripe was ever consumed.
        assert_ne!(xxhash64(b""), xxhash64_seeded(b"", 1));
    }

    // -- FNV-1a (retained: the replay file checksum) -----------------------

    #[test]
    fn fnv_golden_vectors_from_the_fnv_reference() {
        // Published FNV-1a 64 test vectors (isthe.com/chongo/tech/comp/fnv).
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn fnv_incremental_matches_one_shot() {
        let mut h = Fnv1a64::new();
        h.write_bytes(b"foo");
        h.write_bytes(b"bar");
        assert_eq!(h.finish(), fnv1a64(b"foobar"));
    }

    #[test]
    fn fnv_integer_writes_are_little_endian() {
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
    fn fnv_default_is_a_fresh_hasher() {
        let h: Fnv1a64 = Default::default();
        assert_eq!(h.finish(), fnv1a64(b""));
    }

    #[test]
    fn the_two_hashers_disagree_on_a_shared_input() {
        // The M10.1 swap must change every digest: same bytes, different
        // algorithms. If these ever agree, the canonical pins would not move
        // and the swap would be a no-op worth investigating.
        let data = sequence(64);
        assert_ne!(xxhash64(&data), fnv1a64(&data));
    }
}
