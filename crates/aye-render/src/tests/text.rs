use std::time::Duration;

use aye_events::{
    Assertion, Captured, Counts, Event, ExitCode, Failure, Outcome, Phase, Record, TestId,
    TestResult,
};

use super::{feed, lines, streams};
use crate::{ColorChoice, Reporter, Text, TextOptions};

fn text(records: &[Record], verbosity: i8) -> String {
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity,
            color: ColorChoice::Never,
            live: false,
        },
    );
    feed(&mut text, records);
    String::from_utf8(text.into_inner()).unwrap()
}

const PASS_ONLY_SUMMARY: &str =
    "✓ 2 passed · 1 skipped · 1 expected failure · 1 unexpected pass · 2 deselected   0.84 s";

#[test]
fn success_is_the_header_the_skips_and_the_summary() {
    assert_eq!(
        text(&streams::pass_only(), 0),
        lines(&[
            "aye · repo · python 3.13.1 · 5 tests · 2 workers",
            "",
            "skipped  no network (1)",
            PASS_ONLY_SUMMARY,
        ])
    );
    // -v adds nothing when nothing went bad.
    assert_eq!(
        text(&streams::pass_only(), 1),
        text(&streams::pass_only(), 0)
    );
}

#[test]
fn very_verbose_shows_every_test_with_its_duration() {
    assert_eq!(
        text(&streams::pass_only(), 2),
        lines(&[
            "aye · repo · python 3.13.1 · 5 tests · 2 workers",
            "collected 5 tests (2 deselected) in 4 ms",
            "",
            "✓ passed  tests/test_math.py::test_add  1 ms",
            "✓ passed  tests/test_math.py::test_param[1]  2 ms",
            "  skipped tests/test_math.py::test_net  0 ms — no network",
            "  expected failure tests/test_math.py::test_bug  3 ms — bug #12",
            "  unexpected pass tests/test_math.py::test_fixed  1 ms",
            "",
            "skipped  no network (1)",
            PASS_ONLY_SUMMARY,
        ])
    );
    // And at the engine's level, what a passing test printed, and its
    // worker.
    assert_eq!(
        text(&streams::pass_only(), 3),
        lines(&[
            "aye · repo · python 3.13.1 · 5 tests · 2 workers",
            "collected 5 tests (2 deselected) in 4 ms",
            "",
            "a worker started (pid 4242, 3.13.1)",
            "✓ passed  tests/test_math.py::test_add  1 ms",
            "✓ passed  tests/test_math.py::test_param[1]  2 ms",
            "  skipped tests/test_math.py::test_net  0 ms — no network",
            "  expected failure tests/test_math.py::test_bug  3 ms — bug #12",
            "  unexpected pass tests/test_math.py::test_fixed  1 ms",
            "  stdout   fixed <now> & \"done\"",
            "a worker finished (exit status 0)",
            "",
            "skipped  no network (1)",
            PASS_ONLY_SUMMARY,
        ])
    );
}

const MIXED_SUMMARY: &str = "✗ 4 failed · ! 2 errors · ✓ 1 passed   1.23 s";
const MIXED_DETAILS: &str =
    "details  aye show 82c7e 62091 9925b  ·  every failure: aye show --failed";

#[test]
fn bad_outcomes_are_a_line_each_as_they_happen() {
    assert_eq!(
        text(&streams::mixed(), 0),
        lines(&[
            "✗ 82c7e  tests/test_cart.py::test_total",
            "         assert 'spam' == 'eggs' at tests/test_cart.py:12",
            "! 62091  tests/test_db.py::test_load (setup)",
            "         ConnectionError: refused at src/app/db.py:30",
            "! 9925b  tests/test_io.py::test_write",
            "         assert False at tests/test_io.py:20",
            "✗ 5cb21  tests/test_users.py::test_find[1]",
            "         KeyError: 'user1' at src/app/users.py:41",
            "✗ 7a736  tests/test_users.py::test_find[2]",
            "         same as 5cb21 · KeyError: 'user2'",
            "✗ 4a59c  tests/test_users.py::test_find[3]",
            "         same as 5cb21",
            "",
            MIXED_SUMMARY,
            MIXED_DETAILS,
        ])
    );
}

#[test]
fn verbose_shows_each_bad_outcome_in_detail() {
    assert_eq!(
        text(&streams::mixed(), 1),
        lines(&[
            "✗ 82c7e  tests/test_cart.py::test_total",
            "  failed in the test · 5 ms",
            "",
            "  tests/test_cart.py:12",
            "  →  12 │ assert cart.name() == 'eggs'",
            "  assert 'spam' == 'eggs'",
            "    - eggs",
            "    + spam",
            "",
            "  rerun    aye tests/test_cart.py::test_total",
            "  more     aye show 82c7e -vv   (every frame, output, locals)",
            "",
            "! 62091  tests/test_db.py::test_load",
            "  error in setup · 3 ms",
            "",
            "  src/app/db.py:30",
            "  →  30 │ return socket.create_connection(ADDR)",
            "  ConnectionError: refused",
            "",
            "  rerun    aye tests/test_db.py::test_load",
            "  more     aye show 62091 -vv   (every frame, output, locals)",
            "",
            "! 9925b  tests/test_io.py::test_write",
            "  error in the test · 7 ms",
            "",
            "  tests/test_io.py:20",
            "  →  20 │ assert False",
            "  assert False",
            "  also in teardown: RuntimeError: cleanup failed",
            "",
            "  rerun    aye tests/test_io.py::test_write",
            "  more     aye show 9925b -vv   (every frame, output, locals)",
            "",
            "✗ 5cb21  tests/test_users.py::test_find[1]",
            "  failed in the test · 1 ms",
            "",
            "  src/app/users.py:41",
            "  →  41 │ return USERS[name]",
            "  KeyError: 'user1'",
            "",
            "  rerun    aye 'tests/test_users.py::test_find[1]'",
            "  more     aye show 5cb21 -vv   (every frame, output, locals)",
            "",
            "✗ 7a736  tests/test_users.py::test_find[2]",
            "         same as 5cb21 · KeyError: 'user2'",
            "✗ 4a59c  tests/test_users.py::test_find[3]",
            "         same as 5cb21",
            "",
            MIXED_SUMMARY,
            MIXED_DETAILS,
        ])
    );
}

#[test]
fn very_verbose_shows_everything_and_groups_nothing() {
    let out = text(&streams::mixed(), 2);
    assert!(!out.contains("same as"), "{out}");
    assert!(!out.contains("more     "), "{out}");
    assert!(
        out.contains(&lines(&[
            "  frames   tests/test_cart.py:12 in test_total",
            "             assert cart.name() == 'eggs'",
            "  stdout   loading cart",
            "           item 1",
            "           item 2",
            "           item 3",
            "           total computed",
            "  log      WARNING cart: price missing",
            "",
            "  rerun    aye tests/test_cart.py::test_total",
            "",
            "✓ passed  tests/test_cart.py::test_ok  1 ms",
        ])),
        "{out}"
    );
    // The setup failure was raised in the standard library: the project's
    // frames, and the other one counted by its package.
    assert!(
        out.contains(&lines(&[
            "  frames   tests/conftest.py:8 in db",
            "             return connect()",
            "           src/app/db.py:30 in connect",
            "             return socket.create_connection(ADDR)",
            "           1 frame in socket",
        ])),
        "{out}"
    );
    assert_eq!(out.matches("  rerun    ").count(), 6);
}

#[test]
fn quiet_shows_the_summary_alone() {
    assert_eq!(text(&streams::mixed(), -1), lines(&[MIXED_SUMMARY]));
    assert_eq!(
        text(&streams::lifetime(), -1),
        "! 3 errors · ✓ 1 passed · ! 1 teardown error   0.05 s\n"
    );
    assert_eq!(text(&streams::pass_only(), -1), lines(&[PASS_ONLY_SUMMARY]));
}

#[test]
fn quiet_still_shows_errors_and_failed_workers() {
    assert_eq!(
        text(&streams::diagnostics(), -1),
        lines(&[
            "error[syntax-error] tests/test_bad.py:3: Expected an expression",
            "",
            "worker 1 failed (exit status -9)",
            "  Traceback (most recent call last):",
            "  MemoryError",
            "",
            "interrupted · ✓ 1 passed · ! 1 collection error   0.09 s",
        ])
    );
}

#[test]
fn verbosity_beyond_the_scale_is_its_end() {
    for records in [
        streams::mixed(),
        streams::lifetime(),
        streams::diagnostics(),
    ] {
        assert_eq!(text(&records, -5), text(&records, -1));
        assert_eq!(text(&records, 9), text(&records, 3));
    }
}

#[test]
fn identical_failures_after_the_first_name_it() {
    let out = text(&streams::identical(50), 0);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(
        out[..4],
        [
            "✗ 6485b  tests/test_api.py::test_call[0]",
            "         TimeoutError: timed out at tests/test_api.py:6",
            "✗ db69b  tests/test_api.py::test_call[1]",
            "         same as 6485b",
        ]
    );
    assert_eq!(
        out.iter()
            .filter(|line| line.ends_with("same as 6485b"))
            .count(),
        49
    );
    // One key in the details: the rest are the same.
    assert_eq!(
        out[out.len() - 2..],
        [
            "✗ 50 failed   0.30 s",
            "details  aye show 6485b  ·  every failure: aye show --failed",
        ]
    );
}

#[test]
fn very_verbose_shows_every_failure_whole() {
    let out = text(&streams::identical(50), 2);
    assert!(!out.contains("same as"), "{out}");
    assert_eq!(out.matches("  rerun    ").count(), 50);
}

#[test]
fn tests_a_lifetime_kept_from_running_are_listed_under_it_from_verbose() {
    let lifetime = "module:tests/test_big.py";
    let mut records = Vec::new();
    for i in 0..12 {
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
    let out = text(&records, 0);
    assert_eq!(
        out,
        lines(&[
            "! 91123  setup of module:tests/test_big.py",
            "         ImportError: no module named x",
            "         kept 12 tests from running: aye show 91123",
        ])
    );
    let listed = |verbosity| {
        text(&records, verbosity)
            .lines()
            .filter(|line| line.contains("tests/test_big.py::test_") && !line.contains("rerun"))
            .count()
    };
    assert_eq!(listed(1), 10);
    assert!(text(&records, 1).contains("\n           … 2 more\n"));
    assert_eq!(listed(2), 12);
    assert_eq!(listed(-1), 0);
}

#[test]
fn a_failed_workers_error_is_whole_from_very_verbose() {
    let error = (0..15).fold(String::new(), |error, i| error + &format!("line {i}\n"));
    let mut record = Record::new(Event::WorkerFinished {
        exit_status: Some(1),
        error: Some(error),
    });
    record.envelope.worker = Some(2);
    let records = [record];
    let out = text(&records, 0);
    assert!(
        out.starts_with("worker 2 failed (exit status 1)\n  line 5\n"),
        "{out}"
    );
    // The heading, the last ten lines, and a blank line.
    assert_eq!(out.lines().count(), 1 + 10 + 1);
    let out = text(&records, 2);
    assert!(
        out.starts_with("worker 2 failed (exit status 1)\n  line 0\n"),
        "{out}"
    );
    assert_eq!(out.lines().count(), 1 + 15 + 1);
    // Never hidden, never doubled by the engine's line.
    assert_eq!(text(&records, -1), text(&records, 0));
    assert_eq!(text(&records, 3), out);
}

#[test]
fn a_failed_lifetime_is_shown_once() {
    let mut records = streams::lifetime();
    let repeat = records
        .iter()
        .find(|record| matches!(record.event, Event::LifetimeFinished { .. }))
        .cloned()
        .unwrap();
    records.insert(12, repeat);
    assert_eq!(
        text(&records, 0),
        lines(&[
            "! 656f4  setup of class:tests/test_db.py::TestDb",
            "         RuntimeError: no database at tests/test_db.py:9",
            "         kept 3 tests from running: aye show 656f4",
            "! 5d893  teardown of module:tests/test_db.py",
            "         OSError: busy at tests/test_db.py:4",
            "",
            // The failed teardown blocked no test, so no count holds it.
            "! 3 errors · ✓ 1 passed · ! 1 teardown error   0.05 s",
            "details  aye show 656f4 5d893  ·  every failure: aye show --failed",
        ])
    );
    assert_eq!(
        text(&records, 1),
        lines(&[
            "! 656f4  setup of class:tests/test_db.py::TestDb",
            "  error in setup · 1 ms",
            "",
            "  tests/test_db.py:9",
            "  →  9 │ cls.conn = connect()",
            "  RuntimeError: no database",
            "",
            "  not run  a2c40  tests/test_db.py::TestDb::test_a",
            "           2c818  tests/test_db.py::TestDb::test_b",
            "           68511  tests/test_db.py::TestDb::test_c",
            "  rerun    aye tests/test_db.py::TestDb::test_a",
            "  more     aye show 656f4 -vv   (every frame, output, locals)",
            "",
            "! 5d893  teardown of module:tests/test_db.py",
            "  error in teardown · 2 ms",
            "",
            "  tests/test_db.py:4",
            "  →  4 │ shutil.rmtree(TMP)",
            "  OSError: busy",
            "",
            "  more     aye show 5d893 -vv   (every frame, output, locals)",
            "",
            "! 3 errors · ✓ 1 passed · ! 1 teardown error   0.05 s",
            "details  aye show 656f4 5d893  ·  every failure: aye show --failed",
        ])
    );
}

#[test]
fn blocked_tests_are_shown_when_their_lifetime_never_reports() {
    let records: Vec<Record> = streams::lifetime()
        .into_iter()
        .filter(|record| !matches!(record.event, Event::LifetimeFinished { .. }))
        .collect();
    assert_eq!(
        text(&records, 0),
        lines(&[
            "! a2c40  tests/test_db.py::TestDb::test_a",
            "         not run: setup of class:tests/test_db.py::TestDb failed",
            "! 2c818  tests/test_db.py::TestDb::test_b",
            "         not run: setup of class:tests/test_db.py::TestDb failed",
            "! 68511  tests/test_db.py::TestDb::test_c",
            "         not run: setup of class:tests/test_db.py::TestDb failed",
            "",
            "! 3 errors · ✓ 1 passed   0.05 s",
            "details  aye show a2c40 2c818 68511  ·  every failure: aye show --failed",
        ])
    );
}

#[test]
fn diagnostics_and_worker_errors() {
    assert_eq!(
        text(&streams::diagnostics(), 0),
        lines(&[
            "error[syntax-error] tests/test_bad.py:3: Expected an expression",
            "",
            "worker 1 failed (exit status -9)",
            "  Traceback (most recent call last):",
            "  MemoryError",
            "",
            "warnings  1 of duplicate-definition",
            "interrupted · ✓ 1 passed · ! 1 collection error   0.09 s",
        ])
    );
    let full = text(&streams::diagnostics(), 2);
    assert!(full.contains("\ninfo[refined] tests/test_dup.py:1: imported to find its tests\n"));
    assert!(full.contains(
        "\nwarning[duplicate-definition] tests/test_dup.py:10: test_x is defined twice; only the last one runs\n"
    ));
    assert!(!text(&streams::diagnostics(), 1).contains("info[refined]"));
}

#[test]
fn warnings_are_grouped_by_code_below_very_verbose() {
    let mut records = vec![Record::new(Event::CollectionStarted)];
    for (i, code) in ["unused", "unused", "shadowed", "odd", "unused", "late"]
        .iter()
        .enumerate()
    {
        records.push(Record::new(Event::Diagnostic(aye_events::Diagnostic {
            severity: aye_events::Severity::Warning,
            code: (*code).into(),
            path: format!("tests/t{i}.py").into(),
            span: None,
            message: "never runs".into(),
        })));
    }
    records.push(finished_run(ExitCode::NoTests, Counts::default()));
    assert_eq!(
        text(&records, 0),
        lines(&[
            "warnings  3 of unused, 1 of shadowed, 1 of odd, …",
            "no tests ran   0.00 s",
        ])
    );
    // -vv: every one; -q: none.
    let out = text(&records, 2);
    assert_eq!(out.lines().count(), 6 + 2);
    assert!(out.starts_with("warning[unused] tests/t0.py: never runs\n"));
    assert_eq!(text(&records, -1), "no tests ran   0.00 s\n");
}

#[test]
fn skips_are_grouped_by_reason_most_first() {
    let mut records = Vec::new();
    for (i, reason) in [
        "Linux only",
        "needs GPU",
        "needs GPU",
        "slow",
        "",
        "needs GPU",
    ]
    .iter()
    .enumerate()
    {
        let mut result = TestResult::new(TestId::new(format!("t.py::test_{i}")), Outcome::Skipped);
        result.reason = (!reason.is_empty()).then(|| (*reason).to_owned());
        records.push(Record::new(Event::TestFinished(Box::new(result))));
    }
    records.push(finished_run(
        ExitCode::Ok,
        Counts {
            skipped: 6,
            ..Counts::default()
        },
    ));
    assert_eq!(
        text(&records, 0),
        lines(&[
            "skipped  needs GPU (3) · Linux only (1) · slow (1) · …",
            "6 skipped   0.00 s",
        ])
    );
}

#[test]
fn collect_only_lists_the_cases() {
    let listing = lines(&[
        "tests/test_a.py::test_one",
        "tests/test_a.py::test_param[…]",
        "tests/test_b.py::TestB::test_two",
        "collected 3 tests (1 deselected) in 4 ms",
    ]);
    for verbosity in -1..=3 {
        assert_eq!(text(&streams::collect_only(), verbosity), listing);
    }
}

fn finished_run(exit_code: ExitCode, counts: Counts) -> Record {
    Record::new(Event::RunFinished {
        exit_code,
        counts,
        duration: Duration::ZERO,
        stopped: None,
    })
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
        Record::new(Event::SubOutcome {
            id: id.clone(),
            name: "(i=3)".into(),
            outcome: Outcome::Passed,
            reason: None,
            failure: None,
            duration: Duration::ZERO,
        }),
        Record::new(Event::TestFinished(Box::new(TestResult::new(
            id,
            Outcome::Failed,
        )))),
        finished_run(
            ExitCode::TestsFailed,
            Counts {
                failed: 1,
                ..Counts::default()
            },
        ),
    ];
    assert_eq!(
        text(&records, 0),
        lines(&[
            "✗ ace7d  tests/test_s.py::TestS::test_many › (i=2)",
            "         AssertionError: 2 is even",
            "",
            "✗ 1 failed   0.00 s",
            "details  aye show ace7d",
        ])
    );
    assert_eq!(
        text(&records, 1),
        lines(&[
            "✗ ace7d  tests/test_s.py::TestS::test_many › (i=2)",
            "  failed in the test · 0 ms",
            "",
            "  AssertionError: 2 is even",
            "",
            "  rerun    aye tests/test_s.py::TestS::test_many",
            "  more     aye show ace7d -vv   (every frame, output, locals)",
            "",
            "✗ 1 failed   0.00 s",
            "details  aye show ace7d",
        ])
    );
}

#[test]
fn long_explanations_and_output_are_cut_below_very_verbose() {
    let mut result = TestResult::new(TestId::new("t.py::test_long"), Outcome::Failed);
    result.failures = vec![Failure {
        message: "assert [0, 1, 2] == []".into(),
        assertion: Some(Assertion {
            op: "==".into(),
            left: "x".repeat(400),
            right: "[]".into(),
            diff: (0..45).map(|i| format!("+ {i}")).collect(),
            ..Default::default()
        }),
        ..Failure::default()
    }];
    result.output = Captured {
        stdout: (0..60).fold(String::new(), |out, i| out + &format!("{i}\n")),
        ..Captured::default()
    };
    let records = [Record::new(Event::TestFinished(Box::new(result)))];
    let out = text(&records, 1);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(out[3], "  assert [0, 1, 2] == []");
    assert_eq!(out[4].chars().count(), "    left:  ".len() + 300);
    assert!(out[4].ends_with("x…"));
    assert_eq!(out[6], "    + 0");
    assert_eq!(out[20], "    + 14");
    assert_eq!(out[21], "    … 30 more lines");
    // No output at L2.
    assert!(!out.iter().any(|line| line.contains("stdout")));
    // -vv: the whole comparison, the output's last 50 lines.
    let out = text(&records, 2);
    let out: Vec<&str> = out.lines().collect();
    assert_eq!(out[4], format!("    left:  {}", "x".repeat(400)));
    assert_eq!(out[50], "    + 44");
    assert_eq!(out[52], "  stdout   … 10 earlier lines");
    assert_eq!(out[53], "           10");
    assert_eq!(out[102], "           59");
}

#[test]
fn colour_is_honoured() {
    let colored = |records: &[Record], verbosity| {
        let mut text = Text::new(
            Vec::new(),
            TextOptions {
                verbosity,
                color: ColorChoice::Always,
                live: false,
            },
        );
        feed(&mut text, records);
        String::from_utf8(text.into_inner()).unwrap()
    };
    let out = colored(&streams::mixed(), 0);
    // Symbols with colours, keys bold magenta, locations cyan.
    assert!(
        out.contains("\u{1b}[1m\u{1b}[31m✗\u{1b}[0m \u{1b}[1m\u{1b}[35m82c7e\u{1b}[0m"),
        "{out:?}"
    );
    assert!(
        out.contains("\u{1b}[36mtests/test_cart.py:12\u{1b}[0m"),
        "{out:?}"
    );
    let out = colored(&streams::mixed(), 1);
    assert!(out.contains("\u{1b}[31m- eggs\u{1b}[0m"));
    assert!(out.contains("\u{1b}[32m+ spam\u{1b}[0m"));
    // What the engine did is dimmed.
    let out = colored(&streams::pass_only(), 3);
    assert!(
        out.contains("\u{1b}[2ma worker started (pid 4242, 3.13.1)\u{1b}[0m\n"),
        "{out:?}"
    );
    // Not a live terminal: no hyperlink.
    assert!(!out.contains("\u{1b}]8;;"));
    for verbosity in -1..=3 {
        assert!(!text(&streams::mixed(), verbosity).contains('\u{1b}'));
        assert!(!text(&streams::pass_only(), verbosity).contains('\u{1b}'));
    }
}

#[test]
fn locations_are_hyperlinks_on_a_live_terminal_with_colour() {
    let live = |color| {
        let mut text = Text::new(
            Vec::new(),
            TextOptions {
                verbosity: 0,
                color,
                live: true,
            },
        );
        feed(&mut text, &streams::full());
        String::from_utf8(text.into_inner()).unwrap()
    };
    let out = live(ColorChoice::AlwaysAnsi);
    assert!(
        out.contains("\u{1b}]8;;file:///home/me/tests_pytest/tests/test_cart.py\u{1b}\\\u{1b}[36mtests/test_cart.py:34\u{1b}[0m\u{1b}]8;;\u{1b}\\"),
        "{out:?}"
    );
    assert!(!live(ColorChoice::Never).contains('\u{1b}'));
}

#[test]
fn the_header_comes_once_when_the_first_worker_starts() {
    let mut text = Text::new(Vec::new(), TextOptions::default());
    let records = streams::pass_only();
    let started = records
        .iter()
        .position(|record| matches!(record.event, Event::WorkerStarted { .. }))
        .unwrap();
    for record in &records[..started] {
        text.report(record).unwrap();
    }
    assert_eq!(text.into_inner(), [] as [u8; 0]);
    let mut text = Text::new(Vec::new(), TextOptions::default());
    let mut twice = records.clone();
    twice.insert(started, records[started].clone());
    for record in &twice[..=started + 1] {
        text.report(record).unwrap();
    }
    assert_eq!(
        String::from_utf8(text.into_inner()).unwrap(),
        "aye · repo · python 3.13.1 · 5 tests · 2 workers\n"
    );
}

#[test]
fn a_bad_outcome_without_a_failure_says_what_it_printed_last() {
    let mut result = TestResult::new(TestId::new("t.py::test_crash"), Outcome::Error);
    result.output.stderr = "Fatal Python error: Segmentation fault\n".into();
    let records = [Record::new(Event::TestFinished(Box::new(result)))];
    assert_eq!(
        text(&records, 0),
        lines(&[
            "! 50ff7  t.py::test_crash",
            "         Fatal Python error: Segmentation fault",
        ])
    );
    assert_eq!(
        text(&records, 1),
        lines(&[
            "! 50ff7  t.py::test_crash",
            "  error · 0 ms",
            "",
            "  stderr   Fatal Python error: Segmentation fault",
            "",
            "  rerun    aye t.py::test_crash",
            "  more     aye show 50ff7 -vv   (every frame, output, locals)",
            "",
        ])
    );
}

#[test]
fn renderers_can_move_between_threads() {
    fn send<T: Send>() {}
    send::<Text<std::io::Stdout>>();
    send::<Text<std::io::Stderr>>();
    send::<crate::JsonLines<std::io::Stdout>>();
    send::<crate::JsonLines<std::fs::File>>();
    send::<crate::Junit>();
    send::<crate::GithubAnnotations<std::io::Stdout>>();
    send::<crate::Tee<'static>>();
    assert_eq!(TextOptions::default().verbosity, 0);
}

/// What the pytest adapter sends for a test that failed only through a
/// failed subtest: the subtest's failure, then the test with a bare one.
fn subtest_failure_stream() -> Vec<Record> {
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
    parent.reproduce = Some(aye_events::Reproduce {
        command: vec!["aye".into(), id.as_str().to_owned()],
        inputs: std::collections::BTreeMap::new(),
    });
    vec![
        Record::new(Event::SubOutcome {
            id: id.clone(),
            name: "case (i=1)".into(),
            outcome: Outcome::Failed,
            reason: None,
            failure: Some(Failure {
                phase: Some(Phase::Test),
                exception: Some("AssertionError".into()),
                message: "assert 1 != 1".into(),
                location: Some(aye_events::Location {
                    path: "tests/test_s.py".into(),
                    line: 4,
                }),
                signature: "5ca1ab1e".into(),
                ..Failure::default()
            }),
            duration: Duration::ZERO,
        }),
        Record::new(Event::TestFinished(Box::new(parent))),
        finished_run(
            ExitCode::TestsFailed,
            Counts {
                failed: 1,
                ..Counts::default()
            },
        ),
    ]
}

#[test]
fn a_test_failed_only_by_its_subtests_gets_no_second_block() {
    for verbosity in [0, 1, 2] {
        let out = text(&subtest_failure_stream(), verbosity);
        assert!(
            out.contains("✗ bd751  tests/test_s.py::test_sub › case (i=1)\n"),
            "{out}"
        );
        assert!(
            !out.contains("✗ bd751  tests/test_s.py::test_sub\n"),
            "{out}"
        );
        assert!(!out.contains("contains 1 failed subtest"), "{out}");
    }
    assert_eq!(
        text(&subtest_failure_stream(), 0),
        lines(&[
            "✗ bd751  tests/test_s.py::test_sub › case (i=1)",
            "         assert 1 != 1 at tests/test_s.py:4",
            "",
            "✗ 1 failed   0.00 s",
            "details  aye show bd751",
        ])
    );
}

/// [`subtest_failure_stream`], with the test's own captured output and
/// recorded rerun.
fn subtest_failure_stream_with(output: &Captured, rerun: &[&str]) -> Vec<Record> {
    let mut records = subtest_failure_stream();
    for record in &mut records {
        if let Event::TestFinished(result) = &mut record.event {
            result.output = output.clone();
            result.reproduce.as_mut().unwrap().command =
                rerun.iter().map(|word| (*word).to_string()).collect();
        }
    }
    records
}

#[test]
fn a_test_failed_only_by_its_subtests_still_shows_its_output_at_l3() {
    // As pytest shows "contains 1 failed subtest" and the test's captured
    // output after the subtest's own section.
    let output = Captured {
        stdout: "diagnostic output here\n".into(),
        ..Captured::default()
    };
    let records = subtest_failure_stream_with(&output, &["aye", "tests/test_s.py::test_sub"]);
    assert!(!text(&records, 1).contains("diagnostic output here"));
    let out = text(&records, 2);
    assert!(
        out.contains(&lines(&[
            "✗ bd751  tests/test_s.py::test_sub (contains 1 failed subtest)",
            "  stdout   diagnostic output here",
            "  rerun    aye tests/test_s.py::test_sub",
            "",
        ])),
        "{out}"
    );
}

/// A sub-outcome's block is shown when its test finishes, with the test's
/// recorded rerun: `-c FILE` and the arguments after `--` included.
#[test]
fn a_subtests_block_shows_its_tests_recorded_rerun() {
    let records = subtest_failure_stream_with(
        &Captured::default(),
        &[
            "aye",
            "-c",
            "alt.toml",
            "tests/test_s.py::test_sub",
            "--",
            "--db=pg",
        ],
    );
    assert!(
        text(&records, 1)
            .contains("\n  rerun    aye -c alt.toml tests/test_s.py::test_sub -- '--db=pg'\n")
    );
}

/// A test the run stopped before it finished (`maxfail` dropped the rest of
/// its records) still shows its bad sub-outcomes, with the plain rerun.
#[test]
fn bad_sub_outcomes_of_a_test_that_never_finished_are_shown() {
    let mut records = subtest_failure_stream();
    records.retain(|record| !matches!(record.event, Event::TestFinished(_)));
    assert_eq!(
        text(&records, 0),
        lines(&[
            "✗ bd751  tests/test_s.py::test_sub › case (i=1)",
            "         assert 1 != 1 at tests/test_s.py:4",
            "",
            "✗ 1 failed   0.00 s",
            "details  aye show bd751",
        ])
    );
    assert!(text(&records, 1).contains("\n  rerun    aye tests/test_s.py::test_sub\n"));
}

fn diagnostic(code: &str, path: &str, message: &str) -> Record {
    Record::new(Event::Diagnostic(aye_events::Diagnostic {
        severity: aye_events::Severity::Error,
        code: code.into(),
        path: path.into(),
        span: None,
        message: message.into(),
    }))
}

#[test]
fn errors_are_named_for_what_they_are_and_shown_once() {
    let usage = "usage: pytest.main() [options]\nerror: unrecognized arguments: --bogus";
    let records = [
        diagnostic("usage-error", "", usage),
        diagnostic("usage-error", "", usage),
        diagnostic("usage-error", "", usage),
        finished_run(ExitCode::UsageError, Counts::default()),
    ];
    assert_eq!(
        text(&records, 0),
        lines(&[
            "error[usage-error]: usage: pytest.main() [options]",
            "  error: unrecognized arguments: --bogus",
            "",
            "usage error · ! 1 usage error   0.00 s",
        ])
    );
    let records = [
        diagnostic(
            "internal-error",
            "",
            "pytest: Traceback\nRuntimeError: loop boom",
        ),
        diagnostic(
            "batch-stopped",
            "",
            "pytest stopped the session with an internal error; 1 of 1 tests in the batch never started: tests/test_a.py::test_a",
        ),
        diagnostic(
            "not-found",
            "tests/test_a.py",
            "not found: tests/test_a.py::test_x",
        ),
        finished_run(ExitCode::InternalError, Counts::default()),
    ]
    .map(|mut record| {
        if matches!(record.event, Event::Diagnostic(_)) {
            record.envelope.batch = Some(0);
        }
        record
    });
    let out = text(&records, 0);
    assert!(
        out.starts_with("error[internal-error]: pytest: Traceback\n"),
        "{out}"
    );
    assert!(
        out.contains("error[not-found] tests/test_a.py: not found: tests/test_a.py::test_x\n"),
        "{out}"
    );
    assert!(
        out.ends_with("internal error · ! 1 collection error · ! 1 internal error   0.00 s\n"),
        "{out}"
    );
}

#[test]
fn what_the_command_line_names_and_doesnt_exist_is_a_usage_error() {
    // The binary's own, before anything runs (exit 4), as pytest's
    // `not found:` is a usage error; an explicit file no adapter collects
    // too.
    let records = [
        diagnostic(
            "not-found",
            "tests/test_a.py",
            "not found: tests/test_a.py::test_x",
        ),
        diagnostic("unclaimed", "notes.txt", "no adapter collects notes.txt"),
        finished_run(ExitCode::UsageError, Counts::default()),
    ];
    let out = text(&records, 0);
    assert!(
        out.ends_with("usage error · ! 2 usage errors   0.00 s\n"),
        "{out}"
    );
    assert!(!out.contains("collection error"), "{out}");
    // A requested id the framework can't find in a batch is no usage error
    // (the run exits 2).
    let mut record = diagnostic(
        "not-found",
        "tests/test_a.py",
        "tests/test_a.py::test_x[9]: pytest found no such test",
    );
    record.envelope.batch = Some(3);
    let out = text(
        &[
            record,
            finished_run(ExitCode::Interrupted, Counts::default()),
        ],
        0,
    );
    assert!(
        out.ends_with("interrupted · ! 1 collection error   0.00 s\n"),
        "{out}"
    );
}

#[test]
fn a_run_cut_short_says_why() {
    let stopped = |stopped: &str, exit_code, counts| {
        text(
            &[Record::new(Event::RunFinished {
                exit_code,
                counts,
                duration: Duration::ZERO,
                stopped: Some(stopped.into()),
            })],
            -1,
        )
    };
    let counts = Counts {
        failed: 2,
        error: 1,
        passed: 4,
        ..Counts::default()
    };
    assert_eq!(
        stopped("maxfail", ExitCode::TestsFailed, counts),
        lines(&[
            "stopped after 3 failures (-x/--maxfail)",
            "✗ 2 failed · ! 1 error · ✓ 4 passed   0.00 s",
        ])
    );
    // The verdict already says it.
    assert_eq!(
        stopped("interrupted", ExitCode::Interrupted, counts),
        "interrupted · ✗ 2 failed · ! 1 error · ✓ 4 passed   0.00 s\n"
    );
    assert_eq!(
        stopped("framework", ExitCode::Interrupted, Counts::default()),
        lines(&["stopped early by the framework", "interrupted   0.00 s"])
    );
}

#[test]
fn operands_are_shown_only_when_they_add_to_the_message() {
    let failed = |message: &str, left: &str, right: &str, diff: &[&str]| {
        let mut result = TestResult::new(TestId::new("t.py::test_x"), Outcome::Failed);
        result.failures = vec![Failure {
            message: message.into(),
            assertion: Some(Assertion {
                op: "==".into(),
                left: left.into(),
                right: right.into(),
                diff: diff.iter().map(|line| (*line).to_string()).collect(),
                ..Default::default()
            }),
            ..Failure::default()
        }];
        text(&[Record::new(Event::TestFinished(Box::new(result)))], 1)
    };
    // unittest's own message cuts long values: the operands say more.
    let out = failed(
        "{'a': 1, [20 chars]} != {'a': 1, [20 chars]}",
        "{'a': 1, 'b': 2}",
        "{'a': 1, 'b': 3}",
        &[],
    );
    assert!(
        out.contains("    left:  {'a': 1, 'b': 2}\n    right: {'a': 1, 'b': 3}\n"),
        "{out}"
    );
    // Already in the message.
    let out = failed("assert 4 == 5", "4", "5", &[]);
    assert!(!out.contains("left:"), "{out}");
    // A doctest's: the diff shows expected above got.
    let out = failed(
        "f(): expected '3', got 'two'",
        "two",
        "3",
        &["@@ -1 +1 @@", "-3", "+two"],
    );
    assert!(
        out.contains("  f(): expected '3', got 'two'\n    -3\n    +two\n\n  rerun"),
        "{out}"
    );
    // Spanning lines, with nothing to show how they differ: under each
    // other.
    let out = failed("x", "a\nb", "a\nB", &[]);
    assert!(
        out.contains("    left:  a\n           b\n    right: a\n           B\n"),
        "{out}"
    );
}

fn live_text(records: &[Record], live: bool) -> String {
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity: 0,
            color: ColorChoice::Never,
            live,
        },
    );
    feed(&mut text, records);
    String::from_utf8(text.into_inner()).unwrap()
}

/// A long collection's progress, as the binary sends it, before the rest.
fn with_progress() -> Vec<Record> {
    let mut records = streams::pass_only();
    let at = records
        .iter()
        .position(|r| matches!(r.event, Event::CollectionStarted))
        .unwrap();
    let progress = [(4096, 3, 0), (20480, 12, 0), (20480, 12, 7)];
    for (i, (walked, test_files, refining)) in progress.into_iter().enumerate() {
        records.insert(
            at + 1 + i,
            Record::new(Event::CollectionProgress {
                walked,
                test_files,
                refining,
            }),
        );
    }
    records
}

#[test]
fn a_long_collection_shows_its_progress_on_a_terminal_then_clears_it() {
    let quiet = text(&streams::pass_only(), 0);
    // Not on a terminal: nothing more than without progress.
    assert_eq!(live_text(&with_progress(), false), quiet);
    // On a terminal: each report rewrites one line, which is blanked
    // before anything else, so the rest reads exactly as without it.
    let live = live_text(&with_progress(), true);
    let shown = "collecting: 4096 files walked, 3 test files found";
    assert!(live.starts_with(&format!("\r{shown}")), "{live:?}");
    assert!(live.contains("\rcollecting: 20480 files walked, 12 test files found"));
    assert!(live.contains("\rcollecting: importing 7 files to find their tests"));
    let (progress, rest) = live.rsplit_once('\r').unwrap();
    assert!(progress.ends_with(&" ".repeat(shown.len().min(40))));
    assert_eq!(rest, quiet);
}
