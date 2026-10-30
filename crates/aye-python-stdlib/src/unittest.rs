//! unittest's loader rules, as `TestLoader.discover` applies them: which
//! files it imports, which classes hold tests and which bases bring none,
//! what keeps a test method a test, and when a package's `load_tests`
//! decides its tests.

/// The test method prefix (`TestLoader.testMethodPrefix`).
pub const TEST_METHOD_PREFIX: &str = "test";

/// `TestCase` itself and its subclass in the standard library: `TestCase`
/// subclasses with no tests of their own.
pub const TESTCASE_ROOTS: &[&str] = &[
    "unittest.TestCase",
    "unittest.case.TestCase",
    "unittest.IsolatedAsyncioTestCase",
    "unittest.async_case.IsolatedAsyncioTestCase",
];

/// `unittest.FunctionTestCase`, by both its paths: a base that brings a
/// `runTest` of its own, so a class deriving from it holds a test wherever
/// it is bound.
pub const RUNTEST_BASES: &[&str] = &[
    "unittest.FunctionTestCase",
    "unittest.case.FunctionTestCase",
];

/// `TestCase` subclasses of well-known installed packages, with no tests of
/// their own: importing one into a test module adds no tests. Django's,
/// Django REST framework's, Twisted's, asynctest's and aiounittest's;
/// [`MORE_KNOWN_BASES`] has more.
pub const KNOWN_BASES: &[&str] = &[
    "django.test.TestCase",
    "django.test.SimpleTestCase",
    "django.test.TransactionTestCase",
    "django.test.LiveServerTestCase",
    "django.test.testcases.TestCase",
    "django.test.testcases.SimpleTestCase",
    "django.test.testcases.TransactionTestCase",
    "django.test.testcases.LiveServerTestCase",
    "django.contrib.staticfiles.testing.StaticLiveServerTestCase",
    "rest_framework.test.APITestCase",
    "rest_framework.test.APISimpleTestCase",
    "rest_framework.test.APITransactionTestCase",
    "rest_framework.test.APILiveServerTestCase",
    "twisted.trial.unittest.TestCase",
    "twisted.trial.unittest.SynchronousTestCase",
    "asynctest.TestCase",
    "aiounittest.AsyncTestCase",
];

/// More `TestCase` subclasses of well-known installed packages with no
/// tests of their own: absl's, Tornado's, aiohttp's, Flask-Testing's,
/// pyfakefs's and testtools'. [`looks_like_tests`] knows them; the pytest
/// adapter's bases are [`TESTCASE_ROOTS`] and [`KNOWN_BASES`] alone.
pub const MORE_KNOWN_BASES: &[&str] = &[
    "absl.testing.absltest.TestCase",
    "absl.testing.parameterized.TestCase",
    "tornado.testing.AsyncTestCase",
    "tornado.testing.AsyncHTTPTestCase",
    "aiohttp.test_utils.AioHTTPTestCase",
    "flask_testing.TestCase",
    "pyfakefs.fake_filesystem_unittest.TestCase",
    "testtools.TestCase",
];

/// Whether a name imported from an installed package aye doesn't read
/// looks like a class holding tests (`Test*`, `*Test`, `*Tests`,
/// `*TestCase`), other than the well-known bases that hold none. Installed
/// packages are otherwise taken to export no test classes.
pub fn looks_like_tests(name: &str, path: &str) -> bool {
    if KNOWN_BASES.contains(&path) || MORE_KNOWN_BASES.contains(&path) {
        return false;
    }
    let own = path.rsplit('.').next().unwrap_or(path);
    [name, own].iter().any(|n| {
        n.starts_with("Test")
            || n.ends_with("Test")
            || n.ends_with("Tests")
            || n.ends_with("TestCase")
    })
}

/// Endings of class names unittest cases usually have.
const CASE_NAME_ENDINGS: [&str; 4] = ["Test", "Tests", "TestCase", "TestCases"];

/// Whether a class's name ends as unittest cases' names usually do
/// (`*Test`, `*Tests`, `*TestCase`, `*TestCases`), for a class the reader
/// can't see: the pytest adapter's guess. [`looks_like_tests`], the
/// unittest adapter's, also takes `Test*` and leaves the known bases out.
pub fn named_like_test_case(name: &str) -> bool {
    CASE_NAME_ENDINGS.iter().any(|end| name.ends_with(end))
}

/// Decorators that keep a test method callable (and its name).
pub const KEEPING_DECORATORS: &[&str] = &[
    "staticmethod",
    "classmethod",
    "unittest.skip",
    "unittest.skipIf",
    "unittest.skipUnless",
    "unittest.expectedFailure",
    "unittest.case.skip",
    "unittest.case.skipIf",
    "unittest.case.skipUnless",
    "unittest.case.expectedFailure",
    "unittest.mock.patch",
    "unittest.mock.patch.object",
    "unittest.mock.patch.dict",
    "unittest.mock.patch.multiple",
    "mock.patch",
    "mock.patch.object",
    "mock.patch.dict",
    "mock.patch.multiple",
    "functools.wraps",
    "functools.lru_cache",
    "functools.cache",
    "abc.abstractmethod",
    "typing.override",
    "typing_extensions.override",
    "typing.no_type_check",
    "contextlib.contextmanager",
    "contextlib.asynccontextmanager",
];

/// Decorators that make a method a descriptor that isn't callable.
pub const PROPERTY_DECORATORS: &[&str] = &[
    "property",
    "functools.cached_property",
    "abc.abstractproperty",
    "types.DynamicClassAttribute",
];

/// Class decorators that return the class with its test names unchanged
/// (`mock.patch` wraps each `test*` method in place).
pub const CLASS_DECORATORS: &[&str] = &[
    "unittest.skip",
    "unittest.skipIf",
    "unittest.skipUnless",
    "unittest.expectedFailure",
    "unittest.case.skip",
    "unittest.case.skipIf",
    "unittest.case.skipUnless",
    "unittest.case.expectedFailure",
    "unittest.mock.patch",
    "unittest.mock.patch.object",
    "unittest.mock.patch.dict",
    "unittest.mock.patch.multiple",
    "mock.patch",
    "mock.patch.object",
    "mock.patch.dict",
    "mock.patch.multiple",
    "typing.final",
    "typing_extensions.final",
];

/// Class decorators of the standard library that return the class (or, for
/// `dataclass(slots=True)`, a copy with the same bases and names) without
/// adding `test*` names, but may change how it is instantiated: accepted
/// on classes whose bases can't make them a `TestCase` (a dataclass the
/// test module imports, say).
pub const PLAIN_CLASS_DECORATORS: &[&str] = &[
    "dataclasses.dataclass",
    "functools.total_ordering",
    "enum.unique",
    "enum.verify",
    "typing.runtime_checkable",
    "typing_extensions.runtime_checkable",
];

/// unittest's `VALID_MODULE_NAME`, `[_a-z]\w*\.py$` with `re.IGNORECASE`:
/// the files discovery may import. Under `IGNORECASE`, `[a-z]` also takes
/// `İ`, `ı`, `ſ` and the Kelvin sign; `\w` is a letter, a digit or `_`.
pub fn valid_module_name(name: &str) -> bool {
    let Some(stem) = name
        .len()
        .checked_sub(3)
        .filter(|&at| name.is_char_boundary(at) && name[at..].eq_ignore_ascii_case(".py"))
        .map(|at| &name[..at])
    else {
        return false;
    };
    let mut chars = stem.chars();
    let first_ok = chars.next().is_some_and(|c| {
        c == '_'
            || c.is_ascii_alphabetic()
            || matches!(c, '\u{130}' | '\u{131}' | '\u{17f}' | '\u{212a}')
    });
    first_ok && chars.all(|c| c == '_' || c.is_alphanumeric())
}

/// Whether a package's `__init__.py` may define `load_tests`, which then
/// decides every test below it: the word `load_tests`, or a way to bind
/// names the source doesn't spell (a star import, a module `__getattr__`,
/// `globals()`, `setattr`, …). The unittest adapter's executor applies the
/// same rule to the same bytes (`LOAD_TESTS_MARKERS`), so both halves agree
/// on which packages take over their files.
pub fn may_define_load_tests(source: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"load_tests",
        b"__getattr__",
        b"__dir__",
        b"__dict__",
        b"__class__",
        b"globals",
        b"locals",
        b"vars",
        b"exec",
        b"setattr",
    ];
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut at = 0;
    while at < source.len() {
        if !word(source[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < source.len() && word(source[at]) {
            at += 1;
        }
        let identifier = &source[start..at];
        if MARKERS.contains(&identifier) {
            return true;
        }
        if identifier == b"import" {
            let mut next = at;
            while next < source.len()
                && (source[next].is_ascii_whitespace()
                    || source[next] == b'\\'
                    || source[next] == 0x0b)
            {
                next += 1;
            }
            if source.get(next) == Some(&b'*') {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_names_follow_valid_module_name() {
        for name in ["test_x.py", "_x.py", "T.PY", "ſx.py", "té.py", "x1.py"] {
            assert!(valid_module_name(name), "{name}");
        }
        for name in ["1x.py", "t-x.py", "t.x.py", ".py", "x.pyc", "x.p", "éx.py"] {
            assert!(!valid_module_name(name), "{name}");
        }
    }
}
