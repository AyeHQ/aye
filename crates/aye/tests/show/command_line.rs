//! `aye show` on the command line: its exit codes, its help, and where
//! `show` is read as the command rather than a path or a name.

use std::path::Path;

use aye_test::{code, project, stderr, stdout};

use crate::{FAIL, command, key, python, run, show, suite};

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
