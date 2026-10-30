//! What `conftest.py` files (and the plugin modules they name in
//! `pytest_plugins`, register or import from) do to collection: hooks,
//! fixtures, `collect_ignore`, the modules they change and whether they
//! reach module namespaces; and what project modules define, for telling
//! whether a name a test module imports may be a unittest case.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Which support files count.** Hooks pytest calls per path count from
//!   the conftests in the file's directory and above, within pytest's
//!   confcutdir, the project files their `pytest_plugins` name or their
//!   `pluginmanager.register(module)` / `import_plugin("name")` load, and the
//!   project modules `-p NAME` loads (from addopts, `PYTEST_ADDOPTS` or the
//!   arguments after `--`) or `PYTEST_PLUGINS` names; with `--noconftest`,
//!   no conftest counts.
//!   Session-wide hooks (`pytest_collection`, `pytest_make_parametrize_id`,
//!   `pytest_collection_modifyitems`) count from any conftest of the
//!   project, found once by walking it, and from the plugins test modules
//!   load through `pytest_plugins`, since pytest applies them to every test
//!   once loaded. What a support module imports (`from m import name`, `from
//!   m import *`, honouring `__all__`) is followed into project files, for
//!   fixtures and hooks; a test module's imports are followed for the names
//!   its tests may request. Project files are found as Python finds them
//!   (see "Imports" below) from the importing directory's package base,
//!   the rootdir, the project root, their `src`, `pythonpath`, `PYTHONPATH`
//!   and what conftests put on `sys.path`. Installed plugins and modules
//!   are not seen. `-p
//!   no:unittest` makes `TestCase` classes plain classes (pytest's
//!   `class-has-init`), and `-p no:python` collects nothing: no `.py` file
//!   is claimed for tests, not even one on the command line, which is then
//!   `unclaimed`, as pytest's "found no collectors" (status 4), unless
//!   pytest's doctest plugin collects it.

mod fixtures;
mod hooks;
mod index;
mod ini_changes;
mod summary;

use aye_python_stdlib::unittest::{KNOWN_BASES, TESTCASE_ROOTS};

pub(crate) use fixtures::{
    BUILTIN_FIXTURES, FixtureDef, default_argnames, fixture_of, funcargnames,
};
pub(crate) use hooks::{GLOBAL_HOOKS, MARKING_HOOKS, REFINING_HOOKS};
pub(crate) use index::Supports;
pub(crate) use summary::{ClassMaker, IgnoreList, Summary, makes_classes, requested_names};

/// Packages whose names never hold tests (nor fixtures or hooks) when
/// imported into a test module or a conftest, and whose classes are never
/// unittest cases with tests of their own: the standard library's usual
/// modules and widespread libraries that ship no test cases.
const NO_TESTS: &[&str] = &[
    "_pytest",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "attr",
    "attrs",
    "base64",
    "bisect",
    "builtins",
    "calendar",
    "codecs",
    "collections",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "django",
    "email",
    "enum",
    "errno",
    "factory",
    "fnmatch",
    "fractions",
    "functools",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "httpx",
    "hypothesis",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "logging",
    "marshmallow",
    "math",
    "mock",
    "multiprocessing",
    "numbers",
    "numpy",
    "operator",
    "os",
    "pandas",
    "pathlib",
    "pickle",
    "platform",
    "pprint",
    "pydantic",
    "pytest",
    "queue",
    "random",
    "re",
    "requests",
    "rest_framework",
    "secrets",
    "shlex",
    "shutil",
    "signal",
    "socket",
    "sqlalchemy",
    "sqlite3",
    "ssl",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "sys",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "tomllib",
    "traceback",
    "types",
    "typing",
    "typing_extensions",
    "unittest",
    "urllib",
    "uuid",
    "warnings",
    "weakref",
    "xml",
    "zipfile",
    "zoneinfo",
];

/// Whether names imported from `path` are known not to be tests (nor
/// fixtures or hooks).
pub(crate) fn no_tests(path: &str) -> bool {
    let top = path.split('.').next().unwrap_or_default();
    NO_TESTS.contains(&top) || TESTCASE_ROOTS.contains(&path) || KNOWN_BASES.contains(&path)
}
