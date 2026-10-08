//! A cheap hash for the maps the core fills by the hundred thousand, such as
//! a log's entries by hash: a multiply per eight bytes, where the standard
//! library's SipHash costs about ten times more.
//!
//! Its key is drawn once per process from the standard library's random state,
//! so keys read from a folder cannot be chosen to collide where that state is
//! random; on wasm32 it is fixed, as SipHash's keys are there.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::OnceLock;

/// Builds a [`QuickHasher`] from this process's key.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Quick {
    key: u64,
}

impl Default for Quick {
    fn default() -> Self {
        static KEY: OnceLock<u64> = OnceLock::new();
        let key = *KEY.get_or_init(|| RandomState::new().hash_one(0x7051_6b61_u64));
        Self { key }
    }
}

impl BuildHasher for Quick {
    type Hasher = QuickHasher;

    fn build_hasher(&self) -> QuickHasher {
        QuickHasher(self.key)
    }
}

/// Fibonacci hashing of each 64-bit word.
pub(crate) struct QuickHasher(u64);

/// 2^64 divided by the golden ratio.
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

impl QuickHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0 ^ word).wrapping_mul(GOLDEN);
    }
}

impl Hasher for QuickHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut words = bytes.chunks_exact(8);
        for word in &mut words {
            self.add(u64::from_le_bytes(word.try_into().expect("eight bytes")));
        }
        let rest = words.remainder();
        let mut last = [0; 8];
        last[..rest.len()].copy_from_slice(rest);
        self.add(u64::from_le_bytes(last) ^ (rest.len() as u64) << 59);
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_u128(&mut self, n: u128) {
        self.add(n as u64);
        self.add((n >> 64) as u64);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    /// The high bits mixed down: a multiply leaves the low ones weakest.
    fn finish(&self) -> u64 {
        self.0 ^ self.0 >> 29
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn equal_keys_hash_alike_and_near_keys_apart() {
        let quick = Quick::default();
        assert_eq!(quick.hash_one("tags"), Quick::default().hash_one("tags"));
        let texts: HashSet<u64> = (0..10_000)
            .map(|n| quick.hash_one(format!("t{n}")))
            .collect();
        assert_eq!(texts.len(), 10_000);
        let ids: HashSet<u64> = (0..10_000u128).map(|n| quick.hash_one(n << 64)).collect();
        assert_eq!(ids.len(), 10_000);
        assert_ne!(quick.hash_one(""), quick.hash_one("\0"));
    }
}
