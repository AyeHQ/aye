use std::sync::atomic::AtomicU32;

use aye_events::{BatchIndex, RunNumber};

use super::*;
use crate::positional;

#[test]
fn annotation_paths_are_relative_to_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let top = dir.path();
    fs::create_dir_all(top.join(".git")).unwrap();
    fs::create_dir_all(top.join("sub/project")).unwrap();
    let root = top.join("sub/project");
    assert_eq!(repository_prefix(&root, Some(top)), "sub/project");
    // Without GITHUB_WORKSPACE, the directory holding `.git`.
    assert_eq!(repository_prefix(&root, None), "sub/project");
    assert_eq!(repository_prefix(top, Some(top)), "");
    // A workspace that doesn't hold the project is no help.
    let other = tempfile::tempdir().unwrap();
    assert_eq!(repository_prefix(&root, Some(other.path())), "sub/project");
    let lone = tempfile::tempdir().unwrap();
    assert_eq!(repository_prefix(lone.path(), Some(other.path())), "");
}

#[test]
fn an_unwritable_report_is_found_before_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("pyproject.toml");
    fs::write(&file, "").unwrap();
    assert!(writable(&file.join("out.xml")).is_err());
    assert!(writable(dir.path()).is_err());
    let nested = dir.path().join("reports/ci/out.xml");
    writable(&nested).unwrap();
    assert!(nested.is_file());
    // An existing report isn't truncated until it is rewritten.
    fs::write(&nested, "old").unwrap();
    writable(&nested).unwrap();
    assert_eq!(fs::read_to_string(&nested).unwrap(), "old");
}

/// A reporter that fails like a closed pipe after `ok` records.
struct Closing {
    ok: usize,
    calls: Arc<AtomicU32>,
}

impl Reporter for Closing {
    fn report(&mut self, _: &Record) -> io::Result<()> {
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

impl Reporter for Counting {
    fn report(&mut self, _: &Record) -> io::Result<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[test]
fn a_closed_stdout_starves_no_other_reporter() {
    let stdout_calls = Arc::new(AtomicU32::new(0));
    let seen = Arc::new(AtomicU32::new(0));
    let mut stream = Stream {
        reporters: Tee::new()
            .with(Stdout::new(Closing {
                ok: 1,
                calls: Arc::clone(&stdout_calls),
            }))
            .with(Counting(Arc::clone(&seen))),
        history: None,
        store: Keep::new(RunWriter::new(std::env::temp_dir().join("aye-no-store"), 0)),
        stamper: Stamper::with_run("r"),
        errors: Errors::default(),
        started: Instant::now(),
        printer: Printer,
    };
    for _ in 0..5 {
        stream.emit(Event::CollectionStarted);
    }
    let failed = AtomicBool::new(false);
    let code = stream.finish(ExitCode::Ok, Counts::default(), None, &failed);
    assert_eq!(code, ExitCode::Ok);
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
    let mut stream = Stream {
        reporters: Tee::new().with(Counting(Arc::clone(&seen))),
        history: None,
        store: Keep::new(RunWriter::new(&runs, 10)),
        stamper: Stamper::with_run("0123456789abcdef"),
        errors: Errors::default(),
        started: Instant::now(),
        printer: Printer,
    };
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
    let failed = AtomicBool::new(false);
    let counts = Counts {
        passed: 1,
        ..Counts::default()
    };
    stream.finish(ExitCode::TestsFailed, counts, None, &failed);
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

impl Reporter for Keeping {
    fn report(&mut self, record: &Record) -> io::Result<()> {
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
    let mut stream = Stream {
        reporters: Tee::new().with(Keeping(Arc::clone(&seen))),
        history: Some(Remember {
            history: &mut history,
            dir: state.clone(),
        }),
        store: Keep::new(writer),
        stamper: Stamper::with_run("0123456789abcdef"),
        errors: Errors::default(),
        started: Instant::now(),
        printer: Printer,
    };
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
    let failed = AtomicBool::new(false);
    stream.finish(ExitCode::TestsFailed, Counts::default(), None, &failed);
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
    let stream = Stream {
        reporters: Tee::new().with(File {
            inner: Junit::new(blocker.join("out.xml")),
            failed: Arc::clone(&failed),
            printer: Printer,
        }),
        history: None,
        store: Keep::new(RunWriter::new(std::env::temp_dir().join("aye-no-store"), 0)),
        stamper: Stamper::with_run("r"),
        errors: Errors::default(),
        started: Instant::now(),
        printer: Printer,
    };
    let code = stream.finish(ExitCode::Ok, Counts::default(), None, &failed);
    assert!(failed.load(Ordering::Relaxed));
    assert_eq!(code, ExitCode::TestsFailed);
}

#[test]
fn ids_with_parameters_on_the_command_line_go_to_the_adapters() {
    let root = Path::new("/repo");
    let cwd = Path::new("/repo/tests");
    let positionals: Vec<Positional> = [
        "test_p.py::test_dyn[9]",
        "/repo/tests/test_p.py::TestK::test_m[a-b]",
        "test_p.py::test_plain",
        "test_p.py",
        "pkg.mod.test_x[1]",
        "../tests/test_p.py::test_dyn[9]",
    ]
    .iter()
    .map(|arg| positional::resolve(cwd, root, arg))
    .collect();
    let requested = requested_ids(&positionals);
    assert_eq!(
        requested,
        [
            TestId::new("tests/test_p.py::test_dyn[9]"),
            TestId::new("tests/test_p.py::TestK::test_m[a-b]")
        ]
    );
}

/// Round 3's #81: a rerun printed from a subdirectory that holds a path of
/// the same name as the test's names the test from there.
#[test]
fn reruns_name_the_test_from_where_aye_runs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("tests/tests")).unwrap();
    fs::write(root.join("tests/test_a.py"), "").unwrap();
    fs::write(root.join("tests/test_b.py"), "").unwrap();
    fs::write(root.join("tests/tests/test_a.py"), "").unwrap();
    let tests = root.join("tests");
    let here = Here {
        cwd: &tests,
        root,
        rerun_args: &[],
    };
    assert_eq!(
        here.local_id(&TestId::new("tests/test_a.py::test_x[1]"))
            .as_deref(),
        Some("test_a.py::test_x[1]")
    );
    // No other file of that name here: the id reruns as it is.
    assert_eq!(here.local_id(&TestId::new("tests/test_b.py::test_x")), None);
    assert_eq!(
        Here {
            cwd: root,
            root,
            rerun_args: &[],
        }
        .local_id(&TestId::new("tests/test_a.py::test_x")),
        None
    );

    let failed = |reproduce: Option<Vec<&str>>, outcome| {
        let mut result = aye_events::TestResult::new(
            aye_events::TestId::new("tests/test_a.py::test_x"),
            outcome,
        );
        result.reproduce = reproduce.map(|command| aye_events::Reproduce {
            command: command.into_iter().map(str::to_string).collect(),
            ..aye_events::Reproduce::default()
        });
        let mut record = Record::new(Event::TestFinished(Box::new(result)));
        here.rerun_from_here(&mut record);
        let Event::TestFinished(result) = record.event else {
            unreachable!()
        };
        result.reproduce.map(|r| r.command)
    };
    assert_eq!(
        failed(
            Some(vec!["aye", "tests/test_a.py::test_x", "--", "-x"]),
            aye_events::Outcome::Failed
        ),
        Some(
            ["aye", "test_a.py::test_x", "--", "-x"]
                .map(str::to_string)
                .to_vec()
        )
    );
    // An adapter that sets none gets one where `aye <id>` would mislead.
    assert_eq!(
        failed(None, aye_events::Outcome::Error),
        Some(["aye", "test_a.py::test_x"].map(str::to_string).to_vec())
    );
    assert_eq!(failed(None, aye_events::Outcome::Passed), None);

    // aye's own options come before the id (C26); the id is still found.
    assert_eq!(
        failed(
            Some(vec![
                "aye",
                "-c",
                "alt.ini",
                "tests/test_a.py::test_x",
                "--",
                "-x"
            ]),
            aye_events::Outcome::Failed
        ),
        Some(
            ["aye", "-c", "alt.ini", "test_a.py::test_x", "--", "-x"]
                .map(str::to_string)
                .to_vec()
        )
    );
    // Only the id is rewritten, not a framework argument that spells it.
    assert_eq!(
        failed(
            Some(vec![
                "aye",
                "tests/test_a.py::test_x",
                "--",
                "tests/test_a.py::test_x"
            ]),
            aye_events::Outcome::Failed
        ),
        Some(
            ["aye", "test_a.py::test_x", "--", "tests/test_a.py::test_x"]
                .map(str::to_string)
                .to_vec()
        )
    );
    let configured = Here {
        cwd: &tests,
        root,
        rerun_args: &["-c".to_string(), "alt.ini".to_string()],
    };
    let mut record = Record::new(Event::TestFinished(Box::new(aye_events::TestResult::new(
        aye_events::TestId::new("tests/test_a.py::test_x"),
        aye_events::Outcome::Failed,
    ))));
    configured.rerun_from_here(&mut record);
    let Event::TestFinished(result) = record.event else {
        unreachable!()
    };
    assert_eq!(
        result.reproduce.map(|r| r.command),
        Some(
            ["aye", "-c", "alt.ini", "test_a.py::test_x"]
                .map(str::to_string)
                .to_vec()
        )
    );
}

/// Finding F40: GNU `timeout` signals aye, then its process group, aye
/// included: two signals for one stop request, which must not kill the run
/// before it reports.
#[test]
fn a_signal_delivered_twice_is_one_stop_request() {
    let start = Instant::now();
    let mut presses = Presses::default();
    assert_eq!(presses.press(start), Press::Stop);
    // The same request again, however often it arrives within the window.
    assert_eq!(presses.press(start), Press::Repeat);
    assert_eq!(
        presses.press(start + Duration::from_millis(1)),
        Press::Repeat
    );
    assert_eq!(
        presses.press(start + REPEAT_WINDOW.saturating_sub(Duration::from_millis(1))),
        Press::Repeat
    );
    // A second request, from someone who saw the run still stopping.
    assert_eq!(presses.press(start + REPEAT_WINDOW), Press::Kill);
    assert_eq!(presses.press(start + Duration::from_secs(5)), Press::Kill);
    // A clock that seems to go back is no second request.
    let mut presses = Presses::default();
    let later = start + Duration::from_secs(1);
    assert_eq!(presses.press(later), Press::Stop);
    assert_eq!(presses.press(start), Press::Repeat);
}

/// Finding F32: a file holding a `TestCase` (the unittest adapter's) and a
/// test function (pytest's) that fails to import is one collection error
/// in pytest, which collects the file once, not one per adapter.
#[test]
fn a_file_two_adapters_load_in_one_module_is_one_collection_error() {
    let batch = |batch: u32, event: Event| {
        let mut record = Record::new(event);
        record.envelope.batch = Some(BatchIndex::from_u32(batch));
        record
    };
    let started = |id: u32, adapter: &'static str| {
        batch(
            id,
            Event::BatchStarted {
                adapter: adapter.into(),
                cases: 1,
            },
        )
    };
    let error = |id: u32, path: &str| {
        batch(
            id,
            Event::Diagnostic(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::CollectionError,
                path: path.into(),
                span: None,
                message: format!("cannot import {path}"),
            }),
        )
    };
    let severity = |record: &Record| match &record.event {
        Event::Diagnostic(d) => d.severity,
        _ => unreachable!(),
    };
    let mut once = OneModuleErrors::default();
    for (id, adapter) in [(0, "unittest"), (1, "pytest"), (2, "doctest")] {
        once.see(&mut started(id, adapter));
    }
    let mut seen = |id: u32, path: &str| {
        let mut record = error(id, path);
        once.see(&mut record);
        record
    };
    let first = seen(0, "test_imp.py");
    assert_eq!(severity(&first), Severity::Error);
    let again = seen(1, "test_imp.py");
    assert_eq!(severity(&again), Severity::Info);
    let Event::Diagnostic(info) = &again.event else {
        unreachable!()
    };
    assert!(
        info.message
            .starts_with("one collection error with the unittest adapter's"),
        "{}",
        info.message
    );
    assert!(info.message.ends_with("cannot import test_imp.py"));
    // pytest's doctest items are another collector: the doctest adapter's
    // error stays, and so does a second one pytest's batch reports for its
    // own doctest collector (without the doctest adapter), beyond what the
    // unittest adapter reported.
    assert_eq!(severity(&seen(2, "test_imp.py")), Severity::Error);
    assert_eq!(severity(&seen(1, "test_imp.py")), Severity::Error);
    assert_eq!(severity(&seen(0, "test_imp.py")), Severity::Info);
    // Another file is another error.
    assert_eq!(severity(&seen(1, "test_other.py")), Severity::Error);
    // One adapter alone keeps all its errors.
    assert_eq!(severity(&seen(1, "test_other.py")), Severity::Error);
    // Whichever batch comes first, the file has as many errors as the
    // adapter reporting the most for it.
    assert_eq!(severity(&seen(1, "test_two.py")), Severity::Error);
    assert_eq!(severity(&seen(1, "test_two.py")), Severity::Error);
    assert_eq!(severity(&seen(0, "test_two.py")), Severity::Info);
}

/// aye's pytest front-end lists every pytest file in refinement: a file
/// that fails to import has pytest's error from collection, and the
/// unittest adapter's batch error for it is the same `Module`'s.
#[test]
fn a_collection_error_from_refinement_counts_as_the_pytest_adapters() {
    let error = |path: &str| Diagnostic {
        severity: Severity::Error,
        code: DiagnosticCode::CollectionError,
        path: path.into(),
        span: None,
        message: format!("cannot import {path}"),
    };
    let mut once = OneModuleErrors::default();
    once.refined(&aye_adapter_pytest::NAME, &error("test_imp.py"));
    // A conftest's, or anything but an error, is another matter.
    once.refined(&aye_adapter_pytest::NAME, &error("conftest.py"));
    once.refined(
        &aye_adapter_pytest::NAME,
        &Diagnostic {
            severity: Severity::Warning,
            ..error("test_warn.py")
        },
    );
    let mut started = Record::new(Event::BatchStarted {
        adapter: "unittest".into(),
        cases: 1,
    });
    started.envelope.batch = Some(BatchIndex::from_u32(3));
    once.see(&mut started);
    let severity = |path: &str, once: &mut OneModuleErrors| {
        let mut record = Record::new(Event::Diagnostic(error(path)));
        record.envelope.batch = Some(BatchIndex::from_u32(3));
        once.see(&mut record);
        match record.event {
            Event::Diagnostic(d) => (d.severity, d.message),
            _ => unreachable!(),
        }
    };
    let (seen, message) = severity("test_imp.py", &mut once);
    assert_eq!(seen, Severity::Info);
    assert!(
        message.starts_with("one collection error with the pytest adapter's"),
        "{message}"
    );
    assert_eq!(severity("conftest.py", &mut once).0, Severity::Error);
    assert_eq!(severity("test_warn.py", &mut once).0, Severity::Error);
}

/// Recheck regressions 2 and 4: dotted names and keys are resolved by
/// collecting the whole project, whose refinement reports on all of it
/// from the framework's default start. What the run reaches decides which
/// of collection's diagnostics are the run's, and the paths it starts from
/// go to the adapters.
#[test]
fn what_dotted_names_reach_decides_what_collection_reports() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        "tests/test_a.py",
        "tests/sub/test_s.py",
        "testing/test_g.py",
        "other/test_o.py",
    ] {
        fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        fs::write(root.join(file), "").unwrap();
    }
    let positionals = |args: &[&str]| -> Vec<Positional> {
        args.iter()
            .map(|arg| positional::resolve(root, root, arg))
            .collect()
    };
    let cases: Vec<Case> = [
        "tests/test_a.py::TestX::test_y",
        "tests/test_a.py::TestX::test_z",
        "testing/test_g.py::test_g",
        "other/test_o.py::test_o",
    ]
    .iter()
    .map(|id| Case::new(*id, "pytest"))
    .collect();
    let diagnostic = |severity, code: DiagnosticCode, path: &str| Diagnostic {
        severity,
        code,
        path: path.into(),
        span: None,
        message: "m".to_string(),
    };
    let skipped = |path: &str| diagnostic(Severity::Info, DiagnosticCode::Skipped, path);
    let usage = |path: &str| diagnostic(Severity::Error, DiagnosticCode::UsageError, path);

    // No argument, or only paths: collection walked only what the run
    // reaches, and everything is the run's.
    assert_eq!(Reach::of(&[], root, &cases), None);
    assert_eq!(
        Reach::of(&positionals(&["tests", "other/test_o.py"]), root, &cases),
        None
    );

    // A dotted name: the files of the cases it selects.
    let reach = Reach::of(&positionals(&["tests.test_a"]), root, &cases).unwrap();
    assert_eq!(reach.paths, Vec::<PathBuf>::new());
    assert_eq!(reach.selected, [root.join("tests/test_a.py")]);
    // A skipped directory or module elsewhere is not counted.
    assert!(!reach.keeps(root, &skipped("tests/sub")));
    assert!(!reach.keeps(root, &skipped("testing/test_g.py")));
    // A conftest pytest loads first only in its default start isn't the
    // run's; one on the way to a selected file is.
    assert!(!reach.keeps(root, &usage("testing/conftest.py")));
    assert!(reach.keeps(root, &usage("tests/conftest.py")));
    assert!(reach.keeps(root, &usage("conftest.py")));
    // Nor is a conftest above the root.
    assert!(reach.keeps(root, &usage("../conftest.py")));
    // pytest rejecting its arguments is always the run's, and so is
    // everything else.
    assert!(reach.keeps(root, &usage("")));
    let error = diagnostic(
        Severity::Error,
        DiagnosticCode::CollectionError,
        "testing/test_g.py",
    );
    assert!(reach.keeps(root, &error));
    let info = diagnostic(Severity::Info, DiagnosticCode::NoTests, "testing/test_g.py");
    assert!(reach.keeps(root, &info));
    // When an argument named nothing, an error elsewhere may be why.
    let lost = Reach {
        unmatched: true,
        ..reach.clone()
    };
    assert!(lost.keeps(root, &usage("testing/conftest.py")));
    assert!(!lost.keeps(root, &skipped("tests/sub")));

    // A dotted name reaches the skipped module or package it names.
    let reach = Reach::of(&positionals(&["testing.test_g"]), root, &cases).unwrap();
    assert!(reach.keeps(root, &skipped("testing/test_g.py")));
    assert!(!reach.keeps(root, &skipped("tests/sub")));

    // Paths beside them: what a path holds is reached, in the order of the
    // arguments; a key resolves like a dotted name.
    let key = cases[3].id.key().to_string();
    let reach = Reach::of(
        &positionals(&["tests", &key, "tests.test_a", "testing/test_g.py::test_g"]),
        root,
        &cases,
    )
    .unwrap();
    assert_eq!(
        reach.paths,
        [root.join("tests"), root.join("testing/test_g.py")]
    );
    assert_eq!(
        reach.selected,
        [
            root.join("tests"),
            root.join("other/test_o.py"),
            root.join("tests/test_a.py"),
            root.join("testing/test_g.py"),
        ]
    );
    assert!(reach.keeps(root, &skipped("tests/sub")));
    assert!(reach.keeps(root, &skipped("testing/test_g.py")));
    assert!(!reach.keeps(root, &skipped("other")));
    assert!(reach.keeps(root, &usage("tests/sub/conftest.py")));
    assert!(reach.keeps(root, &usage("other/conftest.py")));
    assert!(reach.keeps(root, &usage("testing/conftest.py")));
}

/// Recheck regression 3: one conftest that fails at start-up was two usage
/// errors with the default adapters, the doctest adapter's and pytest's,
/// worded apart. It is one error, whatever the words.
#[test]
fn a_conftest_two_adapters_fail_to_load_is_one_error() {
    let diagnostic = |code: DiagnosticCode, path: &str, message: &str| Diagnostic {
        severity: Severity::Error,
        code,
        path: path.into(),
        span: None,
        message: message.to_string(),
    };
    let mut once = OneConftestError::default();
    // In refinement, each adapter reports it once.
    let mut first = diagnostic(
        DiagnosticCode::UsageError,
        "tests/conftest.py",
        "Skipped while …",
    );
    once.see(None, &mut first);
    assert_eq!(first.severity, Severity::Error);
    let mut again = diagnostic(
        DiagnosticCode::UsageError,
        "tests/conftest.py",
        "ImportError while …",
    );
    once.see(None, &mut again);
    assert_eq!(again.severity, Severity::Info);
    assert_eq!(
        again.message,
        "also reported by another adapter, one usage error as pytest loads a conftest \
         once, in its words: ImportError while …"
    );
    // Another code, another file, or a file that isn't a conftest, is
    // another error.
    for (code, path) in [
        (DiagnosticCode::CollectionError, "tests/conftest.py"),
        (DiagnosticCode::UsageError, "other/conftest.py"),
        (DiagnosticCode::CollectionError, "tests/test_imp.py"),
        (DiagnosticCode::CollectionError, "tests/test_imp.py"),
        (DiagnosticCode::UsageError, ""),
        (DiagnosticCode::UsageError, ""),
    ] {
        let mut other = diagnostic(code.clone(), path, "x");
        once.see(None, &mut other);
        assert_eq!(other.severity, Severity::Error, "{code} {path}");
    }

    // In batches: one adapter's repeats stay (renderers show them once),
    // another adapter's is an info.
    let batch = |batch: u32, event: Event| {
        let mut record = Record::new(event);
        record.envelope.batch = Some(BatchIndex::from_u32(batch));
        record
    };
    let mut once = OneConftestError::default();
    for (id, adapter) in [(0, "pytest"), (1, "pytest"), (2, "doctest")] {
        once.see_record(&mut batch(
            id,
            Event::BatchStarted {
                adapter: adapter.into(),
                cases: 1,
            },
        ));
    }
    let mut seen = |id: u32, message: &str| {
        let mut record = batch(
            id,
            Event::Diagnostic(diagnostic(
                DiagnosticCode::CollectionError,
                "sub/conftest.py",
                message,
            )),
        );
        once.see_record(&mut record);
        let Event::Diagnostic(diagnostic) = record.event else {
            unreachable!()
        };
        diagnostic
    };
    assert_eq!(seen(0, "Failed while …").severity, Severity::Error);
    assert_eq!(seen(1, "Failed while …").severity, Severity::Error);
    let info = seen(2, "Failed while …");
    assert_eq!(info.severity, Severity::Info);
    assert_eq!(
        info.message,
        "also reported by the doctest adapter, one collection error as pytest loads a \
         conftest once"
    );
}

/// pytest counts a skipped module or package that `--pyargs` names, and
/// finds it; a test in it, or a module in a skipped directory, it counts
/// the skip for but doesn't find.
#[test]
fn a_dotted_name_reaches_the_skips_of_what_it_names() {
    for (word, path, named) in [
        ("other.test_mod", "other/test_mod.py", Named::Whole),
        ("other.test_mod.__init__", "other/test_mod.py", Named::Whole),
        ("other", "other/test_mod.py", Named::Whole),
        ("tests.sub", "tests/sub", Named::Whole),
        ("tests", "tests/sub", Named::Whole),
        ("other.test_mod.test_m", "other/test_mod.py", Named::Part),
        (
            "other.test_mod.Class.test_m",
            "other/test_mod.py",
            Named::Part,
        ),
        ("tests.sub.test_s", "tests/sub", Named::Part),
        ("tests.sub.deep.test_s.test_x", "tests/sub", Named::Part),
        ("tests.test_a", "tests/sub", Named::Nothing),
        ("tests.test_a.TestX", "tests/test_a.py.bak", Named::Nothing),
        ("tests.subway", "tests/sub", Named::Nothing),
        ("other.test_mod", "other/test_mod_two.py", Named::Nothing),
        ("cd12fbc3", "tests/sub", Named::Nothing),
        ("tests.sub::x", "tests/sub", Named::Nothing),
        ("tests..sub", "tests/sub", Named::Nothing),
    ] {
        assert_eq!(dotted_reach(word, path), named, "{word} {path}");
    }
}

/// A file below a conftest that failed to load has no tests because of it,
/// which the conftest's error says: collection's `no-tests` note, which
/// says only that the file has none, is left out (it showed beside a
/// start-up conftest's usage error, and beside a directory's collection
/// error, where pytest reports the error alone).
#[test]
fn a_file_below_a_broken_conftest_gets_no_no_tests_note() {
    let diagnostic = |severity: Severity, code: DiagnosticCode, path: &str| Diagnostic {
        severity,
        code,
        path: path.into(),
        span: None,
        message: String::new(),
    };
    let collected = [
        diagnostic(
            Severity::Error,
            DiagnosticCode::UsageError,
            "tests/conftest.py",
        ),
        diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "sub/deep/conftest.py",
        ),
        // Not a conftest's: no directory is explained by it.
        diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "other/test_x.py",
        ),
        // A conftest's info explains nothing either.
        diagnostic(
            Severity::Info,
            DiagnosticCode::UsageError,
            "more/conftest.py",
        ),
    ];
    let broken = broken_conftest_dirs(&collected);
    assert_eq!(broken, ["tests/", "sub/deep/"]);
    let explained = |path: &str| {
        explained_by_a_conftest(
            &broken,
            &diagnostic(Severity::Info, DiagnosticCode::NoTests, path),
        )
    };
    assert!(explained("tests/test_t.py"));
    assert!(explained("tests/pkg/test_p.py"));
    assert!(explained("sub/deep/test_d.py"));
    assert!(!explained("tests2/test_t.py"));
    assert!(!explained("sub/test_s.py"));
    assert!(!explained("other/test_y.py"));
    assert!(!explained("more/test_m.py"));
    // Only the `no-tests` info.
    assert!(!explained_by_a_conftest(
        &broken,
        &diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "tests/test_t.py"
        )
    ));
    // A root conftest's failure explains every file.
    let root = [diagnostic(
        Severity::Error,
        DiagnosticCode::UsageError,
        "conftest.py",
    )];
    let broken = broken_conftest_dirs(&root);
    assert!(explained_by_a_conftest(
        &broken,
        &diagnostic(Severity::Info, DiagnosticCode::NoTests, "test_top.py")
    ));
}

#[test]
fn the_session_is_every_file_with_a_case_by_adapter_in_collection_order() {
    let cases: Vec<Case> = [
        ("b.py::test_x", "pytest"),
        ("b.py::test_y", "pytest"),
        ("b.py::Doc", "doctest"),
        ("a.py::test_x", "pytest"),
        ("b.py::test_z", "pytest"),
        ("c.txt::c.txt", "doctest"),
    ]
    .into_iter()
    .map(|(id, adapter)| Case::new(id, adapter))
    .collect();
    let files = session_files(&cases, None);
    assert_eq!(files.len(), 2);
    assert_eq!(files["pytest"], ["b.py", "a.py"]);
    assert_eq!(files["doctest"], ["b.py", "c.txt"]);
    assert!(session_files(&[], None).is_empty());
}

/// With dotted names or keys, collection walks the whole project, but the
/// session is what `pytest <the paths the run starts from>` collects: a
/// file elsewhere (whose directory's conftest that run never loads) isn't
/// listed.
#[test]
fn with_dotted_names_the_session_is_what_the_run_starts_from() {
    let root = Path::new("/repo");
    let cases: Vec<Case> = [
        ("testing/test_g.py::test_g", "pytest"),
        ("tests/test_a.py::test_a", "pytest"),
        ("tests/sub/test_s.py::test_s", "pytest"),
        ("tests2/test_t.py::test_t", "pytest"),
        ("lib.py::lib.f", "doctest"),
    ]
    .into_iter()
    .map(|(id, adapter)| Case::new(id, adapter))
    .collect();
    let starts = [root.join("tests"), root.join("lib.py")];
    let files = session_files(&cases, Some((&starts, root)));
    assert_eq!(files["pytest"], ["tests/test_a.py", "tests/sub/test_s.py"]);
    assert_eq!(files["doctest"], ["lib.py"]);
}
