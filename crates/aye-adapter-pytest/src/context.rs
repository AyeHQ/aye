//! The pytest adapter's context ([`PytestContext`]): what the front-end
//! needs to start as pytest would start itself, and what the runner takes
//! from pytest's options, built once by the recognizer and sent with every
//! batch and refinement. The binary reads the latter as the adapter's
//! [`FrameworkDefaults`] ([`PytestContext::defaults`]).

use std::path::PathBuf;

use aye_adapter::{FrameworkDefaults, JunitDefaults};
use aye_config::WorkerCount;
use aye_events::IdPrefix;
use aye_protocol::MaxFail;
use serde::Serialize;

use crate::PytestRecognizer;
use crate::native::NativeContext;

/// The pytest adapter's context. "The options" are addopts,
/// `PYTEST_ADDOPTS` and the arguments after `--`, in pytest's order (the
/// last one wins). Its keys are written in order, whatever the order of
/// the fields ([`aye_protocol::AdapterContext`]).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PytestContext {
    /// pytest's rootdir, absolute.
    pub(crate) rootdir: String,
    /// pytest's inifile, absolute; `null` without one.
    pub(crate) inifile: Option<String>,
    /// The absolute paths pytest would start collecting from in the user's
    /// run ([`aye_adapter::Recognizer::roots`]): the front-end starts from
    /// them (their initial conftests, and their initial paths, which the
    /// ignore rules spare and whose files pytest collects whatever their
    /// names). When the command line also selects by dotted name or test
    /// key, the binary adds `selected_paths` to a batch's context (the path
    /// arguments and the files those selectors select), which the
    /// front-end starts from instead.
    pub(crate) roots: Vec<String>,
    /// The `-n` value of the options, which the binary takes for the worker
    /// count when aye's own settings give none.
    pub(crate) xdist_workers: Option<String>,
    /// The effective `-x` / `--maxfail N` of the options (`null` when none
    /// sets a limit), which the runner counts across workers when aye's own
    /// command line gives none.
    pub(crate) maxfail: Option<u32>,
    /// The absolute path of the report the options ask for (`null` when
    /// none does, or the last one is empty). aye writes the report from the
    /// event stream.
    pub(crate) junitxml: Option<String>,
    /// The options' class name prefix for that report.
    pub(crate) junit_prefix: Option<String>,
    /// pytest's JUnit settings as `getini` gives them, with pytest's
    /// defaults, which the binary writes that report by.
    pub(crate) junit_suite_name: String,
    pub(crate) junit_logging: String,
    pub(crate) junit_log_passing_tests: bool,
    pub(crate) junit_duration_report: String,
    pub(crate) junit_family: String,
    /// The last `-k` of the options (`null` when absent), which the binary
    /// applies statically when its own command line gives none, so the
    /// tests it drops are counted as deselected.
    pub(crate) keyword: Option<String>,
    /// The last `-m` of the options, likewise.
    pub(crate) tags: Option<String>,
    /// The `--deselect` prefixes of the options, as prefixes of aye's ids
    /// (relative to the project root rather than the rootdir). The
    /// recognizer drops no case: the binary applies them in selection,
    /// which counts what they drop as deselected.
    pub(crate) deselect: Vec<String>,
    /// The plugins `PYTEST_PLUGINS` names, as `-p NAME` arguments, so that
    /// the refinement cache's rule for plugins loaded by name covers them
    /// (the workers get the variable itself from the environment).
    pub(crate) env_plugins: Vec<String>,
    /// Module names more than one test directory's `sys.path` entry holds,
    /// under the `prepend` and `append` import modes. A test module that
    /// imports one of them from its own directory gets `import:NAME` as its
    /// cases' first affinity key, so that those modules run in one batch,
    /// where Python keeps the first module of the name.
    pub(crate) colliding_modules: Vec<String>,
    /// What the front-end starts from, from the native recognizer only
    /// ([`crate::native::NativeRecognizer`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) native: Option<NativeContext>,
}

impl PytestContext {
    /// `recognizer`'s context, without the native section.
    pub(crate) fn new(recognizer: &PytestRecognizer) -> Self {
        let config = &recognizer.config;
        let path = |path: &std::path::Path| path.to_string_lossy().into_owned();
        Self {
            rootdir: path(&config.rootdir),
            inifile: config.inifile.as_deref().map(path),
            roots: recognizer
                .collection_args
                .iter()
                .map(|root| path(root))
                .collect(),
            xdist_workers: config.xdist_workers.clone(),
            maxfail: config.maxfail,
            junitxml: config.junitxml.as_deref().map(path),
            junit_prefix: config.junit_prefix.clone(),
            junit_suite_name: config.junit.suite_name.clone(),
            junit_logging: config.junit.logging.clone(),
            junit_log_passing_tests: config.junit.log_passing_tests,
            junit_duration_report: config.junit.duration_report.clone(),
            junit_family: config.junit.family.clone(),
            keyword: config.keyword.clone(),
            tags: config.tags.clone(),
            deselect: recognizer.deselect_ids(),
            env_plugins: config
                .env_plugins
                .iter()
                .flat_map(|name| ["-p".to_string(), name.clone()])
                .collect(),
            colliding_modules: recognizer.colliding().iter().cloned().collect(),
            native: None,
        }
    }

    /// What the binary takes from pytest's options, as the context gives
    /// it: a `-n` aye can't read runs one worker.
    pub(crate) fn defaults(&self) -> FrameworkDefaults {
        FrameworkDefaults {
            workers: self
                .xdist_workers
                .as_deref()
                .map(|n| n.parse().unwrap_or(WorkerCount::Count(1))),
            maxfail: self.maxfail.and_then(MaxFail::new),
            keyword: self.keyword.clone(),
            tags: self.tags.clone(),
            deselect: self.deselect.iter().cloned().map(IdPrefix::from).collect(),
            junit: JunitDefaults {
                path: self.junitxml.as_deref().map(PathBuf::from),
                prefix: self.junit_prefix.clone(),
                suite_name: Some(self.junit_suite_name.clone()),
                logging: Some(self.junit_logging.clone()),
                log_passing_tests: Some(self.junit_log_passing_tests),
                duration_report: Some(self.junit_duration_report.clone()),
                family: Some(self.junit_family.clone()),
            },
        }
    }
}
