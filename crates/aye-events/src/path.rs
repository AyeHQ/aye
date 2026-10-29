//! Paths and places in the project's files: the project-relative path,
//! spans and locations.

use std::borrow::Borrow;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A path in the project, as aye's ids, spans, diagnostics and stored runs
/// spell it: relative to aye's root, with `/` separators
/// (`tests/test_cart.py`). A path outside the root climbs out of it with
/// leading `..` parts (`../lib/test_x.py`).
///
/// [`ProjectPath::new`] checks that spelling. What aye reads is kept as it
/// was written instead ([`ProjectPath::as_written`]), so that no record is
/// lost over its path and each writes back byte for byte, and so is what a
/// relativizer gives for a path it can't make relative: a path outside the
/// root that shares only the filesystem's root with it is spelled absolute
/// by the binary's relativizer and on another drive by collection's
/// (finding 3 of docs/reorganisation.md), and a worker spells absolute a
/// method unittest inherits from outside the root (finding 6).
///
/// It orders, hashes and compares as its text, and serializes as it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectPath(Box<str>);

impl ProjectPath {
    /// `path`, if it is spelled as a project path: not empty, not `.` (the
    /// root itself), not absolute (a leading `/` or `\`, or a drive), its
    /// `/`-separated parts never empty or `.`, and `..` only before its first
    /// name. A `\` inside it is a character of a name, as it is on Unix.
    pub fn new(path: impl Into<Box<str>>) -> Result<Self, InvalidProjectPath> {
        let path = path.into();
        match misspelling(&path) {
            None => Ok(Self(path)),
            Some(why) => Err(InvalidProjectPath { path, why }),
        }
    }

    /// `path` as a worker, a stored log or a relativizer wrote it, whatever
    /// its spelling (see the type's docs).
    pub fn as_written(path: impl Into<Box<str>>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path borrowed, for the queries both forms share.
    pub fn as_path(&self) -> ProjectPathRef<'_> {
        ProjectPathRef(&self.0)
    }

    pub fn into_string(self) -> String {
        self.0.into_string()
    }
}

impl fmt::Display for ProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ProjectPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for ProjectPath {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for ProjectPath {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for ProjectPath {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl From<ProjectPathRef<'_>> for ProjectPath {
    fn from(path: ProjectPathRef<'_>) -> Self {
        Self(path.0.into())
    }
}

impl Serialize for ProjectPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

/// Never rejects: a path is read as it was written.
impl<'de> Deserialize<'de> for ProjectPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Box::<str>::deserialize(deserializer).map(Self)
    }
}

/// A [`ProjectPath`] borrowed: from one, or from the text of a test id
/// (whose path a worker may spell any way, like the rest of the id).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectPathRef<'a>(&'a str);

impl<'a> ProjectPathRef<'a> {
    /// `path` as it was written, unchecked, as [`ProjectPath::as_written`].
    pub fn as_written(path: &'a str) -> Self {
        Self(path)
    }

    pub fn as_str(self) -> &'a str {
        self.0
    }

    pub fn to_project_path(self) -> ProjectPath {
        ProjectPath::from(self)
    }

    /// Its `/`-separated parts, empty ones (an absolute path's leading `/`)
    /// left out.
    pub fn components(self) -> impl DoubleEndedIterator<Item = &'a str> {
        self.0.split('/').filter(|part| !part.is_empty())
    }

    /// Its last part: the file's or the directory's own name.
    pub fn file_name(self) -> &'a str {
        self.0.rsplit('/').next().unwrap_or(self.0)
    }

    /// The directory it is in, when it names one: `tests/unit/t.py` is in
    /// `tests/unit`, `t.py` in none.
    pub fn parent(self) -> Option<Self> {
        self.0.rsplit_once('/').map(|(parent, _)| Self(parent))
    }

    /// Whether it is `dir`, or lies below it, by whole parts: `tests/t.py`
    /// is in `tests`, not in `te`.
    pub fn starts_with_dir(self, dir: ProjectPathRef<'_>) -> bool {
        self.0
            .strip_prefix(dir.0)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

impl fmt::Display for ProjectPathRef<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl PartialEq<str> for ProjectPathRef<'_> {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl PartialEq<&str> for ProjectPathRef<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<ProjectPath> for ProjectPathRef<'_> {
    fn eq(&self, other: &ProjectPath) -> bool {
        self.0 == &*other.0
    }
}

impl PartialEq<ProjectPathRef<'_>> for ProjectPath {
    fn eq(&self, other: &ProjectPathRef<'_>) -> bool {
        &*self.0 == other.0
    }
}

/// Why a text is not spelled as a [`ProjectPath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidProjectPath {
    path: Box<str>,
    why: &'static str,
}

impl InvalidProjectPath {
    /// The text, back.
    pub fn into_path(self) -> Box<str> {
        self.path
    }
}

impl fmt::Display for InvalidProjectPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a project path: {}", self.path, self.why)
    }
}

impl std::error::Error for InvalidProjectPath {}

/// What keeps `path` from being spelled as a [`ProjectPath`], if anything.
fn misspelling(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("it is empty");
    }
    if path == "." {
        return Some("it is the root itself");
    }
    let drive = path.as_bytes().get(1) == Some(&b':');
    if path.starts_with(['/', '\\']) || drive {
        return Some("it is absolute");
    }
    let mut named = false;
    for part in path.split('/') {
        match part {
            "" => return Some("it has an empty part"),
            "." => return Some("it has a `.` part"),
            ".." if named => return Some("it climbs back out with `..`"),
            ".." => {}
            _ => named = true,
        }
    }
    None
}

/// A file and a byte range in it. Separate from ids: failure locations,
/// snapshots and the source rewriter all point at spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// The file, relative to the project root.
    pub path: ProjectPath,
    pub start: u32,
    pub end: u32,
    /// 1-based line of `start`, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A file and a 1-based line, where only a line is known (traceback frames).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_path_is_relative_to_the_root_with_slashes() {
        for path in [
            "tests/test_cart.py",
            "t.py",
            "../lib/t.py",
            "..",
            "a\\b.py",
            "t[1].py",
        ] {
            assert_eq!(ProjectPath::new(path).unwrap().as_str(), path);
        }
        for (path, why) in [
            ("", "it is empty"),
            (".", "it is the root itself"),
            ("/abs/t.py", "it is absolute"),
            ("C:/abs/t.py", "it is absolute"),
            ("tests//t.py", "it has an empty part"),
            ("tests/", "it has an empty part"),
            ("./t.py", "it has a `.` part"),
            ("\\abs\\t.py", "it is absolute"),
            ("tests/../t.py", "it climbs back out with `..`"),
        ] {
            let err = ProjectPath::new(path).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("`{path}` is not a project path: {why}")
            );
        }
    }

    /// What a worker or a stored log wrote reads back as it was, absolute
    /// or empty included.
    #[test]
    fn a_path_reads_and_writes_as_written() {
        for text in [
            r#""tests/t.py""#,
            r#""/abs/t.py""#,
            r#""""#,
            r#""C:\\x\\t.py""#,
        ] {
            let path: ProjectPath = serde_json::from_str(text).unwrap();
            assert_eq!(serde_json::to_string(&path).unwrap(), text);
        }
    }

    #[test]
    fn its_parts_are_whole_names() {
        let path = ProjectPath::new("tests/unit/test_x.py").unwrap();
        let path = path.as_path();
        assert_eq!(
            path.components().collect::<Vec<_>>(),
            ["tests", "unit", "test_x.py"]
        );
        assert_eq!(path.file_name(), "test_x.py");
        assert_eq!(path.parent().unwrap(), "tests/unit");
        assert_eq!(ProjectPathRef::as_written("t.py").parent(), None);
        assert!(path.starts_with_dir(ProjectPathRef::as_written("tests")));
        assert!(path.starts_with_dir(ProjectPathRef::as_written("tests/unit/test_x.py")));
        assert!(!path.starts_with_dir(ProjectPathRef::as_written("te")));
        assert_eq!(
            ProjectPathRef::as_written("/abs/t.py")
                .components()
                .collect::<Vec<_>>(),
            ["abs", "t.py"]
        );
    }

    #[test]
    fn it_orders_as_its_text() {
        let mut paths: Vec<ProjectPath> = ["b.py", "a/z.py", "a.py", "a/b.py"]
            .into_iter()
            .map(ProjectPath::as_written)
            .collect();
        paths.sort();
        assert_eq!(paths, ["a.py", "a/b.py", "a/z.py", "b.py"]);
    }
}
