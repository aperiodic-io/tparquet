//! DECIMAL mode (spec section 4.3): floats as `q / 10^e` with exact exceptions.

/// `P[e]`: the f64 nearest to 10^e; all exact for e <= 22.
pub(crate) const POW10: [f64; 19] =
    [1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15, 1e16, 1e17, 1e18];
pub(crate) const MAX_EXPONENT: u8 = 18;
/// Values the encoder probes when it looks for the exponent.
pub(crate) const PROBE: usize = 4096;

/// A float type DECIMAL can carry.
pub(crate) trait Float: Copy {
    type Bits: Copy + PartialEq;
    fn bits(self) -> Self::Bits;
    fn to_f64(self) -> f64;
    /// the spec's decode of `q` at exponent `e`
    fn decode(q: i64, e: u8) -> Self;
}

impl Float for f64 {
    type Bits = u64;
    fn bits(self) -> u64 {
        self.to_bits()
    }
    fn to_f64(self) -> f64 {
        self
    }
    fn decode(q: i64, e: u8) -> f64 {
        q as f64 / POW10[e as usize]
    }
}

impl Float for f32 {
    type Bits = u32;
    fn bits(self) -> u32 {
        self.to_bits()
    }
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn decode(q: i64, e: u8) -> f32 {
        (q as f64 / POW10[e as usize]) as f32
    }
}

/// `q` with `decode(q, e) == v` bit for bit, if there is one near `v · 10^e`.
pub(crate) fn exact<F: Float>(v: F, e: u8) -> Option<i64> {
    let x = (v.to_f64() * POW10[e as usize]).round();
    // 2^62 keeps `q` far from i64 overflow in pcodec deltas
    if !x.is_finite() || x.abs() >= 4.611_686_018_427_388e18 {
        return None;
    }
    let q = x as i64;
    (F::decode(q, e).bits() == v.bits()).then_some(q)
}

/// Smallest exponent at which at least 99% of the probe is exact.
pub(crate) fn exponent<F: Float>(vals: &[F]) -> Option<u8> {
    let probe = &vals[..vals.len().min(PROBE)];
    if probe.is_empty() {
        return None;
    }
    (0..=MAX_EXPONENT).find(|&e| {
        let hits = probe.iter().filter(|v| exact(**v, e).is_some()).count();
        hits * 100 >= probe.len() * 99
    })
}

/// The integers and exception positions at exponent `e`, or `None` above 1% exceptions.
pub(crate) fn split<F: Float>(vals: &[F], e: u8) -> Option<(Vec<i64>, Vec<usize>)> {
    let limit = vals.len() / 100;
    let mut qs = Vec::with_capacity(vals.len());
    let mut exc = Vec::new();
    let mut prev = 0;
    for (i, v) in vals.iter().enumerate() {
        match exact(*v, e) {
            Some(q) => {
                prev = q;
                qs.push(q);
            }
            None => {
                if exc.len() == limit {
                    return None;
                }
                exc.push(i);
                qs.push(prev);
            }
        }
    }
    Some((qs, exc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pow10_is_exact() {
        for (e, p) in POW10.iter().enumerate() {
            assert_eq!(*p, format!("1e{e}").parse::<f64>().unwrap());
            assert_eq!(*p as u64, 10u64.pow(e as u32));
        }
        assert_eq!(MAX_EXPONENT as usize, POW10.len() - 1);
    }

    #[test]
    fn exact_finds_decimals_and_refuses_the_rest() {
        assert_eq!(exact(1.23f64, 2), Some(123));
        assert_eq!(exact(-0.05f64, 2), Some(-5));
        assert_eq!(exact(1.23f64, 1), None);
        assert_eq!(exact(0.29f64, 2), Some(29)); // 0.29 * 100 = 28.999999999999996
        assert_eq!(exact(5.0f64, 0), Some(5));
        assert_eq!(exact(0.1f32, 1), Some(1));
        assert_eq!(exact(-0.0f64, 3), None); // decodes to +0.0
        assert_eq!(exact(0.0f64, 3), Some(0));
        assert_eq!(exact(f64::NAN, 0), None);
        assert_eq!(exact(f64::INFINITY, 0), None);
        assert_eq!(exact(f64::NEG_INFINITY, 0), None);
        assert_eq!(exact(f32::NAN, 0), None);
    }

    #[test]
    fn exact_stops_below_2_pow_62() {
        let below = 4.611_686_018_427_387e18; // largest f64 below 2^62
        assert!(below < 2f64.powi(62));
        assert_eq!(exact(below, 0), Some(below as i64));
        assert_eq!(exact(2f64.powi(62), 0), None);
        assert_eq!(exact(-2f64.powi(62), 0), None);
        assert_eq!(exact(-below, 0), Some(-below as i64));
    }

    #[test]
    fn exponent_is_the_smallest_with_99_percent() {
        assert_eq!(exponent::<f64>(&[]), None);
        assert_eq!(exponent(&[1.0f64, 2.0]), Some(0));
        assert_eq!(exponent(&[1.5f64, 2.25]), Some(2));
        assert_eq!(exponent(&[1e-30f64]), None); // q would be 0 at every exponent
        assert_eq!(exponent(&[1e300f64]), None); // q would overflow
        assert_eq!(exponent(&[std::f64::consts::PI]), Some(15)); // 17 digits are a decimal too
        // exactly 99 of 100 at e = 1: accepted
        let mut v: Vec<f64> = (0..100).map(|i| i as f64 / 10.0).collect();
        v[0] = 1e-30;
        assert_eq!(exponent(&v), Some(1));
        // 98 of 100 at e = 1: e = 1 refused, and nothing else reaches 99% either
        v[1] = 3e-30;
        assert_eq!(exponent(&v), None);
    }

    #[test]
    fn exponent_only_reads_the_probe() {
        let mut v = vec![1.0f64; PROBE];
        v.extend(std::iter::repeat_n(std::f64::consts::PI, 10 * PROBE));
        assert_eq!(exponent(&v), Some(0));
        v.insert(0, 0.5);
        // 0.5 now sits in the probe, and one miss in 4096 is under 1%
        assert_eq!(exponent(&v), Some(0));
    }

    #[test]
    fn split_allows_exactly_one_percent() {
        let mut v: Vec<f64> = (0..200).map(f64::from).collect();
        v[5] = 0.5;
        v[50] = f64::NAN;
        let (qs, exc) = split(&v, 0).unwrap();
        assert_eq!(exc, vec![5, 50]);
        assert_eq!(qs[5], 4, "an exception repeats the previous q");
        assert_eq!(qs[50], 49);
        assert_eq!(qs[199], 199);
        v[60] = 0.25;
        assert_eq!(split(&v, 0), None, "3 in 200 is over 1%");
    }

    #[test]
    fn split_exception_first_uses_zero() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        v[0] = 0.5;
        let (qs, exc) = split(&v, 0).unwrap();
        assert_eq!((qs[0], exc), (0, vec![0]));
        // under 100 values no exception is allowed
        assert_eq!(split(&v[..99], 0), None);
    }

    #[test]
    fn split_keeps_integers_below_2_pow_62() {
        let below = f64::from_bits(2f64.powi(62).to_bits() - 1);
        let mut v: Vec<f64> = (0..500).map(f64::from).collect();
        v[10] = below;
        v[11] = -below;
        v[20] = 2f64.powi(62);
        v[21] = -2f64.powi(62);
        v[22] = f64::MAX;
        let (qs, exc) = split(&v, 0).unwrap();
        assert_eq!(exc, vec![20, 21, 22]);
        assert_eq!((qs[10], qs[11]), (below as i64, -below as i64));
        assert_eq!((qs[20], qs[21], qs[22]), (19, 19, 19), "exceptions repeat q[19]");
        // at a higher exponent the same bound applies to v * 10^e
        assert_eq!(exact(4.611_686_018_427_387e17f64, 1), Some((1 << 62) - 1_024));
        assert_eq!(exact(4.611_686_018_427_388e17f64, 1), None);
    }

    #[test]
    fn exact_bounds_floats_too() {
        let below = f32::from_bits(2f32.powi(62).to_bits() - 1);
        assert_eq!(exact(below, 0), Some(below as i64));
        assert_eq!(exact(2f32.powi(62), 0), None);
        assert_eq!(exact(-2f32.powi(62), 0), None);
        assert_eq!(exact(f32::MAX, 0), None);
        assert_eq!(exact(1e-18f64, 18), Some(1));
        assert_eq!(exact(1e-18f32, 18), Some(1));
        assert_eq!(exact(-0.0f32, 0), None);
        assert_eq!(exact(f32::from_bits(1), 18), None, "a subnormal is 0 at every exponent");
        assert_eq!(exact(f64::from_bits(1), 18), None);
        // a half rounds away from zero; the check then refuses the q that does not decode back
        assert_eq!(exact(0.125f64, 2), None);
        assert_eq!(exact(0.125f64, 3), Some(125));
        assert_eq!(exact(-0.125f64, 3), Some(-125));
    }
}
