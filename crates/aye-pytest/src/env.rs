//! What pytest reads from the environment the workers inherit.

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
