//! Events and the log's lines: the envelope, the record, and every event
//! kind.

use serde::{Deserialize, Serialize};

use crate::case::Case;
use crate::diagnostic::Diagnostic;
use crate::failure::Failure;
use crate::id::{AdapterName, TestId};
use crate::lifetime::{LifetimeId, Scope};
use crate::outcome::{ExitCode, Outcome};
use crate::result::{Counts, TestResult};
use crate::run::{BatchIndex, EnvironmentId, RunId, RunNumber, WorkerIndex};
use crate::serde_ext::{is_zero, millis};

/// Who stamped what on an event. All fields are optional on the wire so
/// that workers can send partial envelopes the runner completes: workers
/// stamp `batch` and a per-worker sequence and time; the runner stamps
/// `run`, `environment`, `worker` and the run-wide `seq` as it merges worker
/// streams into the run's log.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<EnvironmentId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerIndex>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch: Option<BatchIndex>,
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
        run: Option<RunNumber>,
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
        adapter: AdapterName,
        cases: u32,
    },
    /// A lifetime's setup begins. Its [`Event::LifetimeFinished`] may never
    /// come: a worker that crashes, or that a stop kills, sends none for the
    /// lifetimes it had started, so a consumer must not depend on the pair.
    /// The same lifetime id starts again in each batch that needs it.
    LifetimeStarted {
        lifetime: LifetimeId,
        /// The scope it opens: `session`, `package`, `module` or `class`.
        kind: Scope,
    },
    /// A failed setup is reported once: its failure goes here, and each test
    /// that could not run because of it is `error` with `cause` set to this
    /// lifetime instead of a copy of the failure. Adapters that cannot see
    /// lifetimes (pytest) attach the failure to each test, as pytest does.
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
    /// subtest. The test still finishes with its own outcome. In the first
    /// schema because `subTest` blocks and doctest examples need it now;
    /// snapshots will add one per snapshot.
    SubOutcome {
        id: TestId,
        name: String,
        outcome: Outcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<Failure>,
        /// Why it was skipped or expected to fail (unittest's `skipTest` in
        /// a `subTest` block), as a test carries its own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(rename = "duration_ms", with = "millis", default)]
        duration: std::time::Duration,
    },
    TestFinished(Box<TestResult>),
    /// A name and value pair about the whole run that a test recorded
    /// (pytest's `record_testsuite_property`), emitted when the test records
    /// it. Each batch is a framework session, so a session-scoped fixture
    /// that records one does so once per batch: consumers keep each pair
    /// once, and the JUnit report writes it once on its test suite.
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

#[cfg(test)]
mod tests;
