//! aye's engine: a pytest project run without pytest, and refinement,
//! which a run of pytest's tests can't do without and which is cached.

use std::fs;

#[cfg(unix)]
use aye_test::venv_without_pytest;
use aye_test::{code, events, project, stdout};
#[cfg(unix)]
use serde_json::Value;

use crate::{aye, listed, logged, python};
#[cfg(unix)]
use crate::{aye_config, finished};

#[test]
fn refinement_is_cached_for_the_interpreter_and_the_framework_args() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_p.py",
        "import pytest\n\ndef cases():\n    return [1, 2]\n\n@pytest.mark.parametrize('x', cases())\ndef test_p(x):\n    pass\n",
    )]);
    let ids = |args: &[&str]| {
        let mut argv = vec!["--co", "--output", "json", "-vvv"];
        argv.extend(args);
        let output = aye(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        listed(&output)
    };
    let refined = ["tests/test_p.py::test_p[1]", "tests/test_p.py::test_p[2]"];
    // Without refining, the static guess holds one case for every parameter.
    assert_eq!(ids(&["--no-refine"]), ["tests/test_p.py::test_p"]);
    assert_eq!(ids(&[]), refined);
    let entries = fs::read_dir(dir.path().join(".aye/cache/refine/pytest"))
        .unwrap()
        .count();
    assert_eq!(entries, 1);
    // A hit needs no worker, so it answers even without refining.
    assert_eq!(ids(&["--no-refine"]), refined);
    // The arguments after `--` are part of the key.
    assert_eq!(
        ids(&["--no-refine", "--", "--tb=short"]),
        ["tests/test_p.py::test_p"]
    );
}

/// A pytest project runs on aye's own engine, with an interpreter that has
/// no pytest, and with one that has (aye's `pytest` module wins), and gets
/// the ids, the order and the outcomes plain pytest gives it: conftest
/// fixtures of two scopes with teardown, parametrize with an id, a class,
/// a mark, skip, xfail, `tmp_path`, `monkeypatch`, `capsys`, `raises`, and
/// failing asserts with their structured comparison. `--native`, the
/// prototype's switch, is accepted and changes nothing, and `aye config`
/// says which engine runs.
#[cfg(unix)]
#[test]
fn a_pytest_project_runs_on_ayes_engine_without_pytest() {
    let Some(python) = python() else { return };
    let venvs = tempfile::tempdir().unwrap();
    let Some(bare) = venv_without_pytest(&python, venvs.path()) else {
        return;
    };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\nmarkers = [\"slow: slow tests\"]\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\n\n\n@pytest.fixture(scope=\"session\")\ndef db():\n    data = {\"rows\": []}\n    yield data\n    data.clear()\n\n\n@pytest.fixture\ndef row(db):\n    db[\"rows\"].append(1)\n    yield len(db[\"rows\"])\n    db[\"rows\"].pop()\n",
        ),
        (
            "tests/test_app.py",
            "import os\n\nimport pytest\n\n\ndef test_row(row, db):\n    assert row == 1\n    assert db[\"rows\"] == [1]\n\n\n@pytest.mark.parametrize(\"n, expected\", [(1, 2), (2, 4), pytest.param(3, 7, id=\"odd\")])\ndef test_double(n, expected):\n    assert n * 2 == expected\n\n\nclass TestThings:\n    @pytest.mark.slow\n    def test_tmp(self, tmp_path):\n        (tmp_path / \"f\").write_text(\"x\")\n        assert (tmp_path / \"f\").read_text() == \"x\"\n\n    def test_env(self, monkeypatch):\n        monkeypatch.setenv(\"AYE_X\", \"1\")\n        assert os.environ[\"AYE_X\"] == \"1\"\n\n    def test_out(self, capsys):\n        print(\"hello\")\n        assert capsys.readouterr().out == \"hello\\n\"\n\n\n@pytest.mark.skip(reason=\"not today\")\ndef test_skipped():\n    pass\n\n\n@pytest.mark.xfail(reason=\"known\")\ndef test_xfail():\n    assert 1 == 2\n\n\ndef test_raises():\n    with pytest.raises(ZeroDivisionError):\n        1 / 0\n\n\ndef test_dict():\n    assert {\"a\": 1, \"b\": 2} == {\"a\": 1, \"b\": 3}\n",
        ),
    ]);
    // Plain pytest 9.1.1's ids, in its order, and outcomes.
    let want: Vec<(String, String)> = [
        ("test_row", "passed"),
        ("test_double[1-2]", "passed"),
        ("test_double[2-4]", "passed"),
        ("test_double[odd]", "failed"),
        ("TestThings::test_tmp", "passed"),
        ("TestThings::test_env", "passed"),
        ("TestThings::test_out", "passed"),
        ("test_skipped", "skipped"),
        ("test_xfail", "expected_failure"),
        ("test_raises", "passed"),
        ("test_dict", "failed"),
    ]
    .iter()
    .map(|(id, outcome)| (format!("tests/test_app.py::{id}"), (*outcome).to_string()))
    .collect();
    let sorted = |mut pairs: Vec<(String, String)>| {
        pairs.sort();
        pairs
    };
    let failure = |records: &[Value], id: &str| -> Value {
        events(records, "test_finished")
            .into_iter()
            .find(|e| e["id"] == id)
            .map_or(Value::Null, |e| e["failures"][0].clone())
    };

    for (interpreter, args) in [
        (&bare, &["--output", "json", "-vvv"][..]),
        (&python, &["--output", "json", "-vvv"]),
        (&bare, &["--native", "--output", "json", "-vvv"]),
    ] {
        let (output, output_log) = logged(interpreter, dir.path(), args);
        assert_eq!(code(&output), 1, "{args:?}: {}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "case_collected")
                .iter()
                .all(|e| e["adapter"] == "pytest"),
            "{records:#?}"
        );
        let collected: Vec<&str> = events(&records, "case_collected")
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        let want_ids: Vec<&str> = want.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(collected, want_ids, "pytest's collection order");
        assert_eq!(sorted(finished(&records)), sorted(want.clone()));
        // Failing asserts carry the comparison, as pytest's do.
        for id in [
            "tests/test_app.py::test_dict",
            "tests/test_app.py::test_double[odd]",
        ] {
            let failure = failure(&records, id);
            assert_eq!(failure["exception"], "AssertionError", "{id}: {failure:#}");
            assert_eq!(failure["assertion"]["op"], "==", "{id}: {failure:#}");
        }
        // A rerun is plain `aye <id>`.
        let rerun = events(&records, "test_finished")
            .into_iter()
            .find(|e| e["id"] == "tests/test_app.py::test_dict")
            .map(|e| e["reproduce"]["command"].clone())
            .unwrap();
        assert_eq!(
            rerun,
            serde_json::json!(["aye", "tests/test_app.py::test_dict"])
        );
    }

    // `aye config` says which engine runs the tests.
    for interpreter in [&bare, &python] {
        let config = aye_config(interpreter, dir.path());
        assert_eq!(code(&config), 0, "{}", stdout(&config));
        assert!(
            stdout(&config).contains("\nengine   native\n"),
            "{}",
            stdout(&config)
        );
    }
}

/// aye's pytest front-end imports every pytest file a run runs, so a run
/// can't do without refinement: `--no-refine` is a usage error before
/// anything runs, unless with `--co`, which lists the static prediction
/// without importing anything; a run with no pytest adapter takes it.
#[test]
fn a_run_of_pytests_tests_cant_do_without_refinement() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_a.py",
        "import pytest\n\n@pytest.mark.parametrize('x', [1, 2])\ndef test_a(x):\n    pass\n",
    )]);
    for args in [&["--no-refine"][..], &["--native", "--no-refine"]] {
        let output = aye(&python, dir.path(), args);
        assert_eq!(code(&output), 4, "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--no-refine lists tests without importing them, with --co"),
            "{stderr}"
        );
    }
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--co", "--no-refine", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let listed: Vec<(&str, &str)> = events(&records, "case_collected")
        .iter()
        .map(|e| (e["id"].as_str().unwrap(), e["certainty"].as_str().unwrap()))
        .collect();
    assert_eq!(
        listed,
        [
            ("tests/test_a.py::test_a[1]", "exact"),
            ("tests/test_a.py::test_a[2]", "exact")
        ]
    );
    assert_eq!(events(&records, "worker_started").len(), 0);
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
    )
    .unwrap();
    let output = aye(&python, dir.path(), &["--no-refine"]);
    assert_eq!(code(&output), 5, "{}", stdout(&output));
}
