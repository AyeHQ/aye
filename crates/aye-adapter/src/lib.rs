//! What an adapter implements. [`Adapter`] maps one framework onto aye's
//! engine: it prepares a [`Recognizer`] for a [`Project`] (or fails with an
//! [`AdapterError`]), and gives the runtime face a worker installs
//! ([`Executor`]). The recognizer is its static face: it [`Claim`]s paths,
//! predicts the tests of a parsed [`SourceFile`] (a [`Recognition`]), and
//! reports the [`Setting`]s it resolved, for `aye config`. Ruff's syntax
//! tree, line index and text ranges are re-exported ([`ast`],
//! [`LineIndex`], [`TextRange`], [`TextSize`], [`Ranged`]), so collection
//! and every adapter name the one version of them aye pins.
//!
//! Adapter → runner: `recognize(file, ast, config) → cases`, `refine(files) →
//! cases` for runtime-dependent files, affinity and scope declarations, the
//! metadata mapping (the framework's markers and decorators expressed as the
//! runner's generic tags, requires, serial, timeout, owner, quarantine), the
//! executor `run(batch, policy, services) → events`, and optional transforms for
//! the runner's import hook.
//!
//! Owns: the traits. Does not own: any framework's rules, which live in the
//! `aye-engine-*` crates; the worker protocol (`aye-protocol`); aye's
//! engine (`aye-engine`).
//!
//! Depends on: aye-events, aye-protocol, and Ruff's `ruff_python_ast`,
//! `ruff_source_file` and `ruff_text_size`, with the `get-size2` pin that
//! goes with them (see the workspace `Cargo.toml`). Must not depend on the
//! runner or any adapter.
//!
//! Open: how rich affinity/scope declarations must be to carry pytest's five
//! fixture scopes and unittest's class/module setup.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The static face is optional.** A recognizer may claim a file and
//!   return [`Recognition::refine`] instead of cases; the worker then imports
//!   the file and reports its tests (refinement). Selection keeps whatever it
//!   can't decide, so a wrong or missing prediction costs planning quality,
//!   never correctness.
//! - **Lifetimes are declared as affinity keys** on each case, outermost
//!   first (`tests/test_cart.py`, `tests/test_cart.py::TestCart`). The
//!   scheduler keeps tests sharing a key in one batch; for adapters that map
//!   fully, the engine opens and closes the lifetimes inside the batch in
//!   that order. This carries unittest's class and module setup exactly and
//!   pytest's module and class scopes approximately; pytest's session and
//!   package scopes are pytest's own business inside a batch.
//! - **An adapter prepares knowing the others** ([`Adapter::prepare_among`],
//!   which the binary calls; its default is [`Adapter::prepare`]): doctest's
//!   default text glob depends on pytest running beside it.

use std::path::{Path, PathBuf};

use aye_events::{AdapterName, Case, Diagnostic, LineNumber, ProjectPathRef, Span};
use aye_protocol::Executor;
use serde::{Deserialize, Serialize};

pub use ruff_python_ast as ast;
pub use ruff_source_file::LineIndex;
pub use ruff_text_size::{Ranged, TextRange, TextSize};

/// What adapters know about the project when they prepare.
#[derive(Debug, Clone, Copy)]
pub struct Project<'a> {
    /// The project root, absolute. Test ids are relative to it.
    pub root: &'a Path,
    /// The directory aye was started in, absolute.
    pub cwd: &'a Path,
    /// Explicit paths from the command line, as given (no `::` parts).
    pub args: &'a [PathBuf],
    /// An explicit framework config file (`-c`), if given.
    pub config_file: Option<&'a Path>,
    /// Arguments for the framework given after `--`, unchanged. Adapters
    /// read what affects collection from them and pass them on to
    /// refinement through their context.
    pub framework_args: &'a [String],
}

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    /// The project's configuration for this framework is invalid.
    #[error("{0}")]
    Config(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// One framework, mapped onto aye's engine.
pub trait Adapter: Send + Sync {
    /// Short, stable name: `pytest`.
    fn name(&self) -> AdapterName;

    /// Read the framework's own configuration and return a recognizer for
    /// this project, or `None` when the framework isn't used here.
    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError>;

    /// [`Adapter::prepare`], knowing the names of every adapter the run
    /// uses (this one included), in priority order: an adapter whose
    /// defaults depend on another framework running beside it (doctest's
    /// text files, which pytest collects by default) reads them here. The
    /// binary calls this one.
    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[AdapterName],
    ) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        let _ = adapters;
        self.prepare(project)
    }

    /// The runtime face, run inside workers.
    fn executor(&self) -> Executor;
}

/// What a recognizer says about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The file defines tests for this adapter.
    Tests,
    /// The file affects other files' tests (pytest's `conftest.py`); it is
    /// parsed and handed to [`Recognizer::recognize`] for files below it.
    Support,
    /// Both: the file defines tests and affects the tests below it (a
    /// pytest `conftest.py` that also matches `python_files`).
    TestsAndSupport,
    /// Not this adapter's business.
    Ignore,
}

impl Claim {
    pub const fn has_tests(self) -> bool {
        matches!(self, Self::Tests | Self::TestsAndSupport)
    }

    pub const fn is_support(self) -> bool {
        matches!(self, Self::Support | Self::TestsAndSupport)
    }
}

/// A parsed source file, as handed to recognizers.
#[derive(Clone, Copy)]
pub struct SourceFile<'a> {
    /// Relative to the project root, `/` separators.
    pub rel: ProjectPathRef<'a>,
    pub abs: &'a Path,
    pub source: &'a str,
    pub module: &'a ast::ModModule,
    pub lines: &'a LineIndex,
}

impl SourceFile<'_> {
    /// The line of an offset; [`LineNumber::MAX`] past it.
    pub fn line(&self, offset: TextSize) -> LineNumber {
        u32::try_from(self.lines.line_index(offset).get())
            .ok()
            .and_then(LineNumber::new)
            .unwrap_or(LineNumber::MAX)
    }

    pub fn span(&self, range: TextRange) -> Span {
        Span {
            path: self.rel.to_project_path(),
            start: range.start().to_u32(),
            end: range.end().to_u32(),
            line: Some(self.line(range.start()).get()),
        }
    }

    pub fn text(&self, range: TextRange) -> &str {
        &self.source[range]
    }
}

/// What a recognizer found in one file.
#[derive(Debug, Clone, Default)]
pub struct Recognition {
    pub cases: Vec<Case>,
    /// Why this file's tests can only be known by importing it. When set,
    /// `cases` holds the best static guess (possibly empty), and collection
    /// asks the worker to refine the file.
    pub refine: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// A resolved setting, with where it came from, for `aye config`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub name: String,
    pub value: String,
    /// `pyproject.toml [tool.pytest.ini_options]`, `default`, …
    pub origin: String,
}

/// An adapter's static face, configured for one project.
pub trait Recognizer: Send + Sync {
    /// Where to look when no paths are given (pytest's `testpaths`, or the
    /// current directory). Absolute.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf>;

    /// Whether to walk into a directory. `dir` is absolute.
    fn descend(&self, dir: &Path) -> bool;

    /// What `rel` is to this adapter. `explicit` is true when the user named
    /// the file on the command line.
    fn claim(&self, rel: &str, explicit: bool) -> Claim;

    /// Predict the tests in `file`. `support` holds the support files that
    /// apply to it (pytest's `conftest.py` files in its directory and above),
    /// outermost first.
    fn recognize(&self, file: &SourceFile<'_>, support: &[SourceFile<'_>]) -> Recognition;

    /// The settings this recognizer resolved, with origins.
    fn settings(&self) -> Vec<Setting> {
        Vec::new()
    }

    /// Adapter-specific context sent with every batch and refinement
    /// (pytest's rootdir and inifile).
    fn context(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
}

#[cfg(test)]
mod tests;
