//! What failed last time ([`LastFailed`], each a [`FailedEntry`]), and
//! last failed (`--lf`): the cases kept because they, their files or some
//! of their parameter ids failed last time, and which of the others
//! pytest's summary counts as deselected.

use std::collections::{HashMap, HashSet};

use aye_events::{Case, Certainty, ProjectPath, TestId};

/// One thing that failed last time, as history keeps it (pytest's
/// `lastfailed`): a test, or a file or directory that failed to collect.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FailedEntry {
    Test(TestId),
    /// History keeps it as an id without `::`: its path.
    Path(ProjectPath),
}

impl FailedEntry {
    /// The entry as history writes it: the test's id, or the path.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Test(id) => id.as_str(),
            Self::Path(path) => path.as_str(),
        }
    }
}

impl From<TestId> for FailedEntry {
    /// An id as history keeps it: one without `::` names a file or
    /// directory ([`TestId::is_file`]).
    fn from(id: TestId) -> Self {
        if id.is_file() {
            Self::Path(ProjectPath::as_written(id.into_string()))
        } else {
            Self::Test(id)
        }
    }
}

/// What failed last time, for `--lf` and `--ff`: the tests, and the files
/// and directories that failed to collect, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastFailed {
    tests: HashSet<TestId>,
    paths: HashSet<ProjectPath>,
}

impl LastFailed {
    /// Add `entry`; one already there stays once.
    pub fn insert(&mut self, entry: impl Into<FailedEntry>) {
        match entry.into() {
            FailedEntry::Test(id) => self.tests.insert(id),
            FailedEntry::Path(path) => self.paths.insert(path),
        };
    }

    pub fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.paths.is_empty()
    }

    /// Whether `id` failed last time, as an entry's own: a test's id, or
    /// the path of what failed to collect when `id` has no `::`.
    pub fn contains(&self, id: &TestId) -> bool {
        if id.is_file() {
            self.paths.contains(id.as_str())
        } else {
            self.tests.contains(id)
        }
    }

    /// Every entry as an id, as history keeps it: the form the runner takes
    /// what runs first in ([`Order::first`](crate::Order::first)).
    pub fn ids(&self) -> HashSet<TestId> {
        self.tests
            .iter()
            .cloned()
            .chain(self.paths.iter().map(|path| TestId::new(path.as_str())))
            .collect()
    }
}

impl<E: Into<FailedEntry>> FromIterator<E> for LastFailed {
    fn from_iter<I: IntoIterator<Item = E>>(entries: I) -> Self {
        let mut failed = Self::default();
        for entry in entries {
            failed.insert(entry);
        }
        failed
    }
}

/// What [`Selection::apply`](crate::Selection::apply) remembers of a case the positional arguments
/// select.
pub(crate) struct Seen {
    /// Its file.
    pub(crate) path: String,
    /// Whether it is a module-level test (a single name after the path),
    /// rather than one inside a class.
    pub(crate) top: bool,
    /// Whether it failed last time.
    pub(crate) failed: bool,
    /// Whether it is kept in the end.
    pub(crate) kept: bool,
}

/// Which cases pytest's summary can count as deselected under `--lf`:
/// pytest's last-failed plugin drops most of what it doesn't rerun while
/// collecting, before anything is deselected (`_pytest/cacheprovider.py`).
/// Once collection reaches a file holding a module-level test that failed
/// (files on a failed path come first), files that held no failure are
/// skipped whole, and in each file that held one, module-level tests that
/// didn't fail are dropped unless the file was named on the command line;
/// tests inside classes stay. Only what is left reaches `-k`, `-m` and the
/// plugin's own deselection, and is counted.
pub(crate) struct Visible {
    /// Files on a failed path whose collection is filtered.
    filtered: HashSet<String>,
    /// Files on a failed path.
    failing: HashSet<String>,
    /// Whether any file was filtered, so files without failures are skipped
    /// (they come after the failing ones).
    skipping: bool,
    /// Files a positional argument names (pytest's "initial paths").
    named: HashSet<String>,
}

impl Visible {
    pub(crate) fn new(
        failed: &Failed<'_>,
        within: &[Seen],
        direct: &HashSet<String>,
        names_file: impl Fn(&str) -> bool,
    ) -> Self {
        let mut filtered = HashSet::new();
        let mut failing = HashSet::new();
        let mut named = HashSet::new();
        let mut skipping = false;
        for seen in within {
            if !failed.files.contains(seen.path.as_str()) || !failing.insert(seen.path.clone()) {
                continue;
            }
            if skipping || direct.contains(&seen.path) {
                skipping = true;
                filtered.insert(seen.path.clone());
            }
            if names_file(&seen.path) {
                named.insert(seen.path.clone());
            }
        }
        Self {
            filtered,
            failing,
            skipping,
            named,
        }
    }

    /// Whether pytest's collection keeps the case, so that dropping it
    /// counts as deselecting it.
    pub(crate) fn sees(&self, seen: &Seen) -> bool {
        if !self.failing.contains(&seen.path) {
            return !self.skipping;
        }
        !self.filtered.contains(&seen.path)
            || seen.failed
            || !seen.top
            || self.named.contains(&seen.path)
    }
}

/// What failed last time, indexed for matching cases.
pub(crate) struct Failed<'a> {
    last: &'a LastFailed,
    /// The files and directories that failed to collect, as path
    /// components.
    paths: Vec<Vec<&'a str>>,
    /// Parametrized ids, by the id without their parameter id.
    params: HashMap<&'a str, Vec<&'a TestId>>,
    /// The file (or directory) part of every id.
    files: HashSet<&'a str>,
}

impl<'a> Failed<'a> {
    pub(crate) fn new(last: &'a LastFailed) -> Self {
        let mut paths = Vec::new();
        let mut params: HashMap<&str, Vec<&TestId>> = HashMap::new();
        let mut files = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills lookups; what reaches output is sorted"
        )]
        for path in &last.paths {
            files.insert(path.as_str());
            let path: Vec<&str> = path.as_path().components().collect();
            if !path.is_empty() {
                paths.push(path);
            }
        }
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills lookups; what reaches output is sorted"
        )]
        for id in &last.tests {
            files.insert(id.path().as_str());
            if id.parts().params.is_some() {
                params.entry(id.without_params()).or_default().push(id);
            }
        }
        Self {
            last,
            paths,
            params,
            files,
        }
    }

    /// Whether the case's own id failed last time, or, for a `Partial`
    /// case, some of its parameter ids (pytest's `nodeid in lastfailed`).
    pub(crate) fn names(&self, case: &Case) -> bool {
        self.last.contains(&case.id)
            || (case.certainty == Certainty::Partial && self.params.contains_key(case.id.as_str()))
    }

    /// `None` when the case did not fail last time; otherwise the parameter
    /// ids to narrow it to, empty for the whole case.
    pub(crate) fn judge(&self, case: &Case, narrow: &[TestId]) -> Option<Vec<TestId>> {
        let whole = self.last.contains(&case.id)
            || (!self.paths.is_empty() && {
                let components: Vec<&str> = case.id.path().as_str().split('/').collect();
                self.paths.iter().any(|path| components.starts_with(path))
            });
        if !narrow.is_empty() {
            if whole {
                return Some(narrow.to_vec());
            }
            let failed: Vec<TestId> = narrow
                .iter()
                .filter(|id| self.last.contains(id))
                .cloned()
                .collect();
            return (!failed.is_empty()).then_some(failed);
        }
        if whole {
            return Some(Vec::new());
        }
        if case.certainty != Certainty::Partial {
            return None;
        }
        let mut failed: Vec<TestId> = self
            .params
            .get(case.id.as_str())?
            .iter()
            .map(|id| (*id).clone())
            .collect();
        failed.sort();
        Some(failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An id with `::` is a test's; one without, what failed to collect,
    /// and each is found only as itself.
    #[test]
    fn an_entry_is_a_test_or_a_path_that_failed_to_collect() {
        assert_eq!(
            FailedEntry::from(TestId::new("t.py::test_a[1]")),
            FailedEntry::Test(TestId::new("t.py::test_a[1]"))
        );
        for path in ["tests/test_b.py", "tests", ""] {
            let entry = FailedEntry::from(TestId::new(path));
            assert_eq!(entry, FailedEntry::Path(ProjectPath::as_written(path)));
            assert_eq!(entry.as_str(), path);
        }
        let last: LastFailed = [
            FailedEntry::from(TestId::new("t.py::test_a")),
            FailedEntry::Path(ProjectPath::as_written("tests/test_b.py")),
            FailedEntry::Test(TestId::new("t.py::test_a")),
        ]
        .into_iter()
        .collect();
        assert!(!last.is_empty() && LastFailed::default().is_empty());
        for (id, failed) in [
            ("t.py::test_a", true),
            ("tests/test_b.py", true),
            ("t.py::test_a[1]", false),
            ("tests/test_b.py::test_x", false),
            ("t.py", false),
        ] {
            assert_eq!(last.contains(&TestId::new(id)), failed, "{id}");
        }
        assert_eq!(
            last.ids(),
            HashSet::from([TestId::new("t.py::test_a"), TestId::new("tests/test_b.py")])
        );
    }
}
