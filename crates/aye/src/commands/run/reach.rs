//! What a run reaches when some positional arguments aren't paths
//! ([`Reach`]), which decides which of collection's diagnostics are the
//! run's: a dotted name's reach into what collection skipped
//! ([`dotted_reach`]), and the conftests that failed to load
//! ([`broken_conftest_dirs`]), which explain the files below them.
//!
//! Decided (implementation, October 2026):
//! - **Dotted names and keys stand for the files they select.** They are
//!   resolved among what collection finds over the whole project, whose
//!   refinement starts each session from the framework's default (the
//!   adapters prepared seeing only the path arguments). So the run is the
//!   user's run of `pytest <the path arguments and those files>`: every
//!   adapter's context of a batch gets `selected_paths` (the path
//!   arguments, an id's file, and the files of the cases each other
//!   argument selects, absolute, in the order of the arguments), which the
//!   pytest and doctest adapters start their sessions from (start-up
//!   conftests, initial paths); and of collection's diagnostics, a
//!   `skipped` info (a module or directory skipped while collecting, which
//!   the summaries count as a skip) is the run's only where a path argument
//!   holds it or lies in it, or a dotted name names it, its package or
//!   something in it (`pytest --pyargs`: a dotted name naming a skipped
//!   module is found, and no `not-found`), and a `usage-error` on a file (a
//!   conftest refinement's session loaded at start-up) only where a path
//!   argument holds it, its directory holds a path the run starts from, or
//!   some argument named nothing (it may be why). The others are left out
//!   of the stream. With no positional argument, or only paths, collection
//!   walks only what the run reaches, and nothing changes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use aye_events::{Case, Diagnostic, DiagnosticCode, Severity};
use aye_fs::{normalize_absolute, root_relative};
use aye_select::Selection;

use crate::positional::{Positional, Positionals};
use crate::registry;

/// What the positional arguments reach when some of them aren't paths
/// (dotted names, keys): collection then walks the whole project to
/// resolve them, and refinement reports on all of it, its sessions
/// starting from the framework's default (the context the adapters
/// prepared saw only the path arguments). A diagnostic about a place the
/// user's run never meets is not the run's: `pytest <the paths and the
/// selected files>` wouldn't report it. With no positional argument, or
/// only paths, collection walked only what the run reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Reach {
    /// The path arguments, absolute, spelled from the root.
    pub(super) paths: Vec<PathBuf>,
    /// The paths the user's run starts from: each path argument, and the
    /// files of the collected cases each other argument selects, in the
    /// order of the arguments, each once
    /// ([`aye_protocol::BatchContext::selected_paths`]).
    pub(super) selected: Vec<PathBuf>,
    /// The arguments that aren't paths, as given: dotted names and keys.
    pub(super) words: Vec<String>,
    /// Whether some argument named nothing: an error collection reported
    /// elsewhere may be why, and is kept.
    pub(super) unmatched: bool,
}

impl Reach {
    /// `None` when every positional argument is a path, or there is none.
    pub(super) fn of(positionals: &Positionals, root: &Path, cases: &[Case]) -> Option<Self> {
        if positionals.only_paths() {
            return None;
        }
        let at_root = |relative: &str| normalize_absolute(&root.join(relative));
        let mut paths = Vec::new();
        let mut words = Vec::new();
        let mut selected: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for positional in positionals.iter() {
            match positional {
                Positional::Path { absolute: path, .. } | Positional::Id { file: path, .. } => {
                    let path = at_root(&root_relative(path, root));
                    paths.push(path.clone());
                    if seen.insert(path.clone()) {
                        selected.push(path);
                    }
                }
                Positional::Word(word) => {
                    words.push(word.clone());
                    let one = Selection::default().with_ids([word.as_str()]);
                    for case in cases {
                        if one.truth(case).is_false() {
                            continue;
                        }
                        let file = at_root(case.id.path().as_str());
                        if seen.insert(file.clone()) {
                            selected.push(file);
                        }
                    }
                }
            }
        }
        Some(Self {
            paths,
            words,
            selected,
            unmatched: false,
        })
    }

    /// Whether a diagnostic collection reported is the run's. A `skipped`
    /// info (a module or directory the framework skipped, which the
    /// summaries count as a skip) is when a path argument holds it, or lies
    /// in it, or a dotted name reaches it ([`dotted_reach`]): what keys
    /// select are tests, and a skipped module has none. A `usage-error` on
    /// a file (a conftest that failed at start-up in refinement's session)
    /// is when it lies in a path argument, or its directory holds a path
    /// the run starts from (as `pytest <those paths>` loads it first), or
    /// some argument named nothing. Everything else is the run's.
    pub(super) fn keeps(&self, root: &Path, diagnostic: &Diagnostic) -> bool {
        let at = || normalize_absolute(&root.join(diagnostic.path.as_str()));
        match (diagnostic.severity, &diagnostic.code) {
            (Severity::Info, DiagnosticCode::Skipped) => {
                let at = at();
                self.paths
                    .iter()
                    .any(|path| at.starts_with(path) || path.starts_with(&at))
                    || self
                        .words
                        .iter()
                        .any(|word| dotted_reach(word, diagnostic.path.as_str()) != Named::Nothing)
            }
            (Severity::Error, DiagnosticCode::UsageError) if !diagnostic.path.is_nowhere() => {
                if self.unmatched {
                    return true;
                }
                let at = at();
                let dir = if at.is_dir() {
                    at.as_path()
                } else {
                    at.parent().unwrap_or(&at)
                };
                self.paths.iter().any(|path| at.starts_with(path))
                    || self.selected.iter().any(|path| path.starts_with(dir))
            }
            _ => true,
        }
    }
}

/// The directories of the conftests collection reported an error for (a
/// `conftest.py` that failed to load: a support file,
/// [`registry::support_file`]), relative to the root, each with its
/// trailing `/` (the root's is empty).
pub(super) fn broken_conftest_dirs(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .filter_map(|d| {
            let path = d.path.as_str();
            let file = registry::support_file(path)?;
            Some(&path[..path.len() - file.len()])
        })
        .collect()
}

/// Whether a diagnostic is collection's `no-tests` note on a file below a
/// conftest that failed to load (`broken`, from [`broken_conftest_dirs`]):
/// the framework found no tests there because of it, which the conftest's
/// error says (an initial one's session runs without the tests below it;
/// another fails its directory), so the note, which says only that the file
/// has none, is left out.
pub(super) fn explained_by_a_conftest(broken: &[&str], diagnostic: &Diagnostic) -> bool {
    diagnostic.severity == Severity::Info
        && diagnostic.code == DiagnosticCode::NoTests
        && broken
            .iter()
            .any(|dir| diagnostic.path.as_str().starts_with(dir))
}

/// What a dotted name names of a path collection reported skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Named {
    Nothing,
    /// Something the skip holds or that holds it, but not the name's
    /// whole: tests of a skipped module (`mod.test_x`), a module in a
    /// skipped directory. pytest counts the skip and says it found no such
    /// test.
    Part,
    /// The skipped module or package itself, or a package holding it:
    /// pytest counts the skip, and nothing is missing.
    Whole,
}

/// How the dotted name `word` (`pkg.mod.Class.test`, read from the root as
/// `aye_select::Selector` reads one: `pkg/mod.py` then the names in it, or
/// the package `pkg/mod`; `__init__` left out) reaches `path`, which
/// collection reported skipped (a module, or a directory a conftest
/// skipped), relative to the root. A word that isn't a dotted name (a key,
/// an id) names nothing.
pub(super) fn dotted_reach(word: &str, path: &str) -> Named {
    if word.contains("::") || word.contains('/') {
        return Named::Nothing;
    }
    let parts: Vec<&str> = word.split('.').filter(|part| *part != "__init__").collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return Named::Nothing;
    }
    let inside = |inner: &str, outer: &str| {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    let mut named = Named::Nothing;
    for k in 1..=parts.len() {
        let base = parts[..k].join("/");
        let module = format!("{base}.py");
        let whole = k == parts.len();
        if whole && (path == base || path == module || inside(path, &base)) {
            return Named::Whole;
        }
        if path == module || inside(&module, path) || (whole && inside(&base, path)) {
            named = Named::Part;
        }
    }
    named
}

#[cfg(test)]
mod tests;
