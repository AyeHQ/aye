use std::time::Duration;

use aye_events::{Counts, Envelope, ExitCode, Failure, OutcomeTrail, RunId, RunNumber};

use super::*;

fn id(name: &str) -> TestId {
    TestId::new(name)
}

fn run_started(run: &str, collect_only: bool) -> Record {
    Record {
        envelope: Envelope {
            run: Some(run.into()),
            ..Envelope::default()
        },
        event: Event::RunStarted {
            schema_version: aye_events::SCHEMA_VERSION,
            aye_version: "0.0.1".into(),
            root: "/repo".into(),
            collect_only,
            workers: 1,
            run: None,
        },
    }
}

fn run_finished() -> Record {
    Record::new(Event::RunFinished {
        exit_code: ExitCode::Ok,
        counts: Counts::default(),
        duration: Duration::from_secs(1),
        stopped: None,
    })
}

fn finished(name: &str, outcome: Outcome, ms: u64) -> Record {
    let mut result = TestResult::new(id(name), outcome);
    result.duration = Duration::from_millis(ms);
    if outcome.is_bad() {
        result.failures.push(Failure {
            message: "assert 1 == 2".into(),
            ..Failure::default()
        });
    }
    Record::new(Event::TestFinished(Box::new(result)))
}

fn started(name: &str) -> Record {
    Record::new(Event::TestStarted { id: id(name) })
}

/// A whole run: started, the given tests started and finished, finished.
fn run(history: &mut History, run: &str, tests: &[(&str, Outcome, u64)]) {
    history.record(&run_started(run, false));
    for &(name, outcome, ms) in tests {
        history.record(&started(name));
        history.record(&finished(name, outcome, ms));
    }
    history.record(&run_finished());
}

fn ids(names: &[&str]) -> HashSet<TestId> {
    names.iter().map(|name| id(name)).collect()
}

fn close(left: f64, right: f64) -> bool {
    (left - right).abs() < 1e-9
}

// ---------------------------------------------------------------- fold

#[test]
fn first_duration_is_taken_as_is() {
    let mut history = History::default();
    run(&mut history, "r1", &[("t.py::test_a", Outcome::Passed, 10)]);
    let stats = history.stats(&id("t.py::test_a")).unwrap();
    assert!(close(stats.ewma_ms, 10.0));
    assert!(close(stats.max_ms, 10.0));
    assert!(close(stats.last_ms, 10.0));
    assert_eq!(stats.count, 1);
    assert_eq!(stats.last_outcome, Outcome::Passed);
    assert_eq!(stats.last_run.as_ref().map(RunId::as_str), Some("r1"));
    assert_eq!(stats.flips, 0);
}

#[test]
fn later_durations_are_averaged_with_alpha_0_3() {
    let mut history = History::default();
    run(&mut history, "r1", &[("t.py::test_a", Outcome::Passed, 10)]);
    run(&mut history, "r2", &[("t.py::test_a", Outcome::Failed, 20)]);
    run(&mut history, "r3", &[("t.py::test_a", Outcome::Passed, 5)]);
    let stats = history.stats(&id("t.py::test_a")).unwrap();
    // 10, then 0.3 * 20 + 0.7 * 10 = 13, then 0.3 * 5 + 0.7 * 13 = 10.6.
    assert!(close(stats.ewma_ms, 10.6), "{}", stats.ewma_ms);
    assert!(close(stats.max_ms, 20.0));
    assert!(close(stats.last_ms, 5.0));
    assert_eq!(stats.count, 3);
    assert_eq!(stats.last_outcome, Outcome::Passed);
    assert_eq!(stats.last_run.as_ref().map(RunId::as_str), Some("r3"));
    assert_eq!(
        history.duration(&id("t.py::test_a")),
        Some(Duration::from_secs_f64(0.0106))
    );
}

#[test]
fn the_envelope_run_wins_over_the_run_started() {
    let mut history = History::default();
    history.record(&run_started("r1", false));
    let mut record = finished("t.py::test_a", Outcome::Passed, 1);
    record.envelope.run = Some("r9".into());
    history.record(&started("t.py::test_a"));
    history.record(&record);
    history.record(&started("t.py::test_b"));
    history.record(&finished("t.py::test_b", Outcome::Passed, 1));
    assert_eq!(
        history
            .stats(&id("t.py::test_a"))
            .unwrap()
            .last_run
            .as_ref()
            .map(RunId::as_str),
        Some("r9")
    );
    assert_eq!(
        history
            .stats(&id("t.py::test_b"))
            .unwrap()
            .last_run
            .as_ref()
            .map(RunId::as_str),
        Some("r1")
    );
}

#[test]
fn a_run_without_an_id_clears_last_run() {
    let mut history = History::default();
    run(&mut history, "r1", &[("t.py::test_a", Outcome::Passed, 1)]);
    history.record(&finished("t.py::test_a", Outcome::Passed, 1));
    assert_eq!(history.stats(&id("t.py::test_a")).unwrap().last_run, None);
}

#[test]
fn records_outside_a_run_are_recorded() {
    let mut history = History::default();
    history.record(&finished("t.py::test_a", Outcome::Failed, 3));
    assert_eq!(history.last_failed(), ids(&["t.py::test_a"]));
    assert_eq!(
        history.duration(&id("t.py::test_a")),
        Some(Duration::from_millis(3))
    );
}

#[test]
fn a_test_that_never_ran_keeps_its_durations() {
    let mut history = History::default();
    run(&mut history, "r1", &[("t.py::test_a", Outcome::Passed, 40)]);
    for name in ["t.py::test_a", "t.py::test_b"] {
        let mut result = TestResult::new(id(name), Outcome::Error);
        result.cause = Some("class:t.py::TestX".into());
        history.record(&Record::new(Event::TestFinished(Box::new(result))));
    }
    let a = history.stats(&id("t.py::test_a")).unwrap();
    assert_eq!(a.count, 1);
    assert!(close(a.ewma_ms, 40.0));
    assert_eq!(a.last_outcome, Outcome::Error);
    let b = history.stats(&id("t.py::test_b")).unwrap();
    assert_eq!(b.count, 0);
    assert_eq!(b.ewma(), None);
    assert_eq!(history.duration(&id("t.py::test_b")), None);
    assert!(!history.durations().contains_key(&id("t.py::test_b")));
    assert_eq!(
        history.last_failed(),
        ids(&["t.py::test_a", "t.py::test_b"])
    );
}

#[test]
fn collect_only_runs_record_nothing() {
    let mut history = History::default();
    history.record(&run_started("r1", true));
    history.record(&finished("t.py::test_a", Outcome::Failed, 5));
    history.record(&run_finished());
    assert!(history.is_empty());
    assert!(!history.changed());

    // The next run records again, with or without a run_started.
    history.record(&finished("t.py::test_a", Outcome::Failed, 5));
    assert_eq!(history.last_failed(), ids(&["t.py::test_a"]));
    history.record(&run_started("r2", true));
    history.record(&finished("t.py::test_a", Outcome::Passed, 5));
    history.record(&run_started("r3", false));
    history.record(&started("t.py::test_b"));
    history.record(&finished("t.py::test_b", Outcome::Passed, 5));
    assert_eq!(history.last_failed(), ids(&["t.py::test_a"]));
    assert_eq!(history.stats(&id("t.py::test_a")).unwrap().count, 1);
    assert_eq!(history.stats(&id("t.py::test_b")).unwrap().count, 1);
}

#[test]
fn other_events_are_ignored() {
    let mut history = History::default();
    history.record(&Record::new(Event::Unknown));
    history.record(&Record::new(Event::CollectionStarted));
    history.record(&Record::new(Event::TestStarted {
        id: id("t.py::test_a"),
    }));
    history.record(&Record::new(Event::SubOutcome {
        id: id("t.py::test_a"),
        name: "case 1".into(),
        outcome: Outcome::Failed,
        reason: None,
        failure: None,
        duration: Duration::from_millis(1),
    }));
    assert!(history.is_empty());
    assert!(!history.changed());
}

#[test]
fn records_from_the_wire_fold() {
    let mut history = History::default();
    for line in [
        r#"{"run":"r1","event":"run_started","schema_version":1,"aye_version":"0.0.1","root":"/repo","workers":2}"#,
        r#"{"run":"r1","seq":1,"event":"some_future_event","payload":{"x":1}}"#,
        r#"{"run":"r1","seq":2,"event":"test_started","id":"t.py::test_a"}"#,
        r#"{"run":"r1","seq":3,"event":"test_finished","id":"t.py::test_a","outcome":"failed","duration_ms":1.5}"#,
        r#"{"run":"r1","seq":4,"event":"run_finished","exit_code":1,"counts":{"failed":1},"duration_ms":9.0}"#,
    ] {
        history.record(&serde_json::from_str(line).unwrap());
    }
    assert_eq!(history.last_failed(), ids(&["t.py::test_a"]));
    assert_eq!(
        history.duration(&id("t.py::test_a")),
        Some(Duration::from_micros(1500))
    );
}

// ---------------------------------------------------------------- last failed

#[test]
fn failed_and_error_join_last_failed() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_pass", Outcome::Passed, 1),
            ("t.py::test_fail", Outcome::Failed, 1),
            ("t.py::test_error", Outcome::Error, 1),
            ("t.py::test_skip", Outcome::Skipped, 1),
            ("t.py::test_xfail", Outcome::ExpectedFailure, 1),
            ("t.py::test_xpass", Outcome::UnexpectedPass, 1),
        ],
    );
    assert_eq!(
        history.last_failed(),
        ids(&["t.py::test_fail", "t.py::test_error"])
    );
    assert!(history.is_last_failed(&id("t.py::test_fail")));
    assert!(!history.is_last_failed(&id("t.py::test_pass")));
}

#[test]
fn any_other_outcome_leaves_last_failed() {
    for outcome in [
        Outcome::Passed,
        Outcome::Skipped,
        Outcome::ExpectedFailure,
        Outcome::UnexpectedPass,
    ] {
        let mut history = History::default();
        run(&mut history, "r1", &[("t.py::test_a", Outcome::Failed, 1)]);
        run(&mut history, "r2", &[("t.py::test_a", outcome, 1)]);
        assert!(history.last_failed().is_empty(), "{outcome:?}");
    }
}

#[test]
fn tests_that_did_not_run_stay_in_last_failed() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("a.py::test_1", Outcome::Failed, 1),
            ("a.py::test_2", Outcome::Error, 1),
            ("b.py::test_3", Outcome::Failed, 1),
            ("b.py::test_4", Outcome::Passed, 1),
        ],
    );
    // As after `--lf a.py`: only a.py's failures ran, one is fixed.
    run(
        &mut history,
        "r2",
        &[
            ("a.py::test_1", Outcome::Passed, 1),
            ("a.py::test_2", Outcome::Error, 1),
        ],
    );
    assert_eq!(
        history.last_failed(),
        ids(&["a.py::test_2", "b.py::test_3"])
    );
    // A run in which nothing ran changes nothing.
    run(&mut history, "r3", &[]);
    assert_eq!(
        history.last_failed(),
        ids(&["a.py::test_2", "b.py::test_3"])
    );
}

/// Every skip leaves last failed, whatever its reason, as pytest's does
/// (pytest: a test that failed, then `pytest.skip("not run: flaky")`, is
/// out of `lastfailed`, and `--lf` runs everything). History once kept a
/// test skipped with a reason starting `not run:` as not run, for an
/// adapter's `--setup-plan`, which is a usage error now: no adapter says
/// that any more, and a test's own skip must not keep it in the set.
#[test]
fn every_skip_leaves_last_failed_whatever_its_reason() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_a", Outcome::Failed, 40),
            ("t.py::test_b", Outcome::Failed, 60),
        ],
    );
    assert_eq!(
        history.last_failed(),
        ids(&["t.py::test_a", "t.py::test_b"])
    );
    history.record(&run_started("r2", false));
    for (name, reason) in [
        ("t.py::test_a", "needs a database"),
        ("t.py::test_b", "not run: flaky"),
    ] {
        history.record(&started(name));
        let mut result = TestResult::new(id(name), Outcome::Skipped);
        result.duration = Duration::from_micros(200);
        result.reason = Some(reason.into());
        history.record(&Record::new(Event::TestFinished(Box::new(result))));
    }
    history.record(&run_finished());
    assert!(history.last_failed().is_empty());
    for name in ["t.py::test_a", "t.py::test_b"] {
        let stats = history.stats(&id(name)).unwrap();
        assert_eq!((stats.last_outcome, stats.count), (Outcome::Skipped, 2));
    }
    assert!(history.changed());
}

#[test]
fn a_test_that_finishes_without_starting_did_not_run() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_a", Outcome::Failed, 40),
            ("t.py::test_b", Outcome::Passed, 50),
        ],
    );
    history.record(&run_started("r2", false));
    // A test that never started finishing well changes nothing.
    history.record(&finished("t.py::test_a", Outcome::Skipped, 0));
    history.record(&finished("t.py::test_a", Outcome::Passed, 0));
    // One the runner fails without its having started (a worker that
    // crashed again before it ran) is a failure, but not a duration.
    history.record(&finished("t.py::test_b", Outcome::Error, 0));
    history.record(&run_finished());
    let a = history.stats(&id("t.py::test_a")).unwrap();
    assert_eq!(a.last_outcome, Outcome::Failed);
    assert_eq!(a.last_run.as_ref().map(RunId::as_str), Some("r1"));
    let b = history.stats(&id("t.py::test_b")).unwrap();
    assert_eq!(b.last_outcome, Outcome::Error);
    assert_eq!(b.last_run.as_ref().map(RunId::as_str), Some("r2"));
    assert_eq!(b.count, 1);
    assert!(close(b.ewma_ms, 50.0));
    assert_eq!(
        history.last_failed(),
        ids(&["t.py::test_a", "t.py::test_b"])
    );
    // A start in an earlier run doesn't count for the next one.
    history.record(&run_started("r3", false));
    history.record(&finished("t.py::test_b", Outcome::Passed, 0));
    assert!(history.is_last_failed(&id("t.py::test_b")));
}

#[test]
fn the_last_result_in_a_run_wins() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_a", Outcome::Failed, 1),
            ("t.py::test_a", Outcome::Passed, 1),
            ("t.py::test_b", Outcome::Passed, 1),
            ("t.py::test_b", Outcome::Error, 1),
        ],
    );
    assert_eq!(history.last_failed(), ids(&["t.py::test_b"]));
}

// ---------------------------------------------------------------- queries

#[test]
fn durations_holds_every_test_with_a_duration() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("a.py::test_1", Outcome::Passed, 10),
            ("a.py::test_2", Outcome::Skipped, 0),
        ],
    );
    let durations = history.durations();
    assert_eq!(durations.len(), 2);
    assert_eq!(durations[&id("a.py::test_1")], Duration::from_millis(10));
    assert_eq!(durations[&id("a.py::test_2")], Duration::ZERO);
    assert_eq!(history.duration(&id("a.py::test_9")), None);
}

#[test]
fn durations_by_group_sums_per_group() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("a.py::test_1", Outcome::Passed, 10),
            ("a.py::TestX::test_2", Outcome::Failed, 30),
            ("b.py::test_3", Outcome::Passed, 5),
        ],
    );
    let by_file = history.durations_by_group(|id| id.path().to_project_path());
    assert_eq!(by_file.len(), 2);
    assert_eq!(by_file["a.py"], Duration::from_millis(40));
    assert_eq!(by_file["b.py"], Duration::from_millis(5));
}

#[test]
fn estimate_sums_the_parameter_ids_of_an_unparametrized_id() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_x[1]", Outcome::Passed, 10),
            ("t.py::test_x[2-a]", Outcome::Passed, 20),
            ("t.py::test_x[[nested]]", Outcome::Passed, 5),
            ("t.py::test_xy[1]", Outcome::Passed, 1000),
            ("t.py::test_x_slow", Outcome::Passed, 1000),
            ("t.py::test_y", Outcome::Passed, 7),
        ],
    );
    assert_eq!(
        history.estimate(&id("t.py::test_x")),
        Some(Duration::from_millis(35))
    );
    assert_eq!(
        history.estimate(&id("t.py::test_x[1]")),
        Some(Duration::from_millis(10))
    );
    assert_eq!(
        history.estimate(&id("t.py::test_y")),
        Some(Duration::from_millis(7))
    );
    assert_eq!(history.estimate(&id("t.py::test_z")), None);
    assert_eq!(history.estimate(&id("t.py::test_z[1]")), None);
    assert_eq!(history.estimate(&id("t.py")), None);
}

#[test]
fn estimate_prefers_the_id_itself() {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_x", Outcome::Passed, 3),
            ("t.py::test_x[1]", Outcome::Passed, 10),
        ],
    );
    assert_eq!(
        history.estimate(&id("t.py::test_x")),
        Some(Duration::from_millis(3))
    );
}

// ---------------------------------------------------------------- persistence

fn sample() -> History {
    let mut history = History::default();
    run(
        &mut history,
        "r1",
        &[
            ("a.py::test_1", Outcome::Passed, 10),
            ("a.py::test_2", Outcome::Failed, 20),
            ("b.py::TestX::test_3[x]", Outcome::Error, 3),
        ],
    );
    run(&mut history, "r2", &[("a.py::test_1", Outcome::Skipped, 4)]);
    history
}

#[test]
fn save_and_load_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("project/.aye_cache");
    let history = sample();
    history.save(&cache).unwrap();

    let loaded = History::load(&cache);
    assert_eq!(loaded.tests, history.tests);
    assert_eq!(loaded.last_failed, history.last_failed);
    assert!(!loaded.changed());
}

#[test]
fn recording_continues_from_a_loaded_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut history = History::default();
    run(&mut history, "r1", &[("t.py::test_a", Outcome::Failed, 10)]);
    history.save(dir.path()).unwrap();

    let mut loaded = History::load(dir.path());
    run(&mut loaded, "r2", &[("t.py::test_a", Outcome::Passed, 20)]);
    let stats = loaded.stats(&id("t.py::test_a")).unwrap();
    assert!(close(stats.ewma_ms, 13.0));
    assert_eq!(stats.count, 2);
    assert!(loaded.last_failed().is_empty());
    assert!(loaded.changed());
}

#[test]
fn the_file_format_is_pinned() {
    let mut history = History::default();
    let mut result = TestResult::new(id("t.py::test_b"), Outcome::Failed);
    result.duration = Duration::from_nanos(1_234_567);
    history.record(&Record::new(Event::TestFinished(Box::new(result))));
    run(
        &mut history,
        "r1",
        &[
            ("t.py::test_a", Outcome::Passed, 10),
            ("t.py::test_a", Outcome::Passed, 20),
        ],
    );
    numbered_run(
        &mut history,
        "e14d5f08d1554fbc",
        Some(81),
        &[("t.py::test_c", Outcome::Error, 2)],
    );
    let dir = tempfile::tempdir().unwrap();
    history.save(dir.path()).unwrap();
    let text = fs::read_to_string(dir.path().join(FILE)).unwrap();
    assert_eq!(
        text,
        concat!(
            r#"{"version":1,"tests":{"#,
            r#""t.py::test_a":{"ewma_ms":13.0,"max_ms":20.0,"count":2,"last_ms":20.0,"last_outcome":"passed","last_run":"r1","flips":0,"recent":"p"},"#,
            r#""t.py::test_b":{"ewma_ms":1.235,"max_ms":1.235,"count":1,"last_ms":1.235,"last_outcome":"failed","flips":0,"recent":"f"},"#,
            r#""t.py::test_c":{"ewma_ms":2.0,"max_ms":2.0,"count":1,"last_ms":2.0,"last_outcome":"error","last_run":"e14d5f08d1554fbc","flips":0,"recent":"e","bad_since":81}"#,
            r#"},"last_failed":["t.py::test_b","t.py::test_c"]}"#,
            "\n",
        )
    );
}

// ---------------------------------------------------------------- recent outcomes

/// A whole run numbered `number` in the run store.
fn numbered_run(
    history: &mut History,
    run: &str,
    number: Option<u64>,
    tests: &[(&str, Outcome, u64)],
) {
    let mut opening = run_started(run, false);
    if let Event::RunStarted { run, .. } = &mut opening.event {
        *run = number.and_then(RunNumber::new);
    }
    history.record(&opening);
    for &(name, outcome, ms) in tests {
        history.record(&started(name));
        history.record(&finished(name, outcome, ms));
    }
    history.record(&run_finished());
}

fn recent(history: &History, name: &str) -> (String, Option<u64>) {
    let stats = history.stats(&id(name)).unwrap();
    (
        stats.recent.as_str().to_owned(),
        stats.bad_since.map(RunNumber::get),
    )
}

#[test]
fn recent_has_a_letter_per_run_oldest_first_the_last_20() {
    let mut history = History::default();
    let outcomes = [
        Outcome::Passed,
        Outcome::Failed,
        Outcome::Error,
        Outcome::Skipped,
        Outcome::ExpectedFailure,
        Outcome::UnexpectedPass,
    ];
    for (n, outcome) in (1..).zip(outcomes) {
        numbered_run(&mut history, "x", Some(n), &[("t.py::test_a", outcome, 1)]);
    }
    assert_eq!(recent(&history, "t.py::test_a").0, "pfesxu");
    for n in 7..=30 {
        numbered_run(
            &mut history,
            "x",
            Some(n),
            &[("t.py::test_a", Outcome::Passed, 1)],
        );
    }
    assert_eq!(
        recent(&history, "t.py::test_a").0,
        "p".repeat(OutcomeTrail::MAX)
    );
    numbered_run(
        &mut history,
        "x",
        Some(31),
        &[("t.py::test_a", Outcome::Failed, 1)],
    );
    assert_eq!(
        recent(&history, "t.py::test_a").0,
        format!("{}f", "p".repeat(OutcomeTrail::MAX - 1))
    );
    // A test that didn't run gets no letter; one a lifetime kept from
    // running, a bad one.
    let mut history = History::default();
    numbered_run(
        &mut history,
        "x",
        Some(1),
        &[("t.py::test_a", Outcome::Passed, 1)],
    );
    history.record(&run_started("y", false));
    history.record(&finished("t.py::test_a", Outcome::Skipped, 0));
    history.record(&run_finished());
    assert_eq!(recent(&history, "t.py::test_a").0, "p");
    history.record(&run_started("z", false));
    let mut blocked = TestResult::new(id("t.py::test_a"), Outcome::Error);
    blocked.cause = Some("t.py::T".into());
    history.record(&Record::new(Event::TestFinished(Box::new(blocked))));
    history.record(&run_finished());
    assert_eq!(recent(&history, "t.py::test_a").0, "pe");
}

#[test]
fn bad_since_is_the_run_a_streak_of_bad_outcomes_began_in() {
    let mut history = History::default();
    let test = |outcome| [("t.py::test_a", outcome, 1)];
    numbered_run(&mut history, "x", Some(1), &test(Outcome::Passed));
    assert_eq!(recent(&history, "t.py::test_a").1, None);
    numbered_run(&mut history, "x", Some(2), &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a").1, Some(2));
    // Still bad, failed or error: the streak goes on.
    numbered_run(&mut history, "x", Some(3), &test(Outcome::Error));
    numbered_run(&mut history, "x", Some(4), &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a").1, Some(2));
    // Not bad: none; bad again, a new streak.
    numbered_run(&mut history, "x", Some(5), &test(Outcome::Skipped));
    assert_eq!(recent(&history, "t.py::test_a").1, None);
    numbered_run(&mut history, "x", Some(6), &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a").1, Some(6));
    // A run that isn't stored has no number: a streak it begins has none,
    // one it goes on with keeps its own.
    numbered_run(&mut history, "x", None, &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a").1, Some(6));
    numbered_run(&mut history, "x", None, &test(Outcome::Passed));
    numbered_run(&mut history, "x", None, &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a"), ("pfefsffpf".into(), None));
    numbered_run(&mut history, "x", Some(9), &test(Outcome::Failed));
    assert_eq!(recent(&history, "t.py::test_a").1, None);
}

#[test]
fn previous_is_what_history_knew_before_the_run() {
    let mut history = History::default();
    let test = |outcome| [("t.py::test_a", outcome, 1)];
    // Never recorded: nothing.
    assert_eq!(history.previous(&id("t.py::test_a")), None);
    numbered_run(&mut history, "x", Some(1), &test(Outcome::Passed));
    numbered_run(&mut history, "x", Some(2), &test(Outcome::Failed));
    // Before a run: its outcomes, and the run its bad streak began in.
    assert_eq!(
        history.previous(&id("t.py::test_a")),
        Some(Previous {
            outcomes: "pf".into(),
            since: RunNumber::new(2)
        })
    );
    // During one, the same once this run recorded it, whatever it was in
    // this run; a test that finishes twice in a run has one letter, the
    // last result's.
    let mut opening = run_started("y", false);
    if let Event::RunStarted { run, .. } = &mut opening.event {
        *run = RunNumber::new(3);
    }
    history.record(&opening);
    history.record(&started("t.py::test_a"));
    history.record(&finished("t.py::test_a", Outcome::Passed, 1));
    assert_eq!(
        history.previous(&id("t.py::test_a")),
        Some(Previous {
            outcomes: "pf".into(),
            since: RunNumber::new(2)
        })
    );
    assert_eq!(recent(&history, "t.py::test_a"), ("pfp".into(), None));
    history.record(&started("t.py::test_a"));
    history.record(&finished("t.py::test_a", Outcome::Error, 1));
    assert_eq!(recent(&history, "t.py::test_a"), ("pfe".into(), Some(2)));
    history.record(&run_finished());
    // A test that passed last: no streak.
    numbered_run(&mut history, "x", Some(4), &test(Outcome::Passed));
    assert_eq!(
        history.previous(&id("t.py::test_a")),
        Some(Previous {
            outcomes: "pfep".into(),
            since: None
        })
    );
}

#[test]
fn a_file_without_recent_outcomes_loads_with_its_durations() {
    let history = load_text(
        r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":5.0,"max_ms":5.0,"count":1,"last_ms":5.0,"last_outcome":"failed"}},"last_failed":["t.py::test_a"]}"#,
    );
    assert_eq!(
        history.duration(&id("t.py::test_a")),
        Some(Duration::from_millis(5))
    );
    assert_eq!(recent(&history, "t.py::test_a"), (String::new(), None));
    assert_eq!(history.previous(&id("t.py::test_a")), None);
    // What a hand left in `recent` is tidied: letters only, the last 20.
    let history = load_text(&format!(
        r#"{{"version":1,"tests":{{"t.py::test_a":{{"ewma_ms":5.0,"max_ms":5.0,"count":1,"last_ms":5.0,"last_outcome":"failed","recent":"zz{}f","bad_since":3}}}}}}"#,
        "p".repeat(30)
    ));
    assert_eq!(
        recent(&history, "t.py::test_a"),
        (format!("{}f", "p".repeat(19)), Some(3))
    );
}

fn assert_marked(dir: &Path) {
    let gitignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|line| line == "*"), "{gitignore}");
    let tag = fs::read_to_string(dir.join("CACHEDIR.TAG")).unwrap();
    assert!(tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55"));
}

fn assert_unmarked(dir: &Path) {
    assert!(!dir.join(".gitignore").exists(), "{}", dir.display());
    assert!(!dir.join("CACHEDIR.TAG").exists(), "{}", dir.display());
}

#[test]
fn save_marks_a_directory_it_creates_as_a_cache() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join(".aye_cache");
    sample().save(&cache).unwrap();
    assert_marked(&cache);
}

#[test]
fn the_outermost_directory_created_is_the_one_marked() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("nested/.aye_cache");
    sample().save(&cache).unwrap();
    assert_marked(&dir.path().join("nested"));
    assert_unmarked(&cache);
}

/// Findings 68 and 78: the refinement cache created `.aye/cache/…` first,
/// so `.aye` was never marked.
#[test]
fn the_state_directory_is_marked_whoever_creates_it_first() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".aye");
    // The refinement cache comes first...
    prepare_dir(&state.join("cache")).unwrap();
    assert_marked(&state);
    assert_unmarked(&state.join("cache"));
    // ...and history finds it marked.
    sample().save(&state).unwrap();
    assert_marked(&state);

    // History first, then the cache: no second set of markers inside.
    let other = tempfile::tempdir().unwrap();
    let state = other.path().join(".aye");
    sample().save(&state).unwrap();
    prepare_dir(&state.join("cache")).unwrap();
    assert_marked(&state);
    assert_unmarked(&state.join("cache"));
}

#[test]
fn an_unmarked_state_directory_from_0_0_1_is_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join(".aye");
    fs::create_dir_all(state.join("cache/refine/pytest")).unwrap();
    fs::write(state.join(FILE), "{}").unwrap();
    sample().save(&state).unwrap();
    assert_marked(&state);

    // Anything of the user's in it, and it is left alone.
    let other = tempfile::tempdir().unwrap();
    let state = other.path().join(".aye");
    fs::create_dir_all(state.join("cache/refine/pytest")).unwrap();
    fs::write(state.join("notes.txt"), "mine").unwrap();
    sample().save(&state).unwrap();
    assert_unmarked(&state);

    // So is a directory without the refinement cache.
    let mine = tempfile::tempdir().unwrap();
    fs::write(mine.path().join(FILE), "{}").unwrap();
    sample().save(mine.path()).unwrap();
    assert_unmarked(mine.path());
}

#[test]
fn save_leaves_an_existing_directory_alone() {
    let dir = tempfile::tempdir().unwrap();
    sample().save(dir.path()).unwrap();
    assert!(!dir.path().join(".gitignore").exists());
    assert!(!dir.path().join("CACHEDIR.TAG").exists());
}

#[test]
fn prepare_dir_keeps_existing_markers() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join(".aye_cache");
    prepare_dir(&cache).unwrap();
    fs::write(cache.join(".gitignore"), "mine\n").unwrap();
    prepare_dir(&cache).unwrap();
    sample().save(&cache).unwrap();
    assert_eq!(
        fs::read_to_string(cache.join(".gitignore")).unwrap(),
        "mine\n"
    );
}

#[test]
fn save_replaces_the_file_and_leaves_no_temporary_behind() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join(".aye_cache");
    History::default().save(&cache).unwrap();
    let history = sample();
    history.save(&cache).unwrap();
    assert_eq!(History::load(&cache).tests, history.tests);
    let mut names: Vec<_> = fs::read_dir(&cache)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, [".gitignore", "CACHEDIR.TAG", FILE]);
}

#[cfg(unix)]
#[test]
fn the_file_has_the_permissions_of_an_ordinary_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    sample().save(dir.path()).unwrap();
    fs::write(dir.path().join("ordinary"), "").unwrap();
    let mode = |name: &str| {
        fs::metadata(dir.path().join(name))
            .unwrap()
            .permissions()
            .mode()
    };
    assert_eq!(mode(FILE), mode("ordinary"));
}

#[test]
fn save_reports_a_directory_it_cannot_create() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("taken");
    fs::write(&file, "").unwrap();
    assert!(sample().save(&file).is_err());
    assert!(sample().save(&file.join("below")).is_err());
}

// ---------------------------------------------------------------- invalidation

fn load_text(text: &str) -> History {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(FILE), text).unwrap();
    History::load(dir.path())
}

const VALID: &str = r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":2.0,"max_ms":2.0,"count":1,"last_ms":2.0,"last_outcome":"failed","flips":0}},"last_failed":["t.py::test_a"]}"#;

#[test]
fn a_valid_file_loads() {
    let history = load_text(VALID);
    assert_eq!(history.last_failed(), ids(&["t.py::test_a"]));
    assert_eq!(
        history.duration(&id("t.py::test_a")),
        Some(Duration::from_millis(2))
    );
}

#[test]
fn another_version_loads_as_empty() {
    for version in ["0", "2", "99"] {
        let text = VALID.replace(r#""version":1"#, &format!(r#""version":{version}"#));
        assert!(load_text(&text).is_empty(), "version {version}");
    }
    assert!(load_text(&VALID.replace(r#""version":1,"#, "")).is_empty());
    assert!(load_text(r#"{"version":2,"tests":[1,2,3]}"#).is_empty());
}

#[test]
fn a_missing_file_loads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    assert!(History::load(dir.path()).is_empty());
    assert!(History::load(&dir.path().join("does/not/exist")).is_empty());
}

#[test]
fn a_corrupt_file_loads_as_empty() {
    for text in [
        "",
        "\0\u{1}garbage",
        "null",
        "[]",
        &VALID[..VALID.len() / 2],
        r#"{"version":"1"}"#,
        r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":"fast"}}}"#,
        r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":1,"max_ms":1,"count":1,"last_ms":1,"last_outcome":"unknown"}}}"#,
        r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":-1,"max_ms":1,"count":1,"last_ms":1,"last_outcome":"passed"}}}"#,
        r#"{"version":1,"last_failed":"t.py::test_a"}"#,
    ] {
        assert!(load_text(text).is_empty(), "{text}");
    }
}

#[test]
fn a_directory_in_place_of_the_file_loads_as_empty() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(FILE)).unwrap();
    assert!(History::load(dir.path()).is_empty());
}

#[test]
fn unknown_fields_and_missing_optional_ones_are_tolerated() {
    let history = load_text(
        r#"{"version":1,"future":true,"tests":{"t.py::test_a":{"ewma_ms":1,"max_ms":1,"count":1,"last_ms":1,"last_outcome":"passed","coverage":[1,2]}}}"#,
    );
    let stats = history.stats(&id("t.py::test_a")).unwrap();
    assert_eq!(stats.last_run, None);
    assert_eq!(stats.flips, 0);
    assert!(history.last_failed().is_empty());
    assert!(load_text(r#"{"version":1}"#).is_empty());
}

#[test]
fn absurd_durations_saturate() {
    let history = load_text(
        r#"{"version":1,"tests":{"t.py::test_a":{"ewma_ms":1e307,"max_ms":1e307,"count":1,"last_ms":1e307,"last_outcome":"passed"}}}"#,
    );
    assert_eq!(history.duration(&id("t.py::test_a")), Some(Duration::MAX));
    let dir = tempfile::tempdir().unwrap();
    history.save(dir.path()).unwrap();
    assert_eq!(
        History::load(dir.path()).duration(&id("t.py::test_a")),
        Some(Duration::MAX)
    );
}

// ---------------------------------------------------------------- adapters

/// Real runs' logs (paths replaced by `/repo`) of a project running the
/// doctest, unittest and pytest adapters together: a doctest, a pytest
/// test, a unittest test whose `setUpClass` fails and one failing a
/// subtest; then the same project fixed.
const ADAPTERS_FAILING: &str = include_str!("logs/adapters_failing.jsonl");
const ADAPTERS_PASSING: &str = include_str!("logs/adapters_passing.jsonl");

fn fold(history: &mut History, log: &str) {
    for line in log.lines() {
        history.record(&serde_json::from_str(line).unwrap());
    }
}

#[test]
fn every_adapters_results_are_recorded_per_id() {
    let mut history = History::default();
    fold(&mut history, ADAPTERS_FAILING);
    assert_eq!(
        history.last_failed(),
        ids(&[
            "pkg/m.py::pkg.m.add",
            "tests/test_p.py::test_p",
            "tests/test_u.py::Blocked::test_b",
            "tests/test_u.py::Subs::test_s",
        ])
    );
    // Tests only: no lifetime, no doctest example, no subtest.
    assert_eq!(history.tests.len(), 4);
    let doctest = history.stats(&id("pkg/m.py::pkg.m.add")).unwrap();
    assert_eq!((doctest.last_outcome, doctest.count), (Outcome::Failed, 1));
    // A test a failed class setup kept from running took no time.
    let blocked = history.stats(&blocked_id()).unwrap();
    assert_eq!((blocked.last_outcome, blocked.count), (Outcome::Error, 0));
    assert!(history.duration(&blocked_id()).is_none());

    fold(&mut history, ADAPTERS_PASSING);
    assert!(history.last_failed().is_empty());
    assert_eq!(history.tests.len(), 4);
    let blocked = history.stats(&blocked_id()).unwrap();
    assert_eq!((blocked.last_outcome, blocked.count), (Outcome::Passed, 1));
    assert_eq!(
        history
            .stats(&id("tests/test_u.py::Subs::test_s"))
            .unwrap()
            .count,
        2
    );
}

fn blocked_id() -> TestId {
    id("tests/test_u.py::Blocked::test_b")
}

// ---------------------------------------------------------------- crashes and stops

/// A real run's log (paths replaced by `/repo`): a unittest test calling
/// `os._exit`, so batch 0's module and class lifetimes start and never
/// finish; a new worker runs the test after it.
const UNITTEST_CRASH: &str = include_str!("logs/unittest_crash.jsonl");
/// `timeout -s INT 2 aye` while a unittest test sleeps: its lifetimes and
/// the test start, nothing finishes, then `run_finished`.
const UNITTEST_STOPPED: &str = include_str!("logs/unittest_stopped.jsonl");

/// F43: history doesn't depend on `lifetime_started` and
/// `lifetime_finished` pairing up: a log folds the same without them.
#[test]
fn lifetimes_a_crash_or_a_stop_left_open_change_nothing() {
    let without_lifetimes = |log: &str| -> String {
        log.lines()
            .filter(|line| !line.contains(r#""event":"lifetime_"#))
            .flat_map(|line| [line, "\n"])
            .collect()
    };
    for log in [UNITTEST_CRASH, UNITTEST_STOPPED] {
        assert!(log.contains(r#""event":"lifetime_started""#));
        let mut history = History::default();
        fold(&mut history, log);
        let mut bare = History::default();
        fold(&mut bare, &without_lifetimes(log));
        assert_eq!(history.tests, bare.tests);
        assert_eq!(history.last_failed, bare.last_failed);
    }

    let mut history = History::default();
    fold(&mut history, UNITTEST_CRASH);
    assert_eq!(
        history.last_failed(),
        ids(&["tests/test_a.py::A::test_2_exit"])
    );
    for name in ["tests/test_a.py::A::test_1", "tests/test_a.py::A::test_3"] {
        let stats = history.stats(&id(name)).unwrap();
        assert_eq!((stats.last_outcome, stats.count), (Outcome::Passed, 1));
    }

    // The stopped run's sleeping test never finished: it did not run.
    let mut history = History::default();
    fold(&mut history, UNITTEST_STOPPED);
    assert!(history.last_failed().is_empty());
    assert_eq!(
        history.tests.keys().collect::<Vec<_>>(),
        [&id("tests/test_s.py::S::test_1")]
    );
}
