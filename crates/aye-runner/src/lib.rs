//! Execution at scale. Interpreter discovery, the warm daemon and
//! fork-per-run, the single spawn path every backend wraps (local, sandboxed,
//! container, remote), the scheduler (affinity groups, duration-aware,
//! dynamic hand-out, serial groups, crash recovery), capability routing, the
//! sandbox policy and its enforcers, services provided into the worker, the
//! transport, and the embedded worker (`python/aye_worker.py`) that hosts
//! whichever engine executors the runner sends it.
//!
//! Owns: how tests run, where, and in what order. Does not own: what a test is
//! or how one executes inside the framework; it calls the engine for that.
//!
//! Depends on: aye-engine, aye-events, aye-static. Must not depend on any
//! `aye-engine-*`.
//!
//! Environments come from `aye-provision` (uv as a subprocess, or paths the
//! user supplies); the runner requests them per matrix row and per backend,
//! keys caches on what it gets back, and never installs anything itself.
//!
//! Native features (October 2026): the runner owns the mechanism behind
//! fakes, snapshots and test data. It ties each fake to a test's lifetime and
//! undoes it across forks and crashes, reports which fakes were active, puts
//! the seed and frozen time into the reproduce command, and decides whether a
//! test stayed hermetic. Audit hooks show a test reaching the real network,
//! filesystem or subprocesses; the clock and randomness raise no audit event,
//! so they can only be known by being provided. Snapshot files and literals
//! are only ever written by the Rust process, never by workers.
//!
//! Open: whether network record and replay and a virtual filesystem ship in
//! aye itself or as separate packages on the same mechanism (their edge-case
//! surface is large); whether existing libraries (freezegun, time-machine,
//! respx, vcrpy) count as fakes for the hermetic verdict; snapshot storage
//! (inline literals, files, or both); when snapshot values are written (only
//! on review, from the event log, or as pending files during the run); how
//! the matrix treats a snapshot that differs between Python versions; what a
//! snapshot with no value does locally and in CI; the snapshot file format;
//! snapshots under plain pytest; where the source rewriter shared by
//! snapshots, doctest accept and the migration codemod lives (`aye-collect`
//! or its own crate).
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Interpreter discovery** ([`find_python`]), first match wins:
//!   `--python`, `$AYE_PYTHON`, `[tool.aye] python`, `$VIRTUAL_ENV`, a
//!   `.venv` in the working directory then the root, the interpreter beside
//!   the `aye` binary (beside the file a link to it leads to: `uv tool
//!   install` and pipx link it into `~/.local/bin` from the tool's
//!   environment), `python3` then `python` on `PATH`. An explicit choice
//!   that is a bare name is looked up on `PATH`. Until `aye-provision`
//!   lands, this is the bring-your-own provisioner.
//! - **One spawn path.** [`spawn`] starts `python -c <aye_worker source>`
//!   ([`WORKER_SOURCE`]) in the user's working directory with its standard
//!   streams piped, sends `hello` (aye's engine, then each executor's
//!   modules once, in order, the executors and the services by name) and
//!   reads until `ready`, refusing a worker of another protocol version.
//!   Runs, refinement and later backends all go through it. Workers get
//!   `PYTHONIOENCODING=utf-8` and their own process group, so the terminal's
//!   Ctrl-C reaches only aye, which stops them through a [`StopHandle`].
//! - **Killing a worker kills its process group**: the worker and the
//!   processes its tests started, as the terminal's Ctrl-C would under
//!   pytest. std signals one process only and the workspace forbids unsafe
//!   code, so `kill -9 -<pgid>` runs through `/bin/sh` (`taskkill /T` on
//!   Windows). A group is only signalled while its worker is unreaped, so
//!   its id can't have been reused; what a worker that exited by itself left
//!   running is left alone, as pytest leaves it. A worker that outlives the
//!   grace period after `shutdown` (3s) or after its stream ended (2s), a
//!   thread the tests left running, is killed so.
//! - **Stopping.** [`StopHandle::stop`] makes the run hand out nothing more
//!   and kill its busy workers (within 50ms); [`StopHandle::kill`] kills
//!   every live worker of the runner at once, from the calling thread, for
//!   a caller about to exit (the second Ctrl-C), and any started later. A
//!   worker whose runner was killed outright finds its protocol stream
//!   broken at its next message, and kills its own process group.
//! - **The worker owns its streams.** It keeps private copies of fds 0 and 1
//!   for the protocol, points fd 1 at stderr and fd 0 at the null device, so
//!   a stray print or `input()` can neither corrupt nor consume the stream.
//!   The copies are close-on-exec and sit high (from half the soft fd limit,
//!   at most 512), out of the way of tests that use low fds (systemd's fd
//!   3). A process forked from the worker gets them pointed at the null
//!   device and its messages dropped, so it can neither hold the stream open
//!   nor add records. It leaves `sys.path` as `python -c` sets it, except
//!   that the leading `''` becomes the absolute working directory, as under
//!   `python -m pytest`; frameworks manage their own paths. What the
//!   protocol's path calls (`os.getpid`, the JSON encoder's own C
//!   functions, which `JSONEncoder.encode` would look up in `json.encoder`
//!   at every call, the decoder, `traceback.format_exc`, the stream's
//!   `write`) is bound when the worker starts, so a test that replaces one
//!   and never puts it back can neither silence the stream (a patched
//!   `os.getpid` made it drop every message as a forked child's, and the
//!   run hung) nor garble it (a patched `json.encoder.encode_basestring`
//!   garbled `batch_done`, and the run hung). Every line it
//!   writes is JSON that serde reads, whatever an executor hands it: NaN,
//!   infinities and integers beyond a double's range become `null`, keys
//!   JSON can't hold become text, and lone surrogates become U+FFFD, as aye's
//!   engine cleans text. What can't be written at all (a cycle) raises in
//!   the executor; no invalid line is written.
//! - **Transport.** Each worker line is a [`aye_engine::WorkerLine`]; a line
//!   that isn't one is reported as a `protocol-error` diagnostic and the
//!   worker carries on. A worker's death is noticed by its pid as well as by
//!   the end of its stream, which a process its tests started may hold open.
//!   stderr is drained on a thread: it goes into crash reports (a fatal
//!   error's dump from its start, the crashing thread first; otherwise the
//!   last lines, without pytest's progress letters), and with `capture:
//!   false` it is echoed live, to our stdout or, beside a machine-readable
//!   stdout, to our stderr ([`Echo`]). Once a worker has exited, the rest of
//!   its stderr is awaited for up to 1s for a crash report, but after a
//!   clean shutdown only until it has been silent for 100ms: a subprocess
//!   the tests left running holds the stream open, and would delay the end
//!   of every run.
//! - **Workers are lanes.** A run starts at most `workers` lanes, never more
//!   than there are batches; each pulls the next batch when it finishes one.
//!   A lane's worker id is its index, kept by the worker that replaces a
//!   crashed one, as are `AYE_WORKER=<n>` and, with more than one lane,
//!   `PYTEST_XDIST_WORKER=gw<n>` and `PYTEST_XDIST_WORKER_COUNT`, so
//!   per-worker resources are reused. Only the adapters a run needs are sent.
//! - **A worker serves one adapter.** Two frameworks in one process would
//!   share imported modules (two `conftest.py`, a unittest suite run with
//!   pytest's plugins loaded), so a worker holds only the executor of its
//!   first batch's adapter and runs only that adapter's batches. With
//!   several lanes, a lane asks the queue for its adapter's batches first
//!   (after those holding tests to run first); one of another adapter gets
//!   a new worker, with the lane's index, once the old one has shut down, so
//!   `workers` stays the cap on live workers. With one worker, each
//!   adapter's batch gets its own, in plan order whatever the adapter (the
//!   doctests pytest lists before a file's tests run first); a worker that
//!   listed the session waits, idle, while another adapter's runs, rather
//!   than listing it again, and is announced (`worker_started`) when it
//!   first runs a batch.
//! - **Batches.** One per affinity group, longest-first by history (10ms per
//!   unknown test); with several workers, small groups of one adapter are
//!   merged up to a quarter of a worker's share. A case without affinity is
//!   free: it goes with its file's lifetime group, if any; otherwise, with
//!   several workers, a file's free cases heavier than that share are split,
//!   in order, into chunks of about it, so a big file spreads across
//!   workers. The batches of an executor that lists the session keep plan
//!   order on every lane, as pytest-xdist's workers do: only consecutive
//!   groups merge, and they are handed out in plan order. With one worker,
//!   one batch per adapter in plan order. A batch holding a `serial`
//!   case is never handed out while another such batch runs. Tests to run
//!   first ([`Plan::first`], `--ff`) run first within their batch, in plan
//!   order, across files, and the batches holding them are handed out first.
//! - **The log.** The runner stamps `run`, `environment`, the run-wide `seq`
//!   and `t_ms` with the [`Stamper`](aye_events::Stamper) the binary also
//!   uses for its own records, keeps each worker record's `batch`, and
//!   brackets every worker and batch with started and finished records.
//! - **`maxfail` is counted across workers.** Each batch is sent with what
//!   is left of the budget. When it runs out, nothing more is handed out,
//!   and busy workers end their batches as pytest's `-x` ends a session,
//!   tearing down: a worker whose batch reached its own share ends it by
//!   itself; the others are interrupted (SIGINT, a `KeyboardInterrupt`;
//!   killed on Windows, which has no such signal) and killed if still busy
//!   after 10s, as long as pytest-xdist gives its workers to stop. Records
//!   still arriving are dropped, so the log holds exactly `maxfail` bad
//!   outcomes, except the `lifetime_finished` records that close lifetimes
//!   the log holds open, failures included (a `tearDownClass` that
//!   raised), but for the `KeyboardInterrupt` the run sent. The stop handle
//!   still kills them all at once. `maxfail` 0 is no limit, as in pytest.
//! - **A batch the framework stopped stops the run**: one whose records hold
//!   a `batch-stopped` error diagnostic (aye's pytest adapter emits it
//!   whenever pytest stopped the session, even in the batch's last test:
//!   `pytest.exit()` whatever its `returncode`, a `KeyboardInterrupt`,
//!   `session.shouldstop`, a `session.shouldfail` that isn't aye's
//!   `maxfail`; and for an internal error when tests were left), or whose
//!   status is pytest's 2. pytest ends its session there, so nothing more is
//!   handed out; batches already running finish and are reported. The run
//!   counts as interrupted, except after pytest's internal error: a batch
//!   whose status is 3 (`INTERNALERROR`) makes the run an internal error,
//!   and stops it only with `batch-stopped` (an internal error in
//!   `pytest_sessionfinish`, after every test ran, stops nothing). No other
//!   status means anything alone: `pytest.exit(returncode=4)` is no usage
//!   error; the binary finds those from `usage-error` diagnostics.
//! - **Why a run stopped** is [`RunSummary::stopped`] (`maxfail`,
//!   `interrupted` or `framework`, the first that came up), and the requested
//!   tests that never started are [`RunSummary::not_run`].
//! - **A batch the framework failed quietly**: pytest's status 1 from a
//!   batch with no bad outcome and no error diagnostic (a conftest's
//!   `session.exitstatus = 1`, pytest-cov's `--cov-fail-under`) sets
//!   [`RunSummary::framework_failed`], for the binary to fail the run, as
//!   pytest exits 1; the adapter's `session-failed` warning says why.
//! - **Crashes.** A worker that dies mid-batch fails the test it was running
//!   (`error`, "worker crashed while running this test", its stderr tail as
//!   detail); the batch's ids that never started under their own id are
//!   re-queued once, as a new batch at the front, and are reported as
//!   errors if that one crashes too. An id that stands for several tests (a
//!   case without its parameters, or pytest's stand-ins for parameters whose
//!   ids changed) may have run some: it is re-queued whole, and the batch's
//!   context gets `ran_ids`, the tests that ran, which the adapter doesn't
//!   run again; the pytest adapter honours it, and adapters whose ids are
//!   always exact need not. A worker dying outside any test is an internal
//!   error.
//! - **An adapter that raises** outside any test (`failed`) ends its batch
//!   with a `batch-failed` diagnostic and an internal error; its unfinished
//!   tests are not reported, and the worker goes on with the next batch.
//! - **Refinement** ([`Runner`] implements [`aye_engine::Refiner`]) starts a
//!   fresh worker holding only that adapter's executor, and shuts it down,
//!   unless the executor lists the session.
//! - **Session listing** ([`aye_engine::Executor::lists_session`]). The
//!   worker that refined is kept, with the first lane's environment, for the
//!   next refinement of its adapter and then as the next run's first lane
//!   (shut down when the runner is dropped, or when the run gives that lane
//!   another environment). With several workers and a run to follow (not
//!   [`RunnerOptions::collect_only`]), a worker for each other lane starts
//!   as refinement does, in the background, with its lane's environment,
//!   and lists the files refined (digests only) while the first lane's
//!   worker refines them; the run's lanes take them (they are shut down as
//!   the lane ends unused, when the run gives their lanes other
//!   environments, or when the runner is dropped; killed once it is
//!   stopped). Every other worker of the adapter lists the session
//!   ([`Plan::session_files`], digests only) as it starts, before its first
//!   batch, in parallel across lanes, as does a crashed worker's
//!   replacement, and one started during refinement lists what it hasn't
//!   yet; a worker that can't is a worker that failed to start. The first
//!   lane lists what it hasn't yet, and with several lanes, its
//!   listing is what the others compare theirs with, file by file: a file
//!   listed otherwise runs on the first lane only (the lane hands its ids
//!   back, pinned to the first lane, which doesn't finish while a lane may
//!   still pin some; once it has, a lane runs them itself), with one
//!   `listing-differs` warning naming it. Echo (`-s`) is switched on for
//!   the kept worker when the run takes it.
//! - **A worker's session ends in a batch** ([`aye_engine::Batch::ends_session`]),
//!   so that what fails there is reported and its teardown isn't cut short
//!   by the shutdown's grace: the batch a lane takes when nothing else of
//!   its adapter is left for it (for the first lane, once no lane may still
//!   pin tests to it) ends it, as a pytest-xdist worker's session ends with
//!   its last test; a lane whose worker's session is still open when it
//!   finishes or leaves the adapter sends a batch with no ids that only
//!   ends it (`Queue::closing`), also after `maxfail` ran out (as
//!   pytest's `-x` tears down), but not once the run is stopped. A worker
//!   whose session ended runs no other batch: a batch that comes back to
//!   its lane (a crashed one's) gets a new worker.

mod python;
mod run;
mod schedule;
mod worker;

pub use python::{PythonSource, find_python, venv_python};
pub use run::{
    BatchEnd, BatchSummary, Crash, Plan, RunSummary, Runner, RunnerOptions, StopHandle, StopReason,
};
pub use schedule::DEFAULT_WEIGHT;
pub use worker::{
    DEFAULT_SERVICES, Echo, Exit, Incoming, Killer, SpawnError, Worker, WorkerSpec, describe,
    parse_line, spawn,
};

/// The worker, embedded at build time and run with `python -c`.
pub const WORKER_SOURCE: &str = include_str!("../python/aye_worker.py");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_speaks_this_protocol() {
        let line = format!("\nPROTOCOL = {}\n", aye_engine::PROTOCOL_VERSION);
        assert!(WORKER_SOURCE.contains(&line));
    }

    /// Windows caps a command line at 32767 characters, and quoting can
    /// double the source's length.
    #[test]
    fn the_worker_fits_on_a_windows_command_line() {
        assert!(WORKER_SOURCE.len() < 16_000, "{}", WORKER_SOURCE.len());
    }
}
