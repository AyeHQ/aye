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
//! A worker's states are types: [`Worker::launch`] starts its process, a
//! [`LaunchedWorker`], which can be killed, waited for or asked to exit
//! but takes no request; [`LaunchedWorker::handshake`] makes it a
//! [`Worker`], ready for requests, with the interpreter it reported; and
//! [`Worker::shutdown`] and [`Worker::wait`] end it, with its [`Exit`].
//!
//! The spawn path is in `handshake.rs` ([`Worker::launch`], then
//! [`LaunchedWorker::handshake`]), from a [`WorkerSpec`] (`spec.rs`); the
//! protocol stream in `transport.rs`; the process and its group in
//! `process.rs`; how a worker ended in `exit.rs`; its stderr in
//! `stderr.rs`, and what of it goes into a crash report in
//! `crash_report.rs`; every grace period and poll in `timeouts.rs`.

use std::io;
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
pub use handshake::{LaunchedWorker, SpawnError};
pub use process::Killer;
pub(crate) use process::{Proc, kill_groups};
pub use spec::WorkerSpec;
pub use stderr::Echo;
pub use transport::{Incoming, parse_line};
pub(crate) use transport::{is_reply, shorten};

use exit::Waited;
use stderr::Stderr;
use timeouts::{EXIT_GRACE, KILL_WAIT, SHUTDOWN_GRACE, STDERR_GRACE, STDERR_QUIET};

/// A worker process and the runner's ends of its standard streams: what a
/// worker holds in every state, launched ([`LaunchedWorker`]) or ready
/// ([`Worker`]). Killed with its process group if it still runs when
/// dropped.
#[derive(Debug)]
struct Process {
    proc: Arc<Mutex<Proc>>,
    stdin: Option<ChildStdin>,
    lines: Receiver<Incoming>,
    stderr: Stderr,
}

/// A running worker that has answered the handshake: ready for requests.
#[derive(Debug)]
pub struct Worker {
    process: Process,
    pid: u32,
    python: String,
    version: String,
}

/// Start a worker and complete the handshake.
pub fn spawn(spec: &WorkerSpec) -> Result<Worker, SpawnError> {
    Worker::launch(spec)?.handshake()
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

    pub fn killer(&self) -> Killer {
        self.process.killer()
    }

    /// Kill the worker and its process group.
    pub fn kill(&self) {
        self.process.kill();
    }

    /// Ask the worker to exit, and wait for it, killing it (and its process
    /// group) if it takes longer than a few seconds (a thread the tests left
    /// running).
    pub fn shutdown(self) -> Exit {
        self.process.shutdown()
    }

    /// Wait for a worker whose stream has ended, killing it (and its process
    /// group) if it doesn't exit soon.
    pub fn wait(self) -> Exit {
        self.process.wait()
    }

    pub fn send(&mut self, request: &Request) -> io::Result<()> {
        self.process.send(request)
    }

    /// The next message, blocking. [`Incoming::Eof`] once the stream ended
    /// or the worker exited, and on every call after.
    pub fn recv(&self) -> Incoming {
        self.process.recv()
    }

    /// The next message, or `None` if nothing arrives within `timeout`.
    ///
    /// Once the worker has exited, the rest of what it wrote still arrives,
    /// then [`Incoming::Eof`], even while a process its tests started holds
    /// the stream open.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        self.process.recv_timeout(timeout)
    }

    /// The end of what the worker wrote to stderr so far.
    pub fn stderr_tail(&self) -> String {
        self.process.stderr_tail()
    }

    /// Echo the worker's stderr live from now on, and where, or stop: for a
    /// worker kept from refinement for a run that doesn't capture (`-s`).
    pub fn set_echo(&self, echo: Option<Echo>) {
        self.process.set_echo(echo);
    }
}

impl Process {
    /// Whether the worker process is gone (or can't be waited for).
    fn exited(&self) -> bool {
        let mut proc = lock(&self.proc);
        proc.reaped || !matches!(proc.try_wait(), Ok(None))
    }

    fn killer(&self) -> Killer {
        Killer(Arc::clone(&self.proc))
    }

    fn kill(&self) {
        lock(&self.proc).kill();
    }

    fn shutdown(mut self) -> Exit {
        let _ = self.send(&Request::Shutdown);
        self.stdin.take();
        self.reap(SHUTDOWN_GRACE, STDERR_QUIET)
    }

    fn wait(mut self) -> Exit {
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

impl Drop for Process {
    fn drop(&mut self) {
        let polled = lock(&self.proc).try_wait();
        if matches!(polled, Ok(None)) {
            self.kill();
            let _ = self.exit_within(KILL_WAIT);
        }
    }
}
