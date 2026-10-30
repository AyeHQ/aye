//! The doctest adapter's static face for one project
//! ([`DoctestRecognizer`]): where collection starts, the directories it
//! enters and the files it claims, as pytest's doctest plugin would, and
//! the cases it makes of a file.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aye_adapter::{
    AdapterError, Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested, Setting,
    SourceFile,
};
use aye_events::{AdapterName, AffinityKey, Case, Certainty, Span};
use aye_protocol::AdapterContext;
use aye_pytest::config::ConfigRules;
use aye_pytest::session::is_virtualenv;
use aye_pytest::{conftest, doctest, names};
use aye_python_stdlib::fnmatch;
use aye_python_stdlib::os_path;

use crate::conftests::ProjectConftest;
use crate::context::DoctestContext;
use crate::project_scan::ProjectScan;
use crate::{DoctestConfig, DoctestEnv, NAME, SETS};

/// The doctest adapter's static face for one project.
#[derive(Debug)]
pub struct DoctestRecognizer {
    pub(crate) root: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) config: DoctestConfig,
    /// Where collection starts ([`Recognizer::roots`]).
    pub(crate) roots: Vec<PathBuf>,
    /// `PYTHONOPTIMIZE` strips docstrings.
    pub(crate) stripped: bool,
    /// `PY_IGNORE_IMPORTMISMATCH=1`.
    pub(crate) ignore_import_mismatch: bool,
    /// `PYTEST_PLUGINS` is set.
    pub(crate) env_plugins: bool,
    /// pytest's conftest rules for the run.
    pub(crate) conftests: conftest::Conftests,
    /// What the project's files say about `__module__` (found once, when
    /// needed).
    pub(crate) scan: OnceLock<ProjectScan>,
    /// The project's conftests, and what each may do to doctest items
    /// (found once, when needed).
    pub(crate) project_conftests: OnceLock<Vec<ProjectConftest>>,
    /// The context, built once (see [`Self::doctest_context`]).
    pub(crate) context: OnceLock<DoctestContext>,
}

impl DoctestRecognizer {
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        Self::among(project, &[])
    }

    /// [`Self::new`] for a run that uses `adapters` (by name): beside
    /// `pytest`, `test*.txt` files are doctests without a pytest
    /// configuration file too.
    pub fn among(project: &Project<'_>, adapters: &[AdapterName]) -> Result<Self, AdapterError> {
        Self::with_environment(project, adapters, &DoctestEnv::of(project.env))
    }

    /// [`Self::new`] with `PYTEST_ADDOPTS` and `PYTHONOPTIMIZE` given
    /// rather than taken from the project's environment.
    pub fn with_env(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
        python_optimize: Option<&str>,
    ) -> Result<Self, AdapterError> {
        Self::with_env_among(project, &[], pytest_addopts, python_optimize)
    }

    /// [`Self::among`] with `PYTEST_ADDOPTS` and `PYTHONOPTIMIZE` given,
    /// and no other variable set.
    pub fn with_env_among(
        project: &Project<'_>,
        adapters: &[AdapterName],
        pytest_addopts: Option<&str>,
        python_optimize: Option<&str>,
    ) -> Result<Self, AdapterError> {
        let env = DoctestEnv {
            pytest_addopts: pytest_addopts.map(str::to_string),
            python_optimize: python_optimize.map(str::to_string),
            ..DoctestEnv::default()
        };
        Self::with_environment(project, adapters, &env)
    }

    /// [`Self::among`] with the environment variables given rather than
    /// taken from the project's environment.
    pub fn with_environment(
        project: &Project<'_>,
        adapters: &[AdapterName],
        env: &DoctestEnv,
    ) -> Result<Self, AdapterError> {
        let pyproject = project.root.join("pyproject.toml");
        let table = project
            .tool_aye
            .doctest
            .as_ref()
            .map(|t| (t, pyproject.as_path()));
        let beside_pytest = adapters.contains(&names::PYTEST);
        let config =
            DoctestConfig::discover(project, env.pytest_addopts.as_deref(), table, beside_pytest)?;
        // `PYTHONOPTIMIZE=2` is `-OO`; any other non-empty value is `-O`.
        let stripped = env
            .python_optimize
            .as_deref()
            .is_some_and(|v| v.trim().parse::<u32>().is_ok_and(|n| n >= 2));
        let conftests = conftest::Conftests {
            disabled: config.noconftest,
            confcutdir: config.confcutdir.clone(),
        };
        let mut recognizer = Self {
            root: project.root.to_path_buf(),
            cwd: project.cwd.to_path_buf(),
            config,
            roots: Vec::new(),
            stripped,
            ignore_import_mismatch: env.ignore_import_mismatch,
            env_plugins: env.pytest_plugins.is_some(),
            conftests,
            scan: OnceLock::new(),
            project_conftests: OnceLock::new(),
            context: OnceLock::new(),
        };
        recognizer.roots = recognizer.find_roots(project);
        Ok(recognizer)
    }

    /// The resolved configuration.
    pub fn config(&self) -> &DoctestConfig {
        &self.config
    }

    /// The command-line paths; else `testpaths`, glob-expanded, when run
    /// from the rootdir; else the current directory.
    fn find_roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if !project.args.is_empty() {
            return project
                .args
                .iter()
                .map(|a| os_path::abspath(project.cwd, a))
                .collect();
        }
        if project.cwd == self.config.rootdir && !self.config.testpaths.is_empty() {
            let roots = ConfigRules::Approximate.testpaths(
                &self.config.rootdir,
                &self.config.testpaths,
                SETS,
            );
            if !roots.is_empty() {
                return roots;
            }
        }
        vec![project.cwd.to_path_buf()]
    }

    /// `--ignore` and `--ignore-glob`.
    fn ignored(&self, path: &Path) -> bool {
        if self.config.ignore.iter().any(|p| p == path) {
            return true;
        }
        let text = path.to_string_lossy();
        self.config
            .ignore_glob
            .iter()
            .any(|glob| fnmatch::fnmatch(&text, glob, SETS))
    }

    /// The names `-k` matches for a file: the directories between pytest's
    /// rootdir and the file, and its name.
    fn path_keywords(&self, file: &SourceFile<'_>) -> Vec<String> {
        match file.abs.strip_prefix(&self.config.rootdir) {
            Ok(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect(),
            Err(_) => file.rel.as_str().split('/').map(str::to_string).collect(),
        }
    }

    pub(crate) fn case(
        &self,
        file: &SourceFile<'_>,
        name: &str,
        span: Span,
        affinity: Vec<AffinityKey>,
    ) -> Case {
        let mut case = Case::new(format!("{}::{name}", file.rel), NAME);
        case.span = Some(span);
        let mut keywords = self.path_keywords(file);
        if !keywords.iter().any(|k| k == name) {
            keywords.push(name.to_string());
        }
        case.keywords = keywords;
        case.affinity = affinity;
        case.certainty = Certainty::Exact;
        case
    }
}

impl Recognizer for DoctestRecognizer {
    fn roots(&self, _project: &Project<'_>) -> Vec<PathBuf> {
        self.roots.clone()
    }

    /// pytest's `pytest_ignore_collect` for a directory: `__pycache__`,
    /// virtualenvs, `norecursedirs`, `--ignore` and `--ignore-glob`.
    fn descend(&self, dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if name == "__pycache__" {
            return false;
        }
        if !self.config.collect_in_virtualenv && is_virtualenv(dir) {
            return false;
        }
        if self
            .config
            .norecursedirs
            .iter()
            .any(|pattern| fnmatch::fnmatch_ex(pattern, dir, SETS))
        {
            return false;
        }
        !self.ignored(dir)
    }

    /// pytest's doctest `pytest_collect_file`: a `.py` file (not
    /// `__main__.py`, nor a `setup.py` mentioning setuptools or distutils)
    /// when modules are on; another file when it matches a glob, or is a
    /// `.txt` or `.rst` named on the command line.
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let explicit = requested.is_named();
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let abs = self.root.join(rel);
        let modules = self.config.modules.value;
        let globs = &self.config.globs.value;
        match doctest::collects(name, &abs, explicit, modules, globs, SETS, || {
            self.ignored(&abs)
        }) {
            Some(_) => Claim::Tests,
            None => Claim::Ignore,
        }
    }

    /// A file's cases; when it must be refined, they are only a guess
    /// (`Partial`), which stands when refinement is off or fails. Their
    /// tags and keywords are `Partial` too where a hook or a plugin may
    /// mark doctest items, so that `-k` and `-m` keep them (Core decision
    /// 5).
    fn recognize(
        &self,
        file: &SourceFile<'_>,
        _requested: Requested,
        _support: &[SourceFile<'_>],
    ) -> Recognition {
        let mut recognition = if file.abs.extension().is_some_and(|e| e == "py") {
            self.recognize_module(file)
        } else {
            self.recognize_text(file)
        };
        if recognition.refine.is_some()
            || (!recognition.cases.is_empty() && self.may_be_marked(file.abs).is_some())
        {
            for case in &mut recognition.cases {
                case.certainty = Certainty::Partial;
            }
        }
        recognition
    }

    fn settings(&self) -> Vec<Setting> {
        self.config.settings()
    }

    /// What the executor needs: pytest's rootdir (for `importlib` mode),
    /// the option flags by name, the text files' encoding, the import mode,
    /// `pythonpath`, the `--doctest-report` choice and
    /// `--doctest-ignore-import-errors`: [`DoctestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.doctest_context())
    }

    /// What the context's keys for the runner say
    /// ([`DoctestContext::defaults`]).
    fn defaults(&self) -> FrameworkDefaults {
        self.doctest_context().defaults()
    }
}
