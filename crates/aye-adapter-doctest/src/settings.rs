//! The doctest adapter's configuration: pytest's doctest options, read from
//! pytest's configuration the way pytest would find it (approximately, see
//! the crate docs), and `[tool.aye.doctest]`, which wins over them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Project, Setting};
use aye_config::{Sourced, ValueSource};
use aye_pytest::config::ImportMode;
use aye_pytest::config::approximate::ApproximateConfig;
use aye_pytest::doctest;
use aye_python_stdlib::os_path;

use crate::options::aye_doctest;

/// The doctest adapter's resolved configuration.
#[derive(Debug, Clone)]
pub struct DoctestConfig {
    /// pytest's rootdir: `-k` matches the directories below it.
    pub rootdir: PathBuf,
    /// pytest's configuration file, if any.
    pub inifile: Option<PathBuf>,
    /// A pytest configuration exists (pytest would run here and collect
    /// `test*.txt`; so it would beside the pytest adapter, configured or
    /// not).
    pub pytest_configured: bool,
    /// Collect docstrings from every `.py` file.
    pub modules: Sourced<bool>,
    /// Text files whose name (or path) matches one of these are doctests.
    pub globs: Sourced<Vec<String>>,
    /// Option flag names, applied to every example.
    pub optionflags: Sourced<Vec<String>>,
    /// The encoding of text files.
    pub encoding: Sourced<String>,
    pub testpaths: Vec<String>,
    pub norecursedirs: Vec<String>,
    /// `--ignore`, absolute.
    pub ignore: Vec<PathBuf>,
    /// `--ignore-glob`, absolute.
    pub ignore_glob: Vec<String>,
    pub collect_in_virtualenv: bool,
    /// `--deselect`: node id prefixes, as given (relative to the rootdir).
    pub deselect: Vec<String>,
    pub import_mode: ImportMode,
    pub consider_namespace_packages: bool,
    /// `pythonpath`, absolute.
    pub pythonpath: Vec<PathBuf>,
    /// `--doctest-report`.
    pub report: String,
    /// `--doctest-ignore-import-errors`.
    pub ignore_import_errors: bool,
    /// The run uses the pytest adapter too.
    pub beside_pytest: bool,
    /// pytest's `python_files`: the modules its python plugin imports.
    pub python_files: Vec<String>,
    /// A `.txt` or `.rst` file is named on the command line: a doctest
    /// whatever the globs.
    pub explicit_text: bool,
    /// `--doctest-continue-on-failure`.
    pub continue_on_failure: bool,
    /// pytest's `filterwarnings`, and `-W`/`--pythonwarnings` (which win).
    pub filterwarnings: Vec<String>,
    pub pythonwarnings: Vec<String>,
    /// `--noconftest`.
    pub noconftest: bool,
    /// pytest's confcutdir: `--confcutdir`, else the configuration file's
    /// directory, else the rootdir. No conftest above it is loaded.
    pub confcutdir: PathBuf,
    /// `-x`/`--exitfirst`/`--maxfail`, from pytest's options: `None` when
    /// none says, 0 for no limit.
    pub maxfail: Option<u32>,
    /// `--junitxml`/`--junit-xml`, absolute (`None` when empty or unset),
    /// and `--junitprefix`/`--junit-prefix`.
    pub junitxml: Option<PathBuf>,
    pub junit_prefix: Option<String>,
    /// pytest's `junit_*` settings that the configuration sets, by name.
    pub junit: BTreeMap<String, String>,
    /// `-p NAME` plugins aye can't read, which may mark doctest items.
    pub plugins: Vec<String>,
}

impl DoctestConfig {
    /// Read the configuration for `project`. `pytest_addopts` is
    /// `PYTEST_ADDOPTS`; `aye` is `[tool.aye.doctest]`, with the file it
    /// came from; `beside_pytest` says the run uses the pytest adapter too,
    /// so that pytest's default text glob applies even without a pytest
    /// configuration file.
    pub fn discover(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
        aye: Option<(&toml::Table, &Path)>,
        beside_pytest: bool,
    ) -> Result<Self, AdapterError> {
        let ApproximateConfig {
            rootdir,
            inifile,
            configured,
            doctest_modules,
            doctest_globs,
            doctest_blocked,
            doctest_optionflags,
            doctest_encoding,
            doctest_report,
            ignore_import_errors,
            continue_on_failure,
            testpaths,
            norecursedirs,
            ignore,
            ignore_glob,
            collect_in_virtualenv,
            deselect,
            import_mode,
            consider_namespace_packages,
            pythonpath,
            python_files,
            filterwarnings,
            pythonwarnings,
            noconftest,
            confcutdir,
            maxfail,
            junitxml,
            junit_prefix,
            junit,
            plugins,
        } = ApproximateConfig::discover(project, pytest_addopts)?;
        let aye_path = aye.map(|(_, path)| path.to_path_buf()).unwrap_or_default();
        let aye_settings = aye_doctest(aye.map(|(table, _)| table), &aye_path)?;
        let aye_origin = ValueSource::file(&aye_path, "[tool.aye.doctest]");

        let modules = match aye_settings.modules {
            Some(on) => Sourced::new(on, aye_origin.clone()),
            None => match (&doctest_modules, doctest_blocked) {
                (Some(origin), false) => Sourced::new(true, origin.clone()),
                _ => Sourced::default_value(false),
            },
        };
        let globs = if let Some(globs) = aye_settings.glob {
            Sourced::new(globs, aye_origin.clone())
        } else if doctest_blocked {
            Sourced::default_value(Vec::new())
        } else if let Some((_, origin)) = doctest_globs.last() {
            Sourced::new(
                doctest_globs.iter().map(|(g, _)| g.clone()).collect(),
                origin.clone(),
            )
        } else if configured || beside_pytest {
            // pytest's default `--doctest-glob`: wherever pytest runs.
            Sourced::new(vec!["test*.txt".to_string()], ValueSource::Default)
        } else {
            Sourced::default_value(Vec::new())
        };
        let explicit_text = project.args.iter().any(|arg| {
            arg.extension().is_some_and(|e| e == "txt" || e == "rst")
                && os_path::abspath(project.cwd, arg).is_file()
        });
        let active = modules.value || !globs.value.is_empty() || explicit_text;

        let mut optionflags = match doctest_optionflags {
            Some(value) => value,
            None => Sourced::default_value(vec!["ELLIPSIS".to_string()]),
        };
        if active
            && let Some(bad) = optionflags
                .value
                .iter()
                .find(|f| !doctest::OPTIONFLAGS.contains(&f.as_str()))
        {
            return Err(AdapterError::Config(format!(
                "doctest_optionflags: unknown flag {bad:?} in {} (pytest accepts {})",
                optionflags.source,
                doctest::OPTIONFLAGS.join(", ")
            )));
        }
        if let Some(extra) = aye_settings.optionflags {
            for flag in extra {
                if !optionflags.value.contains(&flag) {
                    optionflags.value.push(flag);
                }
            }
            optionflags.source = aye_origin.clone();
        }
        let encoding = match (aye_settings.encoding, doctest_encoding) {
            (Some(encoding), _) => Sourced::new(encoding, aye_origin),
            (None, Some(value)) => value,
            (None, None) => Sourced::default_value("utf-8".to_string()),
        };
        let report = doctest_report.unwrap_or_else(|| "udiff".to_string());
        if active && !doctest::REPORT_CHOICES.contains(&report.as_str()) {
            return Err(AdapterError::Config(format!(
                "--doctest-report: invalid choice {report:?} (choose from {})",
                doctest::REPORT_CHOICES.join(", ")
            )));
        }
        let import_mode = match import_mode.as_deref() {
            None => ImportMode::Prepend,
            Some(choice) => match ImportMode::parse(choice) {
                Some(mode) => mode,
                None if active => {
                    return Err(AdapterError::Config(format!(
                        "--import-mode: invalid choice {choice:?} (choose from prepend, append, importlib)"
                    )));
                }
                None => ImportMode::Prepend,
            },
        };
        Ok(Self {
            explicit_text,
            continue_on_failure,
            filterwarnings,
            pythonwarnings,
            noconftest,
            confcutdir,
            maxfail,
            junitxml,
            junit_prefix,
            junit,
            plugins,
            testpaths,
            norecursedirs,
            ignore,
            ignore_glob,
            collect_in_virtualenv,
            deselect,
            import_mode,
            consider_namespace_packages,
            pythonpath,
            report,
            ignore_import_errors,
            beside_pytest,
            python_files,
            rootdir,
            inifile,
            pytest_configured: configured,
            modules,
            globs,
            optionflags,
            encoding,
        })
    }

    /// Whether anything is collected at all.
    pub fn active(&self) -> bool {
        self.modules.value || !self.globs.value.is_empty() || self.explicit_text
    }

    /// The resolved settings, with where each came from.
    pub fn settings(&self) -> Vec<Setting> {
        let of_files = match &self.inifile {
            Some(file) => ValueSource::whole_file(file),
            None => ValueSource::Default,
        };
        let mut out = vec![
            Setting {
                name: "rootdir".into(),
                value: self.rootdir.display().to_string(),
                source: of_files.clone(),
            },
            Setting {
                name: "inifile".into(),
                value: self
                    .inifile
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                source: of_files,
            },
        ];
        let mut push = |name: &'static str, value: String, source: &ValueSource| {
            out.push(Setting {
                name: name.into(),
                value,
                source: source.clone(),
            });
        };
        push(
            "modules",
            self.modules.value.to_string(),
            &self.modules.source,
        );
        push("glob", self.globs.value.join(" "), &self.globs.source);
        push(
            "optionflags",
            self.optionflags.value.join(" "),
            &self.optionflags.source,
        );
        push(
            "encoding",
            self.encoding.value.clone(),
            &self.encoding.source,
        );
        out
    }
}
