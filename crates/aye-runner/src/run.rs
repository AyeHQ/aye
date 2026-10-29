//! Running a plan: workers pulling batches, the run's log, `maxfail` across
//! workers, crash recovery, stopping, and the session listing of executors
//! that [list the session](aye_engine::Executor::lists_session).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use aye_engine::{
    Batch, Executor, Policy, RefineError, RefineRequest, Refined, Refiner, Reply, Request,
    WorkerLine,
};
use aye_events::{
    AdapterName, BatchIndex, Case, Counts, Diagnostic, DiagnosticCode, DiagnosticPath, Event,
    Failure, LifetimeId, Outcome, ProjectPath, Record, Severity, Stamper, TestId, TestResult,
    WorkerIndex,
};
use aye_static::EnvVars;

use crate::schedule::{self, Assigned, Queue};
use crate::worker::{Echo, Incoming, Killer, Registry, Worker, WorkerSpec, is_reply, lock, spawn};

/// How often a run checks its [`StopHandle`] while nothing else happens.
const POLL: Duration = Duration::from_millis(50);
/// How long a worker interrupted when `maxfail` ran out may take to end its
/// batch, tearing down, before it is killed: pytest-xdist gives its workers
/// as long to stop.
const INTERRUPT_GRACE: Duration = Duration::from_secs(10);

/// pytest's status for a session whose tests failed.
const STATUS_TESTS_FAILED: i32 = 1;
/// pytest's status for an interrupted session.
const STATUS_INTERRUPTED: i32 = 2;
/// pytest's status for an internal error (`INTERNALERROR`).
const STATUS_INTERNAL_ERROR: i32 = 3;
/// The context key under which a batch re-queued after a crash names the
/// tests that ran before it (see [`Shared::crashed`]).
const RAN_IDS: &str = "ran_ids";

/// By file, a digest of its tests' ids in order ([`Refined::digests`]).
type Digests = BTreeMap<ProjectPath, String>;

/// What a [`Runner`] needs to start workers.
#[derive(Debug, Clone, Default)]
pub struct RunnerOptions {
    /// The interpreter workers run.
    pub python: PathBuf,
    /// The project root, absolute.
    pub root: PathBuf,
    /// The directory workers start in: where aye was started.
    pub cwd: PathBuf,
    /// Workers to run at most (0 counts as 1); never more than there are
    /// batches.
    pub workers: u32,
    /// Every adapter's runtime face. Workers get only those the cases at
    /// hand need.
    pub executors: Vec<Executor>,
    /// Environment variables for every worker.
    pub env: Vec<(OsString, OsString)>,
    /// Where workers' output is echoed when the plan doesn't capture it
    /// (`-s`): stdout beside a text report, stderr beside a machine-readable
    /// one.
    pub echo: Echo,
    /// No run follows refinement (`--co`): no worker starts for the other
    /// lanes while one refines for an executor that lists the session.
    pub collect_only: bool,
}

/// What to run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// In the order they should run within a batch.
    pub cases: Vec<Case>,
    /// For every batch. `capture: false` also echoes workers' output live,
    /// where [`RunnerOptions::echo`] says; `maxfail` 0 is no limit.
    pub policy: Policy,
    /// Sent with each batch, per adapter (the recognizer's `context()`).
    pub contexts: HashMap<AdapterName, serde_json::Value>,
    /// Historical durations, for balancing.
    pub durations: HashMap<TestId, Duration>,
    /// Tests to run first (`--ff`: last run's failures): the batches holding
    /// them are handed out first, and they run first within their batch.
    pub first: HashSet<TestId>,
    /// By adapter, every file collection claimed for its tests (selected or
    /// not), in collection order: what each worker of an adapter whose
    /// executor lists the session lists before its first batch.
    pub session_files: BTreeMap<AdapterName, Vec<ProjectPath>>,
}

impl Default for Plan {
    /// Nothing to run, capturing output.
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            policy: Policy {
                capture: true,
                ..Policy::default()
            },
            contexts: HashMap::new(),
            durations: HashMap::new(),
            first: HashSet::new(),
            session_files: BTreeMap::new(),
        }
    }
}

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
    /// The run was interrupted: by the [`StopHandle`], or by the framework
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

/// Why a run stopped early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `maxfail` bad outcomes were reached.
    Maxfail,
    /// The [`StopHandle`] stopped the run (Ctrl-C).
    Interrupted,
    /// The framework stopped a batch, ending its session there: the
    /// adapter said so with a `batch-stopped` error diagnostic (`pytest.exit`
    /// whatever its `returncode`, a `KeyboardInterrupt`, `shouldstop`, a
    /// `shouldfail` that isn't aye's `maxfail`, an internal error that left
    /// tests), or the batch ended with pytest's status 2. The run
    /// counts as interrupted, except after an internal error (status 3),
    /// which makes it an internal error instead.
    Framework,
}

impl StopReason {
    /// `maxfail`, `interrupted` or `framework`, as `run_finished.stopped`
    /// spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Maxfail => "maxfail",
            Self::Interrupted => "interrupted",
            Self::Framework => "framework",
        }
    }
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchSummary {
    pub id: BatchIndex,
    pub adapter: AdapterName,
    pub worker: WorkerIndex,
    pub cases: u32,
    /// The framework's own status, from `batch_done`.
    pub status: Option<i32>,
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

/// Stops a run from another thread (the binary's signal handler). Sticky: a
/// runner whose handle was used starts no more runs or workers.
#[derive(Debug, Clone, Default)]
pub struct StopHandle {
    stopped: Arc<AtomicBool>,
    /// Every live worker of the runner: its runs' lanes, handshakes and
    /// refinements.
    workers: Registry,
}

impl StopHandle {
    /// Stop the run: no more batches are handed out, running workers are
    /// killed with their process groups, and the run returns with what it
    /// has (within 50ms).
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
    }

    /// Stop, and kill every live worker of the runner with its whole
    /// process group (the processes its tests started) right now, from this
    /// thread: for a caller about to exit (a second Ctrl-C). Workers that
    /// start later are killed at once.
    pub fn kill(&self) {
        self.stop();
        self.workers.kill_all();
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }
}

/// Runs plans on workers, and refines files on them.
#[derive(Debug)]
pub struct Runner {
    options: RunnerOptions,
    stop: StopHandle,
    /// The worker that refined for an adapter whose executor lists the
    /// session, kept for the next run's first lane.
    warm: Mutex<Option<Warm>>,
    /// With several workers, those started for the other lanes while the
    /// first lane's worker refined, which listed the same files meanwhile.
    prelisting: Mutex<Vec<Prelisting>>,
}

/// A worker kept from refinement, with what it listed.
#[derive(Debug)]
struct Warm {
    adapter: AdapterName,
    worker: Worker,
    /// The lanes its environment was made for (see [`lane_env`]).
    lanes: u32,
    digests: Digests,
}

/// A worker started for a lane other than the first while the first lane's
/// worker refines: it lists the files refined (digests only) meanwhile, so
/// that the run's lanes start ready rather than listing then.
#[derive(Debug)]
struct Prelisting {
    adapter: AdapterName,
    lane: WorkerIndex,
    /// The lanes its environment was made for (see [`lane_env`]).
    lanes: u32,
    /// Starts the worker and sends it its listings, in the background.
    worker: thread::JoinHandle<Result<Worker, String>>,
    /// Its listings sent and not answered yet.
    pending: usize,
}

/// The worker a [`Prelisting`]'s thread started, or why it couldn't.
fn started(worker: thread::JoinHandle<Result<Worker, String>>) -> Result<Worker, String> {
    worker
        .join()
        .unwrap_or_else(|_| Err("starting the worker panicked".into()))
}

/// Shut the workers down, or kill them once the runner is stopped, at
/// once.
fn shut_down(prelisting: Vec<Prelisting>, kill: bool) {
    thread::scope(|scope| {
        for prelisted in prelisting {
            scope.spawn(move || match started(prelisted.worker) {
                Ok(worker) if kill => worker.kill(),
                Ok(worker) => {
                    worker.shutdown();
                }
                Err(_) => {}
            });
        }
    });
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
    /// as batches tear down (see [`Shared::wind_down`]).
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
            halted: false,
            killed: false,
            kill_at: None,
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
            if self.executor(&case.adapter).is_some() {
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

        // `maxfail` 0 is no limit, as in pytest.
        let mut policy = plan.policy.clone();
        policy.maxfail = policy.maxfail.filter(|&max| max > 0);

        // Batches of executors that list the session keep plan order on
        // each lane, as each pytest-xdist worker runs its tests in order.
        let ordered: HashSet<&AdapterName> = self
            .options
            .executors
            .iter()
            .filter(|e| e.lists_session)
            .map(|e| &e.adapter)
            .collect();
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
        let adapters: HashSet<&AdapterName> = batches.iter().map(|b| &b.adapter).collect();
        let executors = self
            .options
            .executors
            .iter()
            .filter(|e| adapters.contains(&e.adapter))
            .cloned()
            .collect();
        let mut spec = self.spec(executors);
        spec.echo = (!policy.capture).then_some(self.options.echo);

        // The worker kept from refinement is the first lane's, if the run
        // needs its adapter and gives that lane the environment it has.
        let warm = match lock(&self.warm).take() {
            Some(warm) if adapters.contains(&warm.adapter) && warm.lanes == lanes => {
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
                    .map(|batch| &batch.adapter)
                    .find(|adapter| {
                        self.executor(adapter).is_some_and(|e| e.lists_session)
                            && plan
                                .session_files
                                .get(*adapter)
                                .is_some_and(|f| !f.is_empty())
                    })
                    .cloned(),
            })
            .flatten();
        let early = reference.as_ref().map_or(0, |reference| {
            let of = batches.iter().filter(|b| b.adapter == *reference).count();
            lanes.min(u32::try_from(of).unwrap_or(u32::MAX))
        });
        // The workers started for the other lanes during refinement, if the
        // run needs their adapter and gives their lanes the environment
        // they have.
        let (prelisted, unused): (Vec<Prelisting>, Vec<Prelisting>) =
            std::mem::take(&mut *lock(&self.prelisting))
                .into_iter()
                .partition(|p| adapters.contains(&p.adapter) && p.lanes == lanes);
        let queue = Queue::new(batches);
        if reference.is_some() {
            queue.expect_pins(lanes);
        }

        let shared = Shared {
            queue,
            busy: Mutex::new(HashMap::new()),
            bad: AtomicU32::new(0),
            killing: AtomicBool::new(false),
            spec,
            policy: &policy,
            contexts: &plan.contexts,
            sessions: &plan.session_files,
            listings: Listings::new(reference.as_ref()),
            reference,
            early,
            warm: Mutex::new(warm),
            prelisted: Mutex::new(prelisted.into_iter().map(|p| (p.lane, p)).collect()),
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
                if self.stop.is_stopped() && !log.killed {
                    // Also after `maxfail`: Ctrl-C ends a teardown it waits for.
                    if !log.halted {
                        log.summary.interrupted = true;
                        log.stop(StopReason::Interrupted);
                    }
                    log.kill(&shared);
                }
                if !log.killed && log.kill_at.is_some_and(|at| Instant::now() >= at) {
                    log.kill_at = None;
                    shared.kill_interrupted();
                }
                match rx.recv_timeout(POLL) {
                    Ok(message) => log.handle(message, &shared, policy.maxfail),
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

    fn executor(&self, adapter: &AdapterName) -> Option<Executor> {
        self.options
            .executors
            .iter()
            .find(|e| e.adapter == *adapter)
            .cloned()
    }

    fn spec(&self, executors: Vec<Executor>) -> WorkerSpec {
        let mut spec = WorkerSpec::new(&self.options.python, &self.options.root, &self.options.cwd);
        spec.executors = executors;
        spec.env.clone_from(&self.options.env);
        spec.registry = Some(self.stop.workers.clone());
        spec
    }

    /// Have a worker for each lane but the first among `lanes` list
    /// `request`'s files (digests only) while the first lane's worker
    /// refines them: started in the background, with its lane's
    /// environment, or the one started for an earlier refinement of the
    /// adapter. Those started for another adapter or other lanes shut down.
    fn prelist(&self, executor: &Executor, request: &RefineRequest, lanes: u32) {
        let mut prelisting = lock(&self.prelisting);
        let (mut kept, other): (Vec<Prelisting>, Vec<Prelisting>) =
            std::mem::take(&mut *prelisting)
                .into_iter()
                .partition(|p| p.adapter == request.adapter && p.lanes == lanes);
        shut_down(other, false);
        let listing = RefineRequest {
            digests_only: true,
            ..request.clone()
        };
        for lane in (1..lanes).map(WorkerIndex::from_u32) {
            let listing = listing.clone();
            // A worker that died shows up as the end of its stream when its
            // answer is read.
            let (worker, pending) = if let Some(at) = kept.iter().position(|p| p.lane == lane) {
                // An earlier refinement's: it lists these files too.
                let earlier = kept.swap_remove(at);
                let worker = earlier.worker;
                let listed = thread::spawn(move || {
                    let mut worker = started(worker)?;
                    let _ = worker.send(&Request::Refine(listing));
                    Ok(worker)
                });
                (listed, earlier.pending + 1)
            } else {
                let mut spec = self.spec(vec![executor.clone()]);
                spec.env.extend(lane_env(lane, lanes));
                let listed = thread::spawn(move || {
                    let mut worker = spawn(&spec).map_err(|e| e.to_string())?;
                    let _ = worker.send(&Request::Refine(listing));
                    Ok(worker)
                });
                (listed, 1)
            };
            kept.push(Prelisting {
                adapter: request.adapter.clone(),
                lane,
                lanes,
                worker,
                pending,
            });
        }
        *prelisting = kept;
    }
}

impl Refiner for Runner {
    /// Start a fresh worker with the adapter's executor, ask it to refine,
    /// and shut it down. For an executor that lists the session, the worker
    /// is kept instead, with the environment of a run's first lane, for the
    /// next refinement of that adapter and then the next run's first lane
    /// (or shut down when the runner is dropped); with several workers,
    /// one started for each other lane lists the same files meanwhile
    /// (unless [`RunnerOptions::collect_only`]).
    fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
        let executor = self.executor(&request.adapter).ok_or_else(|| {
            RefineError(format!("no executor for the {} adapter", request.adapter))
        })?;
        if self.stop.is_stopped() {
            return Err(RefineError("interrupted".into()));
        }
        let stopped = || self.stop.is_stopped();
        if !executor.lists_session {
            let mut worker =
                spawn(&self.spec(vec![executor])).map_err(|e| RefineError(e.to_string()))?;
            return match refine_on(&mut worker, request.clone(), &stopped) {
                Ok(refined) => {
                    worker.shutdown();
                    Ok(refined)
                }
                Err(unrefined) => Err(RefineError(unrefined.end(worker, "refining"))),
            };
        }
        let lanes = self.options.workers.max(1);
        if !self.options.collect_only {
            self.prelist(&executor, request, lanes);
        }
        let mut warm = lock(&self.warm);
        let mut kept = match warm.take() {
            Some(kept) if kept.adapter == request.adapter => kept,
            other => {
                if let Some(other) = other {
                    other.worker.shutdown();
                }
                let mut spec = self.spec(vec![executor]);
                spec.env.extend(lane_env(WorkerIndex::FIRST, lanes));
                Warm {
                    adapter: request.adapter.clone(),
                    worker: spawn(&spec).map_err(|e| RefineError(e.to_string()))?,
                    lanes,
                    digests: Digests::new(),
                }
            }
        };
        match refine_on(&mut kept.worker, request.clone(), &stopped) {
            Ok(refined) => {
                kept.digests
                    .extend(refined.digests.clone().unwrap_or_default());
                *warm = Some(kept);
                Ok(refined)
            }
            Err(unrefined) => Err(RefineError(unrefined.end(kept.worker, "refining"))),
        }
    }
}

/// A refinement that brought no answer.
enum Unrefined {
    /// The executor raised: the worker goes on.
    Failed(String),
    /// The worker sent what isn't a protocol message, and was killed.
    Garbage(String),
    /// The worker's stream ended: it exited.
    Eof,
    /// The run or the runner was stopped, and the worker killed.
    Interrupted,
}

impl Unrefined {
    /// Why, once the worker is done with: shut down, or waited for.
    fn end(self, worker: Worker, doing: &str) -> String {
        match self {
            Self::Failed(message) => {
                worker.shutdown();
                message
            }
            Self::Garbage(message) => message,
            Self::Interrupted => "interrupted".into(),
            Self::Eof => {
                let exit = worker.wait();
                let mut message = format!("the worker exited while {doing} ({})", exit.describe());
                if !exit.stderr.is_empty() {
                    message.push_str(":\n");
                    message.push_str(&exit.stderr);
                }
                message
            }
        }
    }
}

/// Send `request` to `worker` and wait for its answer, killing the worker
/// if `stopped` says so meanwhile, or if it breaks the protocol.
fn refine_on(
    worker: &mut Worker,
    request: RefineRequest,
    stopped: &dyn Fn() -> bool,
) -> Result<Refined, Unrefined> {
    // A worker that died shows up below as an end of stream.
    let _ = worker.send(&Request::Refine(request));
    refined_by(worker, stopped)
}

/// The answer to the refinement `worker` was sent, as [`refine_on`] waits
/// for it.
fn refined_by(worker: &Worker, stopped: &dyn Fn() -> bool) -> Result<Refined, Unrefined> {
    loop {
        if stopped() {
            worker.kill();
            return Err(Unrefined::Interrupted);
        }
        match worker.recv_timeout(POLL) {
            Some(Incoming::Line(WorkerLine::Reply(Reply::Refined(refined)))) => return Ok(refined),
            Some(Incoming::Line(WorkerLine::Reply(Reply::Failed { message }))) => {
                return Err(Unrefined::Failed(message));
            }
            Some(Incoming::Garbage { line, error }) => {
                worker.kill();
                return Err(Unrefined::Garbage(format!(
                    "the worker sent a line that is not a protocol message ({error}): {}",
                    shorten(&line)
                )));
            }
            Some(Incoming::Eof) => return Err(Unrefined::Eof),
            Some(_) | None => {}
        }
    }
}

/// The environment variables of lane `lane`'s workers among `lanes`:
/// `AYE_WORKER`, and with several lanes pytest-xdist's, so that per-worker
/// resources are reused.
fn lane_env(lane: WorkerIndex, lanes: u32) -> Vec<(OsString, OsString)> {
    let mut env = vec![(EnvVars::AYE_WORKER.into(), lane.to_string().into())];
    if lanes > 1 {
        env.push((
            EnvVars::PYTEST_XDIST_WORKER.into(),
            lane.xdist_name().into(),
        ));
        env.push((
            EnvVars::PYTEST_XDIST_WORKER_COUNT.into(),
            lanes.to_string().into(),
        ));
    }
    env
}

/// The lanes' listings of the session: the first lane's, by adapter, which
/// the others compare theirs with, and the files already warned about.
struct Listings {
    reference: Mutex<HashMap<AdapterName, Reference>>,
    published: Condvar,
    warned: Mutex<HashSet<ProjectPath>>,
}

enum Reference {
    /// The first lane is still listing.
    Pending,
    Ready(Digests),
    /// The first lane couldn't list, or its executor gave no digests:
    /// nothing is compared.
    Missing,
}

impl Listings {
    fn new(reference: Option<&AdapterName>) -> Self {
        Self {
            reference: Mutex::new(
                reference
                    .map(|adapter| (adapter.clone(), Reference::Pending))
                    .into_iter()
                    .collect(),
            ),
            published: Condvar::new(),
            warned: Mutex::new(HashSet::new()),
        }
    }

    fn publish(&self, adapter: &AdapterName, digests: Option<Digests>) {
        let mut reference = lock(&self.reference);
        if let Some(slot) = reference.get_mut(adapter) {
            *slot = digests.map_or(Reference::Missing, Reference::Ready);
        }
        self.published.notify_all();
    }

    /// The first lane's listing of `adapter`'s session, once it is known;
    /// `None` without one, or once `stopping`.
    fn reference(&self, adapter: &AdapterName, stopping: &dyn Fn() -> bool) -> Option<Digests> {
        let mut reference = lock(&self.reference);
        loop {
            match reference.get(adapter)? {
                Reference::Ready(digests) => return Some(digests.clone()),
                Reference::Missing => return None,
                Reference::Pending if stopping() => return None,
                Reference::Pending => {
                    reference = self
                        .published
                        .wait_timeout(reference, POLL)
                        .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
                }
            }
        }
    }
}

/// What lanes tell the run.
#[derive(Debug)]
enum Message {
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
        status: Option<i32>,
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

/// The run's side: stamps and delivers records, keeps the summary, and
/// decides when to stop.
struct Log<'a> {
    stamper: &'a mut Stamper,
    on_record: &'a mut dyn FnMut(Record),
    summary: RunSummary,
    /// Each batch between its started and finished.
    running: HashMap<BatchIndex, Running>,
    /// The run stopped (`maxfail`, the stop handle): workers' records are
    /// dropped.
    halted: bool,
    /// The run killed every busy worker (the stop handle).
    killed: bool,
    /// When `maxfail` ran out: when the workers it interrupted are killed,
    /// if they are still busy.
    kill_at: Option<Instant>,
}

/// A batch the log has seen start.
#[derive(Debug, Default)]
struct Running {
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
    fn deliver(&mut self, worker: Option<WorkerIndex>, batch: Option<BatchIndex>, event: Event) {
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
    fn kill(&mut self, shared: &Shared<'_>) {
        self.halted = true;
        self.killed = true;
        shared.halt();
    }

    /// Record why the run stops; the first reason stays.
    fn stop(&mut self, reason: StopReason) {
        self.summary.stopped.get_or_insert(reason);
    }

    fn handle(&mut self, message: Message, shared: &Shared<'_>, maxfail: Option<u32>) {
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

/// What the lanes of one run share.
struct Shared<'a> {
    queue: Queue,
    /// Workers in a handshake or a batch, to stop when the run stops.
    busy: Mutex<HashMap<WorkerIndex, Busy>>,
    /// Bad outcomes the lanes have passed on so far. The run stops on its
    /// own count; lanes read this one to stop taking batches without
    /// waiting for it, and to size each batch's `maxfail`.
    bad: AtomicU32,
    /// The run stops its busy workers (`maxfail`, the stop handle), so a
    /// worker's death is no crash.
    killing: AtomicBool,
    spec: WorkerSpec,
    /// The plan's, with `maxfail` 0 made `None`.
    policy: &'a Policy,
    contexts: &'a HashMap<AdapterName, serde_json::Value>,
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

/// Tells the queue a lane is done taking batches, however it ends.
struct LaneDone<'q>(&'q Queue, WorkerIndex);

impl Drop for LaneDone<'_> {
    fn drop(&mut self) {
        self.0.lane_done(self.1);
    }
}

/// A busy worker, and how the run is stopping it.
struct Busy {
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

/// How a batch ended, as its lane saw it.
enum End {
    /// `batch_done`, with the framework's status.
    Done {
        status: Option<i32>,
        track: Track,
    },
    Failed(String, Track),
    Eof(Track),
}

impl Shared<'_> {
    /// Hand nothing more out and kill the busy workers (the stop handle).
    fn halt(&self) {
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
    /// none) and killed if still busy after [`INTERRUPT_GRACE`]
    /// ([`Shared::kill_interrupted`]).
    fn wind_down(&self) {
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
    fn kill_interrupted(&self) {
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
    fn busy(&self, lane: WorkerIndex, killer: Killer) -> bool {
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
    fn ends_by_itself(&self, lane: WorkerIndex) {
        if let Some(busy) = lock(&self.busy).get_mut(&lane)
            && busy.stopping == Stopping::No
        {
            busy.stopping = Stopping::ByItself;
        }
    }

    /// Whether the run is stopping: halted, or its stop handle used.
    fn stopping(&self) -> bool {
        self.queue.is_halted() || self.stop.is_stopped()
    }

    /// Mark a worker idle; whether the run interrupted it.
    fn idle(&self, lane: WorkerIndex) -> bool {
        lock(&self.busy)
            .remove(&lane)
            .is_some_and(|busy| busy.stopping == Stopping::Interrupted)
    }

    /// The next batch's share of `maxfail`: what is left of it, at least 1.
    fn budget(&self) -> Option<u32> {
        let max = self.policy.maxfail?;
        Some(max.saturating_sub(self.bad.load(Ordering::SeqCst)).max(1))
    }

    /// Whether the bad outcomes passed on already reach `maxfail`, so the
    /// run is about to stop.
    fn exhausted(&self) -> bool {
        self.policy
            .maxfail
            .is_some_and(|max| self.bad.load(Ordering::SeqCst) >= max)
    }

    /// Pass a record on, counting bad outcomes.
    fn pass_on(&self, record: Record, send: &dyn Fn(Message)) {
        if let Event::TestFinished(result) = &record.event
            && result.outcome.is_bad()
        {
            self.bad.fetch_add(1, Ordering::SeqCst);
        }
        send(Message::Record(Box::new(record)));
    }

    /// One lane: a worker slot that pulls batches until none are left,
    /// replacing its worker after a crash.
    ///
    /// A worker runs one adapter's batches only: two frameworks in one
    /// process would share imported modules (two `conftest.py` files, a
    /// unittest suite run with pytest's plugins loaded). It holds only that
    /// adapter's executor, and with several lanes, the lane prefers that
    /// adapter's batches; one of another adapter gets a new worker, after
    /// the old one has shut down, so the lanes stay the cap on live
    /// workers. With one lane, batches run in plan order whatever their
    /// adapter (the doctests pytest lists before a file's tests run first),
    /// and a worker that listed the session waits, idle, while another
    /// adapter's worker runs, rather than listing it again: it is announced
    /// (`worker_started`) when it first runs a batch, so each worker's
    /// events still come between its start and its end.
    ///
    /// A worker whose executor lists the session lists it as it starts
    /// ([`Shared::start`]). The first lane starts with the worker kept from
    /// refinement; with several lanes, lanes start with a worker of the
    /// adapter whose listings they compare before they take a batch
    /// ([`Shared::early_start`]). A lane other than the first leaves the
    /// tests of the files it lists otherwise to the first lane
    /// ([`Queue::pin`]).
    fn lane(&self, lane: WorkerIndex, tx: &Sender<Message>) {
        let send = |message| {
            let _ = tx.send(message);
        };
        let _done = LaneDone(&self.queue, lane);
        let mut worker: Option<Worker> = None;
        // The adapter of the lane's last batch: its worker's.
        let mut adapter: Option<AdapterName> = None;
        // The worker's session ended with its last batch: it runs no other.
        let mut ended = false;
        // With one lane: whether the worker was announced, and the worker
        // that listed the session, waiting while another adapter's runs.
        let one = self.lanes <= 1;
        let mut announced = true;
        let mut parked: Option<(Worker, AdapterName, bool)> = None;
        // The files whose tests this lane's worker lists otherwise than the
        // first lane's: their ids run on the first lane.
        let mut pinned: HashSet<ProjectPath> = HashSet::new();
        match self.early_start(lane) {
            Ok(None) => {}
            Ok(Some((started, of, digests))) => {
                if one {
                    announced = false;
                } else {
                    send(Message::WorkerStarted {
                        worker: lane,
                        pid: started.pid(),
                        python: started.python().to_string(),
                        version: started.version().to_string(),
                    });
                }
                pinned = self.compare(lane, &of, digests.as_ref(), &send);
                self.queue.may_pin(lane, !pinned.is_empty());
                worker = Some(started);
                adapter = Some(of);
            }
            Err(error) => {
                if !self.stopping() {
                    send(Message::WorkerFailed {
                        worker: lane,
                        error,
                    });
                }
                return;
            }
        }
        while !self.exhausted()
            && let Some(batch) = self
                .queue
                .take(lane, if one { None } else { adapter.as_ref() })
        {
            if adapter.as_ref() != Some(&batch.adapter) || ended {
                let park = one
                    && !ended
                    && parked.is_none()
                    && worker.is_some()
                    && adapter.as_ref().is_some_and(|of| self.lists_session(of));
                if park {
                    let of = adapter.take().expect("the worker's adapter");
                    parked = Some((worker.take().expect("checked above"), of, announced));
                } else {
                    if let Some(of) = &adapter
                        && !ended
                    {
                        announce(lane, worker.as_ref(), &mut announced, &send);
                        self.end_session(lane, &mut worker, of, &send);
                    }
                    if let Some(other) = worker.take() {
                        announce(lane, Some(&other), &mut announced, &send);
                        let exit = other.shutdown();
                        send(Message::WorkerFinished {
                            worker: lane,
                            exit_status: exit.code(),
                            error: None,
                        });
                    }
                }
                adapter = Some(batch.adapter.clone());
                ended = false;
                if parked
                    .as_ref()
                    .is_some_and(|(_, of, _)| *of == batch.adapter)
                {
                    let (waiting, _, was) = parked.take().expect("checked above");
                    worker = Some(waiting);
                    announced = was;
                }
            }
            if worker.is_none() {
                match self.start(lane, &batch.adapter) {
                    Ok((started, digests)) => {
                        send(Message::WorkerStarted {
                            worker: lane,
                            pid: started.pid(),
                            python: started.python().to_string(),
                            version: started.version().to_string(),
                        });
                        announced = true;
                        pinned = self.compare(lane, &batch.adapter, digests.as_ref(), &send);
                        self.queue.may_pin(lane, !pinned.is_empty());
                        worker = Some(started);
                        ended = false;
                    }
                    Err(error) => {
                        let halted = self.stopping();
                        self.queue.put_back(batch);
                        if !halted {
                            send(Message::WorkerFailed {
                                worker: lane,
                                error,
                            });
                        }
                        return;
                    }
                }
            }
            let batch = if pinned.is_empty() {
                batch
            } else {
                match self
                    .queue
                    .pin(batch, |id| pinned.contains(id.path().as_str()))
                {
                    Some(kept) => kept,
                    None => continue,
                }
            };
            let running = worker.as_mut().expect("started above");
            if !self.busy(lane, running.killer()) {
                self.idle(lane);
                self.queue.put_back(batch);
                break;
            }
            ended |= batch.ends_session && self.lists_session(&batch.adapter);
            announce(lane, worker.as_ref(), &mut announced, &send);
            self.execute(lane, &mut worker, &batch, &send);
            self.queue.finished(&batch);
        }
        // The lane's worker, then the one waiting: each ends its session,
        // announced first if it never was.
        let waiting = parked.map(|(waiting, of, was)| (waiting, Some(of), false, was));
        let current = worker.map(|current| (current, adapter, ended, announced));
        for (worker, of, ended, mut announced) in current.into_iter().chain(waiting) {
            let mut worker = Some(worker);
            announce(lane, worker.as_ref(), &mut announced, &send);
            if let Some(of) = of.filter(|_| !ended) {
                self.end_session(lane, &mut worker, &of, &send);
            }
            if let Some(worker) = worker {
                let exit = worker.shutdown();
                send(Message::WorkerFinished {
                    worker: lane,
                    exit_status: exit.code(),
                    error: None,
                });
            }
        }
        let unused = lock(&self.prelisted).remove(&lane);
        shut_down(unused.into_iter().collect(), self.stop.is_stopped());
    }

    /// Run `batch` on the lane's `worker` (busy already) and report it: its
    /// start, its records, its end. A worker that dies meanwhile is gone
    /// from `worker` ([`Shared::crashed`]).
    fn execute(
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
            adapter: batch.adapter.clone(),
            cases: u32::try_from(batch.ids.len()).unwrap_or(u32::MAX),
            maxfail,
        });
        let end = self.run_batch(lane, running, batch, maxfail, send);
        let interrupted = self.idle(lane);
        match end {
            End::Done { status, track } => {
                let remaining = track.remaining(&batch.ids);
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
                        batch.adapter
                    ),
                });
                send(Message::BatchFinished {
                    worker: lane,
                    batch: batch.id,
                    status: None,
                    end: BatchEnd::Failed,
                    verdict: Verdict::default(),
                    not_run: track.remaining(&batch.ids),
                });
            }
            End::Eof(track) => {
                let dead = worker.take().expect("running above");
                self.crashed(lane, dead, batch, track, send);
            }
        }
    }

    /// Whether `adapter`'s executor lists the session.
    fn lists_session(&self, adapter: &AdapterName) -> bool {
        self.spec
            .executors
            .iter()
            .any(|e| e.adapter == *adapter && e.lists_session)
    }

    /// End the session of the lane's `worker`, of `adapter`, whose last
    /// batch didn't, in a batch of its own ([`Queue::closing`]), so that
    /// what fails there is reported as the session's end and its teardown
    /// isn't cut short by the worker's shutdown, when its executor lists the
    /// session. Not once the run is stopped (its workers are being killed);
    /// after `maxfail`, it still tears the session down, as pytest's `-x`
    /// does, and the log keeps what it reports as it keeps a halted run's.
    fn end_session(
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

    /// A worker for `adapter`'s batches, holding only its executor, that
    /// has listed the session if its executor lists the session: with its
    /// listing's digests then. The one started for the lane during
    /// refinement, if it is of that adapter ([`Shared::prelisted`]).
    fn start(
        &self,
        lane: WorkerIndex,
        adapter: &AdapterName,
    ) -> Result<(Worker, Option<Digests>), String> {
        let prelisted = {
            let mut prelisted = lock(&self.prelisted);
            match prelisted.remove(&lane) {
                Some(p) if p.adapter == *adapter => Some(p),
                Some(other) => {
                    prelisted.insert(lane, other);
                    None
                }
                None => None,
            }
        };
        if let Some(prelisted) = prelisted {
            return self.take_prelisted(lane, prelisted);
        }
        let mut spec = self.spec.clone();
        spec.executors
            .retain(|executor| executor.adapter == *adapter);
        spec.env.extend(lane_env(lane, self.lanes));
        let mut worker = Worker::launch(&spec).map_err(|e| e.to_string())?;
        if !self.busy(lane, worker.killer()) {
            worker.kill();
        }
        let started = match worker.handshake() {
            Ok(()) => self.list(worker, adapter, Digests::new()),
            Err(error) => Err(error.to_string()),
        };
        self.idle(lane);
        started
    }

    /// The worker started for `lane` during refinement, once it has
    /// answered its listings and listed the rest of the session.
    fn take_prelisted(
        &self,
        lane: WorkerIndex,
        prelisted: Prelisting,
    ) -> Result<(Worker, Option<Digests>), String> {
        let Prelisting {
            adapter,
            worker,
            pending,
            ..
        } = prelisted;
        let worker = started(worker)?;
        worker.set_echo(self.spec.echo);
        if !self.busy(lane, worker.killer()) {
            worker.kill();
        }
        let mut digests = Some(Digests::new());
        let mut unrefined = None;
        for _ in 0..pending {
            match refined_by(&worker, &|| self.stopping()) {
                Ok(refined) => match (&mut digests, refined.digests) {
                    (Some(all), Some(these)) => all.extend(these),
                    _ => digests = None,
                },
                Err(error) => {
                    unrefined = Some(error);
                    break;
                }
            }
        }
        let started = match unrefined {
            None => self.list(worker, &adapter, digests.unwrap_or_default()),
            Some(Unrefined::Failed(message)) => {
                worker.shutdown();
                Err(format!("the worker failed to list the session:\n{message}"))
            }
            Some(unrefined) => Err(unrefined.end(worker, "listing the session")),
        };
        self.idle(lane);
        started
    }

    /// A lane's worker before its first batch, with its adapter and its
    /// listing: for the first lane, the one kept from refinement; with
    /// several lanes, for each of the first [`Shared::early`] lanes, one of
    /// the adapter whose session they compare, so that a lane takes its
    /// first batch once it is ready for it, and the first lane's listing is
    /// what the others compare theirs with.
    fn early_start(
        &self,
        lane: WorkerIndex,
    ) -> Result<Option<(Worker, AdapterName, Option<Digests>)>, String> {
        if !lane.is_first() {
            return match &self.reference {
                Some(adapter) if lane.as_u32() < self.early => self
                    .start(lane, adapter)
                    .map(|(worker, digests)| Some((worker, adapter.clone(), digests))),
                _ => Ok(None),
            };
        }
        let warm = lock(&self.warm).take();
        let started = match (warm, &self.reference) {
            (Some(warm), _) => {
                let Warm {
                    adapter,
                    worker,
                    digests,
                    ..
                } = warm;
                let listed = if self.busy(WorkerIndex::FIRST, worker.killer()) {
                    self.list(worker, &adapter, digests)
                } else {
                    worker.kill();
                    Err("interrupted".into())
                };
                self.idle(WorkerIndex::FIRST);
                listed.map(|(worker, digests)| (worker, adapter, digests))
            }
            (None, Some(adapter)) => self
                .start(WorkerIndex::FIRST, adapter)
                .map(|(worker, digests)| (worker, adapter.clone(), digests)),
            (None, None) => return Ok(None),
        };
        if let Some(reference) = &self.reference {
            let digests = match &started {
                Ok((_, adapter, digests)) if adapter == reference => digests.clone(),
                _ => None,
            };
            self.listings.publish(reference, digests);
        }
        started.map(Some)
    }

    /// Have `worker` list the files of `adapter`'s session it hasn't yet
    /// (those `listed` doesn't hold), when its executor lists the session:
    /// the worker, with the digests of every file it listed. A worker that
    /// fails to list is shut down (or killed), and why is the error.
    fn list(
        &self,
        mut worker: Worker,
        adapter: &AdapterName,
        mut listed: Digests,
    ) -> Result<(Worker, Option<Digests>), String> {
        let lists = self
            .spec
            .executors
            .iter()
            .any(|e| e.adapter == *adapter && e.lists_session);
        let Some(session) = self.sessions.get(adapter).filter(|_| lists) else {
            return Ok((worker, None));
        };
        let files: Vec<ProjectPath> = session
            .iter()
            .filter(|file| !listed.contains_key(*file))
            .cloned()
            .collect();
        if files.is_empty() {
            return Ok((worker, Some(listed)));
        }
        let request = RefineRequest {
            adapter: adapter.clone(),
            files,
            context: self.contexts.get(adapter).cloned().unwrap_or_default(),
            digests_only: true,
        };
        match refine_on(&mut worker, request, &|| self.stopping()) {
            Ok(refined) => {
                let digests = refined.digests.map(|digests| {
                    listed.extend(digests);
                    listed
                });
                Ok((worker, digests))
            }
            Err(Unrefined::Failed(message)) => {
                worker.shutdown();
                Err(format!("the worker failed to list the session:\n{message}"))
            }
            Err(unrefined) => Err(unrefined.end(worker, "listing the session")),
        }
    }

    /// The files whose tests lane `lane`'s new worker lists (`digests`)
    /// otherwise than the first lane's, which run on the first lane only,
    /// each warned about once. None for the first lane itself, or without
    /// both listings.
    fn compare(
        &self,
        lane: WorkerIndex,
        adapter: &AdapterName,
        digests: Option<&Digests>,
        send: &dyn Fn(Message),
    ) -> HashSet<ProjectPath> {
        let Some(digests) = digests.filter(|_| !lane.is_first()) else {
            return HashSet::new();
        };
        let Some(reference) = self.listings.reference(adapter, &|| self.stopping()) else {
            return HashSet::new();
        };
        let differ: Vec<&ProjectPath> = self
            .sessions
            .get(adapter)
            .into_iter()
            .flatten()
            .filter(|file| digests.get(*file) != reference.get(*file))
            .collect();
        let mut warned = lock(&self.listings.warned);
        for file in &differ {
            if warned.insert((*file).clone()) {
                send(Message::Warning {
                    worker: lane,
                    diagnostic: Diagnostic {
                        severity: Severity::Warning,
                        code: DiagnosticCode::ListingDiffers,
                        path: (*file).clone().into(),
                        span: None,
                        message: format!(
                            "worker {lane} lists other tests in this file than worker 0 \
                             (their order or parameters differ between processes), so they \
                             run on worker 0 only"
                        ),
                    },
                });
            }
        }
        differ.into_iter().cloned().collect()
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
        let mut context = self
            .contexts
            .get(&batch.adapter)
            .cloned()
            .unwrap_or_default();
        if !batch.ran.is_empty() && context.is_null() {
            context = serde_json::Value::Object(serde_json::Map::new());
        }
        if !batch.ran.is_empty()
            && let Some(object) = context.as_object_mut()
        {
            let ran: Vec<&str> = batch.ran.iter().map(TestId::as_str).collect();
            object.insert(RAN_IDS.to_string(), ran.into());
        }
        let request = Request::Run(Batch {
            id: batch.id,
            adapter: batch.adapter.clone(),
            ids: batch.ids.clone(),
            policy,
            context,
            ends_session: batch.ends_session && self.lists_session(&batch.adapter),
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

    /// The worker died before finishing `batch`. The test it was running
    /// becomes an error; the ids that never started under their own id are
    /// re-queued once, and become errors if they crash a worker again.
    ///
    /// An id that stands for several tests (one without its parameters, or
    /// whose parameters the framework makes under other ids now: pytest's
    /// stand-ins) may have started some of them: it is re-queued whole,
    /// with the ids of the tests that ran ([`RAN_IDS`] in its context), so
    /// that the adapter runs only the rest, and none twice.
    fn crashed(
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
            closing(BatchEnd::Stopped, None, track.remaining(&batch.ids));
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

        let retried = track.unfinished(&batch.ids);
        let unstarted: HashSet<TestId> = track.remaining(&batch.ids).into_iter().collect();
        let ran = track.ran.clone();
        let in_flight = track.in_flight.map(|(id, since)| {
            synthesize(
                id.clone(),
                format!("worker crashed while running this test: {reason}"),
                since.elapsed(),
            );
            id
        });
        if batch.retry {
            for id in retried {
                let message = if unstarted.contains(&id) {
                    format!("worker crashed again before this test ran: {reason}")
                } else {
                    format!("worker crashed again before every test of this id ran: {reason}")
                };
                synthesize(id, message, Duration::ZERO);
            }
        } else if !retried.is_empty() {
            let mut ran_before = batch.ran.clone();
            ran_before.extend(ran);
            self.queue.retry(batch, retried, ran_before);
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

/// Send lane `lane`'s `worker_started` for `worker`, unless `announced`
/// says it was (with one lane, a worker that listed the session is
/// announced when it is first used).
fn announce(
    lane: WorkerIndex,
    worker: Option<&Worker>,
    announced: &mut bool,
    send: &dyn Fn(Message),
) {
    if let Some(worker) = worker
        && !*announced
    {
        *announced = true;
        send(Message::WorkerStarted {
            worker: lane,
            pid: worker.pid(),
            python: worker.python().to_string(),
            version: worker.version().to_string(),
        });
    }
}

/// Which of a batch's tests started and which one is running, for crash
/// recovery.
#[derive(Debug, Default)]
struct Track {
    in_flight: Option<(TestId, Instant)>,
    /// Bad outcomes the batch reported.
    bad: u32,
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
    stopped: bool,
    /// The batch reported an error diagnostic.
    errors: bool,
}

impl Track {
    fn see(&mut self, event: &Event) {
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
    fn remaining(&self, ids: &[TestId]) -> Vec<TestId> {
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
    fn clean(&self) -> bool {
        self.bad == 0 && !self.errors
    }
}

/// What the framework's end of a batch means for the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Verdict {
    /// The framework stopped the batch, ending its session there, so the
    /// run hands nothing more out (see [`StopReason::Framework`]).
    stop: bool,
    /// The framework failed internally: pytest's status 3.
    internal_error: bool,
    /// The framework says tests failed (pytest's status 1), and none did:
    /// see [`RunSummary::framework_failed`].
    failed: bool,
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
    fn of(status: Option<i32>, stopped: bool, clean: bool) -> Self {
        Self {
            stop: stopped || status == Some(STATUS_INTERRUPTED),
            internal_error: status == Some(STATUS_INTERNAL_ERROR),
            failed: clean && status == Some(STATUS_TESTS_FAILED),
        }
    }
}

/// Whether `failure` is a `KeyboardInterrupt`'s.
fn is_interrupt(failure: Option<&Failure>) -> bool {
    failure.is_some_and(|failure| failure.exception.as_deref() == Some("KeyboardInterrupt"))
}

fn first_path(batch: &Assigned) -> DiagnosticPath {
    batch.ids.first().map_or(DiagnosticPath::Nowhere, |id| {
        DiagnosticPath::as_written(id.path().as_str())
    })
}

fn shorten(line: &str) -> String {
    const MAX: usize = 500;
    if line.len() <= MAX {
        return line.to_string();
    }
    let mut end = MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}

#[cfg(test)]
mod tests {
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
    fn long_lines_are_shortened_on_a_char_boundary() {
        let line = "é".repeat(400);
        let short = shorten(&line);
        assert!(short.ends_with('…') && short.len() <= 503);
        assert_eq!(shorten("short"), "short");
    }

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
    fn stop_reasons_are_spelled_as_run_finished_spells_them() {
        let names: Vec<&str> = [
            StopReason::Maxfail,
            StopReason::Interrupted,
            StopReason::Framework,
        ]
        .into_iter()
        .map(StopReason::as_str)
        .collect();
        assert_eq!(names, ["maxfail", "interrupted", "framework"]);
        assert_eq!(StopReason::Framework.to_string(), "framework");
    }

    #[test]
    fn the_framework_stops_the_run_by_a_batch_stopped_error_or_status_2() {
        let stop = Verdict {
            stop: true,
            ..Verdict::default()
        };
        assert_eq!(Verdict::of(Some(2), false, false), stop);
        // `pytest.exit(returncode=…)`, whatever the code: 4 is no usage error.
        for status in [Some(0), Some(2), Some(4), Some(5), Some(7), None] {
            assert_eq!(Verdict::of(status, true, false), stop, "{status:?}");
        }
        // A status alone says nothing about the tests left, but 2.
        for status in [Some(0), Some(1), Some(4), Some(5), Some(7), Some(-1), None] {
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
            Verdict::of(Some(3), true, false),
            Verdict {
                stop: true,
                internal_error: true,
                failed: false,
            }
        );
        // pytest's INTERNALERROR in `pytest_sessionfinish`: every test ran.
        assert_eq!(
            Verdict::of(Some(3), false, true),
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
        assert!(Verdict::of(Some(1), false, true).failed);
        // A bad test or an error diagnostic explains it.
        assert!(!Verdict::of(Some(1), false, false).failed);
        for status in [Some(0), Some(5), None] {
            assert!(!Verdict::of(status, false, true).failed, "{status:?}");
        }
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
