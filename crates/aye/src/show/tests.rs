use aye_events::{Failure, Phase};

use super::*;

#[test]
fn dates_are_utc() {
    assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
    assert_eq!(rfc3339(1_792_091_602_130), "2026-10-15T19:13:22Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
    assert_eq!(rfc3339(4_107_542_399_000), "2100-02-28T23:59:59Z");
}

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
fn ago_says_how_long_since_the_run_finished() {
    let then = 1_792_091_602_130;
    let after = |secs: u64| ago(then + secs * 1000, then);
    assert_eq!(after(0), "just now");
    assert_eq!(after(59), "just now");
    assert_eq!(after(60), "1 min ago");
    assert_eq!(after(150), "2 min ago");
    assert_eq!(after(3_599), "59 min ago");
    assert_eq!(after(3_600), "1 h ago");
    assert_eq!(after(3 * 3_600 + 59), "3 h ago");
    assert_eq!(after(86_400), "1 day ago");
    assert_eq!(after(2 * 86_400 + 1), "2 days ago");
    // A clock that went back: just now.
    assert_eq!(ago(then, then + 5_000), "just now");
}

#[test]
fn the_renderers_are_told_the_runs_number_age_and_stale_files() {
    let mut stored = meta("r81");
    stored.finished_ms = 1_792_091_602_130;
    let stale = [
        ("tests/test_a.py".to_owned(), Change::Edited),
        ("tests/test_b.py".to_owned(), Change::Gone),
    ];
    let told = context(&stored, &stale, stored.finished_ms + 120_000);
    assert_eq!(told.run, Some(81));
    assert_eq!(told.ago.as_deref(), Some("2 min ago"));
    assert_eq!(
        told.stale,
        std::collections::BTreeMap::from([
            ("tests/test_a.py".to_owned(), false),
            ("tests/test_b.py".to_owned(), true)
        ])
    );
    // A run stored under a hex id has no number; one with no finishing
    // time, no age.
    let told = context(&meta("fedcba9876543210"), &[], 5);
    assert_eq!((told.run, told.ago, told.stale.len()), (None, None, 0));
}

#[test]
fn details_come_at_the_second_level_unless_the_flags_say() {
    use clap::Parser as _;
    let verbosity = |argv: &[&str], details, format| {
        let mut all = vec!["aye show"];
        all.extend(argv);
        ShowArgs::try_parse_from(all)
            .unwrap()
            .verbosity(details, format)
    };
    // Details: text's -v, JSON's -vv.
    assert_eq!(verbosity(&[], true, Output::Text), 1);
    assert_eq!(verbosity(&[], true, Output::Json), 2);
    // The list: a run's default.
    assert_eq!(verbosity(&[], false, Output::Text), 0);
    assert_eq!(verbosity(&[], false, Output::Json), 0);
    // Flags mean what they mean in a run.
    assert_eq!(verbosity(&["-v"], true, Output::Json), 1);
    assert_eq!(verbosity(&["-vvv"], true, Output::Text), 3);
    assert_eq!(verbosity(&["-q"], true, Output::Json), -1);
    assert_eq!(verbosity(&["-v", "-q"], true, Output::Text), 0);
    assert_eq!(verbosity(&["-vv"], false, Output::Text), 2);
}

#[test]
fn a_run_is_picked_by_its_number_or_the_start_of_an_old_id() {
    let dir = tempfile::tempdir().unwrap();
    let runs = dir.path().join("runs");
    let started = |run: Option<u64>| {
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
        writer.finish(std::collections::BTreeMap::new()).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    // A run stored before runs were numbered.
    let old = "fedcba9876543210";
    let mut meta = RunStore::new(&runs).meta("r1").unwrap();
    meta.run = old.into();
    meta.finished_ms = 1;
    std::fs::write(
        runs.join(format!("{old}.json")),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    let store = RunStore::new(&runs);
    let picked = |wanted: Option<&str>| pick(&store, wanted, 10).map(|meta| meta.run);
    assert_eq!(picked(None).ok().as_deref(), Some("r2"));
    for wanted in ["r1", "1", "R1", "01"] {
        assert_eq!(picked(Some(wanted)).ok().as_deref(), Some("r1"), "{wanted}");
    }
    assert_eq!(picked(Some("fedc")).ok().as_deref(), Some(old));
    assert_eq!(picked(Some(old)).ok().as_deref(), Some(old));
    for wanted in ["r9", "9", "abcd", "r"] {
        assert!(
            matches!(picked(Some(wanted)), Err(Problem::Usage(message)) if message.contains("stored: r2, r1, fedcba9876543210")),
            "{wanted}"
        );
    }
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
            record.envelope.worker = worker;
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
    let log = Log::of(&records);
    // The second result: worker 0's.
    let shown = [&log.items[1]];
    let feed = feed(
        &records,
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
    let feed = super::feed(&records, &log, &[ok], ShowMode::List, true, None);
    assert_eq!(meta_like(&feed), (ExitCode::TestsFailed, 0, 1));
    // The test's key: both its results, each with its own records.
    let keys = Keys::of(&records);
    let key = keys.key("t.py::T::test_t").unwrap().to_owned();
    let wanted = [(key.as_str(), Wanted::Test(key.clone()))];
    let chosen = choose(&log, &keys, &wanted, &meta("r1")).ok().unwrap();
    let shown: Vec<&Item<'_>> = log
        .items
        .iter()
        .enumerate()
        .filter(|(at, _)| chosen.contains(at))
        .map(|(_, item)| item)
        .collect();
    let feed = super::feed(&records, &log, &shown, ShowMode::Details, true, None);
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
        version: aye_history::runs::VERSION,
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
            let start = aye_events::TestKey::of(&id).0[..digits].to_string();
            seen.insert(start, id.clone()).map(|earlier| (earlier, id))
        })
        .unwrap()
}

/// The ids `args` choose in `records`, sorted, or the problem.
fn chosen(records: &[Record], args: &[&str]) -> Result<Vec<String>, String> {
    let log = Log::of(records);
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
        Err(Problem::Usage(message) | Problem::NoRun(message)) => Err(message),
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
    let key = |id: &str| keys.key(id).unwrap().to_owned();
    // A key, whole or upper case; the start of one test's key, from 4
    // digits; its whole hash.
    let full = aye_events::TestKey::of(&first).0;
    for arg in [key(&first), key(&first).to_ascii_uppercase(), full.clone()] {
        assert_eq!(chosen(&records, &[&arg]), Ok(vec![first.clone()]), "{arg}");
    }
    let other = aye_events::TestKey::of("t.py::test_other").0;
    let unique = (4..=16)
        .map(|len| &other[..len])
        .find(|start| keys.resolve(start).len() == 1)
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
        .find(|hex| keys.resolve(hex).is_empty())
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
    let log = Log::of(&records);
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
    let lifetime_a = [&log.items[3]];
    let feed_a = feed(&records, &log, &lifetime_a, ShowMode::Details, true, None);
    assert_eq!(
        kinds(&feed_a),
        [
            "lifetime_finished@4",
            "lifetime_finished@7",
            "run_finished@-"
        ]
    );
    let test_b = [&log.items[4]];
    let feed_b = feed(&records, &log, &test_b, ShowMode::Details, true, None);
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
