//! The details of the tests named: by key or its start, id, dotted name or
//! path, at each level, with a file changed since the run, a blocked test's
//! setup, and every result of a duplicate id.

use std::fs;

use aye_test::{code, project, stderr, stdout};
use serde_json::Value;

use crate::{
    FAIL, INNER, PARAM_2, TEST_A, count, failures, key, keys, python, run, show, show_json, suite,
    summary, unittest_project,
};

/// The full FNV-1a 64 key of a test id, 16 hex digits.
fn full_key(id: &str) -> String {
    aye_events::TestKey::of(id).to_string()
}

#[test]
fn show_gives_the_details_of_tests_by_key_id_dotted_name_or_path() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let keys = keys(dir.path(), "r1");
    let fail = keys.key(FAIL).unwrap().to_owned();
    assert_eq!(fail.len(), 5);
    assert!(full_key(FAIL).starts_with(&fail));
    // By its key, as the run printed it, its whole hash, its id or its
    // dotted name: its details, and a summary counting it.
    let mut selectors = vec![
        fail.clone(),
        fail.to_ascii_uppercase(),
        full_key(FAIL),
        FAIL.to_owned(),
        "tests.test_a.test_fail".to_owned(),
    ];
    // The start of its key, from 4 digits, when it names it alone.
    if let Some(start) = (4..5)
        .map(|len| full_key(FAIL)[..len].to_owned())
        .find(|start| keys.resolve(start).len() == 1)
    {
        selectors.push(start);
    }
    let first = show_json(dir.path(), &[&fail]);
    assert_eq!(failures(&first), [FAIL]);
    let total = summary(&first);
    assert_eq!((&total["exit"], count(total, "failed")), (&1.into(), 1));
    assert!(
        first[0]["rerun"].as_str().unwrap().ends_with(FAIL),
        "{first:#?}"
    );
    for selector in &selectors {
        assert_eq!(show_json(dir.path(), &[selector]), first, "{selector}");
    }
    // Text: each test named, a failure or not.
    let inner = keys.key(INNER).unwrap();
    let output = show(
        dir.path(),
        &[inner, "tests.test_b.test_ok", "--color", "never"],
    );
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let out = stdout(&output);
    for id in [INNER, "tests/test_b.py::test_ok"] {
        assert!(out.contains(id), "{id}: {out}");
    }
    // A path beside a key selects its tests.
    let lines = show_json(dir.path(), &[&fail, "tests/test_b.py"]);
    assert_eq!(failures(&lines), [FAIL, PARAM_2]);
    let total = summary(&lines);
    assert_eq!((count(total, "failed"), count(total, "passed")), (2, 2));
}

/// Details come at the ladder's second level without `-v` or `-q`: text's
/// `-v`, JSON's `-vv`; the flags mean what they mean in a run.
#[test]
fn show_details_at_each_level() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let fail = key(dir.path(), FAIL);
    let text = |args: &[&str]| {
        let mut all = vec![fail.as_str(), "--color", "never"];
        all.extend(args);
        stdout(&show(dir.path(), &all))
    };
    assert_eq!(text(&[]), text(&["-v"]));
    assert_eq!(text(&[]), text(&["-vv", "-q"]));
    assert!(text(&["-vv"]).contains("computing"), "{}", text(&["-vv"]));
    let lines = |args: &[&str]| {
        let mut all = vec![fail.as_str()];
        all.extend(args);
        show_json(dir.path(), &all)
    };
    assert_eq!(lines(&[]), lines(&["-vv"]));
    assert_ne!(lines(&[]), lines(&["-v"]));
    // -q: the summary alone, in either format.
    assert_eq!(lines(&["-q"]).len(), 1);
    summary(&lines(&["-q"]));
    assert!(!text(&["-q"]).contains(FAIL), "{}", text(&["-q"]));
    // `--failed` too.
    assert_eq!(
        show_json(dir.path(), &["--failed"]),
        show_json(dir.path(), &["--failed", "-vv"])
    );
    // The list stays at a run's default.
    assert_eq!(
        show_json(dir.path(), &[]),
        show_json(dir.path(), &["-v", "-q"])
    );
    assert_eq!(
        stdout(&show(dir.path(), &["--color", "never"])),
        stdout(&show(dir.path(), &["-v", "-q", "--color", "never"]))
    );
}

#[test]
fn show_says_when_a_file_changed_since_the_run() {
    let Some(python) = python() else { return };
    let dir = suite();
    run(&python, dir.path(), &[]);
    let fail = key(dir.path(), FAIL);
    // Unchanged: nothing said.
    let lines = show_json(dir.path(), &[&fail]);
    assert!(lines.iter().all(|line| line["type"] != "stale"));
    fs::write(
        dir.path().join("tests/test_a.py"),
        TEST_A.replace("1 == 2", "1 == 1"),
    )
    .unwrap();
    let lines = show_json(dir.path(), &[&fail]);
    assert_eq!(
        lines[0],
        serde_json::json!({"type": "stale", "path": "tests/test_a.py", "rerun": "aye tests/test_a.py"})
    );
    assert_eq!(failures(&lines), [FAIL]);
    let out = stdout(&show(dir.path(), &[&fail, "--color", "never"]));
    assert!(
        out.starts_with("tests/test_a.py changed since this run; rerun: aye tests/test_a.py\n"),
        "{out}"
    );
    // Only for the files of the tests shown.
    let lines = show_json(dir.path(), &["tests/test_b.py"]);
    assert!(lines.iter().all(|line| line["type"] != "stale"));
    // A file that is gone.
    fs::remove_file(dir.path().join("tests/test_b.py")).unwrap();
    let lines = show_json(dir.path(), &["--failed"]);
    let stale: Vec<&Value> = lines
        .iter()
        .filter(|line| line["type"] == "stale")
        .collect();
    assert_eq!(stale.len(), 2, "{lines:#?}");
    assert_eq!(
        *stale[1],
        serde_json::json!({"type": "stale", "path": "tests/test_b.py", "gone": true})
    );
    let param = key(dir.path(), PARAM_2);
    let out = stdout(&show(dir.path(), &[&param, "--color", "never"]));
    assert!(
        out.starts_with("tests/test_b.py is gone since this run\n"),
        "{out}"
    );
}

/// Two test names whose ids, in `file`, have keys that start with the
/// same 4 digits.
fn alike(file: &str) -> (String, String) {
    let mut seen = std::collections::HashMap::new();
    (0..100_000)
        .find_map(|n| {
            let name = format!("test_{n}");
            let start = full_key(&format!("{file}::{name}"))[..4].to_owned();
            seen.insert(start, name.clone())
                .map(|earlier| (earlier, name))
        })
        .unwrap()
}

/// The start of two tests' keys names neither: 4, with both keys and ids.
#[test]
fn the_start_of_several_keys_lists_them() {
    let Some(python) = python() else { return };
    let (a, b) = alike("tests/test_k.py");
    let dir = project(&[(
        "tests/test_k.py",
        &format!("def {a}():\n    assert 0\n\ndef {b}():\n    pass\n"),
    )]);
    assert_eq!(code(&run(&python, dir.path(), &[])), 1);
    let (a, b) = (
        format!("tests/test_k.py::{a}"),
        format!("tests/test_k.py::{b}"),
    );
    let start = &full_key(&a)[..4];
    let output = show(dir.path(), &[start]);
    assert_eq!(code(&output), 4);
    let err = stderr(&output);
    assert!(
        err.starts_with(&format!(
            "aye: error: {start}: the start of several keys; give more of the one you want:\n"
        )),
        "{err}"
    );
    let keys = keys(dir.path(), "r1");
    for id in [&a, &b] {
        let key = keys.key(id).unwrap();
        assert!(
            err.lines()
                .any(|line| line.split_whitespace().eq([key, id.as_str()])),
            "{err}"
        );
        // Each key names its test.
        assert_eq!(code(&show(dir.path(), &[key])), 0);
    }
    assert_eq!(stdout(&output), "");
}

/// A test a failed `setUpClass` kept from running: its key shows it with
/// the failure that kept it; the class's key, the failure.
#[test]
fn a_blocked_test_is_shown_with_the_setup_that_blocked_it() {
    let Some(python) = python() else { return };
    let dir = unittest_project(&[(
        "tests/test_setup.py",
        "\
import unittest

class Broken(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        raise RuntimeError('no db')

    def test_a(self):
        pass

    def test_b(self):
        pass

class Fine(unittest.TestCase):
    def test_c(self):
        self.fail('c')
",
    )]);
    assert_eq!(code(&run(&python, dir.path(), &[])), 1);
    let class = "tests/test_setup.py::Broken";
    let blocked = key(dir.path(), "tests/test_setup.py::Broken::test_b");
    let lines = show_json(dir.path(), &[&blocked]);
    assert!(failures(&lines).contains(&class.to_owned()), "{lines:#?}");
    assert!(
        !failures(&lines).contains(&"tests/test_setup.py::Fine::test_c".to_owned()),
        "{lines:#?}"
    );
    let text = stdout(&show(dir.path(), &[&blocked, "--color", "never"]));
    assert!(text.contains("test_b"), "{text}");
    assert!(text.contains("no db"), "{text}");
    let lines = show_json(dir.path(), &[&key(dir.path(), class)]);
    assert_eq!(failures(&lines)[0], class, "{lines:#?}");
}

/// A key names every result of its test: a test id that finished twice
/// has both.
#[test]
fn a_key_shows_every_result_of_a_duplicate_id() {
    let Some(python) = python() else { return };
    let dir = unittest_project(&[(
        "tests/test_twice.py",
        "\
import unittest

class T(unittest.TestCase):
    def test_t(self):
        self.fail('boom')

def load_tests(loader, tests, pattern):
    tests.addTests(loader.loadTestsFromTestCase(T))
    return tests
",
    )]);
    let ran = run(&python, dir.path(), &["--color", "never"]);
    assert_eq!(code(&ran), 1, "{}", stderr(&ran));
    let id = "tests/test_twice.py::T::test_t";
    let twice = key(dir.path(), id);
    let lines = show_json(dir.path(), &[&twice]);
    assert_eq!(failures(&lines), [id, id], "{lines:#?}");
    assert_eq!(count(summary(&lines), "failed"), 2);
    // Its id: the same.
    assert_eq!(show_json(dir.path(), &[id]), lines);
}
