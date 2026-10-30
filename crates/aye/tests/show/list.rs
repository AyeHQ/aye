//! The list of a stored run: its tests, what its filters keep, what failed
//! beside the tests, and the run's own summary at its end.

use std::fs;

use aye_test::{code, json_lines, project, stderr, stdout};
use serde_json::Value;

use crate::{
    FAIL, INNER, PARAM_2, count, failures, key, python, run, show, show_json, suite, summary,
    unittest_project,
};

/// The ids of the `test` lines (the list), in order.
fn listed(lines: &[Value]) -> Vec<String> {
    lines
        .iter()
        .filter(|line| line["type"] == "test")
        .map(|line| line["id"].as_str().unwrap().to_owned())
        .collect()
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
