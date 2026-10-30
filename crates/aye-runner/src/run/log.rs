//! The log thread: it stamps and delivers the run's records as the lanes
//! send them, keeps the summary, and decides when the run stops.

use std::collections::HashMap;
use std::time::Instant;

use aye_events::{
    AdapterName, BatchIndex, Diagnostic, Event, Failure, LifetimeId, Record, Severity, Stamper,
    StopReason, WorkerIndex,
};

use crate::summary::{BatchSummary, RunSummary};
use crate::worker::timeouts::INTERRUPT_GRACE;

use super::Shared;
use super::message::Message;

/// The run's side: stamps and delivers records, keeps the summary, and
/// decides when to stop.
pub(super) struct Log<'a> {
    pub(super) stamper: &'a mut Stamper,
    pub(super) on_record: &'a mut dyn FnMut(Record),
    pub(super) summary: RunSummary,
    /// Each batch between its started and finished.
    pub(super) running: HashMap<BatchIndex, Running>,
    /// The run stopped (`maxfail`, the stop handle): workers' records are
    /// dropped.
    pub(super) halted: bool,
    /// The run killed every busy worker (the stop handle).
    pub(super) killed: bool,
    /// When `maxfail` ran out: when the workers it interrupted are killed,
    /// if they are still busy.
    pub(super) kill_at: Option<Instant>,
}

/// A batch the log has seen start.
#[derive(Debug, Default)]
pub(super) struct Running {
    adapter: AdapterName,
    cases: u32,
    /// Its share of `maxfail`, and the bad outcomes it reported.
    maxfail: Option<u32>,
    bad: u32,
    /// The lifetimes it started in the log and hasn't finished, innermost
    /// last.
    open: Vec<LifetimeId>,
}

impl Running {
    /// Whether it reached its own `maxfail`, so the framework stops it and
    /// tears down by itself (see [`Shared::wind_down`]).
    fn finishing(&self) -> bool {
        self.maxfail.is_some_and(|max| self.bad >= max)
    }

    /// Note a lifetime as finished; whether it was open.
    fn close(&mut self, lifetime: &LifetimeId) -> bool {
        let at = self.open.iter().rposition(|open| open == lifetime);
        at.map(|at| self.open.remove(at)).is_some()
    }
}

impl Log<'_> {
    pub(super) fn deliver(
        &mut self,
        worker: Option<WorkerIndex>,
        batch: Option<BatchIndex>,
        event: Event,
    ) {
        let mut record = Record::new(event);
        record.envelope.worker = worker;
        record.envelope.batch = batch;
        self.forward(record);
    }

    fn forward(&mut self, record: Record) {
        let record = self.stamper.stamp(record);
        (self.on_record)(record);
    }

    /// Stop at once: kill every busy worker.
    pub(super) fn kill(&mut self, shared: &Shared<'_>) {
        self.halted = true;
        self.killed = true;
        shared.halt();
    }

    /// Record why the run stops; the first reason stays.
    pub(super) fn stop(&mut self, reason: StopReason) {
        self.summary.stopped.get_or_insert(reason);
    }

    pub(super) fn handle(&mut self, message: Message, shared: &Shared<'_>, maxfail: Option<u32>) {
        match message {
            Message::WorkerStarted {
                worker,
                pid,
                python,
                version,
            } => self.deliver(
                Some(worker),
                None,
                Event::WorkerStarted {
                    pid,
                    python,
                    version: (!version.is_empty()).then_some(version),
                },
            ),
            Message::WorkerFailed { worker, error } => {
                self.summary.internal_error = true;
                self.deliver(
                    Some(worker),
                    None,
                    Event::WorkerFinished {
                        exit_status: None,
                        error: Some(error),
                    },
                );
            }
            Message::BatchStarted {
                worker,
                batch,
                adapter,
                cases,
                maxfail,
            } => {
                self.running.insert(
                    batch,
                    Running {
                        adapter: adapter.clone(),
                        cases,
                        maxfail,
                        ..Running::default()
                    },
                );
                self.deliver(
                    Some(worker),
                    Some(batch),
                    Event::BatchStarted { adapter, cases },
                );
            }
            Message::Record(record) => {
                let running = record
                    .envelope
                    .batch
                    .and_then(|batch| self.running.get_mut(&batch));
                if self.halted {
                    // Batches tear down as they end, as pytest's `-x` and
                    // unittest's `-f` do: what closes the lifetimes the log
                    // holds open still counts, with its failure, but for the
                    // `KeyboardInterrupt` the run sent a batch it interrupted.
                    if let Event::LifetimeFinished {
                        lifetime, failure, ..
                    } = &record.event
                        && let Some(running) = running
                        && (running.finishing() || !is_interrupt(failure.as_ref()))
                        && running.close(lifetime)
                    {
                        self.forward(*record);
                    }
                    return;
                }
                let mut bad = false;
                match &record.event {
                    Event::TestFinished(result) => {
                        self.summary.counts.record(result.outcome);
                        self.summary.finished += 1;
                        bad = result.outcome.is_bad();
                        if let Some(running) = running {
                            running.bad += u32::from(bad);
                        }
                    }
                    Event::LifetimeStarted { lifetime, .. } => {
                        if let Some(running) = running {
                            running.open.push(lifetime.clone());
                        }
                    }
                    Event::LifetimeFinished { lifetime, .. } => {
                        if let Some(running) = running {
                            running.close(lifetime);
                        }
                    }
                    _ => {}
                }
                self.forward(*record);
                if bad && maxfail.is_some_and(|max| self.summary.counts.bad() >= max) {
                    self.summary.stopped_early = true;
                    self.stop(StopReason::Maxfail);
                    self.halted = true;
                    self.kill_at = Some(Instant::now() + INTERRUPT_GRACE);
                    shared.wind_down();
                }
            }
            Message::Problem {
                worker,
                batch,
                code,
                path,
                message,
            } => {
                if self.halted {
                    return;
                }
                self.summary.internal_error = true;
                self.deliver(
                    Some(worker),
                    Some(batch),
                    Event::Diagnostic(Diagnostic {
                        severity: Severity::Error,
                        code,
                        path,
                        span: None,
                        message,
                    }),
                );
            }
            Message::Crashed(crash) => {
                self.summary.internal_error |= crash.test.is_none();
                self.summary.crashes.push(crash);
            }
            Message::Warning { worker, diagnostic } => {
                if !self.halted {
                    self.deliver(Some(worker), None, Event::Diagnostic(diagnostic));
                }
            }
            Message::BatchFinished {
                worker,
                batch,
                status,
                end,
                verdict,
                not_run,
            } => {
                // Once the run has halted, the batch's records were dropped:
                // its status counts no more than they do.
                if !self.halted {
                    self.summary.internal_error |= verdict.internal_error;
                    self.summary.framework_failed |= verdict.failed;
                }
                if verdict.stop && self.summary.stopped.is_none() {
                    // pytest exits 3 after an internal error, not 2.
                    self.summary.interrupted |= !verdict.internal_error;
                    self.stop(StopReason::Framework);
                }
                self.summary.not_run.extend(not_run);
                let Running { adapter, cases, .. } =
                    self.running.remove(&batch).unwrap_or_default();
                self.summary.batches.push(BatchSummary {
                    id: batch,
                    adapter,
                    worker,
                    cases,
                    status,
                    end,
                });
                self.deliver(Some(worker), Some(batch), Event::BatchFinished { status });
            }
            Message::WorkerFinished {
                worker,
                exit_status,
                error,
            } => self.deliver(
                Some(worker),
                None,
                Event::WorkerFinished { exit_status, error },
            ),
        }
    }
}

/// Whether `failure` is a `KeyboardInterrupt`'s.
fn is_interrupt(failure: Option<&Failure>) -> bool {
    failure.is_some_and(|failure| failure.exception.as_deref() == Some("KeyboardInterrupt"))
}
