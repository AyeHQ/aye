use std::path::Path;

use aye_test::{FnmatchAnswers, assert_fnmatch_pins};

use super::*;

use SetTranslation::{CPython, Legacy};

#[test]
fn each_translation_gives_its_pinned_answers() {
    assert_fnmatch_pins(FnmatchAnswers::CPython, |name, pattern| {
        fnmatch(name, pattern, CPython)
    });
    assert_fnmatch_pins(FnmatchAnswers::Legacy, |name, pattern| {
        fnmatch(name, pattern, Legacy)
    });
}

#[test]
fn patterns_match_as_python_fnmatch() {
    // Each checked with `fnmatch.fnmatch` on Python 3.13; `normcase`
    // folds case on Windows only.
    let cases = [
        ("test_x.py", "test*.py", true),
        ("test.py", "test*.py", true),
        ("Test_x.py", "test*.py", cfg!(windows)),
        ("test_x.py", "TEST*.PY", cfg!(windows)),
        ("test_x.pyc", "test*.py", false),
        ("x_test.py", "*_test.py", true),
        ("a.py", "?.py", true),
        ("ab.py", "?.py", false),
        ("b.py", "[a-c].py", true),
        ("d.py", "[a-c].py", false),
        ("d.py", "[!a-c].py", true),
        ("-.py", "[a-].py", true),
        ("].py", "[]].py", true),
        ("[.py", "[.py", true),
        ("x.py", "[z-a].py", false),
        ("x.py", "[!].py", false),
        ("!.py", "[!].py", false),
        ("[!].py", "[!].py", true),
        ("q.py", "[!z-a].py", true),
        ("-.py", "[a-c-e].py", true),
        ("d.py", "[a-c-e].py", false),
        ("e.py", "[a-c-e].py", true),
        ("test_a_b.py", "test_*_*.py", true),
        ("", "*", true),
        ("abc", "a*c*", true),
    ];
    for (name, pattern, want) in cases {
        assert_eq!(fnmatch(name, pattern, CPython), want, "{name} ~ {pattern}");
    }
}

#[test]
fn fnmatch_follows_python() {
    assert!(fnmatch("test_x.py", "test_*.py", Legacy));
    assert!(fnmatch("x_test.py", "*_test.py", Legacy));
    assert!(!fnmatch("test_x.pyc", "test_*.py", Legacy));
    assert!(fnmatch(".git", ".*", Legacy));
    assert!(fnmatch("{arch}", "{arch}", Legacy));
    assert!(!fnmatch("arch", "{arch}", Legacy));
    assert!(fnmatch("a.egg", "*.egg", Legacy));
    assert!(fnmatch("abc", "a?c", Legacy));
    assert!(fnmatch("abc", "a[bx]c", Legacy));
    assert!(!fnmatch("abc", "a[!b]c", Legacy));
    assert!(fnmatch("a]c", "a[]]c", Legacy));
    assert!(fnmatch("a-c", "a[x-]c", Legacy));
    assert!(fnmatch("a[c", "a[c", Legacy));
    assert!(fnmatch("a/b/c", "a*c", Legacy));
    assert!(fnmatch("", "*", Legacy));
    assert!(!fnmatch("", "?", Legacy));
    assert!(fnmatch("aXbXc", "a*b*c", Legacy));
    assert!(!fnmatch("aXbXd", "a*b*c", Legacy));
    assert!(fnmatch("b", "[a-c]", Legacy));
    assert!(!fnmatch("b", "[c-a]", Legacy));
}

#[test]
fn fnmatch_folds_case_and_separators_on_windows() {
    for sets in [CPython, Legacy] {
        assert_eq!(fnmatch("Test_X.py", "test_*.py", sets), cfg!(windows));
        assert_eq!(fnmatch("a/b.py", r"a\*.py", sets), cfg!(windows));
        assert_eq!(fnmatch(r"a\b.py", "A/*.py", sets), cfg!(windows));
        assert!(fnmatch("a/b.py", "a/*.py", sets));
    }
}

#[test]
fn fnmatch_ex_uses_the_name_or_the_whole_path() {
    // Absolute, with a drive on Windows; joined as aye joins paths.
    let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
    let path = &Path::new(root).join("tests/foo/bar/doc/test_foo.py");
    assert!(fnmatch_ex("test_*.py", path, Legacy));
    assert!(fnmatch_ex("tests/**/doc/test*.py", path, Legacy));
    assert!(fnmatch_ex("foo/*/doc/*.py", path, Legacy));
    assert!(!fnmatch_ex("other/*.py", path, Legacy));
    assert_eq!(
        fnmatch_ex("TESTS/**/doc/Test*.py", path, Legacy),
        cfg!(windows)
    );
}

#[test]
fn name_options_are_prefixes_or_globs() {
    let options = vec!["Test".to_string(), "*Suite".to_string()];
    assert!(matches_prefix_or_glob(&options, "TestThing", Legacy));
    assert!(matches_prefix_or_glob(&options, "LoginSuite", Legacy));
    assert!(!matches_prefix_or_glob(&options, "Helper", Legacy));
}
