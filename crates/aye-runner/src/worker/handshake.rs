//! The spawn path: a worker's process started ([`LaunchedWorker`]), then
//! its handshake, which makes it a [`Worker`].

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use aye_protocol::{PROTOCOL_VERSION, Reply, Request, WorkerLine};
use aye_static::EnvVars;

use crate::WORKER_SOURCE;

use super::process::{Proc, own_process_group};
use super::stderr::Stderr;
use super::timeouts::{EXIT_GRACE, STDERR_GRACE};
use super::transport::{is_reply, read_lines};
use super::{Exit, Incoming, Killer, Process, Worker, WorkerSpec};

/// Stray stdout lines kept from before the handshake, for the error.
const STRAY_LINES: usize = 20;

#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("could not start the Python interpreter {}: {source}", python.display())]
    Start { python: PathBuf, source: io::Error },
    #[error(
        "the worker speaks protocol {worker}, but this aye speaks protocol {runner}; \
         the worker does not come from this aye"
    )]
    Protocol { worker: u32, runner: u32 },
    /// The worker answered the handshake with `failed`.
    #[error("the worker failed to start:\n{0}")]
    Hello(String),
    #[error(
        "the worker exited before the handshake ({status}){}",
        output_suffix(output)
    )]
    Exited { status: String, output: String },
}

fn output_suffix(output: &str) -> String {
    if output.is_empty() {
        String::new()
    } else {
        format!(":\n{output}")
    }
}

/// A worker whose process started and which hasn't answered the handshake:
/// it takes no request until [`LaunchedWorker::handshake`] makes it a
/// [`Worker`], but it can be killed (from any thread, by its [`Killer`]),
/// read, asked to exit or waited for.
#[derive(Debug)]
pub struct LaunchedWorker {
    process: Process,
    /// The handshake its spec says, sent by [`LaunchedWorker::handshake`].
    hello: Request,
}

impl Worker {
    /// Start the process, without the handshake, so that the caller can
    /// arrange to kill it before anything blocks.
    pub fn launch(spec: &WorkerSpec) -> Result<LaunchedWorker, SpawnError> {
        let mut command = Command::new(&spec.python);
        command
            .arg("-c")
            .arg(WORKER_SOURCE)
            .current_dir(&spec.cwd)
            .env(EnvVars::PYTHONIOENCODING, "utf-8")
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        own_process_group(&mut command);
        let mut child = command.spawn().map_err(|source| SpawnError::Start {
            python: spec.python.clone(),
            source,
        })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let proc = Arc::new(Mutex::new(Proc {
            child,
            reaped: false,
        }));
        if let Some(registry) = &spec.registry {
            registry.join(&proc);
        }

        let (tx, lines) = mpsc::channel();
        thread::spawn(move || read_lines(stdout, &tx));
        let stderr = Stderr::drain(stderr, spec.echo);

        Ok(LaunchedWorker {
            process: Process {
                proc,
                stdin,
                lines,
                stderr,
            },
            hello: spec.hello(),
        })
    }
}

impl LaunchedWorker {
    pub fn killer(&self) -> Killer {
        self.process.killer()
    }

    /// Kill the worker and its process group.
    pub fn kill(&self) {
        self.process.kill();
    }

    /// Send the handshake and read until the worker is ready: the worker,
    /// with the interpreter it reports. Lines that aren't protocol messages
    /// (a `sitecustomize` printing at startup) are skipped and kept for the
    /// error. A worker that refuses is killed.
    pub fn handshake(self) -> Result<Worker, SpawnError> {
        let Self { mut process, hello } = self;
        // A worker that died at once shows up below as an exit.
        let _ = process.send(&hello);
        let mut stray = VecDeque::new();
        loop {
            match process.recv() {
                Incoming::Line(WorkerLine::Reply(Reply::Ready {
                    protocol,
                    pid,
                    python,
                    version,
                })) => {
                    if protocol != PROTOCOL_VERSION {
                        process.kill();
                        return Err(SpawnError::Protocol {
                            worker: protocol,
                            runner: PROTOCOL_VERSION,
                        });
                    }
                    return Ok(Worker {
                        process,
                        pid,
                        python,
                        version,
                    });
                }
                Incoming::Line(WorkerLine::Reply(Reply::Failed { message })) => {
                    process.kill();
                    return Err(SpawnError::Hello(message));
                }
                Incoming::Line(_) => {}
                Incoming::Garbage { line, error } if is_reply(&line) => {
                    process.kill();
                    return Err(SpawnError::Hello(format!(
                        "unreadable answer to the handshake ({error}): {line}"
                    )));
                }
                Incoming::Garbage { line, .. } => {
                    if stray.len() == STRAY_LINES {
                        stray.pop_front();
                    }
                    stray.push_back(line);
                }
                Incoming::Eof => {
                    let exit = process.reap(EXIT_GRACE, STDERR_GRACE);
                    let mut output: Vec<String> = stray.into_iter().collect();
                    if !exit.stderr.is_empty() {
                        output.push(exit.stderr.clone());
                    }
                    return Err(SpawnError::Exited {
                        status: exit.describe(),
                        output: output.join("\n"),
                    });
                }
            }
        }
    }

    /// What the worker writes before the handshake: lines that aren't
    /// protocol messages, then [`Incoming::Eof`] once it is gone. See
    /// [`Worker::recv`].
    pub fn recv(&self) -> Incoming {
        self.process.recv()
    }

    /// Ask the worker to exit before its handshake, and wait for it, as
    /// [`Worker::shutdown`] does.
    pub fn shutdown(self) -> Exit {
        self.process.shutdown()
    }

    /// Wait for the worker, as [`Worker::wait`] does.
    pub fn wait(self) -> Exit {
        self.process.wait()
    }
}
