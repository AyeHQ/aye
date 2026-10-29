//! Recognition tests. Expected ids are what `python -m unittest discover`
//! loads on Python 3.13, named as the adapter names tests (see the crate
//! docs); the differential test re-checks every suite against a real
//! interpreter when one is available (`AYE_TEST_PYTHON`, or the workspace's
//! `.venv`).

use std::path::{Path, PathBuf};
use std::process::Command;

use aye_adapter::{
    Adapter, Claim, LineIndex, Project, Recognition, Recognizer, RefineReason, SourceFile,
};
use aye_events::Certainty;

use super::*;

mod suites;

use suites::{SUITES, Suite};

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

fn scratch(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        write(dir.path(), rel, body);
    }
    dir
}

/// `[tool.aye]` as the binary loads it from `root`'s `pyproject.toml`.
fn tool_aye(root: &Path) -> aye_config::AyeSettings {
    aye_config::AyeConfig::load(root).unwrap().settings
}

fn recognizer(root: &Path, args: &[PathBuf]) -> Result<UnittestRecognizer, AdapterError> {
    UnittestRecognizer::new(&Project {
        args,
        tool_aye: &tool_aye(root),
        ..Project::new(root, root)
    })
}

/// Recognize every test file the way aye-collect walks: from the roots,
/// entries sorted by name, directories entered in place when `descend`
/// says so, each file offered to `claim`.
fn collect(root: &Path, args: &[PathBuf]) -> Vec<(String, Recognition)> {
    let recognizer = recognizer(root, args).unwrap();
    let project = Project {
        args,
        ..Project::new(root, root)
    };
    let mut out = Vec::new();
    for start in recognizer.roots(&project) {
        if start.is_file() {
            let rel = relative(root, &start);
            if recognizer.claim(&rel, Requested::Named).has_tests() {
                recognize(&recognizer, root, &rel, &mut out);
            }
        } else {
            walk(&recognizer, root, &start, &mut out);
        }
    }
    out
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

fn walk(
    recognizer: &UnittestRecognizer,
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Recognition)>,
) {
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
        let rel = relative(root, &path);
        if recognizer.claim(&rel, Requested::Found).has_tests() {
            recognize(recognizer, root, &rel, out);
        }
    }
}

fn recognize(
    recognizer: &UnittestRecognizer,
    root: &Path,
    rel: &str,
    out: &mut Vec<(String, Recognition)>,
) {
    let parsed = Parsed::read(root, rel);
    out.push((
        rel.to_string(),
        recognizer.recognize(&parsed.file(), Requested::Found, &[]),
    ));
}

/// Every predicted id, in order.
fn ids(found: &[(String, Recognition)]) -> Vec<String> {
    found
        .iter()
        .flat_map(|(_, r)| r.cases.iter().map(|c| c.id.as_str().to_owned()))
        .collect()
}

/// The ids static recognition is sure of (files not refined).
fn exact(found: &[(String, Recognition)]) -> Vec<String> {
    found
        .iter()
        .filter(|(_, r)| r.refine.is_none())
        .flat_map(|(_, r)| {
            assert!(r.cases.iter().all(|c| c.certainty == Certainty::Exact));
            r.cases.iter().map(|c| c.id.as_str().to_owned())
        })
        .collect()
}

fn refined(found: &[(String, Recognition)]) -> Vec<String> {
    found
        .iter()
        .filter(|(_, r)| r.refine.is_some())
        .map(|(rel, _)| rel.clone())
        .collect()
}

// ---------------------------------------------------------------- the referee

/// The interpreter for differential tests: `AYE_TEST_PYTHON`, or the
/// workspace's `.venv`.
#[expect(
    clippy::disallowed_methods,
    reason = "the tests' own settings, not the environment aye reads"
)]
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
        .args(["-c", "import sys; assert sys.version_info >= (3, 10)"])
        .output()
        .is_ok_and(|o| o.status.success());
    ok.then_some(candidate)
}

/// What `python -m unittest discover` loads, as ids, in its order:
/// `TestLoader.discover` with a loader that remembers which discovered
/// module produced each test.
const REFEREE: &str = r#"
import os, sys, unittest
root, start, top, pattern = sys.argv[1:5]

def flatten(suite, out):
    try:
        items = iter(suite)
    except TypeError:
        out.append(suite)
        return out
    for item in items:
        flatten(item, out)
    return out

class Loader(unittest.TestLoader):
    def __init__(self):
        super().__init__()
        self.origins, self.discovered, self.kept = {}, set(), []
        # The name each class is met under in its module (dir()), by test.
        self.bound, self.names = {}, []
    def _get_module_from_name(self, name):
        module = super()._get_module_from_name(name)
        self.discovered.add(name)
        return module
    def loadTestsFromModule(self, module, *args, pattern=None, **kws):
        names = {}
        for name in dir(module):
            obj = getattr(module, name)
            if isinstance(obj, type) and issubclass(obj, unittest.TestCase):
                names.setdefault(id(obj), []).append(name)
        self.names.append(names)
        try:
            suite = super().loadTestsFromModule(module, *args, pattern=pattern, **kws)
        finally:
            self.names.pop()
        if module.__name__ in self.discovered:
            for test in flatten(suite, []):
                if id(test) not in self.origins:
                    self.origins[id(test)] = module.__name__
                    self.kept.append(test)
        return suite
    def loadTestsFromTestCase(self, cls):
        suite = super().loadTestsFromTestCase(cls)
        queue = self.names[-1].get(id(cls)) if self.names else None
        if queue:
            name = queue.pop(0)
            for test in flatten(suite, []):
                self.bound[id(test)] = name
                self.kept.append(test)
        return suite

def qualname(obj):
    name = obj.__qualname__
    return obj.__name__ if "<locals>" in name else name

loader = Loader()
for test in flatten(loader.discover(start, pattern, top), []):
    kind = (type(test).__module__, type(test).__name__)
    if kind in (("unittest.loader", "_FailedTest"), ("unittest.loader", "ModuleSkipped")):
        print("FAILED " + test._testMethodName)
        continue
    file = os.path.relpath(sys.modules[loader.origins[id(test)]].__file__, root).replace(os.sep, "/")
    if isinstance(test, unittest.FunctionTestCase):
        names = [qualname(test._testFunc)]
    elif getattr(test, "_dt_test", None) is not None:
        names = [test._dt_test.name]
    else:
        bound = loader.bound.get(id(test))
        classes = [bound] if bound else qualname(type(test)).split(".")
        names = [*classes, test._testMethodName]
    print("::".join([file, *names]))
"#;

/// unittest's ids for `root` (with the suite's configuration), and the
/// modules that failed to load.
fn referee(python: &Path, root: &Path) -> (Vec<String>, Vec<String>) {
    let config = UnittestConfig::load(root, &tool_aye(root)).unwrap();
    let output = Command::new(python)
        .arg("-c")
        .arg(REFEREE)
        .arg(root)
        .arg(&config.start)
        .arg(&config.top)
        .arg(&config.pattern.value)
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        // Read as UTF-8, as the worker's: Windows' default is the ANSI code
        // page.
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut ids = Vec::new();
    let mut failed = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        match line.strip_prefix("FAILED ") {
            Some(module) => failed.push(module.to_string()),
            None => ids.push(line.to_string()),
        }
    }
    (ids, failed)
}

/// Static recognition against unittest: every exact id is unittest's, in
/// unittest's order within each file, and every file unittest loads tests
/// from is either predicted completely or refined.
fn differential(python: &Path, suite: &Suite) {
    let dir = scratch(suite.files);
    let root = dir.path();
    let (theirs, _) = referee(python, root);
    let found = collect(root, &[]);
    let refined = refined(&found);
    let keep = |id: &String| {
        let file = id.split("::").next().unwrap_or_default();
        !refined.iter().any(|r| r == file)
    };
    let mut theirs: Vec<String> = theirs.into_iter().filter(keep).collect();
    let mut ours = exact(&found);
    // Files may come in another order (a package's `__init__.py` comes
    // first in unittest's walk); within a file the order must agree.
    let by_file = |ids: &mut Vec<String>| {
        ids.sort_by(|a, b| a.split("::").next().cmp(&b.split("::").next()));
    };
    by_file(&mut theirs);
    by_file(&mut ours);
    assert_eq!(
        ours, theirs,
        "suite {}: static ids differ from unittest's",
        suite.name
    );
}

// ---------------------------------------------------------------- tests

#[test]
fn suites_are_recognized_as_pinned() {
    for suite in SUITES {
        let dir = scratch(suite.files);
        let found = collect(dir.path(), &[]);
        assert_eq!(
            exact(&found),
            suite.exact,
            "suite {}: exact ids",
            suite.name
        );
        assert_eq!(
            refined(&found),
            suite.refined,
            "suite {}: refined files",
            suite.name
        );
    }
}

#[test]
fn suites_match_unittest() {
    let Some(python) = python() else {
        eprintln!("skipping: no Python 3.10+ (set AYE_TEST_PYTHON, or run `uv sync`)");
        return;
    };
    for suite in SUITES {
        differential(&python, suite);
    }
}

#[test]
fn pinned_ids_are_unittests() {
    // The pinned lists themselves, against the referee: what unittest
    // loads from the files static recognition is sure of.
    let Some(python) = python() else {
        return;
    };
    for suite in SUITES {
        let dir = scratch(suite.files);
        let (theirs, _) = referee(&python, dir.path());
        let refined = suite.refined;
        let theirs: Vec<&str> = theirs
            .iter()
            .map(String::as_str)
            .filter(|id| !refined.contains(&id.split("::").next().unwrap_or_default()))
            .collect();
        let mut sorted_theirs = theirs.clone();
        sorted_theirs.sort_unstable();
        let mut pinned = suite.exact.to_vec();
        pinned.sort_unstable();
        assert_eq!(pinned, sorted_theirs, "suite {}", suite.name);
    }
}

#[test]
fn the_adapter_names_its_executor() {
    let executor = Unittest.executor();
    assert_eq!(Unittest.name(), "unittest");
    assert_eq!(executor.adapter, "unittest");
    assert_eq!(executor.entry, "aye_engine_unittest:UnittestExecutor");
    assert_eq!(executor.modules, &[MODULE]);
    assert!(
        MODULE
            .source
            .contains("class UnittestExecutor(aye_engine.Executor)")
    );
}

#[test]
fn cases_carry_keywords_affinity_and_spans() {
    let dir = scratch(&[
        ("tests/__init__.py", ""),
        (
            "tests/test_cart.py",
            "import unittest\n\nclass TestCart(unittest.TestCase):\n    def test_add(self):\n        pass\n",
        ),
    ]);
    let found = collect(dir.path(), &[]);
    // The empty `tests/__init__.py` isn't claimed.
    assert_eq!(found.len(), 1);
    let case = &found[0].1.cases[0];
    assert_eq!(case.id.as_str(), "tests/test_cart.py::TestCart::test_add");
    assert_eq!(case.adapter, "unittest");
    assert_eq!(
        case.keywords,
        ["tests", "test_cart.py", "TestCart", "test_add"]
    );
    assert_eq!(
        case.affinity,
        ["tests/test_cart.py", "tests/test_cart.py::TestCart"]
    );
    let span = case.span.as_ref().unwrap();
    assert_eq!(span.path, "tests/test_cart.py");
    assert_eq!(span.line, Some(4));
    let source = std::fs::read_to_string(dir.path().join("tests/test_cart.py")).unwrap();
    assert_eq!(&source[span.start as usize..span.end as usize], "test_add");
    assert_eq!(case.certainty, Certainty::Exact);
}

#[test]
fn inherited_methods_point_at_their_definition() {
    let dir = scratch(&[
        (
            "helpers.py",
            "import unittest\n\nclass Shared(unittest.TestCase):\n    def test_shared(self):\n        pass\n",
        ),
        (
            "test_one.py",
            "from helpers import Shared\n\nclass One(Shared):\n    pass\n",
        ),
    ]);
    let found = collect(dir.path(), &[]);
    let cases = &found[0].1.cases;
    let ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "test_one.py::One::test_shared",
            "test_one.py::Shared::test_shared"
        ]
    );
    assert_eq!(cases[0].span.as_ref().unwrap().path, "helpers.py");
    assert_eq!(cases[0].affinity, ["test_one.py", "test_one.py::One"]);
}

#[test]
fn refined_files_keep_a_partial_guess() {
    let dir = scratch(&[(
        "test_dyn.py",
        "import unittest\n\nclass Plain(unittest.TestCase):\n    def test_a(self):\n        pass\n\n\
         def load_tests(loader, tests, pattern):\n    return tests\n",
    )]);
    let found = collect(dir.path(), &[]);
    let (rel, recognition) = &found[0];
    assert_eq!(rel, "test_dyn.py");
    assert!(
        recognition
            .refine
            .as_ref()
            .map(RefineReason::as_str)
            .unwrap()
            .contains("load_tests")
    );
    assert_eq!(recognition.cases.len(), 1);
    assert_eq!(recognition.cases[0].certainty, Certainty::Partial);
}

#[test]
fn modules_with_no_visible_test_are_refined() {
    // Discovery imports them all the same: an import error there is
    // unittest's error (and a test may only exist at runtime).
    let dir = scratch(&[
        ("test_broken.py", "import nonexistent_module_xyz\n"),
        ("test_helpers.py", "def helper():\n    return 1\n"),
        (
            "test_plain.py",
            "import unittest\nclass A(unittest.TestCase):\n    def test_a(self): pass\n",
        ),
        ("pkg/__init__.py", "from .missing import thing\n"),
        (
            "pkg/test_in.py",
            "import unittest\nclass B(unittest.TestCase):\n    def test_b(self): pass\n",
        ),
        ("empty/__init__.py", ""),
        ("empty/test_nothing.py", "\n"),
        ("dotted.pkg/__init__.py", "import os\n"),
        ("dotted.pkg/test_below.py", "X = 1\n"),
    ]);
    let found = collect(dir.path(), &[]);
    assert_eq!(
        refined(&found),
        [
            "dotted.pkg/__init__.py",
            "empty/test_nothing.py",
            "pkg/__init__.py",
            "test_broken.py",
            "test_helpers.py",
        ]
    );
    let reasons: Vec<&str> = found
        .iter()
        .filter_map(|(_, r)| r.refine.as_ref().map(RefineReason::as_str))
        .collect();
    assert!(reasons.contains(&NO_STATIC_TESTS));
    assert_eq!(
        exact(&found),
        ["pkg/test_in.py::B::test_b", "test_plain.py::A::test_a"]
    );
    // Below a package discovery can't import, nothing is refined: unittest
    // reports the package's one error.
    let below = found
        .iter()
        .find(|(rel, _)| rel == "dotted.pkg/test_below.py")
        .unwrap();
    assert!(below.1.refine.is_none() && below.1.cases.is_empty());
}

#[test]
fn dataclasses_and_their_like_are_no_test_classes() {
    // A test module importing the project's dataclasses: they can't be test
    // classes, so the module is exact. A decorated class whose bases may
    // make it one is still refined.
    let dir = scratch(&[
        (
            "shop/models.py",
            "import dataclasses\nimport enum\nfrom dataclasses import dataclass\n\
             from functools import total_ordering\n\n\
             @dataclass(frozen=True)\nclass Product:\n    name: str\n\n\
             @dataclasses.dataclass(slots=True)\nclass Line(object):\n    qty: int\n\n\
             @total_ordering\nclass Price:\n    def __lt__(self, other): return False\n\n\
             @enum.unique\nclass Color(enum.Enum):\n    RED = 1\n",
        ),
        (
            "shop/__init__.py",
            "from .models import Color, Line, Price, Product\n",
        ),
        (
            "test_shop.py",
            "import unittest\nfrom shop import Color, Line, Price, Product\n\n\
             class ShopTests(unittest.TestCase):\n    def test_product(self): pass\n",
        ),
        (
            "test_mixed.py",
            "import unittest\nfrom functools import total_ordering\n\n\
             class Base(unittest.TestCase):\n    def test_base(self): pass\n\n\
             @total_ordering\nclass Odd(Base):\n    def __lt__(self, other): return False\n",
        ),
    ]);
    let found = collect(dir.path(), &[]);
    // Discovery imports the `shop` package too: it has no test aye can see.
    assert_eq!(refined(&found), ["shop/__init__.py", "test_mixed.py"]);
    assert_eq!(exact(&found), ["test_shop.py::ShopTests::test_product"]);
    if let Some(python) = python() {
        let (theirs, failed) = referee(&python, dir.path());
        assert!(theirs.contains(&"test_shop.py::ShopTests::test_product".to_string()));
        assert_eq!(failed, [] as [std::string::String; 0]);
    }
}

#[test]
fn packages_that_may_define_load_tests_take_over_their_files() {
    let dir = scratch(&[
        (
            "pkg/__init__.py",
            "def load_tests(loader, tests, pattern):\n    return tests\n",
        ),
        (
            "pkg/test_a.py",
            "import unittest\nclass A(unittest.TestCase):\n    def test_a(self): pass\n",
        ),
        ("pkg/sub/__init__.py", ""),
        (
            "pkg/sub/test_b.py",
            "import unittest\nclass B(unittest.TestCase):\n    def test_b(self): pass\n",
        ),
        (
            "test_c.py",
            "import unittest\nclass C(unittest.TestCase):\n    def test_c(self): pass\n",
        ),
    ]);
    let found = collect(dir.path(), &[]);
    assert_eq!(
        refined(&found),
        ["pkg/__init__.py", "pkg/sub/test_b.py", "pkg/test_a.py"]
    );
    let b = found
        .iter()
        .find(|(rel, _)| rel == "pkg/sub/test_b.py")
        .unwrap();
    assert_eq!(b.1.cases[0].affinity[0], "package:pkg");
    assert_eq!(b.1.cases[0].certainty, Certainty::Partial);
    assert_eq!(exact(&found), ["test_c.py::C::test_c"]);
}

#[test]
fn the_load_tests_rule_reads_words() {
    for text in [
        "def load_tests(l, t, p): pass",
        "from .x import *",
        "from .x import\\\n  *",
        "def __getattr__(name): pass",
        "globals()['x'] = 1",
        "setattr(sys.modules[__name__], 'x', 1)",
        "vars()",
        "exec('x')",
    ] {
        assert!(may_define_load_tests(text.as_bytes()), "{text}");
    }
    for text in [
        "",
        "import os\n",
        "executor = 1",
        "my_load_tests = 1",
        "envvars = 1",
        "x = '*'",
    ] {
        assert!(!may_define_load_tests(text.as_bytes()), "{text}");
    }
}

#[test]
fn configuration_is_read_with_origins() {
    let dir = scratch(&[
        (
            "pyproject.toml",
            "[tool.aye.unittest]\nstart-directory = \"tests\"\npattern = \"*_test.py\"\ntop-level-directory = \".\"\n",
        ),
        ("tests/__init__.py", ""),
        (
            "tests/cart_test.py",
            "import unittest\nclass T(unittest.TestCase):\n    def test_x(self): pass\n",
        ),
        (
            "tests/test_other.py",
            "import unittest\nclass T(unittest.TestCase):\n    def test_x(self): pass\n",
        ),
        (
            "cart_test.py",
            "import unittest\nclass T(unittest.TestCase):\n    def test_x(self): pass\n",
        ),
    ]);
    let root = dir.path();
    let recognizer = recognizer(root, &[]).unwrap();
    let settings = recognizer.settings();
    let shown: Vec<(&str, &str, String)> = settings
        .iter()
        .map(|s| {
            let source = s.source.display_row(root).to_string();
            (s.name.as_ref(), s.value.as_str(), source)
        })
        .collect();
    assert_eq!(
        shown,
        [
            (
                "start-directory",
                "tests",
                "pyproject.toml [tool.aye.unittest]".to_string()
            ),
            (
                "pattern",
                "*_test.py",
                "pyproject.toml [tool.aye.unittest]".to_string()
            ),
            (
                "top-level-directory",
                ".",
                "pyproject.toml [tool.aye.unittest]".to_string()
            ),
        ]
    );
    let context = recognizer.context().into_value();
    assert_eq!(
        context["start_directory"],
        root.join("tests").to_string_lossy().as_ref()
    );
    assert_eq!(
        context["top_level_directory"],
        root.to_string_lossy().as_ref()
    );
    assert_eq!(context["pattern"], "*_test.py");
    assert!(context.get("starts").is_none());
    assert_eq!(ids(&collect(root, &[])), ["tests/cart_test.py::T::test_x"]);
}

#[test]
fn defaults_are_reported_as_defaults() {
    let dir = scratch(&[("pyproject.toml", "[project]\nname = 'x'\n")]);
    let recognizer = recognizer(dir.path(), &[]).unwrap();
    let shown: Vec<(String, String, String)> = recognizer
        .settings()
        .into_iter()
        .map(|s| {
            let source = s.source.display_row(dir.path()).to_string();
            (s.name.into_owned(), s.value, source)
        })
        .collect();
    assert_eq!(
        shown,
        [
            ("start-directory".into(), ".".into(), "default".into()),
            ("pattern".into(), "test*.py".into(), "default".into()),
            ("top-level-directory".into(), ".".into(), "default".into()),
        ]
    );
}

#[test]
fn bad_configuration_is_an_error() {
    for (body, wanted) in [
        (
            "[tool.aye.unittest]\nstart_dir = \"x\"\n",
            "unknown key \"start_dir\"",
        ),
        (
            "[tool.aye.unittest]\npattern = 3\n",
            "pattern must be a string",
        ),
        (
            "[tool.aye.unittest]\nstart-directory = \"missing\"\n",
            "is not a directory",
        ),
        ("[tool.aye.unittest]\npattern = \"\"\n", "pattern is empty"),
        (
            "[tool.aye.unittest]\nstart-directory = \".\"\ntop-level-directory = \"sub\"\n",
            "outside the top-level directory",
        ),
        // unittest's "Start directory is not importable".
        (
            "[tool.aye.unittest]\nstart-directory = \"sub\"\ntop-level-directory = \".\"\n",
            "Start directory is not importable",
        ),
    ] {
        let dir = scratch(&[("pyproject.toml", body), ("sub/x.txt", "")]);
        let err = recognizer(dir.path(), &[]).unwrap_err().to_string();
        assert!(err.contains(wanted), "{body}: {err}");
    }
    // A package, or the top-level directory itself, is importable.
    for body in [
        "[tool.aye.unittest]\nstart-directory = \"sub\"\ntop-level-directory = \".\"\n",
        "[tool.aye.unittest]\nstart-directory = \"sub\"\ntop-level-directory = \"sub\"\n",
        "[tool.aye.unittest]\nstart-directory = \"sub\"\ntop-level-directory = \"sub/.\"\n",
    ] {
        let dir = scratch(&[("pyproject.toml", body), ("sub/__init__.py", "")]);
        assert!(recognizer(dir.path(), &[]).is_ok(), "{body}");
    }
}

#[test]
fn explicit_paths_narrow_or_add_starts() {
    let dir = scratch(&[
        (
            "pyproject.toml",
            "[tool.aye.unittest]\nstart-directory = \"tests\"\n",
        ),
        (
            "tests/test_a.py",
            "import unittest\nclass A(unittest.TestCase):\n    def test_a(self): pass\n",
        ),
        (
            "tests/data/test_d.py",
            "import unittest\nclass D(unittest.TestCase):\n    def test_d(self): pass\n",
        ),
        (
            "examples/test_e.py",
            "import unittest\nclass E(unittest.TestCase):\n    def test_e(self): pass\n",
        ),
        (
            "examples/helper.py",
            "import unittest\nclass H(unittest.TestCase):\n    def test_h(self): pass\n",
        ),
    ]);
    let root = dir.path();
    // The start directory: `data` isn't a package, so discovery skips it.
    assert_eq!(ids(&collect(root, &[])), ["tests/test_a.py::A::test_a"]);
    // The root holds the start directory: the same.
    assert_eq!(
        ids(&collect(root, &[PathBuf::from(".")])),
        ["tests/test_a.py::A::test_a"]
    );
    // A directory inside it is entered, package or not.
    assert_eq!(
        ids(&collect(root, &[PathBuf::from("tests/data")])),
        ["tests/data/test_d.py::D::test_d"]
    );
    // One outside is a start of its own, sent to the executor.
    let found = collect(root, &[PathBuf::from("examples")]);
    assert_eq!(ids(&found), ["examples/test_e.py::E::test_e"]);
    let recognizer = recognizer(root, &[PathBuf::from("examples")]).unwrap();
    assert_eq!(
        recognizer.context().into_value()["starts"],
        serde_json::json!([root.join("examples").to_string_lossy()])
    );
    // A file is a test module whatever the pattern.
    assert_eq!(
        ids(&collect(root, &[PathBuf::from("examples/helper.py")])),
        ["examples/helper.py::H::test_h"]
    );
    assert_eq!(
        recognizer.claim("examples/notes.txt", Requested::Named),
        Claim::Ignore
    );
}

#[test]
fn claims_follow_discovery() {
    let dir = scratch(&[
        ("pkg/__init__.py", "from .x import y\n"),
        ("empty/__init__.py", ""),
        ("plain/test_x.py", ""),
    ]);
    let recognizer = recognizer(dir.path(), &[]).unwrap();
    let root = dir.path();
    assert_eq!(
        recognizer.claim("test_x.py", Requested::Found),
        Claim::Tests
    );
    // `fnmatch` folds case on Windows: there `test*.py` takes it.
    let upper = if cfg!(windows) {
        Claim::Tests
    } else {
        Claim::Ignore
    };
    assert_eq!(recognizer.claim("Test_x.py", Requested::Found), upper);
    assert_eq!(
        recognizer.claim("test-x.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("x_test.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("test_x.txt", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("pkg/__init__.py", Requested::Found),
        Claim::Tests
    );
    // An empty `__init__.py` binds nothing.
    assert_eq!(
        recognizer.claim("empty/__init__.py", Requested::Found),
        Claim::Ignore
    );
    assert!(recognizer.descend(&root.join("empty")));
    // The top-level directory isn't a package to discovery.
    assert_eq!(
        recognizer.claim("__init__.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("x_test.py", Requested::Named),
        Claim::Tests
    );
    assert!(recognizer.descend(&root.join("pkg")));
    assert!(!recognizer.descend(&root.join("plain")));
}

/// A file named on the command line is a module alone, as `python -m
/// unittest path.py` loads it, even below a package that may define
/// `load_tests` (which answers for it in discovery).
#[test]
fn a_named_file_is_loaded_alone() {
    let dir = scratch(&[
        ("tests/__init__.py", ""),
        (
            "tests/pkg/__init__.py",
            "def load_tests(loader, tests, pattern):\n    tests.addTests(loader.loadTestsFromName(__name__ + '.test_fast'))\n    return tests\n",
        ),
        (
            "tests/pkg/test_fast.py",
            "import unittest\nclass Fast(unittest.TestCase):\n    def test_fast(self): pass\n",
        ),
        (
            "tests/pkg/test_slow.py",
            "import unittest\nclass Slow(unittest.TestCase):\n    def test_slow(self): self.fail('x')\n",
        ),
    ]);
    let root = dir.path();
    // Walked, the package answers.
    let found = collect(root, &[]);
    assert!(refined(&found).contains(&"tests/pkg/test_slow.py".to_string()));
    assert!(
        recognizer(root, &[])
            .unwrap()
            .context()
            .into_value()
            .get("named")
            .is_none(),
        "no file is named"
    );
    // Named, it is a module of its own.
    let args = [PathBuf::from("tests/pkg/test_slow.py")];
    let found = collect(root, &args);
    assert_eq!(exact(&found), ["tests/pkg/test_slow.py::Slow::test_slow"]);
    assert_eq!(refined(&found), [] as [std::string::String; 0]);
    let context = recognizer(root, &args).unwrap().context().into_value();
    assert_eq!(
        context["named"],
        serde_json::json!([root
            .join("tests")
            .join("pkg")
            .join("test_slow.py")
            .to_string_lossy()])
    );
    // A named file no `load_tests` takes over changes nothing.
    let dir = scratch(&[(
        "test_plain.py",
        "import unittest\nclass P(unittest.TestCase):\n    def test_p(self): pass\n",
    )]);
    let args = [PathBuf::from("test_plain.py")];
    assert!(
        recognizer(dir.path(), &args)
            .unwrap()
            .context()
            .into_value()
            .get("named")
            .is_none()
    );
}

/// The context's bytes as aye 0.0.1 built them (`tests/contexts.jsonl`, one
/// line each): with a start directory, a pattern and a top-level directory
/// from `[tool.aye.unittest]`, a directory named outside the start and a
/// file named below a package that may define `load_tests`; then with
/// nothing set. The refinement cache's key hashes them, so a change here
/// misses every cached entry. The project root is written `<root>`. Pinned
/// where paths are written with `/`.
#[cfg(unix)]
#[test]
fn the_context_is_pinned_byte_for_byte() {
    let set = scratch(&[
        (
            "pyproject.toml",
            "[tool.aye.unittest]\nstart-directory = \"tests\"\npattern = \"*_test.py\"\ntop-level-directory = \".\"\n",
        ),
        ("tests/__init__.py", ""),
        (
            "tests/pkg/__init__.py",
            "def load_tests(loader, tests, pattern):\n    return tests\n",
        ),
        ("tests/pkg/x_test.py", ""),
        ("examples/e_test.py", ""),
    ]);
    let args = [
        PathBuf::from("examples"),
        PathBuf::from("tests/pkg/x_test.py"),
    ];
    let unset = scratch(&[]);
    let written = |root: &Path, args: &[PathBuf]| {
        let text = serde_json::to_string(&root.to_string_lossy()).unwrap();
        serde_json::to_string(&recognizer(root, args).unwrap().context().into_value())
            .unwrap()
            .replace(text.trim_matches('"'), "<root>")
    };
    let pinned: Vec<&str> = include_str!("tests/contexts.jsonl").lines().collect();
    assert_eq!(
        [written(set.path(), &args), written(unset.path(), &[])],
        pinned[..]
    );
}
