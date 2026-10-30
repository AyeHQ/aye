//! `maxfail` across the lanes ([`MaxfailBudget`], each batch's [`Share`]),
//! and how the run stops: the run's [`RunPhase`] as the framework stops a
//! batch, as `maxfail` runs out (the busy workers interrupted, but those
//! whose batch reached its share, which end by themselves) and as the run
//! is stopped (killed).

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use aye_events::{Event, Record, WorkerIndex};
use aye_protocol::MaxFail;

use crate::sync::lock;
use crate::worker::Killer;
use crate::worker::timeouts::INTERRUPT_GRACE;

use super::Shared;
use super::log::RunPhase;
use super::message::Message;

/// `maxfail` across a run's lanes: its limit, and the bad outcomes the
/// lanes have passed on so far.
///
/// The run stops on the log's own count of the outcomes it delivered
/// ([`MaxfailBudget::reached`]); the lanes read theirs to stop taking
/// batches without waiting for it ([`MaxfailBudget::exhausted`]) and to
/// size each batch's [`Share`]. Once it is reached, the log drops what the
/// busy workers still send, but the `lifetime_finished` records that close
/// lifetimes it holds open (see [`RunPhase::WindingDown`]).
#[derive(Debug)]
pub(super) struct MaxfailBudget {
    /// `None` for no limit: `maxfail` 0, as in pytest.
    limit: Option<MaxFail>,
    passed_on: AtomicU32,
}

impl MaxfailBudget {
    /// The plan's `maxfail`, 0 or none being no limit.
    pub(super) fn new(maxfail: Option<u32>) -> Self {
        Self {
            limit: maxfail.and_then(MaxFail::new),
            passed_on: AtomicU32::new(0),
        }
    }

    /// The next batch's share: what is left, at least 1.
    pub(super) fn share(&self) -> Share {
        Share(self.limit.and_then(|max| {
            let left = max
                .get()
                .saturating_sub(self.passed_on.load(Ordering::SeqCst));
            MaxFail::new(left.max(1))
        }))
    }

    /// Whether the bad outcomes passed on already reach the limit, so the
    /// run is about to stop: lanes take no more batches.
    pub(super) fn exhausted(&self) -> bool {
        self.limit
            .is_some_and(|max| self.passed_on.load(Ordering::SeqCst) >= max.get())
    }

    /// Count what a lane passes on to the log: a bad outcome.
    fn count(&self, event: &Event) {
        if let Event::TestFinished(result) = event
            && result.outcome.is_bad()
        {
            self.passed_on.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Whether `bad` outcomes delivered reach the limit: the run stops.
    pub(super) fn reached(&self, bad: u32) -> bool {
        self.limit.is_some_and(|max| bad >= max.get())
    }
}

/// A batch's share of `maxfail`, sent with it: what was left when it
/// started, at least 1; none without a limit. A batch whose bad outcomes
/// reach it ends by itself, its framework tearing down, so the run doesn't
/// interrupt it when `maxfail` runs out ([`Shared::ends_by_itself`]), and
/// its tests that never started are not run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Share(Option<MaxFail>);

impl Share {
    /// As the batch's policy says it.
    pub(super) fn limit(self) -> Option<u32> {
        self.0.map(MaxFail::get)
    }

    /// Whether the batch's `bad` outcomes reach it.
    pub(super) fn reached(self, bad: u32) -> bool {
        self.0.is_some_and(|max| bad >= max.get())
    }
}

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
    /// The framework stopped a batch, ending its session there: hand
    /// nothing more out.
    pub(super) fn framework_stopped(&self) {
        {
            let mut phase = lock(&self.phase);
            if *phase == RunPhase::Running {
                *phase = RunPhase::FrameworkStopped;
            }
        }
        self.queue.halt();
    }

    /// Hand nothing more out and kill the busy workers (the stop handle).
    pub(super) fn halt(&self) {
        *lock(&self.phase) = RunPhase::Killed;
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
    /// none) and killed if still busy after [`INTERRUPT_GRACE`]
    /// ([`Shared::kill_interrupted`]).
    pub(super) fn wind_down(&self) {
        *lock(&self.phase) = RunPhase::WindingDown {
            kill_at: Some(Instant::now() + INTERRUPT_GRACE),
        };
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

    /// The grace after `maxfail` is over: kill the workers
    /// [`Shared::wind_down`] interrupted that are still busy, once.
    pub(super) fn kill_interrupted(&self) {
        if let RunPhase::WindingDown { kill_at } = &mut *lock(&self.phase) {
            *kill_at = None;
        }
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

    /// Whether the run is stopping: it hands nothing more out, or its stop
    /// handle was used.
    pub(super) fn stopping(&self) -> bool {
        !self.phase().hands_out() || self.stop.is_stopped()
    }

    /// Mark a worker idle; whether the run interrupted it.
    pub(super) fn idle(&self, lane: WorkerIndex) -> bool {
        lock(&self.busy)
            .remove(&lane)
            .is_some_and(|busy| busy.stopping == Stopping::Interrupted)
    }

    /// Pass a record on, counting bad outcomes in the run's
    /// [`MaxfailBudget`].
    pub(super) fn pass_on(&self, record: Record, send: &dyn Fn(Message)) {
        self.maxfail.count(&record.event);
        send(Message::Record(Box::new(record)));
    }
}

#[cfg(test)]
mod tests {
    use aye_events::{Outcome, TestId, TestResult};

    use super::*;

    fn finished(outcome: Outcome) -> Event {
        Event::TestFinished(Box::new(TestResult::new(TestId::new("a.py::t"), outcome)))
    }

    #[test]
    fn each_batch_gets_what_is_left_of_maxfail_and_at_least_one() {
        let budget = MaxfailBudget::new(Some(3));
        assert_eq!(budget.share().limit(), Some(3));
        budget.count(&finished(Outcome::Failed));
        budget.count(&finished(Outcome::Passed));
        budget.count(&Event::TestStarted {
            id: TestId::new("a.py::t"),
        });
        budget.count(&finished(Outcome::Error));
        assert_eq!(budget.share().limit(), Some(1));
        assert!(!budget.exhausted());
        budget.count(&finished(Outcome::Failed));
        assert!(budget.exhausted());
        budget.count(&finished(Outcome::Failed));
        assert_eq!(budget.share().limit(), Some(1));
        assert!(!budget.reached(2) && budget.reached(3) && budget.reached(4));
    }

    #[test]
    fn maxfail_zero_or_none_is_no_limit() {
        for maxfail in [None, Some(0)] {
            let budget = MaxfailBudget::new(maxfail);
            budget.count(&finished(Outcome::Failed));
            assert_eq!(budget.share(), Share::default(), "{maxfail:?}");
            assert!(!budget.exhausted() && !budget.reached(u32::MAX));
            assert!(!budget.share().reached(u32::MAX));
        }
    }

    #[test]
    fn a_batch_reaches_its_share_with_as_many_bad_outcomes() {
        let budget = MaxfailBudget::new(Some(5));
        budget.count(&finished(Outcome::Failed));
        budget.count(&finished(Outcome::Failed));
        let share = budget.share();
        assert_eq!(share.limit(), Some(3));
        assert!(!share.reached(2) && share.reached(3));
    }
}
