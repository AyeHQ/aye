use aye_events::{Assertion, Difference, Failure, Frame, Phase, Resource, Scope, Side};

use super::frames::package_of;
use super::*;

#[test]
fn frames_outside_the_project_are_grouped_by_package() {
    let frame = |path: &str| Frame {
        path: path.into(),
        line: 1,
        ..Default::default()
    };
    let frames = [
        frame("tests/a.py"),
        frame("/venv/lib/python3.13/site-packages/requests/api.py"),
        frame("/venv/lib/python3.13/site-packages/requests/sessions.py"),
        frame("/venv/lib/python3.13/site-packages/urllib3/pool.py"),
        frame("src/b.py"),
        frame("/usr/lib/python3.13/socket.py"),
        frame("C:\\Python313\\Lib\\json\\decoder.py"),
        frame("<frozen importlib._bootstrap>"),
    ];
    let items: Vec<String> = frame_items(&frames)
        .into_iter()
        .map(|item| match item {
            FrameItem::Project(frame) => frame.path.to_string(),
            FrameItem::Others { count, package } => others(count, &package),
        })
        .collect();
    assert_eq!(
        items,
        [
            "tests/a.py",
            "2 frames in requests",
            "1 frame in urllib3",
            "src/b.py",
            "1 frame in socket",
            "1 frame in json",
            "1 frame in frozen importlib._bootstrap",
        ]
    );
    assert_eq!(package_of("/opt/tools/helper.py"), "tools");
}

#[test]
fn hints_about_pytest_flags_are_removed() {
    let diff: Vec<String> = [
        "",
        "At index 2 diff: 3 != 4",
        "Use -v to get more diff",
        "Omitting 3 identical items, use -vv to show",
        "Skipping 12 identical leading characters in diff, use -v to show",
        "- spam",
        "+ eggs",
        "",
        "...Full output truncated (3 lines hidden), use '-vv' to show",
    ]
    .map(String::from)
    .to_vec();
    assert_eq!(
        diff_lines(&diff),
        [
            "At index 2 diff: 3 != 4",
            "Omitting 3 identical items",
            "Skipping 12 identical leading characters in diff",
            "- spam",
            "+ eggs",
            "",
            "...Full output truncated (3 lines hidden)",
        ]
    );
}

fn failure(message: &str, detail: &str) -> Failure {
    Failure {
        message: message.into(),
        detail: Some(detail.into()),
        ..Failure::default()
    }
}

#[test]
fn the_rest_of_a_message_comes_from_the_frameworks_text() {
    // Python's traceback, as unittest writes it.
    let python = "Traceback (most recent call last):\n  File \"t.py\", line 5, in test_x\n    self.fail(\"a\\nb\")\nAssertionError: first\nsecond\n  third\n\n";
    assert_eq!(
        message_rest(&failure("first", python)),
        ["second", "  third"]
    );
    // An exception named by its module, as Python prints it.
    let qualified =
        "Traceback (most recent call last):\njson.decoder.JSONDecodeError: Expecting value\nat 1\n";
    assert_eq!(
        message_rest(&failure("JSONDecodeError: Expecting value", qualified)),
        ["at 1"]
    );
    // pytest's: the `E` lines, then the location, which isn't part of it.
    let pytest = "def test_x():\n>       assert f(2) == 5\nE       assert 4 == 5\nE        +  where 4 = f(2)\n\ntests/t.py:2: AssertionError";
    assert_eq!(
        message_rest(&failure("assert 4 == 5", pytest)),
        [" +  where 4 = f(2)"]
    );
    // Indented, as a doctest's "Exception raised:" is.
    let doctest = "Failed example:\n    boom()\nException raised:\n    Traceback (most recent call last):\n    ValueError: a\n    b\n";
    assert_eq!(message_rest(&failure("ValueError: a", doctest)), ["b"]);
}

#[test]
fn a_message_has_no_rest_when_nothing_follows_it_or_it_isnt_found() {
    assert_eq!(
        message_rest(&failure("boom", "ValueError: boom\n")),
        [] as [&str; 0]
    );
    // A doctest mismatch's message isn't in its text.
    let doctest = "Failed example:\n    f()\nExpected:\n    1\nGot:\n    2\n";
    assert_eq!(
        message_rest(&failure("f(): expected '1', got '2'", doctest)),
        [] as [&str; 0]
    );
    // pytest's location line ends with the exception's name, but isn't
    // the message.
    assert_eq!(
        message_rest(&failure(
            "AssertionError",
            "x\n\ntests/t.py:3: AssertionError"
        )),
        [] as [&str; 0]
    );
    // A source line that ends with the message isn't it either.
    assert_eq!(
        message_rest(&failure(
            "boom",
            "    raise X('boom')\n    print(boom)\nmore"
        )),
        [] as [&str; 0]
    );
    // An explained comparison's lines are its diff.
    let mut explained = failure("a != b", "AssertionError: a != b\n- a\n+ b\n");
    explained.assertion = Some(Assertion {
        op: "==".into(),
        left: "a".into(),
        right: "b".into(),
        diff: vec!["- a".into(), "+ b".into()],
        ..Default::default()
    });
    assert_eq!(message_rest(&explained), [] as [&str; 0]);
    assert_eq!(message_rest(&Failure::default()), [] as [&str; 0]);
}

fn assertion(left: &str, right: &str, diff: &[&str]) -> Assertion {
    Assertion {
        op: "==".into(),
        left: left.into(),
        right: right.into(),
        diff: diff.iter().map(|line| (*line).to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn explanations_lose_a_lone_hunk_header() {
    let doctest = assertion("2", "3", &["@@ -1 +1 @@", "-3", "+2"]);
    assert_eq!(explanation(&doctest), ["-3", "+2"]);
    assert!(only_restates(&doctest, &explanation(&doctest)));
    let hunks = assertion("a\nb", "a\nc", &["@@ -1 +1 @@", "-a", "@@ -9 +9 @@", "+c"]);
    assert_eq!(explanation(&hunks).len(), 4);
    // pytest's string diff shows the text without its quotes: more
    // than the operands.
    let strings = assertion("'spam'", "'eggs'", &["- eggs", "+ spam"]);
    assert!(!only_restates(&strings, &explanation(&strings)));
    let restated = assertion("1", "2", &["- 1", "+ 2"]);
    assert!(only_restates(&restated, &explanation(&restated)));
}

#[test]
fn operands_are_shown_when_they_add_to_the_message() {
    let with = |message: &str| Failure {
        message: message.into(),
        ..Failure::default()
    };
    // Already in the message.
    assert!(!operands_add(
        &with("assert 4 == 5"),
        &assertion("4", "5", &[])
    ));
    // Cut from it.
    assert!(operands_add(
        &with("{'a': 1, [41 chars]} != {...}"),
        &assertion("{'a': 1, 'b': 2}", "{'a': 1, 'b': 3}", &[])
    ));
    // Restated by the explanation.
    assert!(!operands_add(
        &with("f(): expected '3', got 'two'"),
        &assertion("2", "3", &["@@ -1 +1 @@", "-3", "+2"])
    ));
    // Spanning lines, with or without an explanation of them.
    assert!(!operands_add(
        &with("x"),
        &assertion("a\nb", "a\nc", &[" a", "-b", "+c"])
    ));
    assert!(operands_add(&with("x"), &assertion("a\nb", "a\nc", &[])));
}

fn compared(
    expected: Option<Side>,
    sources: (Option<&str>, Option<&str>),
    compare: Option<aye_events::Compare>,
) -> Failure {
    Failure {
        phase: Some(Phase::Test),
        message: "assert 2 == 3".into(),
        assertion: Some(Assertion {
            op: "==".into(),
            left: "2".into(),
            right: "3".into(),
            expected,
            left_source: sources.0.map(str::to_owned),
            right_source: sources.1.map(str::to_owned),
            compare,
            ..Default::default()
        }),
        ..Failure::default()
    }
}

fn compare(kind: &str, summary: &str, differs: Vec<Difference>) -> aye_events::Compare {
    aye_events::Compare {
        kind: kind.into(),
        summary: summary.into(),
        differs,
        ..aye_events::Compare::default()
    }
}

fn entry(at: Option<&str>, left: Option<&str>, right: Option<&str>) -> Difference {
    Difference {
        at: at.map(str::to_owned),
        left: left.map(str::to_owned),
        right: right.map(str::to_owned),
    }
}

#[test]
fn a_comparisons_own_message_is_the_one_the_test_gave() {
    let with = |message: &str, left_source: Option<&str>| {
        let mut failure = compared(None, (left_source, None), None);
        failure.message = message.into();
        failure
    };
    let own = |failure: &Failure| {
        own_message(failure, failure.assertion.as_ref().unwrap()).map(str::to_owned)
    };
    // pytest's own text, and pytest's with the test's message first.
    assert_eq!(own(&with("assert 2 == 3", None)), None);
    assert_eq!(
        own(&with("totals differ", None)).as_deref(),
        Some("totals differ")
    );
    // unittest's `first != second`, with `msg` after ` : `.
    assert_eq!(own(&with("{'a': 1} != {'a': 2}", None)), None);
    assert_eq!(
        own(&with("{'a': 1} != {'a': 2} : totals differ", None)).as_deref(),
        Some("totals differ")
    );
    // The doctest adapter's, which names the example.
    assert_eq!(
        own(&with(
            "cart.total(): expected '1', got '2'",
            Some("cart.total()")
        )),
        None
    );
}

#[test]
fn why_says_what_was_expected_and_what_came() {
    let dict = compare(
        "dict",
        "dict differs in 1 of 2 keys",
        vec![entry(Some("['pear']"), Some("2"), Some("3"))],
    );
    // The expected side known: the actual operand's source, where.
    let failure = compared(
        Some(Side::Right),
        (Some("cart.summary()"), Some("{'pear': 3}")),
        Some(dict.clone()),
    );
    assert_eq!(
        why(&failure, true),
        "cart.summary()['pear']: expected 3, actual 2"
    );
    // On the left, the right operand is the actual one.
    let failure = compared(
        Some(Side::Left),
        (Some("{'pear': 2}"), Some("cart.summary()")),
        Some(dict.clone()),
    );
    assert_eq!(
        why(&failure, true),
        "cart.summary()['pear']: expected 2, actual 3"
    );
    // A source too long, or spanning lines, is left out or cut.
    let long = "inventory.levels_for_every_warehouse_in(region)";
    let failure = compared(Some(Side::Right), (Some(long), None), Some(dict.clone()));
    assert_eq!(why(&failure, true), "['pear']: expected 3, actual 2");
    let failure = compared(
        Some(Side::Right),
        (Some("build(\n    1,\n)"), None),
        Some(dict.clone()),
    );
    assert_eq!(why(&failure, true), "build(…['pear']: expected 3, actual 2");
    // Neither side literal: each named by its source.
    let failure = compared(None, (Some("a.total()"), Some("b.total()")), Some(dict));
    assert_eq!(
        why(&failure, true),
        "['pear']: a.total() is 2, b.total() is 3"
    );
    // No entries (numbers, dates): the operands themselves.
    let failure = compared(
        None,
        (Some("cart.total()"), None),
        Some(compare("number", "differ by 1", Vec::new())),
    );
    assert_eq!(why(&failure, true), "cart.total() is 2, right is 3");
    // Several differences: the summary.
    let failure = compared(
        Some(Side::Right),
        (Some("xs"), None),
        Some(compare(
            "list",
            "list differs at 2 of 5 positions",
            vec![
                entry(Some("[1]"), Some("2"), Some("3")),
                entry(Some("[3]"), Some("4"), Some("5")),
            ],
        )),
    );
    assert_eq!(
        why(&failure, true),
        "list differs at 2 of 5 positions: [1], [3]"
    );
    // An item of a set only one side has: the summary too.
    let failure = compared(
        Some(Side::Right),
        (Some("tags"), None),
        Some(compare(
            "set",
            "set: 1 missing",
            vec![entry(None, None, Some("'b'"))],
        )),
    );
    assert_eq!(why(&failure, true), "set: 1 missing");
    // Text: where it differs, in brackets, or the summary.
    let text = compare(
        "str",
        "strings differ at line 2, column 5",
        vec![entry(
            Some("line 2, column 5"),
            Some("'wrld'"),
            Some("'world'"),
        )],
    );
    let failure = compared(
        Some(Side::Right),
        (Some("greeting"), None),
        Some(text.clone()),
    );
    assert_eq!(
        why(&failure, true),
        "greeting (line 2, column 5): expected 'world', actual 'wrld'"
    );
    let failure = compared(Some(Side::Right), (None, None), Some(text));
    assert_eq!(
        why(&failure, true),
        "strings differ at line 2, column 5: expected 'world', actual 'wrld'"
    );
    // One line of text, both shown whole: the place says nothing more.
    let line = compare(
        "str",
        "strings differ at line 1, column 2",
        vec![entry(
            Some("line 1, column 2"),
            Some("'wrld'"),
            Some("'world'"),
        )],
    );
    let failure = compared(
        Some(Side::Right),
        (Some("greeting"), None),
        Some(line.clone()),
    );
    assert_eq!(
        why(&failure, true),
        "greeting: expected 'world', actual 'wrld'"
    );
    let failure = compared(Some(Side::Right), (None, None), Some(line));
    assert_eq!(why(&failure, true), "expected 'world', actual 'wrld'");
    // `==` with an expected side, without a comparison.
    let failure = compared(Some(Side::Right), (None, None), None);
    assert_eq!(why(&failure, true), "expected 3, actual 2");
    // Values are cut to 40 characters.
    let mut failure = compared(Some(Side::Right), (None, None), None);
    failure.assertion.as_mut().unwrap().right = "x".repeat(100);
    assert_eq!(
        why(&failure, true),
        format!("expected {}…, actual 2", "x".repeat(39))
    );
}

#[test]
fn why_blames_the_fixture() {
    let failed = |phase, resource: Option<&str>| Failure {
        phase: Some(phase),
        message: "RuntimeError: boom".into(),
        resource: resource.map(str::to_owned),
        ..Failure::default()
    };
    assert_eq!(
        why(&failed(Phase::Setup, Some("db")), true),
        "fixture db: RuntimeError: boom"
    );
    assert_eq!(
        why(&failed(Phase::Teardown, Some("db")), true),
        "passed; fixture db failed in teardown: RuntimeError: boom"
    );
    assert_eq!(
        why(&failed(Phase::Teardown, None), true),
        "teardown failed: RuntimeError: boom"
    );
    // A lifetime's teardown is no test's.
    assert_eq!(
        why(&failed(Phase::Teardown, None), false),
        "RuntimeError: boom"
    );
    assert_eq!(why(&failed(Phase::Setup, None), true), "RuntimeError: boom");
    // pytest's own fixture for `setup_module` is named for what it runs.
    let xunit = Some("_xunit_setup_module_fixture_tests.test_a");
    assert_eq!(
        why(&failed(Phase::Setup, xunit), true),
        "setup_module: RuntimeError: boom"
    );
    assert_eq!(
        why(&failed(Phase::Teardown, xunit), true),
        "passed; teardown_module failed: RuntimeError: boom"
    );
    assert_eq!(
        setup_chain(&[
            Resource {
                name: "_xunit_setup_class_fixture_TestA".into(),
                scope: Scope::Class,
            },
            Resource {
                name: "cart".into(),
                scope: Scope::Function,
            },
        ]),
        "setup_class (class) → cart (function)"
    );
}

#[test]
fn why_adds_the_hint_to_the_message() {
    let failure = Failure {
        phase: Some(Phase::Setup),
        message: "fixture 'kart' not found".into(),
        hint: Some("did you mean 'cart'?".into()),
        ..Failure::default()
    };
    assert_eq!(
        why(&failure, true),
        "fixture 'kart' not found; did you mean 'cart'?"
    );
    // The whole line is cut to 160 characters.
    let long = Failure {
        message: "m".repeat(400),
        ..Failure::default()
    };
    assert_eq!(why(&long, true).chars().count(), 160);
}
