use aye_events::{
    DiagnosticCode, Event, Failure, FailureSignature, Outcome, Record, TestId, TestResult,
};

use super::{feed, lines, streams};
use crate::{GithubAnnotations, Reporter};

fn github(records: &[Record]) -> String {
    let mut github = GithubAnnotations::new(Vec::new());
    feed(&mut github, records);
    String::from_utf8(github.into_inner()).unwrap()
}

#[test]
fn nothing_when_nothing_failed() {
    assert_eq!(github(&streams::pass_only()), "");
}

#[test]
fn one_annotation_per_failure_signature() {
    assert_eq!(
        github(&streams::mixed()),
        lines(&[
            "::error file=tests/test_cart.py,line=12,title=tests/test_cart.py%3A%3Atest_total::assert 'spam' == 'eggs'%0A- eggs%0A+ spam",
            "::error file=src/app/db.py,line=30,title=tests/test_db.py%3A%3Atest_load::ConnectionError: refused",
            "::error file=tests/test_io.py,line=20,title=tests/test_io.py%3A%3Atest_write::assert False",
            "::error file=src/app/users.py,line=41,title=tests/test_users.py%3A%3Atest_find[1]::KeyError: 'user1'%0A2 more tests failed the same way",
        ])
    );
}

#[test]
fn identical_failures_are_counted_on_one_annotation() {
    assert_eq!(
        github(&streams::identical(50)),
        lines(&[
            "::error file=tests/test_api.py,line=6,title=tests/test_api.py%3A%3Atest_call[0]::TimeoutError: timed out%0A49 more tests failed the same way",
        ])
    );
}

#[test]
fn a_failed_lifetime_is_one_annotation() {
    assert_eq!(
        github(&streams::lifetime()),
        lines(&[
            "::error file=tests/test_db.py,line=9,title=setup of class%3Atests/test_db.py%3A%3ATestDb::RuntimeError: no database%0A3 tests not run",
            "::error file=tests/test_db.py,line=4,title=teardown of module%3Atests/test_db.py::OSError: busy",
        ])
    );
}

#[test]
fn error_diagnostics_are_annotated_at_once() {
    let records = streams::diagnostics();
    let mut github = GithubAnnotations::new(Vec::new()).with_path_prefix("backend/");
    for record in &records[..3] {
        github.report(record).unwrap();
    }
    let early = String::from_utf8(github.into_inner()).unwrap();
    assert_eq!(
        early,
        "::error file=backend/tests/test_bad.py,line=3,title=syntax-error::Expected an expression\n"
    );
    let mut github = GithubAnnotations::new(Vec::new()).with_path_prefix("backend");
    feed(&mut github, &records);
    assert_eq!(
        String::from_utf8(github.into_inner()).unwrap(),
        lines(&[
            "::error file=backend/tests/test_bad.py,line=3,title=syntax-error::Expected an expression",
            "::error title=worker 1 failed::Traceback (most recent call last):%0AMemoryError",
        ])
    );
}

#[test]
fn without_a_location_the_test_definition_is_used() {
    let mut result = TestResult::new(TestId::new("tests/test_x.py::test_y[1]"), Outcome::Failed);
    result.failures = vec![Failure {
        message: "boom".into(),
        frames: vec![aye_events::Frame {
            path: "/usr/lib/python3.13/json/decoder.py".into(),
            line: 355,
            function: None,
            source: None,
            ..Default::default()
        }],
        ..Failure::default()
    }];
    let mut case = aye_events::Case::new("tests/test_x.py::test_y", "pytest");
    case.span = Some(aye_events::Span {
        path: aye_events::ProjectPath::new("tests/test_x.py").unwrap(),
        start: 0,
        end: 0,
        line: Some(14),
    });
    let records = [
        Record::new(Event::CaseCollected(Box::new(case))),
        Record::new(Event::TestFinished(Box::new(result))),
    ];
    assert_eq!(
        github(&records),
        "::error file=tests/test_x.py,line=14,title=tests/test_x.py%3A%3Atest_y[1]::boom\n"
    );
}

#[test]
fn annotations_are_capped() {
    let mut records = Vec::new();
    for i in 0..60 {
        let mut result = TestResult::new(TestId::new(format!("t.py::test_{i}")), Outcome::Failed);
        result.failures = vec![Failure {
            message: format!("failure {i}"),
            signature: FailureSignature::new(format!("sig-{i}")),
            ..Failure::default()
        }];
        records.push(Record::new(Event::TestFinished(Box::new(result))));
    }
    let out = github(&records);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(out.len(), 51);
    assert_eq!(
        out[49],
        "::error file=t.py,title=t.py%3A%3Atest_49::failure 49"
    );
    assert_eq!(
        out[50],
        "::error title=aye::10 more failures not annotated; see the log"
    );
}

#[test]
fn a_diagnostic_about_no_file_is_annotated_on_none() {
    let record = Record::new(Event::Diagnostic(aye_events::Diagnostic {
        severity: aye_events::Severity::Error,
        code: DiagnosticCode::UsageError,
        path: aye_events::DiagnosticPath::Nowhere,
        span: None,
        message: "unrecognized arguments: --bogus".into(),
    }));
    let mut github = GithubAnnotations::new(Vec::new()).with_path_prefix("backend");
    // Every batch reports it; it is annotated once.
    github.report(&record).unwrap();
    github.report(&record).unwrap();
    assert_eq!(
        String::from_utf8(github.into_inner()).unwrap(),
        "::error title=usage-error::unrecognized arguments: --bogus\n"
    );
}
