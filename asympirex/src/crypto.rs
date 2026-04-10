// src/crypto.rs

use rand_core::{Rng, SeedableRng}; // The compiler explicitly suggested using Rng
use rand_chacha::ChaCha20Rng;

pub struct HintPrg {
    rng: ChaCha20Rng,
}

impl HintPrg {
    /// Initialize the PRG using a 32-byte seed.
    /// Each Hint is derived from a unique seed.
    pub fn new(seed: [u8; 32]) -> Self {
        HintPrg {
            rng: ChaCha20Rng::from_seed(seed),
        }
    }

    /// Expand the seed to generate 'n' offsets in the range [0, m).
    /// This represents the O(sqrt{N})_p offline expansion cost.
    pub fn expand_offsets(&mut self, n: usize, m: usize) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(n);
        for _ in 0..n {
            // next_u32() is now available because the Rng trait is in scope
            let offset = (self.rng.next_u32() as usize) % m;
            offsets.push(offset);
        }
        offsets
    }

    /// Generate 'n' random mask bits (booleans).
    /// Used for client-side XOR parity calculations.
    pub fn expand_masks(&mut self, n: usize) -> Vec<bool> {
        let mut masks = Vec::with_capacity(n);
        for _ in 0..n {
            let mask = (self.rng.next_u32() % 2) == 1;
            masks.push(mask);
        }
        masks
    }

    /// Map partition-local offsets to global database indices.
    pub fn compute_global_indices(offsets: &[usize], m: usize) -> Vec<usize> {
        offsets
            .iter()
            .enumerate()
            .map(|(partition_idx, &offset)| partition_idx * m + offset)
            .collect()
    }
}