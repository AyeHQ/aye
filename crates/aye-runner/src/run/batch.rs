//! Running a batch on a lane's worker: the request, the records read
//! back, and what its end means for the run.

use aye_events::{AdapterName, DiagnosticCode, DiagnosticPath, FrameworkStatus, WorkerIndex};
use aye_protocol::{Batch, Reply, Request, WorkerLine};

use crate::schedule::Assigned;
use crate::summary::BatchEnd;
use crate::worker::{Incoming, Worker, is_reply, shorten};

use super::Shared;
use super::crash::Track;
use super::message::Message;

/// How a batch ended, as its lane saw it.
enum End {
    /// `batch_done`, with the framework's status.
    Done {
        status: Option<FrameworkStatus>,
        track: Track,
    },
    Failed(String, Track),
    Eof(Track),
}

impl Shared<'_> {
    /// Run `batch` on the lane's `worker` (busy already) and report it: its
    /// start, its records, its end. A worker that dies meanwhile is gone
    /// from `worker` ([`Shared::crashed`]).
    pub(super) fn execute(
        &self,
        lane: WorkerIndex,
        worker: &mut Option<Worker>,
        batch: &Assigned,
        send: &dyn Fn(Message),
    ) {
        let running = worker.as_mut().expect("a worker runs the batch");
        let maxfail = self.budget();
        send(Message::BatchStarted {
            worker: lane,
            batch: batch.id,
            adapter: batch.spec.adapter.clone(),
            cases: u32::try_from(batch.spec.ids.len()).unwrap_or(u32::MAX),
            maxfail,
        });
        let end = self.run_batch(lane, running, batch, maxfail, send);
        let interrupted = self.idle(lane);
        match end {
            End::Done { status, track } => {
                let remaining = track.remaining(&batch.spec.ids);
                let verdict = Verdict::of(status, track.stopped, track.clean());
                if verdict.stop {
                    // pytest ends its session there; so does the run.
                    self.queue.halt();
                }
                let cut =
                    interrupted || verdict.stop || maxfail.is_some_and(|max| track.bad >= max);
                send(Message::BatchFinished {
                    worker: lane,
                    batch: batch.id,
                    status,
                    end: if interrupted {
                        BatchEnd::Stopped
                    } else {
                        BatchEnd::Done
                    },
                    verdict,
                    not_run: if cut { remaining } else { Vec::new() },
                });
            }
            End::Failed(message, track) => {
                send(Message::Problem {
                    worker: lane,
                    batch: batch.id,
                    code: DiagnosticCode::BatchFailed,
                    path: first_path(batch),
                    message: format!(
                        "the {} adapter failed outside any test:\n{message}",
                        batch.spec.adapter
                    ),
                });
                send(Message::BatchFinished {
                    worker: lane,
                    batch: batch.id,
                    status: None,
                    end: BatchEnd::Failed,
                    verdict: Verdict::default(),
                    not_run: track.remaining(&batch.spec.ids),
                });
            }
            End::Eof(track) => {
                let dead = worker.take().expect("running above");
                self.crashed(lane, dead, batch, track, send);
            }
        }
    }

    /// End the session of the lane's `worker`, of `adapter`, whose last
    /// batch didn't, in a batch of its own ([`Queue::closing`](crate::schedule::Queue::closing)), so that
    /// what fails there is reported as the session's end and its teardown
    /// isn't cut short by the worker's shutdown, when its executor lists the
    /// session. Not once the run is stopped (its workers are being killed);
    /// after `maxfail`, it still tears the session down, as pytest's `-x`
    /// does, and the log keeps what it reports as it keeps a halted run's.
    pub(super) fn end_session(
        &self,
        lane: WorkerIndex,
        worker: &mut Option<Worker>,
        adapter: &AdapterName,
        send: &dyn Fn(Message),
    ) {
        if self.stop.is_stopped() || !self.lists_session(adapter) {
            return;
        }
        let Some(running) = worker.as_mut() else {
            return;
        };
        // Registered busy so that a stop kills it; `maxfail` running out
        // interrupts only the workers busy then.
        self.busy(lane, running.killer());
        if self.stop.is_stopped() {
            self.idle(lane);
            return;
        }
        let batch = self.queue.closing(adapter);
        self.execute(lane, worker, &batch, send);
    }

    /// Run `batch` on `worker`, with `maxfail` its share of the run's.
    fn run_batch(
        &self,
        lane: WorkerIndex,
        worker: &mut Worker,
        batch: &Assigned,
        maxfail: Option<u32>,
        send: &dyn Fn(Message),
    ) -> End {
        let mut policy = self.policy.clone();
        policy.maxfail = maxfail;
        let mut context = self.plan.context(&batch.spec.adapter);
        context.ran_ids = batch.spec.origin.ran().to_vec();
        let request = Request::Run(Batch {
            id: batch.id,
            adapter: batch.spec.adapter.clone(),
            ids: batch.spec.ids.clone(),
            policy,
            context,
            ends_session: batch.ends_session && self.lists_session(&batch.spec.adapter),
        });
        // A worker that died since its last batch shows up below as an end
        // of stream.
        let _ = worker.send(&request);
        let mut track = Track::default();
        let problem = |message: String| Message::Problem {
            worker: lane,
            batch: batch.id,
            code: DiagnosticCode::ProtocolError,
            path: first_path(batch),
            message,
        };
        loop {
            match worker.recv() {
                Incoming::Line(WorkerLine::Record(mut record)) => {
                    track.see(&record.event);
                    if maxfail.is_some_and(|max| track.bad >= max) {
                        self.ends_by_itself(lane);
                    }
                    record.envelope.worker = Some(lane);
                    record.envelope.batch.get_or_insert(batch.id);
                    self.pass_on(*record, send);
                }
                Incoming::Line(WorkerLine::Reply(Reply::BatchDone {
                    batch: done,
                    status,
                })) => {
                    if done != batch.id {
                        send(problem(format!(
                            "worker {lane} finished batch {done} while running batch {}",
                            batch.id
                        )));
                    }
                    return End::Done { status, track };
                }
                Incoming::Line(WorkerLine::Reply(Reply::Failed { message })) => {
                    return End::Failed(message, track);
                }
                Incoming::Line(reply @ WorkerLine::Reply(_)) => {
                    send(problem(format!(
                        "worker {lane} sent an unexpected reply: {}",
                        Incoming::Line(reply)
                    )));
                }
                Incoming::Garbage { line, error } => {
                    send(problem(format!(
                        "worker {lane} sent a line that is not a protocol message ({error}): {}",
                        shorten(&line)
                    )));
                    // A reply that doesn't parse can only have been the end
                    // of the batch; waiting for another would hang.
                    if is_reply(&line) {
                        return End::Done {
                            status: None,
                            track,
                        };
                    }
                }
                Incoming::Eof => return End::Eof(track),
            }
        }
    }
}

/// What the framework's end of a batch means for the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Verdict {
    /// The framework stopped the batch, ending its session there, so the
    /// run hands nothing more out (see [`StopReason::Framework`](aye_events::StopReason::Framework)).
    pub(super) stop: bool,
    /// The framework failed internally: pytest's status 3.
    pub(super) internal_error: bool,
    /// The framework says tests failed (pytest's status 1), and none did:
    /// see [`RunSummary::framework_failed`](crate::RunSummary::framework_failed).
    pub(super) failed: bool,
}

impl Verdict {
    /// From the batch's `batch_done` status, and whether the adapter said
    /// the framework stopped it (a `batch-stopped` error diagnostic). The
    /// diagnostic says it, whatever the status: `pytest.exit(returncode=4)`
    /// makes the status 4, which is no usage error, and a status of 0 to 5
    /// says nothing about the tests left. Of the statuses, only pytest's 2
    /// says it alone. Usage errors are the binary's to find, from their
    /// diagnostics. Status 1 from a `clean` batch (no bad outcome, no error)
    /// fails the run.
    fn of(status: Option<FrameworkStatus>, stopped: bool, clean: bool) -> Self {
        Self {
            stop: stopped || status == Some(FrameworkStatus::INTERRUPTED),
            internal_error: status == Some(FrameworkStatus::INTERNAL_ERROR),
            failed: clean && status == Some(FrameworkStatus::TESTS_FAILED),
        }
    }
}

fn first_path(batch: &Assigned) -> DiagnosticPath {
    batch
        .spec
        .ids
        .first()
        .map_or(DiagnosticPath::Nowhere, |id| {
            DiagnosticPath::as_written(id.path().as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_framework_stops_the_run_by_a_batch_stopped_error_or_status_2() {
        let stop = Verdict {
            stop: true,
            ..Verdict::default()
        };
        assert_eq!(
            Verdict::of(Some(FrameworkStatus::new(2)), false, false),
            stop
        );
        // `pytest.exit(returncode=…)`, whatever the code: 4 is no usage error.
        for status in [
            Some(FrameworkStatus::new(0)),
            Some(FrameworkStatus::new(2)),
            Some(FrameworkStatus::new(4)),
            Some(FrameworkStatus::new(5)),
            Some(FrameworkStatus::new(7)),
            None,
        ] {
            assert_eq!(Verdict::of(status, true, false), stop, "{status:?}");
        }
        // A status alone says nothing about the tests left, but 2.
        for status in [
            Some(FrameworkStatus::new(0)),
            Some(FrameworkStatus::new(1)),
            Some(FrameworkStatus::new(4)),
            Some(FrameworkStatus::new(5)),
            Some(FrameworkStatus::new(7)),
            Some(FrameworkStatus::new(-1)),
            None,
        ] {
            assert_eq!(
                Verdict::of(status, false, false),
                Verdict::default(),
                "{status:?}"
            );
        }
    }

    #[test]
    fn status_3_is_an_internal_error_and_stops_the_run_only_when_the_batch_was_stopped() {
        assert_eq!(
            Verdict::of(Some(FrameworkStatus::new(3)), true, false),
            Verdict {
                stop: true,
                internal_error: true,
                failed: false,
            }
        );
        // pytest's INTERNALERROR in `pytest_sessionfinish`: every test ran.
        assert_eq!(
            Verdict::of(Some(FrameworkStatus::new(3)), false, true),
            Verdict {
                stop: false,
                internal_error: true,
                failed: false,
            }
        );
    }

    #[test]
    fn status_1_from_a_batch_where_nothing_failed_fails_the_run() {
        // A conftest's `session.exitstatus = 1`, `--cov-fail-under`.
        assert!(Verdict::of(Some(FrameworkStatus::new(1)), false, true).failed);
        // A bad test or an error diagnostic explains it.
        assert!(!Verdict::of(Some(FrameworkStatus::new(1)), false, false).failed);
        for status in [
            Some(FrameworkStatus::new(0)),
            Some(FrameworkStatus::new(5)),
            None,
        ] {
            assert!(!Verdict::of(status, false, true).failed, "{status:?}");
        }
    }
}
