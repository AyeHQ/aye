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

mod command_line;
mod details;
mod list;
mod store;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aye_render::Keys;
use aye_test::{Needs, TestContext, code, json_lines, project, runs_dir, stderr};
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
