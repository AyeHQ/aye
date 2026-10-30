use std::fs;

use aye_events::BatchIndex;

use super::*;

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
    let here = RerunFromHere {
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
        RerunFromHere {
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
        here.apply(&mut record);
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
    let configured = RerunFromHere {
        cwd: &tests,
        root,
        rerun_args: &["-c".to_string(), "alt.ini".to_string()],
    };
    let mut record = Record::new(Event::TestFinished(Box::new(aye_events::TestResult::new(
        aye_events::TestId::new("tests/test_a.py::test_x"),
        aye_events::Outcome::Failed,
    ))));
    configured.apply(&mut record);
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
    let mut batches = BatchAdapters::default();
    for (id, adapter) in [(0, "unittest"), (1, "pytest"), (2, "doctest")] {
        batches.see(&started(id, adapter));
    }
    let mut seen = |id: u32, path: &str| {
        let mut record = error(id, path);
        once.see(&mut record, &batches);
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
    let mut batches = BatchAdapters::default();
    batches.see(&started);
    let severity = |path: &str, once: &mut OneModuleErrors| {
        let mut record = Record::new(Event::Diagnostic(error(path)));
        record.envelope.batch = Some(BatchIndex::from_u32(3));
        once.see(&mut record, &batches);
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
    let mut batches = BatchAdapters::default();
    for (id, adapter) in [(0, "pytest"), (1, "pytest"), (2, "doctest")] {
        batches.see(&batch(
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
        once.see_record(&mut record, &batches);
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
