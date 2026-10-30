//! Affinity keys: what a case names of the lifetimes it runs inside, by
//! which the runner keeps the cases that share one together.

use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::lifetime::split_kind;
use crate::path::ProjectPathRef;

/// A key of a lifetime a test runs inside, as its case declares it
/// ([`Case::affinity`](crate::Case::affinity), outermost first). The runner
/// keeps the cases that share their first key in one batch, so that the
/// lifetime is set up once for them, and reads nothing else in a key.
///
/// The adapters spell their keys as the constructors below do, as they
/// always have (decision 6 of docs/reorganisation.md), in Rust and in the
/// Python their workers run, though two may name one lifetime differently:
/// the pytest adapter's front-end writes `module:PATH` for the module the
/// unittest adapter writes as `PATH`. The unittest adapter's file and class
/// keys are the ids of its module and class lifetimes. A key a worker, a
/// stored log or the refinement cache holds is kept as it was written.
///
/// It orders, hashes and compares as its text, and serializes as it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AffinityKey(Box<str>);

impl AffinityKey {
    /// `import:NAME`: the name a module is imported under, which two files
    /// can't share in one session (the pytest and doctest adapters').
    pub fn import(module: &str) -> Self {
        Self(format!("import:{module}").into_boxed_str())
    }

    /// `package:DIR`: a package whose `__init__.py` may define
    /// `load_tests`, which decides the tests below it (the unittest
    /// adapter's).
    pub fn package(dir: ProjectPathRef<'_>) -> Self {
        Self::spelled("package:", dir, &[] as &[&str])
    }

    /// `module:PATH`: a module whose setup runs the user's code (the pytest
    /// adapter's front-end's).
    pub fn module(file: ProjectPathRef<'_>) -> Self {
        Self::spelled("module:", file, &[] as &[&str])
    }

    /// `class:PATH::A::B`: a class whose setup runs the user's code, by its
    /// file and its names, outermost first (the pytest adapter's
    /// front-end's).
    pub fn class(file: ProjectPathRef<'_>, names: &[impl AsRef<str>]) -> Self {
        Self::spelled("class:", file, names)
    }

    /// `PATH`: a module, by its file alone (the unittest and doctest
    /// adapters', and the pytest adapter's static prediction).
    pub fn file(file: ProjectPathRef<'_>) -> Self {
        Self::spelled("", file, &[] as &[&str])
    }

    /// `PATH::A::B`: a class, by its file and its names, outermost first
    /// (the unittest adapter's, and the pytest adapter's static
    /// prediction).
    pub fn file_class(file: ProjectPathRef<'_>, names: &[impl AsRef<str>]) -> Self {
        Self::spelled("", file, names)
    }

    fn spelled(kind: &str, path: ProjectPathRef<'_>, names: &[impl AsRef<str>]) -> Self {
        let mut key = String::from(kind);
        key.push_str(path.as_str());
        for name in names {
            key.push_str("::");
            key.push_str(name.as_ref());
        }
        Self(key.into_boxed_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key taken apart, as the constructor that spells it put it
    /// together.
    pub fn kind(&self) -> AffinityKind<'_> {
        match split_kind(&self.0) {
            (Some("import"), module) => AffinityKind::Import(module),
            (Some("package"), dir) => AffinityKind::Package(ProjectPathRef::as_written(dir)),
            (Some("module"), file) => AffinityKind::Module(ProjectPathRef::as_written(file)),
            (Some("class"), rest) => {
                let (file, names) = file_and_names(rest);
                AffinityKind::Class(file, names)
            }
            (Some(_), _) => AffinityKind::Other(&self.0),
            (None, text) => match file_and_names(text) {
                (file, "") => AffinityKind::File(file),
                (file, names) => AffinityKind::FileClass(file, names),
            },
        }
    }
}

/// A file, then its names after the first `::`, if any.
fn file_and_names(text: &str) -> (ProjectPathRef<'_>, &str) {
    match text.split_once("::") {
        Some((file, names)) => (ProjectPathRef::as_written(file), names),
        None => (ProjectPathRef::as_written(text), ""),
    }
}

/// An affinity key taken apart: the spelling it has, and what it names.
/// Names are `::`-separated, outermost first (`A::B`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AffinityKind<'a> {
    /// `import:NAME`.
    Import(&'a str),
    /// `package:DIR`.
    Package(ProjectPathRef<'a>),
    /// `module:PATH`.
    Module(ProjectPathRef<'a>),
    /// `class:PATH::A::B`.
    Class(ProjectPathRef<'a>, &'a str),
    /// `PATH`.
    File(ProjectPathRef<'a>),
    /// `PATH::A::B`.
    FileClass(ProjectPathRef<'a>, &'a str),
    /// A kind no constructor writes (lower-case letters and a `:`, then
    /// anything), the whole key.
    Other(&'a str),
}

/// A key as it was written.
impl From<&str> for AffinityKey {
    fn from(key: &str) -> Self {
        Self(key.into())
    }
}

/// A key as it was written.
impl From<String> for AffinityKey {
    fn from(key: String) -> Self {
        Self(key.into_boxed_str())
    }
}

impl fmt::Display for AffinityKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for AffinityKey {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for AffinityKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for AffinityKey {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for AffinityKey {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each constructor writes the spelling its adapters always have, and
    /// the key reads back as it.
    #[test]
    fn keys_are_spelled_as_the_adapters_spell_them() {
        let path = ProjectPathRef::as_written;
        let file = path("tests/t.py");
        let dir = path("tests");
        for (key, text, kind) in [
            (
                AffinityKey::import("pkg.test_t"),
                "import:pkg.test_t",
                AffinityKind::Import("pkg.test_t"),
            ),
            (
                AffinityKey::package(dir),
                "package:tests",
                AffinityKind::Package(dir),
            ),
            (
                AffinityKey::module(file),
                "module:tests/t.py",
                AffinityKind::Module(file),
            ),
            (
                AffinityKey::class(file, &["A", "B"]),
                "class:tests/t.py::A::B",
                AffinityKind::Class(file, "A::B"),
            ),
            (
                AffinityKey::file(file),
                "tests/t.py",
                AffinityKind::File(file),
            ),
            (
                AffinityKey::file_class(file, &[String::from("A")]),
                "tests/t.py::A",
                AffinityKind::FileClass(file, "A"),
            ),
        ] {
            assert_eq!(key, text);
            assert_eq!(key.to_string(), text);
            assert_eq!(key.kind(), kind, "{text}");
            assert_eq!(AffinityKey::from(text), key);
            let json = serde_json::to_string(&key).unwrap();
            assert_eq!(json, serde_json::to_string(text).unwrap());
            assert_eq!(serde_json::from_str::<AffinityKey>(&json).unwrap(), key);
        }
        for (text, kind) in [
            ("shared-db", AffinityKind::File(path("shared-db"))),
            ("local:x", AffinityKind::Other("local:x")),
            ("Import:x", AffinityKind::File(path("Import:x"))),
            ("a::b:c", AffinityKind::FileClass(path("a"), "b:c")),
        ] {
            assert_eq!(AffinityKey::from(text).kind(), kind, "{text}");
        }
    }

    /// Keys order and compare as their text.
    #[test]
    fn keys_order_as_their_text() {
        let mut keys = [
            AffinityKey::from("t.py::B"),
            "import:t".into(),
            "t.py".into(),
            "module:t.py".into(),
        ];
        keys.sort();
        assert_eq!(
            keys.map(|key| key.to_string()),
            ["import:t", "module:t.py", "t.py", "t.py::B"]
        );
        assert_ne!(AffinityKey::from("module:t.py"), AffinityKey::from("t.py"));
    }
}
