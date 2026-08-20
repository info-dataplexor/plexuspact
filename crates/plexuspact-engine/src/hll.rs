//! Minimal HyperLogLog distinct counter backing `{ unique: { approx: true } }`
//! (ADR-005). Fixed precision p=14: 16,384 one-byte registers (16 KiB constant
//! memory), standard error ≈ 1.04/√m ≈ 0.81%. Inputs are the same 64-bit
//! `ahash` values the exact path uses, so exact and approx modes observe
//! identical value identity.

/// Register-index bits. m = 2^P registers.
const P: u32 = 14;
const M: usize = 1 << P;
/// Bias-correction constant for m ≥ 128 (Flajolet et al. 2007).
const ALPHA: f64 = 0.721_347_520_444_481_7;

pub(crate) struct Hll {
    registers: Box<[u8; M]>,
}

impl Hll {
    pub fn new() -> Self {
        Hll {
            registers: Box::new([0u8; M]),
        }
    }

    /// Folds one pre-hashed value into the sketch.
    pub fn add(&mut self, hash: u64) {
        let idx = (hash >> (64 - P)) as usize;
        // Rank = position of the leftmost 1-bit in the remaining 64-P bits,
        // counted from 1; all-zero remainder saturates at (64-P)+1.
        let rest = hash << P;
        let rank = if rest == 0 {
            (64 - P + 1) as u8
        } else {
            (rest.leading_zeros() + 1) as u8
        };
        if rank > self.registers[idx] {
            self.registers[idx] = rank;
        }
    }

    /// Estimated distinct count, with the standard small-range correction
    /// (linear counting when the raw estimate is low and empty registers
    /// remain — exact in the regime where approximation would look worst).
    pub fn estimate(&self) -> u64 {
        let m = M as f64;
        let mut sum = 0.0;
        let mut zeros = 0u32;
        for &r in self.registers.iter() {
            sum += 1.0 / (1u64 << r) as f64;
            if r == 0 {
                zeros += 1;
            }
        }
        let raw = ALPHA * m * m / sum;
        let est = if raw <= 2.5 * m && zeros > 0 {
            // Linear counting.
            m * (m / f64::from(zeros)).ln()
        } else {
            raw
        };
        est.round() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(i: u64) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        let mut h = ahash::RandomState::with_seeds(1, 2, 3, 4).build_hasher();
        h.write_u64(i);
        h.finish()
    }

    #[test]
    fn small_cardinality_is_near_exact() {
        let mut hll = Hll::new();
        for i in 0..1_000u64 {
            hll.add(hash(i));
        }
        let est = hll.estimate();
        assert!((950..=1_050).contains(&est), "estimate {est} not within 5%");
    }

    #[test]
    fn large_cardinality_within_error_bound() {
        let mut hll = Hll::new();
        for i in 0..1_000_000u64 {
            hll.add(hash(i));
        }
        let est = hll.estimate() as f64;
        let err = (est - 1_000_000.0).abs() / 1_000_000.0;
        assert!(err < 0.02, "error {err:.4} exceeds 2% bound");
    }

    #[test]
    fn duplicates_do_not_inflate() {
        let mut hll = Hll::new();
        for i in 0..10_000u64 {
            hll.add(hash(i % 100));
        }
        let est = hll.estimate();
        assert!((90..=110).contains(&est), "estimate {est} far from 100");
    }
}
