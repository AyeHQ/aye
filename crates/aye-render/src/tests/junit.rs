use aye_events::{Captured, Event, Failure, Outcome, Phase, Record, TestId, TestResult};

use super::{feed, lines, streams};
use crate::{Junit, Reporter};

fn junit(records: &[Record]) -> String {
    let mut junit = Junit::new("unused.xml");
    for record in records {
        junit.report(record).unwrap();
    }
    junit.xml()
}

#[test]
fn every_outcome_maps_onto_junit() {
    assert_eq!(
        junit(&streams::pass_only()),
        lines(&[
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<testsuites name="aye" tests="5" failures="0" errors="0" skipped="2" time="0.840">"#,
            r#"  <testsuite name="aye" tests="5" failures="0" errors="0" skipped="2" time="0.840">"#,
            r#"    <testcase classname="tests.test_math" name="test_add" file="tests/test_math.py" line="3" time="0.001"/>"#,
            r#"    <testcase classname="tests.test_math" name="test_param[1]" file="tests/test_math.py" line="7" time="0.002"/>"#,
            r#"    <testcase classname="tests.test_math" name="test_net" file="tests/test_math.py" line="12" time="0.000">"#,
            r#"      <skipped message="no network"/>"#,
            r"    </testcase>",
            r#"    <testcase classname="tests.test_math" name="test_bug" file="tests/test_math.py" line="16" time="0.003">"#,
            r#"      <skipped message="bug #12" type="expected_failure"/>"#,
            r"    </testcase>",
            r#"    <testcase classname="tests.test_math" name="test_fixed" file="tests/test_math.py" line="20" time="0.001">"#,
            r#"      <system-out>fixed &lt;now&gt; &amp; "done"</system-out>"#,
            r"    </testcase>",
            r"  </testsuite>",
            r"</testsuites>",
        ])
    );
}

#[test]
fn failures_and_errors_carry_their_detail() {
    assert_eq!(
        junit(&streams::mixed()),
        lines(&[
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<testsuites name="aye" tests="7" failures="4" errors="2" skipped="0" time="1.234">"#,
            r#"  <testsuite name="aye" tests="7" failures="4" errors="2" skipped="0" time="1.234">"#,
            r#"    <testcase classname="tests.test_cart" name="test_total" file="tests/test_cart.py" line="10" time="0.005">"#,
            r#"      <failure message="assert &apos;spam&apos; == &apos;eggs&apos;" type="AssertionError">tests/test_cart.py:12: in test_total"#,
            r"AssertionError</failure>",
            r"      <system-out>loading cart",
            r"item 1",
            r"item 2",
            r"item 3",
            r"total computed",
            r"----- log -----",
            r"WARNING cart: price missing</system-out>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_cart" name="test_ok" file="tests/test_cart.py" line="20" time="0.001"/>"#,
            r#"    <testcase classname="tests.test_db" name="test_load" file="tests/test_db.py" line="5" time="0.003">"#,
            r#"      <error message="ConnectionError: refused" type="ConnectionError">tests/conftest.py:8 in db"#,
            r"    return connect()",
            r"src/app/db.py:30 in connect",
            r"    return socket.create_connection(ADDR)",
            r"/usr/lib/python3.13/socket.py:851 in create_connection",
            r"    raise exceptions[0]",
            r"ConnectionError: refused</error>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_io" name="test_write" file="tests/test_io.py" line="18" time="0.007">"#,
            r#"      <failure message="assert False" type="AssertionError">tests/test_io.py:20 in test_write"#,
            r"    assert False",
            r"assert False</failure>",
            r#"      <error message="RuntimeError: cleanup failed" type="RuntimeError">tests/test_io.py:14 in tmp"#,
            r"    raise RuntimeError('cleanup failed')",
            r"RuntimeError: cleanup failed</error>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_users" name="test_find[1]" file="tests/test_users.py" line="3" time="0.001">"#,
            r#"      <failure message="KeyError: &apos;user1&apos;" type="KeyError">src/app/users.py:41 in find"#,
            r"    return USERS[name]",
            r"KeyError: 'user1'</failure>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_users" name="test_find[2]" file="tests/test_users.py" line="3" time="0.001">"#,
            r#"      <failure message="KeyError: &apos;user2&apos;" type="KeyError">src/app/users.py:41 in find"#,
            r"    return USERS[name]",
            r"KeyError: 'user2'</failure>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_users" name="test_find[3]" file="tests/test_users.py" line="3" time="0.001">"#,
            r#"      <failure message="KeyError: &apos;user1&apos;" type="KeyError">src/app/users.py:41 in find"#,
            r"    return USERS[name]",
            r"KeyError: 'user1'</failure>",
            r"    </testcase>",
            r"  </testsuite>",
            r"</testsuites>",
        ])
    );
}

#[test]
fn a_failed_lifetime_lands_on_the_tests_it_blocked() {
    assert_eq!(
        junit(&streams::lifetime()),
        lines(&[
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<testsuites name="aye" tests="5" failures="0" errors="4" skipped="0" time="0.050">"#,
            r#"  <testsuite name="aye" tests="5" failures="0" errors="4" skipped="0" time="0.050">"#,
            r#"    <testcase classname="tests.test_db.TestDb" name="test_a" file="tests/test_db.py" line="12" time="0.000">"#,
            r#"      <error message="setup of class:tests/test_db.py::TestDb failed: RuntimeError: no database" type="RuntimeError">Traceback (most recent call last):"#,
            r"RuntimeError: no database</error>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_db.TestDb" name="test_b" file="tests/test_db.py" line="15" time="0.000">"#,
            r#"      <error message="setup of class:tests/test_db.py::TestDb failed: RuntimeError: no database" type="RuntimeError">Traceback (most recent call last):"#,
            r"RuntimeError: no database</error>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_db.TestDb" name="test_c" file="tests/test_db.py" line="18" time="0.000">"#,
            r#"      <error message="setup of class:tests/test_db.py::TestDb failed: RuntimeError: no database" type="RuntimeError">Traceback (most recent call last):"#,
            r"RuntimeError: no database</error>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_db" name="test_free" file="tests/test_db.py" line="22" time="0.001"/>"#,
            r#"    <testcase classname="" name="teardown of module:tests/test_db.py" file="tests/test_db.py" line="4" time="0.000">"#,
            r#"      <error message="OSError: busy" type="OSError">tests/test_db.py:4 in tearDownModule"#,
            r"    shutil.rmtree(TMP)",
            r"OSError: busy</error>",
            r"    </testcase>",
            r"  </testsuite>",
            r"</testsuites>",
        ])
    );
}

/// Under `maxfail` the engine reports a failed lifetime before the tests it
/// blocks: the report is the same as when it comes after them, with no test
/// case of its own.
#[test]
fn a_failed_lifetime_reported_before_the_tests_it_blocked_lands_on_them() {
    let after = streams::lifetime();
    let mut before = after.clone();
    let finished = before
        .iter()
        .position(|record| {
            matches!(&record.event, Event::LifetimeFinished { failure: Some(failure), .. }
                if failure.phase == Some(Phase::Setup))
        })
        .unwrap();
    let first_blocked = before
        .iter()
        .position(
            |record| matches!(&record.event, Event::TestFinished(result) if result.cause.is_some()),
        )
        .unwrap();
    let lifetime = before.remove(finished);
    before.insert(first_blocked, lifetime);
    assert_ne!(before, after);
    assert_eq!(junit(&before), junit(&after));
}

#[test]
fn collection_errors_are_test_cases() {
    assert_eq!(
        junit(&streams::diagnostics()),
        lines(&[
            r#"<?xml version="1.0" encoding="utf-8"?>"#,
            r#"<testsuites name="aye" tests="2" failures="0" errors="1" skipped="0" time="0.090">"#,
            r#"  <testsuite name="aye" tests="2" failures="0" errors="1" skipped="0" time="0.090">"#,
            r#"    <testcase classname="" name="tests.test_bad" file="tests/test_bad.py" line="3" time="0.000">"#,
            r#"      <error message="Expected an expression" type="syntax-error">Expected an expression</error>"#,
            r"    </testcase>",
            r#"    <testcase classname="tests.test_dup" name="test_x" file="tests/test_dup.py" line="10" time="0.001"/>"#,
            r"  </testsuite>",
            r"</testsuites>",
        ])
    );
}

#[test]
fn text_xml_cannot_carry_is_shown_visibly() {
    let mut result = TestResult::new(TestId::new("t.py::test_x[\u{1}]"), Outcome::Error);
    result.failures = vec![Failure {
        phase: Some(Phase::Setup),
        message: "bad \u{1b}[31m\"red\"\u{0}".into(),
        detail: Some("line 1\r\nline 2 <&>\u{7}".into()),
        ..Failure::default()
    }];
    result.output = Captured {
        stderr: "\u{1b}[0m]]>".into(),
        ..Captured::default()
    };
    let xml = junit(&[Record::new(Event::TestFinished(Box::new(result)))]);
    assert!(
        xml.contains(r#"<testcase classname="t" name="test_x[#x01]" file="t.py" time="0.000">"#)
    );
    assert!(xml.contains(
        "<error message=\"bad #x1B[31m&quot;red&quot;#x00\">line 1&#13;\nline 2 &lt;&amp;&gt;#x07</error>"
    ));
    assert!(xml.contains("<system-err>#x1B[0m]]&gt;</system-err>"));
}

#[test]
fn the_report_is_written_on_finish() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reports/nested/junit.xml");
    let mut junit = Junit::new(&path);
    assert_eq!(junit.path(), path);
    feed(&mut junit, &streams::mixed());
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(written, junit.xml());
    assert!(written.contains(r#"tests="7" failures="4" errors="2""#));
}

#[test]
fn failed_subtests_keep_their_failure() {
    let id = TestId::new("tests/test_s.py::test_sub");
    let mut parent = TestResult::new(id.clone(), Outcome::Failed);
    parent.failures.push(Failure {
        phase: Some(Phase::Test),
        message: "contains 1 failed subtest".into(),
        ..Failure::default()
    });
    let sub = |name: &str, outcome, failure| {
        Record::new(Event::SubOutcome {
            id: id.clone(),
            name: name.into(),
            outcome,
            reason: None,
            failure,
            duration: std::time::Duration::ZERO,
        })
    };
    let records = [
        sub("case (i=0)", Outcome::Passed, None),
        sub(
            "case (i=1)",
            Outcome::Failed,
            Some(Failure {
                phase: Some(Phase::Test),
                exception: Some("AssertionError".into()),
                message: "assert 1 != 1".into(),
                detail: Some("tests/test_s.py:4: AssertionError".into()),
                ..Failure::default()
            }),
        ),
        Record::new(Event::TestFinished(Box::new(parent))),
    ];
    let xml = junit(&records);
    assert!(
        xml.contains(&lines(&[
            r#"<testsuites name="aye" tests="1" failures="1" errors="0" skipped="0" time="0.000">"#,
        ])),
        "{xml}"
    );
    assert!(
        xml.contains(&lines(&[
            r#"    <testcase classname="tests.test_s" name="test_sub" file="tests/test_s.py" time="0.000">"#,
            r#"      <failure message="case (i=1): assert 1 != 1" type="AssertionError">tests/test_s.py:4: AssertionError</failure>"#,
            r#"      <failure message="contains 1 failed subtest">contains 1 failed subtest</failure>"#,
            r"    </testcase>",
        ])),
        "{xml}"
    );
}

#[test]
fn a_report_that_cannot_be_written_names_its_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("pyproject.toml");
    std::fs::write(&blocker, "").unwrap();
    let path = blocker.join("out.xml");
    let mut junit = Junit::new(&path);
    let err = junit.finish().unwrap_err();
    assert!(
        err.to_string().starts_with(&format!(
            "cannot write the JUnit XML report to {}: ",
            path.display()
        )),
        "{err}"
    );
}

#[test]
fn a_prefix_goes_before_every_classname() {
    let prefixed = |prefix: &str, records: &[Record]| {
        let mut junit = Junit::new("unused.xml").with_prefix(prefix);
        for record in records {
            junit.report(record).unwrap();
        }
        junit.xml()
    };
    let xml = prefixed("ci", &streams::pass_only());
    assert!(
        xml.contains(r#"<testcase classname="ci.tests.test_math" name="test_add""#),
        "{xml}"
    );
    assert!(!xml.contains(r#"classname="tests."#), "{xml}");
    // A collection error has no classname of its own: the prefix alone, as
    // in pytest.
    let xml = prefixed("ci", &streams::diagnostics());
    assert!(
        xml.contains(r#"<testcase classname="ci" name="tests.test_bad""#),
        "{xml}"
    );
    assert!(
        xml.contains(r#"<testcase classname="ci.tests.test_dup" name="test_x""#),
        "{xml}"
    );
    // An empty prefix changes nothing.
    assert_eq!(
        prefixed("", &streams::pass_only()),
        junit(&streams::pass_only())
    );
}

/// The stream of `/tmp/aye-x1/a` (C24): a test recording properties, one
/// passing quietly, one failing with output on every stream, one failing in
/// setup, one skipped, one expected to fail.
fn recorded() -> Vec<Record> {
    let ms = std::time::Duration::from_millis;
    let id = |name: &str| TestId::new(format!("tests/test_x.py::{name}"));
    let output = |stdout: &str, stderr: &str, log: &str| Captured {
        stdout: stdout.into(),
        stderr: stderr.into(),
        log: log.into(),
        truncated: false,
    };
    let case = |name: &str, line: u32| {
        let mut case = aye_events::Case::new(id(name).0, "pytest");
        case.span = Some(aye_events::Span {
            path: "tests/test_x.py".into(),
            start: 0,
            end: 0,
            line: Some(line),
        });
        Record::new(Event::CaseCollected(Box::new(case)))
    };
    let property = |name: &str, value: &str| {
        Record::new(Event::RunProperty {
            name: name.into(),
            value: value.into(),
        })
    };

    let mut prop = TestResult::new(id("test_prop"), Outcome::Passed);
    prop.duration = ms(3);
    prop.body_duration = Some(ms(1));
    prop.output = output("noise\n", "", "");
    prop.properties = vec![
        ("example_key".into(), "1".into()),
        ("other".into(), "a<b".into()),
    ];
    let quiet = TestResult::new(id("test_quiet"), Outcome::Passed);
    let mut fail = TestResult::new(id("test_fail"), Outcome::Failed);
    fail.duration = ms(2);
    fail.output = output("out\n", "err\n", "WARNING  x:test_x.py:15 logged");
    fail.failures.push(Failure {
        phase: Some(Phase::Test),
        exception: Some("AssertionError".into()),
        message: "assert 0".into(),
        detail: Some("E       assert 0".into()),
        ..Failure::default()
    });
    let mut setup = TestResult::new(id("test_setup_error"), Outcome::Error);
    setup.output = output("setup out\n", "", "");
    setup.failures.push(Failure {
        phase: Some(Phase::Setup),
        exception: Some("RuntimeError".into()),
        message: "RuntimeError: boom".into(),
        detail: Some("E       RuntimeError: boom".into()),
        ..Failure::default()
    });
    let mut skip = TestResult::new(id("test_skip"), Outcome::Skipped);
    skip.reason = Some("nope".into());
    skip.output = output("before skip\n", "", "");
    let mut xfail = TestResult::new(id("test_xfail"), Outcome::ExpectedFailure);
    xfail.reason = Some("known".into());
    xfail.output = output("xf\n", "", "");

    vec![
        case("test_prop", 3),
        case("test_quiet", 9),
        case("test_fail", 12),
        case("test_setup_error", 23),
        case("test_skip", 26),
        case("test_xfail", 30),
        property("ARCH", "PPC"),
        Record::new(Event::TestFinished(Box::new(prop))),
        // Every batch's session records it again: written once.
        property("ARCH", "PPC"),
        property("STORAGE", "CEPH"),
        Record::new(Event::TestFinished(Box::new(quiet))),
        Record::new(Event::TestFinished(Box::new(fail))),
        Record::new(Event::TestFinished(Box::new(setup))),
        Record::new(Event::TestFinished(Box::new(skip))),
        Record::new(Event::TestFinished(Box::new(xfail))),
    ]
}

fn junit_with(options: crate::JunitOptions, records: &[Record]) -> String {
    let mut junit = Junit::new("unused.xml").with_options(options);
    for record in records {
        junit.report(record).unwrap();
    }
    junit.xml()
}

/// C24: what tests record reaches the report, by default as aye writes it.
#[test]
fn recorded_properties_go_on_their_test_case_and_the_suite() {
    let xml = junit(&recorded());
    assert!(
        xml.contains(&lines(&[
            r#"  <testsuite name="aye" tests="6" failures="1" errors="1" skipped="2" time="0.005">"#,
            r"    <properties>",
            r#"      <property name="ARCH" value="PPC"/>"#,
            r#"      <property name="STORAGE" value="CEPH"/>"#,
            r"    </properties>",
            r#"    <testcase classname="tests.test_x" name="test_prop" file="tests/test_x.py" line="3" time="0.003">"#,
            r"      <properties>",
            r#"        <property name="example_key" value="1"/>"#,
            r#"        <property name="other" value="a&lt;b"/>"#,
            r"      </properties>",
            r"      <system-out>noise</system-out>",
            r"    </testcase>",
        ])),
        "{xml}"
    );
}

/// C24: pytest's `junit_*` settings, as pytest 9.1.1 writes `/tmp/aye-x1/a`
/// with `junit_suite_name = "mysuite"` and `junit_logging = "all"` (the
/// default xunit2 family: no `file` or `line`), but for pytest's second copy
/// of a skipped test's output.
#[test]
fn pytests_settings_shape_the_report() {
    let options = crate::JunitOptions {
        suite_name: "mysuite".into(),
        logging: crate::JunitLogging::All,
        log_passing_tests: true,
        duration: crate::JunitDuration::Total,
        family: crate::JunitFamily::Xunit2,
    };
    let xml = junit_with(options, &recorded());
    let dashes = |header: &str| {
        let pad = "-".repeat(33);
        format!("{pad} {header} {pad}")
    };
    let (log, out, err) = (
        dashes("Captured Log"),
        dashes("Captured Out"),
        dashes("Captured Err"),
    );
    assert!(
        xml.contains(&lines(&[
            r#"  <testsuite name="mysuite" tests="6" failures="1" errors="1" skipped="2" time="0.005">"#,
            r"    <properties>",
            r#"      <property name="ARCH" value="PPC"/>"#,
            r#"      <property name="STORAGE" value="CEPH"/>"#,
            r"    </properties>",
            r#"    <testcase classname="tests.test_x" name="test_prop" time="0.003">"#,
            r"      <properties>",
            r#"        <property name="example_key" value="1"/>"#,
            r#"        <property name="other" value="a&lt;b"/>"#,
            r"      </properties>",
            &format!("      <system-out>{log}"),
            "",
            &out,
            "noise",
            "",
            "</system-out>",
            &format!("      <system-err>{err}"),
            "",
            "</system-err>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_x" name="test_quiet" time="0.000">"#,
            &format!("      <system-out>{log}"),
            "",
            &out,
            "",
            "</system-out>",
            &format!("      <system-err>{err}"),
            "",
            "</system-err>",
            r"    </testcase>",
            r#"    <testcase classname="tests.test_x" name="test_fail" time="0.002">"#,
            r#"      <failure message="assert 0" type="AssertionError">E       assert 0</failure>"#,
            &format!("      <system-out>{log}"),
            "WARNING  x:test_x.py:15 logged",
            &out,
            "out",
            "",
            "</system-out>",
            &format!("      <system-err>{err}"),
            "err",
            "",
            "</system-err>",
            r"    </testcase>",
        ])),
        "{xml}"
    );
    assert!(!xml.contains("file="), "{xml}");
}

/// C24: `junit_logging = system-out`, `junit_log_passing_tests = false`,
/// `junit_family = xunit1` and `junit_duration_report = call`, as pytest
/// writes them: output only for a test that failed in its body or was
/// skipped, `file` and `line` (aye's 1-based), the body's time.
#[test]
fn passing_tests_can_leave_their_output_out() {
    let options = crate::JunitOptions {
        suite_name: "mysuite".into(),
        logging: crate::JunitLogging::SystemOut,
        log_passing_tests: false,
        duration: crate::JunitDuration::Body,
        family: crate::JunitFamily::Xunit1,
    };
    let xml = junit_with(options, &recorded());
    let out = format!("{pad} Captured Out {pad}", pad = "-".repeat(33));
    for expected in [
        lines(&[
            r#"    <testcase classname="tests.test_x" name="test_prop" file="tests/test_x.py" line="3" time="0.001">"#,
            r"      <properties>",
        ]),
        lines(&[
            r#"    <testcase classname="tests.test_x" name="test_quiet" file="tests/test_x.py" line="9" time="0.000"/>"#,
        ]),
        lines(&[
            r#"      <failure message="assert 0" type="AssertionError">E       assert 0</failure>"#,
            &format!("      <system-out>{out}"),
            "out",
            "",
            "</system-out>",
            r"    </testcase>",
        ]),
        lines(&[
            r#"      <error message="RuntimeError: boom" type="RuntimeError">E       RuntimeError: boom</error>"#,
            r"    </testcase>",
        ]),
        lines(&[
            r#"      <skipped message="nope"/>"#,
            &format!("      <system-out>{out}"),
            "before skip",
            "",
            "</system-out>",
        ]),
        lines(&[
            r#"      <skipped message="known" type="expected_failure"/>"#,
            r"    </testcase>",
        ]),
    ] {
        assert!(xml.contains(&expected), "{expected}\n---\n{xml}");
    }
    // A test without a body of its own timed: its whole time.
    assert!(
        xml.contains(r#"name="test_fail" file="tests/test_x.py" line="12" time="0.002""#),
        "{xml}"
    );
    assert!(!xml.contains("Captured Log"), "{xml}");
}

/// `junit_logging = no`, pytest's default: no output at all.
#[test]
fn pytests_default_writes_no_output() {
    let options = crate::JunitOptions {
        suite_name: "pytest".into(),
        logging: crate::JunitLogging::No,
        log_passing_tests: true,
        duration: crate::JunitDuration::Total,
        family: crate::JunitFamily::Xunit2,
    };
    let xml = junit_with(options, &recorded());
    assert!(!xml.contains("<system-"), "{xml}");
    assert!(xml.contains(r#"<testsuite name="pytest" "#), "{xml}");
    assert!(
        xml.contains(r#"<testcase classname="tests.test_x" name="test_quiet" time="0.000"/>"#),
        "{xml}"
    );
}
