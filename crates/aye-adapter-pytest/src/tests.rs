//! Recognition tests. Expected ids are copied from `pytest --collect-only -q`
//! on pytest 9.1.1; the differential test re-checks them against a real
//! pytest when one is available.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aye_adapter::{Adapter, Claim, LineIndex, Project, Recognition, Recognizer, SourceFile};
use aye_events::Certainty;

use super::*;

/// A parsed file, owning what a `SourceFile` borrows.
struct Parsed {
    rel: String,
    abs: PathBuf,
    source: String,
    ast: ruff_python_parser::Parsed<aye_adapter::ast::ModModule>,
    lines: LineIndex,
}

impl Parsed {
    fn read(root: &Path, rel: &str) -> Self {
        let abs = root.join(rel);
        let source = std::fs::read_to_string(&abs).unwrap();
        Self::new(rel, abs, source)
    }

    fn new(rel: &str, abs: PathBuf, source: String) -> Self {
        let ast = ruff_python_parser::parse_module(&source).unwrap();
        let lines = LineIndex::from_source_text(&source);
        Self {
            rel: rel.to_string(),
            abs,
            source,
            ast,
            lines,
        }
    }

    fn file(&self) -> SourceFile<'_> {
        SourceFile {
            rel: aye_events::ProjectPathRef::as_written(&self.rel),
            abs: &self.abs,
            source: &self.source,
            module: self.ast.syntax(),
            lines: &self.lines,
        }
    }
}

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

fn recognizer(root: &Path) -> PytestRecognizer {
    PytestRecognizer::with_env(
        &Project {
            root,
            cwd: root,
            args: &[],
            config_file: None,
            framework_args: &[],
        },
        None,
    )
    .unwrap()
}

/// Recognize every test file under `root` the way aye-collect would walk
/// it (and pytest would order it): entries sorted by name, directories
/// recursed in place.
fn collect(root: &Path) -> Vec<(String, Recognition)> {
    let recognizer = recognizer(root);
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    let mut out = Vec::new();
    for start in recognizer.roots(&project) {
        walk(&recognizer, root, &start, &mut out);
    }
    out
}

fn walk(
    recognizer: &PytestRecognizer,
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Recognition)>,
) {
    if dir.is_file() {
        let rel = dir
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        recognize_file(recognizer, root, &rel, Requested::Found, out);
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    for path in entries {
        if path.is_dir() {
            if recognizer.descend(&path) {
                walk(recognizer, root, &path, out);
            }
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if recognizer.claim(&rel, Requested::Found).has_tests() {
            recognize_file(recognizer, root, &rel, Requested::Found, out);
        }
    }
}

fn recognize_file(
    recognizer: &PytestRecognizer,
    root: &Path,
    rel: &str,
    requested: Requested,
    out: &mut Vec<(String, Recognition)>,
) {
    let file = Parsed::read(root, rel);
    let mut conftests = Vec::new();
    let parent = file.abs.parent().unwrap().to_path_buf();
    let mut dirs: Vec<&Path> = parent
        .ancestors()
        .take_while(|d| d.starts_with(root))
        .collect();
    dirs.reverse();
    for dir in dirs {
        if dir.join("conftest.py").is_file() {
            let rel = dir
                .join("conftest.py")
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            conftests.push(Parsed::read(root, &rel));
        }
    }
    let support: Vec<SourceFile<'_>> = conftests.iter().map(Parsed::file).collect();
    out.push((
        rel.to_string(),
        recognizer.recognize(&file.file(), requested, &support),
    ));
}

/// Ids of one source file, recognized with the default configuration.
fn ids(source: &str) -> Vec<String> {
    recognize(source)
        .cases
        .into_iter()
        .map(|c| c.id.into_string())
        .collect()
}

fn recognize(source: &str) -> Recognition {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "pytest.ini", "[pytest]\n");
    let recognizer = recognizer(dir.path());
    let file = Parsed::new(
        "test_x.py",
        dir.path().join("test_x.py"),
        source.to_string(),
    );
    recognizer.recognize(&file.file(), Requested::Found, &[])
}

/// The interpreter for differential tests: `AYE_TEST_PYTHON`, or the
/// workspace's `.venv`, when it has pytest.
fn python() -> Option<PathBuf> {
    let candidate = std::env::var_os("AYE_TEST_PYTHON")
        .map(PathBuf::from)
        .or_else(|| {
            let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv");
            [venv.join("bin/python"), venv.join("Scripts/python.exe")]
                .into_iter()
                .find(|p| p.is_file())
        })?;
    let ok = Command::new(&candidate)
        .args(["-c", "import pytest"])
        .output()
        .is_ok_and(|o| o.status.success());
    ok.then_some(candidate)
}

/// `pytest --collect-only -q` in `root`: the node ids, in order, and the
/// files pytest failed to collect.
fn pytest_ids(python: &Path, root: &Path) -> (Vec<String>, Vec<String>) {
    let output = Command::new(python)
        .args([
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-rsE",
            "-p",
            "no:cacheprovider",
            "--continue-on-collection-errors",
        ])
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // Read as UTF-8, as the worker's: Windows' default is the ANSI code
        // page.
        .env("PYTHONIOENCODING", "utf-8")
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ids = stdout
        .lines()
        .take_while(|line| !line.is_empty())
        .filter(|line| line.contains("::"))
        .map(str::to_string)
        .collect();
    // Files that failed, or skipped themselves while being collected.
    let errors = stdout
        .lines()
        .filter_map(|line| {
            if let Some(rest) = line.strip_prefix("ERROR ") {
                let path = rest.split(" - ").next().unwrap_or(rest);
                return Some(path.split("::").next().unwrap_or(path).to_string());
            }
            let rest = line.strip_prefix("SKIPPED [")?;
            let location = rest.split_once("] ")?.1;
            Some(location.split(':').next()?.to_string())
        })
        .collect();
    (ids, errors)
}

/// A pytest plugin that dumps each item's marks (in `iter_markers` order)
/// and the names `-k` matches.
const DUMP_PLUGIN: &str = r#"
import json
import os

from _pytest.mark import KeywordMatcher


def pytest_collection_modifyitems(session, config, items):
    out = []
    for item in items:
        tags = []
        for mark in item.iter_markers():
            if mark.name not in tags:
                tags.append(mark.name)
        names = sorted(KeywordMatcher.from_item(item)._names)
        out.append({"id": item.nodeid, "tags": tags, "keywords": names})
    with open(os.environ["AYE_DUMP"], "w", encoding="utf-8") as f:
        json.dump(out, f)
"#;

/// Each item's id, tags and keywords, as pytest sees them.
fn pytest_metadata(python: &Path, root: &Path) -> Vec<(String, Vec<String>, Vec<String>)> {
    let plugin = tempfile::tempdir().unwrap();
    write(plugin.path(), "aye_dump.py", DUMP_PLUGIN);
    let dump = plugin.path().join("dump.json");
    let status = Command::new(python)
        .args([
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
            "-p",
            "aye_dump",
        ])
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", plugin.path())
        .env("AYE_DUMP", &dump)
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap();
    assert!(
        dump.is_file(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    items
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap().to_string(),
                strings(&item["tags"]),
                strings(&item["keywords"]),
            )
        })
        .collect()
}

/// Compare tags and keywords of exact cases with pytest's.
fn differential_metadata(python: &Path, root: &Path, excluded: &[String]) -> Result<(), String> {
    let theirs = pytest_metadata(python, root);
    let mut report = String::new();
    for (rel, recognition) in collect(root) {
        if excluded.contains(&rel) {
            continue;
        }
        for case in &recognition.cases {
            if case.certainty != Certainty::Exact {
                continue;
            }
            let Some((_, tags, keywords)) = theirs.iter().find(|(id, ..)| *id == case.id.as_str())
            else {
                let _ = writeln!(report, "{}: not collected by pytest", case.id);
                continue;
            };
            if &case.tags != tags {
                let _ = writeln!(report, "{}: tags {:?} != {:?}", case.id, case.tags, tags);
            }
            let ours: std::collections::BTreeSet<&str> =
                case.keywords.iter().map(String::as_str).collect();
            let expected: std::collections::BTreeSet<&str> =
                keywords.iter().map(String::as_str).collect();
            let missing: Vec<_> = expected.difference(&ours).collect();
            // The unparametrized name is extra, and harmless: `-k`
            // matches substrings. So is `__wrapped__` for a decorator that
            // does not use `functools.wraps`.
            let extra: Vec<_> = ours
                .difference(&expected)
                .filter(|k| {
                    !case.id.as_str().ends_with(']') || !case.id.without_params().ends_with(*k)
                })
                .filter(|k| **k != "__wrapped__")
                .collect();
            if !missing.is_empty() || !extra.is_empty() {
                let _ = writeln!(
                    report,
                    "{}: keywords missing {missing:?}, extra {extra:?}",
                    case.id
                );
            }
        }
    }
    if report.is_empty() {
        Ok(())
    } else {
        Err(report)
    }
}

/// Compare recognition with pytest under `root`: every file not refined
/// (and that pytest could import) must match exactly, in order. Returns
/// the files left out.
fn differential(python: &Path, root: &Path) -> Result<Vec<String>, String> {
    let (expected, errors) = pytest_ids(python, root);
    let found = collect(root);
    let refined: Vec<String> = found
        .iter()
        .filter(|(rel, r)| {
            r.refine.is_some()
                || errors
                    .iter()
                    .any(|e| rel == e || rel.starts_with(&format!("{e}/")))
        })
        .map(|(rel, _)| rel.clone())
        .collect();
    let ours: Vec<String> = found
        .iter()
        .filter(|(rel, _)| !refined.contains(rel))
        .flat_map(|(_, r)| r.cases.iter().map(|c| c.id.as_str().to_owned()))
        .collect();
    let theirs: Vec<String> = expected
        .into_iter()
        .filter(|id| {
            let file = id.split("::").next().unwrap_or_default();
            !refined.iter().any(|r| r == file)
        })
        .collect();
    if ours == theirs {
        Ok(refined)
    } else {
        let mut report = String::new();
        let width = ours.len().max(theirs.len());
        for i in 0..width {
            let a = ours.get(i).map_or("", String::as_str);
            let b = theirs.get(i).map_or("", String::as_str);
            let mark = if a == b { "  " } else { "!=" };
            let _ = writeln!(report, "{mark} {a:<60} {b}");
        }
        Err(format!("aye (left) vs pytest (right):\n{report}"))
    }
}

/// Compare with pytest on a directory of your own:
/// `AYE_PYTEST_DIFF_DIR=/tmp/suite cargo test -p aye-adapter-pytest -- --ignored --nocapture diff_dir`.
#[test]
#[ignore = "manual: needs AYE_PYTEST_DIFF_DIR and a Python with pytest"]
fn diff_dir() {
    let dir = PathBuf::from(std::env::var_os("AYE_PYTEST_DIFF_DIR").expect("AYE_PYTEST_DIFF_DIR"));
    let python = python().expect("a Python with pytest");
    for (rel, recognition) in collect(&dir) {
        if let Some(reason) = &recognition.refine {
            println!("refined {rel}: {reason}");
        }
        for case in &recognition.cases {
            if case.certainty != Certainty::Exact {
                println!("  {:?} {}", case.certainty, case.id);
            }
        }
        for diagnostic in &recognition.diagnostics {
            println!("  {}: {}", diagnostic.code, diagnostic.message);
        }
    }
    let excluded = match differential(&python, &dir) {
        Ok(excluded) => {
            let found = collect(&dir);
            let compared: Vec<_> = found
                .iter()
                .filter(|(rel, _)| !excluded.contains(rel))
                .collect();
            let ids: usize = compared.iter().map(|(_, r)| r.cases.len()).sum();
            println!(
                "identical: {ids} ids in {} files ({} files refined or not collected by pytest: {excluded:?})",
                compared.len(),
                excluded.len()
            );
            excluded
        }
        Err(report) => panic!("{report}"),
    };
    if let Err(report) = differential_metadata(&python, &dir, &excluded) {
        panic!("{report}");
    }
    println!("tags and keywords identical");
}

#[test]
fn adapter_faces() {
    let adapter = Pytest::default();
    assert_eq!(adapter.name(), "pytest");
    let executor = adapter.executor();
    assert_eq!(executor.adapter, "pytest");
    // aye's engine, with the pytest front-end: no module drives pytest.
    assert_eq!(executor.entry, "aye_pytest_native:NativeExecutor");
    assert!(
        executor
            .modules
            .iter()
            .all(|module| module.name != "aye_engine_pytest")
    );
    let dir = tempfile::tempdir().unwrap();
    let project = Project {
        root: dir.path(),
        cwd: dir.path(),
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    assert!(adapter.prepare(&project).unwrap().is_some());
}

mod p1;
mod p2;
mod p3;
mod suites;

fn materialize(suite: &suites::Suite) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in suite.files {
        write(dir.path(), rel, body);
    }
    dir
}

#[test]
fn suites_match_pytest() {
    for suite in suites::SUITES {
        let dir = materialize(suite);
        let found = collect(dir.path());
        let refined: Vec<&str> = found
            .iter()
            .filter(|(_, r)| r.refine.is_some())
            .map(|(rel, _)| rel.as_str())
            .collect();
        assert_eq!(
            refined, suite.refined,
            "suite {}: refined files",
            suite.name
        );
        let ours: Vec<&str> = found
            .iter()
            .filter(|(_, r)| r.refine.is_none())
            .flat_map(|(_, r)| r.cases.iter().map(|c| c.id.as_str()))
            .collect();
        let theirs: Vec<&str> = suite
            .pytest
            .iter()
            .copied()
            .filter(|id| {
                !suite
                    .refined
                    .contains(&id.split("::").next().unwrap_or_default())
            })
            .collect();
        assert_eq!(ours, theirs, "suite {}", suite.name);
    }
}

/// The same suites against the pytest at hand, tags and keywords included.
#[test]
fn suites_match_live_pytest() {
    let Some(python) = python() else {
        eprintln!("skipped: no Python with pytest (set AYE_TEST_PYTHON)");
        return;
    };
    for suite in suites::SUITES {
        let dir = materialize(suite);
        let excluded = differential(&python, dir.path())
            .unwrap_or_else(|report| panic!("suite {}: {report}", suite.name));
        differential_metadata(&python, dir.path(), &excluded)
            .unwrap_or_else(|report| panic!("suite {}: {report}", suite.name));
    }
}

fn case<'r>(recognition: &'r Recognition, id: &str) -> &'r aye_events::Case {
    recognition
        .cases
        .iter()
        .find(|c| c.id.as_str() == id)
        .unwrap_or_else(|| {
            panic!(
                "no case {id} in {:?}",
                recognition
                    .cases
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

#[test]
fn pinned_ids() {
    // Each expected list is `pytest --collect-only -q` on pytest 9.1.1.
    assert_eq!(
        ids(
            "import pytest\n@pytest.mark.parametrize('x', [0, 1])\n@pytest.mark.parametrize('y', [2, 3])\ndef test_s(x, y): pass\n"
        ),
        [
            "test_x.py::test_s[2-0]",
            "test_x.py::test_s[2-1]",
            "test_x.py::test_s[3-0]",
            "test_x.py::test_s[3-1]"
        ]
    );
    assert_eq!(
        ids(
            "class TestB:\n    def test_b2(self): pass\n    def test_b1(self): pass\nclass TestC(TestB):\n    def test_c(self): pass\n    def test_b1(self): pass\n"
        ),
        [
            "test_x.py::TestB::test_b2",
            "test_x.py::TestB::test_b1",
            "test_x.py::TestC::test_b2",
            "test_x.py::TestC::test_c",
            "test_x.py::TestC::test_b1"
        ]
    );
    assert_eq!(
        ids("def test_dup(): pass\ndef test_other(): pass\ndef test_dup(): pass\n"),
        ["test_x.py::test_dup", "test_x.py::test_other"]
    );
    assert_eq!(
        ids("import pytest\n@pytest.mark.parametrize('x', [])\ndef test_empty(x): pass\n"),
        ["test_x.py::test_empty[NOTSET]"]
    );
}

#[test]
fn cases_carry_metadata() {
    let recognition = recognize(
        r#"
import pytest

pytestmark = pytest.mark.integration

@pytest.mark.db
class TestCart:
    @pytest.mark.slow
    @pytest.mark.parametrize("currency", ["eur", pytest.param("usd", marks=pytest.mark.xfail)])
    def test_add(self, currency):
        pass
"#,
    );
    assert_eq!(recognition.refine, None);
    let usd = case(&recognition, "test_x.py::TestCart::test_add[usd]");
    assert_eq!(usd.adapter, "pytest");
    assert_eq!(
        usd.tags,
        ["parametrize", "slow", "xfail", "db", "integration"]
    );
    assert_eq!(
        usd.keywords,
        [
            "test_x.py",
            "TestCart",
            "test_add",
            "test_add[usd]",
            "pytestmark",
            "parametrize",
            "slow",
            "xfail",
            "db",
            "integration"
        ]
    );
    // The module's import name first: files sharing it can't share a
    // session, and the scheduler keeps them in one.
    assert_eq!(
        usd.affinity,
        ["import:test_x", "test_x.py", "test_x.py::TestCart"]
    );
    assert_eq!(usd.certainty, Certainty::Exact);
    let span = usd.span.as_ref().unwrap();
    assert_eq!((span.path.as_str(), span.line), ("test_x.py", Some(10)));
    let eur = case(&recognition, "test_x.py::TestCart::test_add[eur]");
    assert_eq!(eur.tags, ["parametrize", "slow", "db", "integration"]);
}

#[test]
fn timeouts_come_from_the_closest_mark() {
    // pytest 9.1.1's `get_closest_marker("timeout")` on the same file.
    let recognition = recognize(
        r#"
import pytest
pytestmark = pytest.mark.timeout(1)

@pytest.mark.timeout(2.5)
@pytest.mark.parametrize("t", [pytest.param(1, marks=pytest.mark.timeout(9))])
def test_own(t): pass

@pytest.mark.parametrize("t", [pytest.param(1, marks=pytest.mark.timeout(9)), 2])
def test_param(t): pass

@pytest.mark.timeout(3)
class TestC:
    def test_class(self): pass
    @pytest.mark.parametrize("t", [pytest.param(1, marks=pytest.mark.timeout(timeout=7))])
    def test_class_param(self, t): pass

def test_module(): pass
"#,
    );
    let timeouts: Vec<(&str, Option<f64>)> = recognition
        .cases
        .iter()
        .map(|c| (c.id.as_str(), c.timeout))
        .collect();
    assert_eq!(
        timeouts,
        [
            ("test_x.py::test_own[1]", Some(2.5)),
            ("test_x.py::test_param[1]", Some(9.0)),
            ("test_x.py::test_param[2]", Some(1.0)),
            ("test_x.py::TestC::test_class", Some(3.0)),
            ("test_x.py::TestC::test_class_param[1]", Some(7.0)),
            ("test_x.py::test_module", Some(1.0)),
        ]
    );
}

#[test]
fn runtime_parametrize_is_refined_with_a_partial_guess() {
    let recognition = recognize(
        r#"
import pytest
from data import CASES

@pytest.mark.parametrize("x", CASES)
def test_imported(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=str)
def test_ids_callable(x): pass

@pytest.mark.parametrize("x", [n for n in range(3)])
def test_comprehension(x): pass

def test_static(): pass
"#,
    );
    let refine = recognition.refine.as_deref().unwrap();
    assert!(
        refine.contains("test_imported: parametrize argvalues are not literal"),
        "{refine}"
    );
    assert!(
        refine.contains("test_ids_callable: parametrize ids are computed at runtime"),
        "{refine}"
    );
    let certainties: Vec<(&str, Certainty)> = recognition
        .cases
        .iter()
        .map(|c| (c.id.as_str(), c.certainty))
        .collect();
    assert_eq!(
        certainties,
        [
            ("test_x.py::test_imported", Certainty::Partial),
            ("test_x.py::test_ids_callable", Certainty::Partial),
            ("test_x.py::test_comprehension", Certainty::Partial),
            ("test_x.py::test_static", Certainty::Exact),
        ]
    );
}

#[test]
fn unknown_decorators_and_marks_are_refined() {
    // A decorator aye can't see through may parametrize, rename, replace
    // or multiply the test (ddt, `parameterized.expand`, registries), and
    // an unread mark may be a `parametrize`: only pytest knows the ids.
    let recognition = recognize(
        r#"
import pytest
from unittest import mock
from helpers import retry, MARKS

@retry
def test_retry(): pass

@mock.patch("os.getcwd")
def test_patched(getcwd): pass

class TestMarked:
    pytestmark = MARKS
    def test_m(self): pass
"#,
    );
    let refine = recognition.refine.as_deref().unwrap();
    assert!(
        refine.contains("test_retry has a decorator aye can't see through"),
        "{refine}"
    );
    assert!(
        refine.contains("TestMarked's marks can't be read"),
        "{refine}"
    );
    assert!(!refine.contains("test_patched"), "{refine}");
    let certainty = |id: &str| case(&recognition, id).certainty;
    assert_eq!(certainty("test_x.py::test_retry"), Certainty::Partial);
    assert_eq!(certainty("test_x.py::test_patched"), Certainty::Exact);
    assert_eq!(
        certainty("test_x.py::TestMarked::test_m"),
        Certainty::Partial
    );
    assert!(
        case(&recognition, "test_x.py::test_patched")
            .keywords
            .iter()
            .any(|k| k == "patchings")
    );
    // Hypothesis keeps the test and its ids; it only leaves attributes.
    let recognition = recognize(
        "from hypothesis import given, strategies as st\n@given(st.integers())\ndef test_prop(x): pass\n",
    );
    assert_eq!(recognition.refine, None);
    assert_eq!(
        case(&recognition, "test_x.py::test_prop").certainty,
        Certainty::Partial
    );
}

#[test]
fn diagnostics() {
    let recognition = recognize(
        r"
class TestInit:
    def __init__(self): pass
    def test_never(self): pass

def test_twice(): pass
def test_twice(): pass

def test_generator():
    yield 1
",
    );
    let codes: Vec<(&str, u32)> = recognition
        .diagnostics
        .iter()
        .map(|d| {
            (
                d.code.as_str(),
                d.span.as_ref().and_then(|s| s.line).unwrap(),
            )
        })
        .collect();
    // A generator test is pytest's collection error, which refinement
    // reports: one error, not a static one besides.
    assert_eq!(codes, [("class-has-init", 2), ("duplicate-definition", 6)]);
    assert_eq!(
        recognition.diagnostics[0].message,
        "cannot collect test class 'TestInit' because it has a __init__ constructor (from: test_x.py)"
    );
    assert!(recognition.refine.as_deref().unwrap().contains("generator"));
}

#[test]
fn ids_are_relative_to_the_project_root() {
    // pytest's rootdir is the parent holding pytest.ini; aye's project root
    // is the subproject. pytest 9.1.1 reports
    // `sub/tests/test_a.py::test_a[1]`.
    let dir = tempfile::tempdir().unwrap();
    let outer = dir.path();
    write(outer, "pytest.ini", "[pytest]\naddopts = -n 2 -ra\n");
    write(
        outer,
        "conftest.py",
        "def pytest_collection_modifyitems(items): pass\n",
    );
    write(outer, "sub/pyproject.toml", "[project]\nname = 'sub'\n");
    let root = outer.join("sub");
    let recognizer = PytestRecognizer::with_env(
        &Project {
            root: &root,
            cwd: &root,
            args: &[],
            config_file: None,
            framework_args: &[],
        },
        None,
    )
    .unwrap();
    assert_eq!(recognizer.config().rootdir, outer);
    let file = Parsed::new(
        "tests/test_a.py",
        root.join("tests/test_a.py"),
        "import pytest\n@pytest.mark.parametrize('x', [1])\ndef test_a(x): pass\n".into(),
    );
    let recognition = recognizer.recognize(&file.file(), Requested::Found, &[]);
    let found = case(&recognition, "tests/test_a.py::test_a[1]");
    assert_eq!(found.keywords[..3], ["sub", "tests", "test_a.py"]);
    // The conftest above the project root is in scope.
    assert_eq!(found.certainty, Certainty::Partial);
    assert_eq!(
        recognizer.context().into_value(),
        serde_json::json!({
            "rootdir": outer.to_string_lossy(),
            "inifile": outer.join("pytest.ini").to_string_lossy(),
            "xdist_workers": "2",
            "maxfail": null,
            "roots": [root.to_string_lossy()],
            "junitxml": null,
            "junit_prefix": null,
            "junit_suite_name": "pytest",
            "junit_logging": "no",
            "junit_log_passing_tests": true,
            "junit_duration_report": "total",
            "junit_family": "xunit2",
            "keyword": null,
            "tags": null,
            "deselect": [],
            "env_plugins": [],
            "colliding_modules": [],
        })
    );
    assert_eq!(recognizer.config().args, ["-ra"]);
}

#[test]
fn roots_follow_testpaths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\ntestpaths = tests integration* missing\n",
    );
    for d in ["tests", "integration_a", "integration_b", "other"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    let recognizer = recognizer(root);
    let at = |cwd: &Path, args: &[PathBuf]| {
        recognizer.roots(&Project {
            root,
            cwd,
            args,
            config_file: None,
            framework_args: &[],
        })
    };
    assert_eq!(
        at(root, &[]),
        [
            root.join("tests"),
            root.join("integration_a"),
            root.join("integration_b")
        ]
    );
    // Not run from the rootdir: the current directory.
    assert_eq!(at(&root.join("other"), &[]), [root.join("other")]);
    assert_eq!(at(root, &[PathBuf::from("other")]), [root.join("other")]);

    write(
        root,
        "pytest.ini",
        "[pytest]\ntestpaths = tests pkg.mod\naddopts = --pyargs\n",
    );
    write(root, "pkg/mod.py", "");
    let recognizer = PytestRecognizer::new(&Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    })
    .unwrap();
    assert_eq!(
        recognizer.roots(&Project {
            root,
            cwd: root,
            args: &[],
            config_file: None,
            framework_args: &[],
        }),
        [root.join("tests"), root.join("pkg/mod.py")]
    );
}

#[test]
fn descend_and_claim() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\naddopts = --ignore=skipped\n");
    write(root, "env/pyvenv.cfg", "");
    write(root, "conda/conda-meta/history", "");
    write(
        root,
        "conftest.py",
        "import sys\ncollect_ignore = ['old', 'test_legacy.py']\nif sys.platform == 'win32':\n    collect_ignore.append('test_posix.py')\ncollect_ignore_glob = ['*_py2.py']\n",
    );
    std::fs::create_dir_all(root.join("old")).unwrap();
    let recognizer = recognizer(root);
    for (dir, expected) in [
        ("tests", true),
        ("__pycache__", false),
        (".git", false),
        ("build", false),
        ("pkg.egg", false),
        ("{arch}", false),
        ("arch", true),
        ("env", false),
        ("conda", false),
        ("old", false),
        ("skipped", false),
    ] {
        assert_eq!(recognizer.descend(&root.join(dir)), expected, "{dir}");
    }
    for (rel, explicit, expected) in [
        ("conftest.py", false, Claim::Support),
        ("tests/conftest.py", false, Claim::Support),
        ("test_a.py", false, Claim::Tests),
        ("a_test.py", false, Claim::Tests),
        ("helper.py", false, Claim::Ignore),
        ("helper.py", true, Claim::Tests),
        ("test_a.txt", true, Claim::Ignore),
        ("test_legacy.py", false, Claim::Ignore),
        ("test_legacy.py", true, Claim::Tests),
        ("test_old_py2.py", false, Claim::Ignore),
        ("test_posix.py", false, Claim::Tests),
    ] {
        let requested = if explicit {
            Requested::Named
        } else {
            Requested::Found
        };
        assert_eq!(recognizer.claim(rel, requested), expected, "{rel}");
    }
    // A conditional collect_ignore entry: only pytest knows.
    let file = Parsed::new(
        "test_posix.py",
        root.join("test_posix.py"),
        "def test_p(): pass\n".into(),
    );
    let conftest = Parsed::read(root, "conftest.py");
    let recognition = recognizer.recognize(&file.file(), Requested::Found, &[conftest.file()]);
    assert!(recognition.refine.unwrap().contains("collect_ignore"));
}

#[test]
fn plugins_named_by_conftests_are_in_scope() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(root, "conftest.py", "pytest_plugins = ['support.params']\n");
    write(root, "support/__init__.py", "");
    write(
        root,
        "support/params.py",
        "def pytest_generate_tests(metafunc): pass\n",
    );
    let recognizer = recognizer(root);
    let file = Parsed::new(
        "test_a.py",
        root.join("test_a.py"),
        "def test_a(x): pass\n".into(),
    );
    let conftest = Parsed::read(root, "conftest.py");
    let recognition = recognizer.recognize(&file.file(), Requested::Found, &[conftest.file()]);
    assert!(
        recognition
            .refine
            .unwrap()
            .contains("pytest_generate_tests")
    );
}

#[test]
fn settings_and_context() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pyproject.toml",
        "[tool.pytest.ini_options]\npython_files = [\"check_*.py\"]\naddopts = \"--numprocesses=auto --dist loadscope -q\"\n",
    );
    let configured = recognizer(root);
    let settings = configured.settings();
    let get = |name: &str| settings.iter().find(|s| s.name == name).unwrap();
    assert_eq!(get("python_files").value, "'check_*.py'");
    assert_eq!(
        get("python_files").origin,
        "pyproject.toml [tool.pytest.ini_options]"
    );
    assert_eq!(get("python_classes").value, "Test");
    assert_eq!(get("python_classes").origin, "default");
    assert_eq!(get("rootdir").value, root.display().to_string());
    assert_eq!(configured.config().args, ["-q"]);
    let context = configured.context().into_value();
    assert_eq!(context["xdist_workers"], "auto");
    assert_eq!(
        context["inifile"],
        root.join("pyproject.toml").to_string_lossy().as_ref()
    );

    let bare = tempfile::tempdir().unwrap();
    let bare = recognizer(bare.path());
    let context = bare.context().into_value();
    assert_eq!(context["inifile"], serde_json::Value::Null);
    assert_eq!(context["xdist_workers"], serde_json::Value::Null);
    assert_eq!(bare.config().args, [] as [String; 0]);
}

/// Recognize `rel` under `root` with its conftests, the way aye-collect
/// hands them over.
fn recognize_at(root: &Path, rel: &str) -> Recognition {
    let mut out = Vec::new();
    recognize_file(&recognizer(root), root, rel, Requested::Found, &mut out);
    out.pop().unwrap().1
}

fn certainties(recognition: &Recognition) -> Vec<(&str, Certainty)> {
    recognition
        .cases
        .iter()
        .map(|c| (c.id.as_str(), c.certainty))
        .collect()
}

#[test]
fn pytestmark_list_operations_are_read() {
    // pytest 9.1.1: test_t[3], test_t[4]; TestK::test_k[5-3], [5-4].
    let recognition = recognize(
        r#"
import pytest
pytestmark = [pytest.mark.slow]
pytestmark.append(pytest.mark.parametrize("b", [3, 4]))

def test_t(b): pass

class TestK:
    def test_k(self, b): pass
"#,
    );
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [
            ("test_x.py::test_t[3]", Certainty::Exact),
            ("test_x.py::test_t[4]", Certainty::Exact),
            ("test_x.py::TestK::test_k[3]", Certainty::Exact),
            ("test_x.py::TestK::test_k[4]", Certainty::Exact),
        ]
    );
    // pytest 9.1.1: TestK::test_k[5].
    let recognition = recognize(
        r#"
import pytest
class TestK:
    pytestmark = []
    pytestmark += [pytest.mark.parametrize("c", [5])]
    pytestmark.extend([pytest.mark.xfail])
    def test_k(self, c): pass
"#,
    );
    assert_eq!(recognition.refine, None);
    let found = case(&recognition, "test_x.py::TestK::test_k[5]");
    assert_eq!(found.certainty, Certainty::Exact);
    assert_eq!(found.tags, ["parametrize", "xfail"]);
}

#[test]
fn marks_aye_cannot_read_are_refined() {
    // Each of these may hold a parametrize, so ids are pytest's to tell.
    for source in [
        "import pytest\ndef make(): return []\npytestmark = make()\ndef test_t(b): pass\n",
        "import pytest\nBASE = [pytest.mark.slow]\npytestmark = BASE + [pytest.mark.parametrize('b', [3])]\ndef test_t(b): pass\n",
        "import pytest\nEXTRA = []\npytestmark = [pytest.mark.slow, *EXTRA]\ndef test_t(b): pass\n",
        "from helpers import MARKS\npytestmark = MARKS\ndef test_t(b): pass\n",
        "import pytest\npytestmark = [pytest.mark.slow]\nif True:\n    pytestmark.append(pytest.mark.parametrize('b', [3]))\ndef test_t(b): pass\n",
        "import pytest\ndef test_t(b): pass\ntest_t.pytestmark = [pytest.mark.parametrize('b', [3])]\n",
        "import pytest\ndef deco(f):\n    return pytest.mark.parametrize('b', [3])(f)\n@deco\ndef test_t(b): pass\n",
        "import pytest\ndef deco(f):\n    pytest.mark.parametrize('b', [3])(f)\n    return f\n@deco\ndef test_t(b): pass\n",
        "import pytest\ndef cases(*v): return pytest.mark.parametrize('b', v)\n@cases(3, 4)\ndef test_t(b): pass\n",
        "class Wrap:\n    def __init__(self, f): self.f = f\n@Wrap\ndef test_t(): pass\n",
    ] {
        let recognition = recognize(source);
        assert!(recognition.refine.is_some(), "{source}");
        assert!(
            recognition
                .cases
                .iter()
                .all(|c| c.certainty == Certainty::Partial),
            "{source}"
        );
    }
    // A local decorator that visibly hands back the function, or a
    // `functools.wraps` wrapper of it, changes nothing.
    let recognition = recognize(
        "import functools\ndef ident(f):\n    return f\ndef wrap(f):\n    @functools.wraps(f)\n    def inner(*a, **k):\n        return f(*a, **k)\n    return inner\n@wrap\n@ident\ndef test_t(): pass\n",
    );
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [("test_x.py::test_t", Certainty::Exact)]
    );
}

#[test]
fn opaque_decorators_on_classes_and_unittest_methods_are_refined() {
    // ddt and `parameterized.expand` replace methods with generated ones.
    let recognition = recognize(
        "import unittest\nfrom ddt import ddt, data\n@ddt\nclass TestThing(unittest.TestCase):\n    def test_plain(self): pass\n",
    );
    assert!(recognition.refine.unwrap().contains("class decorator"));
    let recognition = recognize(
        "import unittest\nfrom parameterized import parameterized\nclass TestP(unittest.TestCase):\n    @parameterized.expand([1, 2])\n    def test_x(self, v): pass\n",
    );
    assert!(
        recognition
            .refine
            .unwrap()
            .contains("TestP.test_x has a decorator aye can't see through")
    );
}

#[test]
fn decorated_generators_are_left_to_pytest() {
    // pytest checks the decorated object: a `functools.wraps` wrapper or a
    // `contextmanager` is a plain function, and runs.
    let recognition = recognize(
        "import contextlib\nfrom drive import inline_callbacks\n@inline_callbacks\ndef test_driven():\n    yield 1\n@contextlib.contextmanager\ndef test_cm():\n    yield 1\n",
    );
    assert!(
        recognition.diagnostics.is_empty(),
        "{:?}",
        recognition.diagnostics
    );
    assert!(recognition.refine.is_some());
    assert_eq!(
        certainties(&recognition),
        [
            ("test_x.py::test_driven", Certainty::Partial),
            ("test_x.py::test_cm", Certainty::Partial),
        ]
    );
    // Marks leave the function a generator: still pytest's collection
    // error, left to refinement to report.
    let recognition = recognize("import pytest\n@pytest.mark.slow\ndef test_gen():\n    yield 1\n");
    assert_eq!(recognition.diagnostics, [] as [aye_events::Diagnostic; 0]);
    assert!(
        recognition
            .refine
            .as_deref()
            .is_some_and(|r| r.contains("test_gen is a generator"))
    );
}

#[test]
fn conditional_definitions_and_deletions_are_refined() {
    let recognition = recognize(
        r#"
import sys
def test_a(): pass
def test_b(): pass
if sys.platform == "win32":
    del test_b
    def test_windows(): pass
class TestK:
    def test_k(self): pass
    if sys.platform == "win32":
        del test_k
try:
    import optional_dependency
    def test_needs_it(): pass
except ImportError:
    pass
try:
    def test_in_try(): pass
except Exception:
    pass
"#,
    );
    let refine = recognition.refine.as_deref().unwrap();
    for reason in [
        "test_b may be deleted at runtime",
        "test_windows is defined conditionally",
        "test_k may be deleted at runtime",
        "test_needs_it is defined conditionally",
    ] {
        assert!(refine.contains(reason), "{reason}: {refine}");
    }
    assert!(!refine.contains("test_in_try"), "{refine}");
    let certainty = |id: &str| case(&recognition, id).certainty;
    assert_eq!(certainty("test_x.py::test_a"), Certainty::Exact);
    assert_eq!(certainty("test_x.py::test_in_try"), Certainty::Exact);
    for id in [
        "test_x.py::test_b",
        "test_x.py::test_windows",
        "test_x.py::TestK::test_k",
        "test_x.py::test_needs_it",
    ] {
        assert_eq!(certainty(id), Certainty::Partial, "{id}");
    }
}

#[test]
fn classes_with_unknown_bases_are_refined_unless_harmless() {
    // A subclass of an imported unittest base inherits its tests, whatever
    // its name (pytest 9.1.1 collects UsersAPICase::test_get).
    for source in [
        "from base import BaseAPICase\nclass UsersAPICase(BaseAPICase):\n    endpoint = '/users'\n",
        "import base\nclass Users(base.BaseAPICase):\n    pass\n",
        "from base import BaseAPICase\nclass Local(BaseAPICase): pass\nclass Users(Local): pass\n",
    ] {
        assert!(recognize(source).refine.is_some(), "{source}");
    }
    // Classes from packages that ship no test cases change nothing.
    let recognition = recognize(
        "import collections\nfrom pydantic import BaseModel\nclass Helper(collections.UserDict): pass\nclass Model(BaseModel): pass\ndef test_ok(): pass\n",
    );
    assert_eq!(recognition.refine, None);
}

#[test]
fn class_members_added_at_runtime_are_refined() {
    for (source, reason) in [
        (
            "def add(cls):\n    cls.test_added = lambda self: None\n    return cls\n@add\nclass TestD:\n    def test_d(self): pass\n",
            "class decorator",
        ),
        (
            "class Meta(type): pass\nclass TestM(metaclass=Meta):\n    def test_m(self): pass\n",
            "TestM has a metaclass",
        ),
        (
            "class Base:\n    def __init_subclass__(cls): pass\nclass TestS(Base):\n    def test_s(self): pass\n",
            "Base defines __init_subclass__",
        ),
        (
            "class TestL:\n    exec('def test_l(self): pass')\n    def test_p(self): pass\n",
            "TestL's body binds names at runtime",
        ),
        (
            "class TestA:\n    def test_a(self): pass\ndef extra(self): pass\nTestA.test_extra = extra\n",
            "the module sets TestA.test_extra",
        ),
    ] {
        let refine = recognize(source).refine.unwrap_or_default();
        assert!(refine.contains(reason), "{source}: {refine}");
    }
    // An abstract base class is no reason.
    let recognition =
        recognize("import abc\nclass TestB(metaclass=abc.ABCMeta):\n    def test_b(self): pass\n");
    assert_eq!(recognition.refine, None);
}

#[test]
fn protocols_abstract_properties_and_function_test_cases() {
    // pytest 9.1.1: Protocol classes and their subclasses have an
    // `__init__`; abstractproperty and abstractclassmethod make a class
    // abstract; FunctionTestCase brings a `runTest`.
    let recognition = recognize(
        r"
import abc, typing, unittest
class TestProto(typing.Protocol):
    def test_p(self): ...
class Base(typing.Protocol):
    def m(self): ...
class TestFromProto(Base):
    def test_fp(self): pass
class TestAbs(abc.ABC):
    @abc.abstractproperty
    def prop(self): pass
    def test_b(self): pass
class TestAbsClass(abc.ABC):
    @abc.abstractclassmethod
    def c(cls): pass
    def test_c(self): pass
class Wrapped(unittest.FunctionTestCase):
    pass
def test_ok(): pass
",
    );
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [
            ("test_x.py::Wrapped::runTest", Certainty::Exact),
            ("test_x.py::test_ok", Certainty::Exact),
        ]
    );
    let codes: Vec<&str> = recognition
        .diagnostics
        .iter()
        .map(|d| d.code.as_str())
        .collect();
    assert_eq!(codes, ["class-has-init", "class-has-init"]);
    // Imported, it is collected too.
    assert!(
        recognize("from unittest import FunctionTestCase\n")
            .refine
            .is_some()
    );
}

#[test]
fn duplicate_ids_are_both_recognized() {
    // pytest 9.1.1 collects 4 items, two of them `test_cross[b-c-a]`;
    // what to make of the duplicate is collection's business.
    assert_eq!(
        ids(
            "import pytest\n@pytest.mark.parametrize('x', ['a', 'c-a'])\n@pytest.mark.parametrize('y', ['b-c', 'b'])\ndef test_cross(x, y): pass\n"
        ),
        [
            "test_x.py::test_cross[b-c-a]",
            "test_x.py::test_cross[b-c-c-a]",
            "test_x.py::test_cross[b-a]",
            "test_x.py::test_cross[b-c-a]",
        ]
    );
}

#[test]
fn imported_fixtures_are_followed_into_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "fixmod.py",
        "import pytest\n@pytest.fixture(params=[1, 2])\ndef pf(request): return request.param\n@pytest.fixture\ndef plain(): pass\n@pytest.fixture(name='named', params=[3])\ndef _named(request): return request.param\n",
    );
    write(root, "reexport.py", "from fixmod import pf as again\n");
    // Star imports, explicit imports, re-exports and explicit names.
    for (conftest, test, refined) in [
        ("from fixmod import *\n", "def test_a(pf): pass\n", true),
        (
            "from fixmod import plain\n",
            "def test_a(plain): pass\n",
            false,
        ),
        ("from fixmod import *\n", "def test_a(plain): pass\n", false),
        (
            "from reexport import again\n",
            "def test_a(again): pass\n",
            true,
        ),
        (
            "from fixmod import _named as other\n",
            "def test_a(named): pass\n",
            true,
        ),
        ("", "from fixmod import pf\ndef test_a(pf): pass\n", true),
        (
            "",
            "from fixmod import plain\ndef test_a(plain): pass\n",
            false,
        ),
    ] {
        write(root, "conftest.py", conftest);
        write(root, "test_a.py", test);
        let recognition = recognize_at(root, "test_a.py");
        assert_eq!(recognition.refine.is_some(), refined, "{conftest} / {test}");
        if !refined {
            assert_eq!(
                certainties(&recognition),
                [("test_a.py::test_a", Certainty::Exact)],
                "{conftest} / {test}"
            );
        }
    }
    // From a module that is not a project file: maybe parametrized.
    write(
        root,
        "conftest.py",
        "from installed_plugin.fixtures import *\n",
    );
    write(root, "test_a.py", "def test_a(pf): pass\n");
    let recognition = recognize_at(root, "test_a.py");
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [("test_a.py::test_a", Certainty::Partial)]
    );
}

#[test]
fn hooks_through_star_imports_registrations_and_plugin_options() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "idhooks.py",
        "def pytest_make_parametrize_id(config, val, argname):\n    return f'S{val}'\n",
    );
    write(
        root,
        "genhooks.py",
        "def pytest_generate_tests(metafunc): pass\n",
    );
    let test = "import pytest\n@pytest.mark.parametrize('x', [3])\ndef test_a(x, dyn): pass\n";
    for conftest in [
        "from idhooks import *\n",
        "from genhooks import *\n",
        "import idhooks\ndef pytest_configure(config):\n    config.pluginmanager.register(idhooks, 'idhooks')\n",
        "def pytest_configure(config):\n    config.pluginmanager.import_plugin('genhooks')\n",
        "def pytest_configure(config):\n    config.pluginmanager.register(make())\n",
        "pytest_plugins = compute()\n",
    ] {
        write(root, "conftest.py", conftest);
        write(root, "test_a.py", test);
        assert!(
            recognize_at(root, "test_a.py").refine.is_some(),
            "{conftest}"
        );
    }
    // A test module that loads a plugin: its own tests, and those of any
    // module collected after it, which the plugin's global hooks reach.
    std::fs::remove_file(root.join("conftest.py")).unwrap();
    write(
        root,
        "test_a.py",
        "pytest_plugins = ['idhooks']\ndef test_a(): pass\n",
    );
    write(root, "test_b.py", test);
    assert!(recognize_at(root, "test_a.py").refine.is_some());
    assert!(
        recognize_at(root, "test_b.py")
            .refine
            .unwrap()
            .contains("pytest_make_parametrize_id")
    );
    // `-p NAME` in addopts loads a project module before anything else.
    std::fs::remove_file(root.join("test_a.py")).unwrap();
    write(root, "pytest.ini", "[pytest]\naddopts = -p genhooks\n");
    assert!(
        recognize_at(root, "test_b.py")
            .refine
            .unwrap()
            .contains("pytest_generate_tests")
    );
}

#[test]
fn blocked_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = -p no:unittest -p no:cacheprovider\n",
    );
    write(
        root,
        "test_a.py",
        "import unittest\nclass TestCase1(unittest.TestCase):\n    def test_a(self): pass\nclass Other(unittest.TestCase):\n    def test_o(self): pass\ndef test_plain(): pass\n",
    );
    // pytest 9.1.1: only test_plain, and a class-has-init warning.
    let recognition = recognize_at(root, "test_a.py");
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [("test_a.py::test_plain", Certainty::Exact)]
    );
    assert_eq!(
        recognition.diagnostics[0].code,
        DiagnosticCode::ClassHasInit
    );
    write(root, "pytest.ini", "[pytest]\naddopts = -p no:python\n");
    assert_eq!(
        recognize_at(root, "test_a.py").cases,
        [] as [aye_events::Case; 0]
    );
}

/// `-p no:python`: nothing collects a Python file, not even one named on
/// the command line, for which `pytest -p no:python test_a.py` says "found
/// no collectors" (status 4): aye's `unclaimed`, not "no tests ran" (5).
/// A conftest still loads, and pytest's doctest plugin still collects
/// modules for their doctests.
#[test]
fn python_files_are_nobodys_with_the_python_plugin_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\naddopts = -p no:python\n");
    write(root, "test_a.py", "def test_a(): pass\n");
    write(root, "conftest.py", "");
    let blocked = recognizer(root);
    assert_eq!(blocked.claim("test_a.py", Requested::Named), Claim::Ignore);
    assert_eq!(blocked.claim("test_a.py", Requested::Found), Claim::Ignore);
    assert_eq!(
        blocked.claim("conftest.py", Requested::Named),
        Claim::Support
    );
    let with_doctests =
        recognizer_with(root, &[], &["--doctest-modules"], None).with_pytest_doctests();
    assert_eq!(
        with_doctests.claim("test_a.py", Requested::Named),
        Claim::Tests
    );
    write(root, "pytest.ini", "[pytest]\n");
    assert_eq!(
        recognizer(root).claim("test_a.py", Requested::Named),
        Claim::Tests
    );
}

#[test]
fn the_first_affinity_key_is_the_import_name() {
    // pytest's prepend and append modes import a rootless file by its
    // basename, so tests/a/test_util.py and tests/b/test_util.py clash
    // ("import file mismatch"); a package makes the name dotted;
    // importlib mode never clashes.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(root, "tests/a/test_util.py", "def test_a(): pass\n");
    write(root, "tests/b/test_util.py", "def test_b(): pass\n");
    write(root, "pkg/__init__.py", "");
    write(root, "pkg/sub/__init__.py", "");
    write(root, "pkg/sub/test_util.py", "def test_c(): pass\n");
    write(root, "my-dir/__init__.py", "");
    write(root, "my-dir/test_dash.py", "def test_d(): pass\n");
    let first = |rel: &str| recognize_at(root, rel).cases[0].affinity.clone();
    assert_eq!(
        first("tests/a/test_util.py"),
        ["import:test_util", "tests/a/test_util.py"]
    );
    assert_eq!(first("tests/b/test_util.py")[0], "import:test_util");
    assert_eq!(first("pkg/sub/test_util.py")[0], "import:pkg.sub.test_util");
    // Not an identifier: not a package to pytest.
    assert_eq!(first("my-dir/test_dash.py")[0], "import:test_dash");

    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --import-mode=importlib\n",
    );
    assert_eq!(first("tests/a/test_util.py")[0], "import:tests.a.test_util");
    assert_eq!(first("pkg/sub/test_util.py")[0], "import:pkg.sub.test_util");

    write(
        root,
        "pytest.ini",
        "[pytest]\nconsider_namespace_packages = true\n",
    );
    assert_eq!(first("pkg/sub/test_util.py")[0], "import:test_util");
}

#[test]
fn context_lifts_maxfail_and_reads_pytest_addopts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = -x -k 'not slow' -ra\n",
    );
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    let plain = recognizer(root);
    let context = plain.context().into_value();
    assert_eq!(context["maxfail"], 1);
    assert_eq!(plain.config().args, ["-k", "not slow", "-ra"]);
    assert_eq!(context["keyword"], "not slow");
    let with_env =
        PytestRecognizer::with_env(&project, Some("--maxfail=3 -n auto -m db -v")).unwrap();
    assert_eq!(
        with_env.config().args,
        ["-k", "not slow", "-ra", "-m", "db", "-v"]
    );
    let with_env = with_env.context().into_value();
    assert_eq!(with_env["maxfail"], 3);
    assert_eq!(with_env["xdist_workers"], "auto");
    assert_eq!(with_env["tags"], "db");
    let unlimited = PytestRecognizer::with_env(&project, Some("--maxfail 0"))
        .unwrap()
        .context()
        .into_value();
    assert_eq!(unlimited["maxfail"], serde_json::Value::Null);
    assert_eq!(unlimited["deselect"], serde_json::json!([]));
}

#[test]
fn context_names_deselect_prefixes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(
        root,
        "sub/pytest.ini",
        "[pytest]\naddopts = --cov=pkg --deselect tests/test_a.py::test_b\n",
    );
    write(
        root,
        "sub/tests/test_a.py",
        "def test_a(): pass\ndef test_b(): pass\n",
    );
    let recognizer = recognizer_with(root, &["sub"], &[], None);
    let context = recognizer.context().into_value();
    // pytest's prefixes are relative to the rootdir, aye's ids to the
    // project root.
    assert_eq!(
        context["deselect"],
        serde_json::json!(["sub/tests/test_a.py::test_b"])
    );
    // The recognizer drops none of the cases they name.
    let recognition = recognize_with(&recognizer, root, "sub/tests/test_a.py");
    assert_eq!(
        certainties(&recognition),
        [
            ("sub/tests/test_a.py::test_a", Certainty::Exact),
            ("sub/tests/test_a.py::test_b", Certainty::Exact)
        ]
    );
}

#[test]
fn param_marks_that_raise_are_refined() {
    for marks in [
        "frozenset([pytest.mark.a])",
        "{pytest.mark.a}",
        "[pytest.mark.a, {pytest.mark.b}]",
        "[pytest.mark.a, [pytest.mark.b]]",
        "\"slow\"",
        "1",
        "None",
        "[helper]",
        "set([pytest.mark.a])",
    ] {
        let source = format!(
            "import pytest\ndef helper(): pass\n@pytest.mark.parametrize(\"v\", [pytest.param(1, marks={marks})])\ndef test_v(v): pass\n"
        );
        let recognition = recognize(&source);
        assert!(recognition.refine.is_some(), "{marks}");
    }
    // Empty collections, marks and names that may hold marks are read.
    let recognition = recognize(
        "import pytest\nfrom helpers import SLOW\n@pytest.mark.parametrize(\"v\", [pytest.param(1, marks=()), pytest.param(2, marks=set()), pytest.param(3, marks=[pytest.mark.a, SLOW])])\ndef test_v(v): pass\n",
    );
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [
            ("test_x.py::test_v[1]", Certainty::Partial),
            ("test_x.py::test_v[2]", Certainty::Partial),
            ("test_x.py::test_v[3]", Certainty::Partial),
        ]
    );
}

#[test]
fn pytest_addopts_reach_static_collection() {
    // `-o` id options and `--ignore` in `PYTEST_ADDOPTS` apply as pytest
    // applies them.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(root, "tests/b/test_b.py", "def test_b(): pass\n");
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    let recognizer = PytestRecognizer::with_env(
        &project,
        Some("-o disable_test_id_escaping_and_forfeit_all_rights_to_community_support=true --ignore=tests/b"),
    )
    .unwrap();
    assert!(!recognizer.descend(&root.join("tests/b")));
    let file = Parsed::new(
        "tests/test_e.py",
        root.join("tests/test_e.py"),
        "import pytest\n@pytest.mark.parametrize('s', ['\u{e9}'])\ndef test_s(s): pass\n".into(),
    );
    assert_eq!(
        recognizer
            .recognize(&file.file(), Requested::Found, &[])
            .cases[0]
            .id
            .as_str(),
        "tests/test_e.py::test_s[\u{e9}]"
    );
}

#[test]
fn project_base_classes_are_read() {
    // A test module's helper classes that derive from project classes are
    // refined only when those may be unittest cases.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(root, "app/__init__.py", "");
    write(
        root,
        "app/models.py",
        "import collections\nclass Base: pass\nclass Model(Base, collections.UserDict): pass\n",
    );
    write(
        root,
        "tests/base.py",
        "import unittest\nclass BaseAPICase(unittest.TestCase):\n    def test_get(self): pass\nclass Plain: pass\n",
    );
    write(
        root,
        "tests/reexport.py",
        "from base import BaseAPICase as Case\n",
    );
    write(
        root,
        "tests/opaque.py",
        "from installed import Thing\nclass Wrapper(Thing): pass\n",
    );
    for (source, refined) in [
        (
            "from app.models import Model\nclass Factory(Model): pass\n",
            false,
        ),
        (
            "import app.models\nclass Factory(app.models.Model): pass\n",
            false,
        ),
        ("from base import Plain\nclass Fake(Plain): pass\n", false),
        (
            "from base import BaseAPICase\nclass Users(BaseAPICase): pass\n",
            true,
        ),
        ("from reexport import Case\nclass Users(Case): pass\n", true),
        (
            "from opaque import Wrapper\nclass Users(Wrapper): pass\n",
            true,
        ),
    ] {
        write(
            root,
            "tests/test_a.py",
            &format!("{source}def test_ok(): pass\n"),
        );
        assert_eq!(
            recognize_at(root, "tests/test_a.py").refine.is_some(),
            refined,
            "{source}"
        );
    }
}

/// A recognizer for `root` run from `root` with these command-line paths,
/// arguments after `--` and `PYTEST_ADDOPTS`.
fn recognizer_with(
    root: &Path,
    args: &[&str],
    framework_args: &[&str],
    env: Option<&str>,
) -> PytestRecognizer {
    let args: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let framework_args: Vec<String> = framework_args.iter().map(|s| (*s).to_string()).collect();
    PytestRecognizer::with_env(
        &Project {
            root,
            cwd: root,
            args: &args,
            config_file: None,
            framework_args: &framework_args,
        },
        env,
    )
    .unwrap()
}

fn recognize_with(recognizer: &PytestRecognizer, root: &Path, rel: &str) -> Recognition {
    recognize_as(recognizer, root, rel, Requested::Found)
}

/// [`recognize_with`], for a file claimed as `requested` says.
fn recognize_as(
    recognizer: &PytestRecognizer,
    root: &Path,
    rel: &str,
    requested: Requested,
) -> Recognition {
    let mut out = Vec::new();
    recognize_file(recognizer, root, rel, requested, &mut out);
    out.pop().unwrap().1
}

#[test]
fn context_carries_the_framework_args_and_the_junit_report() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --junitxml=a.xml -ra\n",
    );
    let with_args = recognizer_with(
        root,
        &[],
        &[
            "--junitxml",
            "out/report.xml",
            "-x",
            "-n",
            "3",
            "--reuse-db",
            "-k",
            "fast",
        ],
        Some("--junit-prefix ci -m slow --junit-xml=env.xml -v"),
    );
    assert_eq!(
        with_args.config().args,
        ["-ra", "-m", "slow", "-v", "--reuse-db", "-k", "fast"]
    );
    let context = with_args.context().into_value();
    assert_eq!(
        context["junitxml"],
        root.join("out")
            .join("report.xml")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(context["junit_prefix"], "ci");
    assert_eq!(context["maxfail"], 1);
    assert_eq!(context["xdist_workers"], "3");
    assert_eq!(context["keyword"], "fast");
    assert_eq!(context["tags"], "slow");
    let plain = recognizer(root);
    assert_eq!(plain.config().args, ["-ra"]);
    let context = plain.context().into_value();
    assert_eq!(
        context["junitxml"],
        root.join("a.xml").to_string_lossy().as_ref()
    );
    // Where pytest starts collecting: the command-line paths, else
    // `testpaths` from the rootdir, else the current directory.
    assert_eq!(
        context["roots"],
        serde_json::json!([root.to_string_lossy()])
    );
    std::fs::create_dir_all(root.join("tests/unit")).unwrap();
    write(root, "pytest.ini", "[pytest]\ntestpaths = tests/*\n");
    assert_eq!(
        recognizer(root).context().into_value()["roots"],
        serde_json::json!([root.join("tests").join("unit").to_string_lossy()])
    );
    assert_eq!(
        recognizer_with(root, &["tests"], &[], None)
            .context()
            .into_value()["roots"],
        serde_json::json!([root.join("tests").to_string_lossy()])
    );
}

/// C24: pytest's JUnit settings, as `getini` gives them, for the report
/// aye writes: the inifile's, then `-o` from addopts, `PYTEST_ADDOPTS` and
/// the arguments after `--`; pytest's defaults otherwise, also for a value
/// pytest couldn't read (it reads them only when it writes a report).
#[test]
fn context_carries_the_junit_settings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let junit = |context: serde_json::Value| {
        [
            "junit_suite_name",
            "junit_logging",
            "junit_log_passing_tests",
            "junit_duration_report",
            "junit_family",
        ]
        .map(|key| context[key].clone())
    };
    write(root, "pyproject.toml", "[project]\nname = \"x\"\n");
    assert_eq!(
        junit(recognizer(root).context().into_value()),
        [
            serde_json::json!("pytest"),
            serde_json::json!("no"),
            serde_json::json!(true),
            serde_json::json!("total"),
            serde_json::json!("xunit2"),
        ]
    );
    write(
        root,
        "pyproject.toml",
        "[tool.pytest.ini_options]\njunit_suite_name = \"mysuite\"\njunit_logging = \"all\"\n\
         junit_log_passing_tests = false\naddopts = \"-o junit_family=legacy\"\n",
    );
    assert_eq!(
        junit(recognizer(root).context().into_value()),
        [
            serde_json::json!("mysuite"),
            serde_json::json!("all"),
            serde_json::json!(false),
            serde_json::json!("total"),
            serde_json::json!("legacy"),
        ]
    );
    let context = recognizer_with(
        root,
        &[],
        &[
            "-o",
            "junit_duration_report=call",
            "-o",
            "junit_logging=out-err",
        ],
        Some("-o junit_logging=log -o junit_log_passing_tests=yes"),
    )
    .context()
    .into_value();
    assert_eq!(
        junit(context),
        [
            serde_json::json!("mysuite"),
            serde_json::json!("out-err"),
            serde_json::json!(true),
            serde_json::json!("call"),
            serde_json::json!("legacy"),
        ]
    );
    write(
        root,
        "pytest.ini",
        "[pytest]\njunit_log_passing_tests = maybe\n",
    );
    assert_eq!(
        recognizer(root).context().into_value()["junit_log_passing_tests"],
        true
    );
}

#[test]
fn framework_args_reach_static_collection() {
    // `-o` and `--ignore` after `--` apply as on pytest's command line.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = -o python_files=t_*.py\n",
    );
    write(root, "tests/b/check_b.py", "def test_b(): pass\n");
    let recognizer = recognizer_with(
        root,
        &[],
        &["-o", "python_files=check_*.py", "--ignore=tests/b"],
        Some("-o python_files=env_*.py"),
    );
    assert!(!recognizer.descend(&root.join("tests/b")));
    assert_eq!(
        recognizer.claim("tests/check_a.py", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("tests/t_a.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("tests/env_a.py", Requested::Found),
        Claim::Ignore
    );
}

#[test]
fn conftests_matching_python_files_are_test_modules() {
    // pytest 9.1.1 collects a conftest.py as a test module when it matches
    // `python_files` or is named on the command line, and still loads it
    // as a conftest; `--ignore` on it only stops the former.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\npython_files = *.py\n");
    write(
        root,
        "tests/conftest.py",
        "import pytest\n@pytest.fixture\ndef fx(): return 1\ndef test_root_conf(fx): pass\n",
    );
    write(root, "tests/test_x.py", "def test_x(fx): pass\n");
    let configured = recognizer(root);
    assert_eq!(
        configured.claim("tests/conftest.py", Requested::Found),
        Claim::TestsAndSupport
    );
    assert_eq!(
        certainties(&recognize_at(root, "tests/conftest.py")),
        [("tests/conftest.py::test_root_conf", Certainty::Exact)]
    );
    if let Some(python) = python() {
        assert_eq!(differential(&python, root), Ok(Vec::new()));
    }

    let ignoring = recognizer_with(root, &[], &["--ignore=tests/conftest.py"], None);
    assert_eq!(
        ignoring.claim("tests/conftest.py", Requested::Found),
        Claim::Support
    );

    write(root, "pytest.ini", "[pytest]\n");
    let default = recognizer(root);
    assert_eq!(
        default.claim("tests/conftest.py", Requested::Found),
        Claim::Support
    );
    assert_eq!(
        default.claim("tests/conftest.py", Requested::Named),
        Claim::TestsAndSupport
    );
}

#[test]
fn noconftest_loads_no_conftest() {
    // pytest 9.1.1 with `--noconftest`: conftest.py is an ordinary file
    // (a test module when it matches `python_files`), and its
    // `collect_ignore` and fixtures don't apply.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\naddopts = --noconftest\n");
    write(
        root,
        "conftest.py",
        "collect_ignore = ['test_skipped.py']\ndef pytest_pycollect_makeitem(collector, name, obj): pass\n",
    );
    write(root, "test_skipped.py", "def test_s(): pass\n");
    let without = recognizer(root);
    assert_eq!(
        without.claim("conftest.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(without.claim("conftest.py", Requested::Named), Claim::Tests);
    assert_eq!(
        without.claim("test_skipped.py", Requested::Found),
        Claim::Tests
    );
    let recognition = recognize_with(&without, root, "test_skipped.py");
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [("test_skipped.py::test_s", Certainty::Exact)]
    );
    if let Some(python) = python() {
        assert_eq!(differential(&python, root), Ok(Vec::new()));
    }
    // Without it, the conftest's `collect_ignore` applies.
    write(root, "pytest.ini", "[pytest]\n");
    assert_eq!(
        recognizer(root).claim("test_skipped.py", Requested::Found),
        Claim::Ignore
    );
}

#[test]
fn pytest_plugins_in_a_non_top_level_conftest_fails_its_directory() {
    // pytest 9.1.1, run without arguments from the rootdir, loads the
    // conftests of the rootdir and its `test*` subdirectories first; a
    // conftest loaded later that defines `pytest_plugins` fails its
    // directory's collection ("ERROR collecting tests/sub").
    let dir = tempfile::tempdir().unwrap();
    // The real path, which pytest prints (the temporary directory is behind
    // a symlink on macOS), without the `\\?\` `canonicalize` puts before it
    // on Windows.
    let real = dir.path().canonicalize().unwrap();
    let root = &match real.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(plain) => PathBuf::from(plain),
        None => real,
    };
    write(root, "pytest.ini", "[pytest]\n");
    write(root, "tests/conftest.py", "pytest_plugins = []\n");
    write(root, "tests/sub/conftest.py", "pytest_plugins: list = []\n");
    write(root, "tests/sub/test_a.py", "def test_a(): pass\n");
    write(root, "tests/sub/deeper/test_d.py", "def test_d(): pass\n");
    write(root, "tests/test_b.py", "def test_b(): pass\n");
    write(
        root,
        "tests/maybe/conftest.py",
        "import os\nif os.environ.get('X'):\n    pytest_plugins = []\n",
    );
    write(root, "tests/maybe/test_m.py", "def test_m(): pass\n");
    let walking = recognizer(root);
    let recognition = recognize_with(&walking, root, "tests/sub/test_a.py");
    assert_eq!(recognition.cases, [] as [aye_events::Case; 0]);
    assert_eq!(recognition.diagnostics.len(), 1);
    let diagnostic = &recognition.diagnostics[0];
    assert_eq!(diagnostic.code, DiagnosticCode::CollectionError);
    assert_eq!(diagnostic.severity, Severity::Error);
    assert_eq!(diagnostic.path, "tests/sub");
    let message = format!(
        "Defining 'pytest_plugins' in a non-top-level conftest is no longer supported:\n\
         It affects the entire test suite instead of just below the conftest as expected.\n  \
         {}\n\
         Please move it to a top level conftest file at the rootdir:\n  \
         {}\n\
         For more information, visit:\n  \
         https://docs.pytest.org/en/stable/deprecations.html#pytest-plugins-in-non-top-level-conftest-files",
        root.join("tests").join("sub").join("conftest.py").display(),
        root.display()
    );
    assert_eq!(diagnostic.message, message);
    // Reported once for the directory; nothing below it is collected.
    let deeper = recognize_with(&walking, root, "tests/sub/deeper/test_d.py");
    assert!(deeper.cases.is_empty() && deeper.diagnostics.is_empty());
    // The `test*` subdirectory's conftest is initial, and a conditional
    // definition is left to pytest.
    assert_eq!(
        recognize_with(&walking, root, "tests/test_b.py")
            .cases
            .len(),
        1
    );
    assert_eq!(
        recognize_with(&walking, root, "tests/maybe/test_m.py")
            .cases
            .len(),
        1
    );
    if let Some(python) = python() {
        let output = Command::new(&python)
            .args([
                "-m",
                "pytest",
                "--collect-only",
                "-q",
                "-p",
                "no:cacheprovider",
            ])
            .current_dir(root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env_remove("PYTEST_ADDOPTS")
            .output()
            .unwrap();
        // Python's stdout writes `\r\n` on Windows; the message has `\n`.
        let stdout = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        assert!(stdout.contains("ERROR collecting tests/sub"), "{stdout}");
        assert!(stdout.contains(&message), "{stdout}");
        assert_eq!(output.status.code(), Some(2));
    }

    // Named on the command line, the directory's conftests are initial.
    let named = recognizer_with(root, &["tests/sub"], &[], None);
    assert_eq!(
        recognize_with(&named, root, "tests/sub/test_a.py")
            .cases
            .len(),
        1
    );
    let named = recognizer_with(root, &["tests/sub/deeper/test_d.py"], &[], None);
    assert_eq!(
        recognize_with(&named, root, "tests/sub/deeper/test_d.py")
            .cases
            .len(),
        1
    );
    // `--pyargs` turns the check off.
    let pyargs = recognizer_with(root, &[], &["--pyargs"], None);
    assert_eq!(
        recognize_with(&pyargs, root, "tests/sub/test_a.py")
            .cases
            .len(),
        1
    );
    // `testpaths` are the arguments when none are given.
    write(root, "pytest.ini", "[pytest]\ntestpaths = tests/sub\n");
    let testpaths = recognizer(root);
    assert_eq!(
        recognize_with(&testpaths, root, "tests/sub/test_a.py")
            .cases
            .len(),
        1
    );
}

#[test]
fn deselect_drops_no_case_and_the_context_names_its_prefixes() {
    // The binary applies `--deselect`'s prefixes in selection, which counts
    // what they drop; the recognizer keeps every case.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --deselect tests/test_a.py::test_x --deselect=tests/test_a.py::TestC::test_m[1\n",
    );
    write(
        root,
        "tests/test_a.py",
        "import pytest\ndef test_x(): pass\ndef test_x2(): pass\ndef test_y(): pass\nclass TestC:\n    @pytest.mark.parametrize('v', [1, 2, 10])\n    def test_m(self, v): pass\n",
    );
    let ids: Vec<String> = recognize_at(root, "tests/test_a.py")
        .cases
        .iter()
        .map(|c| c.id.as_str().to_owned())
        .collect();
    assert_eq!(
        ids,
        [
            "tests/test_a.py::test_x",
            "tests/test_a.py::test_x2",
            "tests/test_a.py::test_y",
            "tests/test_a.py::TestC::test_m[1]",
            "tests/test_a.py::TestC::test_m[2]",
            "tests/test_a.py::TestC::test_m[10]",
        ]
    );
    assert_eq!(
        recognizer(root).context().into_value()["deselect"],
        serde_json::json!([
            "tests/test_a.py::test_x",
            "tests/test_a.py::TestC::test_m[1"
        ])
    );
    // From the arguments after `--` too.
    let recognizer = recognizer_with(root, &[], &["--deselect", "tests/test_a.py::test_y"], None);
    assert_eq!(
        recognize_with(&recognizer, root, "tests/test_a.py")
            .cases
            .len(),
        6
    );
    assert_eq!(
        recognizer.context().into_value()["deselect"],
        serde_json::json!([
            "tests/test_a.py::test_x",
            "tests/test_a.py::TestC::test_m[1",
            "tests/test_a.py::test_y"
        ])
    );
}

#[test]
fn plugins_aye_cannot_read_make_cases_partial() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "myplugin.py", "def helper(): pass\n");
    write(root, "test_a.py", "def test_a(): pass\n");
    for (addopts, certainty) in [
        ("", Certainty::Exact),
        ("-p pytester -p no:cacheprovider", Certainty::Exact),
        ("-p myplugin", Certainty::Exact),
        ("-p installed_plugin", Certainty::Partial),
        ("-pinstalled_plugin", Certainty::Partial),
        (
            "-p installed_plugin -p no:installed_plugin",
            Certainty::Exact,
        ),
    ] {
        write(
            root,
            "pytest.ini",
            &format!("[pytest]\naddopts = {addopts}\n"),
        );
        assert_eq!(
            certainties(&recognize_at(root, "test_a.py")),
            [("test_a.py::test_a", certainty)],
            "{addopts}"
        );
    }
    write(root, "pytest.ini", "[pytest]\n");
    let recognizer = recognizer_with(root, &[], &["-p", "installed_plugin"], None);
    assert_eq!(
        certainties(&recognize_with(&recognizer, root, "test_a.py")),
        [("test_a.py::test_a", Certainty::Partial)]
    );
}

#[test]
fn parametrize_arguments_pytest_refuses_are_refined() {
    // Each fails pytest 9.1.1's collection: marks aren't hashable, scopes
    // are pytest's five names, and `indirect` is a boolean or a sequence of
    // argument names.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    let refused = [
        (
            "test_set.py",
            "import pytest\n@pytest.mark.parametrize('v', [pytest.param(1, marks={pytest.mark.c})])\ndef test_e(v): pass\n",
        ),
        (
            "test_mark_set.py",
            "import pytest\npytestmark = []\npytestmark += {pytest.mark.b}\ndef test_p(): pass\n",
        ),
        (
            "test_scope.py",
            "import pytest\n@pytest.mark.parametrize('v', [1], scope='bogus')\ndef test_s(v): pass\n",
        ),
        (
            "test_ind.py",
            "import pytest\n@pytest.mark.parametrize('v', [1], indirect=['zz'])\ndef test_i(v): pass\n",
        ),
        (
            "test_ind_none.py",
            "import pytest\n@pytest.mark.parametrize('v', [1], indirect=None)\ndef test_n(v): pass\n",
        ),
        (
            "test_ind_dict.py",
            "import pytest\n@pytest.mark.parametrize('v', [1], indirect={'v': 1})\ndef test_d(v): pass\n",
        ),
    ];
    for (rel, source) in refused {
        write(root, rel, source);
        assert!(recognize_at(root, rel).refine.is_some(), "{rel}");
    }
    write(
        root,
        "test_ind_ok.py",
        "import pytest\n@pytest.fixture\ndef x(request): return request.param\n@pytest.mark.parametrize('x', [1, 2], indirect=['x'])\ndef test_l(x): pass\n@pytest.mark.parametrize('x', [3], indirect='x')\ndef test_s(x): pass\n@pytest.mark.parametrize('x, y', [(1, 2)], indirect=())\ndef test_t(x, y): pass\n@pytest.mark.parametrize('y', [5], scope='module')\ndef test_m(y): pass\n",
    );
    let recognition = recognize_at(root, "test_ind_ok.py");
    assert_eq!(recognition.refine, None);
    assert_eq!(
        certainties(&recognition),
        [
            ("test_ind_ok.py::test_l[1]", Certainty::Exact),
            ("test_ind_ok.py::test_l[2]", Certainty::Exact),
            ("test_ind_ok.py::test_s[3]", Certainty::Exact),
            ("test_ind_ok.py::test_t[1-2]", Certainty::Exact),
            ("test_ind_ok.py::test_m[5]", Certainty::Exact),
        ]
    );
    if let Some(python) = python() {
        let (_, errors) = pytest_ids(&python, root);
        for (rel, _) in refused {
            assert!(errors.iter().any(|e| e == rel), "{rel}: {errors:?}");
        }
        let refined = differential(&python, root).unwrap();
        assert_eq!(refined.len(), refused.len());
    }
}
