//! Digests: FNV-1a, 64 and 128 bits, the one hash aye computes itself. It
//! is stable across builds and platforms, unlike `std`'s hasher, which is
//! all a key, a cache's or a test's, needs; and it tells whether bytes
//! changed, nothing more.

/// FNV-1a, 64 bits: a test's key (before it is mixed), the pytest
/// adapter's digest of its modules.
#[derive(Debug, Clone)]
pub struct Fnv64(u64);

impl Fnv64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;

    pub const fn new() -> Self {
        Self(Self::OFFSET)
    }

    /// The hash of `bytes` alone.
    pub fn hash(bytes: &[u8]) -> u64 {
        let mut hash = Self::new();
        hash.write(bytes);
        hash.finish()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(Self::PRIME);
        }
    }

    pub const fn finish(&self) -> u64 {
        self.0
    }
}

impl Default for Fnv64 {
    fn default() -> Self {
        Self::new()
    }
}

/// FNV-1a, 128 bits: a file's digest, the refinement cache's keys and the
/// interpreter's.
#[derive(Debug, Clone)]
pub struct Fnv128(u128);

impl Fnv128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    pub const fn new() -> Self {
        Self(Self::OFFSET)
    }

    /// The hash of `bytes` alone.
    pub fn hash(bytes: &[u8]) -> u128 {
        let mut hash = Self::new();
        hash.write(bytes);
        hash.finish()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u128::from(byte)).wrapping_mul(Self::PRIME);
        }
    }

    /// `bytes` after their length (8 bytes, little-endian), so that
    /// consecutive parts can't run into each other.
    pub fn part(&mut self, bytes: &[u8]) {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.write(&len.to_le_bytes());
        self.write(bytes);
    }

    pub const fn finish(&self) -> u128 {
        self.0
    }
}

impl Default for Fnv128 {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published FNV-1a values.
    #[test]
    fn hashes_are_fnv_1a() {
        assert_eq!(Fnv64::hash(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(Fnv64::hash(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(Fnv64::hash(b"foobar"), 0x8594_4171_f739_67e8);
        assert_eq!(Fnv128::hash(b""), 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d);
        assert_eq!(
            Fnv128::hash(b"a"),
            0xd228_cb69_6f1a_8caf_7891_2b70_4e4a_8964
        );
    }

    /// Written in pieces, the bytes hash as written at once; a part is its
    /// length, then itself.
    #[test]
    fn writes_add_up_and_parts_are_prefixed() {
        let mut pieces = Fnv128::new();
        pieces.write(b"foo");
        pieces.write(b"bar");
        assert_eq!(pieces.finish(), Fnv128::hash(b"foobar"));
        let mut parts = Fnv128::new();
        parts.part(b"ab");
        parts.part(b"c");
        let mut prefixed = Fnv128::new();
        prefixed.write(&2u64.to_le_bytes());
        prefixed.write(b"ab");
        prefixed.write(&1u64.to_le_bytes());
        prefixed.write(b"c");
        assert_eq!(parts.finish(), prefixed.finish());
        let mut other = Fnv128::new();
        other.part(b"a");
        other.part(b"bc");
        assert_ne!(parts.finish(), other.finish());
    }
}
