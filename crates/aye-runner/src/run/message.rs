//! What the lanes tell the log thread.

use aye_events::{
    AdapterName, BatchIndex, Diagnostic, DiagnosticCode, DiagnosticPath, FrameworkStatus, Record,
    TestId, WorkerIndex,
};

use crate::summary::{BatchEnd, Crash};

use super::batch::Verdict;

/// What lanes tell the run.
#[derive(Debug)]
pub(super) enum Message {
    WorkerStarted {
        worker: WorkerIndex,
        pid: u32,
        python: String,
        version: String,
    },
    /// A worker could not start.
    WorkerFailed {
        worker: WorkerIndex,
        error: String,
    },
    BatchStarted {
        worker: WorkerIndex,
        batch: BatchIndex,
        adapter: AdapterName,
        cases: u32,
        /// The batch's share of `maxfail`.
        maxfail: Option<u32>,
    },
    /// From a worker, or made for a test a crash interrupted.
    Record(Box<Record>),
    /// Something the worker sent that breaks the protocol, or an adapter
    /// that raised outside any test.
    Problem {
        worker: WorkerIndex,
        batch: BatchIndex,
        code: DiagnosticCode,
        path: DiagnosticPath,
        message: String,
    },
    Crashed(Crash),
    /// A warning outside any batch (a lane's `listing-differs`).
    Warning {
        worker: WorkerIndex,
        diagnostic: Diagnostic,
    },
    BatchFinished {
        worker: WorkerIndex,
        batch: BatchIndex,
        status: Option<FrameworkStatus>,
        end: BatchEnd,
        /// What the framework's end of the batch means for the run.
        verdict: Verdict,
        /// Its ids none of whose tests started because the batch was cut
        /// short.
        not_run: Vec<TestId>,
    },
    WorkerFinished {
        worker: WorkerIndex,
        exit_status: Option<i32>,
        error: Option<String>,
    },
}
