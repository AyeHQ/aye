//! aye as its tests run it ([`TestContext`]), in the hermetic environment.

use std::path::{Path, PathBuf};
use std::process::Command;

use aye_static::EnvVars;

/// The variables aye reads that a caller's shell may hold, taken out of
/// every run a test makes: which interpreter runs (`AYE_PYTHON`,
/// `VIRTUAL_ENV`), GitHub Actions' annotations, pytest's arguments and
/// plugins, Python's import path and optimisation, pytest's import
/// mismatch, and forced colour.
const UNSET: [&str; 10] = [
    EnvVars::AYE_PYTHON,
    EnvVars::VIRTUAL_ENV,
    EnvVars::GITHUB_ACTIONS,
    EnvVars::GITHUB_WORKSPACE,
    EnvVars::PYTEST_ADDOPTS,
    EnvVars::PYTEST_PLUGINS,
    EnvVars::PYTHONPATH,
    EnvVars::PYTHONOPTIMIZE,
    EnvVars::PY_IGNORE_IMPORTMISMATCH,
    EnvVars::CLICOLOR_FORCE,
];

/// aye as its tests run it: the binary a test target runs, which only the
/// binary's own test targets can name (`env!("CARGO_BIN_EXE_aye")`),
/// started from a directory in the hermetic environment, given an
/// interpreter or not.
///
/// The hermetic environment is the caller's without the variables aye reads
/// that a shell may hold (see the crate's docs), and with
/// `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1`, so that the plugins the interpreter
/// happens to install don't add warnings to what a test checks. A test
/// sets what it means to test on the command it gets.
#[derive(Debug, Clone)]
pub struct TestContext {
    bin: PathBuf,
}

impl TestContext {
    /// aye, as the binary `bin`.
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self { bin: bin.into() }
    }

    /// aye in `cwd`, in the hermetic environment, given no interpreter:
    /// `aye show`, or a run that finds its own.
    pub fn command(&self, cwd: &Path) -> Command {
        let mut command = Command::new(&self.bin);
        command.current_dir(cwd);
        for name in UNSET {
            command.env_remove(name);
        }
        command.env(EnvVars::PYTEST_DISABLE_PLUGIN_AUTOLOAD, "1");
        command
    }

    /// A run with `python`: `aye --python <python> <args>`, in `cwd`, in
    /// the hermetic environment.
    pub fn run(&self, python: &Path, cwd: &Path, args: &[&str]) -> Command {
        let mut command = self.command(cwd);
        command.arg("--python").arg(python).args(args);
        command
    }

    /// `aye config --python <python> <args>`, in `cwd`, in the hermetic
    /// environment.
    pub fn config(&self, python: &Path, cwd: &Path, args: &[&str]) -> Command {
        let mut command = self.command(cwd);
        command.arg("config").arg("--python").arg(python).args(args);
        command
    }

    /// aye in `cwd` with nothing of the caller's environment: for what the
    /// environment itself decides (where the interpreter comes from, the
    /// home directory), each variable given by the test.
    pub fn bare(&self, cwd: &Path) -> Command {
        let mut command = Command::new(&self.bin);
        command.current_dir(cwd).env_clear();
        command
    }
}
