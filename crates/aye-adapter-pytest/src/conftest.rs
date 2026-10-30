//! What `conftest.py` files (and the plugin modules they name in
//! `pytest_plugins`, register or import from) do to collection: hooks,
//! fixtures, `collect_ignore`, the modules they change and whether they
//! reach module namespaces; and what project modules define, for telling
//! whether a name a test module imports may be a unittest case.

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
