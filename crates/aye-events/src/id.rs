//! Identities: of a test, its key, an adapter's name, and the aliases for
//! environments, lifetimes, workers and batches.

use std::borrow::{Borrow, Cow};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::path::ProjectPathRef;

/// The canonical path id of a test: the file path relative to the project
/// root with `/` separators, then `::`-separated names, then the parameter
/// id in brackets (`tests/test_cart.py::TestCart::test_add[eur]`).
///
/// It deliberately has no line: failure locations, snapshots and the source
/// rewriter point at a [`Span`](crate::Span) instead. Parameters stay opaque
/// inside the id; structured parameters, when an adapter knows them, travel
/// beside it on the [`Case`](crate::Case).
///
/// An id is never rejected: what a worker sends flows as it is. It orders
/// as its text, which the history's file and its parameter lookups rely on
/// (`t.py::test_x[…]` sorts between `t.py::test_x[` and `t.py::test_x\`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestId(Box<str>);

impl TestId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into().into_boxed_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0.into_string()
    }

    /// The id taken apart: its file, its names and its parameter id.
    pub fn parts(&self) -> TestIdParts<'_> {
        TestIdParts::parse(&self.0)
    }

    /// The file part of the id, relative to the project root.
    pub fn path(&self) -> ProjectPathRef<'_> {
        self.parts().path
    }

    /// The id without its parameter id: `t.py::test_x[1]` → `t.py::test_x`.
    /// The parameter id opens at the first `[` after the path, since names
    /// never hold one, and the id must end with its `]`; a file's name may
    /// hold `[` (`t[1].py::test_x[1]` → `t[1].py::test_x`).
    pub fn without_params(&self) -> &str {
        match self.parts().params {
            Some(params) => &self.0[..self.0.len() - params.len() - "[]".len()],
            None => &self.0,
        }
    }

    /// What the id lies below: the id up to each `::` in it (its file, then
    /// each enclosing name), then, when it has a parameter id, the id
    /// without it. A test that started covers these as well as itself.
    pub fn ancestors(&self) -> impl Iterator<Item = &str> {
        let bare = self.without_params();
        self.0
            .match_indices("::")
            .map(|(at, _)| &self.0[..at])
            .chain((bare.len() != self.0.len()).then_some(bare))
    }

    /// Whether the id names a file and no test in it: an id without `::`,
    /// such as a module skipped while collecting.
    pub fn is_file(&self) -> bool {
        !self.0.contains("::")
    }

    pub fn key(&self) -> TestKey {
        TestKey::of(&self.0)
    }
}

impl std::fmt::Display for TestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TestId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for TestId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// A test id taken apart, by one rule: the path ends at the first `::`;
/// names never hold `[`, so the first `[` in the names opens the parameter
/// id, which may itself hold `[`, `::` or `.`, when the names end with its
/// `]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestIdParts<'a> {
    /// The file, relative to the project root.
    pub path: ProjectPathRef<'a>,
    /// The names after the path, `::`-separated, without the parameter id:
    /// `TestX::test_y`. Empty for a file's id.
    pub names: &'a str,
    /// The parameter id, without its brackets: `a-1`.
    pub params: Option<&'a str>,
}

impl<'a> TestIdParts<'a> {
    /// `text` taken apart as an id is: a [`TestId`]'s own, or a selector
    /// written as one.
    pub fn parse(text: &'a str) -> Self {
        let Some((path, rest)) = text.split_once("::") else {
            return Self {
                path: ProjectPathRef::as_written(text),
                names: "",
                params: None,
            };
        };
        let (names, params) = match rest.find('[') {
            Some(at) if rest.ends_with(']') => (&rest[..at], Some(&rest[at + 1..rest.len() - 1])),
            _ => (rest, None),
        };
        Self {
            path: ProjectPathRef::as_written(path),
            names,
            params,
        }
    }
}

/// A short, stable key for a test: the FNV-1a 64 hash of its id, mixed
/// (`SplitMix64`'s finalizer) so that every digit depends on every byte,
/// printed as 16 lower-case hex digits. A key is shown by its first digits,
/// and FNV-1a alone leaves the leading ones unchanged by the last bytes:
/// `test_0` and `test_1` would share them.
///
/// It orders as its digits do, and serializes as them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestKey(u64);

impl TestKey {
    /// Its digits.
    pub const DIGITS: usize = 16;

    pub fn of(id: &str) -> Self {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0100_0000_01b3;
        let mut hash = id
            .bytes()
            .fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME));
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^= hash >> 31;
        Self(hash)
    }

    /// Its first `digits` hex digits, all 16 at most: how it is shown.
    pub fn prefix(self, digits: usize) -> String {
        let mut text = self.to_string();
        text.truncate(digits);
        text
    }

    /// Whether its digits start with `prefix`'s.
    pub fn starts_with(self, prefix: &KeyPrefix) -> bool {
        self.to_string().starts_with(prefix.as_str())
    }
}

impl fmt::Display for TestKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// Its 16 hex digits, in either case.
impl FromStr for TestKey {
    type Err = InvalidKey;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != Self::DIGITS || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(InvalidKey(text.into()));
        }
        u64::from_str_radix(text, 16)
            .map(Self)
            .map_err(|_| InvalidKey(text.into()))
    }
}

impl Serialize for TestKey {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TestKey {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = Box::<str>::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The start of a test's key, as a selector or `aye show 3fae6` writes it:
/// 4 to 16 hex digits, kept in lower case.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyPrefix(Box<str>);

impl KeyPrefix {
    /// The fewest digits that make a prefix.
    pub const MIN: usize = 4;

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 4 to 16 hex digits, in either case.
impl FromStr for KeyPrefix {
    type Err = InvalidKey;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !(Self::MIN..=TestKey::DIGITS).contains(&text.len())
            || !text.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(InvalidKey(text.into()));
        }
        Ok(Self(text.to_ascii_lowercase().into_boxed_str()))
    }
}

impl fmt::Display for KeyPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A text that is not a key, or the start of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidKey(Box<str>);

impl fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a test key", self.0)
    }
}

impl std::error::Error for InvalidKey {}

/// The name an adapter goes by (`pytest`): on each case and batch, in a
/// refinement request, and as the key of what a run keeps by adapter. An
/// open set, not an enum: each adapter names itself
/// ([`AdapterName::new_static`]), and the crates on the runner's side carry
/// a name as data and never branch on one. A name a worker, a stored log or
/// a test gives is kept as it was written.
///
/// An adapter's own name is never blank, so the empty name, the default,
/// names none: `aye show`'s case for a name collection didn't list, a
/// batch the runner saw finish but not start.
///
/// It orders, compares and serializes as its text.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterName(Cow<'static, str>);

impl AdapterName {
    /// An adapter's own name, which is never blank.
    pub const fn new_static(name: &'static str) -> Self {
        assert!(!is_blank(name), "an adapter's name is never blank");
        Self(Cow::Borrowed(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `text` is empty or only white space.
const fn is_blank(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if !bytes[at].is_ascii_whitespace() {
            return false;
        }
        at += 1;
    }
    true
}

/// A name as it was written.
impl From<&'static str> for AdapterName {
    fn from(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }
}

/// A name as it was written.
impl From<String> for AdapterName {
    fn from(name: String) -> Self {
        Self(Cow::Owned(name))
    }
}

impl fmt::Display for AdapterName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AdapterName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for AdapterName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for AdapterName {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for AdapterName {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

/// Identifies one environment (interpreter plus dependencies) within a run.
pub type EnvironmentId = String;
/// Identifies one lifetime within a run, e.g. `class:tests/t.py::TestX`.
pub type LifetimeId = String;
/// Dense worker index within a run, from 0.
pub type WorkerId = u32;
/// Dense batch index within a run, from 0.
pub type BatchId = u32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_take_apart_into_path_names_and_params() {
        for (id, path, names, params) in [
            (
                "tests/t.py::TestX::test_y[a-1]",
                "tests/t.py",
                "TestX::test_y",
                Some("a-1"),
            ),
            ("tests/t.py::test_x", "tests/t.py", "test_x", None),
            ("tests/t.py", "tests/t.py", "", None),
            (
                "tests/t.py::test_x[a[1]]",
                "tests/t.py",
                "test_x",
                Some("a[1]"),
            ),
            (
                "tests/t.py::test_x[a::b.c]",
                "tests/t.py",
                "test_x",
                Some("a::b.c"),
            ),
            ("tests/t.py::test_x[a", "tests/t.py", "test_x[a", None),
            (
                "tests/t[1].py::test_x[a]",
                "tests/t[1].py",
                "test_x",
                Some("a"),
            ),
        ] {
            let id = TestId::new(id);
            let parts = id.parts();
            assert_eq!(parts.path, path, "{id}");
            assert_eq!(parts.names, names, "{id}");
            assert_eq!(parts.params, params, "{id}");
        }
    }

    #[test]
    fn an_id_lies_below_its_file_its_names_and_itself_without_params() {
        let ancestors = |id: &str| {
            TestId::new(id)
                .ancestors()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ancestors("t.py::C::test_x[1]"),
            ["t.py", "t.py::C", "t.py::C::test_x"]
        );
        assert_eq!(ancestors("t.py::test_x"), ["t.py"]);
        assert_eq!(ancestors("t.py"), [] as [&str; 0]);
        assert!(TestId::new("t.py").is_file());
        assert!(!TestId::new("t.py::test_x").is_file());
    }

    /// A key prints, serializes and orders as its 16 lower-case digits did
    /// when it was one.
    #[test]
    fn a_key_is_its_sixteen_digits() {
        let keys: Vec<TestKey> = ["", "a", "t.py::test_x", "tests/test_k.py::test_0"]
            .into_iter()
            .map(TestKey::of)
            .collect();
        for key in &keys {
            let text = key.to_string();
            assert_eq!(text.len(), 16);
            assert_eq!(text, text.to_ascii_lowercase());
            assert_eq!(serde_json::to_string(key).unwrap(), format!("\"{text}\""));
            assert_eq!(
                serde_json::from_str::<TestKey>(&format!("\"{text}\"")).unwrap(),
                *key
            );
            assert_eq!(text.parse::<TestKey>().unwrap(), *key);
            assert_eq!(key.prefix(99), text);
        }
        let mut by_key = keys.clone();
        by_key.sort();
        let mut by_text: Vec<String> = keys.iter().map(TestKey::to_string).collect();
        by_text.sort();
        assert_eq!(
            by_key.iter().map(TestKey::to_string).collect::<Vec<_>>(),
            by_text
        );
        assert!("f52a15e9a9b5e89".parse::<TestKey>().is_err());
        assert!("f52a15e9a9b5e89g".parse::<TestKey>().is_err());
    }

    #[test]
    fn a_key_prefix_is_four_to_sixteen_hex_digits() {
        let key = TestKey::of("t.py::test_x");
        let text = key.to_string();
        for digits in [4, 5, 16] {
            let prefix: KeyPrefix = text[..digits].to_ascii_uppercase().parse().unwrap();
            assert_eq!(prefix.as_str(), &text[..digits]);
            assert!(key.starts_with(&prefix));
        }
        let other = if text.starts_with('0') {
            "1000"
        } else {
            "0000"
        };
        assert!(!key.starts_with(&other.parse().unwrap()));
        for text in ["abc", "abcdefabcdefabcde", "abcg"] {
            assert!(text.parse::<KeyPrefix>().is_err(), "{text}");
        }
    }

    #[test]
    fn ids_split_into_path_and_params() {
        let id = TestId::new("tests/t.py::TestX::test_y[a-1]");
        assert_eq!(id.path(), "tests/t.py");
        assert_eq!(id.without_params(), "tests/t.py::TestX::test_y");
        assert_eq!(TestId::new("tests/t.py").without_params(), "tests/t.py");
        assert_eq!(
            TestId::new("tests/t[1].py::test_x").without_params(),
            "tests/t[1].py::test_x"
        );
    }

    /// A `[` in the file's name opens no parameter id: the parameters are
    /// the names' (finding 1 of docs/reorganisation.md).
    #[test]
    fn a_bracket_in_the_path_is_not_a_parameter_id() {
        for (id, without) in [
            ("tests/t[1].py::test_x[a]", "tests/t[1].py::test_x"),
            (
                "tests/t[1].py::TestX::test_y[a-1]",
                "tests/t[1].py::TestX::test_y",
            ),
            ("tests/t[1].py::test_x[a[1]]", "tests/t[1].py::test_x"),
            ("tests/t[1].py", "tests/t[1].py"),
            ("tests/t[1].py::test_x[a", "tests/t[1].py::test_x[a"),
            ("tests/t.py::test_x[a::b]", "tests/t.py::test_x"),
        ] {
            assert_eq!(TestId::new(id).without_params(), without, "{id}");
        }
    }

    /// An adapter's name is its text, borrowed for an adapter's own and
    /// owned for one read back.
    #[test]
    fn an_adapter_name_is_its_text() {
        const PYTEST: AdapterName = AdapterName::new_static("pytest");
        let read: AdapterName = serde_json::from_str("\"pytest\"").unwrap();
        assert_eq!(read, PYTEST);
        assert_eq!(serde_json::to_string(&PYTEST).unwrap(), "\"pytest\"");
        assert_eq!(PYTEST.as_str(), "pytest");
        assert_eq!(PYTEST.to_string(), "pytest");
        assert_eq!(PYTEST, "pytest");
        assert_eq!(AdapterName::from(String::from("pytest")), PYTEST);
        let mut names = [PYTEST, AdapterName::from("doctest"), "unittest".into()];
        names.sort();
        assert_eq!(
            names.map(|name| name.to_string()),
            ["doctest", "pytest", "unittest"]
        );
        let by_name = std::collections::BTreeMap::from([(AdapterName::from("doctest"), 1)]);
        assert_eq!(by_name.get("doctest"), Some(&1));
        assert!(is_blank("") && is_blank(" \t") && !is_blank(" a"));
    }

    #[test]
    fn keys_are_stable_hashes() {
        // FNV-1a 64 of "" and "a" (cbf29ce484222325, af63dc4c8601ec8c), mixed.
        assert_eq!(TestKey::of("").to_string(), "f52a15e9a9b5e89b");
        assert_eq!(TestKey::of("a").to_string(), "02c0bdbf481420f8");
        // Ids that differ in their last byte differ from the first digit.
        assert_eq!(TestKey::of("tests/test_k.py::test_0").prefix(5), "25ece");
        assert_eq!(TestKey::of("tests/test_k.py::test_1").prefix(5), "23ea0");
        assert_eq!(
            TestId::new("t.py::test_x").key(),
            TestKey::of("t.py::test_x")
        );
    }
}
