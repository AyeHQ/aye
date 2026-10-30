//! The spawn path: a worker's process started, then its handshake.

use std::collections::VecDeque;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use aye_protocol::{PROTOCOL_VERSION, Reply, WorkerLine};
use aye_static::EnvVars;

use crate::WORKER_SOURCE;

use super::process::{Proc, own_process_group};
use super::stderr::Stderr;
use super::timeouts::{EXIT_GRACE, STDERR_GRACE};
use super::transport::{is_reply, read_lines};
use super::{Incoming, Worker, WorkerSpec};

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

impl Worker {
    /// Start the process, without the handshake, so that the caller can
    /// arrange to kill it before anything blocks.
    pub fn launch(spec: &WorkerSpec) -> Result<Self, SpawnError> {
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
        let pid = child.id();
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

        Ok(Self {
            pid,
            proc,
            stdin,
            lines,
            stderr,
            hello: Some(spec.hello()),
            python: String::new(),
            version: String::new(),
        })
    }

    /// Send the handshake and read until the worker is ready. Lines that
    /// aren't protocol messages (a `sitecustomize` printing at startup) are
    /// skipped and kept for the error.
    pub fn handshake(&mut self) -> Result<(), SpawnError> {
        if let Some(hello) = self.hello.take() {
            // A worker that died at once shows up below as an exit.
            let _ = self.send(&hello);
        }
        let mut stray = VecDeque::new();
        loop {
            match self.recv() {
                Incoming::Line(WorkerLine::Reply(Reply::Ready {
                    protocol,
                    pid,
                    python,
                    version,
                })) => {
                    if protocol != PROTOCOL_VERSION {
                        self.kill();
                        return Err(SpawnError::Protocol {
                            worker: protocol,
                            runner: PROTOCOL_VERSION,
                        });
                    }
                    self.pid = pid;
                    self.python = python;
                    self.version = version;
                    return Ok(());
                }
                Incoming::Line(WorkerLine::Reply(Reply::Failed { message })) => {
                    self.kill();
                    return Err(SpawnError::Hello(message));
                }
                Incoming::Line(_) => {}
                Incoming::Garbage { line, error } if is_reply(&line) => {
                    self.kill();
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
                    let exit = self.reap(EXIT_GRACE, STDERR_GRACE);
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
}
