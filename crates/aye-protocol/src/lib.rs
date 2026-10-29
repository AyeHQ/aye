//! The worker protocol: what the runner and a worker say to each other. The
//! runner sends [`Request`]s (the handshake, with the modules to install and
//! the executors to build; a refinement; a batch with its [`Policy`] and its
//! [`BatchContext`]: the adapter's [`AdapterContext`] and the keys the binary
//! and the runner add, [`context_keys`]; the shutdown), and the worker
//! answers with [`WorkerLine`]s, each a control [`Reply`] or an event
//! [`Record`]. Beside the messages: a `maxfail` limit ([`MaxFail`]), the
//! protocol's version ([`PROTOCOL_VERSION`]), the services a worker may
//! provide ([`ServiceName`]), an adapter's runtime face ([`Executor`], with
//! the [`ModuleSet`] it installs, of [`ModuleSource`]s), and the
//! [`Refiner`] trait, which the runner implements and collection calls.
//! `docs/protocol.md` describes the protocol on the wire.
//!
//! Runner → engine: services inside the worker (capture, coverage, soft sandbox,
//! timeouts, current-test context, cleanup registration, resources, the
//! patching library), the selection to apply, the run policy, the event sink.
//!
//! Owns: the messages and how they serialize. Does not own: the events and
//! the identities they carry (`aye-events`, whose [`FrameworkStatus`] is
//! re-exported here, since a batch's end carries one), what an adapter
//! implements (`aye-adapter`), or aye's engine (`aye-engine`).
//!
//! Depends on: aye-events. Must not depend on Ruff, a framework or the
//! filesystem, so the runner can speak it without compiling an adapter.
//!
//! Open: the exact refinement messages; which policy knobs an engine must
//! honour versus may ignore.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Services are looked up by name**, with `None` when absent, so new
//!   services stay additive ([`ServiceName`] names them). An engine that
//!   needs a service it doesn't get degrades or reports, never crashes.
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

use std::collections::BTreeMap;
use std::num::NonZeroU32;

use aye_events::{AdapterName, BatchIndex, Case, Diagnostic, ProjectPath, Record, TestId};
use serde::{Deserialize, Serialize};

mod context;
mod module;
mod service;

pub use aye_events::FrameworkStatus;
pub use context::{AdapterContext, BatchContext, context_keys};
pub use module::{ModuleSet, ModuleSource};
pub use service::ServiceName;

/// Version of the runner ↔ worker protocol. Checked at the handshake.
/// 2: [`Executor::lists_session`], [`RefineRequest::digests_only`],
/// [`Refined::digests`] and the executors' `close()` at shutdown.
/// 3: [`Batch::ends_session`].
pub const PROTOCOL_VERSION: u32 = 3;

/// An adapter's runtime face: modules the worker installs, and the executor
/// class it instantiates.
#[derive(Debug, Clone)]
pub struct Executor {
    pub adapter: AdapterName,
    /// Installed in order, after aye's engine (`aye_engine::ENGINE`).
    pub modules: ModuleSet,
    /// `module:Class` of the `aye_engine.Executor` subclass.
    pub entry: &'static str,
    /// The executor keeps what it lists for the batches it runs later (its
    /// imported files, its model), so a worker lists the session once: the
    /// runner keeps the worker that refined as its first lane, and every
    /// other lane lists the session's files before its first batch. The
    /// native pytest executor's; the others list nothing they keep.
    pub lists_session: bool,
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
    /// The adapter's context. A lane's listing before its first batch
    /// ([`Self::digests_only`]) gets the keys the binary adds to its
    /// batches too, so that it lists the session those batches run in.
    #[serde(default)]
    pub context: BatchContext,
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
    /// by requested file, a digest of its tests' ids in order, so that two
    /// workers' listings can be compared without the cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digests: Option<BTreeMap<ProjectPath, ListingDigest>>,
}

/// A digest of the tests a worker lists in a file, their ids in order: the
/// executor's own, or the worker's, the SHA-1 of the ids joined by
/// newlines. Two workers that list a file alike give it the same digest,
/// and the runner only compares them; it is kept as written, and
/// serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ListingDigest(Box<str>);

impl ListingDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
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

/// A `maxfail` limit: stop after this many bad outcomes. Never 0: a run
/// without a limit (`--maxfail 0`) has no `MaxFail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MaxFail(NonZeroU32);

impl MaxFail {
    /// The limit of `n` bad outcomes; `None` for 0, no limit.
    pub const fn new(n: u32) -> Option<Self> {
        match NonZeroU32::new(n) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
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
    pub context: BatchContext,
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
        /// Services to provide.
        services: Vec<ServiceName>,
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
        status: Option<FrameworkStatus>,
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
