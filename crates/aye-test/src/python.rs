//! The interpreter tests run with ([`test_python`]).

use std::fmt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use crate::fixtures::workspace_root;

/// The variable that names the interpreter tests run with, instead of the
/// workspace's `.venv`. The tests' own setting: aye never reads it.
const TEST_PYTHON: &str = "AYE_TEST_PYTHON";

/// What a test needs of the interpreter it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needs {
    /// Python 3.10 or later, aye's oldest: the standard library is all
    /// aye's engine, its executors and the worker import.
    Stdlib,
    /// That, with pytest installed: what the referees run, and what aye's
    /// own `pytest` module must win over (aye never imports pytest).
    Pytest,
}

impl Needs {
    /// Python that exits 0 on an interpreter that has what is needed.
    fn probe(self) -> &'static str {
        match self {
            Self::Stdlib => "import sys; sys.exit(sys.version_info < (3, 10))",
            Self::Pytest => "import sys, pytest; sys.exit(sys.version_info < (3, 10))",
        }
    }
}

impl fmt::Display for Needs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stdlib => "Python 3.10+",
            Self::Pytest => "Python 3.10+ with pytest",
        })
    }
}

/// The interpreter for tests that need `needs`: `AYE_TEST_PYTHON` when it
/// is set and not empty, else the workspace's `.venv` (from `uv sync`),
/// when it has what they need. Looked for once per test binary. Without
/// one it says, once, on stderr, that what needs it is skipped, and gives
/// `None`: the test returns without running.
pub fn test_python(needs: Needs) -> Option<PathBuf> {
    static STDLIB: OnceLock<Option<PathBuf>> = OnceLock::new();
    static PYTEST: OnceLock<Option<PathBuf>> = OnceLock::new();
    let found = match needs {
        Needs::Stdlib => &STDLIB,
        Needs::Pytest => &PYTEST,
    };
    found.get_or_init(|| find(needs)).clone()
}

#[expect(clippy::print_stderr, reason = "a test says why it skips")]
fn find(needs: Needs) -> Option<PathBuf> {
    let candidate = given().unwrap_or_else(|| {
        let venv = workspace_root().join(".venv");
        if cfg!(windows) {
            venv.join("Scripts/python.exe")
        } else {
            venv.join("bin/python")
        }
    });
    let ok = Command::new(&candidate)
        .args(["-c", needs.probe()])
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Some(candidate)
    } else {
        eprintln!("skipping: no {needs} (run `uv sync`, or set {TEST_PYTHON})");
        None
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the tests' own settings, not the environment aye reads"
)]
fn given() -> Option<PathBuf> {
    std::env::var_os(TEST_PYTHON)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
