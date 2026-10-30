//! A lane: a worker slot that pulls batches until none are left
//! ([`Lane`], each of its workers a [`LaneWorker`]), and how it gets its
//! worker ready (started, pre-listed or kept from refinement, with the
//! session listed and compared).

use std::collections::HashSet;
use std::sync::mpsc::Sender;

use aye_events::{AdapterName, Diagnostic, DiagnosticCode, ProjectPath, Severity, WorkerIndex};
use aye_protocol::RefineRequest;

use crate::env::lane_env;
use crate::listing::{Digests, Prelisting, Warm, shut_down, started};
use crate::refine::{Unrefined, refine_on, refined_by};
use crate::schedule::{Assigned, Queue};
use crate::sync::lock;
use crate::worker::Worker;

use super::Shared;
use super::message::Message;

/// Tells the queue a lane is done taking batches, however it ends.
struct LaneDone<'q>(&'q Queue, WorkerIndex);

impl Drop for LaneDone<'_> {
    fn drop(&mut self) {
        self.0.lane_done(self.1);
    }
}

/// A lane of a run, as it pulls batches ([`Shared::lane`]).
struct Lane<'s, 'a> {
    shared: &'s Shared<'a>,
    index: WorkerIndex,
    send: &'s dyn Fn(Message),
    /// The adapter of the lane's last batch, whose batches it asks for
    /// first with several lanes: its worker's, or that of the worker that
    /// crashed.
    adapter: Option<AdapterName>,
    /// Its worker, of that adapter: none before the lane's first batch, or
    /// once it crashed.
    worker: Option<LaneWorker>,
    /// With one lane, the worker that listed the session, waiting while
    /// another adapter's runs.
    parked: Option<LaneWorker>,
    /// The files whose tests the lane's worker lists otherwise than the
    /// first lane's: their ids run on the first lane.
    pinned: HashSet<ProjectPath>,
}

/// A lane's worker: it serves one adapter.
struct LaneWorker {
    worker: Worker,
    adapter: AdapterName,
    session: SessionState,
    announced: Announced,
}

/// Where a lane worker's session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionState {
    /// Its batches run in it, or each in its own for an executor that
    /// doesn't list the session.
    Open,
    /// Its last batch ended it ([`Assigned::ends_session`], for an executor
    /// that lists the session): the worker runs no other.
    Ended,
}

/// Whether a lane worker's `worker_started` was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Announced {
    Yes,
    /// Not yet: with one lane, a worker that listed the session before its
    /// first batch is announced when it first runs one, so that each
    /// worker's events come between its start and its end.
    Deferred,
}

/// A worker ready for its adapter's batches, with the digests of the
/// session files it listed, when its executor lists the session.
struct Listed {
    worker: Worker,
    digests: Option<Digests>,
}

impl Shared<'_> {
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
    pub(super) fn lane(&self, index: WorkerIndex, tx: &Sender<Message>) {
        let send = |message| {
            let _ = tx.send(message);
        };
        let _done = LaneDone(&self.queue, index);
        let Some(mut lane) = Lane::open(self, index, &send) else {
            return;
        };
        if lane.run() {
            lane.close();
        }
    }

    /// A worker for `adapter`'s batches, holding only its executor, that
    /// has listed the session if its executor lists the session: with its
    /// listing's digests then. The one started for the lane during
    /// refinement, if it is of that adapter ([`Shared::prelisted`]).
    fn start(&self, lane: WorkerIndex, adapter: &AdapterName) -> Result<Listed, String> {
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
        spec.runtimes = self.runtimes.get(adapter).cloned().into_iter().collect();
        spec.env.extend(lane_env(lane, self.lanes));
        let launched = Worker::launch(&spec).map_err(|e| e.to_string())?;
        if !self.busy(lane, launched.killer()) {
            launched.kill();
        }
        let started = match launched.handshake() {
            Ok(worker) => self.list(worker, adapter, Digests::new()),
            Err(error) => Err(error.to_string()),
        };
        self.idle(lane);
        started
    }

    /// The worker started for `lane` during refinement, once it has
    /// answered its listings and listed the rest of the session.
    fn take_prelisted(&self, lane: WorkerIndex, prelisted: Prelisting) -> Result<Listed, String> {
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
    fn early_start(&self, lane: WorkerIndex) -> Result<Option<(AdapterName, Listed)>, String> {
        if !lane.is_first() {
            return match &self.reference {
                Some(adapter) if lane.as_u32() < self.early => self
                    .start(lane, adapter)
                    .map(|listed| Some((adapter.clone(), listed))),
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
                listed.map(|listed| (adapter, listed))
            }
            (None, Some(adapter)) => self
                .start(WorkerIndex::FIRST, adapter)
                .map(|listed| (adapter.clone(), listed)),
            (None, None) => return Ok(None),
        };
        if let Some(reference) = &self.reference {
            let digests = match &started {
                Ok((adapter, listed)) if adapter == reference => listed.digests.clone(),
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
    ) -> Result<Listed, String> {
        let lists = self.lists_session(adapter);
        let Some(session) = self.sessions.get(adapter).filter(|_| lists) else {
            return Ok(Listed {
                worker,
                digests: None,
            });
        };
        let files: Vec<ProjectPath> = session
            .iter()
            .filter(|file| !listed.contains_key(*file))
            .cloned()
            .collect();
        if files.is_empty() {
            return Ok(Listed {
                worker,
                digests: Some(listed),
            });
        }
        let request = RefineRequest {
            adapter: adapter.clone(),
            files,
            context: self.plan.context(adapter),
            digests_only: true,
        };
        match refine_on(&mut worker, request, &|| self.stopping()) {
            Ok(refined) => {
                let digests = refined.digests.map(|digests| {
                    listed.extend(digests);
                    listed
                });
                Ok(Listed { worker, digests })
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
}

impl<'s, 'a> Lane<'s, 'a> {
    /// Lane `index`, with the worker it starts with, if any
    /// ([`Shared::early_start`]); `None` if that worker failed to start,
    /// which ends the lane.
    fn open(shared: &'s Shared<'a>, index: WorkerIndex, send: &'s dyn Fn(Message)) -> Option<Self> {
        let mut lane = Self {
            shared,
            index,
            send,
            adapter: None,
            worker: None,
            parked: None,
            pinned: HashSet::new(),
        };
        match shared.early_start(index) {
            Ok(None) => {}
            Ok(Some((adapter, listed))) => {
                let mut worker = LaneWorker::new(listed.worker, adapter.clone());
                if !lane.alone() {
                    worker.announce(index, send);
                }
                lane.compare(&adapter, listed.digests.as_ref());
                lane.worker = Some(worker);
                lane.adapter = Some(adapter);
            }
            Err(error) => {
                if !shared.stopping() {
                    send(Message::WorkerFailed {
                        worker: index,
                        error,
                    });
                }
                return None;
            }
        }
        Some(lane)
    }

    /// Whether the run has this lane only.
    fn alone(&self) -> bool {
        self.shared.lanes <= 1
    }

    /// The adapter whose batches the lane asks for first: with several
    /// lanes, its last batch's; with one, none (batches run in plan order).
    fn prefers(&self) -> Option<&AdapterName> {
        if self.alone() {
            None
        } else {
            self.adapter.as_ref()
        }
    }

    /// Pull batches until none are left for the lane or the bad outcomes
    /// passed on reach `maxfail`; false if a worker failed to start, which
    /// ends the lane there.
    fn run(&mut self) -> bool {
        let shared = self.shared;
        while !shared.maxfail.exhausted()
            && let Some(batch) = shared.queue.take(self.index, self.prefers())
        {
            let ended = self
                .worker
                .as_ref()
                .is_some_and(|worker| worker.session == SessionState::Ended);
            if self.adapter.as_ref() != Some(&batch.spec.adapter) || ended {
                self.leave_for(&batch.spec.adapter);
            }
            if self.worker.is_none() {
                match shared.start(self.index, &batch.spec.adapter) {
                    Ok(listed) => {
                        let mut worker = LaneWorker::new(listed.worker, batch.spec.adapter.clone());
                        worker.announce(self.index, self.send);
                        self.compare(&batch.spec.adapter, listed.digests.as_ref());
                        self.worker = Some(worker);
                    }
                    Err(error) => {
                        let halted = shared.stopping();
                        shared.queue.put_back(batch);
                        if !halted {
                            (self.send)(Message::WorkerFailed {
                                worker: self.index,
                                error,
                            });
                        }
                        return false;
                    }
                }
            }
            let batch = if self.pinned.is_empty() {
                batch
            } else {
                match shared
                    .queue
                    .pin(batch, |id| self.pinned.contains(id.path().as_str()))
                {
                    Some(kept) => kept,
                    None => continue,
                }
            };
            let current = self.worker.as_mut().expect("started above");
            if !shared.busy(self.index, current.worker.killer()) {
                shared.idle(self.index);
                shared.queue.put_back(batch);
                break;
            }
            if batch.ends_session && shared.lists_session(&batch.spec.adapter) {
                current.session = SessionState::Ended;
            }
            current.announce(self.index, self.send);
            self.execute(&batch);
            shared.queue.finished(&batch);
        }
        true
    }

    /// Leave the lane's worker for a batch of `adapter`, another adapter's
    /// or one its session can't take: with one lane, a worker whose
    /// executor lists the session waits, unless one already does; any
    /// other ends its session and shuts down ([`Lane::retire`]). The worker
    /// waiting for `adapter`, if any, is the lane's again.
    fn leave_for(&mut self, adapter: &AdapterName) {
        if let Some(current) = self.worker.take() {
            let park = self.alone()
                && current.session == SessionState::Open
                && self.parked.is_none()
                && self.shared.lists_session(&current.adapter);
            if park {
                self.parked = Some(current);
            } else {
                self.retire(current);
            }
        }
        self.adapter = Some(adapter.clone());
        if self
            .parked
            .as_ref()
            .is_some_and(|waiting| waiting.adapter == *adapter)
        {
            self.worker = self.parked.take();
        }
    }

    /// The files whose tests the lane's new worker of `adapter` lists
    /// (`digests`) otherwise than the first lane's ([`Shared::compare`]),
    /// and whether the lane may pin their batches to the first lane from
    /// now on ([`Queue::may_pin`]).
    fn compare(&mut self, adapter: &AdapterName, digests: Option<&Digests>) {
        self.pinned = self.shared.compare(self.index, adapter, digests, self.send);
        self.shared
            .queue
            .may_pin(self.index, !self.pinned.is_empty());
    }

    /// Run `batch` on the lane's worker, which is gone if it dies meanwhile.
    fn execute(&mut self, batch: &Assigned) {
        let mut current = self.worker.take().expect("a worker runs the batch");
        if let Some(worker) = self
            .shared
            .execute(self.index, current.worker, batch, self.send)
        {
            current.worker = worker;
            self.worker = Some(current);
        }
    }

    /// The lane is done: its worker, then the one waiting, each end their
    /// session; then the worker started for the lane during refinement, if
    /// it never took it, shuts down.
    fn close(mut self) {
        let current = self.worker.take();
        let waiting = self.parked.take();
        for worker in current.into_iter().chain(waiting) {
            self.retire(worker);
        }
        let unused = lock(&self.shared.prelisted).remove(&self.index);
        shut_down(unused.into_iter().collect(), self.shared.stop.is_stopped());
    }

    /// Let `worker` go: announced first if it never was, its session ended
    /// if it is open ([`Shared::end_session`]), then shut down, unless it
    /// died meanwhile.
    fn retire(&self, mut worker: LaneWorker) {
        worker.announce(self.index, self.send);
        let LaneWorker {
            worker,
            adapter,
            session,
            ..
        } = worker;
        let alive = match session {
            SessionState::Open => self
                .shared
                .end_session(self.index, worker, &adapter, self.send),
            SessionState::Ended => Some(worker),
        };
        if let Some(worker) = alive {
            let exit = worker.shutdown();
            (self.send)(Message::WorkerFinished {
                worker: self.index,
                exit_status: exit.code(),
                error: None,
            });
        }
    }
}

impl LaneWorker {
    /// A worker of `adapter`'s, its session open, not yet announced.
    fn new(worker: Worker, adapter: AdapterName) -> Self {
        Self {
            worker,
            adapter,
            session: SessionState::Open,
            announced: Announced::Deferred,
        }
    }

    /// Send lane `lane`'s `worker_started` for the worker, unless it was.
    fn announce(&mut self, lane: WorkerIndex, send: &dyn Fn(Message)) {
        if self.announced == Announced::Deferred {
            self.announced = Announced::Yes;
            send(Message::WorkerStarted {
                worker: lane,
                pid: self.worker.pid(),
                python: self.worker.python().to_string(),
                version: self.worker.version().to_string(),
            });
        }
    }
}
