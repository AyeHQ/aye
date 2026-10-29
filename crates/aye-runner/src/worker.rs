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

use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use aye_engine::{
    ENGINE_MODULE, Executor, ExecutorSpec, ModuleSource, PROTOCOL_VERSION, Reply, Request,
    WorkerLine, services,
};

use aye_static::EnvVars;

use crate::WORKER_SOURCE;

/// How long a worker may take to exit after `shutdown` before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// How long a worker may take to exit once its protocol stream has ended.
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for a killed worker to be gone.
const KILL_WAIT: Duration = Duration::from_secs(5);
/// How long to wait for the rest of stderr once the worker has exited.
/// Bounded, because a subprocess the tests left behind may hold it open.
const STDERR_GRACE: Duration = Duration::from_secs(1);
/// After a clean `shutdown`, how long stderr may stay silent before it
/// counts as ended: nothing goes into a report then, and a subprocess the
/// tests left running, which holds the stream open, would otherwise delay
/// the end of every run by [`STDERR_GRACE`]. A crash report waits for the
/// whole grace instead, so as not to lose the end of the crash's dump.
const STDERR_QUIET: Duration = Duration::from_millis(100);
/// How often a blocked read checks whether the worker is still alive.
const EXIT_POLL: Duration = Duration::from_millis(100);
/// Once the worker has exited, how long its stream may stay silent before it
/// counts as ended. What is left in the pipe arrives at once, and the stream
/// ends by itself unless a process the tests started holds the pipe open,
/// which may be forever. Generous, so that a reader thread starved of CPU
/// doesn't lose the worker's last lines.
const DRAIN: Duration = Duration::from_secs(1);
/// Bytes of stderr kept for crash reports.
const STDERR_KEEP: usize = 64 * 1024;
/// Lines and bytes of stderr put into a report.
const TAIL_LINES: usize = 30;
const TAIL_BYTES: usize = 8000;
/// Stray stdout lines kept from before the handshake, for the error.
const STRAY_LINES: usize = 20;
/// How faulthandler (and `Py_FatalError`) starts a dump.
const FATAL: &str = "Fatal Python error: ";

/// The services a worker provides unless told otherwise.
pub const DEFAULT_SERVICES: [&str; 2] = [services::CONTEXT, services::CLEANUP];

/// Where a worker's output is echoed when it isn't captured (`-s`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Echo {
    /// Our stdout, beside the text report.
    #[default]
    Stdout,
    /// Our stderr, so that a machine-readable stdout (`--output json`)
    /// stays one record per line.
    Stderr,
}

/// How to start a worker.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// The interpreter.
    pub python: PathBuf,
    /// The project root, absolute; sent in the handshake.
    pub root: PathBuf,
    /// The directory the worker starts in.
    pub cwd: PathBuf,
    /// The executors the worker instantiates. Their modules are installed
    /// after aye's engine, in order, each name once.
    pub executors: Vec<Executor>,
    /// Services to provide, by name.
    pub services: Vec<String>,
    /// Environment variables set on top of the inherited environment, after
    /// `PYTHONIOENCODING=utf-8`.
    pub env: Vec<(OsString, OsString)>,
    /// Echo the worker's stderr (where its stdout goes too) live, as with
    /// `-s`, and where to. It is kept for crash reports either way.
    pub echo: Option<Echo>,
    /// The runner's live workers, which this one joins.
    pub(crate) registry: Option<Registry>,
}

impl WorkerSpec {
    pub fn new(
        python: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            python: python.into(),
            root: root.into(),
            cwd: cwd.into(),
            executors: Vec::new(),
            services: DEFAULT_SERVICES.iter().map(ToString::to_string).collect(),
            env: Vec::new(),
            echo: None,
            registry: None,
        }
    }

    /// The handshake this spec sends.
    pub fn hello(&self) -> Request {
        let mut seen = HashSet::new();
        let modules = std::iter::once(&ENGINE_MODULE)
            .chain(self.executors.iter().flat_map(|e| e.modules))
            .filter(|module| seen.insert(module.name))
            .map(|module| ModuleSource::from(*module))
            .collect();
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            root: self.root.to_string_lossy().into_owned(),
            modules,
            executors: self
                .executors
                .iter()
                .map(|e| ExecutorSpec {
                    adapter: e.adapter.clone(),
                    entry: e.entry.to_string(),
                    lists_session: e.lists_session,
                })
                .collect(),
            services: self.services.clone(),
        }
    }
}

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

/// One thing read from a worker's protocol stream.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Line(WorkerLine),
    /// A line that is not a protocol message.
    Garbage {
        line: String,
        error: String,
    },
    /// The stream ended: the worker exited or closed it.
    Eof,
}

/// The worker's process. Everything that reaps it goes through here, so its
/// process group is only ever signalled while its pid can't have been
/// reused: until it is reaped, a dead worker's pid stays taken.
#[derive(Debug)]
struct Proc {
    child: Child,
    reaped: bool,
}

impl Proc {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let polled = self.child.try_wait();
        if matches!(polled, Ok(Some(_))) {
            self.reaped = true;
        }
        polled
    }

    /// Kill the worker and its process group, unless it was reaped already
    /// (it exited by itself; what its tests left running is theirs).
    fn kill(&mut self) {
        if !self.reaped {
            kill_groups(&[self.child.id()]);
            let _ = self.child.kill();
        }
    }

    /// Interrupt the worker alone, as Ctrl-C interrupts pytest: SIGINT, a
    /// `KeyboardInterrupt` in Python, which ends its batch, tearing down.
    /// Where there is no such signal (Windows), kill it.
    fn interrupt(&mut self) {
        if self.reaped {
            return;
        }
        #[cfg(unix)]
        {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "kill -INT \"$1\"", "sh", &self.child.id().to_string()]);
            quietly(&mut command);
        }
        #[cfg(not(unix))]
        self.kill();
    }
}

/// A handle that kills or interrupts a worker from any thread.
#[derive(Debug, Clone)]
pub struct Killer(Arc<Mutex<Proc>>);

impl Killer {
    /// Kill the worker and its process group.
    pub fn kill(&self) {
        lock(&self.0).kill();
    }

    /// Interrupt the worker alone (SIGINT), so that its framework ends the
    /// batch and tears down; kill it where there is no such signal.
    pub fn interrupt(&self) {
        lock(&self.0).interrupt();
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
    fn join(&self, proc: &Arc<Mutex<Proc>>) {
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

/// Kill each worker's process group: the worker and whatever its tests
/// started that stayed in the group (a worker leads its own, see
/// [`own_process_group`]). std can only signal a single process and the
/// workspace forbids the unsafe code `killpg` takes, so the shell's `kill`
/// sends the signals. On Windows, `taskkill /T` ends each worker's process
/// tree. The worker itself is killed directly as well, in case this fails.
fn kill_groups(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "kill -9 \"$@\"", "sh"])
            .args(pids.iter().map(|pid| format!("-{pid}")));
        quietly(&mut command);
    }
    #[cfg(windows)]
    for pid in pids {
        let mut command = Command::new("taskkill");
        command.args(["/F", "/T", "/PID", &pid.to_string()]);
        quietly(&mut command);
    }
}

#[cfg(any(unix, windows))]
fn quietly(command: &mut Command) {
    let _ = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// How a worker ended.
#[derive(Debug, Clone)]
pub struct Exit {
    /// `None` when it couldn't be waited for.
    pub status: Option<ExitStatus>,
    /// The end of its stderr.
    pub stderr: String,
}

impl Exit {
    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|status| status.code())
    }

    /// `exit status 3`, `killed by signal 11 (SIGSEGV)`.
    pub fn describe(&self) -> String {
        describe(self.status)
    }
}

/// What polling for a worker's exit found.
enum Waited {
    Exited(ExitStatus),
    Running,
    /// It can't be waited for.
    Unknown,
}

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

    pub fn send(&mut self, request: &Request) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "stdin is closed"))?;
        let mut line = serde_json::to_vec(request)?;
        line.push(b'\n');
        stdin.write_all(&line)?;
        stdin.flush()
    }

    /// The next message, blocking. [`Incoming::Eof`] once the stream ended
    /// or the worker exited, and on every call after.
    pub fn recv(&self) -> Incoming {
        loop {
            if let Some(incoming) = self.recv_timeout(EXIT_POLL) {
                return incoming;
            }
        }
    }

    /// The next message, or `None` if nothing arrives within `timeout`.
    ///
    /// Once the worker has exited, the rest of what it wrote still arrives,
    /// then [`Incoming::Eof`], even while a process its tests started holds
    /// the stream open.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Incoming> {
        match self.lines.recv_timeout(timeout) {
            Ok(incoming) => Some(incoming),
            Err(RecvTimeoutError::Disconnected) => Some(Incoming::Eof),
            Err(RecvTimeoutError::Timeout) => self
                .exited()
                .then(|| self.lines.recv_timeout(DRAIN).unwrap_or(Incoming::Eof)),
        }
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

    /// The end of what the worker wrote to stderr so far.
    pub fn stderr_tail(&self) -> String {
        self.stderr.tail(Duration::ZERO, Duration::ZERO)
    }

    /// Echo the worker's stderr live from now on, and where, or stop: for a
    /// worker kept from refinement for a run that doesn't capture (`-s`).
    pub fn set_echo(&self, echo: Option<Echo>) {
        lock(&self.stderr.0.0).echo = echo;
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

/// Parse a worker's stdout into messages until it ends.
fn read_lines(stdout: impl Read, tx: &Sender<Incoming>) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let text = String::from_utf8_lossy(&buf);
                let text = text.trim_end_matches(['\n', '\r']);
                if text.trim().is_empty() {
                    continue;
                }
                if tx.send(parse_line(text)).is_err() {
                    return;
                }
            }
        }
    }
    let _ = tx.send(Incoming::Eof);
}

/// One protocol line, or garbage with the reason.
pub fn parse_line(text: &str) -> Incoming {
    match serde_json::from_str::<WorkerLine>(text) {
        Ok(line) => Incoming::Line(line),
        Err(error) => Incoming::Garbage {
            line: text.to_string(),
            error: error.to_string(),
        },
    }
}

/// Whether a line that isn't a valid message was meant as a reply.
pub(crate) fn is_reply(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| value.get("reply").is_some())
}

/// Workers get their own process group, so the terminal's Ctrl-C reaches
/// only aye, which then stops them (see `StopHandle`), and so that killing
/// a worker can kill what its tests started (see [`kill_groups`]).
fn own_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

/// A worker's stderr, drained on a thread.
#[derive(Debug, Clone)]
struct Stderr(Arc<(Mutex<Tail>, Condvar)>);

#[derive(Debug, Default)]
struct Tail {
    bytes: Vec<u8>,
    /// Where what is taken in is echoed, unless the reader was given its
    /// own writer.
    echo: Option<Echo>,
    done: bool,
    /// When the last bytes were taken in.
    last: Option<Instant>,
    /// Bytes are being taken in (echoed) right now.
    busy: bool,
}

impl Stderr {
    fn drain(stream: impl Read + Send + 'static, echo: Option<Echo>) -> Self {
        let stderr = Self(Arc::default());
        lock(&stderr.0.0).echo = echo;
        let shared = stderr.clone();
        thread::spawn(move || shared.read(stream, None::<io::Sink>));
        stderr
    }

    /// Take the stream in until it ends, echoing it to `to` if given, else
    /// where [`Tail::echo`] says at the time.
    fn read(&self, mut stream: impl Read, mut to: Option<impl Write>) {
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let echo = {
                        let mut tail = lock(&self.0.0);
                        tail.busy = true;
                        tail.echo
                    };
                    let chunk = &buf[..n];
                    match (to.as_mut(), echo) {
                        (Some(out), _) => write_flushed(out, chunk),
                        (None, Some(Echo::Stdout)) => write_flushed(&mut io::stdout(), chunk),
                        (None, Some(Echo::Stderr)) => write_flushed(&mut io::stderr(), chunk),
                        (None, None) => {}
                    }
                    let mut tail = lock(&self.0.0);
                    tail.bytes.extend_from_slice(&buf[..n]);
                    if tail.bytes.len() > 2 * STDERR_KEEP {
                        let cut = tail.bytes.len() - STDERR_KEEP;
                        tail.bytes.drain(..cut);
                    }
                    tail.busy = false;
                    tail.last = Some(Instant::now());
                    drop(tail);
                    self.0.1.notify_all();
                }
            }
        }
        lock(&self.0.0).done = true;
        self.0.1.notify_all();
    }

    /// What goes into a report, once the worker has exited: waits up to
    /// `wait` for the stream to end first, but no longer than it stays
    /// silent for `quiet` (a process the tests started may hold it open for
    /// good). See [`report`].
    fn tail(&self, wait: Duration, quiet: Duration) -> String {
        let (tail, changed) = &*self.0;
        let start = Instant::now();
        let deadline = start + wait;
        let mut guard = lock(tail);
        loop {
            let silent_since = guard.last.map_or(start, |last| last.max(start));
            let until = if guard.busy {
                deadline
            } else {
                (silent_since + quiet).min(deadline)
            };
            let now = Instant::now();
            if guard.done || now >= until {
                break;
            }
            guard = changed
                .wait_timeout(guard, until - now)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(guard, _)| guard);
        }
        report(&String::from_utf8_lossy(&guard.bytes))
    }
}

fn write_flushed(out: &mut impl Write, bytes: &[u8]) {
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

/// The part of a worker's stderr that goes into a report, at most
/// [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
///
/// A fatal error's dump (faulthandler's, on a crash) prints the innermost
/// frame first, so the last dump is kept from its start, the crashing
/// thread's stack first. Otherwise the end is kept, without the lines that
/// are only pytest's progress letters (its terminal output goes to stderr
/// too), which say nothing about a crash.
fn report(text: &str) -> String {
    if let Some(at) = text.rfind(FATAL) {
        return head(&crashing_thread_first(&text[at..]));
    }
    let lines: Vec<&str> = text.lines().filter(|line| !is_progress(line)).collect();
    last(&lines.join("\n"))
}

/// pytest's progress letters, as a quiet terminal reporter prints them.
fn is_progress(line: &str) -> bool {
    let line = line.trim_end();
    !line.is_empty()
        && line
            .chars()
            .all(|c| matches!(c, '.' | 's' | 'x' | 'X' | 'F' | 'E'))
}

/// A fatal error's dump with the crashing thread's stack (`Current thread
/// …`) right after the header, before the other threads'.
fn crashing_thread_first(dump: &str) -> String {
    let lines: Vec<&str> = dump.lines().collect();
    let is_thread =
        |line: &&str| line.starts_with("Current thread ") || line.starts_with("Thread 0x");
    let Some(current) = lines.iter().position(|l| l.starts_with("Current thread ")) else {
        return dump.to_string();
    };
    let threads = lines.iter().position(is_thread).unwrap_or(current);
    let end = lines[current..]
        .iter()
        .position(|line| line.trim().is_empty())
        .map_or(lines.len(), |n| current + n);
    let mut ordered = lines[..threads].to_vec();
    ordered.extend_from_slice(&lines[current..end]);
    ordered.extend_from_slice(&lines[threads..current]);
    ordered.extend_from_slice(&lines[end..]);
    ordered.join("\n")
}

/// The start of `text`: at most [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
fn head(text: &str) -> String {
    let text = text.trim();
    let mut end = text.len().min(TAIL_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lines: Vec<&str> = text[..end].lines().take(TAIL_LINES).collect();
    lines.join("\n")
}

/// The end of `text`: at most [`TAIL_LINES`] lines and [`TAIL_BYTES`] bytes.
fn last(text: &str) -> String {
    let text = text.trim_end();
    let mut start = text.len().saturating_sub(TAIL_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let text = &text[start..];
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// `exit status 3`, `killed by signal 11 (SIGSEGV)`.
pub fn describe(status: Option<ExitStatus>) -> String {
    let Some(status) = status else {
        return "exit status unknown".to_string();
    };
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return match signal_name(signal) {
                Some(name) => format!("killed by signal {signal} ({name})"),
                None => format!("killed by signal {signal}"),
            };
        }
    }
    status.to_string()
}

/// Names of the signals whose numbers POSIX fixes.
#[cfg_attr(not(unix), allow(dead_code))]
fn signal_name(signal: i32) -> Option<&'static str> {
    Some(match signal {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return None,
    })
}

impl fmt::Display for Incoming {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Line(line) => match serde_json::to_string(line) {
                Ok(text) => f.write_str(&text),
                Err(_) => write!(f, "{line:?}"),
            },
            Self::Garbage { line, .. } => f.write_str(line),
            Self::Eof => f.write_str("end of stream"),
        }
    }
}

/// Lock, ignoring poisoning: a panicked reader thread leaves valid data.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use aye_engine::EmbeddedModule;
    use aye_events::{AdapterName, Event};

    use super::*;

    const ONE: EmbeddedModule = EmbeddedModule {
        name: "one",
        source: "",
    };
    const TWO: EmbeddedModule = EmbeddedModule {
        name: "two",
        source: "",
    };

    #[test]
    fn hello_installs_the_engine_then_each_module_once() {
        let mut spec = WorkerSpec::new("python", "/repo", "/repo");
        spec.executors = vec![
            Executor {
                adapter: AdapterName::new_static("a"),
                modules: &[ONE, TWO],
                entry: "two:A",
                lists_session: false,
            },
            Executor {
                adapter: AdapterName::new_static("b"),
                modules: &[TWO, ENGINE_MODULE],
                entry: "two:B",
                lists_session: true,
            },
        ];
        let Request::Hello {
            protocol,
            root,
            modules,
            executors,
            services,
        } = spec.hello()
        else {
            panic!("expected hello");
        };
        assert_eq!(protocol, PROTOCOL_VERSION);
        assert_eq!(root, "/repo");
        let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["aye_engine", "one", "two"]);
        assert_eq!(executors.len(), 2);
        assert_eq!(executors[1].entry, "two:B");
        assert_eq!(
            executors
                .iter()
                .map(|e| e.lists_session)
                .collect::<Vec<_>>(),
            [false, true]
        );
        assert_eq!(services, ["context", "cleanup"]);
    }

    #[test]
    fn lines_are_messages_or_garbage() {
        let (tx, rx) = mpsc::channel();
        let input = concat!(
            "{\"reply\":\"batch_done\",\"batch\":2}\n",
            "\n",
            "not json\r\n",
            "{\"batch\":2,\"event\":\"test_started\",\"id\":\"t.py::test_a\"}\n",
            "{\"no\":\"event\"}",
        );
        read_lines(input.as_bytes(), &tx);
        let got: Vec<Incoming> = rx.try_iter().collect();
        assert_eq!(got.len(), 5, "{got:?}");
        assert_eq!(
            got[0],
            Incoming::Line(WorkerLine::Reply(Reply::BatchDone {
                batch: 2,
                status: None
            }))
        );
        assert!(matches!(&got[1], Incoming::Garbage { line, .. } if line == "not json"));
        let Incoming::Line(WorkerLine::Record(record)) = &got[2] else {
            panic!("expected a record: {:?}", got[2]);
        };
        assert_eq!(record.envelope.batch, Some(2));
        assert!(matches!(record.event, Event::TestStarted { .. }));
        assert!(matches!(&got[3], Incoming::Garbage { .. }));
        assert_eq!(got[4], Incoming::Eof);
    }

    #[test]
    fn replies_are_told_from_other_lines() {
        assert!(is_reply(r#"{"reply":"batch_done","batch":"x"}"#));
        assert!(!is_reply(r#"{"event":"test_finished"}"#));
        assert!(!is_reply("reply"));
    }

    #[test]
    fn stderr_is_echoed_and_its_tail_kept() {
        let stderr = Stderr(Arc::default());
        let mut echoed = Vec::new();
        let mut text = String::new();
        for i in 1..=100 {
            writeln!(text, "line {i}").unwrap();
        }
        stderr.read(text.as_bytes(), Some(&mut echoed));
        assert_eq!(echoed, text.as_bytes());
        let tail = stderr.tail(Duration::ZERO, Duration::ZERO);
        assert!(tail.starts_with("line 71\n"), "{tail}");
        assert!(tail.ends_with("line 100"), "{tail}");
    }

    #[test]
    fn the_kept_stderr_is_bounded() {
        let stderr = Stderr(Arc::default());
        let big = "é".repeat(3 * STDERR_KEEP);
        stderr.read(big.as_bytes(), None::<Vec<u8>>);
        assert!(lock(&stderr.0.0).bytes.len() <= 2 * STDERR_KEEP);
        let tail = stderr.tail(Duration::ZERO, Duration::ZERO);
        assert!(tail.len() <= TAIL_BYTES && tail.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_crash_report_keeps_the_start_of_the_fatal_error_dump() {
        let mut text = String::from("early noise\n..F.Fatal Python error: Segmentation fault\n\n");
        text.push_str(
            "Thread 0x0a (most recent call first):\n  File \"helper.py\", line 3 in loop\n\n",
        );
        text.push_str("Current thread 0x0b (most recent call first):\n");
        text.push_str("  File \"ctypes/__init__.py\", line 541 in string_at\n");
        text.push_str("  File \"test_segv.py\", line 8 in test_segv\n");
        for i in 0..40 {
            writeln!(
                text,
                "  File \"pluggy/_callers.py\", line {i} in _multicall"
            )
            .unwrap();
        }
        text.push_str("\nExtension modules: _ctypes (total: 1)\n");
        let report = report(&text);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(
            lines[0], "Fatal Python error: Segmentation fault",
            "{report}"
        );
        assert_eq!(lines[2], "Current thread 0x0b (most recent call first):");
        assert!(lines[3].contains("string_at") && lines[4].contains("test_segv.py\", line 8"));
        assert!(lines.len() <= TAIL_LINES, "{report}");
        assert!(!report.contains("early noise") && !report.contains("Thread 0x0a"));
    }

    #[test]
    fn a_crash_report_skips_pytest_progress_letters() {
        assert_eq!(report("."), "");
        assert_eq!(report("..F\nabout to crash\n.s"), "about to crash");
        // Text that merely contains those letters stays.
        assert_eq!(report("Fix.\n"), "Fix.");
    }

    #[test]
    fn a_dump_without_a_current_thread_is_kept_as_is() {
        let report = report("Fatal Python error: Aborted\n\nStack (most recent call first):\n  x");
        assert_eq!(
            report,
            "Fatal Python error: Aborted\n\nStack (most recent call first):\n  x"
        );
    }

    #[test]
    fn tail_waits_for_the_stream_to_end_but_not_forever() {
        let stderr = Stderr(Arc::default());
        let started = Instant::now();
        let wait = Duration::from_millis(30);
        assert_eq!(stderr.tail(wait, wait), "");
        assert!(started.elapsed() >= wait);
    }

    #[test]
    fn tail_stops_waiting_once_the_stream_is_quiet() {
        let stderr = Stderr(Arc::default());
        stderr.read("about to exit\n".as_bytes(), None::<Vec<u8>>);
        lock(&stderr.0.0).done = false; // a subprocess holds it open
        let started = Instant::now();
        let tail = stderr.tail(Duration::from_secs(10), Duration::from_millis(30));
        let elapsed = started.elapsed();
        assert_eq!(tail, "about to exit");
        assert!(
            elapsed >= Duration::from_millis(30) && elapsed < Duration::from_secs(5),
            "{elapsed:?}"
        );
    }

    #[test]
    fn tail_waits_while_bytes_are_being_taken_in_however_quiet() {
        let stderr = Stderr(Arc::default());
        lock(&stderr.0.0).busy = true; // echoing to a slow terminal
        let reader = stderr.clone();
        let echo = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            reader.read("the last line\n".as_bytes(), None::<Vec<u8>>);
        });
        let started = Instant::now();
        let tail = stderr.tail(Duration::from_secs(10), Duration::from_millis(10));
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert_eq!(tail, "the last line");
        echo.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn exit_statuses_are_described() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            describe(Some(ExitStatus::from_raw(3 << 8))),
            "exit status 3"
        );
        assert_eq!(
            describe(Some(ExitStatus::from_raw(11))),
            "killed by signal 11 (SIGSEGV)"
        );
        assert_eq!(
            describe(Some(ExitStatus::from_raw(31))),
            "killed by signal 31"
        );
        assert_eq!(describe(None), "exit status unknown");
    }
}
