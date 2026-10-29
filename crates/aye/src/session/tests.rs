use std::sync::atomic::AtomicU32;

use super::*;

fn selector(cwd: &Path, root: &Path, arg: &str) -> String {
    resolve(cwd, root, arg).selector
}

#[test]
fn selectors_are_made_relative_to_the_root() {
    let root = Path::new("/repo");
    let cwd = Path::new("/repo/tests");
    assert_eq!(
        selector(cwd, root, "test_a.py::test_x[1]"),
        "tests/test_a.py::test_x[1]"
    );
    assert_eq!(selector(cwd, root, "../src/"), "src");
    assert_eq!(selector(cwd, root, "pkg.mod.TestX"), "pkg.mod.TestX");
    assert_eq!(selector(root, root, "."), ".");
}

#[test]
fn paths_outside_the_root_climb_out_instead_of_doubling_the_slash() {
    let root = Path::new("/repo/sub");
    assert_eq!(selector(root, root, "../x/test_a.py"), "../x/test_a.py");
    assert_eq!(relative_below_root(Path::new("/repo/x"), root), "../x");
    assert_eq!(relative_below_root(Path::new("/repo/sub/a/b"), root), "a/b");
}

#[test]
fn a_path_only_the_root_has_is_taken_from_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("tests/sub")).unwrap();
    fs::write(root.join("tests/test_a.py"), "").unwrap();
    let cwd = root.join("tests");
    // From tests/, `tests/test_a.py` is the root's: the id aye printed.
    let id = resolve(&cwd, root, "tests/test_a.py::test_fail");
    assert_eq!(id.selector, "tests/test_a.py::test_fail");
    assert_eq!(
        id.path.as_deref(),
        Some(root.join("tests/test_a.py").as_path())
    );
    assert!(id.exists() && id.id);
    // The current directory comes first, as in pytest.
    assert_eq!(selector(&cwd, root, "test_a.py"), "tests/test_a.py");
    // A bare word naming a top-level directory, from below.
    let word = resolve(&root.join("tests/sub"), root, "tests");
    assert_eq!(word.selector, "tests");
    assert!(word.exists() && !word.id);
    // Neither has it: kept relative to the current directory.
    let missing = resolve(&cwd, root, "nope.py");
    assert_eq!(missing.selector, "tests/nope.py");
    assert!(!missing.exists());
    // Not a path: a dotted name or a key.
    let dotted = resolve(&cwd, root, "pkg.mod::x");
    assert_eq!(dotted.path, None);
    assert!(dotted.id);
}

#[test]
fn a_root_reached_through_dot_dot_is_normalized() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("pyproject.toml"), "").unwrap();
    fs::write(root.join("tests/test_b.py"), "").unwrap();
    for (cwd, arg, want) in [
        (root.join("tests"), "..", "."),
        (root.join("src"), "../tests/test_b.py", "tests/test_b.py"),
        (
            root.clone(),
            "tests/../tests/test_b.py::t",
            "tests/test_b.py::t",
        ),
    ] {
        let given = [normalize_absolute(&cwd.join(path_part(arg)))];
        let found = normalize_absolute(&aye_config::find_root(&aye_fs::common_ancestor(
            &cwd, &given,
        )));
        assert_eq!(found, root, "{arg} from {}", cwd.display());
        assert_eq!(selector(&cwd, &found, arg), want);
    }
}

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
        .log(&meta.run)
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
                run: Some(number),
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
    assert_eq!(number, Some(1));
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
        since: Some(2),
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
        matches!(log[0].event, Event::RunStarted { run: Some(1), .. }),
        "{:?}",
        log[0]
    );
    // History, saved: one letter for this run each; `test_y`'s streak
    // began in it.
    let saved = History::load(&state);
    let kept = |name: &str| {
        let test = saved.stats(&TestId::new(name)).unwrap();
        (test.recent.clone(), test.bad_since)
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
        }),
        history: None,
        store: Keep::new(RunWriter::new(std::env::temp_dir().join("aye-no-store"), 0)),
        stamper: Stamper::with_run("r"),
        errors: Errors::default(),
        started: Instant::now(),
    };
    let code = stream.finish(ExitCode::Ok, Counts::default(), None, &failed);
    assert!(failed.load(Ordering::Relaxed));
    assert_eq!(code, ExitCode::TestsFailed);
}

#[test]
fn diagnostic_codes_choose_the_exit_code() {
    let diagnostic = |severity, code: &str| Diagnostic {
        severity,
        code: code.into(),
        path: DiagnosticPath::Nowhere,
        span: None,
        message: String::new(),
    };
    let mut errors = Errors::default();
    errors.see(&diagnostic(Severity::Warning, "usage-error"));
    assert!(!errors.any && !errors.usage);
    errors.see(&diagnostic(Severity::Error, "collection-error"));
    assert!(errors.any && !errors.usage && !errors.internal);
    errors.see(&diagnostic(Severity::Error, "usage-error"));
    assert!(errors.usage);
    errors.see(&diagnostic(Severity::Error, "internal-error"));
    assert!(errors.internal);
    let mut errors = Errors::default();
    errors.see(&diagnostic(Severity::Error, "unclaimed"));
    assert!(errors.usage);
    errors.see(&diagnostic(Severity::Error, "not-found"));
    assert!(errors.not_found);
}

/// Every diagnostic code aye 0.0.1 knows, and one it doesn't, with what an
/// error with it decides of the exit code, as 0.0.1 decides it, before any
/// batch and from one: `usage` (4), `internal` (3), or `error`, which fails
/// the run as the rest of it says (2, or 1). Collection's diagnostics come
/// before any batch.
const EXIT: [(&str, &str, &str); 26] = [
    ("syntax-error", "error", "error"),
    ("unreadable", "error", "error"),
    ("not-found", "usage", "error"),
    ("unclaimed", "usage", "usage"),
    ("duplicate-id", "error", "error"),
    ("no-tests", "error", "error"),
    ("not-refined", "error", "error"),
    ("refine-failed", "error", "error"),
    ("class-has-init", "error", "error"),
    ("duplicate-definition", "error", "error"),
    ("collection-error", "error", "error"),
    ("usage-error", "usage", "usage"),
    ("unknown-option", "error", "error"),
    ("plugin-not-loaded", "error", "error"),
    ("internal-error", "internal", "internal"),
    ("skipped", "error", "error"),
    ("ignored", "error", "error"),
    ("batch-stopped", "error", "error"),
    ("session-failed", "error", "error"),
    ("protocol-error", "error", "error"),
    ("batch-failed", "error", "error"),
    ("no-executor", "error", "error"),
    ("listing-differs", "error", "error"),
    ("stand-in", "error", "error"),
    ("adapter-error", "error", "error"),
    ("some-other-code", "error", "error"),
];

/// An error diagnostic with each code decides the exit code as in 0.0.1:
/// collection's (before any batch, where a `not-found` names what the
/// command line asked for) as `run` decides on them before the run, and
/// a batch's as `run_exit_code` does after it.
#[test]
fn every_code_decides_the_exit_code_as_before() {
    for (code, before, from_batch) in EXIT {
        let mut errors = Errors::default();
        errors.see(&Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            path: DiagnosticPath::Nowhere,
            span: None,
            message: String::new(),
        });
        assert!(errors.any, "{code}");
        let collecting = if errors.internal {
            "internal"
        } else if errors.not_found || errors.usage {
            "usage"
        } else {
            "error"
        };
        let running = if errors.internal {
            "internal"
        } else if errors.usage {
            "usage"
        } else {
            "error"
        };
        assert_eq!((collecting, running), (before, from_batch), "{code}");
    }
}

/// A recognizer that collects nothing and hands out `context`.
struct Context(serde_json::Value);

impl aye_engine::Recognizer for Context {
    fn roots(&self, _: &Project<'_>) -> Vec<PathBuf> {
        Vec::new()
    }

    fn descend(&self, _: &Path) -> bool {
        false
    }

    fn claim(&self, _: &str, _: bool) -> aye_engine::Claim {
        aye_engine::Claim::Ignore
    }

    fn recognize(
        &self,
        _: &aye_engine::SourceFile<'_>,
        _: &[aye_engine::SourceFile<'_>],
    ) -> aye_engine::Recognition {
        aye_engine::Recognition::default()
    }

    fn context(&self) -> serde_json::Value {
        self.0.clone()
    }
}

/// Adapters with these contexts, named `pytest`, `doctest` and `unittest`
/// in turn.
fn prepared(contexts: &[serde_json::Value]) -> Vec<Prepared> {
    contexts
        .iter()
        .zip(["pytest", "doctest", "unittest"])
        .map(|(context, name)| {
            let recognizer: Box<dyn aye_engine::Recognizer> = Box::new(Context(context.clone()));
            (name, recognizer)
        })
        .collect()
}

fn args(argv: &[&str]) -> Args {
    use clap::Parser as _;
    Args::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
}

/// The `-k` and `-m` texts static selection applies to the first
/// adapter's (pytest's) cases.
fn expressions(argv: &[&str], contexts: &[serde_json::Value]) -> (Option<String>, Option<String>) {
    expressions_for("pytest", argv, contexts)
}

/// The `-k` and `-m` texts static selection applies to `adapter`'s cases.
fn expressions_for(
    adapter: &str,
    argv: &[&str],
    contexts: &[serde_json::Value],
) -> (Option<String>, Option<String>) {
    let selection = selection(&args(argv), [], &prepared(contexts)).unwrap();
    let text = |e: Option<&aye_select::Expression>| e.map(|e| e.as_str().to_string());
    (
        text(
            selection
                .keyword
                .as_ref()
                .or_else(|| selection.own_keyword.get(adapter)),
        ),
        text(
            selection
                .tags
                .as_ref()
                .or_else(|| selection.own_tags.get(adapter)),
        ),
    )
}

fn pair(keyword: Option<&str>, tags: Option<&str>) -> (Option<String>, Option<String>) {
    (keyword.map(str::to_string), tags.map(str::to_string))
}

#[test]
fn the_frameworks_own_expressions_apply_when_the_command_line_gives_none() {
    let addopts = serde_json::json!({"keyword": "not slow", "tags": "db", "maxfail": null});
    let both = std::slice::from_ref(&addopts);
    assert_eq!(expressions(&[], both), pair(Some("not slow"), Some("db")));
    // Each one separately: the command line's wins where it is given.
    assert_eq!(
        expressions(&["-k", "fast"], both),
        pair(Some("fast"), Some("db"))
    );
    assert_eq!(
        expressions(&["-m", "net"], both),
        pair(Some("not slow"), Some("net"))
    );
    // An empty expression turns the framework's off, as in pytest.
    assert_eq!(expressions(&["-k", ""], both), pair(None, Some("db")));
    // The arguments after `--` come last in pytest's arguments: theirs wins.
    assert_eq!(
        expressions(&["-k", "fast", "--", "-k", "quick", "-vmnet"], both),
        pair(Some("quick"), Some("net"))
    );
    // When they can't be read, the command line's, else the framework's
    // own: nothing re-applies an expression in a batch.
    assert_eq!(
        expressions(&["-k", "fast", "--", "@more-args"], both),
        pair(Some("fast"), Some("db"))
    );
    assert_eq!(
        expressions(&["--", "-Zk", "x"], both),
        pair(Some("not slow"), Some("db"))
    );
    // Nothing in the context, or something that isn't text.
    assert_eq!(
        expressions(&[], &[serde_json::Value::Null]),
        pair(None, None)
    );
    assert_eq!(
        expressions(&[], &[serde_json::json!({"keyword": 3, "tags": null})]),
        pair(None, None)
    );
    // Each adapter's own applies to its own cases: doctest's context, which
    // names none, leaves its cases alone.
    let other = serde_json::json!({"keyword": "not slow", "tags": "net"});
    let two = [addopts.clone(), other];
    assert_eq!(expressions(&[], &two), pair(Some("not slow"), Some("db")));
    assert_eq!(
        expressions_for("doctest", &[], &two),
        pair(Some("not slow"), Some("net"))
    );
    let silent = [addopts.clone(), serde_json::json!({"rootdir": "/x"})];
    assert_eq!(expressions_for("doctest", &[], &silent), pair(None, None));
    assert_eq!(
        expressions(&[], &silent),
        pair(Some("not slow"), Some("db"))
    );
    // The command line's apply to every adapter's cases.
    assert_eq!(
        expressions_for("doctest", &["-k", "fast"], &silent),
        pair(Some("fast"), None)
    );
}

#[test]
fn only_the_command_lines_malformed_expressions_are_usage_errors() {
    let malformed = serde_json::json!({"keyword": "not (", "tags": "a and"});
    // The framework's own: left for it to report, and not applied.
    assert_eq!(
        expressions(&[], std::slice::from_ref(&malformed)),
        pair(None, None)
    );
    assert_eq!(expressions(&["--", "-k", "or"], &[]), pair(None, None));
    // The command line's: an error, even where `--` overrides it.
    for argv in [
        &["-k", "not ("][..],
        &["-m", "a and"],
        &["-k", "not (", "--", "-k", "x"],
    ] {
        assert!(
            selection(&args(argv), [], &prepared(&[])).is_err(),
            "{argv:?}"
        );
    }
}

#[test]
fn the_junit_report_can_come_from_the_framework() {
    let cwd = Path::new("/work/sub");
    let context = |path: &str| prepared(&[serde_json::json!({ "junitxml": path })]);
    assert_eq!(
        junit_path(cwd, None, &context("reports/out.xml")),
        Some(PathBuf::from("/work/sub/reports/out.xml"))
    );
    // The command line's wins.
    assert_eq!(
        junit_path(
            cwd,
            Some(Path::new("mine.xml")),
            &context("reports/out.xml")
        ),
        Some(PathBuf::from("/work/sub/mine.xml"))
    );
    assert_eq!(
        junit_path(cwd, None, &context("/abs/out.xml")),
        Some(PathBuf::from("/abs/out.xml"))
    );
    // pytest writes no report for an empty path.
    assert_eq!(junit_path(cwd, None, &context("")), None);
    assert_eq!(
        junit_path(cwd, None, &prepared(&[serde_json::Value::Null])),
        None
    );
    assert_eq!(junit_path(cwd, None, &[]), None);
    // The command line's is read as pytest reads it; the framework's is
    // the path the adapter worked out.
    if let Some(home) = home() {
        assert_eq!(
            junit_path(cwd, Some(Path::new("~/r.xml")), &[]),
            Some(Path::new(&home).join("r.xml"))
        );
    }
    assert_eq!(
        junit_path(cwd, Some(Path::new("")), &context("x.xml")),
        None
    );
    assert_eq!(
        junit_path(cwd, None, &context("$HOME/r.xml")),
        Some(PathBuf::from("/work/sub/$HOME/r.xml"))
    );
    // pytest's `--junitprefix` comes with it.
    assert_eq!(
        junit_prefix(&prepared(&[
            serde_json::Value::Null,
            serde_json::json!({ "junit_prefix": "ci" })
        ])),
        "ci"
    );
    assert_eq!(junit_prefix(&context("x.xml")), "");
}

/// The home directory `~` stands for, as [`aye_fs::expand_path`] finds it.
fn home() -> Option<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
}

#[cfg(unix)]
#[test]
fn a_path_through_another_name_of_the_root_is_spelled_from_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("real");
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(root.join("tests/test_a.py"), "").unwrap();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    // As macOS's `/tmp` is `/private/tmp`: the current directory reads as
    // the real path, the argument names the other.
    let given = alias.join("tests/test_a.py");
    let arg = format!("{}::test_x", given.display());
    let id = resolve(&root, &root, &arg);
    assert_eq!(id.selector, "tests/test_a.py::test_x");
    assert!(id.exists());
    assert_eq!(root_relative(&alias.join("tests"), &root), "tests");
    // Outside the root either way: climbs out, as before.
    assert_eq!(
        root_relative(&dir.path().join("elsewhere/x.py"), &root),
        "../elsewhere/x.py"
    );
}

#[test]
fn every_output_the_configuration_accepts_parses() {
    for name in aye_config::OUTPUTS {
        assert!(name.parse::<Output>().is_ok(), "{name}");
        assert!(name.to_uppercase().parse::<Output>().is_ok(), "{name}");
        assert_eq!(name.parse::<Output>().unwrap().name(), name);
    }
}

/// `--output`, else `[tool.aye] output`, else text, and where it came
/// from, as `aye config` shows it.
#[test]
fn the_output_is_the_command_lines_else_the_configurations_else_text() {
    let config = |output: Option<&str>| AyeConfig {
        settings: aye_config::AyeSettings {
            output: output.map(str::to_string),
            ..Default::default()
        },
        origin: aye_config::Origin::Default,
    };
    assert_eq!(output(&args(&[]), &config(None)), (Output::Text, "default"));
    assert_eq!(
        output(&args(&[]), &config(Some("json"))),
        (Output::Json, "[tool.aye] output")
    );
    assert_eq!(
        output(&args(&["--output", "text"]), &config(Some("json"))),
        (Output::Text, "--output")
    );
    assert_eq!(
        output(&args(&["--output", "JSON", "-vv"]), &config(None)),
        (Output::Json, "--output")
    );
}

#[test]
fn a_usage_error_a_batch_reports_exits_4() {
    use aye_runner::RunSummary;
    let usage = Errors {
        any: true,
        usage: true,
        ..Errors::default()
    };
    let counts = Counts {
        passed: 3,
        failed: 1,
        ..Counts::default()
    };
    let ran = RunSummary::default();
    // pytest rejecting its arguments, or an initial conftest that fails to
    // load (pytest exits 4), whatever else the batches reported.
    assert_eq!(
        run_exit_code(&ran, usage, &counts, false),
        ExitCode::UsageError
    );
    assert_eq!(
        run_exit_code(&ran, usage, &Counts::default(), true),
        ExitCode::UsageError
    );
    // An interruption, then an internal error, decide first.
    let interrupted = RunSummary {
        interrupted: true,
        ..RunSummary::default()
    };
    assert_eq!(
        run_exit_code(&interrupted, usage, &counts, false),
        ExitCode::Interrupted
    );
    let internal = Errors {
        internal: true,
        ..usage
    };
    assert_eq!(
        run_exit_code(&ran, internal, &counts, false),
        ExitCode::InternalError
    );
    // Other errors during the run.
    let collection = Errors {
        any: true,
        ..Errors::default()
    };
    assert_eq!(
        run_exit_code(&ran, collection, &counts, false),
        ExitCode::Interrupted
    );
    assert_eq!(
        run_exit_code(&ran, collection, &Counts::default(), true),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&ran, Errors::default(), &counts, false),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&ran, Errors::default(), &Counts::default(), false),
        ExitCode::NoTests
    );
}

/// C22: a batch whose framework failed it although none of its tests did
/// (a conftest's `session.exitstatus = 1`, pytest-cov's
/// `--cov-fail-under`) fails the run, as pytest exits 1; the rest decides
/// first.
#[test]
fn a_framework_that_failed_a_clean_batch_fails_the_run() {
    use aye_runner::RunSummary;
    let failed = RunSummary {
        framework_failed: true,
        ..RunSummary::default()
    };
    let passed = Counts {
        passed: 3,
        ..Counts::default()
    };
    assert_eq!(
        run_exit_code(&failed, Errors::default(), &passed, false),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&RunSummary::default(), Errors::default(), &passed, false),
        ExitCode::Ok
    );
    let interrupted = RunSummary {
        interrupted: true,
        ..failed.clone()
    };
    assert_eq!(
        run_exit_code(&interrupted, Errors::default(), &passed, false),
        ExitCode::Interrupted
    );
    let usage = Errors {
        any: true,
        usage: true,
        ..Errors::default()
    };
    assert_eq!(
        run_exit_code(&failed, usage, &passed, false),
        ExitCode::UsageError
    );
}

/// C24: the JUnit report follows the framework's settings that the
/// adapters' contexts give, in pytest's spellings, each from the first
/// context that gives it; aye's own otherwise.
#[test]
fn the_junit_report_follows_the_frameworks_settings() {
    assert_eq!(junit_options(&[]), JunitOptions::default());
    assert_eq!(
        junit_options(&prepared(&[serde_json::json!({ "rootdir": "/repo" })])),
        JunitOptions::default()
    );
    let pytest = serde_json::json!({
        "junit_suite_name": "mysuite",
        "junit_logging": "out-err",
        "junit_log_passing_tests": false,
        "junit_duration_report": "call",
        "junit_family": "legacy",
    });
    assert_eq!(
        junit_options(&prepared(&[serde_json::json!({}), pytest])),
        JunitOptions {
            suite_name: "mysuite".into(),
            logging: JunitLogging::OutErr,
            log_passing_tests: false,
            duration: JunitDuration::Body,
            family: JunitFamily::Xunit1,
        }
    );
    // pytest's defaults, as the context gives them.
    let defaults = serde_json::json!({
        "junit_suite_name": "pytest",
        "junit_logging": "no",
        "junit_log_passing_tests": true,
        "junit_duration_report": "total",
        "junit_family": "xunit2",
    });
    assert_eq!(
        junit_options(&prepared(&[defaults])),
        JunitOptions {
            suite_name: "pytest".into(),
            logging: JunitLogging::No,
            log_passing_tests: true,
            duration: JunitDuration::Total,
            family: JunitFamily::Xunit2,
        }
    );
    // A bool written as an ini file writes it; one pytest can't read keeps
    // the default.
    for (value, expected) in [
        ("False", false),
        ("off", false),
        ("YES", true),
        ("maybe", true),
    ] {
        let context = serde_json::json!({ "junit_log_passing_tests": value });
        assert_eq!(
            junit_options(&prepared(&[context])).log_passing_tests,
            expected,
            "{value}"
        );
    }
}

/// C26: a rerun repeats aye's own options that shape the run, as given:
/// `-c FILE` (or `--config-file`), not `--python` or the output's.
#[test]
fn reruns_repeat_ayes_options_that_shape_the_run() {
    assert_eq!(rerun_args(&args(&[])), Vec::<String>::new());
    assert_eq!(rerun_args(&args(&["-c", "alt.ini"])), ["-c", "alt.ini"]);
    assert_eq!(
        rerun_args(&args(&[
            "--config-file",
            "../cfg/alt.ini",
            "--python",
            "/usr/bin/python3",
            "-v",
            "--output",
            "json",
            "--junitxml",
            "out.xml",
            "-x",
        ])),
        ["-c", "../cfg/alt.ini"]
    );
    let context = with_rerun_args(
        serde_json::json!({ "rootdir": "/repo" }),
        &["-c".into(), "alt.ini".into()],
    );
    assert_eq!(
        context,
        serde_json::json!({ "rootdir": "/repo", "rerun_args": ["-c", "alt.ini"] })
    );
    let context = serde_json::json!({ "rootdir": "/repo" });
    assert_eq!(with_rerun_args(context.clone(), &[]), context);
}

/// `--native`, the prototype's switch, does nothing now that aye's engine
/// runs every test: a rerun doesn't repeat it.
#[test]
fn reruns_leave_native_out() {
    assert_eq!(rerun_args(&args(&["--native"])), Vec::<String>::new());
    assert_eq!(
        rerun_args(&args(&["-c", "alt.ini", "--native"])),
        ["-c", "alt.ini"]
    );
}

#[test]
fn maxfail_after_the_double_dash_wins_over_the_command_lines() {
    let none = prepared(&[serde_json::json!({ "maxfail": null })]);
    // As pytest's `--maxfail 3 -x` and `-x --maxfail 3`: the arguments
    // after `--` come last.
    assert_eq!(
        maxfail(&args(&["--maxfail", "3", "--", "-x"]), &none),
        Some(1)
    );
    assert_eq!(
        maxfail(&args(&["-x", "--", "--maxfail", "3"]), &none),
        Some(3)
    );
    assert_eq!(maxfail(&args(&["-x", "--", "--maxfail=0"]), &none), None);
    assert_eq!(maxfail(&args(&["-x", "--", "-v"]), &none), Some(1));
    // Without either, the framework's own (pytest's addopts).
    let addopts = prepared(&[serde_json::json!({ "maxfail": 2 })]);
    assert_eq!(maxfail(&args(&[]), &addopts), Some(2));
    assert_eq!(maxfail(&args(&["--maxfail", "5"]), &addopts), Some(5));
    assert_eq!(maxfail(&args(&["--maxfail", "0"]), &addopts), None);
    assert_eq!(maxfail(&args(&["--", "--maxfail", "4"]), &addopts), Some(4));
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
    .map(|arg| resolve(cwd, root, arg))
    .collect();
    let requested = requested_ids(&positionals);
    assert_eq!(
        requested,
        [
            "tests/test_p.py::test_dyn[9]",
            "tests/test_p.py::TestK::test_m[a-b]"
        ]
    );
    let context = with_requested(serde_json::json!({ "rootdir": "/repo" }), &requested);
    assert_eq!(
        context,
        serde_json::json!({
            "rootdir": "/repo",
            "requested_ids": ["tests/test_p.py::test_dyn[9]", "tests/test_p.py::TestK::test_m[a-b]"],
        })
    );
    // Nothing to add: the context is the adapter's.
    let context = serde_json::json!({ "rootdir": "/repo" });
    assert_eq!(with_requested(context.clone(), &[]), context);
    assert_eq!(
        with_requested(serde_json::Value::Null, &requested),
        serde_json::Value::Null
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

/// Finding F28: an `@file` after `--` hides what pytest's `-k` and `-m`
/// would be, but no adapter applies an expression at runtime (aye's engine
/// runs pytest's tests too), so every adapter's cases get the command
/// line's (else their own), rather than run unfiltered.
#[test]
fn unreadable_framework_args_leave_every_adapter_the_command_lines() {
    let pytest = serde_json::json!({"keyword": "not slow", "tags": "db"});
    let doctest = serde_json::json!({"keyword": "own", "tags": null});
    let unittest = serde_json::json!({});
    let contexts = [pytest, doctest, unittest];
    let argv = ["-k", "one", "-m", "fast", "--", "@args.txt"];
    for adapter in ["pytest", "doctest", "unittest"] {
        assert_eq!(
            expressions_for(adapter, &argv, &contexts),
            pair(Some("one"), Some("fast")),
            "{adapter}"
        );
    }
    // Without the command line's, each keeps its own.
    let argv = ["--", "@args.txt"];
    assert_eq!(
        expressions_for("pytest", &argv, &contexts),
        pair(Some("not slow"), Some("db"))
    );
    // `--native` changes nothing.
    assert_eq!(
        expressions_for(
            "pytest",
            &["--native", "-k", "one", "--", "@args.txt"],
            &contexts
        ),
        pair(Some("one"), Some("db"))
    );
    assert_eq!(
        expressions_for("doctest", &argv, &contexts),
        pair(Some("own"), None)
    );
    assert_eq!(
        expressions_for("unittest", &argv, &contexts),
        pair(None, None)
    );
    // An empty one turns selection off for them too.
    assert_eq!(
        expressions_for("doctest", &["-k", "", "--", "@args.txt"], &contexts),
        pair(None, None)
    );
    // An unknown short option that may hold the letter is as unreadable.
    assert_eq!(
        expressions_for("unittest", &["-k", "one", "--", "-Zk", "x"], &contexts),
        pair(Some("one"), None)
    );
    assert_eq!(
        expressions_for("pytest", &["-k", "one", "--", "-Zk", "x"], &contexts),
        pair(Some("one"), Some("db"))
    );
}

/// Finding F32: a file holding a `TestCase` (the unittest adapter's) and a
/// test function (pytest's) that fails to import is one collection error
/// in pytest, which collects the file once, not one per adapter.
#[test]
fn a_file_two_adapters_load_in_one_module_is_one_collection_error() {
    let batch = |batch: u32, event: Event| {
        let mut record = Record::new(event);
        record.envelope.batch = Some(batch);
        record
    };
    let started = |id: u32, adapter: &str| {
        batch(
            id,
            Event::BatchStarted {
                adapter: adapter.to_string(),
                cases: 1,
            },
        )
    };
    let error = |id: u32, path: &str| {
        batch(
            id,
            Event::Diagnostic(Diagnostic {
                severity: Severity::Error,
                code: codes::COLLECTION_ERROR.to_string(),
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
        code: codes::COLLECTION_ERROR.to_string(),
        path: path.into(),
        span: None,
        message: format!("cannot import {path}"),
    };
    let mut once = OneModuleErrors::default();
    once.refined("pytest", &error("test_imp.py"));
    // A conftest's, or anything but an error, is another matter.
    once.refined("pytest", &error("conftest.py"));
    once.refined(
        "pytest",
        &Diagnostic {
            severity: Severity::Warning,
            ..error("test_warn.py")
        },
    );
    let mut started = Record::new(Event::BatchStarted {
        adapter: "unittest".to_string(),
        cases: 1,
    });
    started.envelope.batch = Some(3);
    once.see(&mut started);
    let severity = |path: &str, once: &mut OneModuleErrors| {
        let mut record = Record::new(Event::Diagnostic(error(path)));
        record.envelope.batch = Some(3);
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
        args.iter().map(|arg| resolve(root, root, arg)).collect()
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
    let diagnostic = |severity, code: &str, path: &str| Diagnostic {
        severity,
        code: code.to_string(),
        path: path.into(),
        span: None,
        message: "m".to_string(),
    };
    let skipped = |path: &str| diagnostic(Severity::Info, codes::SKIPPED, path);
    let usage = |path: &str| diagnostic(Severity::Error, codes::USAGE_ERROR, path);

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
        codes::COLLECTION_ERROR,
        "testing/test_g.py",
    );
    assert!(reach.keeps(root, &error));
    let info = diagnostic(Severity::Info, "no-tests", "testing/test_g.py");
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

    // The adapters get them as absolute paths, when there are some.
    let context = with_selected_paths(serde_json::json!({"roots": ["/x"]}), &reach.selected);
    let tests = root.join("tests").to_string_lossy().into_owned();
    assert_eq!(context["selected_paths"][0], serde_json::json!(tests));
    assert_eq!(context["roots"], serde_json::json!(["/x"]));
    let context = serde_json::json!({"roots": ["/x"]});
    assert_eq!(with_selected_paths(context.clone(), &[]), context);
}

/// Recheck regression 3: one conftest that fails at start-up was two usage
/// errors with the default adapters, the doctest adapter's and pytest's,
/// worded apart. It is one error, whatever the words.
#[test]
fn a_conftest_two_adapters_fail_to_load_is_one_error() {
    let diagnostic = |code: &str, path: &str, message: &str| Diagnostic {
        severity: Severity::Error,
        code: code.to_string(),
        path: path.into(),
        span: None,
        message: message.to_string(),
    };
    let mut once = OneConftestError::default();
    // In refinement, each adapter reports it once.
    let mut first = diagnostic(codes::USAGE_ERROR, "tests/conftest.py", "Skipped while …");
    once.see(None, &mut first);
    assert_eq!(first.severity, Severity::Error);
    let mut again = diagnostic(
        codes::USAGE_ERROR,
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
        (codes::COLLECTION_ERROR, "tests/conftest.py"),
        (codes::USAGE_ERROR, "other/conftest.py"),
        (codes::COLLECTION_ERROR, "tests/test_imp.py"),
        (codes::COLLECTION_ERROR, "tests/test_imp.py"),
        (codes::USAGE_ERROR, ""),
        (codes::USAGE_ERROR, ""),
    ] {
        let mut other = diagnostic(code, path, "x");
        once.see(None, &mut other);
        assert_eq!(other.severity, Severity::Error, "{code} {path}");
    }

    // In batches: one adapter's repeats stay (renderers show them once),
    // another adapter's is an info.
    let batch = |batch: u32, event: Event| {
        let mut record = Record::new(event);
        record.envelope.batch = Some(batch);
        record
    };
    let mut once = OneConftestError::default();
    for (id, adapter) in [(0, "pytest"), (1, "pytest"), (2, "doctest")] {
        once.see_record(&mut batch(
            id,
            Event::BatchStarted {
                adapter: adapter.to_string(),
                cases: 1,
            },
        ));
    }
    let mut seen = |id: u32, message: &str| {
        let mut record = batch(
            id,
            Event::Diagnostic(diagnostic(
                codes::COLLECTION_ERROR,
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
    let diagnostic = |severity: Severity, code: &str, path: &str| Diagnostic {
        severity,
        code: code.to_string(),
        path: path.into(),
        span: None,
        message: String::new(),
    };
    let no_tests = aye_collect::codes::NO_TESTS;
    let collected = [
        diagnostic(Severity::Error, codes::USAGE_ERROR, "tests/conftest.py"),
        diagnostic(
            Severity::Error,
            codes::COLLECTION_ERROR,
            "sub/deep/conftest.py",
        ),
        // Not a conftest's: no directory is explained by it.
        diagnostic(Severity::Error, codes::COLLECTION_ERROR, "other/test_x.py"),
        // A conftest's info explains nothing either.
        diagnostic(Severity::Info, codes::USAGE_ERROR, "more/conftest.py"),
    ];
    let broken = broken_conftest_dirs(&collected);
    assert_eq!(broken, ["tests/", "sub/deep/"]);
    let explained =
        |path: &str| explained_by_a_conftest(&broken, &diagnostic(Severity::Info, no_tests, path));
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
        &diagnostic(Severity::Error, codes::COLLECTION_ERROR, "tests/test_t.py")
    ));
    // A root conftest's failure explains every file.
    let root = [diagnostic(
        Severity::Error,
        codes::USAGE_ERROR,
        "conftest.py",
    )];
    let broken = broken_conftest_dirs(&root);
    assert!(explained_by_a_conftest(
        &broken,
        &diagnostic(Severity::Info, no_tests, "test_top.py")
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
