//! Exact integer square root (plan §5.8: "isqrt where a length is needed").

/// Floor of the square root of `n`, by the classic binary digit-by-digit method.
///
/// Exact for the whole `u64` range, uses only integer arithmetic, and is therefore
/// bit-identical on every platform (FD-5). Property-tested: `g*g <= n < (g+1)*(g+1)`.
pub const fn isqrt(n: u64) -> u64 {
    let mut x = n;
    let mut g: u64 = 0;
    let mut b: u64 = 1 << 62;
    while b > n {
        b >>= 2;
    }
    while b != 0 {
        if x >= g + b {
            x -= g + b;
            g = (g >> 1) + b;
        } else {
            g >>= 1;
        }
        b >>= 2;
    }
    g
}

#[cfg(test)]
mod tests {
    use super::isqrt;

    #[test]
    fn known_values() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(2), 1);
        assert_eq!(isqrt(3), 1);
        assert_eq!(isqrt(4), 2);
        assert_eq!(isqrt(8), 2);
        assert_eq!(isqrt(9), 3);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        assert_eq!(isqrt(1 << 32), 65_536);
        assert_eq!(isqrt(1 << 62), 1 << 31);
        assert_eq!(isqrt(u64::MAX), 4_294_967_295);
    }

    #[test]
    fn is_floor_of_the_true_root() {
        // Dense sweep of a small range.
        for n in 0u64..4096 {
            let g = isqrt(n);
            assert!(g * g <= n, "g*g <= n failed at n={n}");
            assert!(n < (g + 1) * (g + 1), "n < (g+1)^2 failed at n={n}");
        }
        // Geometric samples up the whole u64 range; the bounds are checked in u128
        // so nothing overflows near the top.
        let mut n: u64 = 4096;
        loop {
            check_root_floor(n - 1);
            match n.checked_mul(2) {
                Some(next) => n = next,
                None => break,
            }
        }
        check_root_floor(u64::MAX);
    }

    fn check_root_floor(n: u64) {
        let g = u128::from(isqrt(n));
        assert!(g * g <= u128::from(n));
        assert!(u128::from(n) < (g + 1) * (g + 1));
    }
}
