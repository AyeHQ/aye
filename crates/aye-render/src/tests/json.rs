use std::time::Duration;

use aye_events::{
    Assertion, Captured, Counts, Event, ExitCode, Failure, Frame, Location, Outcome, Phase, Record,
    TestId, TestResult,
};
use serde_json::Value;

use super::{feed, lines, streams};
use crate::JsonLines;

/// JSON lines at `verbosity`.
fn json(records: &[Record], verbosity: i8) -> String {
    let mut json = JsonLines::new(Vec::new(), verbosity);
    feed(&mut json, records);
    String::from_utf8(json.into_inner()).unwrap()
}

/// JSON lines at `-v`: a line per bad outcome.
fn verbose_json(records: &[Record]) -> String {
    json(records, 1)
}

const MIXED_SUMMARY: &str = r#"{"type":"summary","exit":1,"tests":5,"failed":4,"errors":2,"passed":1,"s":1.23,"failures":["82c7e","62091","9925b","5cb21","7a736","4a59c"],"more":"aye show <key> --output json"}"#;

#[test]
fn success_is_the_summary() {
    let summary = r#"{"type":"summary","exit":0,"tests":5,"passed":2,"skipped":1,"expected_failure":1,"unexpected_pass":1,"deselected":2,"s":0.84}"#;
    for verbosity in -1..=3 {
        assert_eq!(
            json(&streams::pass_only(), verbosity),
            lines(&[summary]),
            "{verbosity}"
        );
    }
}

#[test]
fn the_default_is_the_summary_with_the_keys_of_what_failed() {
    assert_eq!(json(&streams::mixed(), 0), lines(&[MIXED_SUMMARY]));
    // A failed lifetime is a key of its own, beside the tests it blocked.
    assert_eq!(
        json(&streams::lifetime(), 0),
        lines(&[
            r#"{"type":"summary","exit":1,"tests":4,"errors":3,"passed":1,"teardown_error":1,"s":0.05,"failures":["a2c40","2c818","68511","656f4","5d893"],"more":"aye show <key> --output json"}"#
        ])
    );
}

#[test]
fn the_failures_are_capped() {
    let out = json(&streams::identical(60), 0);
    let summary: Value = serde_json::from_str(out.trim_end()).unwrap();
    assert_eq!(summary["failures"].as_array().unwrap().len(), 50);
    assert_eq!(summary["failures"][0], "6485b");
    assert_eq!(summary["failures_omitted"], 10);
}

#[test]
fn verbose_is_a_line_per_bad_outcome() {
    assert_eq!(
        verbose_json(&streams::mixed()),
        lines(&[
            r#"{"type":"fail","key":"82c7e","id":"tests/test_cart.py::test_total","outcome":"failed","at":"tests/test_cart.py:12","why":"assert 'spam' == 'eggs'"}"#,
            r#"{"type":"fail","key":"62091","id":"tests/test_db.py::test_load","outcome":"error","phase":"setup","at":"src/app/db.py:30","why":"ConnectionError: refused"}"#,
            r#"{"type":"fail","key":"9925b","id":"tests/test_io.py::test_write","outcome":"error","at":"tests/test_io.py:20","why":"assert False"}"#,
            r#"{"type":"fail","key":"5cb21","id":"tests/test_users.py::test_find[1]","outcome":"failed","at":"src/app/users.py:41","why":"KeyError: 'user1'"}"#,
            // The same signature: named, with its own message when it differs.
            r#"{"type":"fail","key":"7a736","id":"tests/test_users.py::test_find[2]","outcome":"failed","same_as":"5cb21","at":"src/app/users.py:41","why":"KeyError: 'user2'"}"#,
            r#"{"type":"fail","key":"4a59c","id":"tests/test_users.py::test_find[3]","outcome":"failed","same_as":"5cb21","at":"src/app/users.py:41"}"#,
            MIXED_SUMMARY,
        ])
    );
}

#[test]
fn very_verbose_adds_the_detail() {
    let out = json(&streams::mixed(), 2);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(
        out[..4],
        [
            r#"{"type":"fail","key":"82c7e","id":"tests/test_cart.py::test_total","outcome":"failed","at":"tests/test_cart.py:12","why":"assert 'spam' == 'eggs'","code":"assert cart.name() == 'eggs'","diff":["- eggs","+ spam"],"rerun":"aye tests/test_cart.py::test_total","ms":5}"#,
            r#"{"type":"fail","key":"62091","id":"tests/test_db.py::test_load","outcome":"error","phase":"setup","at":"src/app/db.py:30","why":"ConnectionError: refused","code":"return socket.create_connection(ADDR)","rerun":"aye tests/test_db.py::test_load","ms":3}"#,
            r#"{"type":"fail","key":"9925b","id":"tests/test_io.py::test_write","outcome":"error","at":"tests/test_io.py:20","why":"assert False","code":"assert False","also":["teardown: RuntimeError: cleanup failed"],"rerun":"aye tests/test_io.py::test_write","ms":7}"#,
            r#"{"type":"fail","key":"5cb21","id":"tests/test_users.py::test_find[1]","outcome":"failed","at":"src/app/users.py:41","why":"KeyError: 'user1'","code":"return USERS[name]","rerun":"aye 'tests/test_users.py::test_find[1]'","ms":1}"#,
        ]
    );
    // Still grouped.
    assert_eq!(
        out[4],
        r#"{"type":"fail","key":"7a736","id":"tests/test_users.py::test_find[2]","outcome":"failed","same_as":"5cb21","at":"src/app/users.py:41","why":"KeyError: 'user2'"}"#
    );
}

#[test]
fn the_engines_level_adds_frames_output_and_locals_and_groups_nothing() {
    let out = json(&streams::mixed(), 3);
    let out: Vec<Value> = out
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        out[0]["output"],
        serde_json::json!({
            "stdout": "loading cart\nitem 1\nitem 2\nitem 3\ntotal computed",
            "log": "WARNING cart: price missing",
        })
    );
    assert_eq!(
        out[1]["frames"],
        serde_json::json!([
            "tests/conftest.py:8 in db: return connect()",
            "src/app/db.py:30 in connect: return socket.create_connection(ADDR)",
            "1 frame in socket",
        ])
    );
    assert!(out.iter().all(|line| line.get("same_as").is_none()));
    assert_eq!(out[4]["why"], "KeyError: 'user2'");
    assert_eq!(
        out[4]["frames"][0],
        "src/app/users.py:41 in find: return USERS[name]"
    );
}

#[test]
fn fifty_identical_failures_are_one_report_and_references() {
    let out = json(&streams::identical(50), 2);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(out.len(), 50 + 1);
    assert_eq!(
        out[0],
        r#"{"type":"fail","key":"6485b","id":"tests/test_api.py::test_call[0]","outcome":"failed","at":"tests/test_api.py:6","why":"TimeoutError: timed out","code":"client.get('/')","rerun":"aye 'tests/test_api.py::test_call[0]'","ms":2}"#
    );
    assert_eq!(
        out[1],
        r#"{"type":"fail","key":"db69b","id":"tests/test_api.py::test_call[1]","outcome":"failed","same_as":"6485b","at":"tests/test_api.py:6"}"#
    );
    assert_eq!(
        out[1..50]
            .iter()
            .filter(|line| line.contains(r#""same_as":"6485b""#))
            .count(),
        49
    );
}

#[test]
fn a_failed_lifetime_is_reported_once_with_what_it_blocked() {
    let mut records = streams::lifetime();
    // The same lifetime failing again, then a test it blocked arriving late.
    let repeat = records[11].clone();
    let mut late = TestResult::new(
        TestId::new("tests/test_db.py::TestDb::test_d"),
        Outcome::Error,
    );
    late.cause = Some("class:tests/test_db.py::TestDb".into());
    records.insert(12, repeat);
    records.insert(13, Record::new(Event::TestFinished(Box::new(late))));
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"lifetime_fail","key":"656f4","lifetime":"class:tests/test_db.py::TestDb","phase":"setup","at":"tests/test_db.py:9","why":"RuntimeError: no database","blocked":["a2c40","2c818","68511"]}"#,
            r#"{"type":"fail","key":"92e40","id":"tests/test_db.py::TestDb::test_d","outcome":"error","cause":"class:tests/test_db.py::TestDb"}"#,
            r#"{"type":"lifetime_fail","key":"5d893","lifetime":"module:tests/test_db.py","phase":"teardown","at":"tests/test_db.py:4","why":"OSError: busy"}"#,
            r#"{"type":"summary","exit":1,"tests":4,"errors":3,"passed":1,"teardown_error":1,"s":0.05,"failures":["a2c40","2c818","68511","656f4","92e40","5d893"],"more":"aye show <key> --output json"}"#,
        ])
    );
    // In detail: where, the blocked test to rerun, how long it took.
    let detail = json(&streams::lifetime(), 2);
    assert!(
        detail.starts_with(r#"{"type":"lifetime_fail","key":"656f4","lifetime":"class:tests/test_db.py::TestDb","phase":"setup","at":"tests/test_db.py:9","why":"RuntimeError: no database","code":"cls.conn = connect()","rerun":"aye tests/test_db.py::TestDb::test_a","ms":1,"blocked":["a2c40","2c818","68511"]}"#),
        "{detail}"
    );
}

#[test]
fn blocked_tests_are_reported_even_without_their_lifetime() {
    let records: Vec<Record> = streams::lifetime()
        .into_iter()
        .filter(|record| !matches!(record.event, Event::LifetimeFinished { .. }))
        .collect();
    let out = verbose_json(&records);
    assert_eq!(
        out.lines().collect::<Vec<_>>()[..3],
        [
            r#"{"type":"fail","key":"a2c40","id":"tests/test_db.py::TestDb::test_a","outcome":"error","cause":"class:tests/test_db.py::TestDb"}"#,
            r#"{"type":"fail","key":"2c818","id":"tests/test_db.py::TestDb::test_b","outcome":"error","cause":"class:tests/test_db.py::TestDb"}"#,
            r#"{"type":"fail","key":"68511","id":"tests/test_db.py::TestDb::test_c","outcome":"error","cause":"class:tests/test_db.py::TestDb"}"#,
        ]
    );
}

#[test]
fn many_blocked_tests_are_counted() {
    let lifetime = "module:tests/test_big.py";
    let mut records = Vec::new();
    for i in 0..25 {
        let mut result = TestResult::new(
            TestId::new(format!("tests/test_big.py::test_{i}")),
            Outcome::Error,
        );
        result.cause = Some(lifetime.into());
        records.push(Record::new(Event::TestFinished(Box::new(result))));
    }
    records.push(Record::new(Event::LifetimeFinished {
        lifetime: lifetime.into(),
        failure: Some(Failure {
            phase: Some(Phase::Setup),
            message: "ImportError: no module named x".into(),
            ..Failure::default()
        }),
        duration: Duration::ZERO,
    }));
    let out = verbose_json(&records);
    let line: Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(line["type"], "lifetime_fail");
    assert_eq!(line["key"], "91123");
    assert_eq!(line["blocked"].as_array().unwrap().len(), 20);
    assert_eq!(line["blocked"][19], "85ded");
    assert_eq!(line["blocked_omitted"], 5);
}

#[test]
fn collection_errors_and_worker_errors() {
    let summary = r#"{"type":"summary","exit":2,"tests":1,"passed":1,"s":0.09}"#;
    assert_eq!(
        verbose_json(&streams::diagnostics()),
        lines(&[
            r#"{"type":"collection_error","path":"tests/test_bad.py","line":3,"code":"syntax-error","error":"Expected an expression"}"#,
            r#"{"type":"worker_error","worker":1,"error":"Traceback (most recent call last):\nMemoryError"}"#,
            summary,
        ])
    );
    // Not at the default level.
    assert_eq!(json(&streams::diagnostics(), 0), lines(&[summary]));
}

#[test]
fn collect_only_lists_the_cases() {
    let cases = lines(&[
        r#"{"type":"case","id":"tests/test_a.py::test_one"}"#,
        r#"{"type":"case","id":"tests/test_a.py::test_param","partial":true}"#,
        r#"{"type":"case","id":"tests/test_b.py::TestB::test_two"}"#,
        r#"{"type":"summary","exit":0,"tests":3,"deselected":1,"s":0.01}"#,
    ]);
    for verbosity in 0..=3 {
        assert_eq!(json(&streams::collect_only(), verbosity), cases);
    }
    assert_eq!(
        json(&streams::collect_only(), -1),
        lines(&[r#"{"type":"summary","exit":0,"tests":3,"deselected":1,"s":0.01}"#])
    );
}

#[test]
fn failed_sub_outcomes_stand_for_their_test() {
    let id = TestId::new("tests/test_s.py::TestS::test_many");
    let records = [
        Record::new(Event::SubOutcome {
            id: id.clone(),
            name: "(i=2)".into(),
            outcome: Outcome::Failed,
            reason: None,
            failure: Some(Failure {
                phase: Some(Phase::Test),
                message: "AssertionError: 2 is even".into(),
                ..Failure::default()
            }),
            duration: Duration::ZERO,
        }),
        Record::new(Event::TestFinished(Box::new(TestResult::new(
            id,
            Outcome::Failed,
        )))),
    ];
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"fail","key":"ace7d","id":"tests/test_s.py::TestS::test_many","sub":"(i=2)","outcome":"failed","why":"AssertionError: 2 is even"}"#
        ])
    );
}

#[test]
fn every_string_is_bounded() {
    let mut result = TestResult::new(TestId::new("t.py::test_big"), Outcome::Failed);
    let frame = |i: u32| Frame {
        path: "t.py".into(),
        line: i,
        function: Some("f".into()),
        source: Some("s".repeat(1000)),
        before: if i == 9 {
            vec!["b".repeat(1000)]
        } else {
            Vec::new()
        },
        ..Default::default()
    };
    result.failures = vec![
        Failure {
            phase: Some(Phase::Test),
            exception: Some("E".repeat(1000)),
            message: "m".repeat(2000),
            assertion: Some(Assertion {
                op: "==".into(),
                left: "l".repeat(2000),
                right: "r".repeat(2000),
                diff: (0..100)
                    .map(|i| format!("- {i} {}", "d".repeat(500)))
                    .collect(),
                ..Default::default()
            }),
            location: Some(Location {
                path: "t.py".into(),
                line: 9,
            }),
            frames: (1..=9).map(frame).collect(),
            hint: Some("h".repeat(1000)),
            ..Failure::default()
        },
        Failure {
            phase: Some(Phase::Teardown),
            message: "x".repeat(2000),
            ..Failure::default()
        },
    ];
    result.output = Captured {
        stdout: (0..100).fold(String::new(), |out, i| out + &format!("line {i}\n")),
        stderr: "e".repeat(5000),
        ..Captured::default()
    };
    let out = json(&[Record::new(Event::TestFinished(Box::new(result)))], 3);
    let line: Value = serde_json::from_str(out.trim_end()).unwrap();
    let chars = |value: &Value| value.as_str().unwrap().chars().count();
    assert_eq!(chars(&line["why"]), 160);
    assert!(line["why"].as_str().unwrap().ends_with("m…"));
    assert_eq!(chars(&line["code"]), 300);
    assert_eq!(chars(&line["before"][0]), 300);
    assert_eq!(chars(&line["left"]), 500);
    assert_eq!(chars(&line["right"]), 500);
    let diff = line["diff"].as_array().unwrap();
    assert_eq!(diff.len(), 30);
    assert_eq!(chars(&diff[0]), 200);
    assert_eq!(diff[29], "… 71 more lines");
    assert_eq!(chars(&line["hint"]), 500);
    let frames = line["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 9);
    assert!(frames[0].as_str().unwrap().starts_with("t.py:1 in f: sss"));
    assert_eq!(chars(&frames[8]), 300);
    assert_eq!(chars(&line["also"][0]), 500);
    let stdout = line["output"]["stdout"].as_str().unwrap();
    assert!(stdout.starts_with("[50 earlier lines omitted]\nline 50\n"));
    assert!(stdout.ends_with("line 99"));
    assert_eq!(chars(&line["output"]["stderr"]), 4000);
    assert!(line["output"]["stderr"].as_str().unwrap().starts_with("…e"));
}

#[test]
fn passes_are_never_mentioned() {
    let records = [
        Record::new(Event::TestFinished(Box::new(TestResult::new(
            TestId::new("t.py::test_ok"),
            Outcome::Passed,
        )))),
        Record::new(Event::RunFinished {
            exit_code: ExitCode::Ok,
            counts: Counts {
                passed: 1,
                ..Counts::default()
            },
            duration: Duration::from_micros(2500),
            stopped: None,
        }),
    ];
    for verbosity in -1..=3 {
        assert_eq!(
            json(&records, verbosity),
            lines(&[r#"{"type":"summary","exit":0,"passed":1,"s":0.0}"#])
        );
    }
}

#[test]
fn a_bad_outcome_without_a_failure_says_what_it_printed_last() {
    let mut result = TestResult::new(TestId::new("t.py::test_crash"), Outcome::Error);
    result.output.stderr = "Fatal Python error: Segmentation fault\n".into();
    result.reproduce = Some(aye_events::Reproduce {
        command: vec![
            "aye".into(),
            "t.py::test_crash".into(),
            "-p".into(),
            "seed=3".into(),
        ],
        inputs: std::collections::BTreeMap::new(),
    });
    let records = [Record::new(Event::TestFinished(Box::new(result)))];
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"fail","key":"50ff7","id":"t.py::test_crash","outcome":"error","why":"Fatal Python error: Segmentation fault"}"#
        ])
    );
    assert_eq!(
        json(&records, 2),
        lines(&[
            r#"{"type":"fail","key":"50ff7","id":"t.py::test_crash","outcome":"error","why":"Fatal Python error: Segmentation fault","rerun":"aye t.py::test_crash -p 'seed=3'","ms":0,"output":{"stderr":"Fatal Python error: Segmentation fault"}}"#
        ])
    );
}

#[test]
fn a_test_failed_only_by_its_subtests_gets_no_second_line() {
    let id = TestId::new("tests/test_s.py::test_sub");
    let mut parent = TestResult::new(id.clone(), Outcome::Failed);
    // As the pytest adapter sends it.
    parent.failures.push(Failure {
        phase: Some(Phase::Test),
        message: "contains 1 failed subtest".into(),
        signature: "16b555694f3fa454".into(),
        detail: Some("contains 1 failed subtest".into()),
        ..Failure::default()
    });
    let records = [
        Record::new(Event::SubOutcome {
            id: id.clone(),
            name: "case (i=1)".into(),
            outcome: Outcome::Failed,
            reason: None,
            failure: Some(Failure {
                phase: Some(Phase::Test),
                message: "assert 1 != 1".into(),
                ..Failure::default()
            }),
            duration: Duration::ZERO,
        }),
        Record::new(Event::TestFinished(Box::new(parent))),
    ];
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","sub":"case (i=1)","outcome":"failed","why":"assert 1 != 1"}"#
        ])
    );
}

fn sub_failed(id: &TestId, name: &str, message: &str, signature: &str) -> Record {
    Record::new(Event::SubOutcome {
        id: id.clone(),
        name: name.into(),
        outcome: Outcome::Failed,
        reason: None,
        failure: Some(Failure {
            phase: Some(Phase::Test),
            message: message.into(),
            signature: signature.into(),
            ..Failure::default()
        }),
        duration: Duration::ZERO,
    })
}

#[test]
fn a_test_failed_only_by_its_subtests_puts_its_output_and_rerun_on_the_first() {
    let id = TestId::new("tests/test_s.py::test_sub");
    let mut parent = TestResult::new(id.clone(), Outcome::Failed);
    parent.failures.push(Failure {
        phase: Some(Phase::Test),
        message: "contains 2 failed subtests".into(),
        signature: "16b555694f3fa454".into(),
        detail: Some("contains 2 failed subtests".into()),
        ..Failure::default()
    });
    parent.output = Captured {
        stdout: "diagnostic output here\n".into(),
        ..Captured::default()
    };
    parent.reproduce = Some(aye_events::Reproduce {
        command: vec!["aye".into(), id.as_str().to_owned(), "-x".into()],
        inputs: std::collections::BTreeMap::new(),
    });
    let other = TestId::new("tests/test_t.py::test_t");
    let records = [
        sub_failed(&id, "(i=1)", "assert 1 != 1", "a1"),
        // Another worker's test finishes in between: its line comes first.
        Record::new(Event::TestFinished(Box::new(TestResult::new(
            other,
            Outcome::Failed,
        )))),
        sub_failed(&id, "(i=3)", "assert 3 != 3", "a1"),
        Record::new(Event::TestFinished(Box::new(parent))),
    ];
    // L1: where and why alone.
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"fail","key":"3d4cd","id":"tests/test_t.py::test_t","outcome":"failed"}"#,
            r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","sub":"(i=1)","outcome":"failed","why":"assert 1 != 1"}"#,
            r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","sub":"(i=3)","outcome":"failed","same_as":"bd751 › (i=1)","why":"assert 3 != 3"}"#,
        ])
    );
    // L2: the test's rerun on the first; L3: its output too.
    let detail = json(&records, 2);
    assert!(
        detail.contains(r#""sub":"(i=1)","outcome":"failed","why":"assert 1 != 1","rerun":"aye tests/test_s.py::test_sub -x","ms":0}"#),
        "{detail}"
    );
    let all = json(&records, 3);
    assert!(
        all.contains(r#""rerun":"aye tests/test_s.py::test_sub -x","ms":0,"output":{"stdout":"diagnostic output here"}}"#),
        "{all}"
    );
}

#[test]
fn sub_outcomes_of_a_test_that_also_failed_itself_come_before_its_line() {
    let id = TestId::new("tests/test_s.py::test_sub");
    let mut parent = TestResult::new(id.clone(), Outcome::Failed);
    parent.failures.push(Failure {
        phase: Some(Phase::Test),
        exception: Some("ValueError".into()),
        message: "ValueError: after the subtests".into(),
        ..Failure::default()
    });
    let records = [
        sub_failed(&id, "(i=1)", "assert 1 != 1", ""),
        Record::new(Event::TestFinished(Box::new(parent))),
    ];
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","sub":"(i=1)","outcome":"failed","why":"assert 1 != 1"}"#,
            r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","outcome":"failed","why":"ValueError: after the subtests"}"#,
        ])
    );
}

#[test]
fn sub_outcomes_of_a_test_that_never_finished_are_still_reported() {
    let id = TestId::new("tests/test_s.py::test_sub");
    let records = [
        sub_failed(&id, "(i=1)", "assert 1 != 1", ""),
        Record::new(Event::RunFinished {
            exit_code: ExitCode::Interrupted,
            counts: Counts::default(),
            duration: Duration::ZERO,
            stopped: Some("interrupted".into()),
        }),
    ];
    let line = r#"{"type":"fail","key":"bd751","id":"tests/test_s.py::test_sub","sub":"(i=1)","outcome":"failed","why":"assert 1 != 1"}"#;
    assert_eq!(
        verbose_json(&records),
        lines(&[
            line,
            r#"{"type":"summary","exit":2,"s":0.0,"stopped":"interrupted","failures":["bd751"],"more":"aye show <key> --output json"}"#,
        ])
    );
    // And without a summary, when the stream ends.
    assert_eq!(verbose_json(&records[..1]), lines(&[line]));
}

#[test]
fn the_summary_says_why_the_run_stopped() {
    let records = [Record::new(Event::RunFinished {
        exit_code: ExitCode::TestsFailed,
        counts: Counts {
            failed: 1,
            passed: 1,
            ..Counts::default()
        },
        duration: Duration::from_millis(250),
        stopped: Some("maxfail".into()),
    })];
    assert_eq!(
        verbose_json(&records),
        lines(&[
            r#"{"type":"summary","exit":1,"failed":1,"passed":1,"s":0.25,"stopped":"maxfail"}"#
        ])
    );
}

#[test]
fn the_summary_names_a_stored_run() {
    let mut records = streams::pass_only();
    if let Event::RunStarted { run, .. } = &mut records[0].event {
        *run = Some(81);
    }
    assert!(json(&records, -1).starts_with(r#"{"type":"summary","run":"r81","exit":0,"#));
}

#[test]
fn an_error_every_batch_repeats_is_reported_once() {
    let record = Record::new(Event::Diagnostic(aye_events::Diagnostic {
        severity: aye_events::Severity::Error,
        code: "usage-error".into(),
        path: String::new(),
        span: None,
        message: "unrecognized arguments: --bogus".into(),
    }));
    assert_eq!(
        verbose_json(&[record.clone(), record.clone(), record]),
        lines(&[
            r#"{"type":"collection_error","path":"","code":"usage-error","error":"unrecognized arguments: --bogus"}"#
        ])
    );
}

fn compared(left: &str, right: &str, diff: &[&str]) -> Record {
    let mut result = TestResult::new(TestId::new("docs/t.txt::t.txt"), Outcome::Failed);
    result.failures = vec![Failure {
        phase: Some(Phase::Test),
        exception: Some("DocTestFailure".into()),
        message: "f(): expected …".into(),
        assertion: Some(Assertion {
            op: "==".into(),
            left: left.into(),
            right: right.into(),
            diff: diff.iter().map(|line| (*line).to_string()).collect(),
            ..Default::default()
        }),
        ..Failure::default()
    }];
    Record::new(Event::TestFinished(Box::new(result)))
}

#[test]
fn a_comparison_says_each_thing_once() {
    let detail = |record| json(&[record], 2);
    // A diff that only restates one-line operands: the operands alone.
    let out = detail(compared("2", "3", &["@@ -1 +1 @@", "-3", "+2"]));
    assert!(
        out.contains(r#""why":"f(): expected …","left":"2","right":"3","rerun""#),
        "{out}"
    );
    // Operands spanning lines, which the diff shows: the diff alone,
    // without its lone hunk header.
    let out = detail(compared(
        "a\nb\nc",
        "a\nB\nc",
        &["@@ -1,3 +1,3 @@", " a", "-B", "+b", " c"],
    ));
    assert!(
        out.contains(r#""why":"f(): expected …","diff":[" a","-B","+b"," c"],"rerun""#),
        "{out}"
    );
    // Spanning lines without a diff: the operands.
    let out = detail(compared("a\nb", "a\nB", &[]));
    assert!(
        out.contains(r#""why":"f(): expected …","left":"a\nb","right":"a\nB","rerun""#),
        "{out}"
    );
}

// ---------------------------------------------------------------- levels

#[test]
fn quiet_is_the_summary_alone_without_the_failures() {
    for records in [
        streams::pass_only(),
        streams::mixed(),
        streams::lifetime(),
        streams::diagnostics(),
        streams::collect_only(),
        streams::identical(5),
        streams::full(),
    ] {
        let quiet = json(&records, -1);
        assert_eq!(quiet.lines().count(), 1, "{quiet}");
        assert!(quiet.starts_with(r#"{"type":"summary""#), "{quiet}");
        assert!(
            !quiet.contains("failures") && !quiet.contains("more"),
            "{quiet}"
        );
        assert_eq!(json(&records, -7), quiet);
    }
}

#[test]
fn very_verbose_adds_a_line_per_diagnostic() {
    let mut records = streams::diagnostics();
    // A warning every batch repeats is one line.
    let warning = records[3].clone();
    records.insert(4, warning);
    assert_eq!(
        json(&records, 2),
        lines(&[
            r#"{"type":"collection_error","path":"tests/test_bad.py","line":3,"code":"syntax-error","error":"Expected an expression"}"#,
            r#"{"type":"diagnostic","severity":"warning","path":"tests/test_dup.py","line":10,"code":"duplicate-definition","message":"test_x is defined twice; only the last one runs"}"#,
            r#"{"type":"diagnostic","severity":"info","path":"tests/test_dup.py","line":1,"code":"refined","message":"imported to find its tests"}"#,
            r#"{"type":"worker_error","worker":1,"error":"Traceback (most recent call last):\nMemoryError"}"#,
            r#"{"type":"summary","exit":2,"tests":1,"passed":1,"s":0.09}"#,
        ])
    );
}

#[test]
fn very_verbose_lines_are_bounded() {
    let diagnostic = Record::new(Event::Diagnostic(aye_events::Diagnostic {
        severity: aye_events::Severity::Warning,
        code: "c".repeat(300),
        path: "p".repeat(900),
        span: None,
        message: "m".repeat(9000),
    }));
    let out = json(&[diagnostic], 2);
    let out: Value = serde_json::from_str(out.trim_end()).unwrap();
    let chars = |value: &Value| value.as_str().unwrap().chars().count();
    assert_eq!(chars(&out["code"]), 100);
    assert_eq!(chars(&out["path"]), 500);
    assert_eq!(chars(&out["message"]), 1000);
}

#[test]
fn every_line_is_typed() {
    for records in [
        streams::pass_only(),
        streams::mixed(),
        streams::lifetime(),
        streams::diagnostics(),
        streams::collect_only(),
        streams::full(),
    ] {
        for verbosity in -1..=3 {
            for line in json(&records, verbosity).lines() {
                let value: Value = serde_json::from_str(line).unwrap();
                assert!(value["type"].is_string(), "{line}");
            }
        }
    }
}

#[test]
fn no_level_writes_the_raw_log() {
    for records in [streams::mixed(), streams::lifetime()] {
        for verbosity in -1..=9 {
            assert!(!json(&records, verbosity).contains(r#""event":"#));
        }
    }
}
