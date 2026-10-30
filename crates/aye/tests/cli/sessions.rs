//! Workers and the framework's sessions: what a run's workers share, a
//! plugin's options that pin nothing, a session's teardown, and a session
//! the framework failed.

use std::fs;

use aye_test::{code, events, project, stdout};

use crate::{PASSING, aye, context, finished, logged, python};

#[test]
fn a_runs_workers_share_one_testrun_uid() {
    let Some(python) = python() else { return };
    let writes = "import os\n\ndef test_uid():\n    with open(os.path.join(os.path.dirname(__file__), '..', 'uid-' + __name__), 'w') as f:\n        f.write(os.environ['PYTEST_XDIST_TESTRUNUID'])\n";
    let dir = project(&[("tests/test_a.py", writes), ("tests/test_b.py", writes)]);
    let uids = || {
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "-n", "2"],
        );
        assert_eq!(code(&output), 0, "{}", stdout(&output));
        let workers = events(&output_log, "worker_started").len();
        assert_eq!(workers, 2);
        ["uid-test_a", "uid-test_b"].map(|name| fs::read_to_string(dir.path().join(name)).unwrap())
    };
    let [a, b] = uids();
    assert_ne!(a, "");
    assert_eq!(a, b);
    // One per run.
    assert_ne!(uids()[0], a);
}

/// No pytest session runs, so nothing needs one: an option of a plugin
/// that writes its data once per pytest session (pytest-cov's `--cov`)
/// neither pins the run to one worker nor takes pytest's doctests from the
/// doctest adapter, as `aye config` shows. (aye's engine loads no
/// third-party plugin: what the option does in a run is the front-end's.)
#[test]
fn session_plugins_options_pin_nothing() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules --cov=pkg\"\n",
        ),
        (
            "pkg/__init__.py",
            "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
        ),
        ("test_a.py", PASSING),
        ("test_b.py", PASSING),
    ]);
    let output = context()
        .command(dir.path())
        .args(["config", "-n", "2", "--python"])
        .arg(&python)
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("\nworkers  2\n"), "{text}");
    assert!(
        text.contains(
            "\nadapters doctest, pytest  (default: pytest's rules, which run unittest's \
             TestCase classes too)\n"
        ),
        "{text}"
    );
}

/// Finding C22: a conftest or plugin that fails a session none of whose
/// tests failed (`session.exitstatus`, pytest-cov's `--cov-fail-under`)
/// fails the run, as pytest exits 1, however many workers run it.
#[test]
fn a_session_the_framework_failed_fails_the_run() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    session.exitstatus = 1\n",
        ),
        ("tests/test_a.py", PASSING),
        ("tests/test_b.py", PASSING),
    ]);
    for workers in ["1", "2"] {
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["-n", workers, "--output", "json", "-vvv"],
        );
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "diagnostic")
                .iter()
                .any(|d| d["code"] == "session-failed"),
            "{}",
            stdout(&output)
        );
        assert_eq!(events(&records, "run_finished")[0]["exit_code"], 1);
    }
}

/// A session-scoped fixture whose teardown fails: pytest tears the session
/// down in its last test's teardown, which errors (exit 1). With one worker
/// aye ends the session with the same test; with several, each worker's
/// session ends with its last batch, as under pytest-xdist, and the run
/// fails all the same.
#[test]
fn a_session_teardown_that_fails_is_the_last_tests_error() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "import pytest\n\n@pytest.fixture(scope='session')\ndef res():\n    yield 1\n    raise RuntimeError('broke')\n",
        ),
        (
            "tests/test_a.py",
            "def test_a1(res):\n    pass\n\ndef test_a2(res):\n    pass\n",
        ),
        ("tests/test_b.py", "def test_b(res):\n    pass\n"),
    ]);
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["-n", "1", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert_eq!(
        finished(&output_log),
        [
            ("tests/test_a.py::test_a1".to_string(), "passed".to_string()),
            ("tests/test_a.py::test_a2".to_string(), "passed".to_string()),
            ("tests/test_b.py::test_b".to_string(), "error".to_string()),
        ]
    );
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["-n", "2", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let records = output_log;
    let teardowns = events(&records, "lifetime_finished")
        .iter()
        .filter(|e| !e["failure"].is_null())
        .count()
        + finished(&records)
            .iter()
            .filter(|(_, outcome)| outcome == "error")
            .count();
    assert!(teardowns >= 1, "{}", stdout(&output));
}

/// A session's teardown runs to its end, however long it takes, in every
/// worker that set the session up: pytest waits for it, and the worker's
/// shutdown mustn't cut it short.
#[test]
fn a_session_teardown_runs_to_its_end_in_every_worker() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "import os, time\nimport pytest\n\n@pytest.fixture(scope='session')\ndef res():\n    worker = os.environ['AYE_WORKER']\n    open(f'up-{worker}', 'w').close()\n    yield 1\n    time.sleep(3.5)\n    open(f'done-{worker}', 'w').close()\n",
        ),
        ("tests/test_a.py", "def test_a(res):\n    pass\n"),
        ("tests/test_b.py", "def test_b(res):\n    pass\n"),
    ]);
    for workers in ["1", "2"] {
        let output = aye(
            &python,
            dir.path(),
            &["-n", workers, "--output", "json", "-vvv"],
        );
        assert_eq!(code(&output), 0, "{}", stdout(&output));
        let mut up = 0;
        for lane in 0..2 {
            if dir.path().join(format!("up-{lane}")).exists() {
                up += 1;
                assert!(
                    dir.path().join(format!("done-{lane}")).exists(),
                    "-n {workers}: worker {lane}'s session teardown was cut short"
                );
            }
            let _ = fs::remove_file(dir.path().join(format!("up-{lane}")));
            let _ = fs::remove_file(dir.path().join(format!("done-{lane}")));
        }
        assert!(up >= 1, "-n {workers}: {}", stdout(&output));
    }
}
