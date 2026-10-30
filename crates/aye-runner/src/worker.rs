//! The single spawn path and the transport.
//!
//! Every worker, whatever it is for (a run's batches or a refinement), starts
//! here: `python -c <aye_worker source>` with its standard streams piped,
//! the handshake sent on stdin, and one JSON line per message back on
//! stdout. Its stderr is drained on a thread, kept for crash reports and
//! echoed live when output isn't captured.
//!
//! A worker leads its own process group, and killing a worker kills that
//! group, so that the processes its tests started die with it. A worker's
//! death is noticed by its pid as well as by the end of its stream, which a
//! process its tests started may hold open.
//!
//! The spawn path is in `handshake.rs` ([`Worker::launch`], then
//! [`Worker::handshake`]), from a [`WorkerSpec`] (`spec.rs`); the protocol
//! stream in `transport.rs`; the process and its group in `process.rs`;
//! how a worker ended in `exit.rs`; its stderr in `stderr.rs`, and what of
//! it goes into a crash report in `crash_report.rs`; every grace period
//! and poll in `timeouts.rs`.

use std::process::ChildStdin;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use aye_protocol::Request;

use crate::sync::lock;

mod crash_report;
mod exit;
mod handshake;
mod process;
mod spec;
mod stderr;
pub(crate) mod timeouts;
mod transport;

pub use exit::{Exit, describe};
pub use handshake::SpawnError;
pub use process::Killer;
pub(crate) use process::{Proc, kill_groups};
pub use spec::WorkerSpec;
pub use stderr::Echo;
pub(crate) use transport::is_reply;
pub use transport::{Incoming, parse_line};

use exit::Waited;
use stderr::Stderr;
use timeouts::{EXIT_GRACE, KILL_WAIT, SHUTDOWN_GRACE, STDERR_GRACE, STDERR_QUIET};

/// A running worker that has answered the handshake.
#[derive(Debug)]
pub struct Worker {
    proc: Arc<Mutex<Proc>>,
    stdin: Option<ChildStdin>,
    lines: Receiver<Incoming>,
    stderr: Stderr,
    /// The handshake, until it is sent.
    hello: Option<Request>,
    pid: u32,
    python: String,
    version: String,
}

/// Start a worker and complete the handshake.
pub fn spawn(spec: &WorkerSpec) -> Result<Worker, SpawnError> {
    let mut worker = Worker::launch(spec)?;
    worker.handshake()?;
    Ok(worker)
}

impl Worker {
    /// The worker's process id, as it reported it.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// `sys.executable` in the worker.
    pub fn python(&self) -> &str {
        &self.python
    }

    /// The first line of `sys.version` in the worker.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Whether the worker process is gone (or can't be waited for).
    fn exited(&self) -> bool {
        let mut proc = lock(&self.proc);
        proc.reaped || !matches!(proc.try_wait(), Ok(None))
    }

    pub fn killer(&self) -> Killer {
        Killer(Arc::clone(&self.proc))
    }

    /// Kill the worker and its process group.
    pub fn kill(&self) {
        lock(&self.proc).kill();
    }

    /// Ask the worker to exit, and wait for it, killing it (and its process
    /// group) if it takes longer than a few seconds (a thread the tests left
    /// running).
    pub fn shutdown(mut self) -> Exit {
        let _ = self.send(&Request::Shutdown);
        self.stdin.take();
        self.reap(SHUTDOWN_GRACE, STDERR_QUIET)
    }

    /// Wait for a worker whose stream has ended, killing it (and its process
    /// group) if it doesn't exit soon.
    pub fn wait(mut self) -> Exit {
        self.stdin.take();
        self.reap(EXIT_GRACE, STDERR_GRACE)
    }

    /// Wait up to `grace` for the worker to exit, then kill it; then wait
    /// for the rest of its stderr, but no longer than it stays silent for
    /// `quiet`.
    fn reap(&mut self, grace: Duration, quiet: Duration) -> Exit {
        let status = match self.exit_within(grace) {
            Waited::Exited(status) => Some(status),
            Waited::Unknown => None,
            Waited::Running => {
                self.kill();
                match self.exit_within(KILL_WAIT) {
                    Waited::Exited(status) => Some(status),
                    Waited::Running | Waited::Unknown => None,
                }
            }
        };
        Exit {
            status,
            stderr: self.stderr.tail(STDERR_GRACE, quiet),
        }
    }

    /// Poll for the worker's exit for up to `within`. The process is only
    /// locked for each poll, so that it can be killed meanwhile.
    fn exit_within(&self, within: Duration) -> Waited {
        let deadline = Instant::now() + within;
        let mut pause = Duration::from_millis(1);
        loop {
            let polled = lock(&self.proc).try_wait();
            match polled {
                Ok(Some(status)) => return Waited::Exited(status),
                Err(_) => return Waited::Unknown,
                Ok(None) if Instant::now() >= deadline => return Waited::Running,
                Ok(None) => {}
            }
            thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(20));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let polled = lock(&self.proc).try_wait();
        if matches!(polled, Ok(None)) {
            self.kill();
            let _ = self.exit_within(KILL_WAIT);
        }
    }
}
