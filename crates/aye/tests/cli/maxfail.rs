//! Stopping early: `-x` and `--maxfail`, from the command line and from
//! pytest's configuration, and why a run cut short says it stopped.

use aye_test::{code, events, json_lines, project, stdout};

use crate::{aye, logged, python, summary};

#[test]
fn the_last_of_exitfirst_and_maxfail_wins() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "test_m.py",
        "import pytest\n\n@pytest.mark.parametrize('i', range(8))\ndef test_t(i):\n    assert i % 2 == 0\n",
    )]);
    let failed = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "-n", "1"];
        argv.extend(args);
        let (_output, output_log) = logged(&python, dir.path(), &argv);
        let records = output_log;
        let run = events(&records, "run_finished")[0].clone();
        (run["counts"]["failed"].as_u64().unwrap(), run)
    };
    assert_eq!(failed(&["-x", "--maxfail", "3"]).0, 3);
    assert_eq!(failed(&["--maxfail", "3", "-x"]).0, 1);
    // The arguments after `--` come last, as in pytest's `--maxfail 3 -x`.
    assert_eq!(failed(&["--maxfail", "3", "--", "-x"]).0, 1);
    assert_eq!(failed(&["-x", "--", "--maxfail", "3"]).0, 3);
    assert_eq!(failed(&["-x", "--", "--maxfail=0"]).0, 4);
    // 0 is no limit, as in pytest.
    let (count, run) = failed(&["--maxfail", "0"]);
    assert_eq!(count, 4);
    assert!(run.get("stopped").is_none(), "{run}");
}

#[test]
fn a_run_cut_short_says_why() {
    let Some(python) = python() else { return };
    let mut files = vec![];
    let failing = "def test_f():\n    assert 0\n\ndef test_p():\n    pass\n";
    for name in ["tests/test_1.py", "tests/test_2.py", "tests/test_3.py"] {
        files.push((name, failing));
    }
    let dir = project(&files);
    let output = aye(&python, dir.path(), &["--color", "never", "-x"]);
    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    assert!(
        text.contains("\nstopped after 1 failure (-x/--maxfail)\n✗ 1 failed   "),
        "{text}"
    );
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "-x", "-n", "2"],
    );
    assert_eq!(events(&output_log, "run_finished")[0]["stopped"], "maxfail");
    let lines = aye(
        &python,
        dir.path(),
        &["--output", "json", "-v", "--maxfail", "2"],
    );
    assert!(
        json_lines(&lines).last().unwrap()["stopped"] == "maxfail",
        "{}",
        stdout(&lines)
    );
    // Nothing is said when every test ran.
    let (_all, all_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert!(events(&all_log, "run_finished")[0].get("stopped").is_none());

    let exits = project(&[(
        "tests/test_1.py",
        "import pytest\n\ndef test_a():\n    pass\n\ndef test_b():\n    pytest.exit('bye')\n\ndef test_c():\n    pass\n",
    )]);
    let output = aye(&python, exits.path(), &["--color", "never"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    assert!(
        summary(&output).starts_with("interrupted"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn maxfail_in_addopts_is_counted_across_workers() {
    let Some(python) = python() else { return };
    let failing = "def test_f():\n    assert 0\n\ndef test_p():\n    pass\n";
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"-x\"\n",
        ),
        ("tests/test_1.py", failing),
        ("tests/test_2.py", failing),
        ("tests/test_3.py", failing),
    ]);
    let run = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "-n", "3"];
        argv.extend(args);
        let records = logged(&python, dir.path(), &argv).1;
        events(&records, "run_finished")[0]["counts"]["failed"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(run(&[]), 1);
    // The command line has the last word, as it does in pytest.
    assert_eq!(run(&["--maxfail", "2"]), 2);
    assert_eq!(run(&["--maxfail", "0"]), 3);
}
