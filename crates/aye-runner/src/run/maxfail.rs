//! `maxfail` across the lanes, and how the run stops their busy workers:
//! interrupted when `maxfail` runs out, killed when the run is stopped.

use std::sync::atomic::Ordering;

use aye_events::{Event, Record, WorkerIndex};

use crate::sync::lock;
use crate::worker::Killer;

use super::Shared;
use super::message::Message;

/// A busy worker, and how the run is stopping it.
pub(super) struct Busy {
    killer: Killer,
    stopping: Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stopping {
    /// Not yet.
    No,
    /// Its batch reached its own `maxfail`: its framework ends the batch,
    /// tearing down, by itself.
    ByItself,
    /// The run interrupted it when `maxfail` ran out.
    Interrupted,
}

impl Shared<'_> {
    /// Hand nothing more out and kill the busy workers (the stop handle).
    pub(super) fn halt(&self) {
        self.killing.store(true, Ordering::SeqCst);
        self.queue.halt();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "signals every busy worker; which goes first is not observable"
        )]
        for busy in lock(&self.busy).values() {
            busy.killer.kill();
        }
    }

    /// `maxfail` ran out: hand nothing more out, and end the busy workers'
    /// batches as pytest's `-x` ends a session, tearing down. A worker
    /// whose batch reached its own `maxfail` ends it by itself; the others
    /// are interrupted (SIGINT, a `KeyboardInterrupt`; killed where there is
    /// none) and killed if still busy after [`INTERRUPT_GRACE`](crate::worker::timeouts::INTERRUPT_GRACE)
    /// ([`Shared::kill_interrupted`]).
    pub(super) fn wind_down(&self) {
        self.killing.store(true, Ordering::SeqCst);
        self.queue.halt();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "signals every busy worker; which goes first is not observable"
        )]
        for busy in lock(&self.busy).values_mut() {
            if busy.stopping == Stopping::No {
                busy.stopping = Stopping::Interrupted;
                busy.killer.interrupt();
            }
        }
    }

    /// Kill the workers [`Shared::wind_down`] interrupted that are still
    /// busy.
    pub(super) fn kill_interrupted(&self) {
        #[expect(
            clippy::iter_over_hash_type,
            reason = "signals every busy worker; which goes first is not observable"
        )]
        for busy in lock(&self.busy).values() {
            if busy.stopping == Stopping::Interrupted {
                busy.killer.kill();
            }
        }
    }

    /// Mark a worker busy; false if the run has stopped, in which case it
    /// must not start anything.
    pub(super) fn busy(&self, lane: WorkerIndex, killer: Killer) -> bool {
        let busy = Busy {
            killer,
            stopping: Stopping::No,
        };
        lock(&self.busy).insert(lane, busy);
        !self.stopping()
    }

    /// The lane's batch reached its own `maxfail`: its framework ends it,
    /// so the run doesn't interrupt it. Marked before the bad outcome is
    /// passed on, so before the run can stop for it.
    pub(super) fn ends_by_itself(&self, lane: WorkerIndex) {
        if let Some(busy) = lock(&self.busy).get_mut(&lane)
            && busy.stopping == Stopping::No
        {
            busy.stopping = Stopping::ByItself;
        }
    }

    /// Whether the run is stopping: halted, or its stop handle used.
    pub(super) fn stopping(&self) -> bool {
        self.queue.is_halted() || self.stop.is_stopped()
    }

    /// Mark a worker idle; whether the run interrupted it.
    pub(super) fn idle(&self, lane: WorkerIndex) -> bool {
        lock(&self.busy)
            .remove(&lane)
            .is_some_and(|busy| busy.stopping == Stopping::Interrupted)
    }

    /// The next batch's share of `maxfail`: what is left of it, at least 1.
    pub(super) fn budget(&self) -> Option<u32> {
        let max = self.policy.maxfail?;
        Some(max.saturating_sub(self.bad.load(Ordering::SeqCst)).max(1))
    }

    /// Whether the bad outcomes passed on already reach `maxfail`, so the
    /// run is about to stop.
    pub(super) fn exhausted(&self) -> bool {
        self.policy
            .maxfail
            .is_some_and(|max| self.bad.load(Ordering::SeqCst) >= max)
    }

    /// Pass a record on, counting bad outcomes.
    pub(super) fn pass_on(&self, record: Record, send: &dyn Fn(Message)) {
        if let Event::TestFinished(result) = &record.event
            && result.outcome.is_bad()
        {
            self.bad.fetch_add(1, Ordering::SeqCst);
        }
        send(Message::Record(Box::new(record)));
    }
}
