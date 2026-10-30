use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use aye_events::RunNumber;
use aye_history::{History, RunWriter};
use aye_render::Junit;
use aye_stream::{Consumer, FailOnFinish, StopAfterError, Tee};

use super::*;
use crate::printer::Printer;

/// A stream of the run `run` into `reporters`, history and the run store,
/// whose report files say they failed in `failed`.
fn stream_of<'a>(
    run: &str,
    reporters: Tee<'a>,
    failed: &Arc<AtomicBool>,
    history: Option<Remember<'a>>,
    store: Keep,
) -> Stream<'a> {
    let sink = Sink::new(reporters, Arc::clone(failed), history, store, Printer);
    Stream::new(Stamper::with_run(run), sink, Instant::now())
}

/// A reporter that fails like a closed pipe after `ok` records.
struct Closing {
    ok: usize,
    calls: Arc<AtomicU32>,
}

impl Consumer for Closing {
    fn consume(&mut self, _: &Record) -> io::Result<()> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.ok == 0 {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        self.ok -= 1;
        Ok(())
    }
}

/// Counts what it sees.
struct Counting(Arc<AtomicU32>);

impl Consumer for Counting {
    fn consume(&mut self, _: &Record) -> io::Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn a_closed_stdout_starves_no_other_reporter() {
    let stdout_calls = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(AtomicU32::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let mut stream = stream_of(
        "r",
        Tee::new()
            .with(StopAfterError::new(Closing {
                ok: 1,
                calls: Arc::clone(&stdout_calls),
            }))
            .with(Counting(Arc::clone(&seen))),
        &failed,
        None,
        Keep::new(RunWriter::new(std::env::temp_dir().join("aye-no-store"), 0)),
    );
    for _ in 0..5 {
        stream.emit(Event::CollectionStarted);
    }
    let ended = stream.finish(ExitCode::Ok, Counts::default(), None);
    assert_eq!(ended.code, ExitCode::Ok);
    // Every record, run_finished included, reached the other reporter.
    assert_eq!(seen.load(Ordering::Relaxed), 6);
    // stdout was tried until it failed, then left alone.
    assert_eq!(stdout_calls.load(Ordering::Relaxed), 2);
}

/// The run store is fed every record, `run_finished` included, after the
/// reporters, and keeps the run when the stream ends, with what its
/// summary counts beside the tests' counts.
#[test]
fn the_run_store_keeps_every_record_of_the_stream() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join(".aye/runs");
    let seen = Arc::new(AtomicU32::new(0));
    let failed = Arc::new(AtomicBool::new(false));
    let mut stream = stream_of(
        "0123456789abcdef",
        Tee::new().with(Counting(Arc::clone(&seen))),
        &failed,
        None,
        Keep::new(RunWriter::new(&runs, 10)),
    );
    stream.emit(Event::RunStarted {
        schema_version: aye_events::SCHEMA_VERSION,
        aye_version: "0.0.1".into(),
        root: dir.path().display().to_string(),
        collect_only: false,
        workers: 1,
        run: None,
    });
    stream.emit(Event::CollectionStarted);
    stream.emit(Event::TestFinished(Box::new(aye_events::TestResult::new(
        aye_events::TestId::new("t.py::T::test_x"),
        aye_events::Outcome::Passed,
    ))));
    stream.emit(Event::LifetimeFinished {
        lifetime: "t.py::T".into(),
        failure: Some(aye_events::Failure {
            phase: Some(aye_events::Phase::Teardown),
            message: "boom".into(),
            ..aye_events::Failure::default()
        }),
        duration: Duration::ZERO,
    });
    let counts = Counts {
        passed: 1,
        ..Counts::default()
    };
    stream.finish(ExitCode::TestsFailed, counts, None);
    assert_eq!(seen.load(Ordering::Relaxed), 5);
    let store = aye_history::RunStore::new(&runs);
    let meta = store.last().unwrap();
    assert_eq!(meta.run, "r1");
    assert_eq!(meta.exit_code, Some(ExitCode::TestsFailed));
    assert_eq!(
        meta.extra,
        BTreeMap::from([("teardown_error".to_owned(), 1)])
    );
    let kinds: Vec<&str> = store
        .log(&meta.run.to_string())
        .unwrap()
        .iter()
        .map(|record| record.event.kind())
        .collect();
    assert_eq!(
        kinds,
        [
            "run_started",
            "collection_started",
            "test_finished",
            "lifetime_finished",
            "run_finished"
        ]
    );
}

/// Keeps every record it sees.
struct Keeping(Arc<std::sync::Mutex<Vec<Record>>>);

impl Consumer for Keeping {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        self.0.lock().unwrap().push(record.clone());
        Ok(())
    }
}

/// The run's number, claimed before `run_started`, which says it; each bad
/// outcome's `history`, from history as it was before the run, which every
/// reporter and the run store see; history, saved with the run's outcomes.
#[test]
fn bad_outcomes_say_how_the_test_ended_before_the_run() {
    use aye_events::{Outcome, Previous, TestId, TestResult};
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".aye");
    let runs = state.join(aye_history::runs::DIR);
    let finished = |name: &str, outcome| {
        Event::TestFinished(Box::new(TestResult::new(TestId::new(name), outcome)))
    };
    // Two runs before: `test_x` passed then failed (from r1); `test_y`
    // passed.
    let mut history = History::default();
    for (number, x) in [(1, Outcome::Passed), (2, Outcome::Failed)] {
        for event in [
            Event::RunStarted {
                schema_version: aye_events::SCHEMA_VERSION,
                aye_version: "0.0.1".into(),
                root: dir.path().display().to_string(),
                collect_only: false,
                workers: 1,
                run: RunNumber::new(number),
            },
            Event::TestStarted {
                id: TestId::new("t.py::test_x"),
            },
            finished("t.py::test_x", x),
            Event::TestStarted {
                id: TestId::new("t.py::test_y"),
            },
            finished("t.py::test_y", Outcome::Passed),
            Event::RunFinished {
                exit_code: ExitCode::Ok,
                counts: Counts::default(),
                duration: Duration::ZERO,
                stopped: None,
            },
        ] {
            history.record(&Record::new(event));
        }
    }
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut writer = RunWriter::new(&runs, 10);
    let number = writer.claim();
    assert_eq!(number, RunNumber::new(1));
    let failed = Arc::new(AtomicBool::new(false));
    let mut stream = stream_of(
        "0123456789abcdef",
        Tee::new().with(Keeping(Arc::clone(&seen))),
        &failed,
        Some(Remember {
            history: &mut history,
            dir: state.clone(),
        }),
        Keep::new(writer),
    );
    stream.emit(Event::RunStarted {
        schema_version: aye_events::SCHEMA_VERSION,
        aye_version: "0.0.1".into(),
        root: dir.path().display().to_string(),
        collect_only: false,
        workers: 1,
        run: number,
    });
    stream.emit(finished("t.py::test_x", Outcome::Failed));
    // Its second result in the run: still what it was before the run.
    stream.emit(finished("t.py::test_x", Outcome::Error));
    stream.emit(finished("t.py::test_y", Outcome::Error));
    // A test without history, and one that didn't fail: none.
    stream.emit(finished("t.py::test_new", Outcome::Failed));
    stream.emit(finished("t.py::test_ok", Outcome::Passed));
    stream.finish(ExitCode::TestsFailed, Counts::default(), None);
    let said = |records: &[Record]| -> Vec<(String, Option<Previous>)> {
        records
            .iter()
            .filter_map(|record| match &record.event {
                Event::TestFinished(result) => {
                    Some((result.id.as_str().to_owned(), result.history.clone()))
                }
                _ => None,
            })
            .collect()
    };
    let x = Previous {
        outcomes: "pf".into(),
        since: RunNumber::new(2),
    };
    let y = Previous {
        outcomes: "pp".into(),
        since: None,
    };
    let expected = [
        ("t.py::test_x".to_owned(), Some(x.clone())),
        ("t.py::test_x".to_owned(), Some(x)),
        ("t.py::test_y".to_owned(), Some(y)),
        ("t.py::test_new".to_owned(), None),
        ("t.py::test_ok".to_owned(), None),
    ];
    assert_eq!(said(&seen.lock().unwrap()), expected);
    let store = aye_history::RunStore::new(&runs);
    let log = store.log("r1").unwrap();
    assert_eq!(said(&log), expected);
    assert!(
        matches!(log[0].event, Event::RunStarted { run, .. } if run == RunNumber::new(1)),
        "{:?}",
        log[0]
    );
    // History, saved: one letter for this run each; `test_y`'s streak
    // began in it.
    let saved = History::load(&state);
    let kept = |name: &str| {
        let test = saved.stats(&TestId::new(name)).unwrap();
        (test.recent.clone(), test.bad_since.map(RunNumber::get))
    };
    assert_eq!(kept("t.py::test_x"), ("pfe".into(), Some(2)));
    assert_eq!(kept("t.py::test_y"), ("ppe".into(), Some(1)));
    assert_eq!(kept("t.py::test_new"), ("f".into(), Some(1)));
}

#[test]
fn a_report_that_fails_at_the_end_fails_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let failed = Arc::new(AtomicBool::new(false));
    let blocker = dir.path().join("file");
    fs::write(&blocker, "").unwrap();
    let stream = stream_of(
        "r",
        Tee::new().with(FailOnFinish::new(
            Junit::new(blocker.join("out.xml")),
            Arc::clone(&failed),
            |err| Printer.error(err),
        )),
        &failed,
        None,
        Keep::new(RunWriter::new(std::env::temp_dir().join("aye-no-store"), 0)),
    );
    let ended = stream.finish(ExitCode::Ok, Counts::default(), None);
    assert!(failed.load(Ordering::Relaxed));
    // `run_finished` said 0; the process exits 1 (`crate::main`).
    assert_eq!(
        ended,
        RunEnd {
            code: ExitCode::Ok,
            report_failed: true
        }
    );
}
