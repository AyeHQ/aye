//! `aye show` end to end: every run is kept in `.aye/runs`, numbered, and
//! `aye show` lists a stored run, or shows the details of the tests named,
//! without running anything.
//!
//! These tests pin what `aye show` decides (which run, which results, at
//! which level, with which exit code) and what the store keeps, through
//! what stays put in the renderers' output: ids, JSON lines' `type`, `id`,
//! `lifetime`, `exit`, `passed` and `failed`, and `stale` lines. The
//! renderers' own wording is theirs to pin.
//!
//! Runs need an interpreter, the one aye-test finds; without one the tests
//! are skipped, loudly. `aye show` itself needs none, and is never given
//! one. Every run is in aye-test's hermetic environment.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aye_render::Keys;
use aye_test::{Needs, TestContext, code, json_lines, project, runs_dir, stderr, stdout};
use serde_json::Value;

/// The interpreter runs are given: any will do.
fn python() -> Option<PathBuf> {
    aye_test::test_python(Needs::Stdlib)
}

/// aye, as this test target builds it.
fn context() -> TestContext {
    TestContext::new(env!("CARGO_BIN_EXE_aye"))
}

/// aye with `args`, in `cwd`, in the hermetic environment.
fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut command = context().command(cwd);
    command.args(args);
    command
}

/// A run, with `--python`.
fn run(python: &Path, cwd: &Path, args: &[&str]) -> Output {
    context().run(python, cwd, args).output().unwrap()
}

/// `aye show`, without an interpreter.
fn show(cwd: &Path, args: &[&str]) -> Output {
    let mut all = vec!["show"];
    all.extend(args);
    command(cwd, &all).output().unwrap()
}

/// `aye show` with `args`, in JSON lines, which must exit 0.
fn show_json(cwd: &Path, args: &[&str]) -> Vec<Value> {
    let mut all = vec!["--output", "json"];
    all.extend(args);
    let output = show(cwd, &all);
    assert_eq!(code(&output), 0, "{args:?}: {}", stderr(&output));
    json_lines(&output)
}

/// The ids of the bad outcomes JSON lines detail: each `fail` line's test,
/// each `lifetime_fail` line's lifetime, in order.
fn failures(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .filter_map(|line| match line["type"].as_str()? {
            "fail" => line["id"].as_str(),
            "lifetime_fail" => line["lifetime"].as_str(),
            _ => None,
        })
        .map(str::to_owned)
        .collect()
}

/// The ids of the `test` lines (the list), in order.
fn listed(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line["type"] == "test")
        .map(|line| line["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The `summary` line, the last.
fn summary(lines: &[Value]) -> &Value {
    let last = lines.last().expect("no lines");
    assert_eq!(last["type"], "summary", "{lines:#?}");
    last
}

/// A summary's count, absent when 0.
fn count(summary: &Value, name: &str) -> u64 {
    summary[name].as_u64().unwrap_or(0)
}

const TEST_A: &str = "\
import pytest

@pytest.fixture
def broken():
    raise RuntimeError('no db')

def test_pass():
    pass

def test_fail():
    print('computing')
    assert 1 == 2

@pytest.mark.slow
def test_slow():
    pass

@pytest.mark.skip(reason='not now')
def test_skip():
    pass

class TestK:
    def test_inner(self, broken):
        pass
";

const TEST_B: &str = "\
import pytest

@pytest.mark.parametrize('x', [1, 2])
def test_param(x):
    assert x == 1

def test_ok():
    pass
";

const FAIL: &str = "tests/test_a.py::test_fail";
const INNER: &str = "tests/test_a.py::TestK::test_inner";
const PARAM_2: &str = "tests/test_b.py::test_param[2]";

/// Two files: in the order the run finishes them, `test_fail`,
/// `TestK::test_inner` and `test_param[2]` fail.
fn suite() -> tempfile::TempDir {
    project(&[
        ("tests/test_a.py", TEST_A),
        ("tests/test_b.py", TEST_B),
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\nmarkers = [\"slow\"]\n",
        ),
    ])
}

/// The full FNV-1a 64 key of a test id, 16 hex digits.
fn full_key(id: &str) -> String {
    aye_events::TestKey::of(id).to_string()
}

fn last(dir: &Path) -> String {
    fs::read_to_string(runs_dir(dir).join(aye_history::runs::LAST))
        .unwrap()
        .trim()
        .to_owned()
}

/// The keys of the run stored as `run`, as the run printed them.
fn keys(dir: &Path, run: &str) -> Keys {
    let records = aye_history::RunStore::new(runs_dir(dir)).log(run).unwrap();
    Keys::of(&records)
}

/// The key the last run gave `id`.
fn key(dir: &Path, id: &str) -> String {
    keys(dir, &last(dir)).key(id).unwrap().to_owned()
}

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
fn show_lists_the_last_run() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let ids = [
        "tests/test_a.py::test_pass",
        FAIL,
        "tests/test_a.py::test_slow",
        "tests/test_a.py::test_skip",
        INNER,
        "tests/test_b.py::test_param[1]",
        PARAM_2,
        "tests/test_b.py::test_ok",
    ];
    // Text: every result, in the order of the run.
    let output = show(dir.path(), &["--color", "never"]);
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    let places: Vec<usize> = ids
        .iter()
        .map(|id| out.find(id).unwrap_or_else(|| panic!("{id}: {out}")))
        .collect();
    assert!(places.is_sorted(), "{out}");
    // JSON: a `test` line each, then the run's summary.
    let lines = show_json(dir.path(), &[]);
    assert_eq!(listed(&lines), ids);
    let outcomes: Vec<&str> = lines
        .iter()
        .filter(|line| line["type"] == "test")
        .map(|line| line["outcome"].as_str().unwrap())
        .collect();
    assert_eq!(
        outcomes,
        [
            "passed", "failed", "passed", "skipped", "error", "passed", "failed", "passed"
        ]
    );
    let total = summary(&lines);
    assert_eq!(
        (
            &total["exit"],
            count(total, "passed"),
            count(total, "failed")
        ),
        (&1.into(), 4, 2)
    );
    // -q: the summary alone.
    let quiet = show_json(dir.path(), &["-q"]);
    assert_eq!(quiet.len(), 1, "{quiet:#?}");
    summary(&quiet);
    // Nothing ran: no worker, no history change, the same stored run.
    assert_eq!(
        fs::read_dir(dir.path().join(".aye/runs")).unwrap().count(),
        3
    );
}

#[test]
fn show_filters_the_list_and_failed_details_every_failure() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let ids = |args: &[&str]| listed(&show_json(dir.path(), args));
    assert_eq!(ids(&["--outcome", "error"]), [INNER]);
    assert_eq!(
        ids(&["--outcome", "skipped,error"]),
        ["tests/test_a.py::test_skip", INNER]
    );
    assert_eq!(
        ids(&["--outcome", "skipped", "--outcome", "error"]),
        ["tests/test_a.py::test_skip", INNER]
    );
    assert_eq!(
        ids(&["-k", "inner or ok"]),
        [INNER, "tests/test_b.py::test_ok"]
    );
    assert_eq!(ids(&["-m", "slow"]), ["tests/test_a.py::test_slow"]);
    assert_eq!(
        ids(&["tests/test_b.py"]),
        [
            "tests/test_b.py::test_param[1]",
            PARAM_2,
            "tests/test_b.py::test_ok"
        ]
    );
    // A filter keeping nothing shows nothing, and isn't an error.
    assert_eq!(ids(&["-k", "nothing_like_this"]), [] as [String; 0]);
    // `--failed`: the details of every failure, and a summary counting
    // them; with a path, of its failures.
    let lines = show_json(dir.path(), &["--failed"]);
    assert_eq!(failures(&lines), [FAIL, INNER, PARAM_2]);
    let total = summary(&lines);
    assert_eq!((count(total, "failed"), count(total, "passed")), (2, 0));
    assert_eq!(
        failures(&show_json(dir.path(), &["tests/test_b.py", "--failed"])),
        [PARAM_2]
    );
    // The same as naming them.
    let named = [FAIL, INNER, PARAM_2].map(|id| key(dir.path(), id));
    let by_keys: Vec<&str> = named.iter().map(String::as_str).collect();
    assert_eq!(show_json(dir.path(), &by_keys), lines);
    // From a subdirectory, paths are read as a run reads them.
    let sub = show(
        &dir.path().join("tests"),
        &["test_b.py", "--output", "json", "-q"],
    );
    assert_eq!(count(summary(&json_lines(&sub)), "passed"), 2);
}

#[test]
fn show_gives_the_details_of_tests_by_key_id_dotted_name_or_path() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let keys = keys(dir.path(), "r1");
    let fail = keys.key(FAIL).unwrap().to_owned();
    assert_eq!(fail.len(), 5);
    assert!(full_key(FAIL).starts_with(&fail));
    // By its key, as the run printed it, its whole hash, its id or its
    // dotted name: its details, and a summary counting it.
    let mut selectors = vec![
        fail.clone(),
        fail.to_ascii_uppercase(),
        full_key(FAIL),
        FAIL.to_owned(),
        "tests.test_a.test_fail".to_owned(),
    ];
    // The start of its key, from 4 digits, when it names it alone.
    if let Some(start) = (4..5)
        .map(|len| full_key(FAIL)[..len].to_owned())
        .find(|start| keys.resolve(start).len() == 1)
    {
        selectors.push(start);
    }
    let first = show_json(dir.path(), &[&fail]);
    assert_eq!(failures(&first), [FAIL]);
    let total = summary(&first);
    assert_eq!((&total["exit"], count(total, "failed")), (&1.into(), 1));
    assert!(
        first[0]["rerun"].as_str().unwrap().ends_with(FAIL),
        "{first:#?}"
    );
    for selector in &selectors {
        assert_eq!(show_json(dir.path(), &[selector]), first, "{selector}");
    }
    // Text: each test named, a failure or not.
    let inner = keys.key(INNER).unwrap();
    let output = show(
        dir.path(),
        &[inner, "tests.test_b.test_ok", "--color", "never"],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    for id in [INNER, "tests/test_b.py::test_ok"] {
        assert!(out.contains(id), "{id}: {out}");
    }
    // A path beside a key selects its tests.
    let lines = show_json(dir.path(), &[&fail, "tests/test_b.py"]);
    assert_eq!(failures(&lines), [FAIL, PARAM_2]);
    let total = summary(&lines);
    assert_eq!((count(total, "failed"), count(total, "passed")), (2, 2));
}

/// Details come at the ladder's second level without `-v` or `-q`: text's
/// `-v`, JSON's `-vv`; the flags mean what they mean in a run.
#[test]
fn show_details_at_each_level() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let fail = key(dir.path(), FAIL);
    let text = |args: &[&str]| {
        let mut all = vec![fail.as_str(), "--color", "never"];
        all.extend(args);
        stdout(&show(dir.path(), &all))
    };
    assert_eq!(text(&[]), text(&["-v"]));
    assert_eq!(text(&[]), text(&["-vv", "-q"]));
    assert!(text(&["-vv"]).contains("computing"), "{}", text(&["-vv"]));
    let lines = |args: &[&str]| {
        let mut all = vec![fail.as_str()];
        all.extend(args);
        show_json(dir.path(), &all)
    };
    assert_eq!(lines(&[]), lines(&["-vv"]));
    assert_ne!(lines(&[]), lines(&["-v"]));
    // -q: the summary alone, in either format.
    assert_eq!(lines(&["-q"]).len(), 1);
    summary(&lines(&["-q"]));
    assert!(!text(&["-q"]).contains(FAIL), "{}", text(&["-q"]));
    // `--failed` too.
    assert_eq!(
        show_json(dir.path(), &["--failed"]),
        show_json(dir.path(), &["--failed", "-vv"])
    );
    // The list stays at a run's default.
    assert_eq!(
        show_json(dir.path(), &[]),
        show_json(dir.path(), &["-v", "-q"])
    );
    assert_eq!(
        stdout(&show(dir.path(), &["--color", "never"])),
        stdout(&show(dir.path(), &["-v", "-q", "--color", "never"]))
    );
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
    let fail = keys(dir.path(), "r1").key(FAIL).unwrap().to_owned();
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
fn show_says_when_a_file_changed_since_the_run() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let fail = key(dir.path(), FAIL);
    // Unchanged: nothing said.
    let lines = show_json(dir.path(), &[&fail]);
    assert!(lines.iter().all(|line| line["type"] != "stale"));
    fs::write(
        dir.path().join("tests/test_a.py"),
        TEST_A.replace("1 == 2", "1 == 1"),
    )
    .unwrap();
    let lines = show_json(dir.path(), &[&fail]);
    assert_eq!(
        lines[0],
        serde_json::json!({"type": "stale", "path": "tests/test_a.py", "rerun": "aye tests/test_a.py"})
    );
    assert_eq!(failures(&lines), [FAIL]);
    let out = stdout(&show(dir.path(), &[&fail, "--color", "never"]));
    assert!(
        out.starts_with("tests/test_a.py changed since this run; rerun: aye tests/test_a.py\n"),
        "{out}"
    );
    // Only for the files of the tests shown.
    let lines = show_json(dir.path(), &["tests/test_b.py"]);
    assert!(lines.iter().all(|line| line["type"] != "stale"));
    // A file that is gone.
    fs::remove_file(dir.path().join("tests/test_b.py")).unwrap();
    let lines = show_json(dir.path(), &["--failed"]);
    let stale: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "stale")
        .collect();
    assert_eq!(stale.len(), 2, "{lines:#?}");
    assert_eq!(
        *stale[1],
        serde_json::json!({"type": "stale", "path": "tests/test_b.py", "gone": true})
    );
    let param = key(dir.path(), PARAM_2);
    let out = stdout(&show(dir.path(), &[&param, "--color", "never"]));
    assert!(
        out.starts_with("tests/test_b.py is gone since this run\n"),
        "{out}"
    );
}

#[test]
fn show_exit_codes() {
    let Some(python) = python() else { return };
    let dir = suite();
    // No run stored yet: 5.
    let none = show(dir.path(), &[]);
    assert_eq!(code(&none), 5);
    assert!(
        stderr(&none).contains("no run is stored"),
        "{}",
        stderr(&none)
    );
    assert_eq!(code(&show(dir.path(), &["--runs"])), 5);
    assert_eq!(code(&show(dir.path(), &["--log"])), 5);
    // A collect-only run isn't stored.
    run(&python, dir.path(), &["--co"]);
    assert_eq!(code(&show(dir.path(), &[])), 5);
    run(&python, dir.path(), &[]);
    // Whatever the outcomes, what could be shown exits 0.
    let fail = key(dir.path(), FAIL);
    assert_eq!(code(&show(dir.path(), &[&fail])), 0);
    assert_eq!(code(&show(dir.path(), &["--failed"])), 0);
    assert_eq!(code(&show(dir.path(), &["--log"])), 0);
    // A selector naming no test of the run: 4, with the closest ids.
    let bad = show(dir.path(), &["tests/test_a.py::test_fial"]);
    assert_eq!(code(&bad), 4);
    let err = stderr(&bad);
    assert!(
        err.contains("tests/test_a.py::test_fial: no test of run r1")
            && err.contains("the closest:\n  tests/test_a.py::test_fail\n"),
        "{err}"
    );
    assert_eq!(stdout(&bad), "");
    // Digits are no failure number: a name like any other.
    for name in ["9", "tests/test_c.py", "no_such_name"] {
        let bad = show(dir.path(), &[name]);
        assert_eq!(code(&bad), 4, "{name}");
        assert!(
            stderr(&bad).contains(&format!("{name}: no test of run r1 has that name")),
            "{}",
            stderr(&bad)
        );
    }
    // Bad options and expressions: 4.
    assert_eq!(code(&show(dir.path(), &["-k", "a and"])), 4);
    assert_eq!(code(&show(dir.path(), &["--outcome", "won"])), 4);
    assert_eq!(code(&show(dir.path(), &["--bogus"])), 4);
    assert_eq!(code(&show(dir.path(), &["--runs", &fail])), 4);
    assert_eq!(code(&show(dir.path(), &["--log", &fail])), 4);
    assert_eq!(code(&show(dir.path(), &["--log", "--failed"])), 4);
    assert_eq!(code(&show(dir.path(), &["--log", "--runs"])), 4);
    // Help.
    let help = show(dir.path(), &["--help"]);
    assert_eq!(code(&help), 0);
    for word in ["3fae6", "--log", "r81"] {
        assert!(stdout(&help).contains(word), "{word}: {}", stdout(&help));
    }
}

/// Two test names whose ids, in `file`, have keys that start with the
/// same 4 digits.
fn alike(file: &str) -> (String, String) {
    let mut seen = std::collections::HashMap::new();
    (0..100_000)
        .find_map(|n| {
            let name = format!("test_{n}");
            let start = full_key(&format!("{file}::{name}"))[..4].to_owned();
            seen.insert(start, name.clone())
                .map(|earlier| (earlier, name))
        })
        .unwrap()
}

/// The start of two tests' keys names neither: 4, with both keys and ids.
#[test]
fn the_start_of_several_keys_lists_them() {
    let Some(python) = python() else { return };
    let (a, b) = alike("tests/test_k.py");
    let dir = project(&[(
        "tests/test_k.py",
        &format!("def {a}():\n    assert 0\n\ndef {b}():\n    pass\n"),
    )]);
    assert_eq!(code(&run(&python, dir.path(), &[])), 1);
    let (a, b) = (
        format!("tests/test_k.py::{a}"),
        format!("tests/test_k.py::{b}"),
    );
    let start = &full_key(&a)[..4];
    let output = show(dir.path(), &[start]);
    assert_eq!(code(&output), 4);
    let err = stderr(&output);
    assert!(
        err.starts_with(&format!(
            "aye: error: {start}: the start of several keys; give more of the one you want:\n"
        )),
        "{err}"
    );
    let keys = keys(dir.path(), "r1");
    for id in [&a, &b] {
        let key = keys.key(id).unwrap();
        assert!(
            err.lines()
                .any(|line| line.split_whitespace().eq([key, id.as_str()])),
            "{err}"
        );
        // Each key names its test.
        assert_eq!(code(&show(dir.path(), &[key])), 0);
    }
    assert_eq!(stdout(&output), "");
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

#[test]
fn a_path_named_show_is_run_as_a_path() {
    let Some(python) = python() else { return };
    let dir = project(&[("show/test_s.py", "def test_s():\n    pass\n")]);
    let output = command(dir.path(), &["show", "--python", python.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    assert!(stdout(&output).contains("1 passed"), "{}", stdout(&output));
}

#[test]
fn the_help_names_aye_show() {
    let output = command(Path::new(env!("CARGO_MANIFEST_DIR")), &["--help"])
        .output()
        .unwrap();
    assert!(stdout(&output).contains("aye show"), "{}", stdout(&output));
}

#[test]
fn show_after_an_option_is_read_as_a_name_and_says_where_show_goes() {
    let Some(python) = python() else { return };
    let dir = suite();
    // `aye --python X show`: `show` is a positional argument there.
    let output = run(&python, dir.path(), &["--color", "never", "show"]);
    assert_eq!(code(&output), 4);
    assert!(
        stdout(&output).contains("not found: show (for `aye show`, put `show` first"),
        "{}",
        stdout(&output)
    );
}

/// A unittest project: `files` beside a `tests` package, run by the
/// unittest adapter alone.
fn unittest_project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let mut all = vec![
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
        ),
        ("tests/__init__.py", ""),
    ];
    all.extend_from_slice(files);
    project(&all)
}

const TEARDOWN: &str = "\
import unittest

class T(unittest.TestCase):
    @classmethod
    def tearDownClass(cls):
        raise RuntimeError('teardown boom')

    def test_one(self):
        pass

    def test_two(self):
        pass
";

const MODULE_SKIP: &str = "\
import pytest
pytest.skip('whole module skipped', allow_module_level=True)

def test_x():
    pass
";

const FAILING: &str = "\
def test_a():
    assert 1 == 2

def test_b():
    pass
";

const BROKEN: &str = "def test_bad(:\n    pass\n";

/// Without a filter, `aye show` gives the run's own summary: what it
/// counts beside the tests too (a module skipped while collecting, a
/// collection error, a failed teardown that blocked no test), in text and
/// JSON, and so does `--runs`.
#[test]
fn the_whole_list_ends_with_the_runs_own_summary() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_modskip.py", MODULE_SKIP),
        ("tests/test_f.py", FAILING),
        ("tests/test_broken.py", BROKEN),
    ]);
    let args = ["--continue-on-collection-errors", "--color", "never"];
    let ran = run(&python, dir.path(), &[&args[..], &["-q"]].concat());
    assert_eq!(code(&ran), 1, "{}", stderr(&ran));
    // -q: the same lines, the error diagnostic and the summary.
    assert_eq!(
        stdout(&show(dir.path(), &["-q", "--color", "never"])),
        stdout(&ran)
    );
    let ran = run(
        &python,
        dir.path(),
        &[&args[..], &["-q", "--output", "json"]].concat(),
    );
    // The same summary, but for `tests`, which only the run's collection
    // says.
    let mut ran_summary = json_lines(&ran)[0].clone();
    ran_summary.as_object_mut().unwrap().remove("tests");
    assert_eq!(
        json_lines(&show(dir.path(), &["-q", "--output", "json"])),
        [ran_summary]
    );
    assert_eq!(count(summary(&json_lines(&ran)), "skipped"), 1);
    // The list: the collection error as the run shows it, a line for the
    // module skipped, the tests, then the summary.
    let error = "error[syntax-error] tests/test_broken.py:1: syntax error at 1:14: Expected a parameter or the end of the parameter list\n";
    let list = stdout(&show(dir.path(), &["--color", "never"]));
    assert!(list.starts_with(error), "{list}");
    for id in [
        "tests/test_modskip.py",
        "tests/test_f.py::test_a",
        "tests/test_f.py::test_b",
    ] {
        assert!(list.contains(id), "{id}: {list}");
    }
    let lines = show_json(dir.path(), &[]);
    assert_eq!(lines[0]["type"], "collection_error");
    assert_eq!(
        (&lines[1]["type"], &lines[1]["id"], &lines[1]["outcome"]),
        (
            &"test".into(),
            &"tests/test_modskip.py".into(),
            &"skipped".into()
        )
    );
    assert_eq!(
        listed(&lines),
        [
            "tests/test_modskip.py",
            "tests/test_f.py::test_a",
            "tests/test_f.py::test_b"
        ]
    );
    // `--outcome skipped` keeps the module, and counts it.
    let skipped = show_json(dir.path(), &["--outcome", "skipped"]);
    assert_eq!(listed(&skipped), ["tests/test_modskip.py"]);
    assert_eq!(count(summary(&skipped), "skipped"), 1);
    let runs = show_json(dir.path(), &["--runs"]);
    assert_eq!(
        (&runs[0]["skipped"], &runs[0]["collection_error"]),
        (&1.into(), &1.into())
    );

    // A failed teardown that kept no test from running.
    let dir = unittest_project(&[("tests/test_td.py", TEARDOWN)]);
    let ran = run(&python, dir.path(), &["-q", "--color", "never"]);
    assert_eq!(code(&ran), 1, "{}", stderr(&ran));
    assert_eq!(
        stdout(&show(dir.path(), &["-q", "--color", "never"])),
        stdout(&ran)
    );
    let lines = show_json(dir.path(), &["-q"]);
    assert_eq!(
        (&lines[0]["exit"], count(&lines[0], "passed")),
        (&1.into(), 2)
    );
    let runs = show_json(dir.path(), &["--runs"]);
    assert_eq!(runs[0]["teardown_error"], 1);
}

/// What failed beside the tests (a failed lifetime that blocked no test, a
/// collection error) is listed, `--failed` details it, and so does its
/// name or key.
#[test]
fn what_failed_beside_the_tests_is_listed_and_shown() {
    let Some(python) = python() else { return };
    let dir = unittest_project(&[("tests/test_td.py", TEARDOWN)]);
    run(&python, dir.path(), &[]);
    let class = "tests/test_td.py::T";
    let list = stdout(&show(dir.path(), &["--color", "never"]));
    for id in [
        "tests/test_td.py::T::test_one",
        "tests/test_td.py::T::test_two",
        class,
    ] {
        assert!(list.contains(id), "{id}: {list}");
    }
    let failed = show(dir.path(), &["--failed", "--color", "never"]);
    assert_eq!(code(&failed), 0);
    assert!(stdout(&failed).contains(class), "{}", stdout(&failed));
    assert!(!stdout(&failed).contains("test_one"), "{}", stdout(&failed));
    let lines = show_json(dir.path(), &["--failed"]);
    assert_eq!(failures(&lines), [class]);
    let teardown = &lines[0];
    assert_eq!(
        (&teardown["type"], &teardown["phase"], &teardown["at"]),
        (
            &"lifetime_fail".into(),
            &"teardown".into(),
            &"tests/test_td.py:6".into()
        )
    );
    assert_eq!(summary(&lines)["exit"], 1);
    // The class's id: its tests, and its teardown's failure.
    let shown = show_json(dir.path(), &[class]);
    assert_eq!(failures(&shown), [class]);
    assert_eq!(count(summary(&shown), "passed"), 2);
    let text = stdout(&show(dir.path(), &[class, "--color", "never"]));
    assert!(text.contains("teardown boom"), "{text}");
    // The lifetime's key: the failure alone.
    let lifetime = key(dir.path(), class);
    let shown = show_json(dir.path(), &[&lifetime]);
    assert_eq!(failures(&shown), [class]);
    assert_eq!(count(summary(&shown), "passed"), 0);

    // A collection error that stopped the run: shown, and by its file.
    let dir = project(&[
        ("tests/test_ok.py", "def test_ok():\n    pass\n"),
        ("tests/test_broken.py", BROKEN),
    ]);
    assert_eq!(code(&run(&python, dir.path(), &[])), 2);
    let error = "error[syntax-error] tests/test_broken.py:1: syntax error at 1:14: Expected a parameter or the end of the parameter list\n";
    let shown = stdout(&show(dir.path(), &["--color", "never"]));
    assert!(shown.starts_with(error), "{shown}");
    let by_file = show(dir.path(), &["tests/test_broken.py", "--color", "never"]);
    assert_eq!(code(&by_file), 0, "{}", stderr(&by_file));
    assert!(stdout(&by_file).starts_with(error), "{}", stdout(&by_file));
    let lines = show_json(dir.path(), &["--failed"]);
    assert_eq!(lines[0]["type"], "collection_error");
    assert_eq!(summary(&lines)["exit"], 2);
    // Fixed since: said first.
    fs::write(
        dir.path().join("tests/test_broken.py"),
        "def test_bad():\n    pass\n",
    )
    .unwrap();
    let lines = show_json(dir.path(), &[]);
    assert_eq!(
        lines[0],
        serde_json::json!({"type": "stale", "path": "tests/test_broken.py", "rerun": "aye tests/test_broken.py"})
    );
}

/// A test a failed `setUpClass` kept from running: its key shows it with
/// the failure that kept it; the class's key, the failure.
#[test]
fn a_blocked_test_is_shown_with_the_setup_that_blocked_it() {
    let Some(python) = python() else { return };
    let dir = unittest_project(&[(
        "tests/test_setup.py",
        "\
import unittest

class Broken(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise RuntimeError('no db')

    def test_a(self):
        pass

    def test_b(self):
        pass

class Fine(unittest.TestCase):
    def test_c(self):
        self.fail('c')
",
    )]);
    assert_eq!(code(&run(&python, dir.path(), &[])), 1);
    let class = "tests/test_setup.py::Broken";
    let blocked = key(dir.path(), "tests/test_setup.py::Broken::test_b");
    let lines = show_json(dir.path(), &[&blocked]);
    assert!(failures(&lines).contains(&class.to_owned()), "{lines:#?}");
    assert!(
        !failures(&lines).contains(&"tests/test_setup.py::Fine::test_c".to_owned()),
        "{lines:#?}"
    );
    let text = stdout(&show(dir.path(), &[&blocked, "--color", "never"]));
    assert!(text.contains("test_b"), "{text}");
    assert!(text.contains("no db"), "{text}");
    let lines = show_json(dir.path(), &[&key(dir.path(), class)]);
    assert_eq!(failures(&lines)[0], class, "{lines:#?}");
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

/// A key names every result of its test: a test id that finished twice
/// has both.
#[test]
fn a_key_shows_every_result_of_a_duplicate_id() {
    let Some(python) = python() else { return };
    let dir = unittest_project(&[(
        "tests/test_twice.py",
        "\
import unittest

class T(unittest.TestCase):
    def test_t(self):
        self.fail('boom')

def load_tests(loader, tests, pattern):
    tests.addTests(loader.loadTestsFromTestCase(T))
    return tests
",
    )]);
    let ran = run(&python, dir.path(), &["--color", "never"]);
    assert_eq!(code(&ran), 1, "{}", stderr(&ran));
    let id = "tests/test_twice.py::T::test_t";
    let twice = key(dir.path(), id);
    let lines = show_json(dir.path(), &[&twice]);
    assert_eq!(failures(&lines), [id, id], "{lines:#?}");
    assert_eq!(count(summary(&lines), "failed"), 2);
    // Its id: the same.
    assert_eq!(show_json(dir.path(), &[id]), lines);
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

/// The summary's `exit` is the stored run's, whatever is shown of it.
#[test]
fn the_summarys_exit_is_the_runs() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let lines = show_json(dir.path(), &["tests/test_b.py::test_ok"]);
    let total = summary(&lines);
    assert_eq!((&total["exit"], count(total, "passed")), (&1.into(), 1));
    // Nothing failed: `--failed` shows nothing, and isn't an error.
    let dir = project(&[("tests/test_a.py", "def test_a():\n    pass\n")]);
    run(&python, dir.path(), &[]);
    let lines = show_json(dir.path(), &["--failed"]);
    assert_eq!(lines.len(), 1, "{lines:#?}");
    let total = summary(&lines);
    assert_eq!((&total["exit"], count(total, "passed")), (&0.into(), 0));
    assert_eq!(code(&show(dir.path(), &["--failed"])), 0);
}

#[test]
fn show_help_and_usage_name_aye_show() {
    let dir = project(&[]);
    let help = show(dir.path(), &["--help"]);
    assert_eq!(code(&help), 0);
    let out = stdout(&help);
    assert!(
        out.starts_with("Show a stored run's results again, without running anything.\n"),
        "{out}"
    );
    assert!(
        out.contains("\nUsage: aye show [OPTIONS] [TEST]...\n"),
        "{out}"
    );
    let short = stdout(&show(dir.path(), &["-h"]));
    assert!(
        short.starts_with("Show a stored run's results again, without running anything\n"),
        "{short}"
    );
    for args in [&["--bogus"][..], &["--runs", "1"]] {
        let output = show(dir.path(), args);
        assert_eq!(code(&output), 4);
        assert!(
            stderr(&output).contains("Usage: aye show "),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}
