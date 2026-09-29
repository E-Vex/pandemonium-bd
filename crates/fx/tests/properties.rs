//! Property tests for the fx crate (plan §6.1): conversion round-trips, arithmetic
//! laws where they are expected to hold, and no panics on extremes.

use pandemonium_fx::{fnv1a64, isqrt, Fnv1a64, Fx, Rng, Vec2Fx};
use proptest::prelude::*;

proptest! {
    /// Whole integers survive from_int -> floor_int exactly (the round-trip the
    /// content loader's integer authoring depends on, plan §10.2).
    #[test]
    fn from_int_floor_round_trips(i in -32_768i32..=32_767) {
        prop_assert_eq!(Fx::from_int(i).floor_int(), i);
    }

    /// Millis convert reproducibly: the raw value is the truncated exact ratio.
    #[test]
    fn from_milli_is_the_truncated_ratio(m in -(1 << 24)..(1 << 24)) {
        let expected = ((m as i64) * 65_536) / 1000;
        prop_assert_eq!(Fx::from_milli(m).raw() as i64, expected);
    }

    /// Addition commutes and associates whenever saturation cannot intervene
    /// (raws within ±2^16 keep sums far inside i32).
    #[test]
    fn add_laws_hold_in_range(a in -65_536i32..65_536, b in -65_536i32..65_536, c in -65_536i32..65_536) {
        let (fa, fb, fc) = (Fx::from_raw(a), Fx::from_raw(b), Fx::from_raw(c));
        prop_assert_eq!(fa + fb, fb + fa);
        prop_assert_eq!((fa + fb) + fc, fa + (fb + fc));
    }

    /// Multiplication commutes everywhere — including saturating extremes.
    #[test]
    fn mul_commutes_always(a in proptest::num::i32::ANY, b in proptest::num::i32::ANY) {
        prop_assert_eq!(
            Fx::from_raw(a).mul(Fx::from_raw(b)),
            Fx::from_raw(b).mul(Fx::from_raw(a))
        );
    }

    /// Multiplying by one is the identity everywhere (64-bit intermediate keeps
    /// even i32::MIN exact).
    #[test]
    fn mul_by_one_is_identity(a in proptest::num::i32::ANY) {
        prop_assert_eq!(Fx::from_raw(a).mul(Fx::ONE), Fx::from_raw(a));
    }

    /// mul -> div recovers the original within two raw units when the divisor is at
    /// least one whole unit and the *product* stays representable (truncation error
    /// is bounded by 2^16 / divisor + 1). Ranges are sized so |a*b| < 2^24 raw,
    /// far inside i32 — saturation is a separate, deliberately tested contract.
    #[test]
    fn mul_then_div_recovers_within_two_ulps(
        a in -(1 << 20)..(1 << 20),
        b in 65_536..(1 << 20),
    ) {
        let (fa, fb) = (Fx::from_raw(a), Fx::from_raw(b));
        let round = fa.mul(fb).div(fb);
        let diff = (round.raw() as i64 - fa.raw() as i64).abs();
        prop_assert!(diff <= 2, "diff {diff} for a={a} b={b}");
    }

    /// Division by zero never panics and saturates by dividend sign (module contract).
    #[test]
    fn div_by_zero_saturates_by_sign(a in proptest::num::i32::ANY) {
        let fa = Fx::from_raw(a);
        let got = fa.div(Fx::ZERO);
        let want = if a > 0 { Fx::MAX } else if a < 0 { Fx::MIN } else { Fx::ZERO };
        prop_assert_eq!(got, want);
    }

    /// Checked division is Some exactly where the quotient fits in i32.
    #[test]
    fn checked_div_agrees_in_range(a in -(1 << 20)..(1 << 20), b in -(1 << 20)..(1 << 20)) {
        let (fa, fb) = (Fx::from_raw(a), Fx::from_raw(b));
        if b != 0 {
            prop_assert_eq!(fa.checked_div(fb), Some(fa.div(fb)));
        }
    }

    /// isqrt is the exact floor: g*g <= n < (g+1)^2 (checked in u128 to cover
    /// the top of the u64 range without overflow).
    #[test]
    fn isqrt_is_the_floor_of_the_root(n in proptest::num::u64::ANY) {
        let g = isqrt(n);
        let g128 = u128::from(g);
        let n128 = u128::from(n);
        let upper = (g128 + 1) * (g128 + 1);
        prop_assert!(g128 * g128 <= n128);
        prop_assert!(n128 < upper);
    }

    /// Vector length dominates each component, exactly.
    #[test]
    fn vec_len_bounds_components(x in -(1 << 24)..(1 << 24), y in -(1 << 24)..(1 << 24)) {
        let v = Vec2Fx::new(Fx::from_raw(x), Fx::from_raw(y));
        let len = v.len();
        prop_assert!(len >= v.x.abs());
        prop_assert!(len >= v.y.abs());
        prop_assert!(v.len_sq_raw() >= (x as i64 * x as i64) as u64);
    }

    /// Vector subtraction and addition round-trip in range.
    #[test]
    fn vec_add_sub_round_trips(
        ax in -(1 << 20)..(1 << 20), ay in -(1 << 20)..(1 << 20),
        bx in -(1 << 20)..(1 << 20), by in -(1 << 20)..(1 << 20),
    ) {
        let a = Vec2Fx::new(Fx::from_raw(ax), Fx::from_raw(ay));
        let b = Vec2Fx::new(Fx::from_raw(bx), Fx::from_raw(by));
        prop_assert_eq!((a + b) - b, a);
    }

    /// Normalizing a real-magnitude vector yields unit length within a tight band
    /// (vectors shorter than ~1/16 tile are degenerate by contract, DEBT-002).
    #[test]
    fn vec_normalized_is_unit_length_for_real_magnitudes(
        mx in 4096u32..(1 << 24), my in 4096u32..(1 << 24),
        sx in proptest::bool::ANY, sy in proptest::bool::ANY,
    ) {
        let x = if sx { mx as i32 } else { -(mx as i32) };
        let y = if sy { my as i32 } else { -(my as i32) };
        let v = Vec2Fx::new(Fx::from_raw(x), Fx::from_raw(y));
        let len_raw = v.normalized().len().raw();
        prop_assert!(len_raw <= 65_536 + 64, "len too large: {len_raw}");
        prop_assert!(len_raw >= 65_536 - 64, "len too small: {len_raw}");
    }

    /// Equal seeds reproduce sequences; the sequence itself is the test's oracle.
    #[test]
    fn rng_is_reproducible(seed in proptest::num::u64::ANY, stream in proptest::num::u64::ANY) {
        let mut a = Rng::new(seed, stream);
        let mut b = Rng::new(seed, stream);
        for _ in 0..64 {
            prop_assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    /// bounded() never leaves its range, for any modulus.
    #[test]
    fn rng_bounded_stays_in_range(seed in proptest::num::u64::ANY, n in 2u32..=100_000) {
        let mut r = Rng::seeded(seed);
        for _ in 0..64 {
            let v = r.bounded(n);
            prop_assert!(v < n);
        }
    }

    /// The incremental hasher equals the one-shot function for arbitrary bytes.
    #[test]
    fn hash_incremental_matches_one_shot(b in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut h = Fnv1a64::new();
        // Split at an arbitrary point to exercise incremental use.
        let (l, r) = b.split_at(b.len() / 2);
        h.write_bytes(l);
        h.write_bytes(r);
        prop_assert_eq!(h.finish(), fnv1a64(&b));
    }
}
