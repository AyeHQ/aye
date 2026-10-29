//! The runner against real worker processes, with a fake adapter whose test
//! names say what each test does. Skipped when no Python is available: they
//! use `.venv` from `uv sync`, or `AYE_TEST_PYTHON`.

#![expect(
    clippy::iter_over_hash_type,
    reason = "each worker's calls are checked alone, in any order"
)]

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use aye_engine::{EmbeddedModule, Executor, Policy, RefineRequest, Refiner, Reply, Request};
use aye_events::{Case, Event, Outcome, ProjectPath, Record, TestId, TestResult};
#[cfg(unix)]
use aye_runner::Worker;
use aye_runner::{
    BatchEnd, Echo, Incoming, Plan, RunSummary, Runner, RunnerOptions, SpawnError, Stamper,
    StopReason, WorkerSpec, spawn,
};

/// `test_fail` fails, `test_sleep_<ms>` sleeps, `test_crash` kills the
/// worker, `test_crash_once` does so once per directory, `test_env` skips
/// with the worker variables as its reason, `test_print` prints,
/// `test_garbage` writes a non-protocol line onto the protocol stream,
/// `test_spawn` starts a subprocess that sleeps (its pid in
/// `<name>.pid`), `test_thread` leaves a thread running, `test_exec`
/// replaces the worker with `sleep`, `test_hold` crashes while a subprocess
/// holds the protocol stream open, `test_signal` lets `test_await` go on,
/// `test_deaf` ignores SIGINT from then on. A
/// batch holding `die_on_load` kills the worker before any test, one holding
/// `load_raises` raises outside any test, `test_exit_session[_<status>]`
/// ends its batch there as pytest's `pytest.exit` does (status 2 by default),
/// after running the tests before it, with a `batch-stopped` error naming
/// the tests left, as aye's pytest adapter reports it, a batch holding
/// `test_status_<status>` runs every test, then ends with that status and
/// no diagnostic, and one holding `test_odd_record` first writes a record
/// holding what JSON or serde can't, past aye's engine's cleaning.
/// `test_whoami` skips with its worker's pid and executors as its reason,
/// `test_leak` replaces `os.getpid` and `json.dumps` for good, and
/// `test_expand` stands for five tests, `test_expand[0]` to `[4]` (but
/// those its context's `ran_ids` name), the third of which crashes the
/// worker once. A file whose name holds `teardown` has a module lifetime
/// whose teardown takes 0.5s, then writes `<file>.torn` (and raises, in
/// `teardown_fails`). The `other` adapter's executor is the same one. The
/// `listing` adapter's lists the session: it notes each `refine`, `run` and
/// `close` in `calls-<pid>.log`, with its worker's `AYE_WORKER`, and lists
/// `test_a` then `test_b` in each file, the other way round in a file whose
/// name holds `differs` on any worker but the first, which takes 30s to
/// list a file whose name holds `slow`.
const FAKE_SOURCE: &str = r#"
import gc
import os
import signal
import subprocess
import sys
import threading
import time

import aye_engine as engine


def protocol_fd():
    [channel] = [o for o in gc.get_objects() if type(o).__name__ == "Channel"]
    return channel._stream.fileno()


def note_pid(name, pid):
    with open(os.path.join(os.environ["FAKE_DIR"], f"{name}.pid"), "w") as f:
        f.write(str(pid))


class FakeExecutor(engine.EngineExecutor):
    adapter = "fake"

    def run(self, batch, sink):
        if any("odd_record" in id for id in batch.ids):
            big, nan, name = 10**400, float("nan"), "caf\udce9.py"
            odd = {"nan": nan, "inf": [float("inf")], "big": big, "huge": 10**5000, (1, 2): nan}
            record = {"batch": batch.id, "event": "diagnostic", "severity": "info", "code": "odd"}
            record.update(path=name, message=f"{name} {big}", odd=odd)
            record[name] = name
            sink._write(record)
        stop = next((id for id in batch.ids if "exit_session" in id), None)
        if stop is None:
            done = super().run(batch, sink)
            statuses = [int(id.rsplit("_", 1)[1]) for id in batch.ids if "_status_" in id]
            return statuses[0] if statuses else done
        for at, id in enumerate(batch.ids):
            sink.test_started(id)
            if id != stop:
                self.body(id)()
            outcome = "error" if id == stop else "passed"
            sink.emit("test_finished", id=id, outcome=outcome, duration_ms=0.0)
            if id == stop:
                left = batch.ids[at + 1 :]
                if left:
                    sink.diagnostic(
                        "error",
                        "batch-stopped",
                        "",
                        f"pytest stopped the session: Exit: stop; {len(left)} of "
                        f"{len(batch.ids)} tests in the batch never started: {', '.join(left)}",
                    )
                last = id.rsplit("_", 1)[1]
                return int(last) if last.isdigit() else 2
        return 0

    def load(self, batch):
        context = batch.context if isinstance(batch.context, dict) else {}
        ran = set(context.get("ran_ids", []))
        ids = []
        for id in batch.ids:
            if id.endswith("test_expand"):
                ids += [f"{id}[{i}]" for i in range(5) if f"{id}[{i}]" not in ran]
            else:
                ids.append(id)
        batch.ids = ids
        if any("die_on_load" in id for id in batch.ids):
            sys.stderr.write("dying on load\n")
            sys.stderr.flush()
            os._exit(4)
        if any("load_raises" in id for id in batch.ids):
            raise RuntimeError("cannot load this batch")
        lifetimes = {}
        tests = []
        for id in batch.ids:
            path = id.split("::")[0]
            if path not in lifetimes:
                teardown = self.teardown(path) if "teardown" in path else None
                lifetimes[path] = engine.Lifetime(path, "module", teardown=teardown)
            tests.append(engine.Test(id, self.body(id), lifetimes=[lifetimes[path]]))
        return tests

    def teardown(self, path):
        def run():
            time.sleep(0.5)
            open(os.path.join(os.environ["FAKE_DIR"], f"{path}.torn"), "w").close()
            if "teardown_fails" in path:
                raise RuntimeError("teardown broke")

        return run

    def body(self, id):
        name = id.rsplit("::", 1)[-1]

        def run():
            marker = os.path.join(os.environ.get("FAKE_DIR", "."), "signal")
            if "deaf" in name:
                signal.signal(signal.SIGINT, signal.SIG_IGN)
            if "signal" in name:
                open(marker, "w").close()
            if "await" in name:
                deadline = time.monotonic() + 30
                while not os.path.exists(marker) and time.monotonic() < deadline:
                    time.sleep(0.01)
            if "spawn" in name:
                child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
                note_pid(name, child.pid)
            if "thread" in name:
                threading.Thread(target=time.sleep, args=(60,)).start()
            if "exec" in name:
                os.execv("/bin/sleep", ["sleep", "60"])
            if "hold" in name:
                fd = protocol_fd()
                child = subprocess.Popen(["sleep", "60"], pass_fds=[fd])
                note_pid(name, child.pid)
                os._exit(3)
            if "sleep" in name:
                time.sleep(int(name.rsplit("_", 1)[1]) / 1000)
            if "print" in name:
                print(f"hello from {id}")
            if "garbage" in name:
                for obj in gc.get_objects():
                    if type(obj).__name__ == "Channel":
                        obj._stream.write("this is not json\n")
                        obj._stream.flush()
            if name == "test_expand[2]":
                marker = os.path.join(os.environ["FAKE_DIR"], "expand_crashed")
                if not os.path.exists(marker):
                    open(marker, "w").close()
                    os._exit(3)
            if "whoami" in name:
                [worker] = [o for o in gc.get_objects() if type(o).__name__ == "Worker"]
                raise engine.Skip(f"{os.getpid()} {','.join(sorted(worker.executors))}")
            if "leak" in name:
                import json

                os.getpid = lambda: 1
                json.dumps = lambda *args, **kwargs: '{"stub": true}'
            if "env" in name:
                get = os.environ.get
                raise engine.Skip(
                    f"{get('AYE_WORKER')}/{get('PYTEST_XDIST_WORKER', '-')}"
                    f"/{get('PYTEST_XDIST_WORKER_COUNT', '-')}"
                )
            if "crash_once" in name:
                marker = os.path.join(os.environ["FAKE_DIR"], name)
                if not os.path.exists(marker):
                    open(marker, "w").close()
                    os._exit(3)
            elif "crash" in name:
                sys.stderr.write(f"about to crash in {id}\n")
                sys.stderr.flush()
                os._exit(3)
            if "fail" in name:
                raise AssertionError("boom")

        return run

    def refine(self, files, context):
        if context == "explode":
            raise RuntimeError("cannot refine")
        if context == "thread":
            threading.Thread(target=time.sleep, args=(60,)).start()
        cases = [
            {"id": f"{f}::test_refined", "adapter": "fake", "certainty": "refined"} for f in files
        ]
        return {"cases": cases, "diagnostics": []}


class OtherExecutor(FakeExecutor):
    adapter = "other"


def note_call(*parts):
    path = os.path.join(os.environ["FAKE_DIR"], f"calls-{os.getpid()}.log")
    with open(path, "a") as f:
        f.write(" ".join([os.environ.get("AYE_WORKER", "-"), *parts]) + "\n")


class ListingExecutor(FakeExecutor):
    adapter = "listing"

    def refine(self, files, context, digests_only=False):
        note_call("refine", str(int(digests_only)), ",".join(files))
        first = os.environ.get("AYE_WORKER", "0") == "0"
        cases = []
        for f in files:
            names = ["test_a", "test_b"]
            if "differs" in f and not first:
                names.reverse()
            if "slow" in f and not first:
                time.sleep(30)
            cases += [{"id": f"{f}::{n}", "adapter": "listing", "certainty": "refined"} for n in names]
        return {"cases": cases, "diagnostics": []}

    def run(self, batch, sink):
        note_call("run", ",".join(batch.ids))
        return super().run(batch, sink)

    def close(self):
        note_call("close")
"#;

const FAKE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "fake_executor",
    source: FAKE_SOURCE,
};

const FAKE: Executor = Executor {
    adapter: "fake",
    modules: &[FAKE_MODULE],
    entry: "fake_executor:FakeExecutor",
    lists_session: false,
};

const OTHER: Executor = Executor {
    adapter: "other",
    modules: &[FAKE_MODULE],
    entry: "fake_executor:OtherExecutor",
    lists_session: false,
};

const LISTING: Executor = Executor {
    adapter: "listing",
    modules: &[FAKE_MODULE],
    entry: "fake_executor:ListingExecutor",
    lists_session: true,
};

fn python() -> Option<PathBuf> {
    if let Some(python) = env::var_os("AYE_TEST_PYTHON").filter(|p| !p.is_empty()) {
        return Some(python.into());
    }
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv");
    aye_runner::venv_python(&venv)
}

macro_rules! python_or_skip {
    () => {
        match python() {
            Some(python) => python,
            None => {
                eprintln!("skipped: no Python (run `uv sync`, or set AYE_TEST_PYTHON)");
                return;
            }
        }
    };
}

struct Fixture {
    dir: tempfile::TempDir,
    runner: Runner,
}

fn fixture(python: PathBuf, workers: u32) -> Fixture {
    fixture_echoing(python, workers, Echo::default())
}

fn fixture_echoing(python: PathBuf, workers: u32, echo: Echo) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let runner = Runner::new(RunnerOptions {
        python,
        root: root.clone(),
        cwd: root.clone(),
        workers,
        executors: vec![FAKE, OTHER, LISTING],
        env: vec![("FAKE_DIR".into(), root.into_os_string())],
        echo,
        collect_only: false,
    });
    Fixture { dir, runner }
}

/// Cases whose file is their lifetime, as the adapters but the native
/// engine declare: a file's cases share a batch.
fn cases(ids: &[&str]) -> Vec<Case> {
    free_cases(ids)
        .into_iter()
        .map(|mut case| {
            case.affinity = vec![case.id.path().to_string()];
            case
        })
        .collect()
}

/// Cases that share no lifetime: the scheduler may split their files.
fn free_cases(ids: &[&str]) -> Vec<Case> {
    ids.iter().map(|id| Case::new(*id, "fake")).collect()
}

fn durations(entries: &[(&str, u64)]) -> HashMap<TestId, Duration> {
    entries
        .iter()
        .map(|(id, ms)| (TestId::new(*id), Duration::from_millis(*ms)))
        .collect()
}

impl Fixture {
    fn dir(&self) -> &Path {
        self.dir.path()
    }

    /// The pid a `test_spawn` or `test_hold` test noted, waiting for it.
    #[cfg(unix)]
    fn noted_pid(&self, name: &str) -> u32 {
        let path = self.dir().join(format!("{name}.pid"));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(text) = std::fs::read_to_string(&path)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "{name} never noted its pid");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn run(&self, plan: &Plan) -> (Vec<Record>, RunSummary) {
        let mut records = Vec::new();
        let summary = self
            .runner
            .run(plan, &mut Stamper::new(), &mut |r| records.push(r));
        (records, summary)
    }

    fn run_ids(&self, ids: &[&str]) -> (Vec<Record>, RunSummary) {
        self.run(&Plan {
            cases: cases(ids),
            ..Plan::default()
        })
    }
}

fn results(records: &[Record]) -> Vec<&TestResult> {
    records
        .iter()
        .filter_map(|r| match &r.event {
            Event::TestFinished(result) => Some(&**result),
            _ => None,
        })
        .collect()
}

fn outcomes(records: &[Record]) -> Vec<(&str, Outcome)> {
    results(records)
        .into_iter()
        .map(|r| (r.id.as_str(), r.outcome))
        .collect()
}

fn kinds(records: &[Record]) -> Vec<&'static str> {
    records.iter().map(|r| r.event.kind()).collect()
}

fn find<'a>(records: &'a [Record], kind: &str) -> Vec<&'a Record> {
    records.iter().filter(|r| r.event.kind() == kind).collect()
}

#[test]
fn the_handshake_reports_the_interpreter() {
    let python = python_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let mut spec = WorkerSpec::new(&python, dir.path(), dir.path());
    spec.executors = vec![FAKE];
    let worker = spawn(&spec).unwrap();
    assert!(worker.pid() > 0);
    assert_ne!(worker.python(), "");
    assert!(worker.version().starts_with("3."), "{}", worker.version());
    let exit = worker.shutdown();
    assert_eq!(exit.code(), Some(0), "{}", exit.stderr);
}

#[test]
fn a_worker_answers_requests_it_cannot_serve_with_failed() {
    let python = python_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let mut worker = spawn(&WorkerSpec::new(&python, dir.path(), dir.path())).unwrap();
    worker
        .send(&Request::Refine(RefineRequest {
            adapter: "nope".into(),
            files: vec![],
            context: serde_json::Value::Null,
            digests_only: false,
        }))
        .unwrap();
    let Incoming::Line(aye_engine::WorkerLine::Reply(Reply::Failed { message })) = worker.recv()
    else {
        panic!("expected failed");
    };
    assert!(
        message.contains("no executor for the 'nope' adapter"),
        "{message}"
    );
    assert_eq!(worker.shutdown().code(), Some(0));
}

#[test]
fn a_missing_interpreter_cannot_start() {
    let dir = tempfile::tempdir().unwrap();
    let error = spawn(&WorkerSpec::new(
        dir.path().join("no-python"),
        dir.path(),
        dir.path(),
    ))
    .unwrap_err();
    assert!(matches!(error, SpawnError::Start { .. }), "{error}");
    assert!(error.to_string().contains("no-python"), "{error}");
}

#[test]
fn a_worker_that_cannot_start_is_reported_once_per_lane() {
    let dir = tempfile::tempdir().unwrap();
    let runner = Runner::new(RunnerOptions {
        python: dir.path().join("no-python"),
        root: dir.path().into(),
        cwd: dir.path().into(),
        workers: 2,
        executors: vec![FAKE],
        ..RunnerOptions::default()
    });
    let plan = Plan {
        cases: cases(&["a.py::test_a", "b.py::test_b"]),
        ..Plan::default()
    };
    let mut records = Vec::new();
    let summary = runner.run(&plan, &mut Stamper::new(), &mut |r| records.push(r));
    assert!(summary.internal_error);
    assert_eq!(summary.finished, 0);
    assert_eq!(kinds(&records), ["worker_finished", "worker_finished"]);
    let Event::WorkerFinished { error, .. } = &records[0].event else {
        unreachable!()
    };
    assert!(error.as_deref().unwrap().contains("no-python"));
}

#[cfg(unix)]
fn fake_interpreter(dir: &Path, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("fake-python");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
#[test]
fn a_worker_of_another_protocol_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(
        dir.path(),
        r#"echo '{"reply":"ready","protocol":99,"pid":1,"python":"fake","version":"0"}'; cat >/dev/null"#,
    );
    let error = spawn(&WorkerSpec::new(python, dir.path(), dir.path())).unwrap_err();
    assert!(
        matches!(
            error,
            SpawnError::Protocol {
                worker: 99,
                runner: aye_engine::PROTOCOL_VERSION
            }
        ),
        "{error}"
    );
    let message = error.to_string();
    let ours = format!("protocol {}", aye_engine::PROTOCOL_VERSION);
    assert!(
        message.contains("protocol 99") && message.contains(&ours),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_answer_to_the_handshake_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(
        dir.path(),
        r#"echo '{"reply":"ready","protocol":"one"}'; cat >/dev/null"#,
    );
    let error = spawn(&WorkerSpec::new(python, dir.path(), dir.path())).unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, SpawnError::Hello(_)), "{message}");
    assert!(
        message.contains("unreadable answer to the handshake"),
        "{message}"
    );
}

#[cfg(unix)]
#[test]
fn output_before_a_failed_handshake_is_kept_for_the_error() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(
        dir.path(),
        "echo 'site says hi'; echo 'Traceback: boom' >&2; exit 7",
    );
    let error = spawn(&WorkerSpec::new(python, dir.path(), dir.path())).unwrap_err();
    let message = error.to_string();
    assert!(matches!(error, SpawnError::Exited { .. }), "{message}");
    assert!(message.contains("exit status 7"), "{message}");
    assert!(message.contains("site says hi"), "{message}");
    assert!(message.contains("Traceback: boom"), "{message}");
}

#[test]
fn records_are_stamped_and_ordered() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = Plan {
        cases: cases(&["a.py::test_pass", "a.py::test_fail", "b.py::test_pass"]),
        ..Plan::default()
    };
    let mut stamper = Stamper::with_run("run-1");
    stamper.stamp(Record::new(Event::CollectionStarted));
    let mut records = Vec::new();
    let summary = f.runner.run(&plan, &mut stamper, &mut |r| records.push(r));

    assert_eq!(
        kinds(&records),
        [
            "worker_started",
            "batch_started",
            "lifetime_started",
            "test_started",
            "test_finished",
            "test_started",
            "test_finished",
            "lifetime_finished",
            "lifetime_started",
            "test_started",
            "test_finished",
            "lifetime_finished",
            "batch_finished",
            "worker_finished",
        ]
    );
    for (seq, record) in (1..).zip(&records) {
        let envelope = &record.envelope;
        assert_eq!(envelope.seq, Some(seq));
        assert_eq!(envelope.run.as_deref(), Some("run-1"));
        assert_eq!(envelope.worker, Some(0));
        let in_batch = !record.event.kind().starts_with("worker_");
        assert_eq!(envelope.batch, in_batch.then_some(0), "{record:?}");
    }
    assert!(
        records
            .windows(2)
            .all(|w| w[0].envelope.t_ms <= w[1].envelope.t_ms)
    );
    assert_eq!(stamper.next_seq(), 15);

    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_pass", Outcome::Passed),
            ("a.py::test_fail", Outcome::Failed),
            ("b.py::test_pass", Outcome::Passed),
        ]
    );
    let Event::WorkerStarted { pid, python, .. } = &records[0].event else {
        unreachable!()
    };
    assert!(*pid > 0 && !python.is_empty());
    assert_eq!((summary.requested, summary.finished), (3, 3));
    assert_eq!((summary.counts.passed, summary.counts.failed), (2, 1));
    assert_eq!(summary.batches.len(), 1);
    assert_eq!(summary.batches[0].status, Some(1));
    assert_eq!(summary.batches[0].end, BatchEnd::Done);
    assert!(!summary.internal_error && !summary.stopped_early && summary.crashes.is_empty());
    let Event::WorkerFinished { exit_status, error } = &records[13].event else {
        unreachable!()
    };
    assert_eq!((*exit_status, error.as_deref()), (Some(0), None));
}

#[test]
fn batches_are_handed_out_as_workers_free_up() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let plan = Plan {
        cases: cases(&[
            "b.py::test_env",
            "slow.py::test_sleep_2000",
            "c.py::test_env",
            "d.py::test_env",
        ]),
        durations: durations(&[
            ("slow.py::test_sleep_2000", 2000),
            ("b.py::test_env", 200),
            ("c.py::test_env", 200),
            ("d.py::test_env", 200),
        ]),
        ..Plan::default()
    };
    let (records, summary) = f.run(&plan);
    assert_eq!(find(&records, "worker_started").len(), 2);
    assert_eq!(summary.batches.len(), 4);
    let slow = &summary.batches[0];
    assert_eq!(slow.cases, 1);
    let others: Vec<_> = summary.batches[1..].iter().map(|b| b.worker).collect();
    assert_eq!(others, [1 - slow.worker; 3], "{:?}", summary.batches);

    for result in results(&records) {
        if result.id.as_str().ends_with("test_env") {
            let record = records
                .iter()
                .find(|r| matches!(&r.event, Event::TestFinished(t) if t.id == result.id))
                .unwrap();
            let worker = record.envelope.worker.unwrap();
            assert_eq!(
                result.reason.as_deref(),
                Some(&*format!("{worker}/gw{worker}/2"))
            );
        }
    }
    assert_eq!(summary.counts.skipped, 3);
    assert_eq!(summary.counts.passed, 1);
}

#[test]
fn serial_batches_never_overlap() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let mut plan = Plan {
        cases: cases(&[
            "s1.py::test_sleep_300",
            "s2.py::test_sleep_300",
            "x.py::test_sleep_100",
        ]),
        ..Plan::default()
    };
    plan.cases[0].serial = true;
    plan.cases[1].serial = true;
    let (records, summary) = f.run(&plan);
    assert_eq!(summary.counts.passed, 3);
    let span = |file: &str| {
        let batch = summary
            .batches
            .iter()
            .find(|b| {
                records.iter().any(|r| {
                    r.envelope.batch == Some(b.id)
                        && matches!(&r.event, Event::TestStarted { id } if id.path() == file)
                })
            })
            .unwrap()
            .id;
        let seq = |kind: &str| {
            records
                .iter()
                .find(|r| r.envelope.batch == Some(batch) && r.event.kind() == kind)
                .and_then(|r| r.envelope.seq)
                .unwrap()
        };
        (seq("batch_started"), seq("batch_finished"))
    };
    let (s1, s2) = (span("s1.py"), span("s2.py"));
    assert!(s1.1 < s2.0 || s2.1 < s1.0, "{s1:?} {s2:?}");
}

#[test]
fn never_more_workers_than_batches() {
    let python = python_or_skip!();
    let f = fixture(python, 8);
    let (records, summary) = f.run_ids(&["a.py::test_env", "a.py::test_pass", "b.py::test_env"]);
    assert_eq!(find(&records, "worker_started").len(), 2);
    assert_eq!(summary.batches.len(), 2);
    assert_eq!(summary.finished, 3);
}

#[test]
fn one_worker_gets_no_xdist_variables() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, _) = f.run_ids(&["a.py::test_env"]);
    assert_eq!(results(&records)[0].reason.as_deref(), Some("0/-/-"));
}

#[test]
fn maxfail_stops_every_worker() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let plan = Plan {
        cases: cases(&[
            "a.py::test_fail",
            "slow.py::test_sleep_5000",
            "c.py::test_pass",
            "d.py::test_fail",
        ]),
        durations: durations(&[
            ("slow.py::test_sleep_5000", 5000),
            ("a.py::test_fail", 1000),
            ("c.py::test_pass", 1000),
            ("d.py::test_fail", 1000),
        ]),
        policy: Policy {
            maxfail: Some(1),
            capture: true,
            ..Policy::default()
        },
        ..Plan::default()
    };
    let started = Instant::now();
    let (records, summary) = f.run(&plan);
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(summary.stopped_early);
    assert_eq!(outcomes(&records), [("a.py::test_fail", Outcome::Failed)]);
    assert_eq!(summary.counts.bad(), 1);
    // The slow batch is interrupted mid-test, or before its worker was ready
    // under load; the failing batch ends by itself. The other two never
    // start.
    assert!(
        summary
            .batches
            .iter()
            .all(|b| b.id <= 1 && matches!(b.end, BatchEnd::Done | BatchEnd::Stopped)),
        "{:#?}",
        summary.batches
    );
    if let Some(slow) = summary.batches.iter().find(|b| b.id == 0) {
        assert_eq!(slow.end, BatchEnd::Stopped);
    }
    assert_eq!(
        find(&records, "batch_started").len(),
        find(&records, "batch_finished").len()
    );
    let finished = find(&records, "worker_finished");
    assert_eq!(finished.len(), find(&records, "worker_started").len());
    for record in finished {
        let Event::WorkerFinished { error, .. } = &record.event else {
            unreachable!()
        };
        assert_eq!(error, &None);
    }
    assert!(!summary.internal_error);
}

#[test]
fn maxfail_with_one_worker_reports_exactly_that_many() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = Plan {
        cases: cases(&[
            "a.py::test_pass",
            "a.py::test_fail",
            "a.py::test_fail_too",
            "b.py::test_pass",
        ]),
        policy: Policy {
            maxfail: Some(2),
            capture: true,
            ..Policy::default()
        },
        ..Plan::default()
    };
    let (records, summary) = f.run(&plan);
    assert!(summary.stopped_early);
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_pass", Outcome::Passed),
            ("a.py::test_fail", Outcome::Failed),
            ("a.py::test_fail_too", Outcome::Failed),
        ]
    );
}

fn maxfail(ids: &[&str], max: u32) -> Plan {
    Plan {
        cases: cases(ids),
        policy: Policy {
            maxfail: Some(max),
            capture: true,
            ..Policy::default()
        },
        ..Plan::default()
    }
}

/// pytest's `-x` and unittest's `-f` stop the session, then tear down: a
/// slow teardown (stopping a container) is not cut short.
#[test]
fn maxfail_lets_the_batch_that_reached_it_tear_down() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = maxfail(
        &["slow_teardown.py::test_fail", "slow_teardown.py::test_pass"],
        1,
    );
    let (records, summary) = f.run(&plan);
    assert!(
        f.dir().join("slow_teardown.py.torn").exists(),
        "the teardown never ran"
    );
    assert!(summary.stopped_early);
    assert_eq!(
        outcomes(&records),
        [("slow_teardown.py::test_fail", Outcome::Failed)]
    );
    assert_eq!(
        summary.not_run,
        [TestId::new("slow_teardown.py::test_pass")]
    );
    let ends: Vec<BatchEnd> = summary.batches.iter().map(|b| b.end).collect();
    assert_eq!(ends, [BatchEnd::Done]);
    // The lifetime it opened in the log is closed there.
    assert_eq!(find(&records, "lifetime_started").len(), 1);
    assert_eq!(find(&records, "lifetime_finished").len(), 1);
    assert!(!summary.internal_error);
}

/// unittest's `-f` reports a `tearDownClass` that raised after the failure
/// that stopped it.
#[test]
fn maxfail_keeps_the_teardown_failures_that_follow_it() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = maxfail(
        &[
            "teardown_fails.py::test_fail",
            "teardown_fails.py::test_pass",
        ],
        1,
    );
    let (records, summary) = f.run(&plan);
    assert_eq!(summary.counts.bad(), 1);
    let finished = find(&records, "lifetime_finished");
    assert_eq!(finished.len(), 1, "{:?}", kinds(&records));
    let Event::LifetimeFinished {
        lifetime, failure, ..
    } = &finished[0].event
    else {
        unreachable!()
    };
    assert_eq!(lifetime, "teardown_fails.py");
    let failure = failure.as_ref().expect("the teardown's failure");
    assert_eq!(failure.message, "RuntimeError: teardown broke");
    let order = kinds(&records);
    let at = order
        .iter()
        .position(|k| *k == "lifetime_finished")
        .unwrap();
    assert_eq!(order[at + 1], "batch_finished");
}

/// pytest-xdist shuts its workers down when `maxfail` runs out; the others'
/// batches stop at their next test, tearing down, not killed outright. On
/// Windows, which has no SIGINT to send them, they are killed: nothing
/// tears down there.
#[test]
fn maxfail_interrupts_the_other_workers_which_tear_down() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let slow = "slow_teardown.py::test_signal_sleep_20000";
    let mut plan = maxfail(&["a.py::test_await_fail", slow], 1);
    plan.durations = durations(&[(slow, 20_000), ("a.py::test_await_fail", 1000)]);
    let started = Instant::now();
    let (records, summary) = f.run(&plan);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    let tears_down = cfg!(unix);
    assert_eq!(
        f.dir().join("slow_teardown.py.torn").exists(),
        tears_down,
        "whether the interrupted batch tore down"
    );
    assert_eq!(
        outcomes(&records),
        [("a.py::test_await_fail", Outcome::Failed)]
    );
    let ends: Vec<(u32, BatchEnd)> = summary.batches.iter().map(|b| (b.id, b.end)).collect();
    assert_eq!(ends, [(0, BatchEnd::Stopped), (1, BatchEnd::Done)]);
    assert!(!summary.internal_error);
    // Both lifetimes are closed in the log where the interrupted one tore
    // down cleanly; only the other one where it was killed.
    assert_eq!(
        find(&records, "lifetime_finished").len(),
        if tears_down { 2 } else { 1 }
    );
}

/// A worker that ignores the interrupt is killed after the grace period.
#[cfg(unix)]
#[test]
fn maxfail_kills_an_interrupted_worker_that_does_not_stop() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let stubborn = "b.py::test_signal_deaf_sleep_60000";
    let mut plan = maxfail(&["a.py::test_await_fail", stubborn], 1);
    plan.durations = durations(&[(stubborn, 60_000), ("a.py::test_await_fail", 1000)]);
    let started = Instant::now();
    let (records, summary) = f.run(&plan);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(9) && elapsed < Duration::from_secs(30),
        "{elapsed:?}"
    );
    assert_eq!(
        outcomes(&records),
        [("a.py::test_await_fail", Outcome::Failed)]
    );
    let ends: Vec<(u32, BatchEnd)> = summary.batches.iter().map(|b| (b.id, b.end)).collect();
    assert_eq!(ends, [(0, BatchEnd::Stopped), (1, BatchEnd::Done)]);
}

#[test]
fn a_crash_fails_the_running_test_and_requeues_the_rest() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&[
        "a.py::test_pass",
        "a.py::test_crash",
        "a.py::test_after",
        "b.py::test_pass",
    ]);
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_pass", Outcome::Passed),
            ("a.py::test_crash", Outcome::Error),
            ("a.py::test_after", Outcome::Passed),
            ("b.py::test_pass", Outcome::Passed),
        ]
    );
    let crashed = results(&records)[1];
    let failure = crashed.failure().unwrap();
    assert_eq!(
        failure.message,
        "worker crashed while running this test: exit status 3"
    );
    let detail = failure.detail.as_deref().unwrap();
    assert!(
        detail.contains("about to crash in a.py::test_crash"),
        "{detail}"
    );

    assert_eq!(summary.crashes.len(), 1);
    assert_eq!(
        summary.crashes[0].test.as_ref().map(TestId::as_str),
        Some("a.py::test_crash")
    );
    assert!(!summary.internal_error);
    let ends: Vec<(u32, BatchEnd, u32)> = summary
        .batches
        .iter()
        .map(|b| (b.id, b.end, b.cases))
        .collect();
    assert_eq!(ends, [(0, BatchEnd::Crashed, 4), (1, BatchEnd::Done, 2)]);

    let pids: Vec<u32> = find(&records, "worker_started")
        .iter()
        .map(|r| match r.event {
            Event::WorkerStarted { pid, .. } => pid,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(pids.len(), 2);
    assert_ne!(pids[0], pids[1]);
    let finished = find(&records, "worker_finished");
    let Event::WorkerFinished { exit_status, error } = &finished[0].event else {
        unreachable!()
    };
    assert_eq!(*exit_status, Some(3));
    assert_eq!(
        error.as_deref(),
        Some("worker crashed while running a.py::test_crash (exit status 3)")
    );
    let order = kinds(&records);
    let crash_at = order.iter().position(|k| *k == "worker_finished").unwrap();
    assert_eq!(
        &order[crash_at - 2..=crash_at + 2],
        [
            "test_finished",
            "batch_finished",
            "worker_finished",
            "worker_started",
            "batch_started",
        ]
    );
}

#[test]
fn a_crash_that_happens_once_is_recovered_from() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["a.py::test_crash_once", "a.py::test_next"]);
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_crash_once", Outcome::Error),
            ("a.py::test_next", Outcome::Passed),
        ]
    );
    assert_eq!(summary.crashes.len(), 1);
}

#[test]
fn a_second_crash_reports_the_rest_as_errors() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["x.py::test_die_on_load", "x.py::test_other"]);
    assert_eq!(
        outcomes(&records),
        [
            ("x.py::test_die_on_load", Outcome::Error),
            ("x.py::test_other", Outcome::Error),
        ]
    );
    for result in results(&records) {
        let failure = result.failure().unwrap();
        assert_eq!(
            failure.message,
            "worker crashed again before this test ran: exit status 4"
        );
        assert!(failure.detail.as_deref().unwrap().contains("dying on load"));
    }
    assert_eq!(summary.crashes.len(), 2);
    assert!(summary.crashes.iter().all(|c| c.test.is_none()));
    assert!(summary.internal_error);
    let ends: Vec<BatchEnd> = summary.batches.iter().map(|b| b.end).collect();
    assert_eq!(ends, [BatchEnd::Crashed, BatchEnd::Crashed]);
    let Event::WorkerFinished { error, .. } = &find(&records, "worker_finished")[0].event else {
        unreachable!()
    };
    let error = error.as_deref().unwrap();
    assert!(
        error.starts_with("worker crashed outside any test (exit status 4):\n"),
        "{error}"
    );
    assert!(error.contains("dying on load"), "{error}");
}

#[test]
fn an_adapter_that_raises_fails_its_batch_only() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["x.py::test_load_raises"]);
    assert!(summary.internal_error);
    assert_eq!(summary.batches[0].end, BatchEnd::Failed);
    assert_eq!(results(&records), [] as [&aye_events::TestResult; 0]);
    let diagnostics = find(&records, "diagnostic");
    let Event::Diagnostic(diagnostic) = &diagnostics[0].event else {
        unreachable!()
    };
    assert_eq!(diagnostic.code, "batch-failed");
    assert_eq!(diagnostic.path, "x.py");
    assert!(
        diagnostic.message.contains("cannot load this batch"),
        "{}",
        diagnostic.message
    );
    let Event::WorkerFinished { exit_status, error } = &records.last().unwrap().event else {
        panic!("expected worker_finished last");
    };
    assert_eq!((*exit_status, error), (Some(0), &None));
}

#[test]
fn a_line_that_is_not_protocol_is_reported_and_the_run_goes_on() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["a.py::test_garbage", "a.py::test_pass"]);
    assert!(summary.internal_error);
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_garbage", Outcome::Passed),
            ("a.py::test_pass", Outcome::Passed),
        ]
    );
    let diagnostics = find(&records, "diagnostic");
    assert_eq!(diagnostics.len(), 1);
    let Event::Diagnostic(diagnostic) = &diagnostics[0].event else {
        unreachable!()
    };
    assert_eq!(diagnostic.code, "protocol-error");
    assert!(
        diagnostic.message.contains("this is not json"),
        "{}",
        diagnostic.message
    );
}

#[test]
fn the_stop_handle_kills_running_workers() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let stop = f.runner.stop_handle();
    let stopper = thread::spawn(move || {
        thread::sleep(Duration::from_millis(500));
        stop.stop();
    });
    let started = Instant::now();
    let (records, summary) = f.run_ids(&["a.py::test_sleep_5000", "b.py::test_sleep_5000"]);
    stopper.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "{:?}",
        started.elapsed()
    );
    assert!(summary.interrupted);
    assert!(!summary.internal_error);
    assert_eq!(results(&records), [] as [&aye_events::TestResult; 0]);
    assert!(summary.batches.iter().all(|b| b.end == BatchEnd::Stopped));
    assert_eq!(
        find(&records, "worker_finished").len(),
        find(&records, "worker_started").len()
    );
}

#[test]
fn refinement_round_trips() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let refined = f
        .runner
        .refine(&RefineRequest {
            adapter: "fake".into(),
            files: vec![path("a.py"), path("b.py")],
            context: serde_json::Value::Null,
            digests_only: false,
        })
        .unwrap();
    let ids: Vec<&str> = refined.cases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["a.py::test_refined", "b.py::test_refined"]);
    assert_eq!(refined.cases[0].certainty, aye_events::Certainty::Refined);

    let error = f
        .runner
        .refine(&RefineRequest {
            adapter: "fake".into(),
            files: vec![path("a.py")],
            context: "explode".into(),
            digests_only: false,
        })
        .unwrap_err();
    assert!(error.0.contains("RuntimeError: cannot refine"), "{error}");

    let error = f
        .runner
        .refine(&RefineRequest {
            adapter: "nope".into(),
            files: vec![],
            context: serde_json::Value::Null,
            digests_only: false,
        })
        .unwrap_err();
    assert_eq!(error.0, "no executor for the nope adapter");
}

/// Echoing goes to the process's own stdout or stderr, so this test runs
/// itself in a child process and reads what it printed.
#[test]
fn output_is_echoed_only_when_not_capturing() {
    let python = python_or_skip!();
    if env::var_os("AYE_ECHO_CHILD").is_some() {
        let loud = |id: &str| Plan {
            cases: cases(&[id]),
            policy: Policy {
                capture: false,
                ..Policy::default()
            },
            ..Plan::default()
        };
        let f = fixture(python.clone(), 1);
        f.run(&Plan {
            cases: cases(&["a.py::test_print_quiet"]),
            ..Plan::default()
        });
        f.run(&loud("a.py::test_print_loud"));
        fixture_echoing(python.clone(), 1, Echo::Stderr).run(&loud("a.py::test_print_aside"));
        // A worker kept from refinement, which started without echo.
        let f = fixture(python, 1);
        f.refine_listing(&["w.py"]);
        let (_, summary) = f.run(&Plan {
            cases: vec![listing("w.py::test_print_kept")],
            session_files: session(&["w.py"]),
            ..loud("w.py::test_print_kept")
        });
        assert_eq!(f.calls().len(), 1, "the kept worker ran the test");
        assert_eq!(summary.finished, 1);
        return;
    }
    let output = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "output_is_echoed_only_when_not_capturing",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AYE_ECHO_CHILD", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}");
    assert!(
        stdout.contains("hello from a.py::test_print_loud"),
        "{stdout}"
    );
    assert!(
        stdout.contains("hello from w.py::test_print_kept"),
        "{stdout}"
    );
    assert!(!stdout.contains("test_print_quiet"), "{stdout}");
    // Beside a machine-readable stdout, the echo goes to stderr.
    assert!(!stdout.contains("test_print_aside"), "{stdout}");
    assert!(
        stderr.contains("hello from a.py::test_print_aside"),
        "{stderr}"
    );
}

/// Whether a process is still alive.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    Command::new("/bin/sh")
        .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
        .status()
        .unwrap()
        .success()
}

/// Wait for a process to be gone; false if it outlives `within`.
#[cfg(unix)]
fn gone_within(pid: u32, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while alive(pid) {
        if Instant::now() > deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
    true
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    let _ = Command::new("/bin/sh")
        .args(["-c", &format!("kill -9 {pid} 2>/dev/null")])
        .status();
}

/// A worker that ignores `shutdown` (and SIGTERM) is killed once the grace
/// period is over, instead of hanging the runner.
#[cfg(unix)]
#[test]
fn a_worker_that_outlives_its_grace_period_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(dir.path(), "trap '' TERM INT HUP\nexec sleep 60");
    let worker = Worker::launch(&WorkerSpec::new(python, dir.path(), dir.path())).unwrap();
    let started = Instant::now();
    let exit = worker.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(exit.describe(), "killed by signal 9 (SIGKILL)");
}

/// Killing a worker kills its process group: what its tests started.
#[cfg(unix)]
#[test]
fn killing_a_worker_kills_its_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(
        dir.path(),
        "sleep 60 &\necho $! > \"$FAKE_DIR/child.pid\"\nwait",
    );
    let mut spec = WorkerSpec::new(python, dir.path(), dir.path());
    spec.env
        .push(("FAKE_DIR".into(), dir.path().as_os_str().to_owned()));
    let worker = Worker::launch(&spec).unwrap();
    let pid_file = dir.path().join("child.pid");
    let deadline = Instant::now() + Duration::from_secs(10);
    let child: u32 = loop {
        if let Ok(pid) = std::fs::read_to_string(&pid_file)
            .unwrap_or_default()
            .trim()
            .parse()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the fake worker never started");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(alive(child));
    worker.kill();
    let gone = gone_within(child, Duration::from_secs(5));
    if !gone {
        kill_pid(child);
    }
    assert!(gone, "the worker's subprocess outlived it");
    assert_eq!(worker.wait().describe(), "killed by signal 9 (SIGKILL)");
}

/// A worker that dies while a process it started holds its stream open is
/// seen to die by its pid.
#[cfg(unix)]
#[test]
fn a_dead_worker_is_noticed_while_its_stream_is_held_open() {
    let dir = tempfile::tempdir().unwrap();
    let python = fake_interpreter(
        dir.path(),
        "sleep 60 &\necho $! > \"$FAKE_DIR/child.pid\"\necho 'last words'\nexit 3",
    );
    let mut spec = WorkerSpec::new(python, dir.path(), dir.path());
    spec.env
        .push(("FAKE_DIR".into(), dir.path().as_os_str().to_owned()));
    let worker = Worker::launch(&spec).unwrap();
    let started = Instant::now();
    let first = worker.recv();
    let second = worker.recv();
    let exit = worker.wait();
    if let Ok(pid) = std::fs::read_to_string(dir.path().join("child.pid")) {
        kill_pid(pid.trim().parse().unwrap());
    }
    assert!(
        matches!(&first, Incoming::Garbage { line, .. } if line == "last words"),
        "{first:?}"
    );
    assert_eq!(second, Incoming::Eof);
    assert_eq!(exit.code(), Some(3));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_thread_the_tests_left_running_does_not_hang_the_run() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let started = Instant::now();
    let (records, summary) = f.run_ids(&["a.py::test_thread", "a.py::test_pass"]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_thread", Outcome::Passed),
            ("a.py::test_pass", Outcome::Passed),
        ]
    );
    assert!(!summary.internal_error && summary.stopped.is_none());
    let Event::WorkerFinished { exit_status, error } = &records.last().unwrap().event else {
        panic!("expected worker_finished last");
    };
    // Killed after the shutdown grace period: by SIGKILL, which leaves no
    // exit status, or on Windows by `TerminateProcess`, which leaves 1.
    let killed = if cfg!(windows) { Some(1) } else { None };
    assert_eq!((*exit_status, error), (killed, &None));
}

#[test]
fn a_thread_left_running_while_refining_does_not_hang_refinement() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let started = Instant::now();
    let refined = f
        .runner
        .refine(&RefineRequest {
            adapter: "fake".into(),
            files: vec![path("a.py")],
            context: "thread".into(),
            digests_only: false,
        })
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(refined.cases.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_worker_replaced_by_another_program_is_a_crash_not_a_hang() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let started = Instant::now();
    let (records, summary) = f.run_ids(&["a.py::test_exec", "a.py::test_after"]);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_exec", Outcome::Error),
            ("a.py::test_after", Outcome::Passed),
        ]
    );
    assert_eq!(summary.crashes[0].reason, "killed by signal 9 (SIGKILL)");
}

#[cfg(unix)]
#[test]
fn a_crash_is_noticed_while_a_subprocess_holds_the_protocol_stream() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let started = Instant::now();
    let (records, summary) = f.run_ids(&["a.py::test_hold", "a.py::test_after"]);
    kill_pid(f.noted_pid("test_hold"));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_hold", Outcome::Error),
            ("a.py::test_after", Outcome::Passed),
        ]
    );
    assert_eq!(summary.crashes[0].reason, "exit status 3");
}

#[cfg(unix)]
#[test]
fn stopping_kills_what_the_tests_started() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let stop = f.runner.stop_handle();
    let (records, summary) = thread::scope(|scope| {
        let run = scope.spawn(|| f.run_ids(&["a.py::test_spawn_sleep_30000", "a.py::test_pass"]));
        let child = f.noted_pid("test_spawn_sleep_30000");
        assert!(alive(child));
        stop.stop();
        let ran = run.join().unwrap();
        let gone = gone_within(child, Duration::from_secs(5));
        if !gone {
            kill_pid(child);
        }
        assert!(gone, "the test's subprocess outlived the stop");
        ran
    });
    assert!(summary.interrupted);
    assert_eq!(summary.stopped, Some(StopReason::Interrupted));
    assert_eq!(results(&records), [] as [&aye_events::TestResult; 0]);
    assert_eq!(summary.not_run, [TestId::new("a.py::test_pass")]);
}

/// `kill` takes every live worker down at once, from the calling thread,
/// with the processes their tests started, and no new worker survives it.
#[cfg(unix)]
#[test]
fn killing_through_the_stop_handle_takes_every_worker_down_at_once() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let stop = f.runner.stop_handle();
    let plan = Plan {
        cases: cases(&[
            "a.py::test_spawn_a_sleep_30000",
            "b.py::test_spawn_b_sleep_30000",
            "c.py::test_pass",
        ]),
        durations: durations(&[
            ("a.py::test_spawn_a_sleep_30000", 30_000),
            ("b.py::test_spawn_b_sleep_30000", 30_000),
        ]),
        ..Plan::default()
    };
    let started = Instant::now();
    let (records, summary, children) = thread::scope(|scope| {
        let run = scope.spawn(|| f.run(&plan));
        let children = [
            f.noted_pid("test_spawn_a_sleep_30000"),
            f.noted_pid("test_spawn_b_sleep_30000"),
        ];
        stop.kill();
        let (records, summary) = run.join().unwrap();
        (records, summary, children)
    });
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
    for child in children {
        let gone = gone_within(child, Duration::from_secs(5));
        if !gone {
            kill_pid(child);
        }
        assert!(gone, "a test's subprocess outlived kill()");
    }
    assert!(summary.interrupted && !summary.internal_error);
    assert_eq!(results(&records), [] as [&aye_events::TestResult; 0]);
    assert!(summary.batches.iter().all(|b| b.end == BatchEnd::Stopped));
    assert!(summary.not_run.contains(&TestId::new("c.py::test_pass")));
    // A runner whose workers were killed starts no more.
    let (records, again) = f.run_ids(&["d.py::test_pass"]);
    assert!(again.interrupted && records.is_empty());
}

#[test]
fn a_batch_the_framework_interrupts_stops_the_run() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&[
        "a.py::test_pass",
        "a.py::test_exit_session",
        "a.py::test_after",
        "b.py::test_pass",
    ]);
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_pass", Outcome::Passed),
            ("a.py::test_exit_session", Outcome::Error),
        ]
    );
    assert!(summary.interrupted && !summary.internal_error);
    assert_eq!(summary.stopped, Some(StopReason::Framework));
    assert_eq!(summary.batches[0].status, Some(2));
    assert_eq!(
        summary.not_run,
        [
            TestId::new("a.py::test_after"),
            TestId::new("b.py::test_pass")
        ]
    );
}

#[test]
fn an_interrupted_batch_stops_the_hand_out_and_lets_running_batches_finish() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    // a.py's batch ends itself once b.py's is running.
    let plan = Plan {
        cases: cases(&[
            "a.py::test_await",
            "a.py::test_exit_session_7",
            "a.py::test_after",
            "b.py::test_signal_sleep_1000",
            "c.py::test_pass",
        ]),
        durations: durations(&[
            ("a.py::test_await", 9000),
            ("b.py::test_signal_sleep_1000", 8000),
            ("c.py::test_pass", 10),
        ]),
        ..Plan::default()
    };
    let (records, summary) = f.run(&plan);
    let got = outcomes(&records);
    assert!(
        got.contains(&("a.py::test_exit_session_7", Outcome::Error)),
        "{got:?}"
    );
    assert!(
        got.contains(&("b.py::test_signal_sleep_1000", Outcome::Passed)),
        "{got:?}"
    );
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(summary.stopped, Some(StopReason::Framework));
    assert!(summary.interrupted && !summary.internal_error);
    let mut not_run: Vec<&str> = summary.not_run.iter().map(TestId::as_str).collect();
    not_run.sort_unstable();
    assert_eq!(not_run, ["a.py::test_after", "c.py::test_pass"]);
}

/// The test of `b.py`'s batch in [`after_a_batch`].
const B_SLEEPS: &str = "b.py::test_signal_sleep_1000";

/// A run of three batches on two workers. `a.py`'s (`a.py::test_await`,
/// `first`, `a.py::test_after`) goes on once `b.py`'s is running, and ends
/// while it sleeps; `c.py`'s is handed out after it unless `first` stopped
/// the run.
fn after_a_batch(f: &Fixture, first: &str) -> (Vec<Record>, RunSummary) {
    let _ = std::fs::remove_file(f.dir().join("signal"));
    f.run(&Plan {
        cases: cases(&[
            "a.py::test_await",
            first,
            "a.py::test_after",
            B_SLEEPS,
            "c.py::test_pass",
        ]),
        durations: durations(&[("a.py::test_await", 9000), (B_SLEEPS, 8000)]),
        ..Plan::default()
    })
}

fn batch_of<'a>(
    summary: &'a RunSummary,
    records: &[Record],
    id: &str,
) -> &'a aye_runner::BatchSummary {
    let batch = records
        .iter()
        .find(|r| matches!(&r.event, Event::TestFinished(t) if t.id.as_str() == id))
        .and_then(|r| r.envelope.batch)
        .unwrap_or_else(|| panic!("{id} never finished"));
    summary.batches.iter().find(|b| b.id == batch).unwrap()
}

/// `pytest.exit(returncode=4)` makes the batch's status 4: that is an
/// interruption, which the adapter's `batch-stopped` error says, and no
/// usage error.
#[test]
fn an_exit_with_a_usage_error_status_is_an_interruption() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&[
        "a.py::test_pass",
        "a.py::test_exit_session_4",
        "a.py::test_after",
        "b.py::test_pass",
    ]);
    assert!(summary.interrupted && !summary.internal_error);
    assert_eq!(summary.stopped, Some(StopReason::Framework));
    assert_eq!(summary.batches[0].status, Some(4));
    assert_eq!(
        summary.not_run,
        [
            TestId::new("a.py::test_after"),
            TestId::new("b.py::test_pass")
        ]
    );
    let stopped: Vec<&Record> = find(&records, "diagnostic");
    assert_eq!(stopped.len(), 1);
    assert!(matches!(&stopped[0].event, Event::Diagnostic(d) if d.code == "batch-stopped"));
}

/// pytest's internal error mid-batch (status 3, with the adapter's
/// `batch-stopped`) ends its session, so the run stops too; it is an
/// internal error, not an interruption.
#[test]
fn an_internal_error_that_stops_a_batch_stops_the_run_as_an_internal_error() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let (records, summary) = after_a_batch(&f, "a.py::test_exit_session_3");
    assert!(summary.internal_error && !summary.interrupted);
    assert_eq!(summary.stopped, Some(StopReason::Framework));
    assert_eq!(
        batch_of(&summary, &records, "a.py::test_exit_session_3").status,
        Some(3)
    );
    assert!(
        outcomes(&records).contains(&(B_SLEEPS, Outcome::Passed)),
        "the running batch finishes"
    );
    let mut not_run: Vec<&str> = summary.not_run.iter().map(TestId::as_str).collect();
    not_run.sort_unstable();
    assert_eq!(not_run, ["a.py::test_after", "c.py::test_pass"]);
}

/// A batch whose status alone is 2 stops the run as `batch-stopped` does.
#[test]
fn status_2_alone_stops_the_run() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let (records, summary) = after_a_batch(&f, "a.py::test_status_2");
    assert!(summary.interrupted && !summary.internal_error);
    assert_eq!(summary.stopped, Some(StopReason::Framework));
    assert_eq!(results(&records).len(), 4, "{:?}", outcomes(&records));
    assert!(outcomes(&records).contains(&(B_SLEEPS, Outcome::Passed)));
    assert_eq!(summary.not_run, [TestId::new("c.py::test_pass")]);
}

/// Any other status alone stops nothing: a 4 is no usage error (the binary
/// finds those from their diagnostics), and a 3 (pytest's internal error in
/// `pytest_sessionfinish`, every test run) is an internal error only.
#[test]
fn other_statuses_alone_do_not_stop_the_run() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    for status in [3, 4, 7] {
        let first = format!("a.py::test_status_{status}");
        let (records, summary) = after_a_batch(&f, &first);
        assert_eq!(
            results(&records).len(),
            5,
            "{status}: {:?}",
            outcomes(&records)
        );
        assert_eq!(summary.stopped, None, "{status}");
        assert!(
            !summary.interrupted && summary.not_run.is_empty(),
            "{status}"
        );
        assert_eq!(summary.internal_error, status == 3, "{status}");
        assert_eq!(batch_of(&summary, &records, &first).status, Some(status));
    }
}

/// Whatever an executor writes, past aye's engine's cleaning, reaches the
/// runner as a record: NaN, infinities, integers beyond a double's range,
/// keys JSON can't hold and lone surrogates never make a line serde rejects.
#[test]
fn every_line_the_worker_writes_is_json_serde_reads() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["a.py::test_odd_record"]);
    assert!(!summary.internal_error, "{records:?}");
    let diagnostics: Vec<&aye_events::Diagnostic> = records
        .iter()
        .filter_map(|r| match &r.event {
            Event::Diagnostic(d) => Some(d),
            _ => None,
        })
        .collect();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code, "odd");
    assert_eq!(diagnostics[0].path, "caf\u{fffd}.py");
    assert!(diagnostics[0].message.ends_with(&"0".repeat(400)));
    assert_eq!(
        outcomes(&records),
        [("a.py::test_odd_record", Outcome::Passed)]
    );
}

/// A subprocess a test left running holds the worker's stderr open; the
/// worker's shutdown doesn't wait for it to close.
#[cfg(unix)]
#[test]
fn a_subprocess_left_running_does_not_delay_the_end_of_the_run() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let mut marks = Vec::new();
    f.runner.run(
        &Plan {
            cases: cases(&["a.py::test_spawn"]),
            ..Plan::default()
        },
        &mut Stamper::new(),
        &mut |r| marks.push((r.event.kind(), Instant::now())),
    );
    kill_pid(f.noted_pid("test_spawn"));
    let at = |kind| marks.iter().find(|(k, _)| *k == kind).unwrap().1;
    let shutdown = at("worker_finished") - at("batch_finished");
    assert!(shutdown < Duration::from_millis(700), "{shutdown:?}");
}

#[test]
fn maxfail_zero_is_no_limit() {
    let python = python_or_skip!();
    let f = fixture(python, 2);
    let plan = Plan {
        cases: cases(&["a.py::test_fail", "a.py::test_pass", "b.py::test_fail"]),
        policy: Policy {
            maxfail: Some(0),
            capture: true,
            ..Policy::default()
        },
        ..Plan::default()
    };
    let (records, summary) = f.run(&plan);
    assert_eq!(results(&records).len(), 3);
    assert_eq!((summary.counts.failed, summary.counts.passed), (2, 1));
    assert!(summary.stopped.is_none() && !summary.stopped_early);
}

#[test]
fn maxfail_records_why_the_run_stopped_and_what_never_ran() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = Plan {
        cases: cases(&["a.py::test_fail", "a.py::test_pass", "b.py::test_pass"]),
        policy: Policy {
            maxfail: Some(1),
            capture: true,
            ..Policy::default()
        },
        ..Plan::default()
    };
    let (_, summary) = f.run(&plan);
    assert_eq!(summary.stopped, Some(StopReason::Maxfail));
    let not_run: Vec<&str> = summary.not_run.iter().map(TestId::as_str).collect();
    assert_eq!(not_run, ["a.py::test_pass", "b.py::test_pass"]);
}

#[test]
fn tests_to_run_first_run_first_across_files() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let plan = Plan {
        cases: cases(&[
            "a.py::test_a1",
            "a.py::test_a2_fail",
            "b.py::test_b1",
            "b.py::test_b2_fail",
        ]),
        first: ["a.py::test_a2_fail", "b.py::test_b2_fail"]
            .into_iter()
            .map(TestId::new)
            .collect(),
        ..Plan::default()
    };
    let (records, _) = f.run(&plan);
    let order: Vec<&str> = outcomes(&records).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        order,
        [
            "a.py::test_a2_fail",
            "b.py::test_b2_fail",
            "a.py::test_a1",
            "b.py::test_b1"
        ]
    );
}

/// A worker runs one adapter's batches only, holding only its executor;
/// a lane that gets another adapter's batch starts a new worker, after
/// shutting its own down, so `workers` stays the cap on live workers.
#[test]
fn a_worker_runs_one_adapters_batches_only() {
    let python = python_or_skip!();
    for workers in [1, 2] {
        let f = fixture(python.clone(), workers);
        let mut plan_cases = Vec::new();
        for (file, adapter) in [
            ("a.py", "fake"),
            ("b.py", "other"),
            ("c.py", "fake"),
            ("d.py", "other"),
            ("e.py", "fake"),
        ] {
            plan_cases.push(Case::new(format!("{file}::test_whoami"), adapter));
        }
        let (records, summary) = f.run(&Plan {
            cases: plan_cases,
            ..Plan::default()
        });
        assert!(!summary.internal_error, "{records:?}");
        let mut seen: HashMap<String, (String, String)> = HashMap::new();
        for result in results(&records) {
            let reason = result.reason.as_deref().unwrap();
            let (pid, executors) = reason.split_once(' ').unwrap();
            let adapter = if ["a.py", "c.py", "e.py"].contains(&result.id.path().as_str()) {
                "fake"
            } else {
                "other"
            };
            assert_eq!(executors, adapter, "{workers}: {reason}");
            let (first, _) = seen
                .entry(pid.to_string())
                .or_insert_with(|| (adapter.to_string(), executors.to_string()));
            assert_eq!(first, adapter, "{workers}: worker {pid} ran both adapters");
        }
        assert_eq!(results(&records).len(), 5);
        // Never more live workers than lanes.
        let mut live = 0_i32;
        let mut most = 0;
        for record in &records {
            match record.event {
                Event::WorkerStarted { .. } => live += 1,
                Event::WorkerFinished { .. } => live -= 1,
                _ => {}
            }
            most = most.max(live);
        }
        assert_eq!(live, 0);
        assert!(most <= i32::try_from(workers).unwrap(), "{workers}: {most}");
        assert!(seen.len() >= 2);
    }
}

/// With one worker, each adapter's cases are one batch, and the worker of
/// the first is replaced for the second.
#[test]
fn one_worker_is_replaced_between_adapters() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run(&Plan {
        cases: vec![
            Case::new("a.py::test_whoami", "fake"),
            Case::new("b.py::test_whoami", "other"),
            Case::new("c.py::test_whoami", "fake"),
        ],
        ..Plan::default()
    });
    let adapters: Vec<&str> = summary.batches.iter().map(|b| b.adapter.as_str()).collect();
    assert_eq!(adapters, ["fake", "other"]);
    assert_eq!(
        kinds(&records)
            .into_iter()
            .filter(|k| k.starts_with("worker_"))
            .collect::<Vec<_>>(),
        [
            "worker_started",
            "worker_finished",
            "worker_started",
            "worker_finished"
        ]
    );
    assert!(summary.batches.iter().all(|b| b.worker == 0));
}

/// A test that replaces `os.getpid` or `json.dumps` and never puts it back
/// once made the worker drop every message as a forked child's (the run
/// hung waiting for `batch_done`) or write the stub instead of them.
#[test]
fn a_test_that_replaces_what_the_worker_uses_loses_nothing() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["a.py::test_leak", "a.py::test_fail", "a.py::test_pass"]);
    assert!(!summary.internal_error, "{records:?}");
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_leak", Outcome::Passed),
            ("a.py::test_fail", Outcome::Failed),
            ("a.py::test_pass", Outcome::Passed),
        ]
    );
    assert_eq!(find(&records, "diagnostic"), [] as [&aye_events::Record; 0]);
}

/// An id that stands for several tests, some of which ran before a crash,
/// is re-queued whole with the ids that ran, so the adapter runs only the
/// rest: none lost, none twice.
#[test]
fn a_crash_retry_runs_the_rest_of_a_partial_case_once() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run_ids(&["p.py::test_expand", "p.py::test_pass"]);
    assert_eq!(
        outcomes(&records),
        [
            ("p.py::test_expand[0]", Outcome::Passed),
            ("p.py::test_expand[1]", Outcome::Passed),
            ("p.py::test_expand[2]", Outcome::Error),
            ("p.py::test_expand[3]", Outcome::Passed),
            ("p.py::test_expand[4]", Outcome::Passed),
            ("p.py::test_pass", Outcome::Passed),
        ]
    );
    assert_eq!(summary.crashes.len(), 1);
    let ends: Vec<(BatchEnd, u32)> = summary.batches.iter().map(|b| (b.end, b.cases)).collect();
    assert_eq!(ends, [(BatchEnd::Crashed, 2), (BatchEnd::Done, 2)]);
}

/// pytest's status 1 from a batch where no test failed (a conftest's
/// `session.exitstatus`) fails the run, as pytest exits 1.
#[test]
fn status_1_without_a_bad_test_is_a_failed_run() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (_, summary) = f.run_ids(&["a.py::test_status_1"]);
    assert!(summary.framework_failed);
    assert_eq!(summary.counts.bad(), 0);
    let (_, summary) = f.run_ids(&["a.py::test_status_1", "a.py::test_fail"]);
    assert!(!summary.framework_failed);
    let (_, summary) = f.run_ids(&["a.py::test_status_0"]);
    assert!(!summary.framework_failed);
}

// ------------------------------------------------------------ session listing

/// What a `listing` worker did, in order: its `AYE_WORKER`, then `refine`
/// with whether digests only and the files, `run` with the ids, or `close`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    lane: String,
    kind: String,
    args: Vec<String>,
}

impl Fixture {
    /// Each `listing` worker's calls, by pid.
    fn calls(&self) -> HashMap<u32, Vec<Call>> {
        let mut calls = HashMap::new();
        for entry in std::fs::read_dir(self.dir()).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let Some(pid) = name
                .strip_prefix("calls-")
                .and_then(|rest| rest.strip_suffix(".log"))
            else {
                continue;
            };
            let text = std::fs::read_to_string(self.dir().join(&name)).unwrap();
            let lines = text
                .lines()
                .map(|line| {
                    let mut parts = line.split(' ').map(str::to_string);
                    Call {
                        lane: parts.next().unwrap(),
                        kind: parts.next().unwrap(),
                        args: parts.collect(),
                    }
                })
                .collect();
            calls.insert(pid.parse().unwrap(), lines);
        }
        calls
    }

    fn refine_listing(&self, files: &[&str]) -> aye_engine::Refined {
        self.runner
            .refine(&RefineRequest {
                adapter: "listing".into(),
                files: files.iter().map(|f| path(f)).collect(),
                context: serde_json::Value::Null,
                digests_only: false,
            })
            .unwrap()
    }
}

/// A `listing` case that shares no lifetime.
fn listing(id: &str) -> Case {
    Case::new(id, "listing")
}

fn session(files: &[&str]) -> std::collections::BTreeMap<String, Vec<ProjectPath>> {
    [(
        "listing".to_string(),
        files.iter().map(|f| path(f)).collect(),
    )]
    .into_iter()
    .collect()
}

fn path(file: &str) -> ProjectPath {
    ProjectPath::new(file).unwrap()
}

fn started_pids(records: &[Record]) -> HashMap<u32, u32> {
    records
        .iter()
        .filter_map(|r| match r.event {
            Event::WorkerStarted { pid, .. } => Some((r.envelope.worker.unwrap(), pid)),
            _ => None,
        })
        .collect()
}

/// Whose `close` ran: every listing worker's, once, last.
fn assert_closed_once(calls: &HashMap<u32, Vec<Call>>) {
    for (pid, calls) in calls {
        let closes = calls.iter().filter(|c| c.kind == "close").count();
        assert_eq!(closes, 1, "{pid}: {calls:?}");
        assert_eq!(calls.last().unwrap().kind, "close", "{pid}: {calls:?}");
    }
}

/// The worker that refined runs as the first lane, without listing again;
/// every other lane lists the whole session (digests only) before its
/// first batch; every worker closes its executor as it shuts down.
#[test]
fn the_worker_that_refined_is_the_first_lane_and_every_other_lane_lists_first() {
    let python = python_or_skip!();
    let f = fixture(python, 3);
    let files = ["a.py", "b.py", "c.py", "d.py"];
    let refined = f.refine_listing(&files);
    assert_eq!(refined.cases.len(), 8);
    let digests = refined
        .digests
        .expect("a session-listing executor's digests");
    assert_eq!(digests.keys().collect::<Vec<_>>(), files);
    let mut plan_cases: Vec<Case> = files
        .iter()
        .map(|f| listing(&format!("{f}::test_a")))
        .collect();
    plan_cases.push(listing("a.py::test_sleep_300"));
    let (records, summary) = f.run(&Plan {
        cases: plan_cases,
        session_files: session(&files),
        durations: durations(&[
            ("a.py::test_sleep_300", 3000),
            ("b.py::test_a", 1000),
            ("c.py::test_a", 1000),
            ("d.py::test_a", 1000),
        ]),
        ..Plan::default()
    });
    assert!(!summary.internal_error, "{records:?}");
    assert_eq!(summary.finished, 5);
    let pids = started_pids(&records);
    assert_eq!(pids.len(), 3, "{records:?}");
    let calls = f.calls();
    let first = &calls[&pids[&0]];
    assert_eq!(first[0].kind, "refine");
    assert_eq!(first[0].args, ["0", "a.py,b.py,c.py,d.py"], "{first:?}");
    assert_eq!(first[0].lane, "0");
    assert!(
        first.iter().skip(1).all(|c| c.kind != "refine"),
        "the first lane listed everything already: {first:?}"
    );
    for lane in [1, 2] {
        let calls = &calls[&pids[&lane]];
        assert_eq!(calls[0].lane, lane.to_string());
        assert_eq!(
            (calls[0].kind.as_str(), calls[0].args.clone()),
            ("refine", vec!["1".to_string(), files.join(",")]),
            "{lane}: {calls:?}"
        );
        assert!(
            calls[1..].iter().all(|c| c.kind != "refine"),
            "{lane}: {calls:?}"
        );
    }
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert_closed_once(&calls);
    assert!(find(&records, "diagnostic").is_empty(), "{records:?}");
}

/// A file a lane lists otherwise than the first lane runs there only; one
/// `listing-differs` warning names it, however many lanes found it.
#[test]
fn a_file_listed_otherwise_runs_on_the_first_lane_with_one_warning() {
    let python = python_or_skip!();
    let f = fixture(python, 3);
    let files = ["a.py", "differs.py", "z.py"];
    f.refine_listing(&files);
    let mut plan_cases = vec![listing("a.py::test_sleep_1000")];
    let mut history = vec![("a.py::test_sleep_1000".to_string(), 1000)];
    for n in 0..6 {
        let id = format!("differs.py::test_x{n}");
        history.push((id.clone(), 300));
        plan_cases.push(listing(&id));
        let id = format!("z.py::test_x{n}");
        history.push((id.clone(), 300));
        plan_cases.push(listing(&id));
    }
    let (records, summary) = f.run(&Plan {
        cases: plan_cases,
        session_files: session(&files),
        durations: history
            .iter()
            .map(|(id, ms)| (TestId::new(id.as_str()), Duration::from_millis(*ms)))
            .collect(),
        ..Plan::default()
    });
    assert!(!summary.internal_error, "{records:?}");
    assert_eq!(summary.finished, 13);
    for result in find(&records, "test_finished") {
        let Event::TestFinished(t) = &result.event else {
            unreachable!()
        };
        if t.id.path() == "differs.py" {
            assert_eq!(result.envelope.worker, Some(0), "{}", t.id);
        }
    }
    let lanes: std::collections::HashSet<u32> = find(&records, "test_finished")
        .iter()
        .filter_map(|r| r.envelope.worker)
        .collect();
    assert!(lanes.len() > 1, "z.py spreads across lanes: {lanes:?}");
    let warnings: Vec<&Record> = find(&records, "diagnostic");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    let Event::Diagnostic(warning) = &warnings[0].event else {
        unreachable!()
    };
    assert_eq!(
        (warning.code.as_str(), warning.path.as_str()),
        ("listing-differs", "differs.py")
    );
    assert_eq!(warning.severity, aye_events::Severity::Warning);
    assert!(
        warning.message.contains("worker 0 only"),
        "{}",
        warning.message
    );
}

/// A heavy file of free cases is split by duration share across lanes;
/// a file whose cases share a lifetime stays one batch.
#[test]
fn free_cases_spread_across_lanes_and_affinity_groups_stay_whole() {
    let python = python_or_skip!();
    let f = fixture(python, 4);
    let free: Vec<String> = (0..8)
        .map(|n| format!("big.py::test_sleep_200_{n}"))
        .collect();
    let free: Vec<&str> = free.iter().map(String::as_str).collect();
    let mut plan_cases = free_cases(&free);
    plan_cases.extend(cases(&[
        "kept.py::test_a",
        "kept.py::test_b",
        "kept.py::test_c",
    ]));
    let history: Vec<(&str, u64)> = free.iter().map(|id| (*id, 200)).collect();
    let (records, summary) = f.run(&Plan {
        cases: plan_cases,
        durations: durations(&history),
        ..Plan::default()
    });
    assert!(!summary.internal_error, "{records:?}");
    assert_eq!(summary.finished, 11);
    let batches_of = |path: &str| -> std::collections::HashSet<u32> {
        find(&records, "test_finished")
            .iter()
            .filter(|r| matches!(&r.event, Event::TestFinished(t) if t.id.path() == path))
            .filter_map(|r| r.envelope.batch)
            .collect()
    };
    assert!(batches_of("big.py").len() >= 4, "{:?}", summary.batches);
    assert_eq!(batches_of("kept.py").len(), 1);
    let lanes: std::collections::HashSet<u32> = find(&records, "test_finished")
        .iter()
        .filter(|r| matches!(&r.event, Event::TestFinished(t) if t.id.path() == "big.py"))
        .filter_map(|r| r.envelope.worker)
        .collect();
    assert!(lanes.len() >= 2, "{lanes:?}");
}

/// With one worker, the worker that refined runs everything in plan order,
/// listing only what it hadn't; without a refinement, the worker lists the
/// session before its first batch.
#[test]
fn one_worker_runs_everything_in_plan_order_on_the_worker_that_refined() {
    let python = python_or_skip!();
    let f = fixture(python.clone(), 1);
    f.refine_listing(&["b.py"]);
    let order = [
        "b.py::test_b",
        "b.py::test_a",
        "a.py::test_a",
        "c.py::test_a",
    ];
    let plan = Plan {
        cases: order.iter().map(|id| listing(id)).collect(),
        session_files: session(&["b.py", "a.py", "c.py"]),
        ..Plan::default()
    };
    let (records, summary) = f.run(&plan);
    assert!(!summary.internal_error, "{records:?}");
    let ran: Vec<&str> = outcomes(&records).into_iter().map(|(id, _)| id).collect();
    assert_eq!(ran, order);
    assert_eq!(summary.batches.len(), 1);
    let pids = started_pids(&records);
    assert_eq!(pids.len(), 1);
    let calls = f.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let kinds: Vec<(&str, Vec<String>)> = calls[&pids[&0]]
        .iter()
        .map(|c| (c.kind.as_str(), c.args.clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("refine", vec!["0".to_string(), "b.py".to_string()]),
            ("refine", vec!["1".to_string(), "a.py,c.py".to_string()]),
            ("run", vec![order.join(",")]),
            ("close", vec![]),
        ]
    );

    let g = fixture(python, 1);
    let (records, _) = g.run(&plan);
    let calls = g.calls();
    let pid = started_pids(&records)[&0];
    let kinds: Vec<&str> = calls[&pid].iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["refine", "run", "close"]);
    assert_eq!(calls[&pid][0].args, ["1", "b.py,a.py,c.py"]);
}

/// A worker that crashes is replaced by one that lists the session again
/// before it runs the rest.
#[test]
fn a_replacement_worker_lists_the_session_again() {
    let python = python_or_skip!();
    let f = fixture(python, 1);
    let (records, summary) = f.run(&Plan {
        cases: ["a.py::test_a", "a.py::test_crash_once", "a.py::test_b"]
            .iter()
            .map(|id| listing(id))
            .collect(),
        session_files: session(&["a.py"]),
        ..Plan::default()
    });
    assert_eq!(summary.crashes.len(), 1, "{records:?}");
    assert_eq!(
        outcomes(&records),
        [
            ("a.py::test_a", Outcome::Passed),
            ("a.py::test_crash_once", Outcome::Error),
            ("a.py::test_b", Outcome::Passed),
        ]
    );
    let calls = f.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    for calls in calls.values() {
        assert_eq!(calls[0].kind, "refine", "{calls:?}");
        assert_eq!(calls[0].args, ["1", "a.py"]);
        assert_eq!(calls[1].kind, "run", "{calls:?}");
    }
}

/// A worker kept from refinement that no run takes is shut down, its
/// executor closed: when the runner is dropped (`--co`), and when the run
/// gives the first lane another environment (fewer lanes than workers).
#[test]
fn a_worker_kept_from_refinement_and_never_used_is_closed() {
    let python = python_or_skip!();
    let f = fixture(python.clone(), 2);
    f.refine_listing(&["a.py"]);
    let Fixture { dir, runner } = f;
    drop(runner);
    let f = Fixture {
        dir,
        runner: Runner::new(RunnerOptions::default()),
    };
    // The worker that refined, and the one that listed for the other lane.
    let calls = f.calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    let mut kinds: Vec<Vec<String>> = calls
        .values()
        .map(|calls| {
            calls
                .iter()
                .map(|c| format!("{} {}", c.lane, c.kind))
                .collect()
        })
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        [["0 refine", "0 close"], ["1 refine", "1 close"]],
        "{calls:?}"
    );

    // A run with fewer lanes than workers gives its lanes other
    // environments: the workers started during refinement close unused.
    let f = fixture(python, 4);
    f.refine_listing(&["a.py", "b.py"]);
    let (records, summary) = f.run(&Plan {
        cases: cases(&["a.py::test_a", "b.py::test_a"])
            .into_iter()
            .map(|mut case| {
                case.adapter = "listing".into();
                case
            })
            .collect(),
        session_files: session(&["a.py", "b.py"]),
        ..Plan::default()
    });
    assert!(!summary.internal_error, "{records:?}");
    let pids = started_pids(&records);
    assert_eq!(pids.len(), 2);
    let calls = f.calls();
    assert_eq!(calls.len(), 6, "{calls:?}");
    let unused: Vec<&Vec<Call>> = calls
        .iter()
        .filter(|(pid, _)| !pids.values().any(|p| p == *pid))
        .map(|(_, calls)| calls)
        .collect();
    assert_eq!(unused.len(), 4);
    for calls in unused {
        let kinds: Vec<&str> = calls.iter().map(|c| c.kind.as_str()).collect();
        assert_eq!(kinds, ["refine", "close"]);
    }
    assert_closed_once(&calls);
}

/// With several workers, a worker for each other lane lists the refined
/// files (digests only) while the first lane's worker refines them, and
/// the run's lanes take them: they start ready, without listing again.
/// Each lists what every refinement refined.
#[test]
fn the_other_lanes_list_while_the_first_refines_and_the_run_takes_them() {
    let python = python_or_skip!();
    let f = fixture(python, 3);
    f.refine_listing(&["a.py", "b.py"]);
    f.refine_listing(&["c.py"]);
    // Before any run: one worker per lane, each having listed both.
    let deadline = Instant::now() + Duration::from_secs(30);
    let listed = loop {
        let calls = f.calls();
        let done = calls.len() == 3 && calls.values().all(|c| c.len() == 2);
        if done {
            break calls;
        }
        assert!(Instant::now() < deadline, "{calls:?}");
        thread::sleep(Duration::from_millis(20));
    };
    let mut by_lane: HashMap<String, u32> = HashMap::new();
    for (pid, calls) in &listed {
        let only = if calls[0].lane == "0" { "0" } else { "1" };
        assert_eq!(calls[0].args, [only, "a.py,b.py"], "{calls:?}");
        assert_eq!(calls[1].args, [only, "c.py"], "{calls:?}");
        by_lane.insert(calls[0].lane.clone(), *pid);
    }
    let files = ["a.py", "b.py", "c.py"];
    let (records, summary) = f.run(&Plan {
        cases: files
            .iter()
            .map(|f| listing(&format!("{f}::test_sleep_300")))
            .collect(),
        session_files: session(&files),
        durations: durations(&[
            ("a.py::test_sleep_300", 1000),
            ("b.py::test_sleep_300", 1000),
            ("c.py::test_sleep_300", 1000),
        ]),
        ..Plan::default()
    });
    assert!(!summary.internal_error, "{records:?}");
    assert_eq!(summary.finished, 3);
    let pids = started_pids(&records);
    for lane in 0..3u32 {
        assert_eq!(pids[&lane], by_lane[&lane.to_string()], "{records:?}");
    }
    let calls = f.calls();
    assert_eq!(calls.len(), 3, "{calls:?}");
    for calls in calls.values() {
        assert!(calls[2..].iter().all(|c| c.kind != "refine"), "{calls:?}");
    }
    assert_closed_once(&calls);
    assert!(find(&records, "diagnostic").is_empty(), "{records:?}");
}

/// Under `--co` no run follows: refinement starts no worker for the other
/// lanes.
#[test]
fn collecting_only_starts_no_worker_for_the_other_lanes() {
    let python = python_or_skip!();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let runner = Runner::new(RunnerOptions {
        python,
        root: root.clone(),
        cwd: root.clone(),
        workers: 3,
        executors: vec![LISTING],
        env: vec![("FAKE_DIR".into(), root.into_os_string())],
        collect_only: true,
        ..RunnerOptions::default()
    });
    let f = Fixture { dir, runner };
    f.refine_listing(&["a.py"]);
    let Fixture { dir, runner } = f;
    drop(runner);
    let f = Fixture {
        dir,
        runner: Runner::new(RunnerOptions::default()),
    };
    let calls = f.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let kinds: Vec<&str> = calls.values().flatten().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["refine", "close"]);
}

/// A stopped runner kills the workers still listing for other lanes rather
/// than waiting for them.
#[test]
fn a_stopped_runner_kills_the_workers_listing_for_other_lanes() {
    let python = python_or_skip!();
    let f = fixture(python, 3);
    f.refine_listing(&["slow.py"]);
    let deadline = Instant::now() + Duration::from_secs(30);
    while f.calls().len() < 3 {
        assert!(Instant::now() < deadline, "{:?}", f.calls());
        thread::sleep(Duration::from_millis(20));
    }
    let Fixture { dir, runner } = f;
    runner.stop_handle().stop();
    let started = Instant::now();
    drop(runner);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let f = Fixture {
        dir,
        runner: Runner::new(RunnerOptions::default()),
    };
    for (pid, calls) in f.calls() {
        if calls[0].lane != "0" {
            assert!(calls.iter().all(|c| c.kind != "close"), "{calls:?}");
            assert_gone(pid);
        }
    }
}

#[cfg(unix)]
fn assert_gone(pid: u32) {
    assert!(gone_within(pid, Duration::from_secs(5)), "{pid} is alive");
}

#[cfg(not(unix))]
fn assert_gone(_pid: u32) {}
