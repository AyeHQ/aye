//! Where the JUnit XML report goes, and how it is written ([`JunitTarget`]).

use std::path::{Path, PathBuf};

use aye_adapter::FrameworkDefaults;
use aye_events::AdapterName;
use aye_render::{JunitDuration, JunitFamily, JunitLogging, JunitOptions};
use aye_static::ProcessEnv;

use super::first;

/// The JUnit XML report a run writes: where, what goes before its
/// classnames, and the framework's settings for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JunitTarget {
    pub(crate) path: PathBuf,
    pub(crate) prefix: String,
    pub(crate) options: JunitOptions,
}

impl JunitTarget {
    /// The report the command line (`given`) or the framework asks for, if
    /// any ([`path`]).
    pub(super) fn resolve(
        env: &ProcessEnv,
        cwd: &Path,
        given: Option<&Path>,
        defaults: &[(AdapterName, FrameworkDefaults)],
    ) -> Option<Self> {
        Some(Self {
            path: path(env, cwd, given, defaults)?,
            prefix: prefix(defaults),
            options: options(defaults),
        })
    }
}

/// Where the report goes: `--junitxml` (`given`), read as pytest reads it
/// (variables and `~` expanded, over `env`, the environment aye started
/// in), else the framework's own (the path pytest's `--junitxml` in
/// addopts, `PYTEST_ADDOPTS` or the arguments after `--` gives, from the
/// adapter's defaults). Relative to `cwd`; empty means none.
pub(super) fn path(
    env: &ProcessEnv,
    cwd: &Path,
    given: Option<&Path>,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Option<PathBuf> {
    let path = match given {
        Some(given) => aye_fs::expand_path(&given.to_string_lossy(), |name| {
            env.get_os(name).map(ToOwned::to_owned)
        }),
        None => first(defaults, |d| d.junit.path.clone())?,
    };
    (!path.as_os_str().is_empty()).then(|| cwd.join(path))
}

/// What goes before the JUnit report's classnames: the framework's own
/// (pytest's `--junitprefix`), wherever the report's path came from.
pub(super) fn prefix(defaults: &[(AdapterName, FrameworkDefaults)]) -> String {
    first(defaults, |d| d.junit.prefix.clone()).unwrap_or_default()
}

/// How the JUnit report is written: the framework's own settings, from the
/// first adapter that gives each (pytest's `junit_suite_name`,
/// `junit_logging`, `junit_log_passing_tests`, `junit_duration_report` and
/// `junit_family`, in pytest's spellings), else aye's own.
pub(super) fn options(defaults: &[(AdapterName, FrameworkDefaults)]) -> JunitOptions {
    let mut options = JunitOptions::default();
    if let Some(name) = first(defaults, |d| d.junit.suite_name.clone()) {
        options.suite_name = name;
    }
    if let Some(logging) = first(defaults, |d| d.junit.logging.clone()) {
        options.logging = JunitLogging::from(logging.as_str());
    }
    if let Some(duration) = first(defaults, |d| d.junit.duration_report.clone()) {
        options.duration = JunitDuration::from(duration.as_str());
    }
    if let Some(family) = first(defaults, |d| d.junit.family.clone()) {
        options.family = JunitFamily::from(family.as_str());
    }
    if let Some(value) = first(defaults, |d| d.junit.log_passing_tests) {
        options.log_passing_tests = value;
    }
    options
}
