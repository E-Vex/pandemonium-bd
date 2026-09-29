//! PCG32 deterministic random generator (plan §5.4: fx::Rng, PCG32 or xoshiro128**).
//!
//! Implementation: PCG-XSH-RR 64/32 ("pcg32") as specified in Melissa O'Neill's
//! `pcg_basic.c` — a 64-bit LCG state with a stream constant, hashed to a 32-bit
//! output with an xorshift and a rotation. Constants and the seeding ritual are
//! transcribed from the reference (docs/ASSUMPTIONS.md A-009). All arithmetic is
//! explicitly wrapping or fixed-width, so sequences are bit-identical everywhere
//! (FD-5), and the full state is inspectable for canonical hashing (plan §6.4).

/// Multiplier of the underlying 64-bit LCG, from `pcg_basic.c`.
const MULTIPLIER: u64 = 6_364_136_223_846_793_005;

/// The PCG32 random generator. Own one per match, seeded from the match seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
    inc: u64,
}

impl Rng {
    /// Default stream selector for [`Rng::seeded`] (arbitrary, fixed forever).
    pub const DEFAULT_STREAM: u64 = 0x9E37_79B9_7F4A_7C15;

    /// Canonical pcg32 seeding (`pcg32_srandom`): state starts at zero, the stream
    /// is shifted into the increment field (forced odd), the generator is stepped,
    /// the seed is added to the state, and it is stepped again. Distinct `(seed,
    /// stream)` pairs therefore diverge immediately and reproducibly.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Self {
            state: 0,
            inc: (stream << 1) | 1,
        };
        let _ = rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        let _ = rng.next_u32();
        rng
    }

    /// Convenience constructor on the default stream.
    pub fn seeded(seed: u64) -> Self {
        Self::new(seed, Self::DEFAULT_STREAM)
    }

    /// Draws 32 random bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULTIPLIER).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ (old >> 27)) >> 32) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Draws 64 random bits as a fixed composition of two 32-bit draws: the first
    /// draw is the high half, the second the low half.
    pub fn next_u64(&mut self) -> u64 {
        let hi = self.next_u32() as u64;
        let lo = self.next_u32() as u64;
        (hi << 32) | lo
    }

    /// Uniform value in `0..n`. Returns 0 for `n <= 1`. Uses rejection sampling on
    /// the 2^32 boundary, so the distribution is unbiased and, because the
    /// underlying sequence is fixed, fully deterministic.
    pub fn bounded(&mut self, n: u32) -> u32 {
        if n <= 1 {
            return 0;
        }
        let threshold = n.wrapping_neg() % n;
        loop {
            let r = self.next_u32();
            if r >= threshold {
                return r % n;
            }
        }
    }

    /// The full generator state as `(state, inc)` — the canonical form for state
    /// hashing and exact restore (plan §6.4). `inc` is always odd.
    pub fn state_parts(&self) -> (u64, u64) {
        (self.state, self.inc)
    }

    /// Restores a generator from [`Rng::state_parts`] output. The increment must be
    /// the odd value originally produced by the generator; passing an even value
    /// yields a generator with different (but still deterministic) stream behavior.
    pub fn from_state_parts(state: u64, inc: u64) -> Self {
        Self { state, inc }
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn same_seed_and_stream_reproduce_the_sequence() {
        let mut a = Rng::new(42, 54);
        let mut b = Rng::new(42, 54);
        for _ in 0..1000 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
        assert_eq!(a.state_parts(), b.state_parts());
    }

    #[test]
    fn seeded_matches_new_on_the_default_stream() {
        let mut a = Rng::seeded(7);
        let mut b = Rng::new(7, Rng::DEFAULT_STREAM);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_streams_diverge() {
        let mut a = Rng::new(42, 1);
        let mut b = Rng::new(42, 2);
        let sa: Vec<u32> = (0..100).map(|_| a.next_u32()).collect();
        let sb: Vec<u32> = (0..100).map(|_| b.next_u32()).collect();
        assert_ne!(sa, sb);
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::seeded(1);
        let mut b = Rng::seeded(2);
        let sa: Vec<u32> = (0..100).map(|_| a.next_u32()).collect();
        let sb: Vec<u32> = (0..100).map(|_| b.next_u32()).collect();
        assert_ne!(sa, sb);
    }

    #[test]
    fn next_u64_is_the_fixed_composition_of_two_u32_draws() {
        let mut a = Rng::seeded(9);
        let hi = a.next_u32();
        let lo = a.next_u32();
        let mut b = Rng::seeded(9);
        assert_eq!(b.next_u64(), ((hi as u64) << 32) | lo as u64);
    }

    #[test]
    fn bounded_stays_in_range() {
        let mut r = Rng::seeded(123);
        for _ in 0..2000 {
            let v = r.bounded(8);
            assert!(v < 8);
        }
        assert_eq!(r.bounded(0), 0);
        assert_eq!(r.bounded(1), 0);
    }

    #[test]
    fn bounded_covers_its_small_domain() {
        let mut r = Rng::seeded(2026);
        let mut seen = 0u32;
        for _ in 0..10_000 {
            seen |= 1u32 << r.bounded(8);
        }
        assert_eq!(seen, 0xFF);
    }

    #[test]
    fn state_parts_round_trip_resumes_the_sequence() {
        let mut a = Rng::seeded(55);
        for _ in 0..37 {
            let _ = a.next_u32();
        }
        let parts = a.state_parts();
        let mut b = Rng::from_state_parts(parts.0, parts.1);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }
}
