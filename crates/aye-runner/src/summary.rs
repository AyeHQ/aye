//! What a run reports beside its records: [`RunSummary`], with each
//! batch's [`BatchSummary`] and each [`Crash`].

use aye_events::{
    AdapterName, BatchIndex, Counts, FrameworkStatus, StopReason, TestId, WorkerIndex,
};

/// What happened, beyond the records themselves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunSummary {
    /// Every batch handed to a worker, by id: in hand-out order.
    pub batches: Vec<BatchSummary>,
    /// A fold over the delivered `test_finished` records.
    pub counts: Counts,
    /// Cases in the plan.
    pub requested: u32,
    /// `test_finished` records delivered. A case collected without its
    /// parameters finishes as several tests, so this may exceed `requested`.
    pub finished: u32,
    /// Something failed outside any test: a worker that could not start, a
    /// worker that died between tests, an adapter that raised, a protocol
    /// error, a case no executor handles, a batch the framework ended with
    /// an internal error (pytest's status 3).
    pub internal_error: bool,
    /// A batch ended with the framework's status for failed tests
    /// (pytest's 1) although none of its tests failed and it reported no
    /// error: a conftest or plugin failed the session after the fact
    /// (`session.exitstatus`, pytest-cov's `--cov-fail-under`). pytest
    /// exits 1 then. Not counted once the run has halted.
    pub framework_failed: bool,
    /// `maxfail` was reached and the rest abandoned.
    pub stopped_early: bool,
    /// The run was interrupted: by the [`StopHandle`](crate::StopHandle), or by the framework
    /// stopping a batch other than for an internal error (see
    /// [`StopReason::Framework`]).
    pub interrupted: bool,
    /// Why the run stopped before every requested test ran, if it did: the
    /// first reason that came up.
    pub stopped: Option<StopReason>,
    /// Requested ids none of whose tests started: the rest of each batch the
    /// run's stop cut short, and every batch never handed out (the run
    /// stopped, or no worker could start). Tests the framework deselects at
    /// runtime aren't in it.
    pub not_run: Vec<TestId>,
    pub crashes: Vec<Crash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSummary {
    pub id: BatchIndex,
    pub adapter: AdapterName,
    pub worker: WorkerIndex,
    pub cases: u32,
    /// The framework's own status, from `batch_done`.
    pub status: Option<FrameworkStatus>,
    pub end: BatchEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchEnd {
    /// The worker answered `batch_done`.
    Done,
    /// The adapter raised outside any test.
    Failed,
    /// The worker died.
    Crashed,
    /// The run stopped (`maxfail` or the stop handle) and interrupted or
    /// killed the worker.
    Stopped,
}

/// A worker that died before finishing its batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crash {
    pub worker: WorkerIndex,
    pub batch: BatchIndex,
    /// The test that was running, if any.
    pub test: Option<TestId>,
    /// `exit status 3`, `killed by signal 11 (SIGSEGV)`.
    pub reason: String,
    /// The end of the worker's stderr.
    pub stderr: String,
}
