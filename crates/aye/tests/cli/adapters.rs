//! The adapters: which run by default, whatever the interpreter installs;
//! which takes a test two of them collect; a file or a package's
//! `load_tests` two of them read; and how their batches run together.

use std::fs;
#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use aye_test::venv_without_pytest;
use aye_test::{code, events, project, stdout};

#[cfg(unix)]
use crate::aye_config;
use crate::{aye, finished, listed, logged, python, summary};

/// What `aye config` says about the adapters: their names and why.
#[cfg(unix)]
fn adapters_line(python: &Path, cwd: &Path) -> String {
    let output = aye_config(python, cwd);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    stdout(&output)
        .lines()
        .find_map(|line| line.strip_prefix("adapters "))
        .unwrap_or_else(|| panic!("no adapters line: {}", stdout(&output)))
        .trim()
        .to_string()
}

/// Without `[tool.aye] adapters`, the default is doctest then pytest, for
/// every project, whatever the interpreter installs: a project of
/// `TestCase` classes with no pytest configuration and no conftest.py, run
/// with an interpreter that has no pytest, runs them by pytest's rules on
/// aye's engine; `[tool.aye] adapters = ["unittest"]` gives them to
/// unittest's discovery.
#[cfg(unix)]
#[test]
fn the_default_adapters_dont_depend_on_what_is_installed() {
    let Some(python) = python() else { return };
    let venvs = tempfile::tempdir().unwrap();
    let Some(bare) = venv_without_pytest(&python, venvs.path()) else {
        return;
    };
    let dir = project(&[
        ("tests/__init__.py", ""),
        (
            "tests/test_shop.py",
            "import unittest\n\n\nclass CartTests(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(1 + 1, 2)\n\n    def test_total(self):\n        self.assertEqual(sum([1, 2]), 4)\n",
        ),
    ]);
    let want = [
        (
            "tests/test_shop.py::CartTests::test_add".to_owned(),
            "passed".to_owned(),
        ),
        (
            "tests/test_shop.py::CartTests::test_total".to_owned(),
            "failed".to_owned(),
        ),
    ];
    for interpreter in [&bare, &python] {
        assert_eq!(
            adapters_line(interpreter, dir.path()),
            "doctest, pytest  (default: pytest's rules, which run unittest's TestCase classes too)"
        );
        let (output, output_log) = logged(interpreter, dir.path(), &["--output", "json", "-vvv"]);
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "case_collected")
                .iter()
                .all(|e| e["adapter"] == "pytest"),
            "{}",
            stdout(&output)
        );
        assert_eq!(finished(&records), want);
    }
    // Selection by dotted names.
    let (one, one_log) = logged(
        &bare,
        dir.path(),
        &[
            "--output",
            "json",
            "-vvv",
            "tests.test_shop.CartTests.test_add",
        ],
    );
    assert_eq!(code(&one), 0, "{}", stdout(&one));
    assert_eq!(finished(&one_log).len(), 1);
    // unittest's discovery, when the project asks for it.
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
    )
    .unwrap();
    assert_eq!(
        adapters_line(&bare, dir.path()),
        "unittest  (from [tool.aye] adapters)"
    );
    let (output, output_log) = logged(&bare, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let records = output_log;
    assert!(
        events(&records, "case_collected")
            .iter()
            .all(|e| e["adapter"] == "unittest")
    );
    assert_eq!(finished(&records), want);
}

/// With `["unittest", "pytest"]`, unittest runs the `TestCase` methods both
/// collect, without a duplicate-id warning, and pytest the rest.
#[test]
fn a_higher_priority_adapter_takes_the_tests_both_collect_quietly() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\"]\n",
        ),
        (
            "test_mixed.py",
            "import unittest\n\n\nclass T(unittest.TestCase):\n    def test_case(self):\n        pass\n\n\ndef test_function():\n    pass\n",
        ),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let collected: Vec<(String, String)> = events(&records, "case_collected")
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["adapter"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        collected,
        [
            (
                "test_mixed.py::T::test_case".to_owned(),
                "unittest".to_owned()
            ),
            (
                "test_mixed.py::test_function".to_owned(),
                "pytest".to_owned()
            ),
        ]
    );
    assert!(events(&records, "diagnostic").is_empty(), "{records:#?}");
}

/// Finding F32: a file holding a `TestCase` (the unittest adapter's) and a
/// test function (pytest's) that fails to import is one collection error,
/// as pytest, which collects the file once, reports it.
#[test]
fn a_file_two_adapters_load_fails_to_import_once() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\"]\n",
        ),
        (
            "test_imp.py",
            "import os\nimport unittest\n\nif not os.environ.get('NOPE'):\n    import nonexistent_zzz\n\n\nclass TestA(unittest.TestCase):\n    def test_a(self):\n        pass\n\n\ndef test_f():\n    pass\n",
        ),
    ]);
    let output = aye(&python, dir.path(), &["--output", "json", "-v"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    let errors = stdout(&output)
        .lines()
        .filter(|line| line.contains(r#""type":"collection_error""#))
        .count();
    assert_eq!(errors, 1, "{}", stdout(&output));
    let output = aye(
        &python,
        dir.path(),
        &["--continue-on-collection-errors", "--junitxml", "j.xml"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        summary(&output) == "! 1 collection error",
        "{}",
        stdout(&output)
    );
    let junit = fs::read_to_string(dir.path().join("j.xml")).unwrap();
    assert!(junit.contains(r#"errors="1""#), "{junit}");
    // The other adapter's error is still in the stream, as an info (its
    // batch runs past the error, which aye's pytest front-end met while it
    // listed the file).
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &[
            "--continue-on-collection-errors",
            "--output",
            "json",
            "-vvv",
        ],
    );
    let records = output_log;
    let severities: Vec<&str> = events(&records, "diagnostic")
        .iter()
        .filter(|d| d["code"] == "collection-error")
        .map(|d| d["severity"].as_str().unwrap())
        .collect();
    assert_eq!(severities, ["error", "info"]);
}

/// Finding F7: a package's `load_tests` that discovers another pattern
/// makes tests in files aye's walk doesn't claim; they run at the
/// package's place, as unittest's discovery runs them.
#[test]
fn tests_a_packages_load_tests_finds_elsewhere_keep_the_packages_place() {
    let Some(python) = python() else { return };
    let case = |class: &str, method: &str| {
        format!(
            "import unittest\n\n\nclass {class}(unittest.TestCase):\n    def {method}(self):\n        pass\n"
        )
    };
    let (check, a, z) = (
        case("Check", "test_check"),
        case("A", "test_a"),
        case("Z", "test_z"),
    );
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
        ),
        (
            "pkg/__init__.py",
            "import os\n\n\ndef load_tests(loader, standard_tests, pattern):\n    here = os.path.dirname(__file__)\n    standard_tests.addTests(\n        loader.discover(start_dir=here, pattern='check_*.py', top_level_dir=os.path.dirname(here))\n    )\n    return standard_tests\n",
        ),
        ("pkg/check_one.py", check.as_str()),
        ("test_a.py", a.as_str()),
        ("test_z.py", z.as_str()),
    ]);
    let order = [
        "pkg/check_one.py::Check::test_check",
        "test_a.py::A::test_a",
        "test_z.py::Z::test_z",
    ];
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--co", "--output", "json", "-vvv", "-v"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let listed = output_log;
    let collected: Vec<&str> = events(&listed, "case_collected")
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(collected, order);
    // The package's `load_tests` made a test: it has one.
    assert!(
        events(&listed, "diagnostic")
            .iter()
            .all(|d| d["code"] != "no-tests"),
        "{listed:#?}"
    );
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let ran: Vec<String> = finished(&output_log)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ran, order);
}

/// pytest's doctests, run by the doctest adapter beside pytest by default:
/// a package's `__init__.py` comes first in its directory, a file's
/// doctests before its tests, dotted names select doctests, and modules
/// without doctests are no files without tests.
#[test]
fn doctests_run_beside_pytest_in_pytests_order() {
    let Some(python) = python() else { return };
    let doc = |name: &str| format!("def {name}():\n    \"\"\"\n    >>> 1 + 1\n    2\n    \"\"\"\n");
    let init = doc("f");
    let alpha = format!("{}\ndef test_alpha():\n    pass\n", doc("g"));
    let beta = doc("h");
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\npython_files = [\"*.py\"]\n",
        ),
        ("pkg/__init__.py", init.as_str()),
        ("pkg/Alpha.py", alpha.as_str()),
        ("pkg/beta.py", beta.as_str()),
        ("pkg/plain.py", "X = 1\n"),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--co", "--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    // pytest 9.1.1's `--co -q`, in its order.
    assert_eq!(
        listed(&output),
        [
            "pkg/__init__.py::pkg.f",
            "pkg/Alpha.py::pkg.Alpha.g",
            "pkg/Alpha.py::test_alpha",
            "pkg/beta.py::pkg.beta.h",
        ]
    );
    let collected = output_log;
    assert!(
        events(&collected, "diagnostic").is_empty(),
        "{:#?}",
        events(&collected, "diagnostic")
    );
    for (selector, ids) in [
        ("pkg.beta.h", vec!["pkg/beta.py::pkg.beta.h"]),
        (
            "pkg.Alpha",
            vec!["pkg/Alpha.py::pkg.Alpha.g", "pkg/Alpha.py::test_alpha"],
        ),
        ("pkg.f", vec!["pkg/__init__.py::pkg.f"]),
    ] {
        let (output, output_log) =
            logged(&python, dir.path(), &["--output", "json", "-vvv", selector]);
        assert_eq!(code(&output), 0, "{selector}: {}", stdout(&output));
        let ran: Vec<String> = finished(&output_log)
            .into_iter()
            .map(|(id, outcome)| {
                assert_eq!(outcome, "passed", "{id}");
                id
            })
            .collect();
        assert_eq!(ran, ids, "{selector}");
    }
}

/// With one worker, a run goes in plan order across adapters, as pytest
/// runs a file's doctests before its tests.
#[test]
fn one_worker_runs_the_adapters_batches_in_plan_order() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n",
        ),
        (
            "test_alpha.py",
            "def g():\n    \"\"\"\n    >>> 1 + 1\n    2\n    \"\"\"\n\ndef test_alpha():\n    pass\n",
        ),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let ran: Vec<String> = finished(&records).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ran,
        ["test_alpha.py::test_alpha.g", "test_alpha.py::test_alpha"]
    );
    // The worker that listed pytest's session waits for the doctests'
    // (it lists once), and each worker's events come between its start and
    // its end.
    let lifecycle: Vec<String> = records
        .iter()
        .filter_map(|r| match r["event"].as_str()? {
            "batch_started" => Some(format!("batch {}", r["adapter"].as_str()?)),
            "worker_started" | "worker_finished" => Some(r["event"].as_str()?.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle,
        [
            "worker_started",
            "batch doctest",
            "worker_finished",
            "worker_started",
            "batch pytest",
            "worker_finished"
        ],
        "{}",
        stdout(&output)
    );
}
