//! Refinement on workers: the runner as collection's
//! [`Refiner`].

use aye_protocol::{RefineError, RefineRequest, Refined, Refiner, Reply, Request, WorkerLine};

use crate::Runner;
use crate::env::LaneEnv;
use crate::listing::{Digests, Warm};
use crate::sync::lock;
use crate::worker::timeouts::POLL;
use crate::worker::{Incoming, Worker, shorten, spawn};

impl Refiner for Runner {
    /// Start a fresh worker with the adapter's executor, ask it to refine,
    /// and shut it down. For an executor that lists the session, the worker
    /// is kept instead, with the environment of a run's first lane, for the
    /// next refinement of that adapter and then the next run's first lane
    /// (or shut down when the runner is dropped); with several workers,
    /// one started for each other lane lists the same files meanwhile
    /// (unless [`RunnerOptions::collect_only`](crate::RunnerOptions::collect_only)).
    fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
        let runtime = self
            .runtimes
            .get(&request.adapter)
            .cloned()
            .ok_or_else(|| {
                RefineError(format!("no executor for the {} adapter", request.adapter))
            })?;
        if self.stop.is_stopped() {
            return Err(RefineError("interrupted".into()));
        }
        let stopped = || self.stop.is_stopped();
        if !runtime.session.lists_session() {
            let mut worker =
                spawn(&self.spec(vec![runtime])).map_err(|e| RefineError(e.to_string()))?;
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
            self.prelist(&runtime, request, lanes);
        }
        let mut warm = lock(&self.warm);
        let mut kept = match warm.take() {
            Some(kept) if kept.adapter == request.adapter => kept,
            other => {
                if let Some(other) = other {
                    other.worker.shutdown();
                }
                let mut spec = self.spec(vec![runtime]);
                let env = LaneEnv::first(lanes);
                spec.env.extend(env.vars());
                Warm {
                    adapter: request.adapter.clone(),
                    worker: spawn(&spec).map_err(|e| RefineError(e.to_string()))?,
                    env,
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
pub(crate) enum Unrefined {
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
    pub(crate) fn end(self, worker: Worker, doing: &str) -> String {
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
pub(crate) fn refine_on(
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
pub(crate) fn refined_by(
    worker: &Worker,
    stopped: &dyn Fn() -> bool,
) -> Result<Refined, Unrefined> {
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
