//! Lifetimes: what the log calls one, and the scope a resource lives for
//! and a lifetime opens.

use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::id::TestKey;
use crate::path::ProjectPathRef;
use crate::serde_ext::string_enum;

/// What the log calls a lifetime: the id its `lifetime_started` and
/// `lifetime_finished` carry, and a test it kept from running names as its
/// `cause`. A lifetime that starts again in another batch has the same id.
///
/// The adapters spell it three ways, kept as they are written (decision 6
/// of docs/reorganisation.md):
/// - a file, then the names in it, as a test's id starts: unittest's module
///   lifetime is its file (`tests/t.py`), and a class's is its module's id
///   and the class's names (`tests/t.py::A::B`);
/// - a kind, a `:`, and what it names: unittest's module whose file is not
///   the project's is `module:NAME`, NAME its dotted name, and its classes'
///   ids follow from it (`module:pkg.mod::A`); the docs' examples write
///   `class:tests/t.py::TestX`;
/// - [`LifetimeId::SESSION`], which the native engine gives the session and
///   package lifetimes it closes after the last test.
///
/// An id is never rejected: what a worker sends flows as it is. It orders,
/// hashes and compares as its text, and serializes as it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LifetimeId(Box<str>);

impl LifetimeId {
    /// The native engine's session lifetime, and its packages'.
    pub const SESSION: &'static str = "session";

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file the id names: after its kind, if it has one, up to the
    /// first `::` (`class:tests/t.py::TestX` → `tests/t.py`). An id that
    /// names no file reads as what it spells there, as the renderers have
    /// always looked a lifetime's file up: `module:pkg.mod` as `pkg.mod`,
    /// and [`LifetimeId::SESSION`] as `session`.
    pub fn file(&self) -> ProjectPathRef<'_> {
        let (_, rest) = split_kind(&self.0);
        ProjectPathRef::as_written(rest.split_once("::").map_or(rest, |(path, _)| path))
    }

    /// Its key, which the renderers show beside it, as they show a test's.
    pub fn key(&self) -> TestKey {
        TestKey::of(&self.0)
    }
}

/// `text`'s kind and what follows it, when it starts with one: lower-case
/// ASCII letters and a `:` that doesn't open a `::` (`class:t.py::A`).
pub(crate) fn split_kind(text: &str) -> (Option<&str>, &str) {
    match text.split_once(':') {
        Some((kind, rest))
            if !kind.is_empty()
                && !rest.starts_with(':')
                && kind.bytes().all(|b| b.is_ascii_lowercase()) =>
        {
            (Some(kind), rest)
        }
        _ => (None, text),
    }
}

/// An id as it was written.
impl From<&str> for LifetimeId {
    fn from(id: &str) -> Self {
        Self(id.into())
    }
}

/// An id as it was written.
impl From<String> for LifetimeId {
    fn from(id: String) -> Self {
        Self(id.into_boxed_str())
    }
}

impl fmt::Display for LifetimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for LifetimeId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for LifetimeId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for LifetimeId {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for LifetimeId {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

string_enum! {
    /// How long something set up for tests lives: a resource's scope
    /// (pytest's fixture scopes), and the kind of a lifetime, which is the
    /// scope it opens (a lifetime never opens `function`). It orders widest
    /// first, as a resource may depend only on resources of its own scope or
    /// wider ones, and a scope this aye doesn't know comes last.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Scope {
        /// The framework's session: once per worker, and kept from one batch
        /// to the next.
        Session = "session",
        /// A package and what lies below it.
        Package = "package",
        /// A module.
        Module = "module",
        /// A class.
        Class = "class",
        /// One test.
        Function = "function",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every spelling keeps its text, and names the file the renderers
    /// looked up for it with 0.0.1's `file_of`.
    #[test]
    fn a_lifetime_id_is_its_text_and_names_a_file() {
        for (id, file) in [
            ("tests/t.py", "tests/t.py"),
            ("tests/t.py::A::B", "tests/t.py"),
            ("module:pkg.mod", "pkg.mod"),
            ("module:pkg.mod::A", "pkg.mod"),
            ("class:tests/t.py::TestX", "tests/t.py"),
            (LifetimeId::SESSION, "session"),
            ("a::b:c", "a"),
            (":t.py::A", ":t.py"),
            ("Kind:t.py", "Kind:t.py"),
            ("c::t.py", "c"),
            ("", ""),
        ] {
            let lifetime = LifetimeId::from(id);
            assert_eq!(lifetime.file(), file, "{id}");
            assert_eq!(lifetime.as_str(), id);
            assert_eq!(lifetime.to_string(), id);
            assert_eq!(lifetime.key(), TestKey::of(id));
            let json = serde_json::to_string(&lifetime).unwrap();
            assert_eq!(json, serde_json::to_string(id).unwrap());
            assert_eq!(serde_json::from_str::<LifetimeId>(&json).unwrap(), id);
        }
        let mut ids = [
            LifetimeId::from("t.py::B"),
            "session".into(),
            "module:t".into(),
            "t.py".into(),
        ];
        ids.sort();
        assert_eq!(
            ids.map(|id| id.to_string()),
            ["module:t", "session", "t.py", "t.py::B"]
        );
    }

    /// Each scope is its lower-case name, widest first; one no variant
    /// spells keeps its text and comes last.
    #[test]
    fn scopes_are_their_names_widest_first() {
        let names: Vec<&str> = Scope::ALL.iter().map(Scope::as_str).collect();
        assert_eq!(names, ["session", "package", "module", "class", "function"]);
        assert!(Scope::ALL.is_sorted());
        for scope in Scope::ALL {
            let json = serde_json::to_string(scope).unwrap();
            assert_eq!(json, format!("\"{scope}\""));
            assert_eq!(serde_json::from_str::<Scope>(&json).unwrap(), *scope);
            assert_eq!(Scope::from(scope.as_str()), *scope);
        }
        let other: Scope = serde_json::from_str("\"Dynamic\"").unwrap();
        assert_eq!(other, Scope::Other("Dynamic".into()));
        assert_eq!(serde_json::to_string(&other).unwrap(), "\"Dynamic\"");
        assert!(Scope::Function < other);
    }
}
