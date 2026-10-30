//! What the run store keeps (every run, numbered, with its history; the
//! newest, as `keep-runs` says; two at once), and which stored run `aye
//! show` reads.

use std::fs;

use aye_events::TestId;
use aye_test::{code, json_lines, project, stderr, stdout};
use serde_json::Value;

use crate::{
    FAIL, INNER, PARAM_2, command, count, failures, key, keys, last, python, run, show, show_json,
    suite, summary,
};

#[test]
fn every_run_is_kept_numbered_with_its_history() {
    let Some(python) = python() else { return };
    let dir = suite();
    let output = run(&python, dir.path(), &["--color", "never"]);
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    // The log is kept, gzip-compressed, beside its summary, as `r1`; `last`
    // names it.
    assert_eq!(last(dir.path()), "r1");
    let runs = dir.path().join(".aye/runs");
    assert!(runs.join("r1.jsonl.gz").is_file());
    assert!(runs.join("r1.json").is_file());
    // The JSON lines' failures, as they finish, at the levels with a line
    // each.
    for verbosity in ["-v", "-vv"] {
        let lines = json_lines(&run(&python, dir.path(), &["--output", "json", verbosity]));
        assert_eq!(failures(&lines), [FAIL, INNER, PARAM_2], "{verbosity}");
    }
    assert_eq!(last(dir.path()), "r3");

    // `--log`: the stored log, as stored, whatever the format: its
    // `run_started` says the run's number, its envelope the run's id.
    let log = show(dir.path(), &["--log"]);
    assert_eq!(code(&log), 0, "{}", stderr(&log));
    assert_eq!(
        stdout(&log),
        stdout(&show(dir.path(), &["--log", "--output", "text"]))
    );
    let records = json_lines(&log);
    assert_eq!(records[0]["event"], "run_started");
    assert_eq!(records[0]["run_number"], 3);
    assert_eq!(records[0]["run"].as_str().unwrap().len(), 16);
    assert_eq!(records.last().unwrap()["event"], "run_finished");
    assert_eq!(
        records.len(),
        aye_history::RunStore::new(&runs).log("r3").unwrap().len()
    );
    // Bad outcomes carry how the test ended before: failed since r1. The
    // others carry nothing.
    let history = |records: &[Value], id: &str| -> Value {
        records
            .iter()
            .find(|record| record["event"] == "test_finished" && record["id"] == id)
            .unwrap_or_else(|| panic!("{id}"))["history"]
            .clone()
    };
    assert_eq!(
        history(&records, FAIL),
        serde_json::json!({"outcomes": "ff", "since": 1})
    );
    assert_eq!(
        history(&records, INNER),
        serde_json::json!({"outcomes": "ee", "since": 1})
    );
    assert_eq!(history(&records, "tests/test_a.py::test_pass"), Value::Null);
    // The first run's: no history yet.
    let first = json_lines(&show(dir.path(), &["--log", "--run", "r1"]));
    assert_eq!(first[0]["run_number"], 1);
    assert_eq!(history(&first, FAIL), Value::Null);
    // History keeps each test's last outcomes, and when its bad streak
    // began.
    let saved: Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join(".aye/history.json")).unwrap())
            .unwrap();
    assert_eq!(saved["version"], 1);
    let test = |id: &str| &saved["tests"][id];
    assert_eq!(
        (&test(FAIL)["recent"], &test(FAIL)["bad_since"]),
        (&"fff".into(), &1.into())
    );
    assert_eq!(test("tests/test_a.py::test_pass")["recent"], "ppp");
    assert!(
        test("tests/test_a.py::test_pass")
            .get("bad_since")
            .is_none()
    );
    // A collect-only run takes no number.
    run(&python, dir.path(), &["--co"]);
    run(&python, dir.path(), &["-q"]);
    assert_eq!(last(dir.path()), "r4");
}

#[test]
fn show_picks_an_older_run_and_lists_the_runs() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    run(&python, dir.path(), &["tests/test_b.py"]);
    assert_eq!(last(dir.path()), "r2");
    // The last run is the default.
    let passed = |args: &[&str]| {
        let mut all = vec!["-q"];
        all.extend(args);
        count(summary(&show_json(dir.path(), &all)), "passed")
    };
    assert_eq!(passed(&[]), 2);
    // `--run` takes a run's number, with or without its `r`.
    assert_eq!(passed(&["--run", "r1"]), 4);
    assert_eq!(passed(&["--run", "1"]), 4);
    assert_eq!(passed(&["--run", "r2"]), 2);
    let fail = keys(dir.path(), "r1")
        .key(&TestId::new(FAIL))
        .unwrap()
        .to_string();
    assert_eq!(
        failures(&show_json(dir.path(), &["--run", "r1", &fail])),
        [FAIL]
    );
    // The runs, the last first.
    let runs = show_json(dir.path(), &["--runs"]);
    assert_eq!(runs.len(), 2);
    assert_eq!(
        (&runs[0]["type"], &runs[0]["id"], &runs[0]["last"]),
        (&"run".into(), &"r2".into(), &true.into())
    );
    assert_eq!(runs[0]["exit"], 1);
    assert_eq!(
        (&runs[0]["passed"], &runs[0]["failed"]),
        (&2.into(), &1.into())
    );
    assert_eq!(runs[1]["id"], "r1");
    assert!(runs[1].get("last").is_none());
    assert!(runs[1]["started"].as_str().unwrap().ends_with('Z'));
    let text = stdout(&show(dir.path(), &["--runs"]));
    let text: Vec<&str> = text.lines().collect();
    assert!(text[0].starts_with("r2  "), "{text:?}");
    assert!(text[0].contains(" UTC  exit 1  "), "{text:?}");
    assert!(text[0].ends_with("  (last)"), "{text:?}");
    assert!(text[1].starts_with("r1  "), "{text:?}");
    // A run stored before runs were numbered: by the start of its id.
    let store = dir.path().join(".aye/runs");
    let old = "fedcba9876543210";
    let mut meta: Value =
        serde_json::from_str(&fs::read_to_string(store.join("r1.json")).unwrap()).unwrap();
    meta["run"] = old.into();
    meta["finished_ms"] = 1.into();
    fs::write(store.join(format!("{old}.json")), meta.to_string()).unwrap();
    fs::copy(
        store.join("r1.jsonl.gz"),
        store.join(format!("{old}.jsonl.gz")),
    )
    .unwrap();
    assert_eq!(passed(&["--run", "fedc"]), 4);
    let runs = show_json(dir.path(), &["--runs"]);
    let ids: Vec<&str> = runs.iter().map(|run| run["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["r2", "r1", old]);
    // It counts for no number.
    run(&python, dir.path(), &["tests/test_b.py"]);
    assert_eq!(last(dir.path()), "r3");
    // A run that isn't stored: 4, with those that are.
    for wanted in ["r9", "9", "zzzz"] {
        let unknown = show(dir.path(), &["--run", wanted]);
        assert_eq!(code(&unknown), 4, "{wanted}");
        assert!(
            stderr(&unknown).contains(&format!("stored: r3, r2, r1, {old}")),
            "{}",
            stderr(&unknown)
        );
    }
}

#[test]
fn keeping_no_runs_keeps_none_and_show_says_so() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nkeep-runs = 0\n",
    )
    .unwrap();
    run(&python, dir.path(), &[]);
    let runs = dir.path().join(".aye/runs");
    assert!(
        fs::read_dir(&runs).unwrap().next().is_none(),
        "{:?}",
        fs::read_dir(&runs).unwrap().collect::<Vec<_>>()
    );
    let output = show(dir.path(), &[]);
    assert_eq!(code(&output), 5);
    assert!(
        stderr(&output).contains("keep-runs = 0"),
        "{}",
        stderr(&output)
    );
    // A bad value is a usage error, for runs and `aye show` alike.
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nkeep-runs = -1\n",
    )
    .unwrap();
    assert_eq!(code(&show(dir.path(), &[])), 4);
    assert_eq!(code(&run(&python, dir.path(), &[])), 4);
}

#[test]
fn the_newest_runs_are_kept() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", "def test_a():\n    pass\n"),
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nkeep-runs = 2\n",
        ),
    ]);
    let mut ids = Vec::new();
    for _ in 0..3 {
        assert_eq!(code(&run(&python, dir.path(), &[])), 0);
        ids.push(last(dir.path()));
    }
    assert_eq!(ids, ["r1", "r2", "r3"]);
    let runs = show_json(dir.path(), &["--runs"]);
    let kept: Vec<&str> = runs.iter().map(|run| run["id"].as_str().unwrap()).collect();
    assert_eq!(kept, ["r3", "r2"]);
    assert!(!dir.path().join(".aye/runs/r1.jsonl.gz").exists());
}

#[test]
fn two_runs_at_once_are_both_kept() {
    let Some(python) = python() else { return };
    let dir = suite();
    let python = python.to_str().unwrap();
    let spawn = |path: &str| {
        command(dir.path(), &["--python", python, "-q", path])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    };
    let mut a = spawn("tests/test_a.py");
    let mut b = spawn("tests/test_b.py");
    assert!(a.wait().is_ok() && b.wait().is_ok());
    let runs = show_json(dir.path(), &["--runs"]);
    assert_eq!(runs.len(), 2, "{runs:#?}");
    let mut totals: Vec<u64> = runs
        .iter()
        .map(|run| {
            ["passed", "failed", "error", "skipped"]
                .iter()
                .map(|count| run[count].as_u64().unwrap_or(0))
                .sum()
        })
        .collect();
    totals.sort_unstable();
    assert_eq!(totals, [3, 5]);
    // Each its own number; `last` names one of them, whole.
    let mut ids: Vec<&str> = runs.iter().map(|run| run["id"].as_str().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["r1", "r2"]);
    let last = last(dir.path());
    assert!(runs.iter().any(|run| run["id"] == last.as_str()));
}

/// A run with nothing to show (a path that doesn't exist, a `-k` that
/// matches nothing) isn't kept: `aye show` still shows the run before it,
/// and its number is free again.
#[test]
fn a_run_that_ran_nothing_leaves_the_last_run_alone() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    assert_eq!(last(dir.path()), "r1");
    let typo = run(&python, dir.path(), &["tests/typo.py"]);
    assert_eq!(code(&typo), 4);
    let nothing = run(&python, dir.path(), &["-k", "nothing_like_this"]);
    assert_eq!(code(&nothing), 5);
    assert_eq!(last(dir.path()), "r1");
    let fail = key(dir.path(), FAIL);
    assert_eq!(failures(&show_json(dir.path(), &[&fail])), [FAIL]);
    assert_eq!(show_json(dir.path(), &["--runs"]).len(), 1);
    // Nothing is left of them either.
    assert_eq!(
        fs::read_dir(dir.path().join(".aye/runs")).unwrap().count(),
        3
    );
    run(&python, dir.path(), &[]);
    assert_eq!(last(dir.path()), "r2");
}

/// When the run `last` names is gone, the newest stored run stands for it.
#[test]
fn the_newest_run_stands_for_a_last_run_that_is_gone() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    run(&python, dir.path(), &["tests/test_b.py"]);
    let second = last(dir.path());
    fs::remove_file(dir.path().join(format!(".aye/runs/{second}.json"))).unwrap();
    let lines = show_json(dir.path(), &["-q"]);
    assert_eq!(count(&lines[0], "passed"), 4);
    let runs = show_json(dir.path(), &["--runs"]);
    assert_eq!(runs.len(), 1);
    assert_eq!(
        (&runs[0]["id"], &runs[0]["last"]),
        (&"r1".into(), &true.into())
    );
}
