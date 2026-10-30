//! pytest's conftests: which directories' conftests pytest loads (within
//! its confcutdir) and which it loads before `pytest_configure` (its
//! initial conftests), as each adapter reads them; and, as the doctest
//! adapter needs them, which ones pytest has imported by the time it
//! collects a file (the executor imports them too, for their side effects),
//! which define hooks that may mark doctest items, and which `conftest.py`,
//! collected as a doctest module, pytest finds already imported under the
//! name `conftest` from another file (its "import file mismatch").
//!
//! The two adapters' initial conftests differ: the pytest adapter's
//! [`initial_dirs`] are directories, with their `test*` subdirectories in
//! the order the filesystem lists them, matched case-insensitively on
//! Windows; the doctest adapter's [`Conftests::initial`] are the conftest
//! files themselves, with `test*` directories sorted by name and matched
//! case-sensitively.

use std::path::{Path, PathBuf};

use aye_python_stdlib::identifier::is_ascii_identifier;
use aye_python_stdlib::os_path::normalize_absolute;

/// Hooks that may add marks to doctest items: `-k` and `-m` can't be
/// evaluated on the source alone where one is in scope.
pub const MARKING_HOOKS: [&str; 2] = ["pytest_collection_modifyitems", "pytest_itemcollected"];

/// pytest's `_is_in_confcutdir`: a directory strictly above the confcutdir
/// does not load its conftest.
pub fn in_confcutdir(confcutdir: &Path, dir: &Path) -> bool {
    !(confcutdir.starts_with(dir) && confcutdir != dir)
}

/// The directories pytest loads conftests for before `pytest_configure`
/// (`_set_initial_conftests`): each existing collection argument's (its
/// own, for a directory, and its `test*` subdirectories; its parent, for a
/// file), or else the current directory's. A conftest in one of them or
/// above is "initial".
pub fn initial_dirs(cwd: &Path, args: &[PathBuf]) -> Vec<PathBuf> {
    let test_dirs = |dir: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                // `Path.glob("test*")`: case-insensitive on Windows only.
                if cfg!(windows) {
                    name.to_lowercase().starts_with("test")
                } else {
                    name.starts_with("test")
                }
            })
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    };
    let mut anchors = Vec::new();
    for arg in args {
        let arg = normalize_absolute(arg);
        if arg.is_dir() {
            anchors.extend(test_dirs(&arg));
            anchors.push(arg);
        } else if arg.exists()
            && let Some(parent) = arg.parent()
        {
            anchors.push(parent.to_path_buf());
        }
    }
    if anchors.is_empty() {
        anchors.push(cwd.to_path_buf());
        anchors.extend(test_dirs(cwd));
    }
    anchors
}

/// pytest's conftest rules for one run.
#[derive(Debug, Clone)]
pub struct Conftests {
    /// No conftest is imported (`--noconftest`).
    pub disabled: bool,
    /// pytest's confcutdir: no conftest above it is imported.
    pub confcutdir: PathBuf,
}

impl Conftests {
    /// pytest's `_is_in_confcutdir`: not strictly above the confcutdir.
    pub fn in_confcutdir(&self, dir: &Path) -> bool {
        !self
            .confcutdir
            .ancestors()
            .skip(1)
            .any(|above| above == dir)
    }

    /// The conftests `_loadconftestmodules` imports for `dir`: each
    /// directory's from the top down, within the confcutdir.
    pub fn chain(&self, dir: &Path) -> Vec<PathBuf> {
        if self.disabled {
            return Vec::new();
        }
        let mut dirs: Vec<&Path> = dir.ancestors().collect();
        dirs.reverse();
        dirs.into_iter()
            .filter(|dir| self.in_confcutdir(dir))
            .map(|dir| dir.join("conftest.py"))
            .filter(|path| path.is_file())
            .collect()
    }

    /// pytest's initial conftests (`_set_initial_conftests`), in the order
    /// it imports them: for each anchor (the collection roots that exist,
    /// and the `test*` directories of a directory among them; else `cwd`
    /// and its `test*` directories), the chain of its directory.
    pub fn initial(&self, roots: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
        if self.disabled {
            return Vec::new();
        }
        let mut anchors = Vec::new();
        let add = |anchor: &Path, anchors: &mut Vec<PathBuf>| {
            anchors.push(anchor.to_path_buf());
            if anchor.is_dir() {
                anchors.extend(test_dirs(anchor));
            }
        };
        for root in roots.iter().filter(|root| root.exists()) {
            add(root, &mut anchors);
        }
        if anchors.is_empty() {
            add(cwd, &mut anchors);
        }
        let mut out: Vec<PathBuf> = Vec::new();
        for anchor in anchors {
            let dir = if anchor.is_dir() {
                anchor.as_path()
            } else {
                anchor.parent().unwrap_or(&anchor)
            };
            for conftest in self.chain(dir) {
                if !out.contains(&conftest) {
                    out.push(conftest);
                }
            }
        }
        out
    }
}

/// The `test*` directories of `dir`, by name.
fn test_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("test"))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Whether a conftest is in a package (pytest's `resolve_package_path`
/// finds one): pytest imports it as `package.conftest`, not as `conftest`.
pub fn in_package(conftest: &Path) -> bool {
    conftest.parent().is_some_and(|dir| {
        dir.join("__init__.py").is_file()
            && dir.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                // `str.isidentifier()`; names that aren't ASCII count.
                !name.is_ascii() || is_ascii_identifier(&name)
            })
    })
}

/// Whether `source` names `hook` (as a definition, or anywhere: a hook may
/// be bound by assignment).
pub fn names_hook(source: &str, hook: &str) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    source.match_indices(hook).any(|(at, _)| {
        !source[..at].chars().next_back().is_some_and(word)
            && !source[at + hook.len()..].chars().next().is_some_and(word)
    })
}
