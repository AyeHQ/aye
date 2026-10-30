//! The comparison harness (#12): aye against each project's referee, the
//! framework's own runner, on the example suites in `tests/suites` and the
//! example projects in the repository's `examples/`. aye runs every test on
//! its own engine (pytest's with its pytest front-end and its own `pytest`
//! module, never importing pytest); the referee is plain pytest, or plain
//! unittest, as `record.py` ran it.
//!
//! `tests/harness/record.py` records what the referee does (`make expected`
//! re-records everything): a suite's in its `expected.json`, an example's in
//! `tests/examples/<name>.json`. A project's referee follows from its
//! `[tool.aye] adapters`: plain unittest when unittest is the only adapter,
//! else plain pytest (with its doctest plugin for doctests), which, where
//! unittest is among the adapters, must agree with plain unittest on every
//! test unittest finds (`record.py` says how). This test runs aye on a fresh
//! copy of each project and requires the referee's ids, in its order, the
//! same outcome per id and the same exit status, plus, for every file aye
//! could predict without importing, the same ids from static collection
//! alone (`--co --no-refine`, the static recognizer's prediction).
//!
//! A suite runs as `record.py` ran its referee: with its top-level
//! directories as paths when it has no `tests/`. An example runs from its
//! directory with no paths, as its README does, once per recorded run: by
//! default, and with `AYE_EXAMPLE_FAIL=1`, which turns its deliberate
//! failure on. `PYTEST_ADDOPTS` and `AYE_EXAMPLE_FAIL` from the caller's
//! environment never reach a run.
//!
//! Needs a Python with pytest, the referee's, which aye runs with too (its
//! `pytest` module wins over the installed one): the one aye-test finds.
//! Without one the tests are skipped, loudly. aye runs in the referee's
//! environment, not aye-test's hermetic one: only what `record.py` clears
//! is cleared.
//!
//! `native_suites` (ignored: `cargo test -p aye --test compare --
//! --ignored native_suites --nocapture`) runs every suite and example as
//! the tests above do, against the same recordings, but with an
//! interpreter without pytest (`AYE_NATIVE_PYTHON`, else a venv of the base
//! interpreter made for the run) and `AYE_NATIVE_WORKERS` as `-n` if set
//! (`auto`, a count), and prints a table of what agrees. It never fails: it
//! measures.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aye_test::{Needs, as_records, events, examples_dir, venv_without_pytest};
use serde_json::Value;

mod common;

use common::{kept, runs_in};

/// Removed from every run's environment, so the caller's shell can't change
/// a result (`record.py` clears the same).
const CLEARED: [&str; 2] = ["PYTEST_ADDOPTS", "AYE_EXAMPLE_FAIL"];

/// The referee's interpreter, with pytest.
fn python() -> Option<PathBuf> {
    aye_test::test_python(Needs::Pytest)
}

/// A project the harness compares, a suite or an example, by its test's
/// name.
#[derive(Clone, Copy)]
enum Project {
    Suite(&'static str),
    Example(&'static str),
}

/// A project's directory name from its test's: `-` is `_` in a test name.
fn dir_name(test: &str) -> String {
    test.replace('_', "-")
}

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The directories in `of`, sorted.
fn dirs(of: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(of)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

impl Project {
    fn name(self) -> String {
        match self {
            Self::Suite(name) | Self::Example(name) => dir_name(name),
        }
    }

    fn dir(self) -> PathBuf {
        match self {
            Self::Suite(_) => manifest_dir().join("tests/suites").join(self.name()),
            Self::Example(_) => examples_dir().join(self.name()),
        }
    }

    fn recorded(self) -> PathBuf {
        match self {
            Self::Suite(_) => self.dir().join("expected.json"),
            Self::Example(_) => manifest_dir()
                .join("tests/examples")
                .join(format!("{}.json", self.name())),
        }
    }

    fn label(self) -> String {
        match self {
            Self::Suite(_) => format!("suite {}", self.name()),
            Self::Example(_) => format!("example {}", self.name()),
        }
    }

    /// The paths aye gets: none for an example; for a suite, none when it
    /// has `tests/` under its root (pytest's testpaths or the current
    /// directory apply), else its top-level directories, as `expected.py`'s
    /// `default_args` gives the referee.
    fn paths(self, root: &Path) -> Vec<String> {
        if matches!(self, Self::Example(_)) || root.join("tests").is_dir() {
            return Vec::new();
        }
        dirs(root)
    }

    /// Whether the project can be compared here: a packaged crate has no
    /// `examples/` beside it.
    #[expect(clippy::print_stderr, reason = "a test says why it skips")]
    fn present(self) -> bool {
        if matches!(self, Self::Example(_)) && !examples_dir().is_dir() {
            eprintln!("skipping: no examples/ beside the crate");
            return false;
        }
        true
    }
}

/// A fresh copy of a project, so runs never share history or caches, and
/// without the referee's recording.
fn copy_project(project: Project) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    aye_test::copy_project(&project.dir(), dir.path(), &["expected.json"]);
    dir
}

/// One recorded run of the referee.
struct Run {
    /// Variables the run sets.
    env: Vec<(String, String)>,
    /// Ids in the referee's order.
    collected: Vec<String>,
    outcomes: BTreeMap<String, String>,
    exit_code: i64,
}

struct Recorded {
    referee: String,
    runs: Vec<Run>,
}

fn run_of(value: &Value) -> Run {
    let strings = |key: &str| -> Vec<(String, String)> {
        value[key].as_object().map_or_else(Vec::new, |map| {
            map.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                .collect()
        })
    };
    Run {
        env: strings("env"),
        collected: value["collected"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect(),
        outcomes: strings("outcomes").into_iter().collect(),
        exit_code: value["exit_code"]
            .as_i64()
            .expect("exit_code: re-record with `make expected`"),
    }
}

/// A suite's `expected.json` is one run; an example's file lists its runs.
fn recorded(project: Project) -> Recorded {
    let path = project.recorded();
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; record it with `make expected`", path.display()));
    let value: Value = serde_json::from_str(&text).unwrap();
    let runs = value["runs"].as_array().map_or_else(
        || vec![run_of(&value)],
        |runs| runs.iter().map(run_of).collect(),
    );
    Recorded {
        referee: value["referee"].as_str().unwrap().to_string(),
        runs,
    }
}

/// Runs aye on the project copied to `root`, and the run's log: the one
/// it kept, or a collect-only run's JSON lines read as records.
fn aye(
    python: &Path,
    project: Project,
    root: &Path,
    env: &[(String, String)],
    extra: &[&str],
) -> (Vec<Value>, Option<i32>) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aye"));
    command
        .arg("--python")
        .arg(python)
        .args(["--output", "json", "-vv"])
        .args(extra)
        .args(project.paths(root))
        .current_dir(root);
    for name in CLEARED {
        command.env_remove(name);
    }
    command.envs(env.iter().map(|(k, v)| (k, v)));
    let before = runs_in(root);
    let output = command.output().unwrap();
    let stdout = std::str::from_utf8(&output.stdout).unwrap();
    assert!(
        !stdout.trim().is_empty(),
        "aye printed nothing; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = kept(root, &before).unwrap_or_else(|| as_records(&output));
    (records, output.status.code())
}

/// How a run is named in failure messages: its environment, if any.
fn run_label(project: Project, run: &Run) -> String {
    if run.env.is_empty() {
        project.label()
    } else {
        let env: Vec<String> = run.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{} with {}", project.label(), env.join(" "))
    }
}

fn compare_outcomes(project: Project) {
    let Some(python) = python() else { return };
    if !project.present() {
        return;
    }
    let recorded = recorded(project);
    let referee = &recorded.referee;
    for run in &recorded.runs {
        let label = run_label(project, run);
        let dir = copy_project(project);
        let (records, status) = aye(&python, project, dir.path(), &run.env, &[]);
        let actual: BTreeMap<String, String> = events(&records, "test_finished")
            .into_iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap().to_string(),
                    e["outcome"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        let missing: Vec<_> = run
            .outcomes
            .keys()
            .filter(|id| !actual.contains_key(*id))
            .collect();
        let extra: Vec<_> = actual
            .keys()
            .filter(|id| !run.outcomes.contains_key(*id))
            .collect();
        let differ: Vec<_> = run
            .outcomes
            .iter()
            .filter_map(|(id, want)| {
                let got = actual.get(id)?;
                (got != want).then(|| format!("{id}: {referee} {want}, aye {got}"))
            })
            .collect();
        assert!(
            missing.is_empty() && extra.is_empty() && differ.is_empty(),
            "{label}:\n  missing from aye: {missing:#?}\n  only in aye: {extra:#?}\n  different outcome: {differ:#?}"
        );
        assert_eq!(
            status.map(i64::from),
            Some(run.exit_code),
            "{label}: aye's exit status differs from {referee}'s"
        );
    }
}

fn compare_collection(project: Project) {
    let Some(python) = python() else { return };
    if !project.present() {
        return;
    }
    let recorded = recorded(project);
    let referee = &recorded.referee;
    for run in &recorded.runs {
        let label = run_label(project, run);
        let dir = copy_project(project);

        // With refinement: exactly the referee's ids, in its order.
        let (records, _) = aye(&python, project, dir.path(), &run.env, &["--collect-only"]);
        let collected: Vec<String> = events(&records, "case_collected")
            .into_iter()
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect();
        let want: BTreeSet<&String> = run.collected.iter().collect();
        let got: BTreeSet<&String> = collected.iter().collect();
        let missing: Vec<_> = want.difference(&got).collect();
        let extra: Vec<_> = got.difference(&want).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{label}: collected ids differ from {referee}'s\n  missing from aye: {missing:#?}\n  only in aye: {extra:#?}"
        );
        let first = collected
            .iter()
            .zip(&run.collected)
            .position(|(a, b)| a != b);
        if first.is_some() || collected.len() != run.collected.len() {
            let at = first.unwrap_or_else(|| collected.len().min(run.collected.len()));
            panic!(
                "{label}: collected ids are in another order than {referee}'s; from position \
                 {at}:\n  aye: {:#?}\n  {referee}: {:#?}",
                &collected[at..collected.len().min(at + 5)],
                &run.collected[at..run.collected.len().min(at + 5)],
            );
        }

        // Static only, on a fresh copy so no cached refinement helps: every
        // exact prediction is right, and files that needed no import are
        // predicted completely.
        let fresh = copy_project(project);
        let (records, _) = aye(
            &python,
            project,
            fresh.path(),
            &run.env,
            &["--collect-only", "--no-refine"],
        );
        let unrefined: BTreeSet<String> = events(&records, "diagnostic")
            .into_iter()
            .filter(|d| d["code"] == "not-refined")
            .map(|d| d["path"].as_str().unwrap().to_string())
            .collect();
        let exact: BTreeSet<String> = events(&records, "case_collected")
            .into_iter()
            .filter(|e| e["certainty"] == "exact")
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect();
        let wrong: Vec<_> = exact.iter().filter(|id| !want.contains(id)).collect();
        assert!(
            wrong.is_empty(),
            "{label}: static predictions {referee} doesn't collect: {wrong:#?}"
        );
        let missed: Vec<_> = run
            .collected
            .iter()
            .filter(|id| {
                !unrefined.contains(id.split("::").next().unwrap()) && !exact.contains(*id)
            })
            .collect();
        assert!(
            missed.is_empty(),
            "{label}: tests static collection missed: {missed:#?}"
        );
    }
}

/// The suites and examples compared, one test each for outcomes and for
/// collection, named as their directories but with `_` for `-`.
macro_rules! compared {
    (suites: $($suite:ident),* ; examples: $($example:ident),* $(;)?) => {
        const SUITES: &[&str] = &[$(stringify!($suite)),*];
        const EXAMPLES: &[&str] = &[$(stringify!($example)),*];

        mod outcomes {
            $(#[test] fn $suite() {
                crate::compare_outcomes(crate::Project::Suite(stringify!($suite)));
            })*
        }
        mod collection {
            $(#[test] fn $suite() {
                crate::compare_collection(crate::Project::Suite(stringify!($suite)));
            })*
        }
        mod examples {
            mod outcomes {
                $(#[test] fn $example() {
                    crate::compare_outcomes(crate::Project::Example(stringify!($example)));
                })*
            }
            mod collection {
                $(#[test] fn $example() {
                    crate::compare_collection(crate::Project::Example(stringify!($example)));
                })*
            }
        }
    };
}

compared! {
    suites: outcomes, params, structure, dynamic, rootdir, unittest, doctest,
        pytest_doctest, unittest_pytest;
    examples: pytest, unittest, doctest;
}

fn listed(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|name| dir_name(name)).collect();
    names.sort();
    names
}

#[test]
fn every_suite_is_compared() {
    assert_eq!(
        dirs(&manifest_dir().join("tests/suites")),
        listed(SUITES),
        "add new suites to compared!"
    );
}

#[test]
fn every_example_is_compared() {
    let examples = examples_dir();
    if !examples.is_dir() {
        eprintln!("skipping: no examples/ beside the crate");
        return;
    }
    assert_eq!(
        dirs(&examples),
        listed(EXAMPLES),
        "add new examples to compared!"
    );
    let mut recorded: Vec<String> = fs::read_dir(manifest_dir().join("tests/examples"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    recorded.sort();
    let want: Vec<String> = listed(EXAMPLES)
        .iter()
        .map(|name| format!("{name}.json"))
        .collect();
    assert_eq!(
        recorded, want,
        "tests/examples holds one recording per example (`make expected`)"
    );
}

// ---------------------------------------------------------------- without pytest

/// An interpreter without pytest for `native_suites`: `AYE_NATIVE_PYTHON`,
/// else a venv of `python`'s base interpreter in `dir` ([`venv_without_pytest`]),
/// else `python` itself (aye's `pytest` module shadows the real one all the
/// same).
#[expect(
    clippy::disallowed_methods,
    reason = "the tests' own settings, not the environment aye reads"
)]
fn native_python(python: &Path, dir: &Path) -> (PathBuf, &'static str) {
    if let Some(given) = std::env::var_os("AYE_NATIVE_PYTHON") {
        return (PathBuf::from(given), "AYE_NATIVE_PYTHON");
    }
    match venv_without_pytest(python, dir) {
        Some(venv) => (venv, "a venv without pytest"),
        None => (
            python.to_path_buf(),
            "the interpreter with pytest (no venv without it)",
        ),
    }
}

/// How one recorded run went with `python`: `None` when it agrees with the
/// referee, else why not, in a few words.
fn native_run(python: &Path, workers: Option<&str>, project: Project, run: &Run) -> Option<String> {
    let dir = copy_project(project);
    let mut command = Command::new(env!("CARGO_BIN_EXE_aye"));
    command
        .arg("--python")
        .arg(python)
        .args(["--output", "json", "-vv"])
        .args(workers.into_iter().flat_map(|n| ["-n", n]))
        .args(project.paths(dir.path()))
        .current_dir(dir.path());
    for name in CLEARED {
        command.env_remove(name);
    }
    command.envs(run.env.iter().map(|(k, v)| (k, v)));
    let before = runs_in(dir.path());
    let output = match command.output() {
        Ok(output) => output,
        Err(err) => return Some(format!("aye didn't start: {err}")),
    };
    let records: Vec<Value> = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    if records.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Some(format!(
            "no output: {}",
            stderr.lines().next().unwrap_or("(nothing on stderr)")
        ));
    }
    let actual: BTreeMap<String, String> = events(&records, "test_finished")
        .into_iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap_or_default().to_string(),
                e["outcome"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let collected: Vec<String> = events(&records, "case_collected")
        .into_iter()
        .map(|e| e["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut problems = Vec::new();
    let missing = run
        .outcomes
        .keys()
        .filter(|id| !actual.contains_key(*id))
        .count();
    let extra = actual
        .keys()
        .filter(|id| !run.outcomes.contains_key(*id))
        .count();
    let differ: Vec<String> = run
        .outcomes
        .iter()
        .filter_map(|(id, want)| {
            let got = actual.get(id)?;
            (got != want).then(|| format!("{id}: want {want}, got {got}"))
        })
        .collect();
    if missing > 0 {
        problems.push(format!("{missing} missing"));
    }
    if extra > 0 {
        problems.push(format!("{extra} extra"));
    }
    if !differ.is_empty() {
        let first = differ.first().cloned().unwrap_or_default();
        problems.push(format!("{} differ (first: {first})", differ.len()));
    }
    if collected != run.collected && missing == 0 && extra == 0 {
        problems.push("collection order differs".into());
    }
    let status = output.status.code().map(i64::from);
    if status != Some(run.exit_code) {
        problems.push(format!(
            "exit {} (want {})",
            status.map_or_else(|| "signal".to_string(), |s| s.to_string()),
            run.exit_code
        ));
    }
    if !problems.is_empty() {
        let errors: Vec<String> = events(&records, "diagnostic")
            .into_iter()
            .filter(|d| d["severity"] == "error")
            .take(1)
            .map(|d| {
                let message = d["message"].as_str().unwrap_or_default();
                let line = message.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                let line: String = line.trim().chars().take(120).collect();
                format!("{}: {line}", d["code"].as_str().unwrap_or_default())
            })
            .collect();
        problems.extend(errors.into_iter().map(|e| format!("first error {e}")));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// Every suite and example on an interpreter without pytest, at any
/// worker count, against the referee's recordings: a table, never a
/// failure (see the module's docs).
#[test]
#[ignore = "measures aye's engine without pytest, at AYE_NATIVE_WORKERS; run with --ignored"]
#[expect(
    clippy::disallowed_methods,
    reason = "the tests' own settings, not the environment aye reads"
)]
fn native_suites() {
    let Some(python) = python() else { return };
    let venvs = tempfile::tempdir().unwrap();
    let (native, which) = native_python(&python, venvs.path());
    let workers = std::env::var("AYE_NATIVE_WORKERS").ok();
    let mut projects: Vec<Project> = SUITES.iter().copied().map(Project::Suite).collect();
    if examples_dir().is_dir() {
        projects.extend(EXAMPLES.iter().copied().map(Project::Example));
    }
    let mut rows = Vec::new();
    for project in projects {
        let recorded = recorded(project);
        for run in &recorded.runs {
            let label = run_label(project, run);
            let tests = run.outcomes.len();
            let verdict = native_run(&native, workers.as_deref(), project, run);
            rows.push((label, tests, verdict));
        }
    }
    let width = rows
        .iter()
        .map(|(label, ..)| label.len())
        .max()
        .unwrap_or(0);
    let passed = rows.iter().filter(|(.., v)| v.is_none()).count();
    println!();
    println!("aye's engine against the referees, with {which}:");
    println!("{}", native.display());
    if let Some(workers) = &workers {
        println!("-n {workers} (AYE_NATIVE_WORKERS)");
    }
    for (label, tests, verdict) in &rows {
        match verdict {
            None => println!("  {label:<width$}  {tests:>4} tests  pass"),
            Some(why) => println!("  {label:<width$}  {tests:>4} tests  FAIL  {why}"),
        }
    }
    println!("{passed}/{} runs agree", rows.len());
}
