//! A batch: as planned, as handed to a lane, and as it waits in the queue.

use std::time::Duration;

use aye_events::{AdapterName, BatchIndex, TestId, WorkerIndex};

/// A batch before it is handed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub adapter: AdapterName,
    pub ids: Vec<TestId>,
    pub weight: Duration,
    /// Holds a case that must not run alongside other serial cases.
    pub serial: bool,
    /// Plan position of its first case.
    pub(super) first: usize,
    /// Holds a test to run first.
    pub(super) priority: bool,
}

/// A batch as handed to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assigned {
    pub id: BatchIndex,
    pub adapter: AdapterName,
    pub ids: Vec<TestId>,
    pub serial: bool,
    /// Already re-queued once after a crash.
    pub retry: bool,
    /// For a re-queued batch: the tests that ran in the batch that crashed,
    /// which the adapter must not run again (see `run.rs`).
    pub ran: Vec<TestId>,
    /// The only lane that may run it: the first, for the tests of files
    /// another lane lists otherwise ([`Queue::pin`]).
    pub lane: Option<WorkerIndex>,
    /// Holds a test to run first.
    pub(super) priority: bool,
    /// When it was handed out, nothing else of its adapter was left for
    /// its lane, and nothing could be pinned to it: its worker's last batch
    /// ([`aye_protocol::Batch::ends_session`]), unless a crashed batch comes
    /// back. Not kept when it goes back to the queue.
    pub ends_session: bool,
}

#[derive(Debug)]
pub(super) struct Pending {
    /// Kept when a batch is put back unstarted, so ids stay dense.
    pub(super) id: Option<BatchIndex>,
    pub(super) adapter: AdapterName,
    pub(super) ids: Vec<TestId>,
    pub(super) serial: bool,
    pub(super) retry: bool,
    pub(super) ran: Vec<TestId>,
    pub(super) lane: Option<WorkerIndex>,
    pub(super) priority: bool,
}

impl Pending {
    pub(super) fn of(batch: Assigned, id: Option<BatchIndex>) -> Self {
        Self {
            id,
            adapter: batch.adapter,
            ids: batch.ids,
            serial: batch.serial,
            retry: batch.retry,
            ran: batch.ran,
            lane: batch.lane,
            priority: batch.priority,
        }
    }

    pub(super) fn eligible(&self, lane: WorkerIndex) -> bool {
        self.lane.is_none_or(|only| only == lane)
    }
}
