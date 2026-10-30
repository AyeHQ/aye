//! Collection's errors: pytest's internal and usage errors met while
//! listing, a directory that can't be read, conftests that fail to load,
//! and how each fails a run.

#[cfg(unix)]
use std::fs;

#[cfg(unix)]
use aye_test::events;
use aye_test::{code, project, stdout};

use crate::{
    DOCTEST, DOCTESTS_ON, PASSING, PYTEST_CONFIG, SKIPS, aye, diagnosed, finished, logged, python,
    summary,
};

/// A hook that fails while pytest collects is pytest's `INTERNALERROR`
/// (status 3), which aye's pytest front-end meets while it lists the
/// files, in collection: the run exits 3 before anything runs, `--co` and
/// `--continue-on-collection-errors` too.
#[test]
fn internal_and_usage_errors_from_pytest_keep_their_exit_codes() {
    let Some(python) = python() else { return };
    let broken = project(&[
        ("tests/test_a.py", PASSING),
        (
            "tests/conftest.py",
            "def pytest_collection_modifyitems(items):\n    raise RuntimeError('hook boom')\n",
        ),
    ]);
    for args in [&["--color", "never"][..], &["--co", "--color", "never"]] {
        let output = aye(&python, broken.path(), args);
        assert_eq!(code(&output), 3, "{args:?}: {}", stdout(&output));
        assert!(
            stdout(&output).contains("internal error"),
            "{}",
            stdout(&output)
        );
        assert!(
            !stdout(&output).contains("collection error"),
            "{}",
            stdout(&output)
        );
    }
    let (output, output_log) = logged(
        &python,
        broken.path(),
        &[
            "--continue-on-collection-errors",
            "--output",
            "json",
            "-vvv",
        ],
    );
    assert_eq!(code(&output), 3);
    assert_eq!(finished(&output_log), []);

    // An initial conftest that fails to load: pytest exits 4.
    let dir = project(&[
        ("tests/conftest.py", "import nonexistent_module_xyz\n"),
        ("tests/test_a.py", PASSING),
        ("tests/sub/test_b.py", PASSING),
    ]);
    for workers in ["1", "2"] {
        let output = aye(&python, dir.path(), &["--color", "never", "-n", workers]);
        assert_eq!(code(&output), 4, "-n {workers}: {}", stdout(&output));
        let text = stdout(&output);
        assert_eq!(summary(&output), "usage error · ! 1 usage error", "{text}");
    }
}

/// pytest counts a collection error as a failure: with
/// `--continue-on-collection-errors` the run goes on but exits 1, also
/// when nothing else runs, and under `--co`.
#[test]
fn collection_errors_fail_a_run_that_continues_past_them() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/test_b.py", "def test_b(:\n    pass\n"),
    ]);
    for (args, want) in [
        (vec![], 2),
        (vec!["--continue-on-collection-errors"], 1),
        (vec!["--continue-on-collection-errors", "--co"], 1),
        (
            vec!["--continue-on-collection-errors", "tests/test_b.py"],
            1,
        ),
        (vec!["--co"], 2),
    ] {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), want, "{args:?}: {}", stdout(&output));
        assert_eq!(output_log.last().unwrap()["exit_code"], want);
    }
}

/// Finding C27: pytest fails the collection of a directory it can't read,
/// and so the run (exit 2); aye did run the rest and exit 0.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_a_collection_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/locked/test_l.py", PASSING),
    ]);
    let locked = dir.path().join("tests/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    let run = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args);
        logged(&python, dir.path(), &argv)
    };
    let outputs = [
        run(&[]),
        run(&["--continue-on-collection-errors"]),
        run(&["--co"]),
    ];
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        eprintln!("skipping: permissions aren't enforced here (root?)");
        return;
    }
    for ((output, records), want) in outputs.iter().zip([2, 1, 2]) {
        assert_eq!(code(output), want, "{}", stdout(output));
        let errors: Vec<_> = events(records, "diagnostic")
            .into_iter()
            .filter(|d| d["severity"] == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(errors[0]["code"], "unreadable");
        assert_eq!(errors[0]["path"], "tests/locked");
    }
    assert_eq!(finished(&outputs[1].1).len(), 1);
}

/// A file below a conftest that fails to load gets no `no-tests` note: the
/// conftest's error says why it has no tests, as pytest reports only the
/// error (a start-up one: pytest stops, aye exits 4; one met while
/// collecting: `1 error`, exit 2).
#[test]
fn a_file_below_a_broken_conftest_is_explained_by_its_error() {
    let Some(python) = python() else { return };
    let dynamic = "import pytest\n\nV = [x for x in range(2)]\n\n\
                   @pytest.mark.parametrize('v', V)\ndef test_x(v):\n    pass\n";
    for (conftest, kind, exit) in [
        ("tests/conftest.py", "usage-error", 4),
        ("tests/sub/conftest.py", "collection-error", 2),
    ] {
        let dir = project(&[
            ("pyproject.toml", PYTEST_CONFIG),
            ("tests/test_a.py", "def test_a():\n    pass\n"),
            (conftest, "import zz_missing_module\n"),
            ("tests/sub/test_x.py", dynamic),
        ]);
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "-n", "1"],
        );
        assert_eq!(code(&output), exit, "{conftest}: {}", stdout(&output));
        let records = output_log;
        assert_eq!(
            diagnosed(&records, kind),
            [("error".to_owned(), conftest.to_owned())],
            "{conftest}"
        );
        assert_eq!(diagnosed(&records, "no-tests"), [], "{conftest}");
    }
}

/// Recheck regression 3: with the default adapters and doctests on, one
/// conftest that fails to load is one error, as pytest loads it once,
/// though both adapters report it.
#[test]
fn a_conftest_both_adapters_fail_to_load_is_one_error() {
    let Some(python) = python() else { return };
    // At start-up (a `test*` directory's): pytest stops with a traceback,
    // aye with one usage error.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        ("tests/conftest.py", SKIPS),
        ("tests/m.py", DOCTEST),
        ("tests/test_t.py", "def test_t():\n    pass\n"),
    ]);
    let output = aye(&python, dir.path(), &[]);
    assert_eq!(code(&output), 4, "{}", stdout(&output));
    assert_eq!(summary(&output), "usage error · ! 1 usage error");
    assert_eq!(stdout(&output).matches("error[usage-error]").count(), 1);
    let (_output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(
        diagnosed(&output_log, "usage-error"),
        [
            ("error".to_owned(), "tests/conftest.py".to_owned()),
            ("info".to_owned(), "tests/conftest.py".to_owned())
        ]
    );
    // While collecting (`pytest.fail`, which the adapters worded apart):
    // pytest says `1 passed, 1 error`.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        (
            "sub/conftest.py",
            "import pytest\npytest.fail(\"broken\")\n",
        ),
        ("sub/m.py", DOCTEST),
        ("sub/test_t.py", "def test_t():\n    pass\n"),
    ]);
    for workers in ["1", "3"] {
        let output = aye(
            &python,
            dir.path(),
            &["--continue-on-collection-errors", "-n", workers],
        );
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        assert_eq!(summary(&output), "✓ 1 passed · ! 1 collection error");
    }
}
