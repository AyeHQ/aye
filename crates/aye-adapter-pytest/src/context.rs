//! The pytest adapter's context ([`PytestContext`]): what the front-end
//! needs to start as pytest would start itself, and what the runner takes
//! from pytest's options, built once by the recognizer and sent with every
//! batch and refinement. The binary reads the latter as the adapter's
//! [`FrameworkDefaults`] ([`PytestContext::defaults`]).
//! Its `native` section ([`NativeContext`]) is what the front-end starts
//! from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{FrameworkDefaults, JunitDefaults};
use aye_config::WorkerCount;
use aye_events::IdPrefix;
use aye_protocol::MaxFail;
use aye_pytest::config::ImportMode;
use aye_pytest::getini::{apply_overrides, ini_bool, ini_lines};
use aye_pytest::session::deselect_ids;
use serde::Serialize;
use serde_json::{Value, json};

use crate::embedded::modules_digest;
use crate::options::parse_options;
use crate::{PytestConfig, PytestRecognizer};

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
    /// ([`crate::recognizer::NativeRecognizer`]).
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

/// The `native` section of the pytest adapter's context: what the
/// front-end starts from.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct NativeContext {
    /// pytest's rootdir, absolute.
    rootdir: String,
    /// pytest's inifile, absolute; `null` without one.
    inifile: Option<String>,
    /// pytest's confcutdir, absolute.
    confcutdir: String,
    /// `pythonpath`, absolute, in order.
    pythonpath: Vec<String>,
    /// From `python_files` to `markers`, and `filterwarnings`: lists of
    /// strings, pytest's defaults when unset.
    python_files: Vec<String>,
    python_classes: Vec<String>,
    python_functions: Vec<String>,
    testpaths: Vec<String>,
    norecursedirs: Vec<String>,
    usefixtures: Vec<String>,
    markers: Vec<String>,
    filterwarnings: Vec<String>,
    /// `strict_xfail`, its alias `xfail_strict`, else `strict`.
    xfail_strict: bool,
    /// `strict_markers`, else `strict`; both also from `--strict-markers`
    /// and `--strict`.
    strict_markers: bool,
    /// From `strict_parametrization_ids` to `noconftest`: resolved.
    strict_parametrization_ids: bool,
    disable_test_id_escaping: bool,
    collect_imported_tests: bool,
    consider_namespace_packages: bool,
    noconftest: bool,
    /// `prepend`, `append` or `importlib`.
    import_mode: ImportMode,
    /// `""` for pytest's default.
    empty_parameter_set_mark: String,
    /// The `-p` values in order (`no:NAME` included).
    plugins: Vec<String>,
    /// The modules `PYTEST_PLUGINS` names.
    env_plugins: Vec<String>,
    /// Every raw value of the configuration, by name, after `-o` and the
    /// `--strict…` flags, for `getini` to coerce by the option's type: a
    /// string from an INI file, `[tool.pytest.ini_options]` scalars or
    /// `-o`; a list of strings for an array of `[tool.pytest.ini_options]`;
    /// the TOML value itself (string, bool, number, list) from
    /// `[tool.pytest]` or `pytest.toml`.
    ini: BTreeMap<String, Value>,
    /// pytest's arguments, as the front-end parses them
    /// ([`PytestConfig::args`]: `addopts`, `PYTEST_ADDOPTS`, then those
    /// after `--`; without pytest-xdist's, `-x`/`--maxfail`, the JUnit and,
    /// when the doctest adapter runs, the doctest options, which the runner
    /// and the doctest adapter handle), for options conftests add.
    args: Vec<String>,
    /// [`Self::args`] parsed as pytest 9.1.1 parses its own options,
    /// `{dest: value}` for every built-in option (pytest's dest names and
    /// defaults: `verbose`, `keyword`, `markexpr`, `plugins`, `runxfail`,
    /// `basetemp`, `capture`, `pythonwarnings`, `override_ini`, …),
    /// `maxfail`, `xmlpath` and `junitprefix` from the options the runner
    /// lifted, and `file_or_dir` for positional arguments.
    options: BTreeMap<String, Value>,
    /// The options [`Self::options`] doesn't know (a plugin's, a
    /// conftest's), as given.
    unknown_options: Vec<String>,
    /// A digest of the executor's sources, so that the refinement cache
    /// never answers for other sources.
    modules_digest: String,
}

/// The `native` section for `config`.
pub(crate) fn native_context(config: &PytestConfig) -> NativeContext {
    let args = config.args.clone();
    let mut ini = config.file_ini();
    apply_overrides(&mut ini, &args);
    let mut parsed = parse_options(&args);
    parsed
        .values
        .insert("maxfail".into(), json!(config.maxfail.map_or(0, u64::from)));
    parsed.values.insert(
        "xmlpath".into(),
        config
            .junitxml
            .as_ref()
            .map_or(Value::Null, |p| json!(p.to_string_lossy())),
    );
    parsed
        .values
        .insert("junitprefix".into(), json!(config.junit_prefix));
    let flag = |names: &[&str]| -> Option<bool> {
        names
            .iter()
            .find_map(|name| ini.get(*name))
            .and_then(ini_bool)
    };
    let strict = flag(&["strict"]).unwrap_or(false);
    let path = |p: &Path| p.to_string_lossy().into_owned();
    NativeContext {
        rootdir: path(&config.rootdir),
        inifile: config.inifile.as_deref().map(path),
        confcutdir: path(&config.confcutdir),
        pythonpath: config.pythonpath.iter().map(|p| path(p)).collect(),
        python_files: config.python_files.value.clone(),
        python_classes: config.python_classes.value.clone(),
        python_functions: config.python_functions.value.clone(),
        testpaths: config.testpaths.value.clone(),
        norecursedirs: config.norecursedirs.value.clone(),
        usefixtures: config.usefixtures.value.clone(),
        markers: config.markers.value.clone(),
        filterwarnings: ini.get("filterwarnings").map(ini_lines).unwrap_or_default(),
        xfail_strict: flag(&["strict_xfail", "xfail_strict"]).unwrap_or(strict),
        strict_markers: flag(&["strict_markers"]).unwrap_or(strict),
        strict_parametrization_ids: config.strict_parametrization_ids.value,
        disable_test_id_escaping: config.disable_test_id_escaping.value,
        collect_imported_tests: config.collect_imported_tests.value,
        consider_namespace_packages: config.consider_namespace_packages,
        noconftest: config.noconftest,
        import_mode: config.import_mode,
        empty_parameter_set_mark: config.empty_parameter_set_mark.value.clone(),
        plugins: config.plugins.clone(),
        env_plugins: config.env_plugins.clone(),
        ini,
        args,
        options: parsed.values,
        unknown_options: parsed.unknown,
        modules_digest: modules_digest(),
    }
}

impl PytestRecognizer {
    /// The context ([`aye_adapter::Recognizer::context`]), built the first
    /// time it is asked for, which walks the project once for
    /// [`PytestContext::colliding_modules`].
    pub(crate) fn pytest_context(&self) -> &PytestContext {
        self.context.get_or_init(|| PytestContext::new(self))
    }

    /// `--deselect`'s prefixes as prefixes of aye's ids: relative to the
    /// project root, when the rootdir is inside it.
    fn deselect_ids(&self) -> Vec<String> {
        deselect_ids(&self.config.rootdir, &self.root, &self.config.deselect)
    }
}

#[cfg(test)]
mod tests {
    use aye_adapter::{Project, Recognizer};

    use super::*;
    use crate::adapter::Listing;
    use crate::recognizer::NativeRecognizer;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn recognizer(root: &Path, framework_args: &[String]) -> NativeRecognizer {
        let project = Project {
            framework_args,
            ..Project::new(root, root)
        };
        NativeRecognizer::new(
            PytestRecognizer::with_env(&project, None).unwrap(),
            Listing::Refine,
        )
    }

    #[test]
    fn the_native_context_resolves_the_settings_and_the_raw_ini() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = -ra --strict-markers -p no:cacheprovider -n 4 -x\n\
             markers =\n    slow: slow tests\nfilterwarnings =\n    error\n    ignore::UserWarning\n\
             python_functions = test check\ntimeout = 5\n",
        );
        let cli = strings(&["-o", "xfail_strict=true", "--runslow"]);
        let context = recognizer(root, &cli).context().into_value();
        // Nothing of what only fed a pytest session per batch.
        for key in [
            "addopts",
            "pytest_addopts_env",
            "framework_args",
            "single_session",
            "doctests",
            "adds_items",
        ] {
            assert_eq!(context.get(key), None, "{key}");
        }
        let native = &context["native"];
        assert_eq!(native["rootdir"], json!(root.to_string_lossy()));
        assert_eq!(
            native["inifile"],
            json!(root.join("pytest.ini").to_string_lossy())
        );
        assert_eq!(native["python_functions"], json!(["test", "check"]));
        assert_eq!(native["python_files"], json!(["test_*.py", "*_test.py"]));
        assert_eq!(native["markers"], json!(["slow: slow tests"]));
        assert_eq!(
            native["filterwarnings"],
            json!(["error", "ignore::UserWarning"])
        );
        assert_eq!(native["xfail_strict"], json!(true));
        assert_eq!(native["strict_markers"], json!(true));
        assert_eq!(native["import_mode"], json!("prepend"));
        assert_eq!(native["ini"]["timeout"], json!("5"));
        assert_eq!(native["ini"]["xfail_strict"], json!("true"));
        assert_eq!(native["ini"]["strict_markers"], json!("true"));
        // `-n` and `-x` are the runner's: not in the arguments the
        // front-end parses, but `maxfail` says what was asked.
        assert_eq!(
            native["args"],
            json!([
                "-ra",
                "--strict-markers",
                "-p",
                "no:cacheprovider",
                "-o",
                "xfail_strict=true",
                "--runslow"
            ])
        );
        assert_eq!(native["options"]["maxfail"], json!(1));
        assert_eq!(native["options"]["plugins"], json!(["no:cacheprovider"]));
        assert_eq!(native["options"]["strict_markers"], json!(true));
        assert_eq!(native["unknown_options"], json!(["--runslow"]));
        assert_eq!(native["modules_digest"], json!(modules_digest()));
    }

    #[test]
    fn toml_values_keep_their_shape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pyproject.toml",
            "[tool.pytest.ini_options]\nxfail_strict = true\nmarkers = [\"slow\", \"fast\"]\n",
        );
        let native = &recognizer(root, &[]).context().into_value()["native"];
        assert_eq!(native["ini"]["xfail_strict"], json!("True"));
        assert_eq!(native["ini"]["markers"], json!(["slow", "fast"]));
        assert_eq!(native["xfail_strict"], json!(true));
        write(
            root,
            "pyproject.toml",
            "[tool.pytest]\nstrict = true\nminversion = \"9.0\"\n",
        );
        let native = &recognizer(root, &[]).context().into_value()["native"];
        assert_eq!(native["ini"]["strict"], json!(true));
        assert_eq!(native["xfail_strict"], json!(true));
        assert_eq!(native["strict_markers"], json!(true));
    }

    /// The context's bytes as aye 0.0.1 built them (`tests/contexts.jsonl`,
    /// one line each), for a project whose options and ini set every key
    /// the binary reads, and for one that sets nothing. The refinement
    /// cache's key hashes them, so a change here misses every cached entry.
    /// The project root is written `<root>`, and the digest of the
    /// executor's sources `<modules_digest>`, which changes with them.
    /// Pinned where paths are written with `/`.
    #[cfg(unix)]
    #[test]
    fn the_context_is_pinned_byte_for_byte() {
        let everything = tempfile::tempdir().unwrap();
        let root = everything.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = -ra -n 3 --maxfail=2 --junitxml=reports/out.xml \
             --junitprefix=ci -k \"not slow\" -m db --deselect tests/a/test_x.py::test_skip \
             -p no:cacheprovider\ntestpaths = tests\nmarkers =\n    slow: slow tests\n\
             junit_family = xunit1\njunit_log_passing_tests = false\n\
             junit_suite_name = suite\ntimeout = 5\n",
        );
        write(root, "tests/conftest.py", "");
        write(
            root,
            "tests/a/test_x.py",
            "import helpers\n\ndef test_x():\n    pass\n",
        );
        write(root, "tests/a/helpers.py", "");
        write(root, "tests/b/test_y.py", "def test_y():\n    pass\n");
        write(root, "tests/b/helpers.py", "");
        let cli = strings(&["-o", "xfail_strict=true", "--runslow"]);
        let project = Project {
            framework_args: &cli,
            ..Project::new(root, root)
        };
        let env = crate::PytestEnv {
            pytest_addopts: Some("-v"),
            pytest_plugins: Some("myplugin"),
            pythonpath: None,
        };
        let set = NativeRecognizer::new(
            PytestRecognizer::with_environment(&project, env).unwrap(),
            Listing::Refine,
        );
        let nothing = tempfile::tempdir().unwrap();
        let unset = recognizer(nothing.path(), &[]);
        let written = |recognizer: &NativeRecognizer, root: &Path| {
            let root = serde_json::to_string(&root.to_string_lossy()).unwrap();
            let digest = serde_json::to_string(&modules_digest()).unwrap();
            serde_json::to_string(&recognizer.context().into_value())
                .unwrap()
                .replace(root.trim_matches('"'), "<root>")
                .replace(&digest, "\"<modules_digest>\"")
        };
        let pinned: Vec<&str> = include_str!("tests/contexts.jsonl").lines().collect();
        assert_eq!(
            [written(&set, root), written(&unset, nothing.path())],
            pinned[..]
        );
    }

    /// A project: its files, the arguments after `--`, and `-c`.
    type IniProject = (
        &'static [(&'static str, &'static str)],
        &'static [&'static str],
        Option<&'static str>,
    );

    /// Projects the pin of the native context's `ini` reads.
    const INI_PROJECTS: &[IniProject] = &[
        (
            &[(
                "pytest.ini",
                "[pytest]\naddopts = -ra\nmarkers =\n    slow: slow\ntimeout = 5\n",
            )],
            &[],
            None,
        ),
        (&[("pytest.ini", "[other]\nx = 1\n")], &[], None),
        (
            &[(
                "tox.ini",
                "[pytest]\npython_files = check_*.py\nxfail_strict = yes\n",
            )],
            &["-o", "xfail_strict=no", "--strict-markers"],
            None,
        ),
        (
            &[(
                "setup.cfg",
                "[tool:pytest]\naddopts = --doctest-modules\nfilterwarnings =\n    error\n",
            )],
            &[],
            None,
        ),
        (
            &[(
                "pytest.toml",
                "[pytest]\naddopts = [\"-ra\"]\nstrict = true\ntimeout = 5\nratio = 1.5\nwhen = 1979-05-27\n[pytest.sub]\nkey = \"v\"\n",
            )],
            &[],
            None,
        ),
        (
            &[(
                "pyproject.toml",
                "[tool.pytest]\naddopts = [\"-ra\"]\ncollect_imported_tests = false\nminversion = \"9.0\"\n",
            )],
            &[],
            None,
        ),
        (
            &[(
                "pyproject.toml",
                "[tool.pytest.ini_options]\naddopts = \"-ra\"\nxfail_strict = true\ntimeout = 5\nratio = 1.0\nbig = 1e10\nmarkers = [\"slow\", 1, true, 2.5]\nwhen = 1979-05-27\n",
            )],
            &[],
            None,
        ),
        (
            &[("pyproject.toml", "[tool.pytest]\nini_options = 1\n")],
            &[],
            None,
        ),
        (
            &[("pyproject.toml", "[project]\nname = \"x\"\n")],
            &[],
            None,
        ),
        (
            &[("ci/custom.ini", "[pytest]\npython_classes = Check\n")],
            &[],
            Some("ci/custom.ini"),
        ),
        (
            &[("ci/custom.cfg", "[tool:pytest]\npython_classes = Check\n")],
            &[],
            Some("ci/custom.cfg"),
        ),
        (
            &[(
                "ci/other.toml",
                "[tool.pytest.ini_options]\nmarkers = [\"a\"]\n",
            )],
            &[],
            Some("ci/other.toml"),
        ),
        (
            &[("ci/notes.txt", "[pytest]\nx = 1\n")],
            &[],
            Some("ci/notes.txt"),
        ),
        (
            &[("pytest.ini", "[pytest]\nx = 1\n"), ("ci/pytest.ini", "")],
            &["-c", "ci/pytest.ini"],
            None,
        ),
        (
            &[("pytest.ini", "[pytest]\nx = 1\n"), ("elsewhere/keep", "")],
            &["--rootdir=elsewhere", "-o", "y=2"],
            None,
        ),
        (&[], &[], None),
    ];

    /// The native context's `ini` for each of [`INI_PROJECTS`], as JSON.
    fn native_inis() -> Vec<String> {
        INI_PROJECTS
            .iter()
            .map(|(files, framework_args, config_file)| {
                let dir = tempfile::tempdir().unwrap();
                let root = dir.path();
                for (rel, body) in *files {
                    write(root, rel, body);
                }
                let framework_args = strings(framework_args);
                let project = Project {
                    framework_args: &framework_args,
                    config_file: config_file.map(Path::new),
                    ..Project::new(root, root)
                };
                let recognizer = NativeRecognizer::new(
                    PytestRecognizer::with_env(&project, None).unwrap(),
                    Listing::Refine,
                );
                recognizer.context().into_value()["native"]["ini"].to_string()
            })
            .collect()
    }

    /// The native context's `ini` for each of [`INI_PROJECTS`], recorded
    /// while its reader read the configuration file a second time.
    const NATIVE_INIS: &[&str] = &[
        r#"{"addopts":"-ra","markers":"slow: slow","timeout":"5"}"#,
        "{}",
        r#"{"python_files":"check_*.py","strict_markers":"true","xfail_strict":"no"}"#,
        r#"{"addopts":"--doctest-modules","filterwarnings":"error"}"#,
        r#"{"addopts":["-ra"],"ratio":1.5,"strict":true,"sub":{"key":"v"},"timeout":5,"when":"1979-05-27"}"#,
        r#"{"addopts":["-ra"],"collect_imported_tests":false,"minversion":"9.0"}"#,
        r#"{"addopts":"-ra","big":"10000000000.0","markers":["slow","1","True","2.5"],"ratio":"1.0","timeout":"5","when":"1979-05-27","xfail_strict":"True"}"#,
        "{}",
        "{}",
        r#"{"python_classes":"Check"}"#,
        r#"{"python_classes":"Check"}"#,
        r#"{"markers":["a"]}"#,
        "{}",
        "{}",
        r#"{"x":"1","y":"2"}"#,
        "{}",
    ];

    /// The raw ini values the front-end's `getini` coerces are the
    /// configuration file's, as read, then `-o` and the `--strict…` flags
    /// of the front-end's arguments, as they always were.
    #[test]
    fn the_native_ini_holds_the_files_values_as_it_always_has() {
        assert_eq!(native_inis(), NATIVE_INIS);
    }
}
