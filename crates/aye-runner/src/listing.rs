//! The session listing's workers: the one kept from refinement for the
//! next run's first lane ([`Warm`]), those started for the other lanes
//! while it refines ([`Prelisting`]), and the lanes' listings, which the
//! other lanes compare with the first lane's ([`Listings`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Condvar, Mutex};
use std::thread;

use aye_events::{AdapterName, ProjectPath, WorkerIndex};
use aye_protocol::{ListingDigest, RefineRequest, Request, Runtime};

use crate::Runner;
use crate::env::lane_env;
use crate::sync::lock;
use crate::worker::timeouts::POLL;
use crate::worker::{Worker, spawn};

/// By file, a digest of its tests' ids in order ([`Refined::digests`](aye_protocol::Refined::digests)).
pub(crate) type Digests = BTreeMap<ProjectPath, ListingDigest>;

/// A worker kept from refinement, with what it listed.
#[derive(Debug)]
pub(crate) struct Warm {
    pub(crate) adapter: AdapterName,
    pub(crate) worker: Worker,
    /// The lanes its environment was made for (see [`lane_env`]).
    pub(crate) lanes: u32,
    pub(crate) digests: Digests,
}

/// A worker started for a lane other than the first while the first lane's
/// worker refines: it lists the files refined (digests only) meanwhile, so
/// that the run's lanes start ready rather than listing then.
#[derive(Debug)]
pub(crate) struct Prelisting {
    pub(crate) adapter: AdapterName,
    pub(crate) lane: WorkerIndex,
    /// The lanes its environment was made for (see [`lane_env`]).
    pub(crate) lanes: u32,
    /// Starts the worker and sends it its listings, in the background.
    pub(crate) worker: thread::JoinHandle<Result<Worker, String>>,
    /// Its listings sent and not answered yet.
    pub(crate) pending: usize,
}

/// The worker a [`Prelisting`]'s thread started, or why it couldn't.
pub(crate) fn started(
    worker: thread::JoinHandle<Result<Worker, String>>,
) -> Result<Worker, String> {
    worker
        .join()
        .unwrap_or_else(|_| Err("starting the worker panicked".into()))
}

/// Shut the workers down, or kill them once the runner is stopped, at
/// once.
pub(crate) fn shut_down(prelisting: Vec<Prelisting>, kill: bool) {
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

impl Runner {
    /// Have a worker for each lane but the first among `lanes` list
    /// `request`'s files (digests only) while the first lane's worker
    /// refines them: started in the background, with its lane's
    /// environment, or the one started for an earlier refinement of the
    /// adapter. Those started for another adapter or other lanes shut down.
    pub(crate) fn prelist(&self, runtime: &Runtime, request: &RefineRequest, lanes: u32) {
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
                let mut spec = self.spec(vec![runtime.clone()]);
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

/// The lanes' listings of the session: the first lane's, by adapter, which
/// the others compare theirs with, and the files already warned about.
pub(crate) struct Listings {
    reference: Mutex<HashMap<AdapterName, Reference>>,
    published: Condvar,
    pub(crate) warned: Mutex<HashSet<ProjectPath>>,
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
    pub(crate) fn new(reference: Option<&AdapterName>) -> Self {
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

    pub(crate) fn publish(&self, adapter: &AdapterName, digests: Option<Digests>) {
        let mut reference = lock(&self.reference);
        if let Some(slot) = reference.get_mut(adapter) {
            *slot = digests.map_or(Reference::Missing, Reference::Ready);
        }
        self.published.notify_all();
    }

    /// The first lane's listing of `adapter`'s session, once it is known;
    /// `None` without one, or once `stopping`.
    pub(crate) fn reference(
        &self,
        adapter: &AdapterName,
        stopping: &dyn Fn() -> bool,
    ) -> Option<Digests> {
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
