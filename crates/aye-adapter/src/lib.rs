//! What an adapter implements. [`Adapter`] maps one framework onto aye's
//! engine: it prepares a [`Recognizer`] for a [`Project`] (with the
//! environment and `[tool.aye]` as the binary read them), or fails with an
//! [`AdapterError`], and gives the runtime face a worker installs
//! ([`Runtime`]). The recognizer is its static face: it [`Claim`]s paths,
//! named or found ([`Requested`]), predicts the tests of a parsed
//! [`SourceFile`] (a [`Recognition`], with a [`RefineReason`] when only an
//! import can tell), reports the [`Setting`]s it resolved, for `aye
//! config`, says what the framework's own configuration says about the run
//! ([`FrameworkDefaults`], with [`JunitDefaults`]), and gives the context
//! its executor gets. Ruff's syntax tree, line index and text ranges are
//! re-exported ([`ast`], [`LineIndex`], [`TextRange`], [`TextSize`],
//! [`Ranged`]), so collection and every adapter name the one version of
//! them aye pins.
//!
//! Adapter → runner: `recognize(file, ast, config) → cases`, `refine(files) →
//! cases` for runtime-dependent files, affinity and scope declarations, the
//! metadata mapping (the framework's markers and decorators expressed as the
//! runner's generic tags, requires, serial, timeout, owner, quarantine), the
//! executor `run(batch, policy, services) → events`, and optional transforms for
//! the runner's import hook.
//!
//! Owns: the traits. Does not own: any framework's rules, which live in the
//! `aye-adapter-*` crates; the worker protocol (`aye-protocol`); aye's
//! engine (`aye-engine`).
//!
//! Depends on: aye-events, aye-protocol, aye-config (for `[tool.aye]` and
//! the worker count), aye-static (for the environment), and Ruff's
//! `ruff_python_ast`, `ruff_source_file` and `ruff_text_size`, with the
//! `get-size2` pin that goes with them (see the workspace `Cargo.toml`).
//! Must not depend on the runner or any adapter.
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

use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};

use aye_config::{AyeSettings, ValueSource, WorkerCount};
use aye_events::{AdapterName, Case, Diagnostic, IdPrefix, LineNumber, ProjectPathRef, Span};
use aye_protocol::{AdapterContext, MaxFail, Runtime};
use aye_static::ProcessEnv;

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
    /// The environment aye started in, which the workers inherit, read
    /// once by the binary: the variables a framework reads by name
    /// (`PYTEST_ADDOPTS`), and any a path it expands names. Adapters read
    /// it here, never from the process.
    pub env: &'a ProcessEnv,
    /// `[tool.aye]`, as the binary loaded it from the root's
    /// `pyproject.toml`: an adapter reads its own table here
    /// (`[tool.aye.unittest]`), never from the file.
    pub tool_aye: &'a AyeSettings,
}

impl<'a> Project<'a> {
    /// The project at `root`, aye started in `cwd`, with nothing else
    /// given: no paths, no config file, no framework arguments, an empty
    /// environment and no `[tool.aye]`. For tests, and for the rest given
    /// with struct update syntax.
    pub fn new(root: &'a Path, cwd: &'a Path) -> Self {
        static NO_ENV: ProcessEnv = ProcessEnv::new();
        static NO_TOOL_AYE: AyeSettings = AyeSettings::NONE;
        Self {
            root,
            cwd,
            args: &[],
            config_file: None,
            framework_args: &[],
            env: &NO_ENV,
            tool_aye: &NO_TOOL_AYE,
        }
    }
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

    /// The runtime face: what a worker installs and builds to refine the
    /// adapter's files and run its batches.
    fn runtime(&self) -> Runtime;
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

/// How collection came to ask about a path: a framework's rules for names
/// and ignores apply only to what the walk found, as pytest collects a
/// file on its command line whatever its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    /// Named as a path to collect: on the command line, or a recognizer's
    /// root that is a file (pytest's `testpaths`).
    Named,
    /// Found walking a directory, or listing one for support files.
    Found,
}

impl Requested {
    pub const fn is_named(self) -> bool {
        matches!(self, Self::Named)
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
    pub refine: Option<RefineReason>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Why a file's tests can only be known by importing it, in words: the
/// `not-refined` info says it when refinement is off. It is in no key of
/// the refinement cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefineReason {
    /// The file isn't valid UTF-8, and only the framework knows its
    /// encoding: collection's own reason, when the recognizer gave none.
    NotUtf8,
    /// The adapter's, in its words. A recognizer with several gives them
    /// as one, joined by `; `.
    Adapter(Box<str>),
}

impl RefineReason {
    pub fn as_str(&self) -> &str {
        match self {
            Self::NotUtf8 => "the file is not valid UTF-8",
            Self::Adapter(reason) => reason,
        }
    }
}

impl fmt::Display for RefineReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<String> for RefineReason {
    fn from(reason: String) -> Self {
        Self::Adapter(reason.into())
    }
}

impl From<&str> for RefineReason {
    fn from(reason: &str) -> Self {
        Self::Adapter(reason.into())
    }
}

/// A resolved setting, with where it came from, for `aye config`, which
/// shows its source as a row ([`ValueSource::display_row`]). The
/// refinement cache's key holds its name and value, not its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    pub name: Cow<'static, str>,
    pub value: String,
    /// `pyproject.toml [tool.pytest.ini_options]`, `default`, …
    pub source: ValueSource,
}

/// What a framework's own configuration says about the run as a whole
/// (pytest's options: addopts, `PYTEST_ADDOPTS`, the arguments after
/// `--`), for the binary to go by where aye's own settings say nothing.
/// Of those that hold one value for the run, the binary takes each from
/// the first adapter, in priority order, that gives it; the expressions
/// and the prefixes apply to the adapter's own cases.
///
/// Its adapter's context carries the same, under the keys it always had
/// (`xdist_workers`, `maxfail`, `keyword`, `tags`, `deselect`, `junitxml`,
/// `junit_prefix`, `junit_*`), since the refinement cache's key hashes the
/// context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameworkDefaults {
    /// The worker count (pytest-xdist's `-n`).
    pub workers: Option<WorkerCount>,
    /// The `maxfail` (pytest's `-x` or `--maxfail N`).
    pub maxfail: Option<MaxFail>,
    /// The `-k` expression selection applies to the adapter's cases when
    /// the command line gives none, so that the tests it drops are counted
    /// as deselected.
    pub keyword: Option<String>,
    /// The `-m` expression, likewise.
    pub tags: Option<String>,
    /// The prefixes of aye's ids selection drops from the adapter's cases,
    /// counting them as deselected (pytest's `--deselect`).
    pub deselect: Vec<IdPrefix>,
    /// How the JUnit report is written, and where.
    pub junit: JunitDefaults,
}

/// What a framework's configuration says about the JUnit report aye
/// writes, in pytest's spellings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JunitDefaults {
    /// Where it goes (pytest's `--junitxml`), absolute; empty for no
    /// report.
    pub path: Option<PathBuf>,
    /// What goes before its classnames (`--junitprefix`).
    pub prefix: Option<String>,
    /// `junit_suite_name`.
    pub suite_name: Option<String>,
    /// `junit_logging`.
    pub logging: Option<String>,
    /// `junit_log_passing_tests`.
    pub log_passing_tests: Option<bool>,
    /// `junit_duration_report`.
    pub duration_report: Option<String>,
    /// `junit_family`.
    pub family: Option<String>,
}

/// The name of a file no recognizer is expected to claim by its name
/// ([`Recognizer::claims_any_name`]).
const ANY_NAME: &str = "__aye_any_name__";

/// An adapter's static face, configured for one project.
pub trait Recognizer: Send + Sync {
    /// Where to look when no paths are given (pytest's `testpaths`, or the
    /// current directory). Absolute.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf>;

    /// Whether to walk into a directory. `dir` is absolute.
    fn descend(&self, dir: &Path) -> bool;

    /// What `rel` is to this adapter, asked about as `requested` says.
    fn claim(&self, rel: &str, requested: Requested) -> Claim;

    /// Whether this recognizer claims for tests every file found in `dir`
    /// (relative to the project root, `/` separators, empty for the root)
    /// with `extension` (empty for none), whatever its name: pytest's
    /// doctest modules, `python_files = *.py`. Collection then expects no
    /// test of such a file, and says nothing of one without. By default,
    /// whether [`Recognizer::claim`] claims one named nothing in particular
    /// there (`__aye_any_name__.<extension>`).
    fn claims_any_name(&self, dir: &str, extension: &str) -> bool {
        let mut probe = String::new();
        if !dir.is_empty() {
            probe.push_str(dir);
            probe.push('/');
        }
        probe.push_str(ANY_NAME);
        if !extension.is_empty() {
            probe.push('.');
            probe.push_str(extension);
        }
        self.claim(&probe, Requested::Found).has_tests()
    }

    /// Predict the tests in `file`, which was claimed as `requested` says.
    /// `support` holds the support files that apply to it (pytest's
    /// `conftest.py` files in its directory and above), outermost first.
    fn recognize(
        &self,
        file: &SourceFile<'_>,
        requested: Requested,
        support: &[SourceFile<'_>],
    ) -> Recognition;

    /// The settings this recognizer resolved, with origins.
    fn settings(&self) -> Vec<Setting> {
        Vec::new()
    }

    /// What the framework's own configuration says about the run. None by
    /// default.
    fn defaults(&self) -> FrameworkDefaults {
        FrameworkDefaults::default()
    }

    /// Adapter-specific context sent with every batch and refinement
    /// (pytest's rootdir and inifile): the adapter's typed context, built
    /// once ([`AdapterContext::of`]). None by default.
    fn context(&self) -> AdapterContext {
        AdapterContext::default()
    }
}

#[cfg(test)]
mod tests;
