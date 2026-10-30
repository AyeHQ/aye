//! Digests: FNV-1a, 64 and 128 bits, the one hash aye computes itself, and
//! the digest of a file's content it gives. It is stable across builds and
//! platforms, unlike `std`'s hasher, which is all a key, a cache's or a
//! test's, needs; and it tells whether bytes changed, nothing more.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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

/// The digest of a file's content: its [`Fnv128`] hash. A stored run keeps
/// one for each of its files (`RunMeta.files`), and `aye show` says a file
/// changed since when its digest differs; the refinement cache's key holds
/// the digests of the file and its support files, and of the environment
/// refinement runs in, which is hashed in parts (`From<Fnv128>`).
///
/// It prints and serializes as 32 lower-case hex digits, and reads 32 hex
/// digits in either case. Any other text is not a digest: aye has always
/// written them so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentDigest(u128);

impl ContentDigest {
    /// Its hex digits.
    pub const DIGITS: usize = 32;

    pub fn of(bytes: &[u8]) -> Self {
        Self(Fnv128::hash(bytes))
    }

    /// Its 16 bytes, little-endian, as a key that holds it hashes them.
    pub const fn to_le_bytes(self) -> [u8; 16] {
        self.0.to_le_bytes()
    }
}

/// What a hash was given, in parts or not.
impl From<Fnv128> for ContentDigest {
    fn from(hash: Fnv128) -> Self {
        Self(hash.finish())
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

/// Its 32 hex digits, in either case.
impl FromStr for ContentDigest {
    type Err = InvalidDigest;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != Self::DIGITS || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(InvalidDigest(text.into()));
        }
        u128::from_str_radix(text, 16)
            .map(Self)
            .map_err(|_| InvalidDigest(text.into()))
    }
}

impl Serialize for ContentDigest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Box::<str>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// A text that is not a [`ContentDigest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidDigest(Box<str>);

impl fmt::Display for InvalidDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a content digest", self.0)
    }
}

impl std::error::Error for InvalidDigest {}

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

    /// A digest is the FNV-1a 128 of the bytes, 32 hex digits on the wire,
    /// read in either case; nothing else reads.
    #[test]
    fn a_content_digest_is_32_hex_digits() {
        let digest = ContentDigest::of(b"a");
        assert_eq!(digest.to_string(), "d228cb696f1a8caf78912b704e4a8964");
        assert_eq!(
            ContentDigest::of(b"").to_string(),
            "6c62272e07bb014262b821756295c58d"
        );
        assert_eq!(digest.to_le_bytes(), Fnv128::hash(b"a").to_le_bytes());
        let json = serde_json::to_string(&digest).unwrap();
        assert_eq!(json, "\"d228cb696f1a8caf78912b704e4a8964\"");
        assert_eq!(
            serde_json::from_str::<ContentDigest>(&json).unwrap(),
            digest
        );
        assert_eq!(
            "D228CB696F1A8CAF78912B704E4A8964".parse::<ContentDigest>(),
            Ok(digest)
        );
        for text in [
            "",
            "d228cb696f1a8caf78912b704e4a896",
            "+228cb696f1a8caf78912b704e4a8964",
            "edited",
        ] {
            assert_eq!(
                text.parse::<ContentDigest>(),
                Err(InvalidDigest(text.into())),
                "{text}"
            );
        }
        assert!(serde_json::from_str::<ContentDigest>("\"edited\"").is_err());
    }
}
