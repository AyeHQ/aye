use std::collections::HashMap;
use std::time::Duration;

use aye_events::{
    Case, Counts, Diagnostic, Event, ExitCode, Failure, Outcome, Phase, ProjectPath, Record,
    RunNumber, Severity, TestId, TestResult, WorkerIndex,
};

use aye_stream::{FileChange, ItemIndex};

use super::choose::{closest, distance};
use super::runs::RunLine;
use super::*;

#[test]
fn digits_are_a_key_like_any_other_unless_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let (cwd, root) = (dir.path(), dir.path());
    for arg in ["1", "12345", "3fae6", "tests.test_a.test_x"] {
        assert!(
            matches!(read(cwd, root, arg), Wanted::Test(selector) if selector == arg),
            "{arg}"
        );
    }
    // A file or directory of that name.
    std::fs::create_dir(cwd.join("7")).unwrap();
    assert!(matches!(read(cwd, root, "7"), Wanted::Path(path) if path == "7"));
}

#[test]
fn the_renderers_are_told_the_runs_number_age_and_stale_files() {
    let mut stored = meta("r81");
    stored.finished_ms = 1_792_091_602_130;
    let path = |path: &str| ProjectPath::new(path).unwrap();
    let stale = [
        (path("tests/test_a.py"), FileChange::Edited),
        (path("tests/test_b.py"), FileChange::Gone),
    ];
    let told = context(&stored, &stale, stored.finished_ms + 120_000);
    assert_eq!(told.run, RunNumber::new(81));
    assert_eq!(told.ago.as_deref(), Some("2 min ago"));
    assert_eq!(
        told.stale,
        std::collections::BTreeMap::from([
            (path("tests/test_a.py"), FileChange::Edited),
            (path("tests/test_b.py"), FileChange::Gone)
        ])
    );
    // A run stored under a hex id has no number; one with no finishing
    // time, no age.
    let told = context(&meta("fedcba9876543210"), &[], 5);
    assert_eq!((told.run, told.ago, told.stale.len()), (None, None, 0));
}

#[test]
fn a_run_is_picked_by_its_number_or_the_start_of_an_old_id() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let started = |run: Option<RunNumber>| {
        let mut record = Record::new(Event::RunStarted {
            schema_version: 1,
            aye_version: "0".into(),
            root: dir.path().display().to_string(),
            collect_only: false,
            workers: 1,
            run,
        });
        record.envelope.run = Some("0123456789abcdef".into());
        record
    };
    for _ in 0..2 {
        let mut writer = aye_history::RunWriter::new(&runs, 10);
        let number = writer.claim();
        writer.record(&started(number));
        writer.record(&Record::new(finished("t.py::test_a", Outcome::Passed)));
        writer.finish().unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    // A run stored before runs were numbered.
    let old = "fedcba9876543210";
    let mut meta = RunStore::new(&runs).meta(&"r1".into()).unwrap();
    meta.run = old.into();
    meta.finished_ms = 1;
    std::fs::write(
        runs.join(format!("{old}.json")),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    let store = RunStore::new(&runs);
    let picked = |wanted: Option<&str>| pick(&store, wanted, 10).map(|meta| meta.run.to_string());
    assert_eq!(picked(None).ok().as_deref(), Some("r2"));
    for wanted in ["r1", "1", "R1", "01"] {
        assert_eq!(picked(Some(wanted)).ok().as_deref(), Some("r1"), "{wanted}");
    }
    assert_eq!(picked(Some("fedc")).ok().as_deref(), Some(old));
    assert_eq!(picked(Some(old)).ok().as_deref(), Some(old));
    for wanted in ["r9", "9", "abcd", "r"] {
        assert!(
            matches!(picked(Some(wanted)), Err(AyeError::Usage(err)) if err.to_string().contains("stored: r2, r1, fedcba9876543210")),
            "{wanted}"
        );
    }
    // A number no stored run has is the start of an old id, when one has
    // it; a number a run has names that run.
    for old in ["9abcdef012345678", "1abcdef012345678"] {
        let meta = RunMeta {
            run: old.into(),
            ..meta.clone()
        };
        std::fs::write(
            runs.join(format!("{old}.json")),
            serde_json::to_vec(&meta).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(picked(Some("9")).ok().as_deref(), Some("9abcdef012345678"));
    assert!(picked(Some("r9")).is_err());
    assert_eq!(picked(Some("1")).ok().as_deref(), Some("r1"));
}

#[test]
fn edit_distance() {
    let chars = |s: &str| s.chars().collect::<Vec<_>>();
    assert_eq!(distance(&chars("kitten"), "sitting"), 3);
    assert_eq!(distance(&chars(""), "abc"), 3);
    assert_eq!(distance(&chars("abc"), ""), 3);
    assert_eq!(distance(&chars("été"), "ete"), 2);
}

#[test]
fn the_closest_ids_come_first() {
    let ids: Vec<TestId> = [
        "tests/test_a.py::test_add",
        "tests/test_a.py::test_remove",
        "tests/test_b.py::test_total",
    ]
    .map(TestId::new)
    .to_vec();
    let found = closest("test_ad", &ids);
    assert_eq!(found[0], "tests/test_a.py::test_add");
    assert_eq!(found.len(), 3);
    assert_eq!(
        closest("tests/test_b.py::test_totl", &ids)[0],
        "tests/test_b.py::test_total"
    );
    // A test of that name in a class, or another file; parameters
    // left out, each test once.
    let ids: Vec<TestId> = [
        "tests/test_v.py::TestV::test_parse[1.0]",
        "tests/test_v.py::TestV::test_parse[2.0]",
        "tests/test_v.py::test_other",
    ]
    .map(TestId::new)
    .to_vec();
    assert_eq!(
        closest("tests/test_v.py::test_parse", &ids),
        [
            "tests/test_v.py::TestV::test_parse",
            "tests/test_v.py::test_other"
        ]
    );
    assert_eq!(
        closest("tests.test_v.test_parse", &ids)[0],
        "tests/test_v.py::TestV::test_parse"
    );
}

#[test]
fn a_file_name_is_closest_to_the_ids_that_end_in_it() {
    // A doctest file's id ends in its name, which has a dot: it is not a
    // dotted name's last part.
    let ids: Vec<TestId> = [
        "pkg/__init__.py::pkg",
        "scripts/tool.py::tool",
        "docs/usage.rst::usage.rst",
        "docs/other.rst::other.rst",
    ]
    .map(TestId::new)
    .to_vec();
    assert_eq!(closest("usage.rst", &ids)[0], "docs/usage.rst::usage.rst");
    assert_eq!(closest("usage.rts", &ids)[0], "docs/usage.rst::usage.rst");
    // A file named from elsewhere: the file's name counts.
    let ids: Vec<TestId> = [
        "tests/unit/test_cart.py::test_add",
        "tests/test_carts.py::test_x",
    ]
    .map(TestId::new)
    .to_vec();
    assert_eq!(
        closest("test_cart.py", &ids)[0],
        "tests/unit/test_cart.py::test_add"
    );
    assert_eq!(
        closest("other/test_cart.py", &ids)[0],
        "tests/unit/test_cart.py::test_add"
    );
}

fn stamped(events: Vec<(Option<u32>, Event)>) -> Vec<Record> {
    events
        .into_iter()
        .zip(0_u64..)
        .map(|((worker, event), seq)| {
            let mut record = Record::new(event);
            record.envelope.run = Some("r".into());
            record.envelope.worker = worker.map(WorkerIndex::from_u32);
            record.envelope.seq = Some(seq);
            record
        })
        .collect()
}

fn finished(id: &str, outcome: Outcome) -> Event {
    let mut result = TestResult::new(TestId::new(id), outcome);
    if outcome.is_bad() {
        result.failures = vec![Failure {
            message: format!("{id} failed"),
            ..Failure::default()
        }];
    }
    Event::TestFinished(Box::new(result))
}

fn started(id: &str) -> Event {
    Event::TestStarted {
        id: TestId::new(id),
    }
}

fn kinds(feed: &[Record]) -> Vec<String> {
    feed.iter()
        .map(|record| {
            let seq = record
                .envelope
                .seq
                .map_or_else(|| "-".to_owned(), |seq| seq.to_string());
            format!("{}@{seq}", record.event.kind())
        })
        .collect()
}

#[test]
fn each_result_comes_with_its_own_records_and_a_key_names_them_all() {
    // One id finished twice (a duplicate), on two workers at once.
    let records = stamped(vec![
        (
            None,
            Event::RunStarted {
                schema_version: 1,
                aye_version: "0".into(),
                root: "/p".into(),
                collect_only: false,
                workers: 2,
                run: None,
            },
        ),
        (Some(0), started("t.py::T::test_t")),
        (Some(1), started("t.py::T::test_t")),
        (Some(1), finished("t.py::T::test_t", Outcome::Failed)),
        (Some(0), finished("t.py::T::test_t", Outcome::Failed)),
        (Some(0), started("t.py::test_ok")),
        (Some(0), finished("t.py::test_ok", Outcome::Passed)),
        (
            None,
            Event::RunFinished {
                exit_code: ExitCode::TestsFailed,
                counts: Counts::default(),
                duration: Duration::ZERO,
                stopped: None,
            },
        ),
    ]);
    let log = RunIndex::of(&records);
    // The second result: worker 0's.
    let shown = [&log.items[ItemIndex::from_u32(1)]];
    let feed = feed(
        &log,
        &shown,
        ShowMode::Details,
        true,
        Some(ExitCode::TestsFailed),
    );
    // Its own `test_started` (worker 0's), not the other's.
    assert_eq!(
        kinds(&feed),
        [
            "run_started@0",
            "test_started@1",
            "test_finished@4",
            "run_finished@-"
        ]
    );
    let meta_like = |feed: &[Record]| match &feed.last().unwrap().event {
        Event::RunFinished {
            exit_code, counts, ..
        } => (*exit_code, counts.failed, counts.passed),
        _ => unreachable!(),
    };
    // The run's exit code, the selection's counts.
    assert_eq!(meta_like(&feed), (ExitCode::TestsFailed, 1, 0));
    let ok = log
        .items
        .iter()
        .find(|item| item.name.as_str() == "t.py::test_ok")
        .unwrap();
    let feed = super::feed(&log, &[ok], ShowMode::List, true, None);
    assert_eq!(meta_like(&feed), (ExitCode::TestsFailed, 0, 1));
    // The test's key: both its results, each with its own records.
    let keys = Keys::of(&records);
    let key = keys
        .key(&TestId::new("t.py::T::test_t"))
        .unwrap()
        .to_string();
    let wanted = [(key.as_str(), Wanted::Test(key.clone()))];
    let chosen = choose(&log, &keys, &wanted, &meta("r1")).ok().unwrap();
    let shown: Vec<&RunItem<'_>> = log
        .items
        .iter_enumerated()
        .filter(|(at, _)| chosen.contains(at))
        .map(|(_, item)| item)
        .collect();
    let feed = super::feed(&log, &shown, ShowMode::Details, true, None);
    assert_eq!(
        kinds(&feed),
        [
            "run_started@0",
            "test_started@1",
            "test_started@2",
            "test_finished@3",
            "test_finished@4",
            "run_finished@-"
        ]
    );
    assert_eq!(meta_like(&feed), (ExitCode::TestsFailed, 2, 0));
}

/// A stored run's summary, as `choose` reads it: its id.
fn meta(run: &str) -> RunMeta {
    RunMeta {
        version: aye_history::store::VERSION,
        run: run.into(),
        root: "/p".into(),
        started_ms: 0,
        finished_ms: 0,
        exit_code: None,
        counts: Counts::default(),
        ms: 0,
        stopped: None,
        extra: std::collections::BTreeMap::new(),
        files: std::collections::BTreeMap::new(),
    }
}

/// Two ids whose keys start with the same `digits` digits.
fn alike(digits: usize) -> (String, String) {
    let mut seen: HashMap<String, String> = HashMap::new();
    (0..)
        .find_map(|n| {
            let id = format!("t.py::test_{n}");
            let start = aye_events::TestKey::of(&id).to_string()[..digits].to_string();
            seen.insert(start, id.clone()).map(|earlier| (earlier, id))
        })
        .unwrap()
}

/// The ids `args` choose in `records`, sorted, or the problem.
fn chosen(records: &[Record], args: &[&str]) -> Result<Vec<String>, String> {
    let log = RunIndex::of(records);
    let keys = Keys::of(records);
    let wanted: Vec<(&str, Wanted)> = args
        .iter()
        .map(|arg| (*arg, Wanted::Test((*arg).to_owned())))
        .collect();
    match choose(&log, &keys, &wanted, &meta("r7")) {
        Ok(chosen) => {
            let mut ids: Vec<String> = chosen
                .into_iter()
                .map(|at| log.items[at].name.as_str().to_owned())
                .collect();
            ids.sort();
            ids.dedup();
            Ok(ids)
        }
        Err(AyeError::Usage(err)) => Err(err.to_string()),
        Err(other) => panic!("{other:?}"),
    }
}

#[test]
fn keys_and_their_start_name_tests_and_the_start_of_several_is_a_usage_error() {
    let (first, second) = alike(4);
    let mut events = vec![(
        None,
        Event::CaseCollected(Box::new(Case::new("t.py::test_never", "pytest"))),
    )];
    for id in [&first, &second, &"t.py::test_other".to_owned()] {
        events.push((Some(0), started(id)));
        events.push((Some(0), finished(id, Outcome::Failed)));
    }
    let records = stamped(events);
    let keys = Keys::of(&records);
    let key = |id: &str| keys.key(&TestId::new(id)).unwrap().to_string();
    // A key, whole or upper case; the start of one test's key, from 4
    // digits; its whole hash.
    let full = aye_events::TestKey::of(&first).to_string();
    for arg in [key(&first), key(&first).to_ascii_uppercase(), full.clone()] {
        assert_eq!(chosen(&records, &[&arg]), Ok(vec![first.clone()]), "{arg}");
    }
    let other = aye_events::TestKey::of("t.py::test_other").to_string();
    let unique = (4..=16)
        .map(|len| &other[..len])
        .find(|start| keys.resolve(&start.parse().unwrap()).len() == 1)
        .unwrap();
    assert_eq!(
        chosen(&records, &[unique]),
        Ok(vec!["t.py::test_other".to_owned()])
    );
    // The start of two tests' keys: both, with their keys and ids.
    let both = chosen(&records, &[&full[..4]]).unwrap_err();
    assert!(
        both.starts_with(&format!("{}: the start of several keys", &full[..4])),
        "{both}"
    );
    // A line each, the keys padded to the longest.
    let width = key(&first).len().max(key(&second).len());
    for id in [&first, &second] {
        assert!(
            both.contains(&format!("\n  {:<width$}  {id}", key(id))),
            "{both}"
        );
    }
    // A test collected that has no result.
    let never = key("t.py::test_never");
    assert_eq!(
        chosen(&records, &[&never]),
        Err(format!(
            "{never}: the key of t.py::test_never, which has no result in run r7"
        ))
    );
    // Hex that is no key's start is read as any other name: here, none.
    let none = (0..0x1_0000)
        .map(|n| format!("{n:04x}"))
        .find(|hex| keys.resolve(&hex.parse().unwrap()).is_empty())
        .unwrap();
    let problem = chosen(&records, &[&none]).unwrap_err();
    assert!(
        problem.starts_with(&format!("{none}: no test of run r7 has that name")),
        "{problem}"
    );
    // Ids and dotted names still name tests.
    assert_eq!(
        chosen(&records, &["t.py::test_other", "t.test_other"]),
        Ok(vec!["t.py::test_other".to_owned()])
    );
}

#[test]
fn the_whole_list_has_what_the_summary_counts_beside_the_tests() {
    let diagnostic = |severity, code: &str, path: &str| {
        Event::Diagnostic(Diagnostic {
            severity,
            code: code.into(),
            path: path.into(),
            span: None,
            message: format!("{code} in {path}"),
        })
    };
    let lifetime = |id: &str, phase| Event::LifetimeFinished {
        lifetime: id.into(),
        failure: Some(Failure {
            phase: Some(phase),
            message: format!("{id} failed"),
            ..Failure::default()
        }),
        duration: Duration::ZERO,
    };
    let mut blocked = TestResult::new(TestId::new("t.py::B::test_b"), Outcome::Error);
    blocked.cause = Some("t.py::B".into());
    let records = stamped(vec![
        (None, diagnostic(Severity::Info, "skipped", "t_skip.py")),
        (
            None,
            diagnostic(Severity::Error, "syntax-error", "t_bad.py"),
        ),
        (
            None,
            diagnostic(Severity::Warning, "plugin-not-loaded", "."),
        ),
        (Some(0), finished("t.py::A::test_a", Outcome::Passed)),
        (Some(0), lifetime("t.py::A", Phase::Teardown)),
        (Some(0), Event::TestFinished(Box::new(blocked))),
        (Some(0), lifetime("t.py::B", Phase::Setup)),
        // A class split across batches tears down in each: one result.
        (Some(1), lifetime("t.py::A", Phase::Teardown)),
        (
            None,
            Event::WorkerFinished {
                exit_status: Some(1),
                error: Some("boom".into()),
            },
        ),
    ]);
    let log = RunIndex::of(&records);
    let items: Vec<(&str, Outcome, bool)> = log
        .items
        .iter()
        .map(|item| (item.name.as_str(), item.outcome, item.blocking))
        .collect();
    assert_eq!(
        items,
        [
            ("t_skip.py", Outcome::Skipped, false),
            ("t_bad.py", Outcome::Error, false),
            ("t.py::A::test_a", Outcome::Passed, false),
            ("t.py::A", Outcome::Error, false),
            ("t.py::B::test_b", Outcome::Error, false),
            ("t.py::B", Outcome::Error, true),
            ("", Outcome::Error, false),
        ]
    );
    // Its details: a lifetime's every failure; a blocked test's lifetime.
    let lifetime_a = [&log.items[ItemIndex::from_u32(3)]];
    let feed_a = feed(&log, &lifetime_a, ShowMode::Details, true, None);
    assert_eq!(
        kinds(&feed_a),
        [
            "lifetime_finished@4",
            "lifetime_finished@7",
            "run_finished@-"
        ]
    );
    let test_b = [&log.items[ItemIndex::from_u32(4)]];
    let feed_b = feed(&log, &test_b, ShowMode::Details, true, None);
    assert_eq!(
        kinds(&feed_b),
        ["test_finished@5", "lifetime_finished@6", "run_finished@-"]
    );
    // The names selectors match, each once.
    let names: Vec<String> = log.names().iter().map(ToString::to_string).collect();
    assert_eq!(
        names,
        [
            "t_skip.py",
            "t_bad.py",
            "t.py::A::test_a",
            "t.py::A",
            "t.py::B::test_b",
            "t.py::B"
        ]
    );
}

/// `aye show --runs --output json`'s lines, field for field in the order
/// they are written, with every count, none, and a reason this aye doesn't
/// know that needs escaping.
#[test]
fn a_stored_runs_json_line_keeps_its_fields_order() {
    let line =
        |meta: &RunMeta, last: bool| serde_json::to_string(&RunLine::new(meta, last)).unwrap();
    let mut every = meta("r81");
    every.started_ms = 1_792_091_602_130;
    every.exit_code = Some(ExitCode::TestsFailed);
    every.counts = Counts {
        passed: 1,
        failed: 2,
        error: 3,
        skipped: 4,
        expected_failure: 5,
        unexpected_pass: 6,
        deselected: 7,
    };
    every.extra = [
        ("skipped_modules", 1),
        ("setup_error", 2),
        ("teardown_error", 3),
        ("lifetime_error", 4),
        ("collection_error", 5),
        ("usage_error", 6),
        ("internal_error", 7),
    ]
    .into_iter()
    .map(|(name, n)| (name.to_owned(), n))
    .collect();
    every.ms = 1234;
    every.stopped = Some(aye_events::StopReason::Maxfail);
    assert_eq!(
        line(&every, true),
        r#"{"type":"run","id":"r81","started":"2026-10-15T19:13:22Z","exit":1,"passed":1,"failed":2,"error":3,"skipped":5,"expected_failure":5,"unexpected_pass":6,"deselected":7,"setup_error":2,"teardown_error":3,"lifetime_error":4,"collection_error":5,"usage_error":6,"internal_error":7,"ms":1234,"stopped":"maxfail","last":true}"#
    );
    assert_eq!(
        line(&meta("r82"), false),
        r#"{"type":"run","id":"r82","started":"1970-01-01T00:00:00Z","ms":0}"#
    );
    let mut odd = meta("0a1b2c3d");
    odd.stopped = Some(aye_events::StopReason::from("said \"stop\"\n"));
    assert_eq!(
        line(&odd, false),
        r#"{"type":"run","id":"0a1b2c3d","started":"1970-01-01T00:00:00Z","ms":0,"stopped":"said \"stop\"\n"}"#
    );
}
