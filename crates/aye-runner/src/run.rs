//! Running a plan: workers pulling batches, the run's log, `maxfail` across
//! workers, crash recovery, stopping, and the session listing of executors
//! that [list the session](aye_protocol::SessionMode::ListsSession).
//!
//! [`Runner::run`] starts the run's lanes (`lane.rs`), each of which runs
//! batches on its worker (`batch.rs`) and recovers from its crashes
//! (`crash.rs`), and tells the log thread (`message.rs`), which stamps and
//! delivers the records, keeps the summary and decides when the run stops
//! (`log.rs`); `maxfail.rs` counts `maxfail` across the lanes and stops
//! their busy workers. The workers refinement keeps or starts for a run
//! are the crate's `listing.rs`; refinement itself, its `refine.rs`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use aye_events::{
    AdapterName, Diagnostic, DiagnosticCode, DiagnosticPath, Event, ProjectPath, Record, Severity,
    Stamper, StopReason, WorkerIndex,
};
use aye_protocol::Runtime;

use crate::env::LaneEnv;
use crate::listing::{Listings, Prelisting, Warm, shut_down};
use crate::options::{Plan, RunnerOptions, Runtimes};
use crate::schedule::{self, Queue};
use crate::stop::StopHandle;
use crate::summary::RunSummary;
use crate::sync::lock;
use crate::worker::WorkerSpec;
use crate::worker::timeouts::POLL;

mod batch;
mod crash;
mod lane;
mod log;
mod maxfail;
mod message;

use log::{Log, RunPhase};
use maxfail::{Busy, MaxfailBudget};

/// Runs plans on workers, and refines files on them.
#[derive(Debug)]
pub struct Runner {
    pub(crate) options: RunnerOptions,
    /// The options' runtimes, by adapter.
    pub(crate) runtimes: Runtimes,
    pub(crate) stop: StopHandle,
    /// The worker that refined for an adapter whose executor lists the
    /// session, kept for the next run's first lane.
    pub(crate) warm: Mutex<Option<Warm>>,
    /// With several workers, those started for the other lanes while the
    /// first lane's worker refined, which listed the same files meanwhile.
    pub(crate) prelisting: Mutex<Vec<Prelisting>>,
}

impl Drop for Runner {
    /// Workers kept or started from refinement that no run took (`--co`)
    /// shut down, at once (killed, once the runner is stopped).
    fn drop(&mut self) {
        let kill = self.stop.is_stopped();
        let warm = self
            .warm
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let prelisting = std::mem::take(
            self.prelisting
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner),
        );
        thread::scope(|scope| {
            if let Some(warm) = warm {
                scope.spawn(move || {
                    if kill {
                        warm.worker.kill();
                    } else {
                        warm.worker.shutdown();
                    }
                });
            }
            scope.spawn(move || shut_down(prelisting, kill));
        });
    }
}

impl Runner {
    pub fn new(options: RunnerOptions) -> Self {
        Self {
            runtimes: Runtimes::new(&options.runtimes),
            options,
            stop: StopHandle::default(),
            warm: Mutex::new(None),
            prelisting: Mutex::new(Vec::new()),
        }
    }

    pub fn options(&self) -> &RunnerOptions {
        &self.options
    }

    pub fn stop_handle(&self) -> StopHandle {
        self.stop.clone()
    }

    /// Run `plan`, delivering records to `on_record` in the order they
    /// happen, stamped by `stamper`.
    ///
    /// Every started worker gets `worker_started` and `worker_finished` (a
    /// worker that could not start gets only the latter, with its error),
    /// and every batch `batch_started` and `batch_finished`. Between them,
    /// the worker's records arrive as it sends them, with `worker` set and
    /// their `batch` kept. Every test handed to a worker gets a
    /// `test_finished`, including those a crash interrupted, except when the
    /// adapter fails outside any test (a `batch-failed` diagnostic), when the
    /// framework stops a batch (its tests that never started are in
    /// [`RunSummary::not_run`]), and once the run stops: when `maxfail` is
    /// reached or the stop handle is used, workers' records are dropped and
    /// only the closing records follow, with, after `maxfail`, the
    /// `lifetime_finished` records that close lifetimes the log holds open
    /// as batches tear down.
    pub fn run(
        &self,
        plan: &Plan,
        stamper: &mut Stamper,
        on_record: &mut dyn FnMut(Record),
    ) -> RunSummary {
        let mut log = Log {
            stamper,
            on_record,
            summary: RunSummary {
                requested: u32::try_from(plan.cases.len()).unwrap_or(u32::MAX),
                ..RunSummary::default()
            },
            running: HashMap::new(),
        };
        if self.stop.is_stopped() {
            log.summary.interrupted = true;
            log.summary.stopped = Some(StopReason::Interrupted);
            log.summary.not_run = plan.cases.iter().map(|case| case.id.clone()).collect();
            return log.summary;
        }

        let mut cases = Vec::with_capacity(plan.cases.len());
        let mut orphans: Vec<(&AdapterName, &str, u32)> = Vec::new();
        for case in &plan.cases {
            if self.runtimes.get(&case.adapter).is_some() {
                cases.push(case.clone());
                continue;
            }
            log.summary.not_run.push(case.id.clone());
            if let Some(orphan) = orphans.iter_mut().find(|o| *o.0 == case.adapter) {
                orphan.2 += 1;
            } else {
                orphans.push((&case.adapter, case.id.path().as_str(), 1));
            }
        }
        for (adapter, path, count) in orphans {
            log.summary.internal_error = true;
            log.deliver(
                None,
                None,
                Event::Diagnostic(Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::NoExecutor,
                    path: DiagnosticPath::as_written(path),
                    span: None,
                    message: format!(
                        "no executor for the {adapter} adapter; {count} test{} not run",
                        if count == 1 { "" } else { "s" }
                    ),
                }),
            );
        }

        // Batches of executors that list the session keep plan order on
        // each lane, as each pytest-xdist worker runs its tests in order.
        let ordered: HashSet<&AdapterName> = self.runtimes.listing_session().collect();
        let batches = schedule::plan(
            &cases,
            &plan.durations,
            &plan.first,
            self.options.workers,
            &ordered,
        );
        if batches.is_empty() {
            return log.summary;
        }
        let lanes = self
            .options
            .workers
            .max(1)
            .min(u32::try_from(batches.len()).unwrap_or(u32::MAX));
        let adapters: HashSet<&AdapterName> = batches.iter().map(|b| &b.spec.adapter).collect();
        let mut spec = self.spec(Vec::new());
        spec.echo = (!plan.policy.capture).then_some(self.options.echo);

        // The worker kept from refinement is the first lane's, if the run
        // needs its adapter and gives that lane the environment it has.
        let warm = match lock(&self.warm).take() {
            Some(warm) if adapters.contains(&warm.adapter) && warm.env == LaneEnv::first(lanes) => {
                warm.worker.set_echo(spec.echo);
                Some(warm)
            }
            Some(unused) => {
                unused.worker.shutdown();
                None
            }
            None => None,
        };
        // With several lanes, the first lane's listing of one adapter's
        // session is what the others compare theirs with.
        let reference = (lanes > 1)
            .then(|| match &warm {
                Some(warm) => Some(warm.adapter.clone()),
                None => batches
                    .iter()
                    .map(|batch| &batch.spec.adapter)
                    .find(|adapter| {
                        self.runtimes.lists_session(adapter)
                            && plan
                                .session_files
                                .get(*adapter)
                                .is_some_and(|f| !f.is_empty())
                    })
                    .cloned(),
            })
            .flatten();
        let early = reference.as_ref().map_or(0, |reference| {
            let of = batches
                .iter()
                .filter(|b| b.spec.adapter == *reference)
                .count();
            lanes.min(u32::try_from(of).unwrap_or(u32::MAX))
        });
        // The workers started for the other lanes during refinement, if the
        // run needs their adapter and gives their lanes the environment
        // they have.
        let (prelisted, unused): (Vec<Prelisting>, Vec<Prelisting>) =
            std::mem::take(&mut *lock(&self.prelisting))
                .into_iter()
                .partition(|p| adapters.contains(&p.adapter) && p.env.lanes == lanes);
        let queue = Queue::new(batches);
        if reference.is_some() {
            queue.expect_pins(lanes);
        }

        let shared = Shared {
            queue,
            busy: Mutex::new(HashMap::new()),
            maxfail: MaxfailBudget::new(plan.policy.maxfail),
            phase: Mutex::new(RunPhase::Running),
            spec,
            runtimes: &self.runtimes,
            plan,
            sessions: &plan.session_files,
            listings: Listings::new(reference.as_ref()),
            reference,
            early,
            warm: Mutex::new(warm),
            prelisted: Mutex::new(prelisted.into_iter().map(|p| (p.env.lane, p)).collect()),
            lanes,
            stop: &self.stop,
        };
        let (tx, rx) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(move || shut_down(unused, false));
            for lane in (0..lanes).map(WorkerIndex::from_u32) {
                let tx = tx.clone();
                let shared = &shared;
                scope.spawn(move || shared.lane(lane, &tx));
            }
            drop(tx);
            loop {
                let phase = shared.phase();
                if self.stop.is_stopped() && phase != RunPhase::Killed {
                    // Also after `maxfail`: Ctrl-C ends a teardown it waits for.
                    if !phase.stops_workers() {
                        log.summary.interrupted = true;
                        log.stop(StopReason::Interrupted);
                    }
                    shared.halt();
                }
                if let RunPhase::WindingDown { kill_at: Some(at) } = shared.phase()
                    && Instant::now() >= at
                {
                    shared.kill_interrupted();
                }
                match rx.recv_timeout(POLL) {
                    Ok(message) => log.handle(message, &shared),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        shut_down(
            std::mem::take(&mut *lock(&shared.prelisted))
                .into_values()
                .collect(),
            self.stop.is_stopped(),
        );
        log.summary.not_run.extend(shared.queue.drain());
        log.summary.batches.sort_by_key(|b| b.id);
        log.summary
    }

    pub(crate) fn spec(&self, runtimes: Vec<Runtime>) -> WorkerSpec {
        let mut spec = WorkerSpec::new(&self.options.python, &self.options.root, &self.options.cwd);
        spec.prelude = self.options.prelude;
        spec.runtimes = runtimes;
        spec.env.clone_from(&self.options.env);
        spec.registry = Some(self.stop.workers.clone());
        spec
    }
}

/// What the lanes of one run share.
struct Shared<'a> {
    queue: Queue,
    /// Workers in a handshake or a batch, to stop when the run stops.
    busy: Mutex<HashMap<WorkerIndex, Busy>>,
    /// `maxfail` across the lanes: the bad outcomes they have passed on.
    maxfail: MaxfailBudget,
    /// Where the run is: the log thread moves it on as it stops the run,
    /// and a lane whose batch the framework stopped; the lanes read it
    /// before they start anything, and the log for every record.
    phase: Mutex<RunPhase>,
    /// How the run's workers start, without a runtime: each gets its
    /// adapter's ([`Shared::start`]).
    spec: WorkerSpec,
    runtimes: &'a Runtimes,
    plan: &'a Plan,
    /// The plan's session files, by adapter.
    sessions: &'a BTreeMap<AdapterName, Vec<ProjectPath>>,
    listings: Listings,
    /// The adapter whose session the first lane lists first, for the others
    /// to compare theirs with (several lanes only).
    reference: Option<AdapterName>,
    /// How many lanes start with a worker of that adapter, before they take
    /// a batch: one per batch of it, at most every lane.
    early: u32,
    /// The worker kept from refinement, until the first lane takes it.
    warm: Mutex<Option<Warm>>,
    /// By lane, the worker started for it during refinement, until the
    /// lane takes it ([`Shared::start`]) or ends.
    prelisted: Mutex<HashMap<WorkerIndex, Prelisting>>,
    lanes: u32,
    stop: &'a StopHandle,
}

impl Shared<'_> {
    fn phase(&self) -> RunPhase {
        *lock(&self.phase)
    }

    /// Whether `adapter`'s executor lists the session.
    fn lists_session(&self, adapter: &AdapterName) -> bool {
        self.runtimes.lists_session(adapter)
    }
}

#[cfg(test)]
mod tests {
    use aye_events::{Case, TestId};

    use super::*;

    #[test]
    fn a_stopped_runner_runs_nothing() {
        let runner = Runner::new(RunnerOptions {
            python: "/nonexistent/python".into(),
            root: "/".into(),
            cwd: "/".into(),
            workers: 2,
            ..RunnerOptions::default()
        });
        runner.stop_handle().stop();
        let plan = Plan {
            cases: vec![Case::new("t.py::test_a", "fake")],
            ..Plan::default()
        };
        let mut records = Vec::new();
        let summary = runner.run(&plan, &mut Stamper::new(), &mut |r| records.push(r));
        assert!(summary.interrupted);
        assert_eq!(summary.stopped, Some(StopReason::Interrupted));
        assert_eq!(summary.not_run, [TestId::new("t.py::test_a")]);
        assert_eq!(records.len(), 0, "{records:?}");
    }

    #[test]
    fn cases_without_an_executor_are_reported_once_per_adapter() {
        let runner = Runner::new(RunnerOptions {
            python: "/nonexistent/python".into(),
            root: "/".into(),
            cwd: "/".into(),
            workers: 1,
            ..RunnerOptions::default()
        });
        let plan = Plan {
            cases: vec![
                Case::new("t.py::test_a", "fake"),
                Case::new("t.py::test_b", "fake"),
            ],
            ..Plan::default()
        };
        let mut records = Vec::new();
        let summary = runner.run(&plan, &mut Stamper::new(), &mut |r| records.push(r));
        assert!(summary.internal_error);
        assert_eq!(summary.requested, 2);
        assert_eq!(summary.not_run.len(), 2);
        assert_eq!(records.len(), 1);
        let Event::Diagnostic(diagnostic) = &records[0].event else {
            panic!("expected a diagnostic");
        };
        assert_eq!(diagnostic.code, DiagnosticCode::NoExecutor);
        assert_eq!(diagnostic.path, "t.py");
        assert!(
            diagnostic.message.contains("2 tests not run"),
            "{}",
            diagnostic.message
        );
    }
}
