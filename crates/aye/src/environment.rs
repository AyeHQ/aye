//! The environment aye started in, read once ([`EnvironmentOptions`]).

use std::ffi::OsString;
use std::path::PathBuf;

use aye_static::{EnvVars, ProcessEnv};

/// The environment aye started in, read once, at the start, and handed
/// down: the variables the binary reads by name, and the whole of it
/// (`vars`), for what reads any variable: the adapters (`Project.env`) and
/// the paths variables are expanded in (`--junitxml`).
#[derive(Debug, Clone)]
pub(crate) struct EnvironmentOptions {
    /// `$AYE_PYTHON`: the interpreter, unless `--python` names one. Empty
    /// is unset.
    pub(crate) python: Option<OsString>,
    /// `$VIRTUAL_ENV`: an active virtual environment. Empty is unset.
    pub(crate) virtual_env: Option<OsString>,
    /// `$PATH`, where interpreters are looked for. Empty is unset.
    pub(crate) path: Option<OsString>,
    /// `$GITHUB_WORKSPACE`: in a GitHub Actions step, the repository's
    /// checkout, which annotations' paths are relative to.
    pub(crate) github_workspace: Option<PathBuf>,
    /// Every variable, as read.
    pub(crate) vars: ProcessEnv,
}

impl EnvironmentOptions {
    /// This process's environment: the one read of it.
    pub(crate) fn from_process() -> Self {
        Self::from_vars(ProcessEnv::from_process())
    }

    /// The environment `vars` holds.
    pub(crate) fn from_vars(vars: ProcessEnv) -> Self {
        let set = |name| {
            vars.get_os(name)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        };
        Self {
            python: set(EnvVars::AYE_PYTHON),
            virtual_env: set(EnvVars::VIRTUAL_ENV),
            path: set(EnvVars::PATH),
            github_workspace: vars.get_os(EnvVars::GITHUB_WORKSPACE).map(PathBuf::from),
            vars,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_variable_read_by_name_is_unset_when_empty() {
        let env = EnvironmentOptions::from_vars(ProcessEnv::from_vars([
            ("AYE_PYTHON", ""),
            ("VIRTUAL_ENV", "/venv"),
            ("PATH", ""),
            ("GITHUB_WORKSPACE", "/checkout"),
            ("OTHER", "kept"),
        ]));
        assert_eq!(env.python, None);
        assert_eq!(env.virtual_env.as_deref(), Some("/venv".as_ref()));
        assert_eq!(env.path, None);
        assert_eq!(env.github_workspace, Some(PathBuf::from("/checkout")));
        assert_eq!(env.vars.get("OTHER"), Some("kept"));
        assert_eq!(env.vars.len(), 5);
    }
}
