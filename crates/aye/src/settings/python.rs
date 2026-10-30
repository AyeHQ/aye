//! The interpreter the run asks for, and the one it gets ([`find`]).

use std::ffi::OsStr;
use std::path::Path;

use aye_config::{AyeSettings, Sourced, ValueSource};
use aye_provision::{
    DiscoveryEnv, Interpreter, PythonNotFound, PythonRequest, PythonSource, RequestSource,
};
use aye_static::EnvVars;

use crate::environment::EnvironmentOptions;

/// The interpreter that runs the tests: the one `--python` (`flag`),
/// `$AYE_PYTHON` or `[tool.aye] python` asks for, else the one discovery
/// finds in the environment aye started in, with where it came from.
pub(super) fn find(
    flag: Option<&Path>,
    env: &EnvironmentOptions,
    tool_aye: &AyeSettings,
    root: &Path,
    cwd: &Path,
) -> Result<Sourced<Interpreter>, PythonNotFound> {
    let discovery = DiscoveryEnv {
        virtual_env: env.virtual_env.clone(),
        path: env.path.clone(),
        current_exe: std::env::current_exe().ok(),
    };
    let request = python_request(flag, env.python.as_deref(), tool_aye, root, cwd);
    let interpreter = aye_provision::find(request, &discovery, root, cwd)?;
    let source = interpreter_source(
        interpreter.source,
        tool_aye.python.as_ref().map(|python| &python.source),
    );
    Ok(Sourced::new(interpreter, source))
}

/// The interpreter asked for: `--python`, else `$AYE_PYTHON` (`env`), else
/// `[tool.aye] python`. The first two are relative to the current
/// directory, the configuration's to the project root.
pub(super) fn python_request(
    flag: Option<&Path>,
    env: Option<&OsStr>,
    tool_aye: &AyeSettings,
    root: &Path,
    cwd: &Path,
) -> Option<(PythonRequest, RequestSource)> {
    if let Some(flag) = flag {
        return Some((PythonRequest::parse(flag, cwd), RequestSource::Flag));
    }
    if let Some(env) = env {
        return Some((
            PythonRequest::parse(Path::new(env), cwd),
            RequestSource::Env,
        ));
    }
    let config = tool_aye.python.as_ref()?;
    Some((
        PythonRequest::parse(&config.value, root),
        RequestSource::Config,
    ))
}

/// Where the interpreter came from, as `aye config` shows it: what asked
/// for it (`--python`, `$AYE_PYTHON`, or `[tool.aye] python`, whose source
/// `config` is: a request from the configuration has one), else the rule
/// discovery followed (`$VIRTUAL_ENV`, the project's `.venv` wherever it
/// was, the interpreter beside aye, `PATH`).
pub(super) fn interpreter_source(
    source: PythonSource,
    config: Option<&ValueSource>,
) -> ValueSource {
    match source {
        PythonSource::Requested(RequestSource::Flag) => ValueSource::Cli("--python"),
        PythonSource::Requested(RequestSource::Env) => ValueSource::Env(EnvVars::AYE_PYTHON),
        PythonSource::Requested(RequestSource::Config) => {
            config.cloned().unwrap_or(ValueSource::Default)
        }
        PythonSource::VirtualEnv => ValueSource::Env(EnvVars::VIRTUAL_ENV),
        PythonSource::ProjectVenv(_) => ValueSource::Rule(".venv"),
        PythonSource::BesideAye => ValueSource::Rule("beside aye"),
        PythonSource::SearchPath => ValueSource::Rule("PATH"),
    }
}
