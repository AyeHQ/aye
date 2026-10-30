//! Rustc's `FxHasher`, for the analysis's many small sets and maps of
//! names.

use std::collections::{HashMap, HashSet};

/// Rustc's `FxHasher`: the escape analysis hashes many short names, for
/// which `SipHash` is slow (and resistance to collisions pointless).
#[derive(Default, Clone, Copy)]
pub(crate) struct FxHasher(u64);

impl FxHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub(crate) type FastSet<T> = HashSet<T, std::hash::BuildHasherDefault<FxHasher>>;
pub(crate) type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;
