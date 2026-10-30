//! pytest's session for one project, as the recognizer models it: the
//! configuration, the session's plugins, the conftests above the project
//! root, the collection arguments and their initial conftests, and where
//! project modules are imported from.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **pytest's options.** addopts, `PYTEST_ADDOPTS` and the arguments
//!   after `--` ([`Project::framework_args`]) are read in pytest's order,
//!   the last one winning; the latter two first, as pytest reads them to
//!   find the config (their `-c`, `--rootdir` and `-o`, which may replace
//!   `addopts`). `-o`, `--strict`, `--ignore`, `--ignore-glob`,
//!   `--deselect`, `--confcutdir`, `--noconftest`,
//!   `--collect-in-virtualenv`, `--pyargs`, `--import-mode`, `-p`, `-k` and
//!   `-m` are read statically from all three. `--deselect` drops no case
//!   here: the context names its prefixes as prefixes of aye's ids
//!   (`deselect`), which the binary applies in selection, counting what
//!   they drop as deselected.
//!   pytest-xdist's options, `-x`, `--exitfirst`, `--maxfail`, the JUnit
//!   options (`--junitxml`/`--junit-xml`, `--junitprefix`/`--junit-prefix`)
//!   and pytest's doctest options (`--doctest-modules`, `--doctest-glob`,
//!   `--doctest-report`, `--doctest-continue-on-failure`,
//!   `--doctest-ignore-import-errors`) are taken out of all three: the
//!   runner runs the workers and counts `maxfail` across them, aye writes
//!   the JUnit report from the event stream, and the doctest adapter runs
//!   doctests; without the doctest adapter in the run, the doctest options
//!   stay (see "pytest's own doctests" below). The arguments after `--`
//!   also lose `-o addopts=…`, whose value is `addopts`. What is left, in
//!   pytest's order, is [`PytestConfig::args`], the options the front-end
//!   parses (the context's `native.args`); [`Recognizer::context`] carries
//!   what was lifted out (not the doctest options, which the doctest
//!   adapter reads itself).

pub(crate) mod import_key;
pub(crate) mod plugins;
pub(crate) mod pyargs;
pub(crate) mod scan;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::{AdapterError, Project, Recognizer};
use aye_pytest::config;
use aye_pytest::conftest::{in_confcutdir, initial_dirs};
use aye_python_stdlib::os_path::normalize_absolute;

use crate::conftest::{Summary, Supports};
use crate::marks::EmptyMark;
use crate::marks::ids::IdOptions;
use crate::predict;
use crate::{PytestConfig, PytestEnv, PytestRecognizer};
use plugins::{BUILTIN_PLUGINS, PLUGIN_ENV, PLUGIN_P};

/// Where code imports project modules from: the rootdir (on `sys.path`
/// under `python -m pytest`), the project root, their `src`, and
/// `pythonpath`.
fn plugin_bases(config: &PytestConfig, root: &Path) -> Vec<PathBuf> {
    let mut bases = vec![config.rootdir.clone(), root.to_path_buf()];
    bases.push(config.rootdir.join("src"));
    bases.push(root.join("src"));
    bases.extend(config.pythonpath.iter().cloned());
    let mut seen = Vec::new();
    bases.retain(|b| {
        let new = !seen.contains(b);
        seen.push(b.clone());
        new
    });
    bases
}

impl PytestRecognizer {
    /// The recognizer for `project`, with the variables its environment
    /// holds.
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        Self::with_environment(project, PytestEnv::of(project.env))
    }

    /// [`Self::new`] with `PYTEST_ADDOPTS` given rather than taken from the
    /// project's environment, and neither `PYTEST_PLUGINS` nor `PYTHONPATH`.
    pub fn with_env(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
    ) -> Result<Self, AdapterError> {
        Self::with_environment(
            project,
            PytestEnv {
                pytest_addopts,
                ..PytestEnv::default()
            },
        )
    }

    /// [`Self::new`] with the variables given rather than taken from the
    /// project's environment.
    pub fn with_environment(
        project: &Project<'_>,
        env: PytestEnv<'_>,
    ) -> Result<Self, AdapterError> {
        let config = PytestConfig::discover_with(project, env.pytest_addopts)?
            .with_env_plugins(env.pytest_plugins);
        let env_pythonpath: Vec<PathBuf> = env
            .pythonpath
            .map(|value| {
                std::env::split_paths(value)
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| normalize_absolute(&project.cwd.join(p)))
                    .collect()
            })
            .unwrap_or_default();
        let blocked = |name: &str| {
            config
                .plugins
                .iter()
                .any(|p| p.strip_prefix("no:").is_some_and(|n| n.trim() == name))
        };
        let settings = predict::Settings {
            python_classes: config.python_classes.value.clone(),
            python_functions: config.python_functions.value.clone(),
            ids: IdOptions {
                no_escaping: config.disable_test_id_escaping.value,
                strict: config.strict_parametrization_ids.value,
            },
            empty: EmptyMark::parse(&config.empty_parameter_set_mark.value),
            collect_imported_tests: config.collect_imported_tests.value,
            usefixtures: config.usefixtures.value.clone(),
            unittest_blocked: blocked("unittest"),
            python_blocked: blocked("python"),
        };
        let mut supports = Supports::default();
        supports.bases_first = config.import_mode != config::ImportMode::Prepend;
        let mut bases = plugin_bases(&config, project.root);
        bases.extend(env_pythonpath.iter().cloned());
        // `-p` plugins, then `PYTEST_PLUGINS`' (which no `-p no:` blocks).
        let session_plugins: Vec<&String> =
            config.plugins.iter().chain(&config.env_plugins).collect();
        let (preparse, preparse_labels): (Vec<Arc<Summary>>, Vec<&'static str>) = session_plugins
            .iter()
            .enumerate()
            .filter(|(_, name)| !name.starts_with("no:"))
            .filter_map(|(at, name)| {
                let label = if at < config.plugins.len() {
                    PLUGIN_P
                } else {
                    PLUGIN_ENV
                };
                Some((supports.module(name, &config.rootdir, &bases)?, label))
            })
            .unzip();
        let unseen_plugins = session_plugins
            .iter()
            .enumerate()
            .filter(|(at, name)| {
                // A later `-p no:NAME` blocks it again.
                !name.starts_with("no:")
                    && !BUILTIN_PLUGINS.contains(&name.as_str())
                    && !session_plugins[at + 1..]
                        .iter()
                        .any(|later| later.strip_prefix("no:") == Some(name.as_str()))
                    && supports.module(name, &config.rootdir, &bases).is_none()
            })
            .map(|(_, name)| (*name).clone())
            .collect();
        let mut outer: Vec<Arc<Summary>> = if config.noconftest {
            Vec::new()
        } else {
            project
                .root
                .ancestors()
                .skip(1)
                .filter(|dir| in_confcutdir(&config.confcutdir, dir))
                .filter_map(|dir| supports.conftest_in(dir))
                .collect()
        };
        outer.reverse();
        let mut recognizer = Self {
            root: project.root.to_path_buf(),
            config,
            settings,
            supports,
            preparse,
            preparse_labels,
            outer,
            scan: OnceLock::new(),
            unseen_plugins,
            collection_args: Vec::new(),
            initial_dirs: Vec::new(),
            reported: Mutex::new(HashSet::new()),
            env_pythonpath,
            doctests: None,
            context: OnceLock::new(),
        };
        recognizer.collection_args = recognizer.roots(project);
        recognizer.initial_dirs = initial_dirs(project.cwd, &recognizer.collection_args);
        Ok(recognizer)
    }

    /// The resolved pytest configuration.
    pub fn config(&self) -> &PytestConfig {
        &self.config
    }

    /// The conftest pytest loads from `dir`, if any (none with
    /// `--noconftest`).
    pub(crate) fn conftest_in(&self, dir: &Path) -> Option<Arc<Summary>> {
        if self.config.noconftest {
            return None;
        }
        self.supports.conftest_in(dir)
    }

    /// The directory pytest fails to collect because a conftest in it, not
    /// loaded before `pytest_configure`, defines `pytest_plugins`
    /// (`_check_non_top_pytest_plugins`), and pytest's message. Only
    /// certain definitions count: a guess would drop tests pytest runs.
    pub(crate) fn non_top_level_plugins(
        &self,
        conftests: &[(&Path, Arc<Summary>)],
    ) -> Option<(PathBuf, String)> {
        if self.config.pyargs {
            return None;
        }
        let (dir, _) = conftests.iter().find(|(dir, summary)| {
            summary.defines_plugins && !self.initial_dirs.iter().any(|d| d.starts_with(dir))
        })?;
        let message = format!(
            "Defining 'pytest_plugins' in a non-top-level conftest is no longer supported:\n\
             It affects the entire test suite instead of just below the conftest as expected.\n  \
             {}\n\
             Please move it to a top level conftest file at the rootdir:\n  \
             {}\n\
             For more information, visit:\n  \
             https://docs.pytest.org/en/stable/deprecations.html#pytest-plugins-in-non-top-level-conftest-files",
            normalize_absolute(&dir.join("conftest.py")).display(),
            normalize_absolute(&self.config.confcutdir).display(),
        );
        Some((dir.to_path_buf(), message))
    }

    /// [`plugin_bases`] and `PYTHONPATH`.
    fn basic_bases(&self) -> Vec<PathBuf> {
        let mut bases = plugin_bases(&self.config, &self.root);
        for dir in &self.env_pythonpath {
            if !bases.contains(dir) {
                bases.push(dir.clone());
            }
        }
        bases
    }

    /// Where project modules are imported from: [`plugin_bases`],
    /// `PYTHONPATH`, then what the project's conftests put on `sys.path`.
    pub(crate) fn plugin_bases(&self) -> Vec<PathBuf> {
        let mut bases = self.basic_bases();
        for dir in &self.sys_path().added {
            if !bases.contains(dir) {
                bases.push(dir.clone());
            }
        }
        bases
    }

    /// The support modules pytest has registered, and so called
    /// `pytest_configure` of, by the time it collects the files in `dir`:
    /// the session's plugins (`-p`, `PYTEST_PLUGINS`), the conftests above
    /// the project root and those of `dir` and its ancestors within the
    /// confcutdir. Not the plugins they load: claiming runs while the
    /// project is walked (see [`Self::scan`]), before modules can be looked
    /// up.
    pub(crate) fn configuring(&self, dir: &Path) -> Vec<Arc<Summary>> {
        let mut local: Vec<Arc<Summary>> = dir
            .ancestors()
            .take_while(|d| d.starts_with(&self.root))
            .filter(|d| in_confcutdir(&self.config.confcutdir, d))
            .filter_map(|d| self.conftest_in(d))
            .collect();
        local.reverse();
        self.preparse
            .iter()
            .chain(&self.outer)
            .chain(&local)
            .cloned()
            .collect()
    }
}
