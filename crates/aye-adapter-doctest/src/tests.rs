//! Recognition tests. Expected ids are copied from `pytest -p no:python
//! --collect-only -q` with pytest 9.1.1's doctest plugin; the live tests
//! re-check them, and the keywords, against a real pytest when one is
//! available (`AYE_TEST_PYTHON`, or the workspace's `.venv`).

mod suites;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use aye_adapter::{
    Adapter, Claim, LineIndex, Project, Recognition, Recognizer, RefineReason, SourceFile,
};
use aye_events::{Certainty, DiagnosticCode};

use super::*;
use crate::parser::{clean_doc, docstring, expand_tabs, parse};

// ---------------------------------------------------------------- helpers

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

    /// Python files are parsed; others come with an empty module, as
    /// collection hands them over.
    fn new(rel: &str, abs: PathBuf, source: String) -> Self {
        let python = Path::new(rel).extension().is_some_and(|e| e == "py");
        let ast = ruff_python_parser::parse_module(if python { &source } else { "" }).unwrap();
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

/// The project at `root`, with `[tool.aye]` as the binary loads it from
/// its `pyproject.toml` (kept for the rest of the test run).
fn project<'a>(root: &'a Path, args: &'a [PathBuf], framework_args: &'a [String]) -> Project<'a> {
    let tool_aye = aye_config::AyeSettings::load(root).unwrap();
    Project {
        args,
        framework_args,
        tool_aye: Box::leak(Box::new(tool_aye)),
        ..Project::new(root, root)
    }
}

/// The adapters a run uses, by name.
fn adapters(names: &[&'static str]) -> Vec<AdapterName> {
    names.iter().copied().map(AdapterName::from).collect()
}

fn recognizer(root: &Path) -> DoctestRecognizer {
    DoctestRecognizer::with_env(&project(root, &[], &[]), None, None).unwrap()
}

/// Recognize every claimed file under `root` the way aye-collect walks it:
/// entries sorted by name, directories recursed in place.
fn collect(root: &Path) -> Vec<(String, Recognition)> {
    let recognizer = recognizer(root);
    let mut out = Vec::new();
    for start in recognizer.roots(&project(root, &[], &[])) {
        walk(&recognizer, root, &start, &mut out);
    }
    out
}

fn walk(
    recognizer: &DoctestRecognizer,
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
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if recognizer.claim(&rel, Requested::Found).has_tests() {
            let file = Parsed::read(root, &rel);
            out.push((
                rel,
                recognizer.recognize(&file.file(), Requested::Found, &[]),
            ));
        }
    }
}

/// Recognize one module, `pkg/mod.py` in a project with doctest modules on.
fn recognize(source: &str) -> Recognition {
    let dir = tempfile::tempdir().unwrap();
    write(
        dir.path(),
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(dir.path(), "pkg/__init__.py", "");
    write(dir.path(), "pkg/mod.py", source);
    let recognizer = recognizer(dir.path());
    let file = Parsed::read(dir.path(), "pkg/mod.py");
    recognizer.recognize(&file.file(), Requested::Found, &[])
}

/// The names of a module's tests, and whether it is refined.
fn names(source: &str) -> (Vec<String>, bool) {
    let recognition = recognize(source);
    let names = recognition
        .cases
        .iter()
        .map(|c| c.id.as_str().trim_start_matches("pkg/mod.py::").to_string())
        .collect();
    (names, recognition.refine.is_some())
}

fn exact(source: &str) -> Vec<String> {
    let recognition = recognize(source);
    assert!(
        recognition.refine.is_none(),
        "refined: {:?}\n{source}",
        recognition.refine
    );
    recognition
        .cases
        .iter()
        .map(|c| c.id.as_str().trim_start_matches("pkg/mod.py::").to_string())
        .collect()
}

fn refined(source: &str) -> String {
    let recognition = recognize(source);
    recognition
        .refine
        .unwrap_or_else(|| panic!("not refined:\n{source}"))
        .to_string()
}

// ---------------------------------------------------------------- the parser

#[test]
fn examples_are_counted_as_doctest_parses_them() {
    let cases = [
        (">>> 1\n1\n", 1),
        ("text\n>>> 1\n1\n>>> 2\n2\n", 2),
        // A prompt mid-line is no example.
        ("x = '>>> 1'\n", 0),
        // A comment or a bare prompt is no example.
        (">>> # comment\n", 0),
        (">>>\n", 0),
        // Continuation lines belong to the example.
        (">>> def f():\n...     return 1\n>>> f()\n1\n", 2),
        // A comment line with a continuation is still blank: `$` matches
        // before the final newline.
        (">>> # c\n...\n", 0),
        // Indentation is stripped first, then examples are found.
        ("    >>> 1\n    1\n", 1),
        // Tabs are expanded.
        ("\t>>> 1\n\t1\n", 1),
        // Want lines stop at a blank line or the next prompt.
        (">>> 1\n1\n\nprose\n>>> 2\n2", 2),
        ("", 0),
    ];
    for (text, examples) in cases {
        let parsed = parse(text);
        assert_eq!(parsed.error, None, "{text:?}");
        assert_eq!(parsed.examples, examples, "{text:?}");
    }
}

#[test]
fn what_doctest_rejects_is_an_error() {
    for text in [
        ">>>1\n",
        ">>> def f():\n...return 1\n",
        "    >>> 1\n  1\n",
        "  >>> 1\n    ... 2\n",
        ">>> 1  # doctest: ELLIPSIS\n",
        ">>> # doctest: +SKIP\n",
        ">>> # doctest: +ELLIPSIS\n...\n",
    ] {
        let parsed = parse(text);
        assert!(parsed.error.is_some(), "{text:?}: {parsed:?}");
    }
    for text in [
        ">>> 1  # doctest: +ELLIPSIS\n1\n",
        ">>> 1  # doctest: +ELLIPSIS, -NORMALIZE_WHITESPACE\n1\n",
        ">>> 1  # doctest:+NUMBER +ALLOW_UNICODE\n1\n",
        // A quote ends the directive's group, so this is no directive.
        ">>> f('# doctest: +NOPE')\n",
        // Want lines may be indented further.
        ">>> print('  x')\n  x\n",
    ] {
        let parsed = parse(text);
        assert_eq!(parsed.error, None, "{text:?}");
        assert!(parsed.unknown_options.is_empty(), "{text:?}");
    }
    let parsed = parse(">>> 1  # doctest: +MINE\n1\n");
    assert_eq!(parsed.error, None);
    assert_eq!(parsed.unknown_options, ["MINE"]);
}

#[test]
fn docstrings_are_read_as_written_and_as_python_3_13_cleans_them() {
    assert_eq!(expand_tabs("a\tb\n\tc"), "a       b\n        c");
    assert_eq!(
        clean_doc("  first\n      second\n    third\n"),
        "first\n  second\nthird\n"
    );
    assert_eq!(clean_doc("\tx\n\t  y"), "x\ny");
    assert_eq!(clean_doc("x\n   \n  y"), "x\n \ny");
    // Fine as written (3.12), wrong once cleaned? Neither here.
    let doc = docstring("Doc.\n\n    >>> 1\n    1\n    ");
    assert!(doc.examples && doc.unsure.is_none());
    // As written, the continuation is indented one space more than the
    // prompt (3.12 raises); cleaned, they agree (3.13 doesn't).
    let doc = docstring(" >>> f(\n      ... )\n    ");
    assert!(doc.unsure.is_some(), "{doc:?}");
    assert!(!docstring("no prompt here").examples);
}

#[test]
fn fnmatch_follows_python() {
    let fnmatch = |name: &str, pattern: &str| fnmatch::fnmatch(name, pattern, SETS);
    let fnmatch_ex = |pattern: &str, path: &Path| fnmatch::fnmatch_ex(pattern, path, SETS);
    assert!(fnmatch("test_x.txt", "test*.txt"));
    assert!(!fnmatch("x_test.txt", "test*.txt"));
    assert!(fnmatch("a.md", "*.[mM][dD]"));
    assert!(fnmatch("a.b", "a?b"));
    assert!(!fnmatch("a.b", "a[!.]b"));
    assert!(fnmatch("[x", "[x"));
    assert!(fnmatch("a/b/c.txt", "*.txt"));
    // Absolute paths spelled the platform's way, as the walk gives them.
    let r = std::env::temp_dir().join("r");
    assert!(fnmatch_ex("*.txt", &r.join("docs").join("a.txt")));
    assert!(fnmatch_ex("docs/*.txt", &r.join("docs").join("a.txt")));
    assert!(!fnmatch_ex("docs/*.txt", &r.join("other").join("a.txt")));
    assert!(fnmatch_ex(".*", &r.join(".git")));
}

/// The `fnmatch` answers as aye 0.0.1's copy here gave them, the same as
/// the pytest adapter's.
#[test]
fn fnmatch_answers_are_pinned() {
    aye_test::assert_fnmatch_pins(aye_test::FnmatchAnswers::Legacy, |name, pattern| {
        fnmatch::fnmatch(name, pattern, SETS)
    });
}

// ---------------------------------------------------------------- the finder

#[test]
fn every_object_the_finder_visits() {
    let names = exact(
        r#""""Module.

>>> 1
1
"""
import abc
from typing import final


class C:
    """
    >>> C
    <class 'pkg.mod.C'>
    """

    @property
    def p(self):
        """
        >>> 1
        1
        """

    @p.setter
    def p(self, value):
        """
        >>> 'ignored'
        """

    @p.deleter
    def p(self):
        pass

    @staticmethod
    def s():
        """
        >>> 1
        1
        """

    @classmethod
    def c(cls):
        """
        >>> 1
        1
        """

    @abc.abstractmethod
    def a(self):
        """
        >>> 1
        1
        """

    @final
    def f(self):
        """
        >>> 1
        1
        """

    class Inner:
        """
        >>> 1
        1
        """

        class Deeper:
            def m(self):
                """
                >>> 1
                1
                """

    async def coro(self):
        """
        >>> 1
        1
        """

    def __repr__(self):
        """
        >>> 1
        1
        """


def func():
    """
    >>> 1
    1
    """


def no_docstring():
    pass


def prose():
    """No examples."""
"#,
    );
    assert_eq!(
        names,
        [
            "pkg.mod",
            "pkg.mod.C",
            "pkg.mod.C.Inner",
            "pkg.mod.C.Inner.Deeper.m",
            "pkg.mod.C.__repr__",
            "pkg.mod.C.a",
            "pkg.mod.C.c",
            "pkg.mod.C.coro",
            "pkg.mod.C.f",
            "pkg.mod.C.p",
            "pkg.mod.C.s",
            "pkg.mod.func",
        ]
    );
}

#[test]
fn names_sort_as_python_strings() {
    let names = exact(
        r#"
def b():
    """
    >>> 1
    1
    """

def B():
    """
    >>> 1
    1
    """

def _a():
    """
    >>> 1
    1
    """

def a1():
    """
    >>> 1
    1
    """
"#,
    );
    assert_eq!(
        names,
        ["pkg.mod.B", "pkg.mod._a", "pkg.mod.a1", "pkg.mod.b"]
    );
}

#[test]
fn what_never_runs_or_holds_no_examples_is_no_test() {
    let names = exact(
        r#"
def f():
    """
    >>> 1
    1
    """

if __name__ == "__main__":
    def g():
        """
        >>> 1
        1
        """

if False:
    def h():
        """
        >>> 1
        1
        """

def comment():
    """
    >>> # only a comment
    """

def mid_line():
    """Says x >>> y."""

def raw():
    r"""
    >>> print('\\n')
    \n
    """

X = ">>> not a docstring"
"#,
    );
    assert_eq!(names, ["pkg.mod.f", "pkg.mod.raw"]);
}

#[test]
fn common_module_code_is_exact() {
    // Module-level code that is everywhere and changes nothing doctest sees.
    exact(
        r#"
"""
>>> 1
1
"""
from __future__ import annotations

import logging
import os
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Iterator

__all__ = ["add", "Cart"]
logger = logging.getLogger(__name__)
DEBUG = os.environ.get("DEBUG") == "1"
REGISTRY: dict[str, object] = {}

try:
    import numpy
except ImportError:
    numpy = None


def add(a, b):
    """
    >>> add(1, 2)
    3
    """
    return a + b


TOTAL = add(1, 2)
HANDLERS = {"add": add}


@dataclass(frozen=True)
class Cart:
    """
    >>> Cart().total()
    0
    """

    items: list = field(default_factory=list)

    def total(self):
        """
        >>> Cart([1, 2]).total()
        3
        """
        return sum(self.items)


class Special(Cart):
    def total(self):
        return 0


REGISTRY["add"] = add
print(add(1, 2))
"#,
    );
}

#[test]
fn overloads_and_calls_that_return_nothing_with_doctests_are_exact() {
    let names = exact(
        r#"
from typing import overload


@overload
def f(x: int) -> int: ...
@overload
def f(x: str) -> str: ...
def f(x):
    """
    >>> f(1)
    1
    """
    return x


def helper(values):
    """
    >>> helper([1])
    [1]
    """
    return sorted(values, key=f)


def setup():
    return list(map(f, helper([3, 1])))


TABLE = setup()
"#,
    );
    assert_eq!(names, ["pkg.mod.f", "pkg.mod.helper"]);
}

#[test]
fn modules_that_may_skip_while_imported_are_refined() {
    // pytest skips (or fails to collect) them: their doctests may not exist.
    let doc = "\"\"\"\n    >>> 1\n    1\n    \"\"\"";
    let sources = [
        format!("import pytest\npytest.skip('x', allow_module_level=True)\ndef f():\n    {doc}\n"),
        format!(
            "import sys\nimport pytest\nif sys.platform == 'nope':\n    \
             pytest.skip('x', allow_module_level=True)\ndef f():\n    {doc}\n"
        ),
        format!("import pytest\nnp = pytest.importorskip('aye_missing')\ndef f():\n    {doc}\n"),
        format!(
            "from pytest import importorskip\nimportorskip('aye_missing')\ndef f():\n    {doc}\n"
        ),
        format!(
            "import sys\nimport unittest\nif sys.maxsize:\n    \
             raise unittest.SkipTest('x')\ndef f():\n    {doc}\n"
        ),
        format!(
            "import pytest\ndef check():\n    pytest.skip('x', allow_module_level=True)\n\
             check()\ndef f():\n    {doc}\n"
        ),
    ];
    for source in &sources {
        let why = refined(source);
        assert!(
            why.contains("skip") || why.contains("calls `check`"),
            "{why}\n{source}"
        );
    }
    // Calls of that name in functions that don't run at import change nothing.
    exact(&format!(
        "import pytest\ndef f():\n    {doc}\n    pytest.skip('x')\n"
    ));
    if let Some(python) = python() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = --doctest-modules\n",
        );
        write(root, "pkg/__init__.py", "");
        for (at, source) in sources.iter().enumerate() {
            write(root, &format!("pkg/m{at}.py"), source);
        }
        let refined = differential(&python, root, true).unwrap_or_else(|r| panic!("{r}"));
        assert_eq!(refined.len(), sources.len());
    }
}

#[test]
fn what_the_source_cant_settle_is_refined() {
    let doc = "\"\"\"\n    >>> 1\n    1\n    \"\"\"";
    let cases = [
        ("an alias", format!("def f():\n    {doc}\ng = f\n")),
        (
            "a wrapper",
            format!("def f():\n    {doc}\ng = staticmethod(f)\n"),
        ),
        (
            "a docstring copied",
            format!(
                "import functools\ndef f():\n    {doc}\n@functools.wraps(f)\ndef g():\n    pass\n"
            ),
        ),
        (
            "a decorator",
            format!("import functools\n@functools.cache\ndef f():\n    {doc}\n"),
        ),
        (
            "a local decorator",
            format!("def deco(f):\n    return f\n@deco\ndef f():\n    {doc}\n"),
        ),
        (
            "a class decorator",
            format!("def deco(c):\n    return c\n@deco\nclass C:\n    {doc}\n"),
        ),
        (
            "a conditional definition",
            format!("import sys\nif sys.platform:\n    def f():\n        {doc}\n"),
        ),
        (
            "a definition in try",
            format!("try:\n    def f():\n        {doc}\nexcept ImportError:\n    pass\n"),
        ),
        ("a rebinding", format!("def f():\n    {doc}\nf = None\n")),
        (
            "a redefinition",
            format!("def f():\n    {doc}\ndef f():\n    pass\n"),
        ),
        ("a del", format!("def f():\n    {doc}\ndel f\n")),
        (
            "a star import after",
            format!("def f():\n    {doc}\nfrom os.path import *\n"),
        ),
        ("__test__", "__test__ = {'x': '>>> 1\\n1\\n'}\n".to_string()),
        ("a module __doc__", format!("{doc}\n__doc__ = 'other'\n")),
        (
            "a class __doc__",
            "class C:\n    __doc__ = '>>> 1\\n1\\n'\n".to_string(),
        ),
        (
            "a class __module__",
            format!("class C:\n    {doc}\n    __module__ = 'other'\n"),
        ),
        (
            "an attribute set",
            format!("def f():\n    {doc}\nf.__doc__ = 'x'\n"),
        ),
        (
            "a member added",
            "class C:\n    pass\nC.m = lambda self: 1\n".to_string(),
        ),
        (
            "setattr",
            format!("class C:\n    {doc}\nsetattr(C, 'x', 1)\n"),
        ),
        ("globals()", "globals()['f'] = len\n".to_string()),
        ("exec", "exec('def f(): pass')\n".to_string()),
        (
            "type()",
            "X = type('X', (), {'__doc__': '>>> 1'})\n".to_string(),
        ),
        (
            "sys.modules",
            "import sys\nsys.modules[__name__].x = 1\n".to_string(),
        ),
        (
            "a metaclass",
            format!("class M(type):\n    pass\nclass C(metaclass=M):\n    {doc}\n"),
        ),
        (
            "__init_subclass__",
            "class B:\n    def __init_subclass__(cls):\n        pass\nclass C(B):\n    pass\n"
                .to_string(),
        ),
        (
            "a factory called at import",
            format!(
                "def make():\n    def inner():\n        {doc}\n    return inner\nhelper = make()\n"
            ),
        ),
        (
            "a function returning an alias through a local",
            format!(
                "def f():\n    {doc}\ndef make():\n    g = f\n    return g\nx = None\nx = make()\n"
            ),
        ),
        (
            "a function returning an alias from a loop",
            format!(
                "def f():\n    {doc}\ndef make():\n    for g in (f,):\n        return g\nx = make()\n"
            ),
        ),
        (
            "an instance of a descriptor with doctests",
            format!(
                "class D:\n    {doc}\n    def __get__(self, obj, owner=None):\n        return 1\nclass C:\n    attr = D()\n"
            ),
        ),
        (
            "a class-body helper returning an alias",
            format!("def f():\n    {doc}\nclass C:\n    def _make():\n        return f\n    other = _make()\n"),
        ),
        (
            "a conditional registrar",
            "try:\n    import nope\nexcept ImportError:\n    def install():\n        globals()['x'] = 1\ninstall()\n".to_string(),
        ),
        (
            "a registrar called at import",
            "def install():\n    globals()['x'] = 1\ninstall()\n".to_string(),
        ),
        (
            "a global",
            format!("def f():\n    {doc}\ndef g():\n    global f\n    f = None\n"),
        ),
        ("a raise", "raise ImportError('no')\n".to_string()),
        (
            "a parse error",
            "def f():\n    \"\"\"\n    >>>1\n    \"\"\"\n".to_string(),
        ),
        (
            "an unknown option",
            "def f():\n    \"\"\"\n    >>> 1  # doctest: +MINE\n    1\n    \"\"\"\n".to_string(),
        ),
        (
            "an option on a comment",
            "def f():\n    \"\"\"\n    >>> # doctest: +SKIP\n    \"\"\"\n".to_string(),
        ),
        (
            "a for loop alias",
            format!("def f():\n    {doc}\nfor g in [f]:\n    pass\n"),
        ),
        (
            "a walrus alias",
            format!("def f():\n    {doc}\nprint(g := f)\n"),
        ),
        (
            "a property alias",
            format!(
                "class C:\n    @property\n    def x(self):\n        {doc}\n    @x.setter\n    def y(self, v):\n        pass\n"
            ),
        ),
        (
            "a getter",
            format!(
                "class C:\n    @property\n    def x(self):\n        {doc}\n    @x.getter\n    def x(self):\n        pass\n"
            ),
        ),
        (
            "cached_property",
            format!(
                "import functools\nclass C:\n    @functools.cached_property\n    def x(self):\n        {doc}\n"
            ),
        ),
        (
            "a shadowed decorator",
            format!(
                "def final(f):\n    return f\nclass C:\n    @final\n    def x(self):\n        {doc}\n"
            ),
        ),
        (
            "module-level staticmethod",
            format!("@staticmethod\ndef f():\n    {doc}\n"),
        ),
    ];
    for (what, source) in cases {
        let recognition = recognize(&source);
        assert!(
            recognition.refine.is_some(),
            "{what}: not refined\n{source}"
        );
    }
}

#[test]
fn carriage_returns_fold_unless_an_escape_may_make_one() {
    let found = exact("def f():\r\n    \"\"\"\r\n    >>> 1\r\n    1\r\n    \"\"\"\r\n");
    assert_eq!(found, ["pkg.mod.f"]);
    // An escaped carriage return in an LF file is kept: `>>> 1\r` lacks a
    // blank after its prompt.
    let (_, is_refined) = names("def f():\n    \"\"\"\n    >>>\\r1\n    \"\"\"\n");
    assert!(is_refined);
    refined("def f():\r\n    \"\"\"\r\n    >>> 1\\r\r\n    1\r\n    \"\"\"\r\n");
}

#[test]
fn cases_carry_keywords_affinity_and_a_span() {
    let recognition = recognize("def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n");
    let case = &recognition.cases[0];
    assert_eq!(case.id.as_str(), "pkg/mod.py::pkg.mod.f");
    assert_eq!(case.adapter, "doctest");
    assert_eq!(case.keywords, ["pkg", "mod.py", "pkg.mod.f"]);
    assert_eq!(case.affinity, ["import:pkg.mod", "pkg/mod.py"]);
    assert_eq!(case.certainty, Certainty::Exact);
    let span = case.span.as_ref().unwrap();
    assert_eq!(span.path, "pkg/mod.py");
    assert_eq!(span.line, Some(2));
}

// ---------------------------------------------------------------- configuration

fn config(
    files: &[(&str, &str)],
    framework_args: &[&str],
    env: Option<&str>,
) -> Option<DoctestRecognizer> {
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        write(dir.path(), rel, body);
    }
    let args: Vec<String> = framework_args.iter().map(ToString::to_string).collect();
    let recognizer =
        DoctestRecognizer::with_env(&project(dir.path(), &[], &args), env, None).unwrap();
    recognizer.config().active().then_some(recognizer)
}

#[test]
fn nothing_is_collected_without_configuration() {
    assert!(config(&[], &[], None).is_none());
    assert!(config(&[("pyproject.toml", "[project]\nname = 'x'\n")], &[], None).is_none());
    // With Doctest itself: `prepare` returns no recognizer.
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Doctest
            .prepare(&project(dir.path(), &[], &[]))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_pytest_configuration_collects_test_txt_files() {
    let recognizer = config(&[("pytest.ini", "[pytest]\n")], &[], None).unwrap();
    assert!(!recognizer.config().modules.value);
    assert_eq!(recognizer.config().globs.value, ["test*.txt"]);
    assert_eq!(
        recognizer.claim("docs/test_guide.txt", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("docs/guide.txt", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("docs/guide.txt", Requested::Named),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("docs/guide.rst", Requested::Named),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("docs/guide.md", Requested::Named),
        Claim::Ignore
    );
    assert_eq!(recognizer.claim("mod.py", Requested::Found), Claim::Ignore);
}

#[test]
fn beside_pytest_test_txt_files_need_no_pytest_configuration() {
    // pytest collects `test*.txt` by default, configured or not: beside the
    // pytest adapter, so does doctest's.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(root, "docs/test_guide.txt", "Guide.\n\n>>> 1 + 1\n2\n");
    write(root, "docs/test_prose.txt", "Only prose.\n");
    let among = |names: &[&'static str], args: &[String]| {
        Doctest
            .prepare_among(&project(root, &[], args), &adapters(names))
            .unwrap()
    };
    assert!(among(&["doctest"], &[]).is_none());
    assert!(among(&["unittest", "doctest"], &[]).is_none());
    assert!(Doctest.prepare(&project(root, &[], &[])).unwrap().is_none());
    let recognizer = among(&["pytest", "doctest"], &[]).unwrap();
    assert_eq!(
        recognizer.claim("docs/test_guide.txt", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("docs/guide.txt", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(recognizer.claim("mod.py", Requested::Found), Claim::Ignore);
    let settings = recognizer.settings();
    let glob = settings.iter().find(|s| s.name == "glob").unwrap();
    assert_eq!(
        (
            glob.value.as_str(),
            glob.source.display_row(root).to_string()
        ),
        ("test*.txt", "default".to_string())
    );
    // The other rules still hold: `-p no:doctest`, `[tool.aye.doctest]`.
    let blocked = ["-p".to_string(), "no:doctest".to_string()];
    assert!(among(&["doctest", "pytest"], &blocked).is_none());
    write(
        root,
        "pyproject.toml",
        "[project]\nname = 'x'\n[tool.aye.doctest]\nglob = ['*.md']\n",
    );
    let configured = among(&["pytest", "doctest"], &[]).unwrap();
    assert_eq!(
        configured.claim("docs/test_guide.txt", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        configured.claim("README.md", Requested::Found),
        Claim::Tests
    );
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    // What pytest itself collects there, text files without examples none.
    if let Some(python) = python() {
        let theirs = live(&python, root);
        assert_eq!(theirs.ids, ["docs/test_guide.txt::test_guide.txt"]);
        let found: Vec<String> = ["docs/test_guide.txt", "docs/test_prose.txt"]
            .iter()
            .flat_map(|rel| {
                let file = Parsed::read(root, rel);
                let recognition = recognizer.recognize(&file.file(), Requested::Found, &[]);
                assert!(recognition.refine.is_none());
                recognition.cases.into_iter().map(|c| c.id.into_string())
            })
            .collect();
        assert_eq!(found, theirs.ids);
    }
}

#[test]
fn doctest_modules_comes_from_any_of_pytests_options() {
    let ini = [("tox.ini", "[pytest]\naddopts = -ra --doctest-modules\n")];
    assert!(config(&ini, &[], None).unwrap().config().modules.value);
    let pyproject = [(
        "pyproject.toml",
        "[tool.pytest.ini_options]\naddopts = [\"--doctest-modules\"]\n",
    )];
    assert!(
        config(&pyproject, &[], None)
            .unwrap()
            .config()
            .modules
            .value
    );
    let native = [(
        "pytest.toml",
        "[pytest]\naddopts = [\"--doctest-modules\"]\n",
    )];
    assert!(config(&native, &[], None).unwrap().config().modules.value);
    let setup = [("setup.cfg", "[tool:pytest]\naddopts = --doctest-modules\n")];
    assert!(config(&setup, &[], None).unwrap().config().modules.value);
    let bare = [("pytest.ini", "[pytest]\n")];
    assert!(
        config(&bare, &["--doctest-modules"], None)
            .unwrap()
            .config()
            .modules
            .value
    );
    assert!(
        config(&bare, &[], Some("--doctest-modules"))
            .unwrap()
            .config()
            .modules
            .value
    );
    // `-p no:doctest` turns pytest's doctests off.
    let blocked = config(&bare, &["--doctest-modules", "-p", "no:doctest"], None);
    assert!(blocked.is_none());
    // No configuration at all, but the option after `--`.
    let recognizer = config(&[], &["--doctest-modules"], None).unwrap();
    assert!(recognizer.config().modules.value);
    assert_eq!(
        recognizer.config().globs.value,
        [] as [std::string::String; 0]
    );
}

#[test]
fn doctest_globs_replace_the_default() {
    let recognizer = config(
        &[(
            "pytest.ini",
            "[pytest]\naddopts = --doctest-glob=*.md --doctest-glob *.rst\n",
        )],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(recognizer.config().globs.value, ["*.md", "*.rst"]);
    assert_eq!(
        recognizer.claim("README.md", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("docs/a.rst", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("test_a.txt", Requested::Found),
        Claim::Ignore
    );
}

#[test]
fn tool_aye_doctest_wins_over_pytest() {
    let pyproject = |body: &str| {
        format!(
            "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\ndoctest_optionflags = \"NORMALIZE_WHITESPACE\"\n\n[tool.aye.doctest]\n{body}"
        )
    };
    let off = pyproject("modules = false\nglob = []\n");
    assert!(config(&[("pyproject.toml", &off)], &[], None).is_none());
    let on = pyproject(
        "glob = [\"*.md\"]\noptionflags = [\"FAIL_FAST\", \"NUMBER\"]\nencoding = \"latin-1\"\n",
    );
    let recognizer = config(&[("pyproject.toml", &on)], &[], None).unwrap();
    let config = recognizer.config();
    assert!(config.modules.value);
    assert_eq!(config.globs.value, ["*.md"]);
    assert_eq!(
        config.optionflags.value,
        ["NORMALIZE_WHITESPACE", "FAIL_FAST", "NUMBER"]
    );
    assert_eq!(config.encoding.value, "latin-1");
    let settings = recognizer.settings();
    let glob = settings.iter().find(|s| s.name == "glob").unwrap();
    assert_eq!(
        glob.source.display_row(&recognizer.root).to_string(),
        "pyproject.toml [tool.aye.doctest]"
    );
    // Only aye's own: modules without pytest.
    let only = "[tool.aye.doctest]\nmodules = true\n";
    let recognizer = config_of(&[("pyproject.toml", only)]);
    assert!(recognizer.config().modules.value);
    assert_eq!(
        recognizer.config().globs.value,
        [] as [std::string::String; 0]
    );
    assert_eq!(recognizer.config().optionflags.value, ["ELLIPSIS"]);
}

fn config_of(files: &[(&str, &str)]) -> DoctestRecognizer {
    config(files, &[], None).unwrap()
}

#[test]
fn bad_settings_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    for (body, expected) in [
        (
            "[tool.aye.doctest]\nmodule = true\n",
            "unknown key \"module\"",
        ),
        (
            "[tool.aye.doctest]\nmodules = 1\n",
            "modules: expected true or false",
        ),
        (
            "[tool.aye.doctest]\noptionflags = [\"NOPE\"]\n",
            "unknown flag \"NOPE\"",
        ),
        (
            "[tool.aye.doctest]\nglob = [1]\n",
            "glob: expected a list of strings",
        ),
        (
            "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\ndoctest_optionflags = \"SKIP\"\n",
            "doctest_optionflags: unknown flag \"SKIP\"",
        ),
        (
            "[tool.pytest.ini_options]\naddopts = \"--doctest-modules --doctest-report=fancy\"\n",
            "--doctest-report: invalid choice",
        ),
    ] {
        write(dir.path(), "pyproject.toml", body);
        let err = Doctest
            .prepare(&project(dir.path(), &[], &[]))
            .err()
            .unwrap_or_else(|| panic!("no error for {body}"))
            .to_string();
        assert!(err.contains(expected), "{body}: {err}");
    }
}

#[test]
fn override_ini_and_options_after_the_separator() {
    let recognizer = config(
        &[(
            "pytest.ini",
            "[pytest]\naddopts = --doctest-modules\ndoctest_encoding = latin-1\n",
        )],
        &[
            "-o",
            "doctest_optionflags=NUMBER ELLIPSIS",
            "--doctest-report",
            "ndiff",
        ],
        Some("-odoctest_encoding=utf-16"),
    )
    .unwrap();
    assert_eq!(
        recognizer.config().optionflags.value,
        ["NUMBER", "ELLIPSIS"]
    );
    assert_eq!(recognizer.config().encoding.value, "utf-16");
    assert_eq!(recognizer.config().report, "ndiff");
    let context = recognizer.context().into_value();
    assert_eq!(context["report"], "ndiff");
    assert_eq!(context["optionflags"][0], "NUMBER");
    // `-o addopts=` from the command line replaces the file's.
    let recognizer = config(
        &[("pytest.ini", "[pytest]\naddopts = --doctest-modules\n")],
        &["-o", "addopts=--doctest-glob=*.md"],
        None,
    )
    .unwrap();
    assert!(!recognizer.config().modules.value);
    assert_eq!(recognizer.config().globs.value, ["*.md"]);
}

/// pytest's `--deselect` drops its doctest items too: the prefixes, from
/// `addopts`, `PYTEST_ADDOPTS` and the arguments after `--`, go to the
/// binary as prefixes of aye's ids.
#[test]
fn deselect_prefixes_are_given_as_ayes_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --deselect pkg/mod.py::pkg.mod.f\n",
    );
    let args = ["--deselect=docs/test_x.txt".to_string()];
    let context = DoctestRecognizer::with_env(
        &project(root, &[], &args),
        Some("--deselect pkg/other.py"),
        None,
    )
    .unwrap()
    .context()
    .into_value();
    assert_eq!(
        context["deselect"],
        serde_json::json!(["pkg/mod.py::pkg.mod.f", "pkg/other.py", "docs/test_x.txt"])
    );
    // Node ids are relative to the rootdir; aye's to the project root.
    write(root, "pyproject.toml", "[project]\nname = \"x\"\n");
    std::fs::remove_file(root.join("pytest.ini")).unwrap();
    write(
        root,
        "sub/pytest.ini",
        "[pytest]\naddopts = --doctest-modules --deselect mod.py\n",
    );
    let sub = [root.join("sub")];
    let context = DoctestRecognizer::with_env(&project(root, &sub, &[]), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(context["deselect"], serde_json::json!(["sub/mod.py"]));
}

#[test]
fn claims_follow_pytests_collect_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --ignore=skipped\n",
    );
    write(root, "setup.py", "from setuptools import setup\n");
    write(root, "sub/setup.py", "print('a script')\n");
    let recognizer = recognizer(root);
    assert_eq!(recognizer.claim("mod.py", Requested::Found), Claim::Tests);
    assert_eq!(
        recognizer.claim("pkg/test_x.py", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("conftest.py", Requested::Found),
        Claim::Tests
    );
    assert_eq!(
        recognizer.claim("__main__.py", Requested::Found),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("__main__.py", Requested::Named),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("setup.py", Requested::Named),
        Claim::Ignore
    );
    assert_eq!(
        recognizer.claim("sub/setup.py", Requested::Found),
        Claim::Tests
    );
    assert_eq!(recognizer.claim(".py", Requested::Found), Claim::Ignore);
    assert_eq!(recognizer.claim("mod.pyi", Requested::Found), Claim::Ignore);
    assert!(!recognizer.descend(&root.join("skipped")));
    assert!(!recognizer.descend(&root.join("build")));
    assert!(!recognizer.descend(&root.join(".hidden")));
    assert!(!recognizer.descend(&root.join("pkg/__pycache__")));
    assert!(recognizer.descend(&root.join("pkg")));
    write(root, "env/pyvenv.cfg", "");
    assert!(!recognizer.descend(&root.join("env")));
}

#[test]
fn text_files_are_one_case_when_they_have_examples() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "pytest.ini", "[pytest]\n");
    let recognizer = recognizer(dir.path());
    let recognize_text = |rel: &str, body: &str| {
        write(dir.path(), rel, body);
        let file = Parsed::read(dir.path(), rel);
        recognizer.recognize(&file.file(), Requested::Found, &[])
    };
    let recognition = recognize_text("docs/test_a.txt", "Prose.\r\n\r\n>>> 1\r\n1\r\n");
    assert!(recognition.refine.is_none());
    assert_eq!(recognition.cases.len(), 1);
    let case = &recognition.cases[0];
    assert_eq!(case.id.as_str(), "docs/test_a.txt::test_a.txt");
    assert_eq!(case.keywords, ["docs", "test_a.txt"]);
    assert_eq!(case.affinity, ["docs/test_a.txt"]);
    assert_eq!(
        recognize_text("test_b.txt", "No examples.\n").cases,
        [] as [aye_events::Case; 0]
    );
    // A byte order mark hides the first prompt, as `read_text` keeps it.
    assert_eq!(
        recognize_text("test_c.txt", "\u{feff}>>> 1\n1\n").cases,
        [] as [aye_events::Case; 0]
    );
    let broken = recognize_text("test_d.txt", ">>>1\n");
    assert!(broken.refine.is_some());
}

#[test]
fn modules_without_a_visible_doctest_are_refined() {
    // pytest imports every module with doctest modules on: an import error
    // is a collection error (exit 2), which only the import can tell.
    let files: &[(&str, &str)] = &[
        ("pytest.ini", "[pytest]\naddopts = --doctest-modules\n"),
        ("pkg/__init__.py", ""),
        (
            "pkg/documented.py",
            "\"\"\"No examples here.\"\"\"\npass\n...\n",
        ),
        ("pkg/broken.py", "import nonexistent_module_xyz\n"),
        ("pkg/plain.py", "def f():\n    return 1\n"),
        (
            "pkg/good.py",
            "def f():\n    \"\"\"\n    >>> f()\n    1\n    \"\"\"\n    return 1\n",
        ),
        ("test_prose.txt", "Only prose.\n"),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        write(dir.path(), rel, body);
    }
    let found = collect(dir.path());
    let refined: Vec<(&str, &str)> = found
        .iter()
        .filter_map(|(rel, r)| Some((rel.as_str(), r.refine.as_ref().map(RefineReason::as_str)?)))
        .collect();
    assert_eq!(
        refined,
        [
            ("pkg/broken.py", NO_STATIC_DOCTESTS),
            ("pkg/plain.py", NO_STATIC_DOCTESTS)
        ]
    );
    // A text file without examples is no test, as in pytest.
    let prose = found.iter().find(|(rel, _)| rel == "test_prose.txt");
    assert!(prose.is_some_and(|(_, r)| r.cases.is_empty() && r.refine.is_none()));
    if let Some(python) = python() {
        let refined = differential(&python, dir.path(), true).unwrap_or_else(|r| panic!("{r}"));
        assert_eq!(refined, ["pkg/broken.py", "pkg/plain.py"]);
    }
    // Beside the pytest adapter, which imports test modules and conftests
    // (and reports their import errors), those aren't refined twice.
    write(
        dir.path(),
        "tests/test_mod.py",
        "import nonexistent_module_xyz\n",
    );
    write(dir.path(), "tests/conftest.py", "import os\n");
    let beside = DoctestRecognizer::with_env_among(
        &project(dir.path(), &[], &[]),
        &adapters(&["pytest", "doctest"]),
        None,
        None,
    )
    .unwrap();
    let refine_of = |recognizer: &DoctestRecognizer, rel: &str| {
        let file = Parsed::read(dir.path(), rel);
        recognizer
            .recognize(&file.file(), Requested::Found, &[])
            .refine
    };
    assert_eq!(refine_of(&beside, "tests/test_mod.py"), None);
    assert_eq!(refine_of(&beside, "tests/conftest.py"), None);
    assert!(refine_of(&beside, "pkg/plain.py").is_some());
    let alone = recognizer(dir.path());
    assert!(refine_of(&alone, "tests/test_mod.py").is_some());
    assert!(refine_of(&alone, "tests/conftest.py").is_some());
}

#[test]
fn project_modules_that_set_module_refine_their_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(
        root,
        "pkg/__init__.py",
        "from ._impl import Thing\nimport os\n",
    );
    write(root, "pkg/_impl.py", "class Thing:\n    pass\n");
    write(root, "other/mod.py", "from os.path import join\n");
    // Whether the module is refined for `__module__` (it has no doctest,
    // so it is refined in any case: pytest imports it all the same).
    let check = |rel: &str| {
        let file = Parsed::read(root, rel);
        let refine = recognizer(root)
            .recognize(&file.file(), Requested::Found, &[])
            .refine;
        assert!(refine.is_some());
        refine.filter(|why| why.as_str().contains("__module__"))
    };
    assert_eq!(check("pkg/__init__.py"), None);
    write(
        root,
        "pkg/_impl.py",
        "class Thing:\n    pass\nThing.__module__ = 'pkg'\n",
    );
    assert!(check("pkg/__init__.py").is_some());
    assert_eq!(check("other/mod.py"), None);
    // Another module's name: nothing claims to be `pkg`'s.
    write(
        root,
        "pkg/_impl.py",
        "class Thing:\n    pass\nThing.__module__ = 'elsewhere'\n",
    );
    assert_eq!(check("pkg/__init__.py"), None);
    // NumPy's pattern: a helper sets it, the name is spelled where it's used.
    write(
        root,
        "pkg/_utils.py",
        "def set_module(name):\n    def deco(f):\n        f.__module__ = name\n        return f\n    return deco\n",
    );
    write(
        root,
        "pkg/_impl.py",
        "from ._utils import set_module\n@set_module(\"pkg\")\nclass Thing:\n    pass\n",
    );
    assert!(check("pkg/__init__.py").is_some());
    assert!(assigns_module("setattr(f, '__module__', name)"));
    assert!(assigns_module("    __module__ = 'x'"));
    assert!(!assigns_module("if f.__module__ == name: pass"));
    assert!(!assigns_module("print(cls.__module__)"));
}

#[test]
fn testpaths_and_arguments_are_the_roots() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\ntestpaths = src docs/*\n");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("docs/a")).unwrap();
    std::fs::create_dir_all(root.join("docs/b")).unwrap();
    let recognizer = recognizer(root);
    assert_eq!(
        recognizer.roots(&project(root, &[], &[])),
        [root.join("src"), root.join("docs/a"), root.join("docs/b")]
    );
    let args = [PathBuf::from("src")];
    let recognizer = DoctestRecognizer::with_env(&project(root, &args, &[]), None, None).unwrap();
    assert_eq!(
        recognizer.roots(&project(root, &args, &[])),
        [root.join("src")]
    );
}

#[test]
fn import_modes_and_optimization_refine_every_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --import-mode=importlib\n",
    );
    write(
        root,
        "mod.py",
        "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
    );
    let file = Parsed::read(root, "mod.py");
    let recognition = recognizer(root).recognize(&file.file(), Requested::Found, &[]);
    assert!(recognition.refine.unwrap().as_str().contains("importlib"));
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    let optimized = DoctestRecognizer::with_env(&project(root, &[], &[]), None, Some("2")).unwrap();
    assert!(
        optimized
            .recognize(&file.file(), Requested::Found, &[])
            .refine
            .is_some()
    );
    let plain = DoctestRecognizer::with_env(&project(root, &[], &[]), None, Some("1")).unwrap();
    assert!(
        plain
            .recognize(&file.file(), Requested::Found, &[])
            .refine
            .is_none()
    );
}

#[test]
fn the_executor_is_embedded() {
    let runtime = Doctest.runtime();
    assert_eq!(runtime.adapter, "doctest");
    assert_eq!(
        runtime.entry.to_string(),
        "aye_engine_doctest:DoctestExecutor"
    );
    assert_eq!(runtime.modules.modules(), [MODULE]);
    assert_eq!(runtime.session, SessionMode::PerBatch);
    assert_eq!(MODULE.name, "aye_engine_doctest");
    assert!(MODULE.source.contains("class DoctestExecutor"));
    assert_eq!(Doctest.name(), "doctest");
}

// ---------------------------------------------------------------- suites

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

/// The interpreter for live tests, with pytest: aye-test's.
fn python() -> Option<PathBuf> {
    aye_test::test_python(aye_test::Needs::Pytest)
}

/// A pytest plugin that dumps each doctest item's id and the names `-k`
/// matches, and the files that failed to collect.
const DUMP_PLUGIN: &str = r#"
import json
import os

from _pytest.mark import KeywordMatcher

ERRORS = []


def pytest_collectreport(report):
    if report.failed or (report.skipped and report.nodeid):
        ERRORS.append(report.nodeid.split("::")[0])


def pytest_collection_modifyitems(session, config, items):
    out = []
    for item in items:
        if type(item).__name__ != "DoctestItem":
            continue
        names = sorted(KeywordMatcher.from_item(item)._names)
        out.append({"id": item.nodeid, "keywords": names})
    with open(os.environ["AYE_DUMP"], "w", encoding="utf-8") as f:
        json.dump({"items": out, "errors": ERRORS}, f)
"#;

struct Live {
    ids: Vec<String>,
    keywords: Vec<Vec<String>>,
    errors: Vec<String>,
}

/// pytest's doctest items under `root`, in its order (`-p no:python`, so
/// that a package's `__init__.py` is where aye's walk puts it).
fn live(python: &Path, root: &Path) -> Live {
    let plugin = tempfile::tempdir().unwrap();
    write(plugin.path(), "aye_dump.py", DUMP_PLUGIN);
    let dump = plugin.path().join("dump.json");
    let output = Command::new(python)
        .args([
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
            "-p",
            "no:python",
            "-p",
            "aye_dump",
            "--continue-on-collection-errors",
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
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dump).unwrap()).unwrap();
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    let items = value["items"].as_array().unwrap();
    Live {
        ids: items
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_string())
            .collect(),
        keywords: items.iter().map(|i| strings(&i["keywords"])).collect(),
        errors: strings(&value["errors"]),
    }
}

/// Recognition against pytest under `root`: every file not refined matches
/// exactly, in order, keywords included. A file pytest fails to collect
/// must be refined when `errors_refined`; otherwise (an import error, which
/// only the import knows) it is left out. Returns the refined files.
fn differential(python: &Path, root: &Path, errors_refined: bool) -> Result<Vec<String>, String> {
    let theirs = live(python, root);
    let found = collect(root);
    let mut report = String::new();
    if errors_refined {
        for error in &theirs.errors {
            if !found
                .iter()
                .any(|(rel, r)| rel == error && r.refine.is_some())
            {
                let _ = writeln!(
                    report,
                    "{error}: pytest fails to collect it, aye doesn't refine it"
                );
            }
        }
    }
    let mut refined: Vec<String> = found
        .iter()
        .filter(|(_, r)| r.refine.is_some())
        .map(|(rel, _)| rel.clone())
        .collect();
    let excluded: Vec<&String> = refined.iter().chain(&theirs.errors).collect();
    let ours: Vec<&aye_events::Case> = found
        .iter()
        .filter(|(rel, _)| !excluded.contains(&rel))
        .flat_map(|(_, r)| r.cases.iter())
        .collect();
    let expected: Vec<(usize, &String)> = theirs
        .ids
        .iter()
        .enumerate()
        .filter(|(_, id)| {
            let file = id.split("::").next().unwrap_or_default();
            !excluded.iter().any(|r| *r == file)
        })
        .collect();
    let width = ours.len().max(expected.len());
    let mut differ = false;
    let mut table = String::new();
    for i in 0..width {
        let a = ours.get(i).map_or("", |c| c.id.as_str());
        let b = expected.get(i).map_or("", |(_, id)| id.as_str());
        let mark = if a == b { "  " } else { "!=" };
        differ |= a != b;
        let _ = writeln!(table, "{mark} {a:<60} {b}");
        if a == b
            && let (Some(case), Some((at, _))) = (ours.get(i), expected.get(i))
        {
            let mut mine = case.keywords.clone();
            mine.sort();
            if mine != theirs.keywords[*at] {
                let _ = writeln!(
                    report,
                    "{a}: keywords {mine:?} != {:?}",
                    theirs.keywords[*at]
                );
            }
        }
    }
    if differ {
        let _ = writeln!(report, "aye (left) vs pytest (right):\n{table}");
    }
    if report.is_empty() {
        refined.sort();
        Ok(refined)
    } else {
        Err(report)
    }
}

/// The suites against the pytest at hand.
#[test]
fn suites_match_live_pytest() {
    let Some(python) = python() else { return };
    for suite in suites::SUITES {
        let dir = materialize(suite);
        differential(&python, dir.path(), true)
            .unwrap_or_else(|report| panic!("suite {}: {report}", suite.name));
    }
}

/// Every refinement rule's sample, against pytest: what static recognition
/// keeps exact must be exactly what pytest collects.
#[test]
fn exact_samples_match_live_pytest() {
    let Some(python) = python() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(root, "pkg/__init__.py", "");
    for (at, source) in samples().iter().enumerate() {
        write(root, &format!("pkg/m{at:02}.py"), source);
    }
    differential(&python, root, true).unwrap_or_else(|report| panic!("{report}"));
}

/// A directory of real code (`AYE_DOCTEST_CORPUS`, with a pytest
/// configuration turning doctest modules on) against pytest: every file
/// static recognition keeps exact must match pytest exactly. Prints how
/// many files were refined, and why. Run it with each Python version's
/// pytest (`AYE_TEST_PYTHON`).
#[test]
#[ignore = "needs AYE_DOCTEST_CORPUS"]
#[expect(
    clippy::disallowed_methods,
    reason = "the tests' own settings, not the environment aye reads"
)]
fn corpus_matches_live_pytest() {
    let (Some(python), Some(corpus)) = (python(), std::env::var_os("AYE_DOCTEST_CORPUS")) else {
        eprintln!("skipped: set AYE_DOCTEST_CORPUS (and AYE_TEST_PYTHON)");
        return;
    };
    let root = PathBuf::from(corpus);
    // `AYE_DOCTEST_CORPUS_STRICT`: every file pytest fails to collect must
    // be refined (when its failure is a docstring's, not an import's).
    let strict = std::env::var_os("AYE_DOCTEST_CORPUS_STRICT").is_some();
    let refined = differential(&python, &root, strict).unwrap_or_else(|report| panic!("{report}"));
    let found = collect(&root);
    let exact: usize = found
        .iter()
        .filter(|(_, r)| r.refine.is_none())
        .map(|(_, r)| r.cases.len())
        .sum();
    eprintln!(
        "{} files, {} refined, {exact} exact cases",
        found.len(),
        refined.len()
    );
    for (rel, recognition) in &found {
        if let Some(why) = &recognition.refine {
            eprintln!("  refined {rel}: {why}");
        }
    }
}

/// Module sources the finder keeps exact.
fn samples() -> Vec<String> {
    vec![
        "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n".into(),
        "class C:\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n    @property\n    def p(self):\n        \"\"\"\n        >>> 1\n        1\n        \"\"\"\n    @p.setter\n    def p(self, v):\n        \"\"\"\n        >>> 2\n        \"\"\"\n".into(),
        "\"\"\"\n>>> 1\n1\n\"\"\"\nimport os\nX = os.sep\nif X:\n    Y = 1\n".into(),
        "def f():\n    \"\"\"\n    >>> # comment\n    \"\"\"\ndef g():\n    \"\"\"\n    >>> g()\n    \"\"\"\n".into(),
        "import enum\nclass E(enum.Enum):\n    A = 1\n    def m(self):\n        \"\"\"\n        >>> E.A.m()\n        \"\"\"\n".into(),
        "def outer():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n    def inner():\n        \"\"\"\n        >>> 2\n        \"\"\"\n    return 1\n".into(),
    ]
}

// ---------------------------------------------------------------- fixes

/// pytest's `filterwarnings` and `-W` reach the executor, which applies
/// them as pytest does around collection and each test.
#[test]
fn warning_filters_come_from_pytests_configuration() {
    let recognizer = config(
        &[(
            "pytest.ini",
            "[pytest]\naddopts = --doctest-modules -W error::UserWarning\nfilterwarnings =\n    error\n    ignore::DeprecationWarning\n",
        )],
        &["-Wignore::FutureWarning", "--pythonwarnings=always"],
        Some("-W once"),
    )
    .unwrap();
    let context = recognizer.context().into_value();
    assert_eq!(
        context["filterwarnings"],
        serde_json::json!(["error", "ignore::DeprecationWarning"])
    );
    assert_eq!(
        context["pythonwarnings"],
        serde_json::json!([
            "error::UserWarning",
            "once",
            "ignore::FutureWarning",
            "always"
        ])
    );
    let toml = config(
        &[(
            "pyproject.toml",
            "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\nfilterwarnings = [\"error\"]\n",
        )],
        &["-o", "filterwarnings="],
        None,
    )
    .unwrap();
    assert_eq!(
        toml.context().into_value()["filterwarnings"],
        serde_json::json!([])
    );
    // `-W`'s value is no option: `-W -x` doesn't stop at the first failure.
    let valued = config(
        &[("pytest.ini", "[pytest]\naddopts = --doctest-modules\n")],
        &["-W", "error", "-k", "-x"],
        None,
    )
    .unwrap();
    assert_eq!(
        valued.context().into_value()["maxfail"],
        serde_json::Value::Null
    );
}

/// pytest's `-x`, `--maxfail` and JUnit options in its configuration are the
/// run's when the command line gives none, as beside the pytest adapter.
#[test]
fn maxfail_and_the_junit_report_come_from_pytests_options() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pyproject.toml",
        "[tool.aye]\nadapters = [\"doctest\"]\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules -x --junitxml=out.xml\"\njunit_family = \"xunit1\"\n",
    );
    let context = DoctestRecognizer::with_env(&project(root, &[], &[]), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(context["maxfail"], 1);
    assert_eq!(
        context["junitxml"],
        root.join("out.xml").to_string_lossy().as_ref()
    );
    assert_eq!(context["junit_family"], "xunit1");
    let cases: [(&[&str], Option<&str>, serde_json::Value); 6] = [
        (&["--maxfail", "3"], None, 3.into()),
        (&["--maxfail=0"], None, serde_json::Value::Null),
        (&["-vx"], None, 1.into()),
        (&[], Some("--maxfail 2"), 2.into()),
        (&["--exitfirst", "--maxfail=5"], None, 5.into()),
        (
            &["--junit-xml", "env.xml", "--junitprefix=ci"],
            None,
            1.into(),
        ),
    ];
    for (args, env, maxfail) in cases {
        let args: Vec<String> = args.iter().map(ToString::to_string).collect();
        let context = DoctestRecognizer::with_env(&project(root, &[], &args), env, None)
            .unwrap()
            .context()
            .into_value();
        assert_eq!(context["maxfail"], maxfail, "{args:?} {env:?}");
    }
    let args = [
        "--junit-xml".to_string(),
        "env.xml".to_string(),
        "--junitprefix=ci".to_string(),
    ];
    let context = DoctestRecognizer::with_env(&project(root, &[], &args), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(
        context["junitxml"],
        root.join("env.xml").to_string_lossy().as_ref()
    );
    assert_eq!(context["junit_prefix"], "ci");
    // An empty path turns the report off, as in pytest.
    let args = ["--junitxml=".to_string()];
    let context = DoctestRecognizer::with_env(&project(root, &[], &args), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(context["junitxml"], serde_json::Value::Null);
}

#[test]
fn continue_on_failure_reaches_the_executor() {
    let plain = config(
        &[("pytest.ini", "[pytest]\naddopts = --doctest-modules\n")],
        &[],
        None,
    );
    assert_eq!(
        plain.unwrap().context().into_value()["continue_on_failure"],
        false
    );
    let on = config(
        &[(
            "pytest.ini",
            "[pytest]\naddopts = --doctest-modules --doctest-continue-on-failure\n",
        )],
        &[],
        None,
    );
    assert_eq!(
        on.unwrap().context().into_value()["continue_on_failure"],
        true
    );
}

/// The conftests pytest imports before collecting, for the executor to
/// import them too: the initial ones (the roots' and their `test*`
/// directories', within the confcutdir), none with `--noconftest`.
#[test]
fn the_executor_is_told_pytests_initial_conftests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(root, "conftest.py", "");
    write(root, "tests/conftest.py", "");
    write(root, "app/conftest.py", "");
    write(root, "app/mod.py", "");
    let outside = root.parent().unwrap().join("conftest.py");
    assert!(!outside.exists());
    let context = recognizer(root).context().into_value();
    let names = |context: &serde_json::Value| -> Vec<String> {
        context["conftests"]["initial"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                Path::new(p.as_str().unwrap())
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    };
    assert_eq!(names(&context), ["conftest.py", "tests/conftest.py"]);
    assert_eq!(
        context["conftests"]["confcutdir"],
        root.to_string_lossy().as_ref()
    );
    let args = [PathBuf::from("app/mod.py")];
    let context = DoctestRecognizer::with_env(&project(root, &args, &[]), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(names(&context), ["conftest.py", "app/conftest.py"]);
    let off = ["--noconftest".to_string()];
    let context = DoctestRecognizer::with_env(&project(root, &[], &off), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(context["conftests"], serde_json::Value::Null);
    let cut = ["--confcutdir=app".to_string()];
    let context = DoctestRecognizer::with_env(&project(root, &args, &cut), None, None)
        .unwrap()
        .context()
        .into_value();
    assert_eq!(names(&context), ["app/conftest.py"]);
}

/// A `.txt` or `.rst` file named on the command line is a doctest whatever
/// the globs, even when nothing else is enabled.
#[test]
fn a_named_text_file_is_a_doctest_with_nothing_enabled() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pyproject.toml",
        "[tool.aye]\nadapters = [\"doctest\"]\n",
    );
    write(root, "guide.rst", ">>> 1 + 1\n3\n");
    assert!(Doctest.prepare(&project(root, &[], &[])).unwrap().is_none());
    let args = [PathBuf::from("guide.rst")];
    for names in [&["doctest"][..], &["doctest", "unittest"]] {
        let recognizer = Doctest
            .prepare_among(&project(root, &args, &[]), &adapters(names))
            .unwrap()
            .expect("a recognizer for the named file");
        assert_eq!(
            recognizer.claim("guide.rst", Requested::Named),
            Claim::Tests
        );
        assert_eq!(
            recognizer.claim("other.rst", Requested::Found),
            Claim::Ignore
        );
        assert_eq!(recognizer.claim("mod.py", Requested::Found), Claim::Ignore);
    }
    // A name that isn't a file is no reason.
    let missing = [PathBuf::from("missing.rst")];
    assert!(
        Doctest
            .prepare(&project(root, &missing, &[]))
            .unwrap()
            .is_none()
    );
}

/// An object whose `__module__` is computed may claim any module: its
/// importers are refined, whatever they spell in quotes.
#[test]
fn a_computed_module_refines_the_importers() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(
        root,
        "pkg/__init__.py",
        "\"\"\"The package.\n\n>>> 1 + 1\n2\n\"\"\"\nfrom ._impl import func\n",
    );
    let check = || {
        let file = Parsed::read(root, "pkg/__init__.py");
        recognizer(root)
            .recognize(&file.file(), Requested::Found, &[])
            .refine
            .filter(|why| why.as_str().contains("__module__"))
    };
    let computed = [
        "from ._utils import public_module\n\n@public_module\ndef func():\n    pass\n",
        "def func():\n    pass\n\nfunc.__module__ = __package__\n",
        "class Func:\n    __module__ = __name__.rpartition('.')[0]\n\nfunc = Func\n",
        "def func():\n    pass\n\nsetattr(func, '__module__', __package__)\n",
    ];
    write(
        root,
        "pkg/_utils.py",
        "def public_module(f):\n    f.__module__ = f.__module__.rpartition('.')[0]\n    return f\n",
    );
    for impl_ in computed {
        write(root, "pkg/_impl.py", impl_);
        if impl_.contains("public_module") {
            assert!(check().is_some(), "{impl_}");
            continue;
        }
        write(root, "pkg/_utils.py", "");
        assert!(check().is_some(), "{impl_}");
    }
    // A string, or a parameter its callers spell (NumPy's `set_module`),
    // is no computed name.
    assert!(!computes_module("f.__module__ = 'pkg'\n"));
    assert!(!computes_module(
        "def set_module(name):\n    def deco(f):\n        f.__module__ = name\n        return f\n    return deco\n"
    ));
    assert!(computes_module(
        "f.__module__ = f.__module__.rpartition('.')[0]\n"
    ));
    assert!(computes_module("type('X', (), {'__module__': __name__})\n"));
    write(root, "pkg/_utils.py", "");
    write(root, "pkg/_impl.py", "def func():\n    pass\n");
    assert_eq!(check(), None);
}

/// A `:=` in a default, an annotation or a lambda's default binds in the
/// module: a name with doctests rebound there is refined.
#[test]
fn walruses_where_a_function_is_made_are_seen() {
    let rebinding = [
        "def g(a=(target := 1)):\n    pass\n",
        "def g(*, a=(target := 1)):\n    pass\n",
        "def g(a: (target := int)):\n    pass\n",
        "def g() -> (target := int):\n    pass\n",
        "async def g(a=(target := 1)):\n    pass\n",
        "h = lambda a=(target := 1): a\n",
        "z: (target := int) = 1\n",
    ];
    for rebind in rebinding {
        let source = format!(
            "def target():\n    \"\"\"\n    >>> 'x'\n    'x'\n    \"\"\"\n\n\n{rebind}\n\ndef keep():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n"
        );
        let (names, refined) = names(&source);
        assert!(refined, "{rebind}");
        assert!(names.contains(&"pkg.mod.keep".to_string()), "{rebind}");
    }
    // In a method's default, it binds in the class: the module's stays.
    let source = "def target():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n\nclass C:\n    def m(self, a=(target := 1)):\n        pass\n";
    assert_eq!(exact(source), ["pkg.mod.target"]);
}

/// With `PY_IGNORE_IMPORTMISMATCH=1`, pytest takes whatever module it finds
/// under a file's name: the import decides its doctests.
#[test]
fn ignoring_import_mismatches_refines_modules() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(
        root,
        "a/util.py",
        "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
    );
    let file = Parsed::read(root, "a/util.py");
    assert!(
        recognizer(root)
            .recognize(&file.file(), Requested::Found, &[])
            .refine
            .is_none()
    );
    let env = Env {
        ignore_import_mismatch: true,
        ..Env::default()
    };
    let ignoring =
        DoctestRecognizer::with_environment(&project(root, &[], &[]), &[], &env).unwrap();
    let recognition = ignoring.recognize(&file.file(), Requested::Found, &[]);
    assert!(
        recognition
            .refine
            .unwrap()
            .as_str()
            .contains("PY_IGNORE_IMPORTMISMATCH")
    );
}

/// A conftest hook or a plugin that may mark doctest items makes their
/// tags uncertain: `-k` and `-m` must keep them.
#[test]
fn hooks_that_may_mark_items_make_the_cases_partial() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules\n",
    );
    write(
        root,
        "lib.py",
        "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
    );
    write(root, "docs/test_guide.txt", ">>> 1\n1\n");
    let certainty = |recognizer: &DoctestRecognizer, rel: &str| {
        let file = Parsed::new(
            rel,
            root.join(rel),
            std::fs::read_to_string(root.join(rel)).unwrap(),
        );
        let recognition = recognizer.recognize(&file.file(), Requested::Found, &[]);
        assert!(recognition.refine.is_none());
        recognition.cases[0].certainty
    };
    assert_eq!(certainty(&recognizer(root), "lib.py"), Certainty::Exact);
    write(
        root,
        "sub/conftest.py",
        "import pytest\n\ndef pytest_collection_modifyitems(items):\n    pass\n",
    );
    // Wherever its conftest, pytest calls it for every item.
    assert_eq!(certainty(&recognizer(root), "lib.py"), Certainty::Partial);
    assert_eq!(
        certainty(&recognizer(root), "docs/test_guide.txt"),
        Certainty::Partial
    );
    let off = ["--noconftest".to_string()];
    let without = DoctestRecognizer::with_env(&project(root, &[], &off), None, None).unwrap();
    assert_eq!(certainty(&without, "lib.py"), Certainty::Exact);
    // `pytest_itemcollected` only for the items below its conftest.
    write(
        root,
        "sub/conftest.py",
        "def pytest_itemcollected(item):\n    pass\n",
    );
    assert_eq!(certainty(&recognizer(root), "lib.py"), Certainty::Exact);
    write(
        root,
        "sub/mod.py",
        "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
    );
    assert_eq!(
        certainty(&recognizer(root), "sub/mod.py"),
        Certainty::Partial
    );
    // Plugins aye can't read.
    std::fs::remove_file(root.join("sub/conftest.py")).unwrap();
    let plugin = ["-p".to_string(), "myplugin".to_string()];
    let with_plugin =
        DoctestRecognizer::with_env(&project(root, &[], &plugin), None, None).unwrap();
    assert_eq!(certainty(&with_plugin, "lib.py"), Certainty::Partial);
    let env = Env {
        pytest_plugins: Some("myplugin".into()),
        ..Env::default()
    };
    let env_plugins =
        DoctestRecognizer::with_environment(&project(root, &[], &[]), &[], &env).unwrap();
    assert_eq!(certainty(&env_plugins, "lib.py"), Certainty::Partial);
    let disabled = ["-p".to_string(), "no:cacheprovider".to_string()];
    let disabling =
        DoctestRecognizer::with_env(&project(root, &[], &disabled), None, None).unwrap();
    assert_eq!(certainty(&disabling, "lib.py"), Certainty::Exact);
}

/// A conftest that may skip while pytest loads it (`importorskip`, an
/// optional dependency's usual guard) makes pytest skip its directory: the
/// doctests below it are refined, so that they don't exist when it skips,
/// as in pytest ('1 passed, 1 skipped'). Its other skips, `SkipTest` (a
/// collection error in pytest) and conftests elsewhere change nothing.
#[test]
fn doctests_below_a_conftest_that_may_skip_are_refined() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --doctest-glob=*.txt\n",
    );
    let doc = "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n";
    write(root, "top.py", doc);
    write(root, "sub/m.py", doc);
    write(root, "sub/deeper/d.py", doc);
    write(root, "sub/guide.txt", ">>> 1\n1\n");
    write(root, "other/o.py", doc);
    let refine = |recognizer: &DoctestRecognizer, rel: &str| {
        let file = Parsed::read(root, rel);
        let recognition = recognizer.recognize(&file.file(), Requested::Found, &[]);
        assert_eq!(recognition.cases.len(), 1, "{rel}");
        recognition.refine
    };
    let below = ["sub/m.py", "sub/deeper/d.py", "sub/guide.txt"];
    let skips = [
        "import pytest\nnp = pytest.importorskip('numpy')\n",
        "from pytest import importorskip\nimportorskip('numpy')\n",
        "import sys\nimport pytest\nif sys.platform == 'win32':\n    pytest.skip('posix only')\n",
        "import pytest\ndef check():\n    pytest.skip('x', allow_module_level=True)\ncheck()\n",
        "import pytest\nraise pytest.skip.Exception('x')\n",
    ];
    for source in skips {
        write(root, "sub/conftest.py", source);
        let recognizer = recognizer(root);
        for rel in below {
            let why = refine(&recognizer, rel).unwrap_or_else(|| panic!("{rel}: {source}"));
            assert!(
                why.as_str().starts_with("sub/conftest.py may skip"),
                "{why}"
            );
        }
        assert_eq!(refine(&recognizer, "top.py"), None);
        assert_eq!(refine(&recognizer, "other/o.py"), None);
        // `--noconftest`: pytest loads none.
        let off = ["--noconftest".to_string()];
        let without = DoctestRecognizer::with_env(&project(root, &[], &off), None, None).unwrap();
        assert_eq!(refine(&without, "sub/m.py"), None);
    }
    let no_skips = [
        // In a fixture or a hook: not while loading.
        "import pytest\n@pytest.fixture\ndef db():\n    pytest.skip('no db')\n",
        "import pytest\ndef pytest_runtest_setup(item):\n    pytest.importorskip('numpy')\n",
        // pytest fails the directory: an error the executor reports.
        "import unittest\nraise unittest.SkipTest('x')\n",
        "import os\n",
    ];
    for source in no_skips {
        write(root, "sub/conftest.py", source);
        assert_eq!(refine(&recognizer(root), "sub/m.py"), None, "{source}");
    }
    // One pytest loads at start-up (a `test*` directory's) stops pytest
    // when it skips: no directory is skipped.
    write(root, "tests/conftest.py", skips[0]);
    write(root, "tests/t.py", doc);
    write(root, "sub/conftest.py", "import os\n");
    assert_eq!(refine(&recognizer(root), "tests/t.py"), None);
    std::fs::remove_dir_all(root.join("tests")).unwrap();
    // Above the confcutdir pytest loads none.
    write(root, "sub/conftest.py", skips[0]);
    let cut = ["--confcutdir".to_string(), "sub/deeper".to_string()];
    let cutting = DoctestRecognizer::with_env(&project(root, &[], &cut), None, None).unwrap();
    assert_eq!(refine(&cutting, "sub/deeper/d.py"), None);
    if let Some(python) = python() {
        // The referee: pytest skips `sub` (no item below it) and collects
        // the rest as aye does.
        let refined = differential(&python, root, false).unwrap_or_else(|r| panic!("{r}"));
        for rel in below {
            assert!(refined.iter().any(|r| r == rel), "{rel}: {refined:?}");
        }
    }
}

/// A root `conftest.py` collected as a doctest module after pytest imported
/// another `conftest.py` (an initial one in a `test*` directory, or one in a
/// directory that sorts before it) is pytest's "import file mismatch".
#[test]
fn a_conftest_another_one_replaced_is_an_import_file_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pyproject.toml",
        "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n",
    );
    write(root, "conftest.py", "import os\n");
    write(root, "x/conftest.py", "import os\n");
    write(root, "x/test_x.py", "def test_x():\n    pass\n");
    let recognize = |rel: &str| {
        let file = Parsed::read(root, rel);
        recognizer(root).recognize(&file.file(), Requested::Found, &[])
    };
    // `x/` sorts after `conftest.py`: no mismatch.
    let root_conftest = recognize("conftest.py");
    assert!(root_conftest.diagnostics.is_empty(), "{root_conftest:?}");
    // `app/` sorts before it.
    write(root, "app/conftest.py", "import os\n");
    let mismatched = recognize("conftest.py");
    let [error] = mismatched.diagnostics.as_slice() else {
        panic!("{mismatched:?}");
    };
    assert_eq!(error.code, DiagnosticCode::CollectionError);
    assert!(error.message.starts_with("import file mismatch"));
    assert!(
        error
            .message
            .contains(&root.join("app").join("conftest.py").display().to_string())
    );
    assert!(mismatched.cases.is_empty() && mismatched.refine.is_none());
    // app's own is fine: pytest imported it just before.
    assert_eq!(
        recognize("app/conftest.py").diagnostics,
        [] as [aye_events::Diagnostic; 0]
    );
    // An initial conftest in a `test*` directory does it too.
    std::fs::remove_dir_all(root.join("app")).unwrap();
    write(root, "tests/conftest.py", "import os\n");
    assert_eq!(recognize("conftest.py").diagnostics.len(), 1);
    // A conftest in a package is `package.conftest`: no clash.
    write(root, "tests/__init__.py", "");
    assert_eq!(
        recognize("conftest.py").diagnostics,
        [] as [aye_events::Diagnostic; 0]
    );
    if let Some(python) = python() {
        std::fs::remove_file(root.join("tests/__init__.py")).unwrap();
        let theirs = live(&python, root);
        assert!(
            theirs.errors.contains(&"conftest.py".to_string()),
            "{:?}",
            theirs.errors
        );
    }
}

/// The context's bytes as aye 0.0.1 built them (`tests/contexts.jsonl`, one
/// line each): with pytest's options and ini setting every key the
/// executor and the binary read, `PYTEST_ADDOPTS` and two conftests; then
/// with `--noconftest` and `--maxfail=0`. The refinement cache's key hashes
/// them, so a change here misses every cached entry. The project root is
/// written `<root>`. Pinned where paths are written with `/`.
#[cfg(unix)]
#[test]
fn the_context_is_pinned_byte_for_byte() {
    let set = tempfile::tempdir().unwrap();
    write(
        set.path(),
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules -x --junitxml=out.xml --junitprefix=pre \
         --deselect pkg/mod.py::pkg.mod.f --doctest-report ndiff \
         --doctest-continue-on-failure --doctest-ignore-import-errors\n\
         doctest_optionflags = NUMBER ELLIPSIS\ndoctest_encoding = latin-1\n\
         junit_family = xunit1\njunit_log_passing_tests = no\njunit_logging = all\n\
         filterwarnings =\n    error\npythonpath = src\n",
    );
    write(set.path(), "conftest.py", "");
    write(set.path(), "pkg/conftest.py", "");
    write(
        set.path(),
        "pkg/mod.py",
        "def f():\n    '''\n    >>> 1\n    1\n    '''\n",
    );
    let args = ["-p".to_string(), "someplugin".to_string()];
    let set_up =
        DoctestRecognizer::with_env(&project(set.path(), &[], &args), Some("-W once"), None)
            .unwrap();
    let off = tempfile::tempdir().unwrap();
    write(
        off.path(),
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --noconftest --maxfail=0\n",
    );
    write(off.path(), "conftest.py", "");
    let turned_off = recognizer(off.path());
    let written = |recognizer: &DoctestRecognizer, root: &Path| {
        let text = serde_json::to_string(&root.to_string_lossy()).unwrap();
        serde_json::to_string(&recognizer.context().into_value())
            .unwrap()
            .replace(text.trim_matches('"'), "<root>")
    };
    let pinned: Vec<&str> = include_str!("tests/contexts.jsonl").lines().collect();
    assert_eq!(
        [
            written(&set_up, set.path()),
            written(&turned_off, off.path())
        ],
        pinned[..]
    );
}

/// The binary writes the JUnit report by the configuration's
/// `junit_log_passing_tests`, which this adapter gives as its text, read as
/// pytest's `bool`, as the binary read it before: a text that reads neither
/// way (spaces around it included) leaves pytest's default.
#[test]
fn junit_log_passing_tests_reads_as_pytests_bool() {
    for (text, expected) in [
        ("False", Some(false)),
        ("off", Some(false)),
        ("YES", Some(true)),
        ("1", Some(true)),
        ("maybe", None),
        (" false", None),
    ] {
        let recognizer = config(
            &[(
                "pyproject.toml",
                &format!(
                    "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n\
                     junit_log_passing_tests = \"{text}\"\n"
                ),
            )],
            &[],
            None,
        )
        .unwrap();
        assert_eq!(
            recognizer.defaults().junit.log_passing_tests,
            expected,
            "{text:?}"
        );
    }
    let unset = config(
        &[("pytest.ini", "[pytest]\naddopts = --doctest-modules\n")],
        &[],
        None,
    )
    .unwrap();
    assert_eq!(
        unset.defaults().junit,
        aye_adapter::JunitDefaults::default()
    );
}
