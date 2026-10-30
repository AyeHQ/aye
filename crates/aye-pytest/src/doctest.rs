//! pytest's doctest plugin (`_pytest/doctest.py`): which files it collects
//! doctests from, and the option flags and report styles it accepts.

use std::path::Path;

use aye_python_stdlib::fnmatch::{self, SetTranslation};

/// The option flags pytest's `doctest_optionflags` accepts
/// (`_get_flag_lookup`); any other name fails pytest's collection.
pub const OPTIONFLAGS: [&str; 9] = [
    "DONT_ACCEPT_TRUE_FOR_1",
    "DONT_ACCEPT_BLANKLINE",
    "NORMALIZE_WHITESPACE",
    "ELLIPSIS",
    "IGNORE_EXCEPTION_DETAIL",
    "COMPARISON_FLAGS",
    "ALLOW_UNICODE",
    "ALLOW_BYTES",
    "NUMBER",
];

/// `--doctest-report`'s choices.
pub const REPORT_CHOICES: [&str; 5] = ["none", "cdiff", "ndiff", "udiff", "only_first_failure"];

/// What pytest's doctest plugin makes of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctestFile {
    /// A `DoctestModule` (`--doctest-modules`).
    Module,
    /// A `DoctestTextfile` (`--doctest-glob`, `test*.txt` by default).
    Text,
}

/// What pytest's doctest plugin collects from the file `name` at `abs`
/// (`pytest_collect_file`): with `modules` (`--doctest-modules`), any
/// `.py` file but `__main__.py` and a `setup.py` mentioning setuptools or
/// distutils; a text file `globs` match (read with `sets`), or a `.txt` or
/// `.rst` file on the command line (`explicit`). `ignored` says whether
/// pytest's ignore rules exclude the file, which they never do for one on
/// the command line.
pub fn collects(
    name: &str,
    abs: &Path,
    explicit: bool,
    modules: bool,
    globs: &[String],
    sets: SetTranslation,
    ignored: impl Fn() -> bool,
) -> Option<DoctestFile> {
    let extension = Path::new(name).extension().and_then(|e| e.to_str());
    // pytest never asks its ignore rules about a path on its command line.
    let ignored = || !explicit && ignored();
    if extension == Some("py") {
        if !modules || name == "__main__.py" || is_setup_py(name, abs) || ignored() {
            return None;
        }
        return Some(DoctestFile::Module);
    }
    if explicit && matches!(extension, Some("txt" | "rst")) {
        return Some(DoctestFile::Text);
    }
    if globs
        .iter()
        .any(|glob| fnmatch::fnmatch_ex(glob, abs, sets))
        && !ignored()
    {
        return Some(DoctestFile::Text);
    }
    None
}

/// pytest's `_is_setup_py`: a `setup.py` that mentions setuptools or
/// distutils, whose doctests pytest never collects.
fn is_setup_py(name: &str, abs: &Path) -> bool {
    name == "setup.py"
        && std::fs::read(abs).is_ok_and(|bytes| {
            [&b"setuptools"[..], b"distutils"]
                .iter()
                .any(|word| bytes.windows(word.len()).any(|w| w == *word))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plugin_collects_what_pytest_collects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("setup.py"), "from setuptools import setup\n").unwrap();
        std::fs::write(root.join("other_setup.py"), "").unwrap();
        let globs = ["test*.txt".to_string()];
        let collects = |name: &str, explicit: bool, modules: bool, ignored: bool| {
            let abs = root.join(name);
            collects(
                name,
                &abs,
                explicit,
                modules,
                &globs,
                SetTranslation::Legacy,
                || ignored,
            )
        };
        assert_eq!(
            collects("m.py", false, true, false),
            Some(DoctestFile::Module)
        );
        assert_eq!(collects("m.py", false, false, false), None);
        assert_eq!(collects("__main__.py", true, true, false), None);
        assert_eq!(collects("setup.py", false, true, false), None);
        assert_eq!(
            collects("other_setup.py", false, true, false),
            Some(DoctestFile::Module)
        );
        assert_eq!(
            collects("test_a.txt", false, false, false),
            Some(DoctestFile::Text)
        );
        assert_eq!(collects("notes.txt", false, false, false), None);
        assert_eq!(
            collects("notes.rst", true, false, false),
            Some(DoctestFile::Text)
        );
        assert_eq!(collects("notes.md", true, false, false), None);
        // The ignore rules hold, but not for a file on the command line.
        assert_eq!(collects("test_a.txt", false, false, true), None);
        assert_eq!(collects("m.py", false, true, true), None);
        assert_eq!(
            collects("test_a.txt", true, false, true),
            Some(DoctestFile::Text)
        );
        assert_eq!(
            collects("m.py", true, true, true),
            Some(DoctestFile::Module)
        );
    }
}
