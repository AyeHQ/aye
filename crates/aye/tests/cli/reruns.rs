//! The rerun each bad outcome prints, which works from wherever aye ran.

use std::process::Output;

use aye_test::{code, events, project, stdout};
use serde_json::Value;

use crate::{aye, finished, logged, python};

#[test]
fn printed_reruns_work_from_a_subdirectory() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", "def test_fail():\n    assert 0\n")]);
    let tests = dir.path().join("tests");
    let first = aye(&python, &tests, &["--color", "never", "-v", "test_a.py"]);
    assert_eq!(code(&first), 1);
    let rerun = stdout(&first)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .map(str::to_owned)
        .unwrap();
    assert_eq!(rerun, "tests/test_a.py::test_fail");
    let (again, again_log) = logged(&python, &tests, &["--output", "json", "-vvv", &rerun]);
    assert_eq!(code(&again), 1, "{}", stdout(&again));
    assert_eq!(
        finished(&again_log),
        [("tests/test_a.py::test_fail".to_owned(), "failed".to_owned())]
    );
    // The current directory still comes first.
    let (_local, local_log) = logged(
        &python,
        &tests,
        &["--output", "json", "-vvv", "test_a.py::test_fail"],
    );
    assert_eq!(finished(&local_log).len(), 1);
}

#[test]
fn a_test_failed_only_by_its_subtests_keeps_its_output_and_rerun() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_s.py",
        "def test_sub(subtests):\n    print('diagnostic output here')\n    for i in range(3):\n        with subtests.test(msg='case', i=i):\n            assert i != 1\n",
    )]);
    // As pytest shows the test's captured stdout after the subtest's
    // section: at L3, everything.
    let output = aye(&python, dir.path(), &["--color", "never", "-vv"]);
    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    let key = aye_events::TestKey::of("tests/test_s.py::test_sub").to_string()[..5].to_owned();
    assert!(
        text.contains(&format!(
            "✗ {key}  tests/test_s.py::test_sub › case (i=1)\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "✗ {key}  tests/test_s.py::test_sub (contains 1 failed subtest)\n  stdout   diagnostic output here\n  rerun    aye tests/test_s.py::test_sub\n"
        )),
        "{text}"
    );
    let lines = aye(&python, dir.path(), &["--output", "json", "-vvv"]);
    let fails: Vec<Value> = stdout(&lines)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["type"] == "fail")
        .collect();
    assert_eq!(fails.len(), 1, "{}", stdout(&lines));
    assert_eq!(fails[0]["sub"], "case (i=1)");
    assert_eq!(fails[0]["output"]["stdout"], "diagnostic output here");
    assert_eq!(fails[0]["rerun"], "aye tests/test_s.py::test_sub");
}

/// Round 3's #81: from `root/tests`, holding a `tests/test_a.py` of its
/// own, the printed rerun `aye tests/test_a.py::test_fail` ran that other
/// file. It names the test from where aye ran.
#[test]
fn a_printed_rerun_names_the_test_from_where_aye_ran() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", "def test_fail():\n    assert 0\n"),
        ("tests/tests/test_a.py", "def test_fail():\n    pass\n"),
    ]);
    let tests = dir.path().join("tests");
    let first = aye(&python, &tests, &["--color", "never", "-v", "test_a.py"]);
    assert_eq!(code(&first), 1, "{}", stdout(&first));
    let rerun = stdout(&first)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .map(str::to_owned)
        .unwrap();
    assert_eq!(rerun, "test_a.py::test_fail");
    let (again, again_log) = logged(&python, &tests, &["--output", "json", "-vvv", &rerun]);
    assert_eq!(code(&again), 1, "{}", stdout(&again));
    assert_eq!(
        finished(&again_log),
        [("tests/test_a.py::test_fail".to_owned(), "failed".to_owned())]
    );
    // The stream says so too, for every renderer.
    let (_json, json_log) = logged(&python, &tests, &["--output", "json", "-vvv", "test_a.py"]);
    let records = json_log;
    let result = events(&records, "test_finished")[0];
    assert_eq!(result["reproduce"]["command"][1], "test_a.py::test_fail");
    // From the root the rerun is the id.
    let root = aye(
        &python,
        dir.path(),
        &["--color", "never", "-v", "tests/test_a.py"],
    );
    assert!(
        stdout(&root).contains("rerun    aye tests/test_a.py::test_fail"),
        "{}",
        stdout(&root)
    );
}

/// The `rerun` row of text output at `-v`, split into arguments (none of
/// the tests' have spaces).
fn printed_rerun(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .unwrap_or_else(|| panic!("no rerun in:\n{}", stdout(output)))
        .split(' ')
        .map(str::to_owned)
        .collect()
}

/// Finding C26: aye's own `-c FILE` shapes the run (here, which tests
/// exist), so a printed rerun repeats it, as given, from where aye ran.
#[test]
fn a_printed_rerun_keeps_the_config_file() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("alt.ini", "[pytest]\npython_functions = check_*\n"),
        ("tests/test_a.py", "def check_a():\n    assert 0\n"),
    ]);
    for (cwd, flag, file) in [
        (dir.path().to_path_buf(), "-c", "alt.ini"),
        (dir.path().to_path_buf(), "--config-file", "alt.ini"),
        (dir.path().join("tests"), "-c", "../alt.ini"),
    ] {
        let first = aye(&python, &cwd, &["--color", "never", "-v", flag, file]);
        assert_eq!(code(&first), 1, "{}", stdout(&first));
        let rerun = printed_rerun(&first);
        assert_eq!(rerun, ["-c", file, "tests/test_a.py::check_a"]);
        let args: Vec<&str> = rerun.iter().map(String::as_str).collect();
        let (again, again_log) = logged(
            &python,
            &cwd,
            &[&["--output", "json", "-vvv"], &args[..]].concat(),
        );
        assert_eq!(code(&again), 1, "{}", stdout(&again));
        assert_eq!(
            finished(&again_log),
            [("tests/test_a.py::check_a".to_owned(), "failed".to_owned())]
        );
    }
    // With arguments after `--` too: aye's options, the id, then those.
    let (_json, json_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "-c", "alt.ini", "--", "-v"],
    );
    let records = json_log;
    assert_eq!(
        events(&records, "test_finished")[0]["reproduce"]["command"],
        serde_json::json!([
            "aye",
            "-c",
            "alt.ini",
            "tests/test_a.py::check_a",
            "--",
            "-v"
        ])
    );
}
