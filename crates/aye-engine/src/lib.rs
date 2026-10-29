//! Vocabulary (October 2026), binding for code, docs, events, the command
//! line and reports:
//! - **Runner**: aye, the Rust binary. It decides which tests run, where,
//!   when, in what order and under what limits, then collects and reports
//!   what happened. It knows nothing about how a framework defines or
//!   executes a test. It replaces every framework's own runner (pytest's main
//!   loop, unittest's `TextTestRunner`, Django's `DiscoverRunner`).
//! - **Engine**: aye's engine, written in Python, running inside workers and
//!   shipped with aye from the start. It is the model of what a test is and
//!   how it executes: tests, lifetimes, the order of setup and teardown, and
//!   the fixed outcome set. Frameworks have engines of their own (pytest's
//!   collection and fixtures, unittest's `TestCase.run`); "engine" alone
//!   always means aye's.
//! - **Adapter**: maps one existing framework onto aye's engine. It recognises
//!   the framework's tests, translates its setup into lifetimes and its results
//!   into outcomes. aye ships the pytest, unittest and doctest adapters; Django
//!   is a set of services used by the unittest and pytest adapters.
//! - **Framework**: what people write tests against, an authoring API
//!   (fixtures, `TestCase`, assertions) plus its own engine and usually its own
//!   runner. pytest and unittest are frameworks. aye does not ship one yet;
//!   when it does, it is written directly against aye's engine, with no
//!   adapter.
//! - **Library**: aye's importable features (patching, fakes, decorators,
//!   snapshots, test data), usable in the tests of any framework. It is
//!   neither a runner nor an engine.
//! - **Worker**: a Python process, started and managed by the runner, in which
//!   aye's engine executes batches.
//!
//! The line between runner and engine is the batch: the runner owns everything
//! up to handing a batch to the engine and everything after its events come
//! back; the engine owns everything in between. Every adapter is a front-end
//! that reads its framework's tests into aye's model and never runs them;
//! aye's engine runs every test. unittest's and doctest's class and module
//! setup become lifetimes that aye opens and closes at times the runner
//! decides; pytest's fixtures, with their five scopes, are the engine's
//! resources, read through aye's own `pytest` module, so pytest itself is
//! never imported.
//!
//! Until the crates are renamed, `aye-engine-pytest`, `aye-engine-unittest`
//! and `aye-engine-doctest` are the adapters, and older text in the crate docs
//! that says "engine" for a per-framework component means "adapter".
//!
//! The contract between the runner and any engine. Rust traits only, plus
//! the embedded Python-side contract (`python/aye_engine.py`) that the worker
//! hands to every engine executor.
//!
//! Engine → runner: `recognize(file, ast, config) → cases`, `refine(files) →
//! cases` for runtime-dependent files, affinity and scope declarations, the
//! metadata mapping (the framework's markers and decorators expressed as the
//! runner's generic tags, requires, serial, timeout, owner, quarantine), the
//! executor `run(batch, policy, services) → events`, and optional transforms for
//! the runner's import hook.
//!
//! Runner → engine: services inside the worker (capture, coverage, soft sandbox,
//! timeouts, current-test context, cleanup registration, resources, the
//! patching library), the selection to apply, the run policy, the event sink.
//!
//! Owns: the traits and the executor protocol. Does not own: any framework's
//! rules, which live in the `aye-engine-*` crates; the native engine will be one
//! more implementation of this contract.
//!
//! Depends on: aye-events. Must not depend on the runner or any engine.
//!
//! Open: how rich affinity/scope declarations must be to carry pytest's five
//! fixture scopes and unittest's class/module setup; the exact refinement
//! messages; which policy knobs an engine must honour versus may ignore.
//!
//! Native features (October 2026): snapshots, test data and fakes are runner
//! services plus objects in `aye.testing`, reached the same way from every
//! engine (pytest, unittest, doctest, Django tests). They are not features of
//! the future native engine. The services list grows accordingly: capture,
//! coverage, sandbox, timeouts, current-test context, cleanup, resources,
//! patching, clock, randomness, network, filesystem, snapshots and data
//! builders.
//!
//! Open: whether engines reach services by name with a documented fallback
//! when one is absent (new services stay additive), or through a fixed struct
//! (simpler to type-check, but every new service changes the contract); the
//! aye words for a fake, a snapshot and a data builder.

//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Services are looked up by name**, with `None` when absent, so new
//!   services stay additive ([`services`] lists the names). An engine that
//!   needs a service it doesn't get degrades or reports, never crashes.
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
//! - **The executor protocol is JSON lines** over the worker's original
//!   stdout, versioned by [`PROTOCOL_VERSION`] and checked at the handshake.
//!   Requests are [`Request`]s; worker lines are [`WorkerLine`]s: either a
//!   control [`Reply`] or an event [`Record`].
//! - **An executor may list the session** ([`Executor::lists_session`],
//!   protocol 2): it keeps what a refinement listed (its imported files, its
//!   model) for the batches it runs later, as pytest-xdist's workers each
//!   collect the whole session. The runner then keeps the worker that
//!   refined for the run, and has every other worker list the session's
//!   files before its first batch ([`RefineRequest::digests_only`]),
//!   comparing each file's [`Refined::digests`] with that worker's. A
//!   worker's session ends with its last batch ([`Batch::ends_session`],
//!   protocol 3), whose events and status report what fails there, as
//!   pytest reports a session's end; the worker calls the executor's
//!   optional `close()` when it shuts down, which ends a session no batch
//!   ended (a stopped run), so that what lives as long as the session is
//!   torn down once per worker.
//! - **The Python half is source, not a compiled library.** `aye_engine.py`
//!   and every adapter's executor modules are embedded in the binary and
//!   installed into the worker's `sys.modules` from memory, so they work
//!   however aye was installed and never skew from the binary.
//! - **An adapter prepares knowing the others** ([`Adapter::prepare_among`],
//!   which the binary calls; its default is [`Adapter::prepare`]): doctest's
//!   default text glob depends on pytest running beside it.
//! - **Skips are told by every framework's spelling** (`aye_engine.is_skip`):
//!   aye's `Skip`, unittest's `SkipTest` and pytest's `Skipped` (which says
//!   it lives in `builtins`, so it is also told by its name with its base
//!   `OutcomeException` or pytest's attributes, and by `isinstance` once
//!   pytest is loaded), subclasses included.
//! - **Tests may break what the engine uses; the engine doesn't break.** The
//!   engine binds the standard-library functions it calls (`perf_counter`,
//!   `os.path`'s, `traceback`'s, `math.isfinite`, `hashlib.sha1`) when it
//!   is imported and computes paths with them, and building a failure never
//!   raises (what can't be read is left out), so a test that patches them,
//!   even for the tests after it, changes no result, path or duration.
//!   Adapters do the same for what they use: `perf_counter`, `os_path`
//!   (`os.path` as it was then, its functions looking each other up in a
//!   copy, so `abspath` and `realpath` survive a patched `join`), and
//!   `CaptureIO`; and they load a batch with `os.path`'s own functions back
//!   in place (`unpatched_os_path`), as frameworks import their tests
//!   before running any. The worker does it for the protocol (`json.dumps`,
//!   `os.getpid`).
//! - **Capture without a service** (`captured`, `CaptureIO`): `sys.stdout`
//!   and `sys.stderr` become streams as pytest's default capture
//!   (`--capture=fd`) makes them, text in UTF-8 over bytes (`.buffer`,
//!   `.encoding`) with `errors="replace"` (a lone surrogate is written as
//!   `?` rather than failing the test), with a real `fileno()` (a
//!   temporary file made when first asked for, whose writes are captured
//!   too, after the stream's), still readable when a test closed them, so
//!   a test closing one loses nothing and breaks no later test.
//! - **With `maxfail`, a failed lifetime is reported before the tests it
//!   blocks** (`Lifetimes.report_early`): the runner drops what follows the
//!   bad outcome that reaches the limit, and the tests' `cause` must name a
//!   failure in the log. Without it, the lifetime finishes after them, so
//!   renderers can list them under it.
//! - **Signatures without a location**: a `Failure`'s `site` (not sent)
//!   stands for the place in its signature when it has no location (a
//!   doctest's test and example), so failures in different places that
//!   only read alike aren't grouped.
//! - **Reruns:** `EngineExecutor` sets `reproduce` on every bad outcome,
//!   `aye <rerun_args> <id> -- <args after -->` (`aye_engine.reproduce`;
//!   an executor may override `rerun`), as the pytest adapter does. The
//!   binary adds `rerun_args` to every adapter's context of a batch: aye's
//!   own options that shaped the run (`-c FILE`, as given), which a rerun
//!   needs to run the same tests the same way (`aye_engine.rerun_args`).
//! - **A `sub_outcome` may carry a `reason`** (why a subtest was skipped).
//! - **What a test records for reports** travels in the stream: its
//!   `properties` (`Result.properties`, pytest's `record_property`) and the
//!   run's (`Sink.run_property`, pytest's `record_testsuite_property`), and
//!   its body's duration apart (`Result.body_duration`).
//! - **A failure's `detail`** is Python's text of the exception without the
//!   frames before the user's code (`Paths.is_internal`: the worker's, the
//!   engine's, the framework's), as unittest leaves out its own
//!   (`aye_engine.format_exception(exc, paths)`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_events::{
    AdapterName, BatchIndex, Case, Diagnostic, LineNumber, ProjectPath, ProjectPathRef, Record,
    Span, TestId,
};
use serde::{Deserialize, Serialize};

pub use ruff_python_ast as ast;
pub use ruff_source_file::LineIndex;
pub use ruff_text_size::{Ranged, TextRange, TextSize};

/// Version of the runner ↔ worker protocol. Checked at the handshake.
/// 2: [`Executor::lists_session`], [`RefineRequest::digests_only`],
/// [`Refined::digests`] and the executors' `close()` at shutdown.
/// 3: [`Batch::ends_session`].
pub const PROTOCOL_VERSION: u32 = 3;

/// A Python module embedded in the binary and installed in the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedModule {
    /// Import name, e.g. `aye_engine`.
    pub name: &'static str,
    pub source: &'static str,
}

/// aye's engine: the Python base classes every executor builds on.
pub const ENGINE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_engine",
    source: include_str!("../python/aye_engine.py"),
};

/// aye's test model (prototype): what a front-end produces when it reads a
/// framework's tests, and what the native engine runs. Knows no framework.
pub const MODEL_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_model",
    source: include_str!("../python/aye_model.py"),
};

/// Assertion rewriting (prototype): a service of the native engine, for
/// the modules a front-end asks it to rewrite.
pub const REWRITE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_rewrite",
    source: include_str!("../python/aye_rewrite.py"),
};

/// The native engine (prototype): runs [`MODEL_MODULE`]'s cases, resources
/// and lifetimes; imports no framework.
pub const NATIVE_ENGINE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_native_engine",
    source: include_str!("../python/aye_native_engine.py"),
};

/// Names of the services the runner may provide inside a worker. Engines
/// look them up by name and must cope with any of them being absent.
pub mod services {
    /// Captures stdout, stderr and logging per test.
    pub const CAPTURE: &str = "capture";
    /// The test or lifetime currently running.
    pub const CONTEXT: &str = "context";
    /// Teardown registration on the current test or lifetime.
    pub const CLEANUP: &str = "cleanup";
    pub const COVERAGE: &str = "coverage";
    pub const SANDBOX: &str = "sandbox";
    pub const TIMEOUT: &str = "timeout";
    pub const RESOURCES: &str = "resources";
    pub const PATCHING: &str = "patching";
    pub const CLOCK: &str = "clock";
    pub const RANDOMNESS: &str = "randomness";
    pub const NETWORK: &str = "network";
    pub const FILESYSTEM: &str = "filesystem";
    pub const SNAPSHOTS: &str = "snapshots";
    pub const DATA: &str = "data";
}

// ---------------------------------------------------------------- static face

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

/// An adapter's runtime face: modules the worker installs, and the executor
/// class it instantiates.
#[derive(Debug, Clone)]
pub struct Executor {
    pub adapter: AdapterName,
    /// Installed in order, after [`ENGINE_MODULE`].
    pub modules: &'static [EmbeddedModule],
    /// `module:Class` of the `aye_engine.Executor` subclass.
    pub entry: &'static str,
    /// The executor keeps what it lists for the batches it runs later (its
    /// imported files, its model), so a worker lists the session once: the
    /// runner keeps the worker that refined as its first lane, and every
    /// other lane lists the session's files before its first batch. The
    /// native pytest executor's; the others list nothing they keep.
    pub lists_session: bool,
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

/// Imports files in a worker and reports their real tests. Implemented by
/// the runner; used by collection.
pub trait Refiner {
    fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError>;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefineRequest {
    pub adapter: AdapterName,
    pub files: Vec<ProjectPath>,
    #[serde(default)]
    pub context: serde_json::Value,
    /// List the files for the batches to come, and answer with their
    /// [`Refined::digests`] only, no case: what a lane sends an executor
    /// that [lists the session](Executor::lists_session) before its first
    /// batch. The executor's `refine` gets `digests_only=True`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub digests_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Refined {
    /// Every test in the requested files; replaces the static prediction.
    /// Empty when the request was [`RefineRequest::digests_only`].
    pub cases: Vec<Case>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
    /// For an executor that [lists the session](Executor::lists_session):
    /// by requested file, a digest of its tests' ids in order (the worker's
    /// own is the SHA-1 of the ids joined by newlines), so that two workers'
    /// listings can be compared without the cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<BTreeMap<ProjectPath, String>>,
}

#[derive(Debug, thiserror::Error)]
#[error("refinement failed: {0}")]
pub struct RefineError(pub String);

// ---------------------------------------------------------------- runtime face

/// How a batch must run. Engines honour `maxfail`, `capture` and the
/// selection; the rest they may ignore and say so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    /// Stop the batch after this many bad outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maxfail: Option<u32>,
    /// Capture output per test (`false` for `-s`).
    #[serde(default = "yes")]
    pub capture: bool,
    /// `-v` count minus `-q` count.
    #[serde(default)]
    pub verbosity: i8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    /// Keyword expression, re-applied by the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyword: Option<String>,
    /// Tag expression, re-applied by the engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
    /// Passed to the framework unchanged (`aye -- --reuse-db`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub framework_args: Vec<String>,
}

const fn yes() -> bool {
    true
}

impl Default for Policy {
    /// Capture on, everything else off: the same as an empty JSON object.
    fn default() -> Self {
        Self {
            maxfail: None,
            capture: true,
            verbosity: 0,
            timeout: None,
            keyword: None,
            tags: None,
            framework_args: Vec::new(),
        }
    }
}

/// A unit of work handed to the engine. Everything inside it is the
/// engine's; everything around it is the runner's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub id: BatchIndex,
    pub adapter: AdapterName,
    pub ids: Vec<TestId>,
    pub policy: Policy,
    #[serde(default)]
    pub context: serde_json::Value,
    /// The worker's last batch for its adapter (protocol 3): an executor
    /// that [lists the session](Executor::lists_session) ends it with this
    /// batch, as a pytest-xdist worker ends its session with its last
    /// test: what lives as long as the session closes in the last test's
    /// teardown, then the framework's end of session runs, and what fails
    /// there is the batch's to report (its status included). With no ids,
    /// the batch only ends the session: what fails then has no test to
    /// blame. The runner sends the worker no further batch; others ignore
    /// it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ends_session: bool,
}

/// One embedded module, on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSource {
    pub name: String,
    pub source: String,
}

impl From<EmbeddedModule> for ModuleSource {
    fn from(module: EmbeddedModule) -> Self {
        Self {
            name: module.name.to_string(),
            source: module.source.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorSpec {
    pub adapter: AdapterName,
    /// `module:Class`.
    pub entry: String,
    /// [`Executor::lists_session`]: the worker gives every refinement's
    /// answer [`Refined::digests`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lists_session: bool,
}

/// Runner → worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// First message: what to install and how to reach the project.
    Hello {
        protocol: u32,
        /// The project root, absolute.
        root: String,
        /// Installed into `sys.modules` in order.
        modules: Vec<ModuleSource>,
        executors: Vec<ExecutorSpec>,
        /// Services to provide, by name.
        services: Vec<String>,
    },
    Refine(RefineRequest),
    Run(Batch),
    /// The worker calls each executor's optional `close()`, then exits (as
    /// it does at the end of its input).
    Shutdown,
}

/// Worker → runner control messages (events travel as [`Record`]s).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// Handshake answer to [`Request::Hello`].
    Ready {
        protocol: u32,
        pid: u32,
        /// `sys.executable`.
        python: String,
        /// `sys.version`, first line.
        version: String,
    },
    Refined(Refined),
    /// The batch is over; every test in it has a `test_finished` event,
    /// except those after a `maxfail` stop.
    BatchDone {
        batch: BatchIndex,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<i32>,
    },
    /// The worker failed outside any test while handling a request.
    Failed {
        message: String,
    },
}

/// One line from a worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WorkerLine {
    Reply(Reply),
    Record(Box<Record>),
}

#[cfg(test)]
mod tests;
