//! The doctest adapter's context ([`DoctestContext`]): what the executor
//! needs, and what the runner takes from pytest's options, built once by
//! the recognizer and sent with every batch and refinement.

use serde::Serialize;

use crate::DoctestRecognizer;

/// The doctest adapter's context. Its keys are written in order, whatever
/// the order of the fields ([`aye_protocol::AdapterContext`]).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DoctestContext {
    /// pytest's rootdir, absolute, for `importlib` mode.
    rootdir: String,
    /// pytest's inifile, absolute; `null` without one.
    inifile: Option<String>,
    /// The option flag names applied to every example.
    optionflags: Vec<String>,
    /// The encoding of text files.
    encoding: String,
    /// How pytest would import modules: `prepend`, `append` or
    /// `importlib`.
    import_mode: &'static str,
    consider_namespace_packages: bool,
    /// The directories `pythonpath` puts first on `sys.path`, absolute.
    pythonpath: Vec<String>,
    /// `--doctest-report`.
    report: String,
    /// `--doctest-ignore-import-errors`.
    ignore_import_errors: bool,
    /// `--doctest-continue-on-failure`.
    continue_on_failure: bool,
    /// pytest's `filterwarnings`, and its `-W` options, which win.
    filterwarnings: Vec<String>,
    pythonwarnings: Vec<String>,
    /// The conftests pytest imports at start-up; `null` with
    /// `--noconftest`.
    conftests: Option<Conftests>,
    /// pytest's `--deselect` prefixes, as prefixes of aye's ids.
    pub(crate) deselect: Vec<String>,
    /// pytest's `-x` or `--maxfail N`; `null` for none, or 0.
    pub(crate) maxfail: Option<u32>,
    /// The JUnit report pytest's options ask for, absolute, and its prefix;
    /// `null` when they give none.
    pub(crate) junitxml: Option<String>,
    pub(crate) junit_prefix: Option<String>,
    /// pytest's JUnit settings, as the configuration writes them; left out
    /// when it doesn't set them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) junit_suite_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) junit_logging: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) junit_log_passing_tests: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) junit_duration_report: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) junit_family: Option<String>,
}

/// The conftests pytest imports at start-up, and the directory above which
/// it imports none.
#[derive(Debug, Clone, Serialize)]
struct Conftests {
    /// Absolute, in pytest's order. A batch whose context has
    /// `selected_paths` works them out from those paths instead.
    initial: Vec<String>,
    /// pytest's confcutdir, absolute.
    confcutdir: String,
}

impl DoctestContext {
    pub(crate) fn new(recognizer: &DoctestRecognizer) -> Self {
        let config = &recognizer.config;
        let path = |path: &std::path::Path| path.to_string_lossy().into_owned();
        let junit = |name: &str| config.junit.get(name).cloned();
        Self {
            rootdir: path(&config.rootdir),
            inifile: config.inifile.as_deref().map(path),
            optionflags: config.optionflags.value.clone(),
            encoding: config.encoding.value.clone(),
            import_mode: config.import_mode.as_str(),
            consider_namespace_packages: config.consider_namespace_packages,
            pythonpath: config.pythonpath.iter().map(|dir| path(dir)).collect(),
            report: config.report.clone(),
            ignore_import_errors: config.ignore_import_errors,
            continue_on_failure: config.continue_on_failure,
            filterwarnings: config.filterwarnings.clone(),
            pythonwarnings: config.pythonwarnings.clone(),
            conftests: (!recognizer.conftests.disabled).then(|| Conftests {
                initial: recognizer
                    .conftests
                    .initial(&recognizer.roots, &recognizer.cwd)
                    .iter()
                    .map(|conftest| path(conftest))
                    .collect(),
                confcutdir: path(&recognizer.conftests.confcutdir),
            }),
            deselect: recognizer.deselect_ids(),
            maxfail: config.maxfail.filter(|n| *n > 0),
            junitxml: config.junitxml.as_deref().map(path),
            junit_prefix: config.junit_prefix.clone(),
            junit_suite_name: junit("junit_suite_name"),
            junit_logging: junit("junit_logging"),
            junit_log_passing_tests: junit("junit_log_passing_tests"),
            junit_duration_report: junit("junit_duration_report"),
            junit_family: junit("junit_family"),
        }
    }
}
