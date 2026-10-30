//! The Python names that are contracts (`docs/reorganisation.md` §7.2), as
//! user code sees them when the binary runs it: aye's `pytest` module's
//! identity, its `__all__`, its `_pytest` aliases and its errors for what
//! it lacks; the `__module__` and `__qualname__` of every class `pytest`
//! names, and of every object aye hands user code (builtin fixtures,
//! `Config`, `Cache`, requests, nodes, what the conftest hooks get), with
//! their default reprs; and the qualified names that reach a failure's
//! `detail`. Moving aye's Python must keep every one.
//!
//! The project `fixtures/contracts` records what it sees in its own
//! `observed.json`, which must equal `fixtures/contracts.json`, recorded
//! with aye 0.0.1. Runs with the interpreter aye-test finds, which has
//! pytest, so that aye's own module is shown to win over it; without one
//! the tests are skipped, loudly.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use aye_events::{Event, Outcome, TestResult};
use aye_test::{Needs, TestContext, copy_project, runs_dir, test_python};
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The contracts project, run once in a fresh copy: the copy, and its
/// stored log's results by test name.
fn run(python: &Path) -> (tempfile::TempDir, BTreeMap<String, TestResult>) {
    let dir = tempfile::tempdir().unwrap();
    copy_project(&fixtures().join("contracts"), dir.path(), &[]);
    let output = TestContext::new(env!("CARGO_BIN_EXE_aye"))
        .run(python, dir.path(), &["-q"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{text}");
    let store = aye_history::RunStore::new(runs_dir(dir.path()));
    let last = store.last().expect("a stored run");
    let results = store
        .log(&last.run.to_string())
        .unwrap()
        .into_iter()
        .filter_map(|record| match record.event {
            Event::TestFinished(result) => {
                let name = result.id.as_str().rsplit("::").next().unwrap().to_owned();
                Some((name, *result))
            }
            _ => None,
        })
        .collect();
    (dir, results)
}

#[test]
fn the_names_user_code_sees_are_unchanged() {
    let Some(python) = test_python(Needs::Pytest) else {
        return;
    };
    let (dir, results) = run(&python);
    assert_eq!(results.len(), 17, "{:?}", results.keys());
    for (name, result) in &results {
        let expected = match name.as_str() {
            "test_aye_skip_is_a_skip" => Outcome::Skipped,
            "test_missing_fixture_through_request" | "test_fails" => Outcome::Failed,
            "test_xfail_not_run" => Outcome::ExpectedFailure,
            _ => Outcome::Passed,
        };
        assert_eq!(result.outcome, expected, "{name}: {result:?}");
    }
    let read = |path: &Path| -> BTreeMap<String, Value> {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    };
    let observed = read(&dir.path().join("observed.json"));
    let expected = read(&fixtures().join("contracts.json"));
    let mut differences = Vec::new();
    for (name, value) in &expected {
        match observed.get(name) {
            Some(seen) if seen == value => {}
            Some(seen) => differences.push(format!("{name}: {value} became {seen}")),
            None => differences.push(format!("{name}: {value} is no longer seen")),
        }
    }
    for (name, seen) in &observed {
        if !expected.contains_key(name) {
            differences.push(format!("{name}: {seen} is new"));
        }
    }
    assert!(differences.is_empty(), "{}", differences.join("\n"));
}

/// What `detail` names, and what the engine reports, of aye's own classes:
/// `FixtureLookupError` with its module, `pytest.fail`'s `Failed` without
/// one (its module is `builtins`, as pytest's), `xfail(run=False)`'s
/// reason, and a skip raised with `aye_engine.Skip`.
#[test]
fn the_qualified_names_in_failures_are_unchanged() {
    let Some(python) = test_python(Needs::Pytest) else {
        return;
    };
    let (_dir, results) = run(&python);
    let failure = |name: &str| results[name].failure().unwrap().clone();
    let lookup = failure("test_missing_fixture_through_request");
    assert_eq!(lookup.exception.as_deref(), Some("FixtureLookupError"));
    let detail = lookup.detail.unwrap();
    assert!(
        detail.contains(
            "\naye_native_engine.FixtureLookupError: fixture 'no_such_fixture' not found\n"
        ),
        "{detail}"
    );
    let failed = failure("test_fails");
    assert_eq!(failed.exception.as_deref(), Some("Failed"));
    let detail = failed.detail.unwrap();
    assert!(detail.ends_with("\nFailed: boom\n"), "{detail}");
    assert_eq!(
        results["test_xfail_not_run"].reason.as_deref(),
        Some("[NOTRUN] not run")
    );
    assert_eq!(
        results["test_aye_skip_is_a_skip"].reason.as_deref(),
        Some("skipped by aye's own class")
    );
}
