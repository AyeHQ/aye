//! The binary end to end, on small projects written for each test: how
//! arguments are read, exit codes, and what reaches stdout, the JUnit report
//! and history, one module per theme.
//!
//! Runs with the interpreter aye-test finds, which has pytest, so that aye's
//! own `pytest` module is shown to win over it (aye never imports pytest);
//! some tests also make a venv without pytest. Without one the tests are
//! skipped, loudly. Every run is in aye-test's hermetic environment, where
//! pytest's plugin autoloading is off, so that the plugins the interpreter
//! happens to install (platformdirs ships one) don't add warnings to what a
//! test checks.

mod adapters;
mod arguments;
mod collection_errors;
mod config;
mod engine;
mod junit;
mod maxfail;
mod output;
mod reruns;
mod selection;
mod sessions;
#[cfg(unix)]
mod signals;
mod usage_errors;

#[path = "../common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use aye_test::{Needs, TestContext, as_records, events, stdout};
use serde_json::Value;

use crate::common::{kept, runs_in};

/// aye, as this test target builds it.
fn context() -> TestContext {
    TestContext::new(env!("CARGO_BIN_EXE_aye"))
}

/// The interpreter runs are given: one with pytest.
fn python() -> Option<PathBuf> {
    aye_test::test_python(Needs::Pytest)
}

/// A run with `python`, in `cwd`, in the hermetic environment.
fn aye_command(python: &Path, cwd: &Path, args: &[&str]) -> Command {
    context().run(python, cwd, args)
}

fn aye(python: &Path, cwd: &Path, args: &[&str]) -> Output {
    aye_command(python, cwd, args).output().unwrap()
}

/// The project `cwd` is in: the nearest directory with a `pyproject.toml`.
fn root_of(cwd: &Path) -> &Path {
    cwd.ancestors()
        .find(|dir| dir.join("pyproject.toml").is_file())
        .unwrap_or(cwd)
}

/// Runs aye with `args` in `cwd`, and the run's log: the one it kept, or
/// for a run that keeps none (collect-only, or nothing to show) what its
/// JSON lines say, as records ([`as_records`]; `args` must ask for JSON).
fn logged(python: &Path, cwd: &Path, args: &[&str]) -> (Output, Vec<Value>) {
    let root = root_of(cwd);
    let before = runs_in(root);
    let output = aye(python, cwd, args);
    let log = kept(root, &before).unwrap_or_else(|| as_records(&output));
    (output, log)
}

/// `(id, outcome)` of every `test_finished`.
fn finished(records: &[Value]) -> Vec<(String, String)> {
    events(records, "test_finished")
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["outcome"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

const PASSING: &str = "def test_a():\n    pass\n";

/// The ids `--co --output json` lists, in order.
fn listed(output: &Output) -> Vec<String> {
    events(&as_records(output), "case_collected")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect()
}

/// `aye config`, run in `cwd` with `python`.
fn aye_config(python: &Path, cwd: &Path) -> Output {
    context().config(python, cwd, &[]).output().unwrap()
}

const PYTEST_CONFIG: &str = "[tool.pytest.ini_options]\n";

const DOCTESTS_ON: &str = "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n";

const DOCTEST: &str = "def g():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n";

const SKIPS: &str = "import pytest\npytest.importorskip(\"aye_missing_zz\")\n";

/// The `(severity, path)` of each diagnostic with `code`.
fn diagnosed(records: &[Value], code: &str) -> Vec<(String, String)> {
    events(records, "diagnostic")
        .iter()
        .filter(|d| d["code"] == code)
        .map(|d| {
            (
                d["severity"].as_str().unwrap().to_owned(),
                d["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The text summary line, without its duration: the last line but the
/// details hint.
fn summary(output: &Output) -> String {
    stdout(output)
        .lines()
        .rfind(|line| !line.is_empty() && !line.starts_with("details  "))
        .unwrap_or_default()
        .rsplit_once("   ")
        .map_or("", |(counts, _)| counts)
        .to_owned()
}
