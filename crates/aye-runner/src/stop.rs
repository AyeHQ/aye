//! Stopping a runner from another thread: the [`StopHandle`] the binary's
//! signal handler holds, and the [`Registry`] of every live worker, which
//! it kills.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::sync::lock;
use crate::worker::{Proc, kill_groups};

/// Stops a run from another thread (the binary's signal handler). Sticky: a
/// runner whose handle was used starts no more runs or workers.
#[derive(Debug, Clone, Default)]
pub struct StopHandle {
    stopped: Arc<AtomicBool>,
    /// Every live worker of the runner: its runs' lanes, handshakes and
    /// refinements.
    pub(crate) workers: Registry,
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

/// A runner's live workers, so that all of them can be killed at once from
/// any thread (the binary's second Ctrl-C).
#[derive(Debug, Clone, Default)]
pub(crate) struct Registry(Arc<Mutex<Live>>);

#[derive(Debug, Default)]
struct Live {
    workers: Vec<Weak<Mutex<Proc>>>,
    /// [`Registry::kill_all`] ran: workers that join later are killed at
    /// once.
    killed: bool,
}

impl Registry {
    pub(crate) fn join(&self, proc: &Arc<Mutex<Proc>>) {
        let mut live = lock(&self.0);
        live.workers.retain(|worker| worker.strong_count() > 0);
        if live.killed {
            lock(proc).kill();
        } else {
            live.workers.push(Arc::downgrade(proc));
        }
    }

    /// Kill every live worker with its process group, in one go, and every
    /// worker that starts from now on.
    pub(crate) fn kill_all(&self) {
        let mut live = lock(&self.0);
        live.killed = true;
        let workers: Vec<Arc<Mutex<Proc>>> =
            live.workers.iter().filter_map(Weak::upgrade).collect();
        // Held until each is signalled, so that none is reaped meanwhile.
        let mut procs: Vec<MutexGuard<'_, Proc>> = workers.iter().map(|w| lock(w)).collect();
        let pids: Vec<u32> = procs
            .iter()
            .filter(|proc| !proc.reaped)
            .map(|proc| proc.child.id())
            .collect();
        kill_groups(&pids);
        for proc in &mut procs {
            if !proc.reaped {
                let _ = proc.child.kill();
            }
        }
    }
}
