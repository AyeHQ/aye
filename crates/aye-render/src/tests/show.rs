//! Keys, and what `aye show` asks of the renderers: a stored log's list,
//! part of it with the whole log's keys, what the run store knows beside
//! it, and stale files.

use std::collections::BTreeMap;
use std::time::Duration;

use aye_events::{
    Counts, DiagnosticCode, Event, ExitCode, Outcome, ProjectPath, Record, TestId, TestKey,
};

use super::{feed, lines, streams};
use crate::{
    ColorChoice, Extra, JsonLines, Keys, ShowContext, ShowMode, Tally, Text, TextOptions,
    summary_words,
};

fn text_for(records: &[Record], verbosity: i8, show: Option<(ShowMode, Keys)>) -> String {
    text_with(records, verbosity, show, ShowContext::default())
}

fn text_with(
    records: &[Record],
    verbosity: i8,
    show: Option<(ShowMode, Keys)>,
    context: ShowContext,
) -> String {
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity,
            color: ColorChoice::Never,
            live: false,
        },
    );
    if let Some((mode, keys)) = show {
        text = text.for_show(mode, keys, context);
    }
    feed(&mut text, records);
    String::from_utf8(text.into_inner()).unwrap()
}

fn json_for(records: &[Record], verbosity: i8, show: Option<(ShowMode, Keys)>) -> String {
    json_with(records, verbosity, show, ShowContext::default())
}

fn json_with(
    records: &[Record],
    verbosity: i8,
    show: Option<(ShowMode, Keys)>,
    context: ShowContext,
) -> String {
    let mut json = JsonLines::new(Vec::new(), verbosity);
    if let Some((mode, keys)) = show {
        json = json.for_show(mode, keys, context);
    }
    feed(&mut json, records);
    String::from_utf8(json.into_inner()).unwrap()
}

/// The records of `ids`' tests, between the log's `run_started` and a
/// `run_finished` counting them, as `aye show` feeds them; with their
/// worker's `worker_started` when `workers`.
fn part(records: &[Record], ids: &[&str]) -> Vec<Record> {
    let mut counts = Counts::default();
    let mut part: Vec<Record> = records
        .iter()
        .filter(|record| match &record.event {
            Event::RunStarted { .. } | Event::WorkerStarted { .. } => true,
            Event::TestStarted { id } | Event::SubOutcome { id, .. } => ids.contains(&id.as_str()),
            Event::TestFinished(result) if ids.contains(&result.id.as_str()) => {
                counts.record(result.outcome);
                true
            }
            _ => false,
        })
        .cloned()
        .collect();
    part.push(Record::new(Event::RunFinished {
        exit_code: ExitCode::TestsFailed,
        counts,
        duration: Duration::ZERO,
        stopped: None,
    }));
    part
}

/// The key a run gives `id`.
fn key(records: &[Record], id: &str) -> String {
    Keys::of(records).key(id).unwrap().to_owned()
}

/// Two test ids whose keys share their first five digits.
fn alike() -> (String, String) {
    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    (0..)
        .find_map(|n| {
            let id = format!("tests/t.py::test_{n}");
            let start = TestKey::of(&id).to_string()[..5].to_string();
            seen.insert(start, id.clone()).map(|earlier| (earlier, id))
        })
        .unwrap()
}

fn failed(id: &str, message: &str) -> Record {
    let mut result = aye_events::TestResult::new(TestId::new(id), Outcome::Failed);
    result.failures = vec![aye_events::Failure {
        message: message.into(),
        ..aye_events::Failure::default()
    }];
    Record::new(Event::TestFinished(Box::new(result)))
}

#[test]
fn part_of_a_log_keeps_the_whole_logs_keys() {
    // The second of two ids that start alike has a longer key in the
    // whole log; fed alone, it keeps it.
    let (first, second) = alike();
    let records = [
        failed(&first, "boom"),
        failed(&second, "bang"),
        Record::new(Event::RunFinished {
            exit_code: ExitCode::TestsFailed,
            counts: Counts {
                failed: 2,
                ..Counts::default()
            },
            duration: Duration::ZERO,
            stopped: None,
        }),
    ];
    let long = key(&records, &second);
    assert!(long.len() > 5, "{long}");
    let part = part(&records, &[second.as_str()]);
    let out = text_for(&part, 0, Some((ShowMode::Details, Keys::of(&records))));
    assert_eq!(
        out,
        lines(&[
            &format!("✗ {long}  {second}"),
            &format!("{}bang", " ".repeat(long.len() + 4)),
        ])
    );
    let json = json_for(&part, 1, Some((ShowMode::Details, Keys::of(&records))));
    assert!(
        json.starts_with(&format!(
            r#"{{"type":"fail","key":"{long}","id":"{second}","outcome":"failed","why":"bang","stale":false}}"#
        )),
        "{json}"
    );
    // Counting afresh, it would have five digits.
    assert!(text_for(&part, 0, None).starts_with(&format!("✗ {}  ", &long[..5])));
}

#[test]
fn details_give_a_test_that_didnt_fail_its_line() {
    let records = streams::mixed();
    let ids = [
        "tests/test_cart.py::test_total",
        "tests/test_cart.py::test_ok",
    ];
    let part = part(&records, &ids);
    let keys = || Some((ShowMode::Details, Keys::of(&records)));
    let total = key(&records, ids[0]);
    assert_eq!(
        text_for(&part, 0, keys()),
        lines(&[
            &format!("✗ {total}  tests/test_cart.py::test_total"),
            "         assert 'spam' == 'eggs' at tests/test_cart.py:12",
            "✓ passed  tests/test_cart.py::test_ok  1 ms",
        ])
    );
    // A run's default shows none.
    assert!(!text_for(&records, 0, None).contains("✓ passed  "));
    // -q: the summary alone, as a run's.
    assert_eq!(
        text_for(&part, -1, keys()),
        "✗ 1 failed · ✓ 1 passed   0.00 s\n"
    );
}

#[test]
fn details_say_where_the_run_was_and_whether_the_file_changed() {
    let records = streams::full();
    let id = "tests/test_cart.py::TestCart::test_fails_summary_dict";
    let part = part(&records, &[id]);
    let k = key(&records, id);
    let context = || ShowContext {
        run: Some(81),
        ago: Some("2 min ago".into()),
        stale: BTreeMap::new(),
    };
    let out = text_with(
        &part,
        1,
        Some((ShowMode::Details, Keys::of(&records))),
        context(),
    );
    assert!(
        out.starts_with(&lines(&[
            &format!("✗ {k}  {id}"),
            "  failed in the test · 2 ms · python 3.13.16 · run r81, 2 min ago · file unchanged",
        ])),
        "{out}"
    );
    let mut changed = context();
    changed
        .stale
        .insert(ProjectPath::new("tests/test_cart.py").unwrap(), false);
    let out = text_with(
        &part,
        1,
        Some((ShowMode::Details, Keys::of(&records))),
        changed.clone(),
    );
    assert!(out.contains(" · file changed since\n"), "{out}");
    changed
        .stale
        .insert(ProjectPath::new("tests/test_cart.py").unwrap(), true);
    let out = text_with(
        &part,
        1,
        Some((ShowMode::Details, Keys::of(&records))),
        changed.clone(),
    );
    assert!(out.contains(" · file gone\n"), "{out}");
    // JSON: the run, the Python, whether the file changed.
    let json = json_with(
        &part,
        1,
        Some((ShowMode::Details, Keys::of(&records))),
        changed,
    );
    assert!(
        json.contains(r#","run":"r81","python":"3.13.16","stale":true}"#),
        "{json}"
    );
    // A run's line has none of them.
    let json = json_for(&records, 1, None);
    assert!(!json.contains(r#""stale""#), "{json}");
}

#[test]
fn the_list_is_a_line_per_test_and_the_summary() {
    let records = streams::mixed();
    let list: Vec<Record> = records
        .iter()
        .filter(|r| {
            matches!(
                r.event,
                Event::RunStarted { .. } | Event::TestFinished(_) | Event::RunFinished { .. }
            )
        })
        .cloned()
        .collect();
    let keys = || Some((ShowMode::List, Keys::of(&records)));
    let k = |id: &str| key(&records, id);
    assert_eq!(
        text_for(&list, 0, keys()),
        lines(&[
            &format!(
                "✗ failed  {}  tests/test_cart.py::test_total  5 ms — assert 'spam' == 'eggs'",
                k("tests/test_cart.py::test_total")
            ),
            "✓ passed  tests/test_cart.py::test_ok  1 ms",
            &format!(
                "! error   {}  tests/test_db.py::test_load  3 ms — ConnectionError: refused",
                k("tests/test_db.py::test_load")
            ),
            &format!(
                "! error   {}  tests/test_io.py::test_write  7 ms — assert False",
                k("tests/test_io.py::test_write")
            ),
            &format!(
                "✗ failed  {}  tests/test_users.py::test_find[1]  1 ms — KeyError: 'user1'",
                k("tests/test_users.py::test_find[1]")
            ),
            &format!(
                "✗ failed  {}  tests/test_users.py::test_find[2]  1 ms — KeyError: 'user2'",
                k("tests/test_users.py::test_find[2]")
            ),
            &format!(
                "✗ failed  {}  tests/test_users.py::test_find[3]  1 ms — KeyError: 'user1'",
                k("tests/test_users.py::test_find[3]")
            ),
            "",
            "✗ 4 failed · ! 2 errors · ✓ 1 passed   1.23 s",
        ])
    );
    // -q: the summary alone.
    assert_eq!(
        text_for(&list, -1, keys()),
        "✗ 4 failed · ! 2 errors · ✓ 1 passed   1.23 s\n"
    );
    // JSON: a `test` line each, with its key, from the default level.
    let json = json_for(&list, 0, keys());
    let json: Vec<&str> = json.lines().collect();
    assert_eq!(json.len(), 8);
    assert_eq!(
        json[0],
        format!(
            r#"{{"type":"test","key":"{}","id":"tests/test_cart.py::test_total","outcome":"failed","ms":5,"why":"assert 'spam' == 'eggs'"}}"#,
            k("tests/test_cart.py::test_total")
        )
    );
    assert_eq!(
        json[1],
        format!(
            r#"{{"type":"test","key":"{}","id":"tests/test_cart.py::test_ok","outcome":"passed","ms":1}}"#,
            k("tests/test_cart.py::test_ok")
        )
    );
    let failures: Vec<String> = [
        "tests/test_cart.py::test_total",
        "tests/test_db.py::test_load",
        "tests/test_io.py::test_write",
        "tests/test_users.py::test_find[1]",
        "tests/test_users.py::test_find[2]",
        "tests/test_users.py::test_find[3]",
    ]
    .iter()
    .map(|id| format!("\"{}\"", k(id)))
    .collect();
    assert_eq!(
        json[7],
        format!(
            r#"{{"type":"summary","exit":1,"failed":4,"errors":2,"passed":1,"s":1.23,"failures":[{}],"more":"aye show <key> --output json"}}"#,
            failures.join(",")
        )
    );
    assert_eq!(json_for(&list, -1, keys()).lines().count(), 1);
}

#[test]
fn a_file_changed_since_the_run_is_said_first() {
    let records = [Record::new(Event::RunFinished {
        exit_code: ExitCode::Ok,
        counts: Counts {
            passed: 1,
            ..Counts::default()
        },
        duration: Duration::ZERO,
        stopped: None,
    })];
    let mut text = Text::new(
        Vec::new(),
        TextOptions {
            verbosity: -1,
            color: ColorChoice::Never,
            live: false,
        },
    );
    text.stale("tests/test a.py", false).unwrap();
    text.stale("tests/test_b.py", true).unwrap();
    feed(&mut text, &records);
    assert_eq!(
        String::from_utf8(text.into_inner()).unwrap(),
        lines(&[
            "tests/test a.py changed since this run; rerun: aye 'tests/test a.py'",
            "tests/test_b.py is gone since this run",
            "",
            "✓ 1 passed   0.00 s",
        ])
    );
    let mut json = JsonLines::new(Vec::new(), 0);
    json.stale("tests/test_a.py", false).unwrap();
    json.stale("tests/test_b.py", true).unwrap();
    feed(&mut json, &records);
    assert_eq!(
        String::from_utf8(json.into_inner()).unwrap(),
        lines(&[
            r#"{"type":"stale","path":"tests/test_a.py","rerun":"aye tests/test_a.py"}"#,
            r#"{"type":"stale","path":"tests/test_b.py","gone":true}"#,
            r#"{"type":"summary","exit":0,"passed":1,"s":0.0}"#,
        ])
    );
    // JSON's -q is the summary alone.
    let mut json = JsonLines::new(Vec::new(), -1);
    json.stale("tests/test_a.py", false).unwrap();
    assert_eq!(json.into_inner(), [] as [u8; 0]);
}

/// What `aye show`'s list is fed of the lifetime stream: every result, and
/// the failed lifetime that kept no test from running.
fn listed(records: &[Record]) -> Vec<Record> {
    records
        .iter()
        .filter(|record| match &record.event {
            Event::LifetimeFinished { lifetime, .. } => lifetime.starts_with("module:"),
            Event::RunStarted { .. } | Event::TestFinished(_) | Event::RunFinished { .. } => true,
            _ => false,
        })
        .cloned()
        .collect()
}

#[test]
fn the_list_has_a_line_for_a_failed_lifetime_that_blocked_no_test() {
    let records = streams::lifetime();
    let list = listed(&records);
    let keys = || Some((ShowMode::List, Keys::of(&records)));
    let k = |id: &str| key(&records, id);
    let blocked = "setup of class:tests/test_db.py::TestDb failed";
    assert_eq!(
        text_for(&list, 0, keys()),
        lines(&[
            &format!(
                "! error   {}  tests/test_db.py::TestDb::test_a  0 ms — {blocked}",
                k("tests/test_db.py::TestDb::test_a")
            ),
            &format!(
                "! error   {}  tests/test_db.py::TestDb::test_b  0 ms — {blocked}",
                k("tests/test_db.py::TestDb::test_b")
            ),
            &format!(
                "! error   {}  tests/test_db.py::TestDb::test_c  0 ms — {blocked}",
                k("tests/test_db.py::TestDb::test_c")
            ),
            "✓ passed  tests/test_db.py::test_free  1 ms",
            &format!(
                "! error   {}  teardown of module:tests/test_db.py  2 ms — OSError: busy",
                k("module:tests/test_db.py")
            ),
            "",
            "! 3 errors · ✓ 1 passed · ! 1 teardown error   0.05 s",
        ])
    );
    assert_eq!(
        text_for(&list, -1, keys()),
        "! 3 errors · ✓ 1 passed · ! 1 teardown error   0.05 s\n"
    );
    // JSON: its line at L1, whatever the level.
    for verbosity in [0, 1, 2] {
        let json = json_for(&list, verbosity, keys());
        let json: Vec<&str> = json.lines().collect();
        assert_eq!(
            json[4],
            format!(
                r#"{{"type":"lifetime_fail","key":"{}","lifetime":"module:tests/test_db.py","phase":"teardown","at":"tests/test_db.py:4","why":"OSError: busy"}}"#,
                k("module:tests/test_db.py")
            ),
            "{verbosity}"
        );
        assert!(
            json[5].starts_with(
                r#"{"type":"summary","exit":1,"errors":3,"passed":1,"teardown_error":1,"s":0.05,"failures":["#
            ),
            "{}",
            json[5]
        );
    }
    // A class split across batches tears down in each: one line.
    let mut twice = list.clone();
    twice.insert(5, twice[5].clone());
    assert_eq!(
        text_for(&twice, 0, keys())
            .matches("teardown of module:")
            .count(),
        1
    );
    assert_eq!(
        json_for(&twice, 0, keys()).matches("lifetime_fail").count(),
        1
    );
}

#[test]
fn the_list_shows_errors_and_failed_workers_and_no_other_diagnostic() {
    let records = streams::diagnostics();
    let keys = || Some((ShowMode::List, Keys::of(&records)));
    assert_eq!(
        text_for(&records, 0, keys()),
        lines(&[
            "error[syntax-error] tests/test_bad.py:3: Expected an expression",
            "✓ passed  tests/test_dup.py::test_x  1 ms",
            "",
            "worker 1 failed (exit status -9)",
            "  Traceback (most recent call last):",
            "  MemoryError",
            "",
            "interrupted · ✓ 1 passed · ! 1 collection error   0.09 s",
        ])
    );
    let json = json_for(&records, 1, keys());
    let types: Vec<String> = json
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"].to_string())
        .collect();
    assert_eq!(
        types,
        [
            r#""collection_error""#,
            r#""test""#,
            r#""worker_error""#,
            r#""summary""#
        ]
    );
}

/// A module skipped while collecting: a `skipped` info diagnostic.
fn module_skip() -> Record {
    Record::new(Event::Diagnostic(aye_events::Diagnostic {
        severity: aye_events::Severity::Info,
        code: DiagnosticCode::Skipped,
        path: "tests/test_win.py".into(),
        span: None,
        message: "only on Windows".into(),
    }))
}

#[test]
fn a_module_skipped_while_collecting_is_a_skipped_line() {
    let skipped = Counts {
        passed: 1,
        ..Counts::default()
    };
    let records = vec![
        module_skip(),
        Record::new(Event::TestFinished(Box::new(aye_events::TestResult::new(
            TestId::new("tests/t.py::test_ok"),
            Outcome::Passed,
        )))),
        Record::new(Event::RunFinished {
            exit_code: ExitCode::Ok,
            counts: skipped,
            duration: Duration::ZERO,
            stopped: None,
        }),
    ];
    for mode in [ShowMode::List, ShowMode::Details] {
        let show = || Some((mode, Keys::of(&records)));
        // Details of tests are the tests alone; the list ends on the summary.
        let summary: &[&str] = match mode {
            ShowMode::List => &["", "✓ 1 passed · 1 skipped   0.00 s"],
            ShowMode::Details => &[],
        };
        assert_eq!(
            text_for(&records, 0, show()),
            lines(
                &[
                    &[
                        "  skipped tests/test_win.py — only on Windows",
                        "✓ passed  tests/t.py::test_ok  0 ms",
                    ][..],
                    summary
                ]
                .concat()
            ),
            "{mode:?}"
        );
        // -vv: not an info line too.
        assert!(
            text_for(&records, 2, show())
                .starts_with("  skipped tests/test_win.py — only on Windows\n"),
            "{mode:?}"
        );
        assert_eq!(
            text_for(&records, -1, show()),
            "✓ 1 passed · 1 skipped   0.00 s\n"
        );
    }
    // JSON's list: a `test` line named after the file.
    let json = json_for(&records, 0, Some((ShowMode::List, Keys::of(&records))));
    assert_eq!(
        json.lines().next().unwrap(),
        r#"{"type":"test","id":"tests/test_win.py","outcome":"skipped","ms":0,"reason":"only on Windows"}"#
    );
    assert!(
        json.ends_with("{\"type\":\"summary\",\"exit\":0,\"passed\":1,\"skipped\":1,\"s\":0.0}\n")
    );
    // A run's text counts it with the skips, and shows it as an info from
    // -vv.
    assert_eq!(
        text_for(&records, 0, None),
        lines(&[
            "skipped  only on Windows (1)",
            "✓ 1 passed · 1 skipped   0.00 s",
        ])
    );
    assert!(
        text_for(&records, 2, None)
            .starts_with("info[skipped] tests/test_win.py: only on Windows\n")
    );
}

#[test]
fn a_show_that_shows_nothing_says_so() {
    let records = [Record::new(Event::RunFinished {
        exit_code: ExitCode::TestsFailed,
        counts: Counts::default(),
        duration: Duration::ZERO,
        stopped: None,
    })];
    for mode in [ShowMode::List, ShowMode::Details] {
        assert_eq!(
            text_for(&records, 0, Some((mode, Keys::new()))),
            "no tests shown   0.00 s\n"
        );
    }
    assert_eq!(text_for(&records, 0, None), "no tests ran   0.00 s\n");
}

#[test]
fn a_tally_counts_what_the_summaries_count_beside_the_tests() {
    let mut tally = Tally::default();
    let mut records = streams::lifetime();
    records.extend(streams::diagnostics());
    records.push(module_skip());
    records.push(module_skip());
    for record in &records {
        tally.note(record);
    }
    let extra = tally.extra();
    assert_eq!(
        extra,
        Extra {
            skipped_modules: 1,
            teardown_error: 1,
            collection_error: 1,
            ..Extra::default()
        }
    );
    let named = extra.named();
    assert_eq!(
        named.keys().collect::<Vec<_>>(),
        ["collection_error", "skipped_modules", "teardown_error"]
    );
    assert_eq!(Extra::from_named(&named), extra);
    let counts = Counts {
        failed: 1,
        passed: 2,
        skipped: 1,
        ..Counts::default()
    };
    assert_eq!(
        summary_words(&counts, &extra),
        [
            "1 failed",
            "2 passed",
            "2 skipped",
            "1 teardown error",
            "1 collection error"
        ]
    );
    assert_eq!(
        summary_words(&Counts::default(), &Extra::default()),
        [] as [String; 0]
    );
}
