//! Static selection: the command line's and the framework's own `-k` and
//! `-m`, pytest's `--deselect`, and history's `--lf` and `--ff`.

use aye_test::{as_records, code, events, project, stdout};

use crate::common::{kept, runs_in};

use crate::{PASSING, aye, aye_command, finished, listed, logged, python};

#[test]
fn a_keyword_may_look_like_a_negative_number() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_s.py",
        "import pytest\n\n@pytest.mark.parametrize('a', [1, 2])\n@pytest.mark.parametrize('b', ['x', 'y'])\ndef test_stacked(a, b):\n    pass\n",
    )]);
    let ids = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "--co"];
        argv.extend(args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}");
        events(&output_log, "case_collected")
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&["-k", "-1"]),
        [
            "tests/test_s.py::test_stacked[x-1]",
            "tests/test_s.py::test_stacked[y-1]"
        ]
    );
    assert_eq!(
        ids(&["-k", "-1 and x"]),
        ["tests/test_s.py::test_stacked[x-1]"]
    );
    // As in pytest, a value that looks like a flag is not one.
    assert_eq!(code(&aye(&python, dir.path(), &["-k", "-x"])), 4);
}

#[test]
fn failed_first_runs_last_failures_first() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "def test_a1():\n    pass\n\ndef test_a2():\n    pass\n",
        ),
        ("tests/test_b.py", "def test_b():\n    assert 0\n"),
    ]);
    assert_eq!(code(&aye(&python, dir.path(), &[])), 1);
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &["--ff", "-n", "1", "--output", "json", "-vvv"],
    );
    let started: Vec<_> = events(&output_log, "test_started")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        started,
        [
            "tests/test_b.py::test_b",
            "tests/test_a.py::test_a1",
            "tests/test_a.py::test_a2"
        ]
    );
}

/// Under `--ff`, the cases a run collects are listed with what failed last
/// first, each part in the arguments' order: a test's own id decides, so a
/// `Partial` case is moved only by its narrowed ids (one an id with
/// parameters asks for), never by its parameters' failures.
#[test]
fn failed_first_lists_what_failed_last_first_in_the_arguments_order() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "def test_a1():\n    pass\n\ndef test_a2():\n    assert 0\n",
        ),
        (
            "tests/test_b.py",
            "def test_b1():\n    assert 0\n\ndef test_b2():\n    pass\n",
        ),
        (
            "tests/test_p.py",
            "import pytest\n\ndef cases():\n    return [0, 1]\n\n@pytest.mark.parametrize('n', cases())\ndef test_p(n):\n    assert n == 0\n",
        ),
    ]);
    assert_eq!(code(&aye(&python, dir.path(), &[])), 1);
    let ids = |args: &[&str]| {
        let mut argv = vec!["--co", "--ff", "--output", "json", "-vvv"];
        argv.extend(args);
        let output = aye(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        listed(&output)
    };
    assert_eq!(
        ids(&[]),
        [
            "tests/test_a.py::test_a2",
            "tests/test_b.py::test_b1",
            "tests/test_p.py::test_p[1]",
            "tests/test_a.py::test_a1",
            "tests/test_b.py::test_b2",
            "tests/test_p.py::test_p[0]",
        ]
    );
    assert_eq!(
        ids(&["tests/test_b.py", "tests/test_a.py"]),
        [
            "tests/test_b.py::test_b1",
            "tests/test_a.py::test_a2",
            "tests/test_b.py::test_b2",
            "tests/test_a.py::test_a1",
        ]
    );
    // The arguments after `--` are part of the refinement cache's key, so
    // these get the static guess: `test_p`, `Partial`, whose own id didn't
    // fail, stays in its place; narrowed to `[1]`, it comes first.
    assert_eq!(
        ids(&["--no-refine", "--", "--tb=short"]),
        [
            "tests/test_a.py::test_a2",
            "tests/test_b.py::test_b1",
            "tests/test_a.py::test_a1",
            "tests/test_b.py::test_b2",
            "tests/test_p.py::test_p",
        ]
    );
    assert_eq!(
        ids(&[
            "--no-refine",
            "tests/test_a.py",
            "tests/test_p.py::test_p[1]",
            "--",
            "--tb=short"
        ]),
        [
            "tests/test_a.py::test_a2",
            "tests/test_p.py::test_p[1]",
            "tests/test_a.py::test_a1",
        ]
    );
}

#[test]
fn the_frameworks_own_k_and_m_select_statically() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"-k 'not slow'\"\nmarkers = [\"db\"]\n",
        ),
        (
            "tests/test_a.py",
            "import pytest\n\ndef test_fast():\n    pass\n\ndef test_slow():\n    pass\n\n@pytest.mark.db\ndef test_db():\n    pass\n",
        ),
    ]);
    let run = |args: &[&str], addopts: Option<&str>| {
        let mut argv = vec!["--co", "--output", "json", "-vvv"];
        argv.extend(args);
        let mut command = aye_command(&python, dir.path(), &argv);
        if let Some(addopts) = addopts {
            command.env("PYTEST_ADDOPTS", addopts);
        }
        let output = command.output().unwrap();
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        let deselected = events(&as_records(&output), "collection_finished")[0]["deselected"]
            .as_u64()
            .unwrap();
        (listed(&output), deselected)
    };
    let a = |name: &str| format!("tests/test_a.py::{name}");
    // addopts' `-k` deselects statically, so `--co` and the counts match
    // pytest's.
    assert_eq!(run(&[], None), (vec![a("test_fast"), a("test_db")], 1));
    // `PYTEST_ADDOPTS` adds to addopts.
    assert_eq!(run(&[], Some("-m 'not db'")), (vec![a("test_fast")], 2));
    // The command line's `-k` wins over addopts'.
    assert_eq!(run(&["-k", "slow"], None), (vec![a("test_slow")], 2));
    // The arguments after `--` come last, so theirs wins over both.
    assert_eq!(
        run(&["-k", "slow", "--", "-k", "db"], None),
        (vec![a("test_db")], 2)
    );
    // And the tests run as pytest runs them.
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0);
    assert_eq!(finished(&output_log).len(), 2);
}

/// Finding F28: an `@file` after `--` hides what pytest's `-k` and `-m`
/// would be from aye, but no adapter applies an expression itself (aye's
/// engine runs pytest's tests too), so the command line's selects every
/// adapter's cases.
#[test]
fn an_argument_file_leaves_the_command_lines_k_to_other_adapters() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\", \"doctest\"]\n\n[tool.aye.doctest]\nmodules = true\n",
        ),
        (
            "test_a.py",
            "import unittest\n\n\nclass TestU(unittest.TestCase):\n    def test_one(self):\n        pass\n\n    def test_two(self):\n        pass\n\n\ndef test_func():\n    pass\n",
        ),
        (
            "lib.py",
            "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
        ),
        ("args.txt", "-v\n"),
    ]);
    for args in [
        &["-k", "one", "--output", "json", "-vvv", "--", "@args.txt"][..],
        &["-k", "one", "--output", "json", "-vvv"],
    ] {
        let (output, output_log) = logged(&python, dir.path(), args);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        assert_eq!(
            finished(&output_log),
            [(
                "test_a.py::TestU::test_one".to_string(),
                "passed".to_string()
            )],
            "{args:?}"
        );
    }
    let output = aye(
        &python,
        dir.path(),
        &["-m", "slow", "--output", "json", "-vvv", "--", "@args.txt"],
    );
    assert_eq!(code(&output), 5, "{}", stdout(&output));
}

/// pytest's `--deselect`, from `addopts`, `PYTEST_ADDOPTS` or the arguments
/// after `--`, drops the tests it names (by id prefix, as a string) and
/// counts them as deselected, as pytest's summary does.
#[test]
fn the_frameworks_deselect_is_counted() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--deselect tests/test_a.py::test_b\"\n",
        ),
        (
            "tests/test_a.py",
            "def test_pass():\n    pass\n\ndef test_b():\n    pass\n\ndef test_b2():\n    pass\n",
        ),
        ("tests/test_o.py", PASSING),
    ]);
    let run = |args: &[&str], env: Option<&str>| {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args);
        let mut command = aye_command(&python, dir.path(), &argv);
        if let Some(env) = env {
            command.env("PYTEST_ADDOPTS", env);
        }
        let before = runs_in(dir.path());
        let output = command.output().unwrap();
        let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
        let finished = records.last().unwrap();
        (
            code(&output),
            finished["counts"]["passed"].as_u64().unwrap_or(0),
            finished["counts"]["deselected"].as_u64().unwrap_or(0),
        )
    };
    // pytest 9.1.1: "2 passed, 2 deselected" (`test_b` also drops
    // `test_b2`), "1 passed, 3 deselected" twice, then "1 deselected" (the
    // argument names `test_b` alone) and exit 5.
    assert_eq!(run(&[], None), (0, 2, 2));
    assert_eq!(run(&[], Some("--deselect tests/test_o.py")), (0, 1, 3));
    assert_eq!(
        run(&["--", "--deselect", "tests/test_a.py::test_pass"], None),
        (0, 1, 3)
    );
    assert_eq!(run(&["tests/test_a.py::test_b"], None), (5, 0, 1));
}

/// Findings C23 and C31: under `--lf`, pytest's summary counts as
/// deselected only what its last-failed plugin let collection keep.
#[test]
fn last_failed_counts_deselected_as_pytest_does() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "def test_a1():\n    assert 0\n\ndef test_a2():\n    pass\n",
        ),
        (
            "tests/test_b.py",
            "def test_b1():\n    pass\n\ndef test_b2():\n    pass\n",
        ),
    ]);
    assert_eq!(code(&aye(&python, dir.path(), &[])), 1);
    // pytest 9.1.1 (cache on), after the same first run.
    for (args, deselected) in [
        (vec![], 0),
        (vec!["tests"], 0),
        (vec!["tests/test_a.py", "tests/test_b.py"], 1),
        (vec!["tests/test_a.py"], 1),
    ] {
        let mut argv = vec!["--lf", "--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 1, "{args:?}");
        let records = output_log;
        assert_eq!(finished(&records).len(), 1, "{args:?}");
        assert_eq!(
            records.last().unwrap()["counts"]["deselected"],
            deselected,
            "{args:?}"
        );
    }
}
