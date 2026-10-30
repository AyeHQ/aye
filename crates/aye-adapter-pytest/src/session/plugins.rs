//! The plugins of pytest's session: pytest's own, those of the project it
//! loads for the whole session ([`SessionPlugin`]), and what implements a
//! hook, as the refinement reasons say it ([`HookSource`]).
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **`PYTEST_PLUGINS`** names plugins of the whole session, imported
//!   after the `-p` ones and blocked by a `-p no:NAME`: project modules
//!   there count as `-p` ones do, others make every case `Partial`, and
//!   the context names them as `-p NAME` arguments (`env_plugins`), so the
//!   refinement cache's rule for plugins loaded by name covers them.

use std::fmt;
use std::sync::Arc;

use crate::conftest::Summary;

/// pytest's `builtin_plugins`, and the plugins merged into pytest that it
/// refuses to register (`DEPRECATED_EXTERNAL_PLUGINS`): `-p NAME` for these
/// loads nothing aye doesn't model.
pub(crate) const BUILTIN_PLUGINS: [&str; 37] = [
    "mark",
    "main",
    "runner",
    "fixtures",
    "helpconfig",
    "python",
    "terminal",
    "debugging",
    "unittest",
    "capture",
    "skipping",
    "legacypath",
    "tmpdir",
    "monkeypatch",
    "recwarn",
    "pastebin",
    "assertion",
    "junitxml",
    "doctest",
    "cacheprovider",
    "setuponly",
    "setupplan",
    "stepwise",
    "unraisableexception",
    "threadexception",
    "warnings",
    "logging",
    "reports",
    "faulthandler",
    "subtests",
    "pytester",
    "pytester_assertions",
    "terminalprogress",
    "pytest_catchlog",
    "pytest_capturelog",
    "pytest_faulthandler",
    "pytest_subtests",
];

/// What implements a hook, as refinement reasons name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookSource {
    /// A project module `-p NAME` loads: `a -p plugin`.
    CommandLinePlugin,
    /// A project module `PYTEST_PLUGINS` names: `a PYTEST_PLUGINS plugin`.
    EnvironmentPlugin,
    /// A conftest of the project, or above it: `a conftest.py`.
    Conftest,
    /// A conftest in a test module's scope: `a conftest.py in scope`.
    ConftestInScope,
    /// A plugin a conftest loads: `a plugin a conftest.py loads`.
    PluginOfConftest,
    /// A plugin a conftest in scope loads: `a plugin a conftest.py in scope
    /// loads`.
    PluginOfConftestInScope,
    /// A plugin a test module loads through `pytest_plugins`: `a plugin a
    /// test module loads`.
    PluginOfTestModule,
}

impl HookSource {
    /// How refinement reasons say it.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CommandLinePlugin => "a -p plugin",
            Self::EnvironmentPlugin => "a PYTEST_PLUGINS plugin",
            Self::Conftest => "a conftest.py",
            Self::ConftestInScope => "a conftest.py in scope",
            Self::PluginOfConftest => "a plugin a conftest.py loads",
            Self::PluginOfConftestInScope => "a plugin a conftest.py in scope loads",
            Self::PluginOfTestModule => "a plugin a test module loads",
        }
    }
}

impl fmt::Display for HookSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What makes a project module a plugin of the whole session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PluginSource {
    /// `-p NAME`, from addopts, `PYTEST_ADDOPTS` or the arguments after
    /// `--`.
    CommandLine,
    /// `PYTEST_PLUGINS`.
    Environment,
}

impl PluginSource {
    /// What implements the hooks of a plugin it loads.
    pub(crate) const fn hook_source(self) -> HookSource {
        match self {
            Self::CommandLine => HookSource::CommandLinePlugin,
            Self::Environment => HookSource::EnvironmentPlugin,
        }
    }
}

/// A project module pytest loads as a plugin of the whole session.
#[derive(Debug, Clone)]
pub(crate) struct SessionPlugin {
    pub(crate) summary: Arc<Summary>,
    pub(crate) source: PluginSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each source says what its constant said before there was a type.
    #[test]
    fn hook_sources_say_what_they_always_said() {
        let said = [
            (HookSource::CommandLinePlugin, "a -p plugin"),
            (HookSource::EnvironmentPlugin, "a PYTEST_PLUGINS plugin"),
            (HookSource::Conftest, "a conftest.py"),
            (HookSource::ConftestInScope, "a conftest.py in scope"),
            (HookSource::PluginOfConftest, "a plugin a conftest.py loads"),
            (
                HookSource::PluginOfConftestInScope,
                "a plugin a conftest.py in scope loads",
            ),
            (
                HookSource::PluginOfTestModule,
                "a plugin a test module loads",
            ),
        ];
        for (source, text) in said {
            assert_eq!(source.to_string(), text);
        }
        assert_eq!(
            PluginSource::CommandLine.hook_source(),
            HookSource::CommandLinePlugin
        );
        assert_eq!(
            PluginSource::Environment.hook_source(),
            HookSource::EnvironmentPlugin
        );
    }
}
