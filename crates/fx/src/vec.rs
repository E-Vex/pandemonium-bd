//! 2D fixed-point vector in tile units (plan §6.1).

use super::{isqrt, Fx, FRAC_BITS};
use core::ops::{Add, Neg, Sub};

/// Two-component vector of [`Fx`] values — positions, velocities, and facing
/// directions (plan §5.8: facing is a direction vector, not an angle).
///
/// Contracts: component arithmetic saturates exactly like [`Fx`]; `len` uses the
/// exact integer square root; `normalized` returns the zero vector for a zero input
/// and otherwise has unit length to within a small tolerance (see its docs).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Default)]
pub struct Vec2Fx {
    /// The x component, in tile units.
    pub x: Fx,
    /// The y component, in tile units.
    pub y: Fx,
}

impl Vec2Fx {
    /// The zero vector.
    pub const ZERO: Self = Self {
        x: Fx::ZERO,
        y: Fx::ZERO,
    };

    /// Builds a vector from two components.
    pub const fn new(x: Fx, y: Fx) -> Self {
        Self { x, y }
    }

    /// Builds a vector from two whole integers.
    pub const fn from_ints(x: i32, y: i32) -> Self {
        Self {
            x: Fx::from_int(x),
            y: Fx::from_int(y),
        }
    }

    /// Squared length, in raw Q16.16-squared units (a `u64` sum of squared raw
    /// components) — the preferred form for distance *comparisons*, because it needs
    /// no root (plan §5.8).
    pub fn len_sq_raw(self) -> u64 {
        let sx = self.x.raw() as i64;
        let sy = self.y.raw() as i64;
        ((sx * sx) as u64) + ((sy * sy) as u64)
    }

    /// Length via the exact integer square root of [`Vec2Fx::len_sq_raw`].
    /// Saturates for absurd extremes far outside tile-unit ranges.
    pub fn len(self) -> Fx {
        let r = isqrt(self.len_sq_raw());
        Fx::from_raw(r.min(i32::MAX as u64) as i32)
    }

    /// Euclidean distance between two points. Component subtraction saturates, so
    /// the result degrades (rather than panicking) only at extremes far outside
    /// tile-unit ranges.
    pub fn dist(a: Self, b: Self) -> Fx {
        (a - b).len()
    }

    /// Dot product; saturates like scalar arithmetic.
    pub fn dot(self, rhs: Self) -> Fx {
        self.x.mul(rhs.x) + self.y.mul(rhs.y)
    }

    /// Scales both components by `k`; saturates like scalar arithmetic.
    pub fn scale(self, k: Fx) -> Self {
        Self {
            x: self.x.mul(k),
            y: self.y.mul(k),
        }
    }

    /// True when both components are zero — the "no direction" case.
    pub fn is_zero(self) -> bool {
        self.x == Fx::ZERO && self.y == Fx::ZERO
    }

    /// Direction of the vector with length approximately one, computed with integer
    /// math only.
    ///
    /// The zero vector maps to the zero vector by contract — callers must handle
    /// "no direction" explicitly rather than receiving a fabricated one. For inputs
    /// of realistic geometric magnitude (raw length above ~4096, i.e. about 1/16 of
    /// a tile) the result's length is one to within a couple of raw units; precision
    /// degrades for very short vectors (integer-root floor error dominates). This is
    /// DEBT-002; facing vectors in the simulation are far longer than the threshold.
    pub fn normalized(self) -> Self {
        let len_raw = isqrt(self.len_sq_raw());
        if len_raw == 0 {
            return Self::ZERO;
        }
        let nx = (((self.x.raw() as i64) << FRAC_BITS) / (len_raw as i64))
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let ny = (((self.y.raw() as i64) << FRAC_BITS) / (len_raw as i64))
            .clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        Self {
            x: Fx::from_raw(nx),
            y: Fx::from_raw(ny),
        }
    }
}

impl Add for Vec2Fx {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Vec2Fx {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl Neg for Vec2Fx {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Vec2Fx;
    use crate::Fx;

    #[test]
    fn zero_vector_has_zero_length() {
        assert_eq!(Vec2Fx::ZERO.len(), Fx::ZERO);
        assert_eq!(Vec2Fx::ZERO.len_sq_raw(), 0);
        assert!(Vec2Fx::ZERO.is_zero());
    }

    #[test]
    fn axis_unit_has_exact_unit_length() {
        assert_eq!(Vec2Fx::from_ints(1, 0).len(), Fx::ONE);
        assert_eq!(Vec2Fx::from_ints(0, -1).len(), Fx::ONE);
    }

    #[test]
    fn three_four_five_is_exact() {
        let v = Vec2Fx::from_ints(3, 4);
        assert_eq!(v.len(), Fx::from_int(5));
        assert_eq!(Vec2Fx::dist(Vec2Fx::ZERO, v), Fx::from_int(5));
    }

    #[test]
    fn length_dominates_each_component() {
        let v = Vec2Fx::from_ints(-3, 4);
        let len = v.len();
        assert!(len >= v.x.abs());
        assert!(len >= v.y.abs());
        assert!(v.len_sq_raw() >= 9 * 65_536 * 65_536);
    }

    #[test]
    fn normalized_zero_is_zero_by_contract() {
        assert_eq!(Vec2Fx::ZERO.normalized(), Vec2Fx::ZERO);
    }

    #[test]
    fn normalized_three_four_is_exact_golden() {
        // 196608*65536/327680 = 39321.6 -> 39321; 262144*65536/327680 = 52428.8 -> 52428.
        let n = Vec2Fx::from_ints(3, 4).normalized();
        assert_eq!(n.x.raw(), 39_321);
        assert_eq!(n.y.raw(), 52_428);
    }

    #[test]
    fn normalized_is_unit_length_within_tolerance_for_real_magnitudes() {
        // A third-of-a-tile-scale direction vector: raw length ~21845.
        let v = Vec2Fx::new(Fx::from_milli(300), Fx::from_milli(400));
        let n = v.normalized();
        let len_raw = n.len().raw();
        assert!(len_raw <= 65_536 + 64, "len too large: {len_raw}");
        assert!(len_raw >= 65_536 - 64, "len too small: {len_raw}");
    }

    #[test]
    fn dot_and_scale_agree_with_scalar_math() {
        let a = Vec2Fx::from_ints(1, 2);
        let b = Vec2Fx::from_ints(3, 4);
        assert_eq!(a.dot(b), Fx::from_int(11));
        assert_eq!(a.scale(Fx::from_int(3)), Vec2Fx::from_ints(3, 6));
        assert_eq!(a.scale(Fx::ZERO), Vec2Fx::ZERO);
    }

    #[test]
    fn add_sub_neg_round_trip() {
        let a = Vec2Fx::from_ints(1, -2);
        let b = Vec2Fx::from_ints(-3, 4);
        assert_eq!((a + b) - b, a);
        assert_eq!(-(-a), a);
        assert_eq!(a - a, Vec2Fx::ZERO);
    }

    #[test]
    fn extremes_never_panic() {
        let m = Vec2Fx::new(Fx::MIN, Fx::MAX);
        let _ = m.len();
        let _ = m.len_sq_raw();
        let _ = m.normalized();
        let _ = m.dot(m);
        let _ = m.scale(Fx::MAX);
        let _ = Vec2Fx::dist(m, Vec2Fx::ZERO);
        let _ = -m;
        let _ = m + m;
        let _ = m - m;
    }
}
