// src/crypto.rs

use rand_chacha::ChaCha20Rng;
use rand_core::{Rng, SeedableRng};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct HintExpansion {
    pub offsets: Vec<usize>,
    pub masks: Vec<bool>,
    pub indices: Vec<usize>,
}

pub struct HintPrg {
    rng: ChaCha20Rng,
}

impl HintPrg {
    pub fn new(seed: [u8; 32]) -> Self {
        Self {
            rng: ChaCha20Rng::from_seed(seed),
        }
    }

    /// Expand the seed to generate one local offset per partition.
    pub fn expand_offsets(&mut self, n: usize, m: usize) -> Vec<usize> {
        let mut offsets = Vec::with_capacity(n);
        for _ in 0..n {
            offsets.push((self.rng.next_u32() as usize) % m);
        }
        offsets
    }

    /// Expand the seed to generate one mask bit per partition.
    pub fn expand_masks(&mut self, n: usize) -> Vec<bool> {
        let mut masks = Vec::with_capacity(n);
        for _ in 0..n {
            masks.push((self.rng.next_u32() & 1) == 1);
        }
        masks
    }

    pub fn compute_global_indices(offsets: &[usize], m: usize) -> Vec<usize> {
        offsets
            .iter()
            .enumerate()
            .map(|(partition_idx, &offset)| partition_idx * m + offset)
            .collect()
    }
}

pub fn prf(key: &[u8], input: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(key);
    hasher.update(input);

    let digest = hasher.finalize();
    let mut output = [0u8; 32];
    output.copy_from_slice(&digest);
    output
}

pub fn derive_client_key(master_secret: &[u8; 32], client_id: &str) -> [u8; 32] {
    prf(master_secret, client_id.as_bytes())
}

pub fn derive_hint_seed(user_secret: &[u8; 32], hint_id: usize) -> [u8; 32] {
    prf(user_secret, &hint_id.to_le_bytes())
}

pub fn derive_labeled_seed(user_secret: &[u8; 32], label: &[u8]) -> [u8; 32] {
    prf(user_secret, label)
}

pub fn expand_hint(seed: [u8; 32], n: usize, m: usize) -> HintExpansion {
    let mut prg = HintPrg::new(seed);
    let offsets = prg.expand_offsets(n, m);
    let masks = prg.expand_masks(n);
    let indices = HintPrg::compute_global_indices(&offsets, m);

    HintExpansion {
        offsets,
        masks,
        indices,
    }
}