//! The unittest and doctest adapters' events, from real runs' logs
//! (trimmed, paths replaced by `/repo`), through every renderer. These logs
//! predate the engine's structured comparisons, spans and history: they
//! show what the renderers fall back to without them.

use aye_events::Record;

use super::{feed, lines};
use crate::{ColorChoice, GithubAnnotations, JsonLines, Junit, Reporter, Text, TextOptions};

/// The log (`aye --output json -vvv docs/test_failure.txt`) of
/// `examples/doctest` with `AYE_EXAMPLE_FAIL=1`: one text file whose last
/// example fails, two of its passing examples kept.
const DOCTEST: &str = include_str!("logs/doctest.jsonl");
/// A doctest whose first example fails to import what the others use.
const DOCTEST_CASCADE: &str = include_str!("logs/doctest_cascade.jsonl");
/// The unittest suite's `setUpModule` that raises, blocking two tests, and
/// a test passing all its subtests and one failing one of three.
const UNITTEST: &str = include_str!("logs/unittest.jsonl");
/// unittest failures whose message spans lines: `self.fail("a\nb\nc")`
/// and `raise ValueError("bad\nvery bad")`.
const UNITTEST_MESSAGES: &str = include_str!("logs/unittest_messages.jsonl");
/// pytest's `assert False, "why:\n  because"`.
const PYTEST_MESSAGE: &str = include_str!("logs/pytest_message.jsonl");
/// A module that skips itself at import (pytest's `allow_module_level`),
/// beside a passing test.
const MODULE_SKIP: &str = include_str!("logs/module_skip.jsonl");
/// A unittest `tearDownClass` that raises after its one test passed: the
/// run fails though no test did (unittest: `FAILED (errors=1)`; pytest:
/// `1 passed, 1 error`).
const UNITTEST_TEARDOWN: &str = include_str!("logs/unittest_teardown.jsonl");
/// The same under `-x`, after a failing test: the run stops, and the class
/// still tears down (unittest `-f`: `FAILED (failures=1, errors=1)`).
const UNITTEST_TEARDOWN_MAXFAIL: &str = include_str!("logs/unittest_teardown_maxfail.jsonl");
/// `-x -n 2`: one worker's test fails while the other's `setUpClass` is
/// still running; it fails after the run stopped, so its lifetime is in the
/// log but the tests it blocks are not.
const UNITTEST_SETUP_MAXFAIL: &str = include_str!("logs/unittest_setup_maxfail.jsonl");
/// A test calling `os._exit`: the batch's module and class lifetimes
/// started and never finished, then a new worker ran the rest.
const UNITTEST_CRASH: &str = include_str!("logs/unittest_crash.jsonl");
/// `timeout -s INT 2 aye` while a test sleeps: lifetimes and a test
/// started, never finished, then `run_finished`.
const UNITTEST_STOPPED: &str = include_str!("logs/unittest_stopped.jsonl");

fn log(jsonl: &str) -> Vec<Record> {
    jsonl
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Text output at `verbosity`, without colour.
pub(super) fn text_of(records: &[Record], verbosity: i8) -> String {
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

fn text(jsonl: &str, verbosity: i8) -> String {
    text_of(&log(jsonl), verbosity)
}

/// JSON lines at `verbosity`.
pub(super) fn json_of(records: &[Record], verbosity: i8) -> String {
    let mut json = JsonLines::new(Vec::new(), verbosity);
    feed(&mut json, records);
    String::from_utf8(json.into_inner()).unwrap()
}

fn json(jsonl: &str, verbosity: i8) -> String {
    json_of(&log(jsonl), verbosity)
}

/// JSON lines at `-v`: a line per bad outcome.
fn verbose_json(jsonl: &str) -> String {
    json(jsonl, 1)
}

fn junit(jsonl: &str) -> String {
    let mut junit = Junit::new("unused.xml");
    for record in &log(jsonl) {
        junit.report(record).unwrap();
    }
    junit.xml()
}

fn github(jsonl: &str) -> String {
    let mut github = GithubAnnotations::new(Vec::new());
    feed(&mut github, &log(jsonl));
    String::from_utf8(github.into_inner()).unwrap()
}

// ---------------------------------------------------------------- doctest

#[test]
fn a_failing_doctest_example_is_shown_once_with_its_source_and_diff() {
    assert_eq!(
        text(DOCTEST, 0),
        lines(&[
            "aye · repo · 1 test · 1 worker",
            "",
            "✗ fe2bc  docs/test_failure.txt::test_failure.txt › line 15: >>> cart.total()",
            r#"         cart.total(): expected "Decimal('1.50')", got "Decimal('2.00')" at docs/test_failure.txt:15"#,
            "",
            "✗ 1 failed   0.06 s",
            "details  aye show fe2bc",
        ])
    );
    assert_eq!(
        text(DOCTEST, 1),
        lines(&[
            "aye · repo · 1 test · 1 worker",
            "",
            "✗ fe2bc  docs/test_failure.txt::test_failure.txt › line 15: >>> cart.total()",
            "  failed in the test · 0 ms",
            "",
            "  docs/test_failure.txt:15",
            "  →  15 │ >>> cart.total()",
            r#"  cart.total(): expected "Decimal('1.50')", got "Decimal('2.00')""#,
            "    -Decimal('1.50')",
            "    +Decimal('2.00')",
            "",
            "  rerun    aye docs/test_failure.txt::test_failure.txt",
            "  more     aye show fe2bc -vv   (every frame, output, locals)",
            "",
            "✗ 1 failed   0.06 s",
            "details  aye show fe2bc",
        ])
    );
    // Passing examples aren't listed, even at -vv.
    let full = text(DOCTEST, 2);
    assert!(!full.contains("line 9"), "{full}");
}

#[test]
fn an_agent_gets_one_line_per_failing_example_and_none_for_passing_ones() {
    assert_eq!(
        verbose_json(DOCTEST),
        lines(&[
            r#"{"type":"fail","key":"fe2bc","id":"docs/test_failure.txt::test_failure.txt","sub":"line 15: >>> cart.total()","outcome":"failed","at":"docs/test_failure.txt:15","why":"cart.total(): expected \"Decimal('1.50')\", got \"Decimal('2.00')\""}"#,
            r#"{"type":"summary","exit":1,"tests":1,"failed":1,"s":0.06,"failures":["fe2bc"],"more":"aye show <key> --output json"}"#,
        ])
    );
    // In detail, the diff only restates the values the message holds: the
    // source line and the rerun.
    let detail = json(DOCTEST, 2);
    assert!(
        detail.contains(r#""code":">>> cart.total()","rerun":"aye docs/test_failure.txt::test_failure.txt","ms":0}"#),
        "{detail}"
    );
}

#[test]
fn junit_has_one_testcase_per_doctest_naming_its_first_failing_example() {
    let xml = junit(DOCTEST);
    assert!(
        xml.contains(&lines(&[
            r#"<testsuites name="aye" tests="1" failures="1" errors="0" skipped="0" time="0.060">"#,
        ])),
        "{xml}"
    );
    assert!(
        xml.contains(&lines(&[
            r#"    <testcase classname="docs.test_failure.txt" name="test_failure.txt" file="docs/test_failure.txt" line="1" time="0.002">"#,
            r#"      <failure message="line 15: &gt;&gt;&gt; cart.total(): cart.total(): expected &quot;Decimal(&apos;1.50&apos;)&quot;, got &quot;Decimal(&apos;2.00&apos;)&quot;" type="DocTestFailure">File "docs/test_failure.txt", line 15, in test_failure.txt"#,
            "Failed example:",
            "    cart.total()",
            "Expected:",
            "    Decimal('1.50')",
            "Got:",
            "    Decimal('2.00')</failure>",
            "    </testcase>",
        ])),
        "{xml}"
    );
    assert_eq!(xml.matches("<failure").count(), 1, "{xml}");
}

#[test]
fn github_annotates_the_failing_example_once() {
    assert_eq!(
        github(DOCTEST),
        lines(&[
            "::error file=docs/test_failure.txt,line=15,title=docs/test_failure.txt%3A%3Atest_failure.txt › line 15%3A >>> cart.total()::cart.total(): expected \"Decimal('1.50')\", got \"Decimal('2.00')\"%0A-Decimal('1.50')%0A+Decimal('2.00')",
        ])
    );
}

#[test]
fn examples_failing_after_a_broken_one_get_a_line_each() {
    let out = text(DOCTEST_CASCADE, 0);
    assert_eq!(
        out.matches("✗ 3ae8a  docs/test_cascade.txt").count(),
        6,
        "{out}"
    );
    assert!(
        out.contains(
            "✗ 3ae8a  docs/test_cascade.txt::test_cascade.txt › line 1: >>> from missing_module import thing\n\
             \x20        ModuleNotFoundError: No module named 'missing_module' at docs/test_cascade.txt:1\n"
        ),
        "{out}"
    );
    // At -v, the first three are blocks, the rest their lines.
    let out = text(DOCTEST_CASCADE, 1);
    assert_eq!(out.matches("  rerun    ").count(), 3, "{out}");
    assert!(
        out.contains(
            "✗ 3ae8a  docs/test_cascade.txt::test_cascade.txt › line 6: >>> thing(3)\n\
             \x20        NameError: name 'thing' is not defined at docs/test_cascade.txt:6\n\
             ✗ 3ae8a  docs/test_cascade.txt::test_cascade.txt › line 8: >>> thing(4)\n"
        ),
        "{out}"
    );
    // JSON lines keep where and why for the later ones, and no more.
    let lines: Vec<String> = json(DOCTEST_CASCADE, 2).lines().map(String::from).collect();
    assert_eq!(lines.len(), 7, "{lines:#?}");
    assert!(lines[0].contains(r#""code":"#));
    assert_eq!(
        lines[3],
        r#"{"type":"fail","key":"3ae8a","id":"docs/test_cascade.txt::test_cascade.txt","sub":"line 6: >>> thing(3)","outcome":"failed","at":"docs/test_cascade.txt:6","why":"NameError: name 'thing' is not defined"}"#
    );
}

// ---------------------------------------------------------------- unittest

#[test]
fn a_failed_set_up_module_is_shown_once_above_the_tests_it_blocked() {
    assert_eq!(
        text(UNITTEST, 0),
        lines(&[
            "aye · repo · 6 tests · 1 worker",
            "",
            "! e6d01  setup of tests/test_module_setup.py",
            "         ValueError: module setup failed at tests/test_module_setup.py:5",
            "         kept 2 tests from running: aye show e6d01",
            "✗ c9b76  tests/test_outcomes.py::Subtests::test_one_fails › (i=1)",
            "         1 == 1 at tests/test_outcomes.py:64",
            "",
            "✗ 1 failed · ! 2 errors · ✓ 1 passed   0.06 s",
            "details  aye show e6d01 c9b76  ·  every failure: aye show --failed",
        ])
    );
    assert!(text(UNITTEST, 1).contains(&lines(&[
        "  not run  42640  tests/test_module_setup.py::First::test_first",
        "           6b6c8  tests/test_module_setup.py::Second::test_second",
        "  rerun    aye tests/test_module_setup.py::First::test_first",
    ])));
}

#[test]
fn an_agent_gets_the_lifetime_once_with_the_keys_it_blocked() {
    assert_eq!(
        verbose_json(UNITTEST),
        lines(&[
            r#"{"type":"lifetime_fail","key":"e6d01","lifetime":"tests/test_module_setup.py","phase":"setup","at":"tests/test_module_setup.py:5","why":"ValueError: module setup failed","blocked":["42640","6b6c8"]}"#,
            r#"{"type":"fail","key":"c9b76","id":"tests/test_outcomes.py::Subtests::test_one_fails","sub":"(i=1)","outcome":"failed","at":"tests/test_outcomes.py:64","why":"1 == 1"}"#,
            r#"{"type":"summary","exit":1,"tests":6,"failed":1,"errors":2,"passed":1,"s":0.06,"failures":["42640","6b6c8","e6d01","c9b76"],"more":"aye show <key> --output json"}"#,
        ])
    );
}

#[test]
fn junit_has_an_error_per_blocked_test_with_the_lifetimes_message() {
    let xml = junit(UNITTEST);
    assert!(
        xml.contains(r#"<testsuites name="aye" tests="4" failures="1" errors="2" skipped="0""#),
        "{xml}"
    );
    for test in ["First::test_first", "Second::test_second"] {
        let (class, name) = test.split_once("::").unwrap();
        assert!(
            xml.contains(&format!(
                "    <testcase classname=\"tests.test_module_setup.{class}\" name=\"{name}\" file=\"tests/test_module_setup.py\""
            )),
            "{xml}"
        );
    }
    assert_eq!(
        xml.matches(r#"<error message="setup of tests/test_module_setup.py failed: ValueError: module setup failed" type="ValueError">Traceback (most recent call last):"#).count(),
        2,
        "{xml}"
    );
    // The failed subtest once, named; the passing ones not at all.
    assert!(
        xml.contains(r#"<failure message="(i=1): 1 == 1" type="AssertionError">"#),
        "{xml}"
    );
    assert_eq!(xml.matches("<failure").count(), 1, "{xml}");
    assert!(
        xml.contains(r#"<testcase classname="tests.test_outcomes.Subtests" name="test_all_pass" file="tests/test_outcomes.py" time="0.000"/>"#),
        "{xml}"
    );
}

#[test]
fn github_annotates_the_lifetime_and_the_failed_subtest() {
    assert_eq!(
        github(UNITTEST),
        lines(&[
            "::error file=tests/test_module_setup.py,line=5,title=setup of tests/test_module_setup.py::ValueError: module setup failed%0A2 tests not run",
            "::error file=tests/test_outcomes.py,line=64,title=tests/test_outcomes.py%3A%3ASubtests%3A%3Atest_one_fails › (i=1)::1 == 1",
        ])
    );
}

#[test]
fn a_message_that_spans_lines_is_shown_whole() {
    // L1: its first line.
    let out = text(UNITTEST_MESSAGES, 0);
    assert!(
        out.contains("         first line at tests/test_ml.py:5\n"),
        "{out}"
    );
    // L2: all of it.
    let out = text(UNITTEST_MESSAGES, 1);
    assert!(
        out.contains(&lines(&[
            "  tests/test_ml.py:5",
            r#"  →  5 │ self.fail("first line\nsecond line\nthird line")"#,
            "  first line",
            "  second line",
            "  third line",
        ])),
        "{out}"
    );
    assert!(
        out.contains(&lines(&["  ValueError: bad", "  very bad"])),
        "{out}"
    );
    let out = text(PYTEST_MESSAGE, 1);
    assert!(
        out.contains(&lines(&[
            "  AssertionError: why:",
            "      because",
            "  assert False",
            "",
            "  rerun    aye tests/test_p.py::test_assert_msg",
        ])),
        "{out}"
    );
    let out = json(UNITTEST_MESSAGES, 2);
    assert!(
        out.contains(r#""why":"first line","code":"self.fail(\"first line\\nsecond line\\nthird line\")","message":"first line\nsecond line\nthird line""#),
        "{out}"
    );
    let annotations = github(PYTEST_MESSAGE);
    assert!(
        annotations.contains("::AssertionError: why:%0A    because%0Aassert False"),
        "{annotations}"
    );
}

// ---------------------------------------------------------------- module skip

#[test]
fn a_module_skipped_while_collecting_counts_as_skipped() {
    // pytest: "1 passed, 1 skipped".
    assert_eq!(
        text(MODULE_SKIP, 0),
        lines(&[
            "aye · repo · 1 test · 1 worker",
            "",
            "skipped  whole module (1)",
            "✓ 1 passed · 1 skipped   0.29 s",
        ])
    );
    assert_eq!(
        verbose_json(MODULE_SKIP),
        "{\"type\":\"summary\",\"exit\":0,\"tests\":1,\"passed\":1,\"skipped\":1,\"s\":0.29}\n"
    );
    // pytest's report has a skipped test case named after the module.
    let xml = junit(MODULE_SKIP);
    assert!(
        xml.contains(&lines(&[
            r#"<testsuites name="aye" tests="2" failures="0" errors="0" skipped="1" time="0.287">"#,
            r#"  <testsuite name="aye" tests="2" failures="0" errors="0" skipped="1" time="0.287">"#,
            r#"    <testcase classname="" name="tests.test_s" file="tests/test_s.py" time="0.000">"#,
            r#"      <skipped message="whole module" type="collection"/>"#,
            "    </testcase>",
        ])),
        "{xml}"
    );
}

#[test]
fn a_module_skip_a_batch_reports_or_a_repeat_is_not_counted_again() {
    let mut records = log(MODULE_SKIP);
    let skip = records
        .iter()
        .find(|record| record.event.kind() == "diagnostic")
        .cloned()
        .unwrap();
    let mut from_batch = skip.clone();
    from_batch.envelope.batch = Some(0);
    records.insert(3, skip);
    records.insert(3, from_batch);
    for verbosity in -1..=1 {
        assert!(
            text_of(&records, verbosity).ends_with("✓ 1 passed · 1 skipped   0.29 s\n"),
            "{verbosity}"
        );
        assert!(
            json_of(&records, verbosity).ends_with(
                "{\"type\":\"summary\",\"exit\":0,\"tests\":1,\"passed\":1,\"skipped\":1,\"s\":0.29}\n"
            ),
            "{verbosity}"
        );
    }
    assert!(text_of(&records, 0).contains("\nskipped  whole module (1)\n"));
}

/// A unittest test printing, then failing four of five subtests the same
/// way.
const UNITTEST_SUBTESTS: &str = include_str!("logs/unittest_subtests.jsonl");

#[test]
fn subtests_failing_alike_name_the_first_and_the_tests_output_follows() {
    assert_eq!(
        text(UNITTEST_SUBTESTS, 0),
        lines(&[
            "aye · repo · 1 test · 1 worker",
            "",
            "✗ 77efe  tests/test_o.py::T::test_s › (i=1)",
            "         1 not less than 1 at tests/test_o.py:7",
            "✗ 77efe  tests/test_o.py::T::test_s › (i=2)",
            "         same as 77efe › (i=1) · 2 not less than 1",
            "✗ 77efe  tests/test_o.py::T::test_s › (i=3)",
            "         same as 77efe › (i=1) · 3 not less than 1",
            "✗ 77efe  tests/test_o.py::T::test_s › (i=4)",
            "         same as 77efe › (i=1) · 4 not less than 1",
            "",
            "✗ 1 failed   0.09 s",
            "details  aye show 77efe",
        ])
    );
    // At L3 each is a block, then the test's own output.
    let out = text(UNITTEST_SUBTESTS, 2);
    assert!(
        out.ends_with(&lines(&[
            "✗ 77efe  tests/test_o.py::T::test_s (4 failures above)",
            "  stdout   diagnostic output here",
            "  rerun    aye tests/test_o.py::T::test_s",
            "",
            "✗ 1 failed   0.09 s",
            "details  aye show 77efe",
        ])),
        "{out}"
    );
    assert_eq!(out.matches("  rerun    ").count(), 5, "{out}");
    let xml = junit(UNITTEST_SUBTESTS);
    assert_eq!(xml.matches("<failure").count(), 4, "{xml}");
    assert!(
        xml.contains(r#"<failure message="(i=4): 4 not less than 1""#),
        "{xml}"
    );
}

#[test]
fn a_collect_only_run_counts_no_module_skip() {
    // `pytest --co`: "1 test collected", no skip.
    let records: Vec<Record> = log(MODULE_SKIP)
        .into_iter()
        .map(|mut record| {
            if let aye_events::Event::RunStarted { collect_only, .. } = &mut record.event {
                *collect_only = true;
            }
            record
        })
        .filter(|record| record.envelope.worker.is_none())
        .collect();
    for verbosity in -1..=2 {
        let out = json_of(&records, verbosity);
        let summary = out.lines().last().unwrap();
        assert!(summary.starts_with(r#"{"type":"summary""#), "{out}");
        assert!(!summary.contains("skipped"), "{out}");
        let text = text_of(&records, verbosity);
        let last = text.lines().last().unwrap();
        assert!(last.starts_with("collected 1 test "), "{text}");
    }
}

// ---------------------------------------------------------------- failed lifetimes alone

#[test]
fn a_failed_teardown_that_blocked_no_test_is_named_in_every_summary() {
    // The counts hold test results only: the summary says why the run
    // failed.
    assert_eq!(
        text(UNITTEST_TEARDOWN, 0),
        lines(&[
            "aye · repo · 1 test · 1 worker",
            "",
            "! afeac  teardown of test_a.py::R",
            "         RuntimeError: tdc at test_a.py:5",
            "",
            "✓ 1 passed · ! 1 teardown error   0.06 s",
            "details  aye show afeac",
        ])
    );
    assert_eq!(
        text(UNITTEST_TEARDOWN, -1),
        "✓ 1 passed · ! 1 teardown error   0.06 s\n"
    );
    assert_eq!(
        json(UNITTEST_TEARDOWN, -1),
        "{\"type\":\"summary\",\"exit\":1,\"tests\":1,\"passed\":1,\"teardown_error\":1,\"s\":0.06}\n"
    );
    assert_eq!(
        verbose_json(UNITTEST_TEARDOWN),
        lines(&[
            r#"{"type":"lifetime_fail","key":"afeac","lifetime":"test_a.py::R","phase":"teardown","at":"test_a.py:5","why":"RuntimeError: tdc"}"#,
            r#"{"type":"summary","exit":1,"tests":1,"passed":1,"teardown_error":1,"s":0.06,"failures":["afeac"],"more":"aye show <key> --output json"}"#,
        ])
    );
    assert_eq!(
        github(UNITTEST_TEARDOWN),
        "::error file=test_a.py,line=5,title=teardown of test_a.py%3A%3AR::RuntimeError: tdc\n"
    );
    let xml = junit(UNITTEST_TEARDOWN);
    assert!(
        xml.contains(r#"<testsuites name="aye" tests="2" failures="0" errors="1" skipped="0""#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<testcase classname="" name="teardown of test_a.py::R" file="test_a.py" line="5" time="0.000">"#),
        "{xml}"
    );
}

#[test]
fn a_failed_teardown_after_maxfail_is_counted_beside_the_failure() {
    // pytest -x: "1 failed, 1 error"; unittest -f: "FAILED (failures=1,
    // errors=1)".
    let out = text(UNITTEST_TEARDOWN_MAXFAIL, 0);
    assert!(
        out.ends_with(&lines(&[
            "✗ 8d4fe  tests/test_a.py::A::test_1",
            "         t1 at tests/test_a.py:7",
            "! 5a4ce  teardown of tests/test_a.py::A",
            "         RuntimeError: tdc broke at tests/test_a.py:6",
            "",
            "stopped after 1 failure (-x/--maxfail)",
            "✗ 1 failed · ! 1 teardown error   0.06 s",
            "details  aye show 8d4fe 5a4ce  ·  every failure: aye show --failed",
        ])),
        "{out}"
    );
    assert!(
        verbose_json(UNITTEST_TEARDOWN_MAXFAIL).ends_with(
            "{\"type\":\"summary\",\"exit\":1,\"tests\":2,\"failed\":1,\"teardown_error\":1,\"s\":0.06,\"stopped\":\"maxfail\",\"failures\":[\"8d4fe\",\"5a4ce\"],\"more\":\"aye show <key> --output json\"}\n"
        )
    );
}

#[test]
fn a_failed_setup_whose_tests_the_run_stopped_before_is_counted() {
    let out = text(UNITTEST_SETUP_MAXFAIL, 0);
    assert!(
        out.ends_with(&lines(&[
            "✗ 2a187  tests/test_a.py::A::test_fails",
            "         first failure at tests/test_a.py:6",
            "! 0843c  setup of tests/test_b.py::B",
            "         RuntimeError: setup broke late at tests/test_b.py:11",
            "",
            "stopped after 1 failure (-x/--maxfail)",
            "✗ 1 failed · ! 1 setup error   0.59 s",
            "details  aye show 2a187 0843c  ·  every failure: aye show --failed",
        ])),
        "{out}"
    );
    assert!(
        json(UNITTEST_SETUP_MAXFAIL, -1).ends_with(
            "{\"type\":\"summary\",\"exit\":1,\"tests\":3,\"failed\":1,\"setup_error\":1,\"s\":0.59,\"stopped\":\"maxfail\"}\n"
        )
    );
    let xml = junit(UNITTEST_SETUP_MAXFAIL);
    assert!(
        xml.contains(r#"<testsuites name="aye" tests="2" failures="1" errors="1" skipped="0""#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<testcase classname="" name="setup of tests/test_b.py::B" file="tests/test_b.py" line="11" time="0.000">"#),
        "{xml}"
    );
}

#[test]
fn a_failed_lifetime_that_blocked_tests_counts_through_them_alone() {
    // `setUpModule` blocking two tests: "2 errors", no setup error.
    let out = text(UNITTEST, -1);
    assert_eq!(out, "✗ 1 failed · ! 2 errors · ✓ 1 passed   0.06 s\n");
    assert!(!verbose_json(UNITTEST).contains("setup_error"));
}

// ---------------------------------------------------------------- crashes and stops

/// The log with every lifetime a batch left open closed, without a
/// failure, before its `batch_finished`: what it would be had the worker
/// not crashed or been stopped.
fn closed(records: &[Record]) -> Vec<Record> {
    use aye_events::Event;
    let mut open: Vec<(Option<u32>, aye_events::LifetimeId)> = Vec::new();
    let mut out = Vec::new();
    for record in records {
        match &record.event {
            Event::LifetimeStarted { lifetime, .. } => {
                open.push((record.envelope.batch, lifetime.clone()));
            }
            Event::LifetimeFinished { lifetime, .. } => {
                open.retain(|(batch, held)| (*batch, held) != (record.envelope.batch, lifetime));
            }
            Event::BatchFinished { .. } => {
                let (closing, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut open)
                    .into_iter()
                    .partition(|(batch, _)| *batch == record.envelope.batch);
                open = kept;
                for (_, lifetime) in closing.into_iter().rev() {
                    let mut close = record.clone();
                    close.event = Event::LifetimeFinished {
                        lifetime,
                        failure: None,
                        duration: std::time::Duration::ZERO,
                    };
                    out.push(close);
                }
            }
            _ => {}
        }
        out.push(record.clone());
    }
    out
}

/// What every renderer makes of `records`: text and JSON lines at every
/// level but `-vvv` (where text shows each lifetime starting and
/// finishing), JUnit and annotations.
fn every_renderer(records: &[Record]) -> Vec<String> {
    let jsonl = records
        .iter()
        .map(|record| serde_json::to_string(record).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let mut out: Vec<String> = (-1..=2)
        .flat_map(|verbosity| [text_of(records, verbosity), json_of(records, verbosity)])
        .collect();
    out.push(json_of(records, 3));
    out.push(junit(&jsonl));
    out.push(github(&jsonl));
    out
}

/// F43: a worker that crashes, or that a stop kills, leaves the lifetimes
/// it started without a `lifetime_finished`. No renderer depends on the
/// pair: each says what it would say had they finished, or never started.
/// (Keys come from ids: a lifetime that never started has none, and none
/// of these is shown.)
#[test]
fn lifetimes_a_crash_or_a_stop_left_open_change_no_output() {
    for jsonl in [UNITTEST_CRASH, UNITTEST_STOPPED] {
        let records = log(jsonl);
        let unclosed = records
            .iter()
            .filter(|record| record.event.kind() == "lifetime_started")
            .count()
            > records
                .iter()
                .filter(|record| record.event.kind() == "lifetime_finished")
                .count();
        assert!(unclosed, "the log should leave a lifetime open");
        let closed = closed(&records);
        assert_ne!(closed, records);
        let never_started: Vec<Record> = records
            .iter()
            .filter(|record| {
                !matches!(
                    record.event.kind(),
                    "lifetime_started" | "lifetime_finished"
                )
            })
            .cloned()
            .collect();
        let out = every_renderer(&records);
        assert_eq!(out, every_renderer(&closed));
        assert_eq!(out, every_renderer(&never_started));
    }
    assert_eq!(
        text(UNITTEST_CRASH, 0),
        lines(&[
            "aye · repo · 3 tests · 1 worker",
            "",
            "! c03cb  tests/test_a.py::A::test_2_exit",
            "         worker crashed while running this test: exit status 3",
            "",
            "worker 0 failed (exit status 3)",
            "  worker crashed while running tests/test_a.py::A::test_2_exit (exit status 3)",
            "",
            "! 1 error · ✓ 2 passed   0.11 s",
            "details  aye show c03cb",
        ])
    );
    assert_eq!(
        text(UNITTEST_STOPPED, 0),
        lines(&[
            "aye · repo · 2 tests · 1 worker",
            "",
            "interrupted · ✓ 1 passed   2.06 s",
        ])
    );
}

// ---------------------------------------------------------------- levels

/// The log (`aye --output json -vvv`) of a small pytest project: a passing
/// test that prints, a skip, an xfail, a failure three user frames deep
/// with six lines of output and a 90-line diff, a passing test, and a file
/// with no tests.
const PYTEST_LEVELS: &str = include_str!("logs/pytest_levels.jsonl");
/// The same project's `aye --co --no-refine`: the cases, two warnings
/// (`duplicate-definition`, `class-has-init`) and an info.
const PYTEST_LEVELS_CO: &str = include_str!("logs/pytest_levels_co.jsonl");

/// `text`'s lines, each without its line break.
fn lines_of(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn text_at_each_level_of_a_pytest_run() {
    let summary = "✗ 1 failed · ✓ 2 passed · 1 skipped · 1 expected failure   0.64 s";
    let closing = [
        "",
        "skipped  not on this platform (1)",
        summary,
        "details  aye show 4f3c4",
    ];
    // -q: the summary alone.
    assert_eq!(text(PYTEST_LEVELS, -1), lines(&[summary]));

    // The default: the failure's line.
    assert_eq!(
        text(PYTEST_LEVELS, 0),
        lines(
            &[
                &[
                    "aye · repo · 5 tests · 1 worker",
                    "",
                    "✗ 4f3c4  tests/test_levels.py::test_long",
                    "         assert 'line 0\\nline...e 58\\nline 59' == 'LINE 0\\nline...E 58\\nline 59' at tests/test_levels.py:24",
                ][..],
                &closing[..],
            ]
            .concat()
        )
    );

    // -v: its detail, cut.
    let verbose = text(PYTEST_LEVELS, 1);
    let out = lines_of(&verbose);
    assert_eq!(
        out[2..7],
        [
            "✗ 4f3c4  tests/test_levels.py::test_long",
            "  failed in the test · 1 ms",
            "",
            "  tests/test_levels.py:24",
            "  →  24 │ assert text == lines(range(0, 60, 2))",
        ]
    );
    assert!(out[8].starts_with("    left:  'line 0\\nline 1") && out[8].ends_with('…'));
    assert_eq!(
        out[out.len() - 8..],
        [
            "    … 75 more lines",
            "",
            "  rerun    aye tests/test_levels.py::test_long",
            "  more     aye show 4f3c4 -vv   (every frame, output, locals)",
            closing[0],
            closing[1],
            closing[2],
            closing[3],
        ]
    );

    // -vv: everything, a line per other test, the info.
    let full = text(PYTEST_LEVELS, 2);
    let out = lines_of(&full);
    assert_eq!(
        out[..9],
        [
            "info[no-tests] tests/test_empty.py: claimed for tests by pytest, but has none",
            "",
            "aye · repo · 5 tests · 1 worker",
            "collected 5 tests (2 files imported) in 617 ms",
            "",
            "✓ passed  tests/test_levels.py::test_prints  0 ms",
            "  skipped tests/test_levels.py::test_skipped  0 ms — not on this platform",
            "  expected failure tests/test_levels.py::test_xfail  0 ms — known bug",
            "",
        ]
    );
    assert!(!full.contains('…'), "{full}");
    let diff: Vec<&str> = out
        .iter()
        .filter(|line| line.starts_with("    - LINE") || line.starts_with("    + line"))
        .copied()
        .collect();
    assert_eq!(diff.len(), 60);
    assert_eq!(
        out[out.len() - 20..],
        [
            "  frames   tests/test_levels.py:34 in test_long",
            "             helper()",
            "           tests/test_levels.py:28 in helper",
            "             check(lines(()))",
            "           tests/test_levels.py:24 in check",
            "             assert text == lines(range(0, 60, 2))",
            "  stdout   step 0",
            "           step 1",
            "           step 2",
            "           step 3",
            "           step 4",
            "           step 5",
            "",
            "  rerun    aye tests/test_levels.py::test_long",
            "",
            "✓ passed  tests/test_levels.py::test_dup  0 ms",
            "",
            "skipped  not on this platform (1)",
            summary,
            "details  aye show 4f3c4",
        ]
    );

    // -vvv: the worker and its batch, and what passing tests printed.
    let mut expected = lines_of(&full);
    expected.splice(
        5..5,
        [
            "worker 0 started (pid 62525, /repo/.venv/bin/python)",
            "batch 0 started on worker 0 (pytest, 5 tests)",
        ],
    );
    expected.splice(
        8..8,
        [
            "  stdout   hello from a passing test",
            "           second line",
        ],
    );
    let at = expected.len() - 4;
    expected.splice(
        at..at,
        [
            "batch 0 finished (status 1)",
            "worker 0 finished (exit status 0)",
        ],
    );
    assert_eq!(text(PYTEST_LEVELS, 3), lines(&expected));
}

#[test]
fn text_at_the_engines_level_shows_lifetimes_and_passing_sub_outcomes() {
    let out = text(UNITTEST, 3);
    assert!(
        out.starts_with(&lines(&[
            "aye · repo · 6 tests · 1 worker",
            "collected 6 tests in 1 ms",
            "",
            "worker 0 started (pid 30220, /repo/.venv/bin/python)",
            "batch 0 started on worker 0 (unittest, 6 tests)",
            "lifetime tests/test_module_setup.py started (module)",
            "lifetime tests/test_module_setup.py failed (0 ms)",
            "",
            "! e6d01  setup of tests/test_module_setup.py",
        ])),
        "{out}"
    );
    assert!(
        out.contains(&lines(&[
            "lifetime tests/test_outcomes.py started (module)",
            "lifetime tests/test_outcomes.py::Subtests started (class)",
            "✓ passed  tests/test_outcomes.py::Subtests::test_all_pass › (i=0)  0 ms",
            "✓ passed  tests/test_outcomes.py::Subtests::test_all_pass › (i=1)  0 ms",
            "✓ passed  tests/test_outcomes.py::Subtests::test_all_pass › (i=2)  0 ms",
            "✓ passed  tests/test_outcomes.py::Subtests::test_all_pass  0 ms",
            "✓ passed  tests/test_outcomes.py::Subtests::test_one_fails › (i=0)  0 ms",
            "✓ passed  tests/test_outcomes.py::Subtests::test_one_fails › (i=2)  0 ms",
            "",
            "✗ c9b76  tests/test_outcomes.py::Subtests::test_one_fails › (i=1)",
        ])),
        "{out}"
    );
    assert!(
        out.ends_with(&lines(&[
            "lifetime tests/test_outcomes.py::Subtests finished (1 ms)",
            "lifetime tests/test_outcomes.py finished (1 ms)",
            "batch 0 finished (status 1)",
            "worker 0 finished (exit status 0)",
            "",
            "✗ 1 failed · ! 2 errors · ✓ 1 passed   0.06 s",
            "details  aye show e6d01 c9b76  ·  every failure: aye show --failed",
        ])),
        "{out}"
    );
    // Below it, none of that.
    let full = text(UNITTEST, 2);
    assert!(
        !full.contains("lifetime ") && !full.contains("(i=0)"),
        "{full}"
    );
}

#[test]
fn every_failing_example_is_a_block_from_very_verbose() {
    let out = text(DOCTEST_CASCADE, 2);
    assert_eq!(out.matches("  rerun    ").count(), 6, "{out}");
    assert!(
        out.contains(
            "✗ 3ae8a  docs/test_cascade.txt::test_cascade.txt › line 10: >>> thing(5)\n  failed in the test · 0 ms\n"
        ),
        "{out}"
    );
}

#[test]
fn text_at_each_level_of_a_listing() {
    let cases = [
        "tests/test_levels.py::test_prints",
        "tests/test_levels.py::test_skipped",
        "tests/test_levels.py::test_xfail",
        "tests/test_levels.py::test_long",
        "tests/test_levels.py::test_dup",
        "collected 5 tests in 3 ms",
    ];
    // The listing is the run's result: even `-q` shows it.
    assert_eq!(text(PYTEST_LEVELS_CO, -1), lines(&cases));
    let default = [
        &cases[..],
        &[
            "",
            "warnings  1 of duplicate-definition, 1 of class-has-init",
        ][..],
    ]
    .concat();
    assert_eq!(text(PYTEST_LEVELS_CO, 0), lines(&default));
    assert_eq!(text(PYTEST_LEVELS_CO, 1), lines(&default));
    let full = [
        &["info[not-refined] tests/test_empty.py: pytest must import this file to know its tests (no test is visible statically; pytest imports the module to tell), but refinement is off; keeping the static guess"][..],
        &cases[..],
        &[
            "",
            "warning[duplicate-definition] tests/test_levels.py:37: test_dup is defined again on line 41; pytest only runs the last definition",
            "warning[class-has-init] tests/test_levels.py:45: cannot collect test class 'TestInit' because it has a __init__ constructor (from: tests/test_levels.py)",
        ][..],
    ]
    .concat();
    assert_eq!(text(PYTEST_LEVELS_CO, 2), lines(&full));
    assert_eq!(text(PYTEST_LEVELS_CO, 3), lines(&full));
}

#[test]
fn json_at_each_level_of_a_pytest_run() {
    let summary = r#"{"type":"summary","exit":1,"tests":5,"failed":1,"passed":2,"skipped":1,"expected_failure":1,"s":0.64"#;
    let line = r#"{"type":"fail","key":"4f3c4","id":"tests/test_levels.py::test_long","outcome":"failed","at":"tests/test_levels.py:24","why":"assert 'line 0\\nline...e 58\\nline 59' == 'LINE 0\\nline...E 58\\nline 59'"}"#;
    let failures = r#","failures":["4f3c4"],"more":"aye show <key> --output json"}"#;
    assert_eq!(json(PYTEST_LEVELS, -1), lines(&[&format!("{summary}}}")]));
    assert_eq!(
        json(PYTEST_LEVELS, 0),
        lines(&[&format!("{summary}{failures}")])
    );
    assert_eq!(
        json(PYTEST_LEVELS, 1),
        lines(&[line, &format!("{summary}{failures}")])
    );

    // -vv: the detail, bounded; the info.
    let detail = json(PYTEST_LEVELS, 2);
    let out = lines_of(&detail);
    assert_eq!(out.len(), 3);
    assert_eq!(
        out[0],
        r#"{"type":"diagnostic","severity":"info","path":"tests/test_empty.py","code":"no-tests","message":"claimed for tests by pytest, but has none"}"#
    );
    let fail: serde_json::Value = serde_json::from_str(out[1]).unwrap();
    assert_eq!(fail["code"], "assert text == lines(range(0, 60, 2))");
    let diff = fail["diff"].as_array().unwrap();
    assert_eq!(diff.len(), 30);
    assert_eq!(diff[29], "… 61 more lines");
    assert_eq!(fail["rerun"], "aye tests/test_levels.py::test_long");
    assert!(fail.get("frames").is_none() && fail.get("output").is_none());

    // -vvv: every frame, the output.
    let all = json(PYTEST_LEVELS, 3);
    let fail: serde_json::Value = serde_json::from_str(lines_of(&all)[1]).unwrap();
    assert_eq!(fail["frames"].as_array().unwrap().len(), 3);
    assert_eq!(
        fail["output"]["stdout"],
        "step 0\nstep 1\nstep 2\nstep 3\nstep 4\nstep 5"
    );
}

#[test]
fn json_at_each_level_of_a_failed_lifetime_and_subtest() {
    let summary =
        r#"{"type":"summary","exit":1,"tests":6,"failed":1,"errors":2,"passed":1,"s":0.06"#;
    let failures =
        r#","failures":["42640","6b6c8","e6d01","c9b76"],"more":"aye show <key> --output json"}"#;
    assert_eq!(json(UNITTEST, -1), lines(&[&format!("{summary}}}")]));
    assert_eq!(json(UNITTEST, 0), lines(&[&format!("{summary}{failures}")]));
    assert_eq!(
        json(UNITTEST, 2),
        lines(&[
            r#"{"type":"lifetime_fail","key":"e6d01","lifetime":"tests/test_module_setup.py","phase":"setup","at":"tests/test_module_setup.py:5","why":"ValueError: module setup failed","code":"raise ValueError(\"module setup failed\")","rerun":"aye tests/test_module_setup.py::First::test_first","ms":0,"blocked":["42640","6b6c8"]}"#,
            r#"{"type":"fail","key":"c9b76","id":"tests/test_outcomes.py::Subtests::test_one_fails","sub":"(i=1)","outcome":"failed","at":"tests/test_outcomes.py:64","why":"1 == 1","code":"self.assertNotEqual(i, 1)","rerun":"aye tests/test_outcomes.py::Subtests::test_one_fails","ms":0}"#,
            &format!("{summary}{failures}"),
        ])
    );
}

#[test]
fn json_lines_give_every_failing_example_its_line() {
    let out = json(DOCTEST_CASCADE, 1);
    let out = lines_of(&out);
    assert_eq!(out.len(), 7, "{out:#?}");
    assert_eq!(
        out[0],
        r#"{"type":"fail","key":"3ae8a","id":"docs/test_cascade.txt::test_cascade.txt","sub":"line 1: >>> from missing_module import thing","outcome":"failed","at":"docs/test_cascade.txt:1","why":"ModuleNotFoundError: No module named 'missing_module'"}"#
    );
    assert_eq!(
        out[5],
        r#"{"type":"fail","key":"3ae8a","id":"docs/test_cascade.txt::test_cascade.txt","sub":"line 10: >>> thing(5)","outcome":"failed","at":"docs/test_cascade.txt:10","why":"NameError: name 'thing' is not defined"}"#
    );
}

#[test]
fn json_at_each_level_of_a_listing() {
    let summary = r#"{"type":"summary","exit":0,"tests":5,"s":0.0}"#;
    let cases = [
        r#"{"type":"case","id":"tests/test_levels.py::test_prints"}"#,
        r#"{"type":"case","id":"tests/test_levels.py::test_skipped"}"#,
        r#"{"type":"case","id":"tests/test_levels.py::test_xfail"}"#,
        r#"{"type":"case","id":"tests/test_levels.py::test_long"}"#,
        r#"{"type":"case","id":"tests/test_levels.py::test_dup"}"#,
    ];
    assert_eq!(json(PYTEST_LEVELS_CO, -1), lines(&[summary]));
    let listing = lines(&[&cases[..], &[summary]].concat());
    assert_eq!(json(PYTEST_LEVELS_CO, 0), listing);
    assert_eq!(json(PYTEST_LEVELS_CO, 1), listing);
    let diagnostics = [
        r#"{"type":"diagnostic","severity":"info","path":"tests/test_empty.py","code":"not-refined","message":"pytest must import this file to know its tests (no test is visible statically; pytest imports the module to tell), but refinement is off; keeping the static guess"}"#,
        r#"{"type":"diagnostic","severity":"warning","path":"tests/test_levels.py","line":37,"code":"duplicate-definition","message":"test_dup is defined again on line 41; pytest only runs the last definition"}"#,
        r#"{"type":"diagnostic","severity":"warning","path":"tests/test_levels.py","line":45,"code":"class-has-init","message":"cannot collect test class 'TestInit' because it has a __init__ constructor (from: tests/test_levels.py)"}"#,
    ];
    let full = lines(&[&diagnostics[..], &cases[..], &[summary]].concat());
    assert_eq!(json(PYTEST_LEVELS_CO, 2), full);
    assert_eq!(json(PYTEST_LEVELS_CO, 3), full);
}
