//! What a name or a dotted path is, by the standard library's tables
//! (aye-python-stdlib's) as this adapter reads them: the standard library
//! with unittest, dunder names, and what a class base is.

use aye_python_stdlib::modules::STDLIB;
use aye_python_stdlib::unittest::{RUNTEST_BASES, TESTCASE_ROOTS};

use super::Base;

/// Whether `path` (dotted) is in the standard library, `unittest` included.
pub(crate) fn is_stdlib(path: &str) -> bool {
    let top = path.split('.').next().unwrap_or(path);
    top == "unittest" || STDLIB.contains(&top)
}

pub(crate) fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// A base's dotted path, classified: `TestCase` itself, a plain class of
/// the standard library, or one to resolve.
pub(super) fn classify(path: &str) -> Result<Base, String> {
    if TESTCASE_ROOTS.contains(&path) {
        return Ok(Base::TestCase);
    }
    if path == "builtins.object" {
        return Ok(Base::Object);
    }
    let top = path.split('.').next().unwrap_or(path);
    if top == "doctest" || RUNTEST_BASES.contains(&path) {
        return Err(format!(
            "inherits from {path}, which brings a test of its own"
        ));
    }
    if top == "unittest" || STDLIB.contains(&top) {
        return Ok(Base::Plain(path.to_string()));
    }
    Ok(Base::Path(path.to_string()))
}
