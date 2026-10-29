//! Recorded runs, as the runner would log them.

use std::time::Duration;

use aye_events::{
    Assertion, Captured, Case, Certainty, Counts, Diagnostic, Envelope, Event, ExitCode, Failure,
    Frame, Location, Outcome, Phase, Record, Reproduce, SCHEMA_VERSION, Severity, Span, TestId,
    TestResult,
};

const fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn log(events: Vec<Event>) -> Vec<Record> {
    events
        .into_iter()
        .zip(0..)
        .map(|(event, seq)| Record {
            envelope: Envelope {
                run: Some("r1".into()),
                seq: Some(seq),
                ..Envelope::default()
            },
            event,
        })
        .collect()
}

fn run_started(collect_only: bool) -> Event {
    Event::RunStarted {
        schema_version: SCHEMA_VERSION,
        aye_version: "0.0.1".into(),
        root: "/repo".into(),
        collect_only,
        workers: 2,
        run: None,
    }
}

fn case(id: &str, line: Option<u32>, certainty: Certainty) -> Event {
    let mut case = Case::new(id, "pytest");
    case.span = line.map(|line| Span {
        path: case.id.path().to_project_path(),
        start: 0,
        end: 0,
        line: Some(line),
    });
    case.certainty = certainty;
    Event::CaseCollected(Box::new(case))
}

fn collected(selected: u32, deselected: u32) -> Event {
    Event::CollectionFinished {
        selected,
        deselected,
        refined_files: 0,
        duration: Duration::from_micros(4200),
    }
}

fn finished(id: &str, outcome: Outcome, duration: Duration) -> TestResult {
    let mut result = TestResult::new(TestId::new(id), outcome);
    result.duration = duration;
    result
}

fn test(result: TestResult) -> Event {
    Event::TestFinished(Box::new(result))
}

fn frame(path: &str, line: u32, function: &str, source: &str) -> Frame {
    Frame {
        path: path.into(),
        line,
        function: Some(function.into()),
        source: Some(source.into()),
        ..Default::default()
    }
}

fn run_finished(exit_code: ExitCode, counts: Counts, duration: Duration) -> Event {
    Event::RunFinished {
        exit_code,
        counts,
        duration,
        stopped: None,
    }
}

/// Every test passes or is skipped.
pub(crate) fn pass_only() -> Vec<Record> {
    let mut skipped = finished("tests/test_math.py::test_net", Outcome::Skipped, ms(0));
    skipped.reason = Some("no network".into());
    let mut expected = finished(
        "tests/test_math.py::test_bug",
        Outcome::ExpectedFailure,
        ms(3),
    );
    expected.reason = Some("bug #12".into());
    let mut unexpected = finished(
        "tests/test_math.py::test_fixed",
        Outcome::UnexpectedPass,
        ms(1),
    );
    unexpected.output.stdout = "fixed <now> & \"done\"\n".into();
    log(vec![
        run_started(false),
        Event::CollectionStarted,
        case("tests/test_math.py::test_add", Some(3), Certainty::Exact),
        case(
            "tests/test_math.py::test_param",
            Some(7),
            Certainty::Partial,
        ),
        case("tests/test_math.py::test_net", Some(12), Certainty::Exact),
        case("tests/test_math.py::test_bug", Some(16), Certainty::Exact),
        case("tests/test_math.py::test_fixed", Some(20), Certainty::Exact),
        collected(5, 2),
        Event::WorkerStarted {
            pid: 4242,
            python: "3.13.1".into(),
            version: None,
        },
        Event::TestStarted {
            id: TestId::new("tests/test_math.py::test_add"),
        },
        test(finished(
            "tests/test_math.py::test_add",
            Outcome::Passed,
            Duration::from_micros(1300),
        )),
        test(finished(
            "tests/test_math.py::test_param[1]",
            Outcome::Passed,
            ms(2),
        )),
        test(skipped),
        test(expected),
        test(unexpected),
        Event::WorkerFinished {
            exit_status: Some(0),
            error: None,
        },
        run_finished(
            ExitCode::Ok,
            Counts {
                passed: 2,
                skipped: 1,
                expected_failure: 1,
                unexpected_pass: 1,
                deselected: 2,
                ..Counts::default()
            },
            ms(840),
        ),
    ])
}

fn assertion_failure() -> Failure {
    Failure {
        phase: Some(Phase::Test),
        exception: Some("AssertionError".into()),
        message: "assert 'spam' == 'eggs'".into(),
        assertion: Some(Assertion {
            op: "==".into(),
            left: "'spam'".into(),
            right: "'eggs'".into(),
            diff: vec![
                String::new(),
                "- eggs".into(),
                "+ spam".into(),
                "Use -v to get more diff".into(),
            ],
            ..Default::default()
        }),
        location: Some(Location {
            path: "tests/test_cart.py".into(),
            line: 12,
        }),
        frames: vec![frame(
            "tests/test_cart.py",
            12,
            "test_total",
            "assert cart.name() == 'eggs'",
        )],
        signature: "sig-spam".into(),
        detail: Some("tests/test_cart.py:12: in test_total\nAssertionError".into()),
        ..Default::default()
    }
}

fn setup_failure() -> Failure {
    Failure {
        phase: Some(Phase::Setup),
        exception: Some("ConnectionError".into()),
        message: "ConnectionError: refused".into(),
        assertion: None,
        location: Some(Location {
            path: "/usr/lib/python3.13/socket.py".into(),
            line: 851,
        }),
        frames: vec![
            frame("tests/conftest.py", 8, "db", "return connect()"),
            frame(
                "src/app/db.py",
                30,
                "connect",
                "return socket.create_connection(ADDR)",
            ),
            frame(
                "/usr/lib/python3.13/socket.py",
                851,
                "create_connection",
                "raise exceptions[0]",
            ),
        ],
        signature: "sig-refused".into(),
        detail: None,
        ..Default::default()
    }
}

fn key_error(id_number: u32) -> Failure {
    Failure {
        phase: Some(Phase::Test),
        exception: Some("KeyError".into()),
        message: format!("KeyError: 'user{id_number}'"),
        assertion: None,
        location: Some(Location {
            path: "src/app/users.py".into(),
            line: 41,
        }),
        frames: vec![frame("src/app/users.py", 41, "find", "return USERS[name]")],
        signature: "sig-key".into(),
        detail: None,
        ..Default::default()
    }
}

/// Failures of every shape, interleaved with passes.
pub(crate) fn mixed() -> Vec<Record> {
    let mut total = finished("tests/test_cart.py::test_total", Outcome::Failed, ms(5));
    total.failures = vec![assertion_failure()];
    total.output = Captured {
        stdout: "loading cart\nitem 1\nitem 2\nitem 3\ntotal computed\n".into(),
        stderr: String::new(),
        log: "WARNING cart: price missing\n".into(),
        truncated: false,
    };

    let mut load = finished("tests/test_db.py::test_load", Outcome::Error, ms(3));
    load.failures = vec![setup_failure()];
    load.reproduce = Some(Reproduce {
        command: vec!["aye".into(), "tests/test_db.py::test_load".into()],
        inputs: [("seed".to_owned(), "42".to_owned())].into(),
    });

    let mut both = finished("tests/test_io.py::test_write", Outcome::Error, ms(7));
    both.failures = vec![
        Failure {
            phase: Some(Phase::Test),
            exception: Some("AssertionError".into()),
            message: "assert False".into(),
            frames: vec![frame("tests/test_io.py", 20, "test_write", "assert False")],
            signature: "sig-false".into(),
            ..Failure::default()
        },
        Failure {
            phase: Some(Phase::Teardown),
            exception: Some("RuntimeError".into()),
            message: "RuntimeError: cleanup failed".into(),
            frames: vec![frame(
                "tests/test_io.py",
                14,
                "tmp",
                "raise RuntimeError('cleanup failed')",
            )],
            signature: "sig-cleanup".into(),
            ..Failure::default()
        },
    ];

    let mut first_user = finished("tests/test_users.py::test_find[1]", Outcome::Failed, ms(1));
    first_user.failures = vec![key_error(1)];
    let mut second_user = finished("tests/test_users.py::test_find[2]", Outcome::Failed, ms(1));
    second_user.failures = vec![key_error(2)];
    let mut third_user = finished("tests/test_users.py::test_find[3]", Outcome::Failed, ms(1));
    third_user.failures = vec![key_error(1)];

    log(vec![
        run_started(false),
        Event::CollectionStarted,
        case("tests/test_cart.py::test_total", Some(10), Certainty::Exact),
        case("tests/test_cart.py::test_ok", Some(20), Certainty::Exact),
        case("tests/test_db.py::test_load", Some(5), Certainty::Exact),
        case("tests/test_io.py::test_write", Some(18), Certainty::Exact),
        case(
            "tests/test_users.py::test_find",
            Some(3),
            Certainty::Partial,
        ),
        collected(5, 0),
        test(total),
        test(finished(
            "tests/test_cart.py::test_ok",
            Outcome::Passed,
            ms(1),
        )),
        test(load),
        test(both),
        test(first_user),
        test(second_user),
        test(third_user),
        run_finished(
            ExitCode::TestsFailed,
            Counts {
                passed: 1,
                failed: 4,
                error: 2,
                ..Counts::default()
            },
            Duration::from_millis(1234),
        ),
    ])
}

/// `n` tests that fail the same way.
pub(crate) fn identical(n: u32) -> Vec<Record> {
    let mut events = vec![
        run_started(false),
        case("tests/test_api.py::test_call", Some(4), Certainty::Partial),
        collected(n, 0),
    ];
    for i in 0..n {
        let mut result = finished(
            &format!("tests/test_api.py::test_call[{i}]"),
            Outcome::Failed,
            ms(2),
        );
        result.failures = vec![Failure {
            phase: Some(Phase::Test),
            exception: Some("TimeoutError".into()),
            message: "TimeoutError: timed out".into(),
            location: Some(Location {
                path: "tests/test_api.py".into(),
                line: 6,
            }),
            frames: vec![frame(
                "tests/test_api.py",
                6,
                "test_call",
                "client.get('/')",
            )],
            signature: "sig-timeout".into(),
            ..Failure::default()
        }];
        events.push(test(result));
    }
    events.push(run_finished(
        ExitCode::TestsFailed,
        Counts {
            failed: n,
            ..Counts::default()
        },
        ms(300),
    ));
    log(events)
}

/// A class whose setup fails, keeping its tests from running, and a module
/// whose teardown fails.
pub(crate) fn lifetime() -> Vec<Record> {
    let class = "class:tests/test_db.py::TestDb";
    let blocked = |name: &str| {
        let mut result = finished(
            &format!("tests/test_db.py::TestDb::{name}"),
            Outcome::Error,
            ms(0),
        );
        result.cause = Some(class.into());
        test(result)
    };
    log(vec![
        run_started(false),
        case(
            "tests/test_db.py::TestDb::test_a",
            Some(12),
            Certainty::Exact,
        ),
        case(
            "tests/test_db.py::TestDb::test_b",
            Some(15),
            Certainty::Exact,
        ),
        case(
            "tests/test_db.py::TestDb::test_c",
            Some(18),
            Certainty::Exact,
        ),
        case("tests/test_db.py::test_free", Some(22), Certainty::Exact),
        collected(4, 0),
        Event::LifetimeStarted {
            lifetime: "module:tests/test_db.py".into(),
            kind: "module".into(),
        },
        Event::LifetimeStarted {
            lifetime: class.into(),
            kind: "class".into(),
        },
        blocked("test_a"),
        blocked("test_b"),
        blocked("test_c"),
        Event::LifetimeFinished {
            lifetime: class.into(),
            failure: Some(Failure {
                phase: Some(Phase::Setup),
                exception: Some("RuntimeError".into()),
                message: "RuntimeError: no database".into(),
                location: Some(Location {
                    path: "tests/test_db.py".into(),
                    line: 9,
                }),
                frames: vec![frame(
                    "tests/test_db.py",
                    9,
                    "setUpClass",
                    "cls.conn = connect()",
                )],
                signature: "sig-nodb".into(),
                detail: Some(
                    "Traceback (most recent call last):\nRuntimeError: no database".into(),
                ),
                ..Failure::default()
            }),
            duration: ms(1),
        },
        test(finished(
            "tests/test_db.py::test_free",
            Outcome::Passed,
            ms(1),
        )),
        Event::LifetimeFinished {
            lifetime: "module:tests/test_db.py".into(),
            failure: Some(Failure {
                phase: Some(Phase::Teardown),
                exception: Some("OSError".into()),
                message: "OSError: busy".into(),
                frames: vec![frame(
                    "tests/test_db.py",
                    4,
                    "tearDownModule",
                    "shutil.rmtree(TMP)",
                )],
                signature: "sig-busy".into(),
                ..Failure::default()
            }),
            duration: ms(2),
        },
        run_finished(
            ExitCode::TestsFailed,
            Counts {
                passed: 1,
                error: 3,
                ..Counts::default()
            },
            ms(50),
        ),
    ])
}

fn diagnostic(severity: Severity, code: &str, path: &str, line: u32, message: &str) -> Event {
    Event::Diagnostic(Diagnostic {
        severity,
        code: code.into(),
        path: path.into(),
        span: Some(Span {
            path: aye_events::ProjectPath::as_written(path),
            start: 0,
            end: 0,
            line: Some(line),
        }),
        message: message.into(),
    })
}

/// Collection errors and warnings, and a worker that dies.
pub(crate) fn diagnostics() -> Vec<Record> {
    let mut records = log(vec![
        run_started(false),
        Event::CollectionStarted,
        diagnostic(
            Severity::Error,
            "syntax-error",
            "tests/test_bad.py",
            3,
            "Expected an expression",
        ),
        diagnostic(
            Severity::Warning,
            "duplicate-definition",
            "tests/test_dup.py",
            10,
            "test_x is defined twice; only the last one runs",
        ),
        diagnostic(
            Severity::Info,
            "refined",
            "tests/test_dup.py",
            1,
            "imported to find its tests",
        ),
        case("tests/test_dup.py::test_x", Some(10), Certainty::Exact),
        collected(1, 0),
        test(finished(
            "tests/test_dup.py::test_x",
            Outcome::Passed,
            ms(1),
        )),
        Event::WorkerFinished {
            exit_status: Some(-9),
            error: Some("Traceback (most recent call last):\nMemoryError\n".into()),
        },
        run_finished(
            ExitCode::Interrupted,
            Counts {
                passed: 1,
                ..Counts::default()
            },
            ms(90),
        ),
    ]);
    records[8].envelope.worker = Some(1);
    records
}

/// Collection only: nothing runs.
pub(crate) fn collect_only() -> Vec<Record> {
    log(vec![
        run_started(true),
        Event::CollectionStarted,
        case("tests/test_a.py::test_one", Some(1), Certainty::Exact),
        case("tests/test_a.py::test_param", Some(5), Certainty::Partial),
        case(
            "tests/test_b.py::TestB::test_two",
            Some(3),
            Certainty::Exact,
        ),
        collected(3, 1),
        run_finished(
            ExitCode::Ok,
            Counts {
                deselected: 1,
                ..Counts::default()
            },
            ms(6),
        ),
    ])
}

// ---------------------------------------------------------------- every field

/// What the engine sends for a failure, every field filled: the pointed
/// frame's span, the lines before it and its locals.
struct Raised<'a> {
    path: &'a str,
    line: u32,
    function: &'a str,
    source: &'a str,
    cols: Option<[u32; 2]>,
    before: &'a [&'a str],
    locals: &'a [(&'a str, &'a str)],
}

impl Raised<'_> {
    fn frame(&self) -> Frame {
        Frame {
            path: self.path.into(),
            line: self.line,
            function: Some(self.function.into()),
            source: Some(self.source.into()),
            cols: self.cols,
            before: self.before.iter().map(|line| (*line).to_owned()).collect(),
            locals: self
                .locals
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            ..Default::default()
        }
    }
}

fn raised(phase: Phase, exception: &str, message: &str, at: &Raised<'_>) -> Failure {
    Failure {
        phase: Some(phase),
        exception: Some(exception.into()),
        message: message.into(),
        location: Some(Location {
            path: at.path.into(),
            line: at.line,
        }),
        frames: vec![at.frame()],
        signature: format!("sig-{}-{}", at.path, at.line),
        ..Failure::default()
    }
}

/// A comparison's assertion, as the engine sends it: the operands, which
/// is expected, their sources and how they differ.
fn compared(
    (left, right): (&str, &str),
    expected: Option<aye_events::Side>,
    (left_source, right_source): (&str, &str),
    compare: aye_events::Compare,
) -> Assertion {
    Assertion {
        op: "==".into(),
        left: left.into(),
        right: right.into(),
        diff: vec!["(pytest's explanation, which compare replaces)".into()],
        expected,
        left_source: Some(left_source.into()),
        right_source: Some(right_source.into()),
        compare: Some(compare),
    }
}

fn difference(at: Option<&str>, left: Option<&str>, right: Option<&str>) -> aye_events::Difference {
    aye_events::Difference {
        at: at.map(str::to_owned),
        left: left.map(str::to_owned),
        right: right.map(str::to_owned),
    }
}

fn resources(chain: &[(&str, &str)]) -> Vec<aye_events::Resource> {
    chain
        .iter()
        .map(|(name, scope)| aye_events::Resource {
            name: (*name).into(),
            scope: (*scope).into(),
        })
        .collect()
}

fn bad(id: &str, outcome: Outcome, duration: u64, failures: Vec<Failure>) -> TestResult {
    let mut result = finished(id, outcome, ms(duration));
    result.failures = failures;
    result.reproduce = Some(Reproduce {
        command: vec!["aye".into(), id.into()],
        inputs: std::collections::BTreeMap::new(),
    });
    result
}

fn summary_dict() -> TestResult {
    use aye_events::{Compare, Previous, Side};
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert {'apple': 4, 'pear': 2} == {'apple': 4, 'pear': 3}",
        &Raised {
            path: "tests/test_cart.py",
            line: 34,
            function: "test_fails_summary_dict",
            source: "assert cart.summary() == {\"apple\": 4, \"pear\": 3}",
            cols: Some([7, 21]),
            before: &[
                "cart.add(\"apple\", price_list[\"apple\"], 4)",
                "cart.add(\"pear\", price_list[\"pear\"], 2)",
            ],
            locals: &[
                ("cart", "Cart(items={'apple': 4, 'pear': 2})"),
                (
                    "price_list",
                    "{'apple': Decimal('0.50'), 'pear': Decimal('0.75')}",
                ),
            ],
        },
    );
    failure.assertion = Some(compared(
        ("{'apple': 4, 'pear': 2}", "{'apple': 4, 'pear': 3}"),
        Some(Side::Right),
        ("cart.summary()", "{\"apple\": 4, \"pear\": 3}"),
        Compare {
            kind: "dict".into(),
            summary: "dict differs in 1 of 2 keys".into(),
            same: 1,
            differs: vec![difference(Some("['pear']"), Some("2"), Some("3"))],
            omitted: 0,
        },
    ));
    let mut result = bad(
        "tests/test_cart.py::TestCart::test_fails_summary_dict",
        Outcome::Failed,
        2,
        vec![failure],
    );
    result.resources = resources(&[("price_list", "session"), ("cart", "function")]);
    result.history = Some(Previous {
        outcomes: "pppppppppppp".into(),
        since: None,
    });
    result.output.stdout = "adding apple\nadding pear\n".into();
    result
}

fn patch_binding() -> TestResult {
    use aye_events::{Compare, Previous, Side};
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert datetime(2026, 10, 15, 18, 40) == datetime(2026, 10, 11, 9, 30)",
        &Raised {
            path: "tests/test_checkout.py",
            line: 64,
            function: "test_fails_patch_wrong_binding",
            source: "assert receipt.issued_at == FROZEN_NOW",
            cols: Some([7, 24]),
            before: &[
                "with patch(\"shop.clock.now\", return_value=FROZEN_NOW):",
                "    receipt = checkout(cart)",
            ],
            locals: &[],
        },
    );
    failure.assertion = Some(compared(
        (
            "datetime(2026, 10, 15, 18, 40)",
            "datetime(2026, 10, 11, 9, 30)",
        ),
        Some(Side::Right),
        ("receipt.issued_at", "FROZEN_NOW"),
        Compare {
            kind: "datetime".into(),
            summary: "datetime differs by 4 days, 9:10:00".into(),
            same: 0,
            differs: vec![difference(
                None,
                Some("2026-10-15T18:40:00"),
                Some("2026-10-11T09:30:00"),
            )],
            omitted: 0,
        },
    ));
    let mut result = bad(
        "tests/test_checkout.py::TestCheckout::test_fails_patch_wrong_binding",
        Outcome::Failed,
        3,
        vec![failure],
    );
    result.history = Some(Previous {
        outcomes: "pppppppfff".into(),
        since: Some(78),
    });
    result
}

fn in_setup() -> TestResult {
    let mut failure = raised(
        Phase::Setup,
        "ConnectionError",
        "ConnectionError: database unreachable",
        &Raised {
            path: "tests/conftest.py",
            line: 12,
            function: "unreachable_database",
            source: "raise ConnectionError(\"database unreachable\")",
            cols: Some([0, 45]),
            before: &["@pytest.fixture", "def unreachable_database():"],
            locals: &[],
        },
    );
    // The fixture's body, one level in from its decorator and `def`.
    failure.frames[0].indent = 4;
    failure.resource = Some("unreachable_database".into());
    let mut result = bad(
        "tests/test_setup_errors.py::test_errors_in_setup",
        Outcome::Error,
        1,
        vec![failure],
    );
    result.resources = resources(&[("unreachable_database", "function")]);
    result.history = Some(aye_events::Previous {
        outcomes: "ppfppppfpp".into(),
        since: None,
    });
    result
}

fn in_teardown() -> TestResult {
    let mut failure = raised(
        Phase::Teardown,
        "RuntimeError",
        "RuntimeError: teardown failed",
        &Raised {
            path: "tests/conftest.py",
            line: 20,
            function: "fragile_teardown",
            source: "raise RuntimeError(\"teardown failed\")",
            cols: Some([0, 37]),
            before: &["yield"],
            locals: &[],
        },
    );
    failure.resource = Some("fragile_teardown".into());
    let mut result = bad(
        "tests/test_setup_errors.py::test_errors_in_teardown",
        Outcome::Error,
        1,
        vec![failure],
    );
    result.resources = resources(&[("fragile_teardown", "function")]);
    result
}

fn missing_fixture() -> TestResult {
    let mut failure = raised(
        Phase::Setup,
        "FixtureLookupError",
        "fixture 'kart' not found",
        &Raised {
            path: "tests/test_setup_errors.py",
            line: 30,
            function: "test_errors_missing_fixture",
            source: "def test_errors_missing_fixture(kart):",
            cols: None,
            before: &[],
            locals: &[],
        },
    );
    failure.hint = Some("did you mean 'cart'?".into());
    bad(
        "tests/test_setup_errors.py::test_errors_missing_fixture",
        Outcome::Error,
        0,
        vec![failure],
    )
}

fn restock_levels() -> TestResult {
    use aye_events::{Compare, Side};
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert [1, 2, 3, 4, 5] == [1, 3, 3, 5, 5, 6]",
        &Raised {
            path: "tests/test_inventory.py",
            line: 41,
            function: "test_restock_levels",
            source: "assert inventory.levels() == [1, 3, 3, 5, 5, 6]",
            cols: Some([7, 25]),
            before: &["inventory.restock(\"pear\", 1)"],
            locals: &[],
        },
    );
    failure.assertion = Some(compared(
        ("[1, 2, 3, 4, 5]", "[1, 3, 3, 5, 5, 6]"),
        Some(Side::Right),
        ("inventory.levels()", "[1, 3, 3, 5, 5, 6]"),
        Compare {
            kind: "list".into(),
            summary: "list differs at 2 of 5 positions; 1 item missing".into(),
            same: 3,
            differs: vec![
                difference(Some("[1]"), Some("2"), Some("3")),
                difference(Some("[3]"), Some("4"), Some("5")),
                difference(Some("[5]"), None, Some("6")),
            ],
            omitted: 0,
        },
    ));
    bad(
        "tests/test_inventory.py::test_restock_levels",
        Outcome::Failed,
        1,
        vec![failure],
    )
}

fn receipt_text() -> TestResult {
    use aye_events::{Compare, Side};
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert 'Coffee 9.99\\nTotal: 9.44' == 'Coffee 9.99\\nTotal: 9.45'",
        &Raised {
            path: "tests/test_receipt.py",
            line: 18,
            function: "test_receipt_text",
            source: "assert receipt.text() == EXPECTED_RECEIPT",
            cols: Some([7, 21]),
            before: &[],
            locals: &[],
        },
    );
    failure.assertion = Some(compared(
        ("'Coffee 9.99\\nTotal: 9.44'", "'Coffee 9.99\\nTotal: 9.45'"),
        Some(Side::Right),
        ("receipt.text()", "EXPECTED_RECEIPT"),
        Compare {
            kind: "str".into(),
            summary: "strings differ at line 2, column 11".into(),
            same: 0,
            differs: vec![difference(
                Some("line 2, column 11"),
                Some("'Total: 9.44'"),
                Some("'Total: 9.45'"),
            )],
            omitted: 0,
        },
    ));
    bad(
        "tests/test_receipt.py::test_receipt_text",
        Outcome::Failed,
        1,
        vec![failure],
    )
}

fn totals_agree() -> TestResult {
    use aye_events::Compare;
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert Decimal('9.44') == Decimal('9.45')",
        &Raised {
            path: "tests/test_cart.py",
            line: 52,
            function: "test_totals_agree",
            source: "assert cart.total() == invoice.total()",
            cols: Some([7, 38]),
            before: &[],
            locals: &[],
        },
    );
    failure.assertion = Some(compared(
        ("Decimal('9.44')", "Decimal('9.45')"),
        None,
        ("cart.total()", "invoice.total()"),
        Compare {
            kind: "number".into(),
            summary: "differ by 0.01".into(),
            ..Compare::default()
        },
    ));
    bad(
        "tests/test_cart.py::test_totals_agree",
        Outcome::Failed,
        1,
        vec![failure],
    )
}

fn line_item() -> TestResult {
    use aye_events::{Compare, Side};
    let mut failure = raised(
        Phase::Test,
        "AssertionError",
        "assert LineItem(sku='pear', price=Decimal('1.50'), quantity=2) == LineItem(sku='pear', price=Decimal('1.50'), quantity=3)",
        &Raised {
            path: "tests/test_cart.py",
            line: 61,
            function: "test_line_item",
            source: "assert cart.lines[0] == LineItem(\"pear\", Decimal(\"1.50\"), 3)",
            cols: Some([7, 20]),
            before: &[],
            locals: &[],
        },
    );
    failure.assertion = Some(compared(
        (
            "LineItem(sku='pear', price=Decimal('1.50'), quantity=2)",
            "LineItem(sku='pear', price=Decimal('1.50'), quantity=3)",
        ),
        Some(Side::Right),
        ("cart.lines[0]", "LineItem(\"pear\", Decimal(\"1.50\"), 3)"),
        Compare {
            kind: "dataclass".into(),
            summary: "LineItem differs in 1 of 3 fields".into(),
            same: 2,
            differs: vec![difference(Some(".quantity"), Some("2"), Some("3"))],
            omitted: 0,
        },
    ));
    bad(
        "tests/test_cart.py::test_line_item",
        Outcome::Failed,
        1,
        vec![failure],
    )
}

/// An exception from a library, under a project frame that called it.
fn library(id: &str) -> TestResult {
    let project = Raised {
        path: "src/shop/client.py",
        line: 20,
        function: "get",
        source: "return requests.get(self.base + path, timeout=2)",
        cols: Some([7, 48]),
        before: &[
            "def get(self, path):",
            "    \"\"\"Fetch one page of the API.\"\"\"",
        ],
        locals: &[
            ("path", "'/items'"),
            ("self", "<Client base='http://shop.test'>"),
        ],
    };
    let library_frame = |path: &str, line: u32, function: &str| Frame {
        path: path.into(),
        line,
        function: Some(function.into()),
        source: Some("…".into()),
        ..Frame::default()
    };
    let site = "/home/me/.venv/lib/python3.13/site-packages";
    let mut failure = raised(
        Phase::Test,
        "ConnectionError",
        "requests.exceptions.ConnectionError: HTTPConnectionPool(host='shop.test', port=80): Max retries exceeded",
        &project,
    );
    let mut caller = Raised {
        path: "tests/test_api.py",
        line: 8,
        function: "test_fetch",
        source: "items = client.get(\"/items\")",
        cols: None,
        before: &[],
        locals: &[],
    }
    .frame();
    caller.function = Some(id.rsplit("::").next().unwrap_or(id).into());
    failure.frames = vec![
        caller,
        Frame {
            indent: 4,
            ..project.frame()
        },
        library_frame(&format!("{site}/requests/api.py"), 73, "get"),
        library_frame(&format!("{site}/requests/api.py"), 59, "request"),
        library_frame(&format!("{site}/requests/sessions.py"), 589, "request"),
        library_frame(&format!("{site}/requests/sessions.py"), 703, "send"),
        library_frame(&format!("{site}/requests/adapters.py"), 700, "send"),
    ];
    failure.location = Some(Location {
        path: format!("{site}/requests/adapters.py").into(),
        line: 700,
    });
    failure.signature = "sig-requests".into();
    let mut result = bad(id, Outcome::Error, 2004, vec![failure]);
    result.output.log =
        "WARNING urllib3: Retrying (Retry(total=0)) after connection broken\n".into();
    result
}

/// The mockup's project, every field the engine fills filled: a dict
/// compared with a literal, a date with a named constant, a broken
/// fixture, a failed teardown, an unknown fixture (with history and the
/// run's number); then more shapes beside them: a list, a string, a
/// dataclass, two values neither of which is literal, an exception from a
/// library, and a second test failing the same way; five passing tests,
/// three skipped for two reasons, and a warning.
pub(crate) fn full() -> Vec<Record> {
    let mut skipped = Vec::new();
    for (id, reason) in [
        ("tests/test_ml.py::test_train[gpu]", "needs GPU"),
        ("tests/test_ml.py::test_infer[gpu]", "needs GPU"),
        ("tests/test_ml.py::test_mmap", "Linux only"),
    ] {
        let mut result = finished(id, Outcome::Skipped, ms(0));
        result.reason = Some(reason.into());
        skipped.push(result);
    }
    let passed = |id: &str| finished(id, Outcome::Passed, ms(1));
    let results = [
        passed("tests/test_cart.py::TestCart::test_add"),
        summary_dict(),
        passed("tests/test_cart.py::TestCart::test_remove"),
        passed("tests/test_cart.py::TestCart::test_empty"),
        patch_binding(),
        passed("tests/test_checkout.py::TestCheckout::test_pays"),
        in_setup(),
        in_teardown(),
        missing_fixture(),
        passed("tests/test_inventory.py::test_take"),
        restock_levels(),
        receipt_text(),
        totals_agree(),
        line_item(),
        library("tests/test_api.py::test_fetch"),
        library("tests/test_api.py::test_fetch_again"),
    ]
    .into_iter()
    .chain(skipped)
    .collect::<Vec<_>>();
    let mut events = vec![
        Event::RunStarted {
            schema_version: SCHEMA_VERSION,
            aye_version: "0.0.1".into(),
            root: "/home/me/tests_pytest".into(),
            collect_only: false,
            workers: 1,
            run: Some(81),
        },
        Event::CollectionStarted,
        diagnostic(
            Severity::Warning,
            "plugin-not-loaded",
            ".",
            1,
            "pytest would load the plugin 'platformdirs'; aye loads no third-party plugin yet",
        ),
    ];
    for result in &results {
        events.push(case(result.id.as_str(), Some(1), Certainty::Exact));
    }
    events.push(collected(u32::try_from(results.len()).unwrap(), 0));
    events.push(Event::WorkerStarted {
        pid: 4242,
        python: "/home/me/.venv/bin/python".into(),
        version: Some("3.13.16 (main, Oct  1 2026, 09:00:00) [Clang 17.0.0]".into()),
    });
    let mut counts = Counts::default();
    for result in results {
        counts.record(result.outcome);
        events.push(test(result));
    }
    events.push(Event::WorkerFinished {
        exit_status: Some(0),
        error: None,
    });
    events.push(run_finished(ExitCode::TestsFailed, counts, ms(2120)));
    let mut records = log(events);
    for record in &mut records {
        if matches!(
            record.event,
            Event::WorkerStarted { .. } | Event::TestFinished(_) | Event::WorkerFinished { .. }
        ) {
            record.envelope.worker = Some(0);
        }
    }
    records
}
