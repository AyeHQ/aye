//! A lane: a worker slot that pulls batches until none are left, and how
//! it gets its worker ready (started, pre-listed or kept from
//! refinement, with the session listed and compared).

use std::collections::HashSet;
use std::sync::mpsc::Sender;

use aye_events::{AdapterName, Diagnostic, DiagnosticCode, ProjectPath, Severity, WorkerIndex};
use aye_protocol::RefineRequest;

use crate::env::lane_env;
use crate::listing::{Digests, Prelisting, Warm, shut_down, started};
use crate::refine::{Unrefined, refine_on, refined_by};
use crate::schedule::Queue;
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
    pub(super) fn lane(&self, lane: WorkerIndex, tx: &Sender<Message>) {
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
            if adapter.as_ref() != Some(&batch.spec.adapter) || ended {
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
                adapter = Some(batch.spec.adapter.clone());
                ended = false;
                if parked
                    .as_ref()
                    .is_some_and(|(_, of, _)| *of == batch.spec.adapter)
                {
                    let (waiting, _, was) = parked.take().expect("checked above");
                    worker = Some(waiting);
                    announced = was;
                }
            }
            if worker.is_none() {
                match self.start(lane, &batch.spec.adapter) {
                    Ok((started, digests)) => {
                        send(Message::WorkerStarted {
                            worker: lane,
                            pid: started.pid(),
                            python: started.python().to_string(),
                            version: started.version().to_string(),
                        });
                        announced = true;
                        pinned = self.compare(lane, &batch.spec.adapter, digests.as_ref(), &send);
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
            ended |= batch.ends_session && self.lists_session(&batch.spec.adapter);
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
        let lists = self.lists_session(adapter);
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
            context: self.plan.context(adapter),
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
