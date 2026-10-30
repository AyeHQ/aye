//! A batch: what it runs and how it may be handed out ([`BatchSpec`]), as
//! planned, as it waits in the queue, and as handed to a lane.

use std::time::Duration;

use aye_events::{AdapterName, BatchIndex, TestId, WorkerIndex};

/// What a batch runs, and how it may be handed out: what the scheduler
/// hands a lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchSpec {
    pub adapter: AdapterName,
    /// In the order they run. None for a batch that only ends a session
    /// ([`BatchOrigin::Closing`]).
    pub ids: Vec<TestId>,
    /// Holds a case that must not run alongside other serial cases.
    pub serial: bool,
    pub priority: Priority,
    pub origin: BatchOrigin,
    pub pin: Pin,
}

impl BatchSpec {
    /// One of the plan's batches, which any lane may run.
    pub(crate) fn planned(
        adapter: AdapterName,
        ids: Vec<TestId>,
        serial: bool,
        priority: Priority,
    ) -> Self {
        Self {
            adapter,
            ids,
            serial,
            priority,
            origin: BatchOrigin::Planned,
            pin: Pin::AnyLane,
        }
    }

    /// A batch of `adapter` with no ids, which only ends its worker's
    /// session.
    pub(crate) fn closing(adapter: AdapterName) -> Self {
        Self {
            adapter,
            ids: Vec::new(),
            serial: false,
            priority: Priority::Normal,
            origin: BatchOrigin::Closing,
            pin: Pin::AnyLane,
        }
    }
}

/// Whether a batch holds a test to run first (`--ff`). Ordered as batches
/// are handed out: those that do first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Priority {
    /// It holds a test to run first.
    RunsFirst,
    Normal,
}

/// Where a batch comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BatchOrigin {
    /// The plan, or the part of a planned batch pinned to the first lane.
    Planned,
    /// A crashed batch's ids that never started under their own id,
    /// re-queued once: a retry isn't re-queued again. `ran` holds the tests
    /// that ran in the batch that crashed, which the adapter must not run
    /// again (see `run/crash.rs`).
    Retry { ran: Vec<TestId> },
    /// No ids: it only ends its worker's session
    /// ([`Queue::closing`](super::Queue::closing)).
    Closing,
}

impl BatchOrigin {
    /// The tests that ran before, which the adapter must not run again: a
    /// retry's, none for any other batch.
    pub(crate) fn ran(&self) -> &[TestId] {
        match self {
            Self::Retry { ran } => ran,
            Self::Planned | Self::Closing => &[],
        }
    }
}

/// The lanes that may run a batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pin {
    AnyLane,
    /// The first lane only: the tests of files another lane lists otherwise
    /// ([`Queue::pin`](super::Queue::pin)).
    FirstLane,
}

impl Pin {
    /// Whether `lane` may run the batch.
    pub(crate) fn allows(self, lane: WorkerIndex) -> bool {
        match self {
            Self::AnyLane => true,
            Self::FirstLane => lane.is_first(),
        }
    }
}

/// A batch before it is handed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub spec: BatchSpec,
    pub weight: Duration,
    /// Plan position of its first case.
    pub(super) first: usize,
}

/// A batch as handed to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assigned {
    pub id: BatchIndex,
    pub spec: BatchSpec,
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
    pub(super) spec: BatchSpec,
}

impl Pending {
    pub(super) fn of(batch: Assigned, id: Option<BatchIndex>) -> Self {
        Self {
            id,
            spec: batch.spec,
        }
    }

    pub(super) fn eligible(&self, lane: WorkerIndex) -> bool {
        self.spec.pin.allows(lane)
    }
}
