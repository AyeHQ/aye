//! A worker that died mid-batch: which of the batch's tests started, the
//! errors made for those it cut short, and the re-queue.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use aye_events::{
    DiagnosticCode, Event, Failure, Outcome, Record, Severity, TestId, TestResult, WorkerIndex,
};

use crate::schedule::{Assigned, BatchOrigin};
use crate::summary::{BatchEnd, Crash};
use crate::worker::Worker;

use super::Shared;
use super::batch::Verdict;
use super::message::Message;

impl Shared<'_> {
    /// The worker died before finishing `batch`. The test it was running
    /// becomes an error; the ids that never started under their own id are
    /// re-queued once, and become errors if they crash a worker again.
    ///
    /// An id that stands for several tests (one without its parameters, or
    /// whose parameters the framework makes under other ids now: pytest's
    /// stand-ins) may have started some of them: it is re-queued whole,
    /// with the ids of the tests that ran ([`BatchContext::ran_ids`](aye_protocol::BatchContext::ran_ids)), so
    /// that the adapter runs only the rest, and none twice.
    pub(super) fn crashed(
        &self,
        lane: WorkerIndex,
        dead: Worker,
        batch: &Assigned,
        track: Track,
        send: &dyn Fn(Message),
    ) {
        let exit = dead.wait();
        let closing = |end, error, not_run| {
            send(Message::BatchFinished {
                worker: lane,
                batch: batch.id,
                status: None,
                end,
                verdict: Verdict::default(),
                not_run,
            });
            send(Message::WorkerFinished {
                worker: lane,
                exit_status: exit.code(),
                error,
            });
        };
        if self.killing.load(Ordering::SeqCst) || self.stop.is_stopped() {
            // Killed by the stop, or dead meanwhile: nothing to recover.
            closing(BatchEnd::Stopped, None, track.remaining(&batch.spec.ids));
            return;
        }
        // After a framework stop, the tests re-queued below are never handed
        // out, and end up in `not_run`.
        let reason = exit.describe();
        let detail = (!exit.stderr.is_empty()).then(|| exit.stderr.clone());
        let synthesize = |id: TestId, message: String, duration: Duration| {
            let mut result = TestResult::new(id, Outcome::Error);
            result.duration = duration;
            result.failures.push(Failure {
                message,
                detail: detail.clone(),
                ..Failure::default()
            });
            let mut record = Record::new(Event::TestFinished(Box::new(result)));
            record.envelope.worker = Some(lane);
            record.envelope.batch = Some(batch.id);
            self.pass_on(record, send);
        };

        let retried = track.unfinished(&batch.spec.ids);
        let unstarted: HashSet<TestId> = track.remaining(&batch.spec.ids).into_iter().collect();
        let ran = track.ran.clone();
        let in_flight = track.in_flight.map(|(id, since)| {
            synthesize(
                id.clone(),
                format!("worker crashed while running this test: {reason}"),
                since.elapsed(),
            );
            id
        });
        if let BatchOrigin::Retry { .. } = batch.spec.origin {
            for id in retried {
                let message = if unstarted.contains(&id) {
                    format!("worker crashed again before this test ran: {reason}")
                } else {
                    format!("worker crashed again before every test of this id ran: {reason}")
                };
                synthesize(id, message, Duration::ZERO);
            }
        } else if !retried.is_empty() {
            // Not a retry: none of its tests ran before.
            self.queue.retry(batch, retried, ran);
        }

        let error = match &in_flight {
            Some(id) => format!("worker crashed while running {id} ({reason})"),
            None => match &detail {
                Some(tail) => format!("worker crashed outside any test ({reason}):\n{tail}"),
                None => format!("worker crashed outside any test ({reason})"),
            },
        };
        send(Message::Crashed(Crash {
            worker: lane,
            batch: batch.id,
            test: in_flight,
            reason,
            stderr: exit.stderr.clone(),
        }));
        closing(BatchEnd::Crashed, Some(error), Vec::new());
    }
}

/// Which of a batch's tests started and which one is running, for crash
/// recovery.
#[derive(Debug, Default)]
pub(super) struct Track {
    in_flight: Option<(TestId, Instant)>,
    /// Bad outcomes the batch reported.
    pub(super) bad: u32,
    /// Every id a started test belongs to: itself, without its parameters,
    /// and each enclosing `::` prefix (its class, its file).
    covered: HashSet<String>,
    /// The tests that started, in order, each once, and as a set.
    ran: Vec<TestId>,
    started: HashSet<TestId>,
    /// The adapter said the framework stopped the batch before running all
    /// of it, where it would have ended its whole session (pytest's
    /// `pytest.exit`, a `KeyboardInterrupt`, an internal error): a
    /// `batch-stopped` error diagnostic.
    pub(super) stopped: bool,
    /// The batch reported an error diagnostic.
    errors: bool,
}

impl Track {
    pub(super) fn see(&mut self, event: &Event) {
        match event {
            Event::TestStarted { id } => {
                self.cover(id);
                if self.started.insert(id.clone()) {
                    self.ran.push(id.clone());
                }
                self.in_flight = Some((id.clone(), Instant::now()));
            }
            Event::TestFinished(result) => {
                self.cover(&result.id);
                self.bad += u32::from(result.outcome.is_bad());
                if self
                    .in_flight
                    .as_ref()
                    .is_some_and(|(id, _)| *id == result.id)
                {
                    self.in_flight = None;
                }
            }
            Event::Diagnostic(diagnostic) => {
                let error = diagnostic.severity == Severity::Error;
                self.stopped |= error && diagnostic.code == DiagnosticCode::BatchStopped;
                self.errors |= error;
            }
            _ => {}
        }
    }

    fn cover(&mut self, id: &TestId) {
        self.covered.insert(id.as_str().to_string());
        for ancestor in id.ancestors() {
            self.covered.insert(ancestor.to_string());
        }
    }

    /// The requested ids none of whose tests started.
    pub(super) fn remaining(&self, ids: &[TestId]) -> Vec<TestId> {
        ids.iter()
            .filter(|id| !self.covered.contains(id.as_str()))
            .cloned()
            .collect()
    }

    /// The requested ids that never started under their own id: those that
    /// never started, and those that stand for several tests, some of which
    /// may not have run.
    fn unfinished(&self, ids: &[TestId]) -> Vec<TestId> {
        ids.iter()
            .filter(|id| !self.started.contains(*id))
            .cloned()
            .collect()
    }

    /// No test of the batch had a bad outcome, and it reported no error.
    pub(super) fn clean(&self) -> bool {
        self.bad == 0 && !self.errors
    }
}

#[cfg(test)]
mod tests {
    use aye_events::{Diagnostic, DiagnosticPath};

    use super::*;

    #[test]
    fn track_knows_what_started() {
        let mut track = Track::default();
        track.see(&Event::TestStarted {
            id: TestId::new("a.py::C::t[1]"),
        });
        track.see(&Event::TestFinished(Box::new(TestResult::new(
            TestId::new("a.py::C::t[1]"),
            Outcome::Passed,
        ))));
        track.see(&Event::TestStarted {
            id: TestId::new("b.py::u"),
        });
        assert_eq!(track.in_flight.as_ref().unwrap().0.as_str(), "b.py::u");
        let ids: Vec<TestId> = [
            "a.py::C::t",
            "a.py::C",
            "a.py",
            "b.py::u",
            "b.py::v",
            "a.py::C::tt",
        ]
        .into_iter()
        .map(TestId::new)
        .collect();
        let remaining: Vec<String> = track
            .remaining(&ids)
            .into_iter()
            .map(TestId::into_string)
            .collect();
        assert_eq!(remaining, ["b.py::v", "a.py::C::tt"]);
    }

    #[test]
    fn a_batch_is_clean_without_a_bad_outcome_or_an_error() {
        let mut track = Track::default();
        assert!(track.clean());
        track.see(&Event::Diagnostic(Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::SessionFailed,
            path: DiagnosticPath::Nowhere,
            span: None,
            message: String::new(),
        }));
        assert!(track.clean());
        let mut failed = Track::default();
        failed.see(&Event::TestFinished(Box::new(TestResult::new(
            TestId::new("a.py::t"),
            Outcome::Failed,
        ))));
        assert!(!failed.clean());
        track.see(&Event::Diagnostic(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::CollectionError,
            path: "b.py".into(),
            span: None,
            message: String::new(),
        }));
        assert!(!track.clean());
    }

    #[test]
    fn a_retry_takes_every_id_that_did_not_start_as_itself() {
        let mut track = Track::default();
        for id in ["a.py::t[0]", "a.py::t[1]", "b.py::u[new]", "a.py::t[1]"] {
            track.see(&Event::TestStarted {
                id: TestId::new(id),
            });
        }
        let ids: Vec<TestId> = ["a.py::t", "a.py::t[1]", "b.py::u[old]", "b.py::v"]
            .into_iter()
            .map(TestId::new)
            .collect();
        let names = |ids: Vec<TestId>| ids.into_iter().map(TestId::into_string).collect::<Vec<_>>();
        // What none of whose tests started, for `not_run`.
        assert_eq!(names(track.remaining(&ids)), ["b.py::u[old]", "b.py::v"]);
        // What may not have run in full, for the retry.
        assert_eq!(
            names(track.unfinished(&ids)),
            ["a.py::t", "b.py::u[old]", "b.py::v"]
        );
        assert_eq!(
            names(track.ran.clone()),
            ["a.py::t[0]", "a.py::t[1]", "b.py::u[new]"]
        );
    }

    #[test]
    fn only_a_batch_stopped_error_says_the_framework_stopped_the_batch() {
        let diagnostic = |severity, code: &str| {
            Event::Diagnostic(Diagnostic {
                severity,
                code: code.into(),
                path: DiagnosticPath::Nowhere,
                span: None,
                message: "pytest stopped the session".into(),
            })
        };
        let mut track = Track::default();
        // `--collect-only` and the like, which pytest calls a success.
        track.see(&diagnostic(Severity::Warning, "batch-stopped"));
        track.see(&diagnostic(Severity::Error, "internal-error"));
        track.see(&diagnostic(Severity::Error, "usage-error"));
        assert!(!track.stopped);
        track.see(&diagnostic(Severity::Error, "batch-stopped"));
        assert!(track.stopped);
    }
}
