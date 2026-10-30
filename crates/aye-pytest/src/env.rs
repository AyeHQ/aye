//! What pytest reads from the environment the workers inherit, and Python
//! under it, as each adapter reads them: [`PytestEnv`], the pytest adapter's,
//! and [`DoctestEnv`], the doctest adapter's.

use aye_static::{EnvVars, ProcessEnv};

/// What the pytest adapter reads from the environment, which the workers
/// inherit.
#[derive(Debug, Clone, Copy, Default)]
pub struct PytestEnv<'e> {
    /// `PYTEST_ADDOPTS`.
    pub pytest_addopts: Option<&'e str>,
    /// `PYTEST_PLUGINS`.
    pub pytest_plugins: Option<&'e str>,
    /// `PYTHONPATH`.
    pub pythonpath: Option<&'e str>,
}

impl<'e> PytestEnv<'e> {
    /// The variables, as `env` holds them.
    pub fn of(env: &'e ProcessEnv) -> Self {
        Self {
            pytest_addopts: env.get(EnvVars::PYTEST_ADDOPTS),
            pytest_plugins: env.get(EnvVars::PYTEST_PLUGINS),
            pythonpath: env.get(EnvVars::PYTHONPATH),
        }
    }
}

/// What the doctest adapter's recognizer reads from the environment.
#[derive(Debug, Clone, Default)]
pub struct DoctestEnv {
    /// `PYTEST_ADDOPTS`.
    pub pytest_addopts: Option<String>,
    /// `PYTHONOPTIMIZE`.
    pub python_optimize: Option<String>,
    /// `PY_IGNORE_IMPORTMISMATCH=1`: pytest skips its "import file
    /// mismatch" check.
    pub ignore_import_mismatch: bool,
    /// `PYTEST_PLUGINS`: plugins aye can't read, which may mark items.
    pub pytest_plugins: Option<String>,
}

impl DoctestEnv {
    /// The variables, as `env` holds them.
    pub fn of(env: &ProcessEnv) -> Self {
        Self {
            pytest_addopts: env.get(EnvVars::PYTEST_ADDOPTS).map(str::to_string),
            python_optimize: env.get(EnvVars::PYTHONOPTIMIZE).map(str::to_string),
            ignore_import_mismatch: env.get(EnvVars::PY_IGNORE_IMPORTMISMATCH) == Some("1"),
            pytest_plugins: env
                .get(EnvVars::PYTEST_PLUGINS)
                .filter(|v| !v.trim().is_empty())
                .map(str::to_string),
        }
    }
}
