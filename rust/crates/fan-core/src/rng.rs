/// Tiny deterministic PRNG (xorshift64*) — enough for the simulation's load
/// pattern and the daemon's tick jitter, with zero dependencies. Not a match
/// for C#'s `Random` sequence and doesn't need to be: the sim is a demo plant,
/// not a parity target.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Spread small seeds across the state space; state must be non-zero.
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform integer in [lo, hi) — for the tick jitter (850..1151 ms).
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next_u64() % (hi - lo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            let v = a.next_f64();
            assert_eq!(v, b.next_f64());
            assert!((0.0..1.0).contains(&v));
            let r = a.range(850, 1151);
            b.range(850, 1151);
            assert!((850..1151).contains(&r));
        }
    }
}
