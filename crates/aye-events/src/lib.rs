//! The wire types. Everything that crosses a process boundary or an engine
//! boundary is defined here and nowhere else: events, outcomes, structured
//! failures, captured output, the collected `Case` with its tags, requirements,
//! affinity keys and certainty, run metadata, and the schema version.
//!
//! Owns: the schema and its version. No traits, no logic, no I/O.
//!
//! Depends on: serde only. Must not depend on any other aye crate.
//!
//! Design notes (see docs/events.md once written):
//! - A run is its event log; summaries, history rows, run diffs and every
//!   renderer are folds over it. Events are self-contained and bounded.
//! - The outcome set is fixed; annotations such as *flaky* are separate.
//! - Sub-outcomes (subtests, doctest examples) and scope lifecycle events are
//!   first-class.
//! - Removing or renaming a field bumps `SCHEMA_VERSION`.
//!
//! Native features (October 2026): snapshots, test data and fakes are aye
//! features, not companion projects. They need not ship by 0.1.0, but the
//! schema leaves room for them now so that adding them is not a break.
//! - Additions are additive by construction: events are a tagged union and
//!   every consumer ignores kinds it does not know; every struct tolerates
//!   unknown fields and new fields are optional; only removals and renames
//!   bump `SCHEMA_VERSION`.
//! - Sub-outcomes are in the first schema: `subTest` blocks and doctest
//!   examples need them now, snapshots will add one per snapshot.
//! - A source span (file and byte range) exists separately from the test id,
//!   which deliberately has no line: snapshots, the source rewriter and
//!   failure locations all point at spans.
//! - Payloads these features will add: the snapshot (expected value, actual
//!   value, where the expected value lives), the fakes active during a test,
//!   and the seed and frozen time a test ran with.
//! - A hermetic verdict per test exists before the first release that caches
//!   results, even if it only comes from the sandbox tripwire.
//!
//! Open: whether a snapshot with no value yet maps onto an existing outcome
//! or onto an annotation (a new outcome after release would break every
//! consumer that matches on outcomes); whether the reproduce command is a
//! structured object from day one (command plus an open map of inputs such as
//! seed and frozen time) or a string accepted to change later.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Identity.** A [`TestId`] is the canonical path id: the file path
//!   relative to the project root with `/` separators, then `::`-separated
//!   names, then the parameter id in brackets
//!   (`tests/test_cart.py::TestCart::test_add[eur]`). Parameters stay opaque
//!   inside the id; structured parameters, when an adapter knows them, travel
//!   beside it on the [`Case`]. The [`TestKey`] is a stable 64-bit FNV-1a hash
//!   of the id, printed as 16 hex digits.
//! - **One outcome per test.** A test that passes its body and then fails in
//!   teardown is `error`; every failure is kept, each with its [`Phase`].
//! - **A failed setup is reported once.** The failure goes on the
//!   [`Event::LifetimeFinished`] of the lifetime that failed, and each test
//!   that could not run because of it is `error` with `cause` set to that
//!   lifetime instead of a copy of the failure. Adapters that cannot see
//!   lifetimes (pytest) attach the failure to each test, as pytest does.
//! - **The reproduce command is structured** from day one: an argument
//!   vector plus an open map of inputs (seed, frozen time, environment).
//! - **Envelope.** Workers stamp `batch` and a per-worker sequence and time;
//!   the runner stamps `run`, `environment`, `worker` and the run-wide `seq`
//!   as it merges worker streams into the run's log.
//! - **Counts are test results.** [`Counts`] folds `test_finished` alone. A
//!   module the framework skipped while collecting (pytest's
//!   `allow_module_level` skip, `importorskip`) has no test to finish: it is
//!   a `skipped` info [`Diagnostic`] from collection, and the renderers count
//!   it as one skipped item, as pytest's summary and JUnit report do. No
//!   field was needed for it. Nor is a failed lifetime that kept no test
//!   from running among them (a failed `tearDownClass`, `tearDownModule`
//!   or cleanup; a failed setup whose tests the run stopped before under
//!   `maxfail`), though the run fails on it: its `lifetime_finished`
//!   carries the failure, and the renderers count it from there, as a
//!   setup or teardown error (pytest's summary counts it as an error).
//! - **Lifetime events need not pair.** A worker that crashes, or that a
//!   stop kills, sends no `lifetime_finished` for the lifetimes it had
//!   started, so the log may hold none: a consumer must not depend on the
//!   pair. The same lifetime id starts again in each batch that needs it.
//! - **Properties tests record** travel in the stream, for the JUnit report
//!   and any later consumer: a test's own on its `test_finished`
//!   (`properties`, name and value pairs in order: pytest's
//!   `record_property`), and the run's as [`Event::RunProperty`] (pytest's
//!   `record_testsuite_property`), emitted when the test records it. Each
//!   batch is a framework session, so a session-scoped fixture that records
//!   one does so once per batch: consumers keep each pair once.
//! - **A test's body may be timed apart** (`body_duration_ms`, pytest's
//!   `call` phase), for reports that count the body alone (pytest's
//!   `junit_duration_report = call`); `duration_ms` stays setup, body and
//!   teardown.
//! - **A sub-outcome carries its `reason`** when it was skipped or expected
//!   to fail (unittest's `skipTest` inside `subTest`), as a test does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Bumped only when a field is removed or renamed, or its meaning changes.
pub const SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------- identity

/// The canonical path id of a test. Deliberately has no line number.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestId(pub String);

impl TestId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The file part of the id, relative to the project root.
    pub fn path(&self) -> &str {
        self.0
            .split_once("::")
            .map_or(self.0.as_str(), |(path, _)| path)
    }

    /// The id without its parameter id: `t.py::test_x[1]` → `t.py::test_x`.
    pub fn without_params(&self) -> &str {
        match self.0.find('[') {
            Some(at) if self.0.ends_with(']') && self.0[..at].contains("::") => &self.0[..at],
            _ => &self.0,
        }
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

/// A short, stable key for a test: the FNV-1a 64 hash of its id, mixed
/// (`SplitMix64`'s finalizer) so that every digit depends on every byte. A
/// key is shown by its first digits, and FNV-1a alone leaves the leading
/// ones unchanged by the last bytes: `test_0` and `test_1` would share them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TestKey(pub String);

impl TestKey {
    pub fn of(id: &str) -> Self {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0100_0000_01b3;
        let mut hash = id
            .bytes()
            .fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME));
        hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        hash ^= hash >> 31;
        Self(format!("{hash:016x}"))
    }
}

/// A file and a byte range in it. Separate from ids: failure locations,
/// snapshots and the source rewriter all point at spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Relative to the project root, `/` separators.
    pub path: String,
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

/// Identifies one environment (interpreter plus dependencies) within a run.
pub type EnvironmentId = String;
/// Identifies one lifetime within a run, e.g. `class:tests/t.py::TestX`.
pub type LifetimeId = String;
/// Dense worker index within a run, from 0.
pub type WorkerId = u32;
/// Dense batch index within a run, from 0.
pub type BatchId = u32;

// ---------------------------------------------------------------- outcomes

/// The fixed outcome set. Adding a variant is a breaking change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    /// A check in the test failed.
    Failed,
    /// The test could not run to a verdict: an unexpected exception, or a
    /// failure in setup or teardown.
    Error,
    Skipped,
    /// The test was expected to fail, and did.
    ExpectedFailure,
    /// The test was expected to fail, and passed.
    UnexpectedPass,
}

impl Outcome {
    /// Whether this outcome makes the run fail.
    pub const fn is_bad(self) -> bool {
        matches!(self, Self::Failed | Self::Error)
    }

    /// Severity for combining outcomes: the worse outcome wins.
    pub const fn severity(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Skipped => 1,
            Self::ExpectedFailure => 2,
            Self::UnexpectedPass => 3,
            Self::Failed => 4,
            Self::Error => 5,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Skipped => "skipped",
            Self::ExpectedFailure => "expected_failure",
            Self::UnexpectedPass => "unexpected_pass",
        }
    }
}

/// Where in a test's life something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Setup,
    Test,
    Teardown,
}

// ---------------------------------------------------------------- failures

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Failure {
    pub phase: Option<Phase>,
    /// The exception's type name, e.g. `AssertionError`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
    /// One line.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertion: Option<Assertion>,
    /// Where the exception was raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// User frames, innermost last; runner, engine and framework internals
    /// are removed by the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
    /// Groups identical failures: exception type, normalised message and the
    /// innermost user frame, hashed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    /// The framework's own full text, bounded, for renderers that want it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The resource (pytest's fixture) whose setup or teardown raised it,
    /// when it failed in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// One line suggesting a fix, e.g. the names closest to an unknown
    /// resource (`did you mean cart?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Assertion {
    pub op: String,
    pub left: String,
    pub right: String,
    /// Explanation lines (the diff), without the leading assertion line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff: Vec<String>,
    /// Which operand is the expected value: the one written as a literal or
    /// a named constant. Absent when neither or both are; their source then
    /// names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Side>,
    /// The left operand's source text, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_source: Option<String>,
    /// The right operand's source text, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_source: Option<String>,
    /// How the operands differ, by their structure rather than their text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<Compare>,
}

/// One operand of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

/// How two compared values differ, entry by entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Compare {
    /// What was compared: `dict`, `list`, `tuple`, `set`, `str`, `bytes`,
    /// `dataclass`, `namedtuple`, `datetime`, `number`, or a type's name.
    pub kind: String,
    /// One line saying how they differ, e.g. `dict differs in 1 of 2 keys`.
    pub summary: String,
    /// Entries (keys, items, fields) that are the same.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub same: u32,
    /// Entries that differ, in order, at most 50.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differs: Vec<Difference>,
    /// Differing entries left out of `differs`.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub omitted: u32,
}

/// One entry that differs between two compared values.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Difference {
    /// Where, written as an access on the value: `['pear']`, `[3]`,
    /// `.total`; for text, where it first differs (`line 2, column 5`).
    /// Absent when the values differ as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The left operand's entry, shown as Python shows it; absent when the
    /// left has none there (only the right has it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// The right operand's entry; absent when only the left has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
const fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Frame {
    pub path: String,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The line, without its indentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The failing expression's span in `source`: 0-based character
    /// offsets, the end excluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<[u32; 2]>,
    /// The lines just before `line`, at most two, oldest first, with their
    /// indentation relative to the least indented of them and `line`. Only
    /// on the innermost project frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// How far `line` is indented beyond the least indented of `before`
    /// (`source` has no indentation of its own).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub indent: u32,
    /// The frame's local variables when it failed, as Python shows them,
    /// bounded. Only on the innermost frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locals: Vec<(String, String)>,
}

/// Output captured while a test or lifetime ran. Each stream is bounded.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Captured {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log: String,
    /// Some stream was cut to fit its bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl Captured {
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty() && self.log.is_empty()
    }
}

/// How to rerun exactly this test with exactly these inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reproduce {
    pub command: Vec<String>,
    /// Seed, frozen time, environment, and whatever later features add.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

// ---------------------------------------------------------------- cases

/// How sure collection is that a case exists exactly as described.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// Known from the source alone.
    #[default]
    Exact,
    /// The test exists, but its parameter ids or tags are only known at
    /// runtime; the id is the unparametrized one.
    Partial,
    /// Reported by the framework after importing the file.
    Refined,
}

/// One collected test, as the runner plans with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: TestId,
    /// The adapter that collected it (`pytest`).
    pub adapter: String,
    /// Where the test is defined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// User labels, from the framework's markers or decorators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Names a keyword expression matches against (path parts, enclosing
    /// names, the test name, tags), chosen by the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Capabilities a backend must provide to run this test.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Keys of the lifetimes this test runs inside, outermost first. Tests
    /// sharing a key are kept together when that lifetime is expensive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affinity: Vec<String>,
    /// Must not run concurrently with other serial tests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub serial: bool,
    /// Per-test timeout in seconds, from the framework's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<String>,
    pub certainty: Certainty,
}

impl Case {
    pub fn new(id: impl Into<String>, adapter: impl Into<String>) -> Self {
        Self {
            id: TestId::new(id),
            adapter: adapter.into(),
            span: None,
            tags: Vec::new(),
            keywords: Vec::new(),
            requires: Vec::new(),
            affinity: Vec::new(),
            serial: false,
            timeout: None,
            owner: None,
            quarantine: None,
            certainty: Certainty::Exact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Something collection wants the user to know: a syntax error, a test that
/// will never run, a name defined twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable, short, kebab-case: `syntax-error`, `duplicate-definition`.
    pub code: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    pub message: String,
}

// ---------------------------------------------------------------- results

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    pub id: TestId,
    pub outcome: Outcome,
    #[serde(rename = "duration_ms", with = "millis", default)]
    pub duration: std::time::Duration,
    /// Every failure, in the order they happened (a failed body, then a
    /// failed teardown).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
    /// The failed lifetime that kept this test from running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<LifetimeId>,
    /// Why it was skipped or expected to fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Captured::is_empty")]
    pub output: Captured,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproduce: Option<Reproduce>,
    /// Name and value pairs the test recorded about itself, in order
    /// (pytest's `record_property`, its item's `user_properties`). The JUnit
    /// report writes them on the test case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<(String, String)>,
    /// How long the body alone took (the `test` phase, pytest's `call`),
    /// when the adapter tells it apart from setup and teardown.
    #[serde(
        rename = "body_duration_ms",
        with = "millis::optional",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub body_duration: Option<std::time::Duration>,
    /// The resources (pytest's fixtures) set up for a test with a bad
    /// outcome, in setup order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
    /// How the test ended in earlier runs, from history; set by the runner
    /// on a bad outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Previous>,
}

/// A resource a test used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub name: String,
    /// How long its value lives: `session`, `package`, `module`, `class`
    /// or `function`.
    pub scope: String,
}

/// A test's outcomes in the runs before this one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Previous {
    /// One letter per run, oldest first, at most 20: `p` passed, `f`
    /// failed, `e` error, `s` skipped, `x` expected failure, `u`
    /// unexpected pass.
    pub outcomes: String,
    /// The number of the run in which the test's current streak of bad
    /// outcomes began, when it began before this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<u64>,
}

impl TestResult {
    pub fn new(id: TestId, outcome: Outcome) -> Self {
        Self {
            id,
            outcome,
            duration: std::time::Duration::ZERO,
            failures: Vec::new(),
            cause: None,
            reason: None,
            output: Captured::default(),
            reproduce: None,
            properties: Vec::new(),
            body_duration: None,
            resources: Vec::new(),
            history: None,
        }
    }

    /// The failure renderers lead with: the first one.
    pub fn failure(&self) -> Option<&Failure> {
        self.failures.first()
    }
}

/// Totals for a run; a fold over its `test_finished` events. A module
/// skipped while collecting is not among them, nor a failed lifetime that
/// kept no test from running (see the crate's docs).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub passed: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub error: u32,
    #[serde(default)]
    pub skipped: u32,
    #[serde(default)]
    pub expected_failure: u32,
    #[serde(default)]
    pub unexpected_pass: u32,
    #[serde(default)]
    pub deselected: u32,
}

impl Counts {
    pub fn record(&mut self, outcome: Outcome) {
        let slot = match outcome {
            Outcome::Passed => &mut self.passed,
            Outcome::Failed => &mut self.failed,
            Outcome::Error => &mut self.error,
            Outcome::Skipped => &mut self.skipped,
            Outcome::ExpectedFailure => &mut self.expected_failure,
            Outcome::UnexpectedPass => &mut self.unexpected_pass,
        };
        *slot += 1;
    }

    pub const fn total(&self) -> u32 {
        self.passed
            + self.failed
            + self.error
            + self.skipped
            + self.expected_failure
            + self.unexpected_pass
    }

    pub const fn bad(&self) -> u32 {
        self.failed + self.error
    }
}

/// How a run ended. Values match pytest's wherever pytest defines one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
#[repr(u8)]
pub enum ExitCode {
    /// Every selected test passed (or was skipped or expected to fail).
    Ok = 0,
    /// Some test failed or errored.
    TestsFailed = 1,
    /// Interrupted, including by collection errors.
    Interrupted = 2,
    /// A bug in aye or a worker that died outside any test.
    InternalError = 3,
    /// Bad command line, configuration or expression.
    UsageError = 4,
    /// Nothing was collected or selected.
    NoTests = 5,
}

impl From<ExitCode> for u8 {
    fn from(code: ExitCode) -> Self {
        code as u8
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        Self::from(code as u8)
    }
}

impl TryFrom<u8> for ExitCode {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Ok,
            1 => Self::TestsFailed,
            2 => Self::Interrupted,
            3 => Self::InternalError,
            4 => Self::UsageError,
            5 => Self::NoTests,
            other => return Err(format!("unknown exit code {other}")),
        })
    }
}

// ---------------------------------------------------------------- events

/// Who stamped what on an event. All fields are optional on the wire so
/// that workers can send partial envelopes the runner completes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchId>,
    /// Position in the run's log, assigned by the runner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    /// Milliseconds since the run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ms: Option<f64>,
}

/// One line of the run's log: an envelope and an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(flatten)]
    pub envelope: Envelope,
    #[serde(flatten)]
    pub event: Event,
}

impl Record {
    pub fn new(event: Event) -> Self {
        Self {
            envelope: Envelope::default(),
            event,
        }
    }
}

/// Every event kind. Consumers must ignore kinds they don't know: new kinds
/// are added without bumping the schema, and arrive as [`Event::Unknown`].
/// Events never repeat a field the [`Envelope`] carries (run, environment,
/// worker, batch), since a [`Record`] flattens both into one object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    RunStarted {
        schema_version: u32,
        aye_version: String,
        /// The project root, absolute.
        root: String,
        /// Only collect; nothing runs.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        collect_only: bool,
        #[serde(default)]
        workers: u32,
        /// The run's number in the run store (`r81`), when it is stored.
        /// `run_number` on the wire: `run` is the envelope's, the run's id.
        #[serde(
            rename = "run_number",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        run: Option<u64>,
    },
    CollectionStarted,
    /// How far a long collection has got, for a live display. Sent only
    /// once collection has taken half a second, then at most four times a
    /// second, so a quick collection sends none.
    CollectionProgress {
        /// Files and directories walked.
        walked: u64,
        /// Files an adapter claimed for tests.
        test_files: u64,
        /// Files being imported to know their tests; 0 before refinement.
        #[serde(default, skip_serializing_if = "is_zero")]
        refining: u64,
    },
    Diagnostic(Diagnostic),
    /// One selected case. Emitted for every selected case, so the log alone
    /// knows the inventory.
    CaseCollected(Box<Case>),
    CollectionFinished {
        selected: u32,
        deselected: u32,
        /// Files that had to be imported to know their tests.
        #[serde(default)]
        refined_files: u32,
        #[serde(rename = "duration_ms", with = "millis", default)]
        duration: std::time::Duration,
    },
    /// The worker and environment are in the envelope.
    WorkerStarted {
        pid: u32,
        python: String,
        /// The first line of `sys.version` in the worker (`3.13.1 (main,
        /// …) [Clang …]`), for the renderers' headers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },
    /// The batch and worker are in the envelope.
    BatchStarted {
        adapter: String,
        cases: u32,
    },
    /// A lifetime's setup begins. Its [`Event::LifetimeFinished`] may never
    /// come: a worker that crashes or is stopped sends none.
    LifetimeStarted {
        lifetime: LifetimeId,
        /// `session`, `package`, `module`, `class`, …, in aye's words.
        kind: String,
    },
    LifetimeFinished {
        lifetime: LifetimeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
        #[serde(rename = "duration_ms", with = "millis", default)]
        duration: std::time::Duration,
    },
    TestStarted {
        id: TestId,
    },
    /// A result inside a test: a `subTest` block, a doctest example, a
    /// subtest. The test still finishes with its own outcome.
    SubOutcome {
        id: TestId,
        name: String,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
        /// Why it was skipped or expected to fail (unittest's `skipTest` in
        /// a `subTest` block).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(rename = "duration_ms", with = "millis", default)]
        duration: std::time::Duration,
    },
    TestFinished(Box<TestResult>),
    /// A name and value pair about the whole run that a test recorded
    /// (pytest's `record_testsuite_property`). The JUnit report writes each
    /// pair once on its test suite, however many batches recorded it.
    RunProperty {
        name: String,
        value: String,
    },
    BatchFinished {
        /// The framework's own status for the batch, if it has one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<i32>,
    },
    WorkerFinished {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_status: Option<i32>,
        /// Set when the worker died or failed outside any test.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    RunFinished {
        exit_code: ExitCode,
        counts: Counts,
        #[serde(rename = "duration_ms", with = "millis", default)]
        duration: std::time::Duration,
        /// Why the run stopped before every selected test ran: `maxfail`,
        /// `interrupted`, or `framework` (the framework ended a batch early,
        /// e.g. `pytest.exit`). Absent when everything ran.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stopped: Option<String>,
    },
    /// A kind this version doesn't know.
    #[serde(other)]
    Unknown,
}

impl Event {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::CollectionStarted => "collection_started",
            Self::CollectionProgress { .. } => "collection_progress",
            Self::Diagnostic(_) => "diagnostic",
            Self::CaseCollected(_) => "case_collected",
            Self::CollectionFinished { .. } => "collection_finished",
            Self::WorkerStarted { .. } => "worker_started",
            Self::BatchStarted { .. } => "batch_started",
            Self::LifetimeStarted { .. } => "lifetime_started",
            Self::LifetimeFinished { .. } => "lifetime_finished",
            Self::TestStarted { .. } => "test_started",
            Self::SubOutcome { .. } => "sub_outcome",
            Self::TestFinished(_) => "test_finished",
            Self::RunProperty { .. } => "run_property",
            Self::BatchFinished { .. } => "batch_finished",
            Self::WorkerFinished { .. } => "worker_finished",
            Self::RunFinished { .. } => "run_finished",
            Self::Unknown => "unknown",
        }
    }
}

/// Durations are fractional milliseconds on the wire, under `*_ms` keys.
mod millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_f64((d.as_secs_f64() * 1_000_000.0).round() / 1000.0)
    }

    /// Negative values are zero and values too big for a `Duration` (a
    /// test that patched the clock) saturate: a worker's numbers must never
    /// panic the runner.
    /// A missing or `null` duration (a worker whose clock went non-finite)
    /// reads as zero rather than losing the record.
    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let Some(ms) = Option::<f64>::deserialize(d)? else {
            return Ok(Duration::ZERO);
        };
        Ok(Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(Duration::MAX))
    }

    /// The same, for a duration that may be absent.
    pub(crate) mod optional {
        use std::time::Duration;

        use serde::{Deserializer, Serializer};

        #[expect(clippy::ref_option, reason = "signature fixed by serde")]
        pub(crate) fn serialize<S: Serializer>(
            d: &Option<Duration>,
            s: S,
        ) -> Result<S::Ok, S::Error> {
            match d {
                Some(d) => super::serialize(d, s),
                None => s.serialize_none(),
            }
        }

        /// A `null` reads as absent; see [`super::deserialize`] otherwise.
        pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
            d: D,
        ) -> Result<Option<Duration>, D::Error> {
            let ms = <Option<f64> as serde::Deserialize>::deserialize(d)?;
            Ok(ms.map(|ms| {
                Duration::try_from_secs_f64(ms.max(0.0) / 1000.0).unwrap_or(Duration::MAX)
            }))
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Duration;

        use serde::Deserialize;

        #[derive(Deserialize)]
        struct Timed {
            #[serde(with = "super")]
            ms: Duration,
        }

        fn parse(ms: &str) -> Duration {
            serde_json::from_str::<Timed>(&format!(r#"{{"ms":{ms}}}"#))
                .unwrap()
                .ms
        }

        #[test]
        fn durations_out_of_range_saturate_instead_of_panicking() {
            assert_eq!(parse("1.5"), Duration::from_micros(1500));
            assert_eq!(parse("-3"), Duration::ZERO);
            assert_eq!(parse("3e22"), Duration::MAX);
            assert_eq!(parse("1e303"), Duration::MAX);
        }

        #[test]
        fn a_saturated_duration_serializes_as_a_finite_number() {
            let mut out = Vec::new();
            super::serialize(&Duration::MAX, &mut serde_json::Serializer::new(&mut out)).unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.parse::<f64>().unwrap().is_finite(), "{text}");
        }
    }
}

/// For `skip_serializing_if`.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
const fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests;
