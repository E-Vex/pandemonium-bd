//! Q16.16 fixed-point scalar (plan §6.1).
//!
//! Contracts (plan §5 — every one of these is property-tested):
//!
//! - All arithmetic is pure integer math; results are bit-identical on every
//!   platform and in both debug and release builds.
//! - `mul` and `div` use 64-bit intermediates and round toward zero.
//! - Results that cannot be represented **saturate** to [`Fx::MAX`] / [`Fx::MIN`]
//!   instead of panicking or wrapping — the simulation prefers a defined, detectable
//!   degradation over a crash or a silent wraparound.
//! - Division by zero saturates by the sign of the dividend (the simulation must
//!   validate its own divisions; this contract only guarantees no panics).
//! - Ordering is the total order of the underlying raw value, so `Fx` is a valid
//!   deterministic sort key (plan §5.9).

use core::ops::{Add, Div, Mul, Neg, Sub};

/// Number of fractional bits in the Q16.16 representation.
pub const FRAC_BITS: u32 = 16;

/// Number of raw units in one whole unit: 2^16 = 65536.
pub const FRAC_SCALE: i32 = 1 << FRAC_BITS;

/// Q16.16 fixed-point number with 16 fractional bits (plan §6.1).
///
/// The raw representation is an `i32` counting 1/65536ths, so the representable
/// range is about ±32768 with a resolution of about 0.0000153 — ample for tile-unit
/// positions, speeds, and ranges with integer-authored data (plan §10.2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Fx(i32);

impl Fx {
    /// The value 0.
    pub const ZERO: Self = Self(0);
    /// The value 1.
    pub const ONE: Self = Self(FRAC_SCALE);
    /// The largest representable value (about 32767.99998).
    pub const MAX: Self = Self(i32::MAX);
    /// The smallest representable value (about -32768).
    pub const MIN: Self = Self(i32::MIN);

    /// Builds a value from a raw Q16.16 integer.
    pub const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }

    /// Returns the raw Q16.16 integer — used by canonical hashing (plan §6.4).
    pub const fn raw(self) -> i32 {
        self.0
    }

    /// Converts a whole integer; saturates outside -32768..=32767.
    pub const fn from_int(value: i32) -> Self {
        if value > (i32::MAX >> FRAC_BITS) {
            Self(i32::MAX)
        } else if value < (i32::MIN >> FRAC_BITS) {
            Self(i32::MIN)
        } else {
            Self(value << FRAC_BITS)
        }
    }

    /// Converts thousandths (the plan §10.2 authoring unit — e.g. milli-tiles,
    /// milli-tiles per second), rounding toward zero; saturates outside the
    /// representable range.
    ///
    /// `const` since 1.98: every operation in here (i64 arithmetic and the
    /// `as i32` cast) is const-evaluable, so callers can build compile-time
    /// constants like `Fx::from_milli(2400)` for table-driven content stats
    /// without paying for the conversion at runtime. The clamp is written by
    /// hand because `i64::clamp` is not yet const-stable (it requires the
    /// `Ord` trait bound to be const).
    pub const fn from_milli(milli: i32) -> Self {
        let raw = ((milli as i64) * (FRAC_SCALE as i64)) / 1000;
        let clamped = if raw > (i32::MAX as i64) {
            i32::MAX as i64
        } else if raw < (i32::MIN as i64) {
            i32::MIN as i64
        } else {
            raw
        };
        Self(clamped as i32)
    }

    /// Largest whole integer not greater than the value (rounds toward negative
    /// infinity).
    pub const fn floor_int(self) -> i32 {
        self.0 >> FRAC_BITS
    }

    /// Whole integer obtained by discarding the fractional part toward zero.
    pub const fn trunc_int(self) -> i32 {
        let q = self.0 >> FRAC_BITS;
        if self.0 < 0 && self.0 & (FRAC_SCALE - 1) != 0 {
            q + 1
        } else {
            q
        }
    }

    /// Nearest whole integer; exact halves round up (toward positive infinity).
    pub const fn round_int(self) -> i32 {
        (((self.0 as i64) + (FRAC_SCALE as i64) / 2) >> FRAC_BITS) as i32
    }

    /// Multiplies, rounding toward zero; saturates when the product does not fit
    /// (64-bit intermediate, so realistic tile-unit math never saturates).
    // The inherent `mul` is the canonical documented operation and the `Mul` operator
    // impl delegates to it; keeping both is intentional (plan §5.6 favors explicit,
    // greppable arithmetic call sites in the simulation).
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, rhs: Self) -> Self {
        let p = (self.0 as i64) * (rhs.0 as i64);
        let q = p / (FRAC_SCALE as i64);
        Self(q.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    /// Divides, rounding toward zero; saturates when the quotient does not fit.
    /// Division by zero saturates by the sign of the dividend (documented contract —
    /// see the module docs).
    // Same rationale as `mul` above: inherent canonical op + delegating operator.
    #[allow(clippy::should_implement_trait)]
    pub fn div(self, rhs: Self) -> Self {
        if rhs.0 == 0 {
            return match self.0.signum() {
                1 => Self::MAX,
                -1 => Self::MIN,
                _ => Self::ZERO,
            };
        }
        let q = ((self.0 as i64) << FRAC_BITS) / (rhs.0 as i64);
        Self(q.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    /// Checked multiplication: exact, or `None` when the product does not fit.
    pub fn checked_mul(self, rhs: Self) -> Option<Self> {
        let q = ((self.0 as i64) * (rhs.0 as i64)) / (FRAC_SCALE as i64);
        i32::try_from(q).ok().map(Self)
    }

    /// Checked division: exact, or `None` on a zero divisor or a quotient that does
    /// not fit.
    pub fn checked_div(self, rhs: Self) -> Option<Self> {
        if rhs.0 == 0 {
            return None;
        }
        let q = ((self.0 as i64) << FRAC_BITS) / (rhs.0 as i64);
        i32::try_from(q).ok().map(Self)
    }

    /// Squares the value (convenience for squared-distance math, plan §5.8).
    pub fn square(self) -> Self {
        self.mul(self)
    }

    /// Absolute value; saturating — the most negative value maps to [`Fx::MAX`].
    pub const fn abs(self) -> Self {
        if self.0 == i32::MIN {
            Self(i32::MAX)
        } else if self.0 < 0 {
            Self(-self.0)
        } else {
            Self(self.0)
        }
    }
}

impl Add for Fx {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Fx {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl Mul for Fx {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Fx::mul(self, rhs)
    }
}

impl Div for Fx {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        Fx::div(self, rhs)
    }
}

impl Neg for Fx {
    type Output = Self;
    /// Negation saturates: negating [`Fx::MIN`] yields [`Fx::MAX`].
    fn neg(self) -> Self {
        if self.0 == i32::MIN {
            Self(i32::MAX)
        } else {
            Self(-self.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_is_exactly_65536_raw_units() {
        assert_eq!(Fx::ONE.raw(), 65_536);
        assert_eq!(FRAC_SCALE, 65_536);
        assert_eq!(FRAC_BITS, 16);
    }

    #[test]
    fn from_int_round_trips_exactly() {
        for i in -32_768i32..=32_767 {
            assert_eq!(Fx::from_int(i).floor_int(), i);
            assert_eq!(Fx::from_int(i).trunc_int(), i);
        }
    }

    #[test]
    fn from_int_saturates_outside_the_whole_range() {
        assert_eq!(Fx::from_int(1 << 20), Fx::MAX);
        assert_eq!(Fx::from_int(-(1 << 20)), Fx::MIN);
    }

    #[test]
    fn from_milli_is_const_evaluable() {
        // The const fn compiles in const context — the conversion runs at
        // compile time, so a `const` item carries the converted raw value.
        // Pinned against the same value the runtime path produces.
        const SPEED_2400_MTPS: Fx = Fx::from_milli(2400);
        const RANGE_5000_MT: Fx = Fx::from_milli(5000);
        const NEGATIVE: Fx = Fx::from_milli(-1);
        assert_eq!(SPEED_2400_MTPS, Fx::from_milli(2400));
        assert_eq!(SPEED_2400_MTPS.raw(), 157_286);
        assert_eq!(RANGE_5000_MT, Fx::from_int(5));
        assert_eq!(NEGATIVE.raw(), -65);
        // Const saturation: an out-of-range input still saturates at
        // compile time, mirroring the runtime path's contract.
        const HUGE: Fx = Fx::from_milli(i32::MAX);
        assert_eq!(HUGE, Fx::MAX);
    }

    #[test]
    fn from_milli_is_exact_for_wholes_and_truncates_toward_zero() {
        assert_eq!(Fx::from_milli(1000), Fx::ONE);
        assert_eq!(Fx::from_milli(-1000), -Fx::ONE);
        // 2400 milli = 2.4 tiles -> raw 157286.4 -> 157286.
        assert_eq!(Fx::from_milli(2400).raw(), 157_286);
        // 1 milli -> 65.536 raw -> 65.
        assert_eq!(Fx::from_milli(1).raw(), 65);
        // -1 milli -> -65.536 raw -> -65 (toward zero, not toward negative infinity).
        assert_eq!(Fx::from_milli(-1).raw(), -65);
    }

    #[test]
    fn floor_trunc_and_round_disagree_on_negative_halves() {
        let half = Fx::from_raw(-32_768); // -0.5
        assert_eq!(half.floor_int(), -1);
        assert_eq!(half.trunc_int(), 0);
        assert_eq!(half.round_int(), 0); // halves round up
        assert_eq!(Fx::from_raw(32_768).round_int(), 1);
        let just_past = Fx::from_raw(-32_769); // -0.50001...
        assert_eq!(just_past.round_int(), -1);
    }

    #[test]
    fn mul_is_exact_and_commutes_in_range() {
        let a = Fx::from_milli(1500); // 1.5
        let b = Fx::from_milli(-2000); // -2.0
        assert_eq!(a.mul(b), Fx::from_milli(-3000));
        assert_eq!(a.mul(b), b.mul(a));
    }

    #[test]
    fn mul_by_one_is_identity_even_at_extremes() {
        assert_eq!(Fx::MAX.mul(Fx::ONE), Fx::MAX);
        assert_eq!(Fx::MIN.mul(Fx::ONE), Fx::MIN);
        assert_eq!(Fx::from_raw(123_456).mul(Fx::ONE), Fx::from_raw(123_456));
    }

    #[test]
    fn mul_truncates_toward_zero() {
        let q = Fx::from_milli(100); // raw 6553
        assert_eq!(q.mul(q).raw(), 655); // 6553*6553/65536 = 655.06 -> 655
    }

    #[test]
    fn div_by_zero_saturates_by_sign_without_panicking() {
        assert_eq!(Fx::ONE.div(Fx::ZERO), Fx::MAX);
        assert_eq!((-Fx::ONE).div(Fx::ZERO), Fx::MIN);
        assert_eq!(Fx::ZERO.div(Fx::ZERO), Fx::ZERO);
        assert_eq!(Fx::ONE.checked_div(Fx::ZERO), None);
    }

    #[test]
    fn div_recovers_the_operand_for_exact_ratios() {
        let a = Fx::from_int(7);
        let b = Fx::from_int(4);
        assert_eq!(a.mul(b).div(b), a);
    }

    #[test]
    fn extremes_never_panic() {
        let e = [Fx::MIN, Fx::MAX, Fx::ZERO, Fx::ONE, Fx::from_raw(-1)];
        for &a in &e {
            for &b in &e {
                let _ = a + b;
                let _ = a - b;
                let _ = a * b;
                let _ = a / b;
                let _ = a.mul(b);
                let _ = a.div(b);
                let _ = a.checked_mul(b);
                let _ = a.checked_div(b);
                let _ = a.abs();
                let _ = -a;
                let _ = a.square();
                let _ = a.floor_int();
                let _ = a.trunc_int();
                let _ = a.round_int();
            }
        }
    }

    #[test]
    fn add_is_commutative_even_at_extremes() {
        let e = [Fx::MIN, Fx::MAX, Fx::ZERO, Fx::ONE, Fx::from_raw(-1)];
        for &a in &e {
            for &b in &e {
                assert_eq!(a + b, b + a);
            }
        }
    }

    #[test]
    fn checked_variants_agree_with_saturating_ones_in_range() {
        let a = Fx::from_milli(1234);
        let b = Fx::from_milli(-567);
        assert_eq!(a.checked_mul(b), Some(a.mul(b)));
        assert_eq!(a.checked_div(b), Some(a.div(b)));
    }

    #[test]
    fn ordering_is_the_total_order_of_the_raw_value() {
        assert!(Fx::from_raw(-1) < Fx::ZERO);
        assert!(Fx::ZERO < Fx::ONE);
        assert!(Fx::MIN < Fx::MAX);
    }
}
