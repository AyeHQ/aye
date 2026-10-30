//! pytest's configuration: rootdir and inifile discovery
//! (`_pytest/config/findpaths.py`) and the ini options collection depends
//! on, read with the INI reader ([`crate::ini`]) and `aye-config`'s TOML
//! loader, with the options `addopts`, `PYTEST_ADDOPTS` and the arguments
//! after `--` add, in pytest's order.
//!
//! Two sets of rules find them ([`ConfigRules`]): pytest's own, which
//! [`PytestConfig`] reads by, the pytest adapter's; and the doctest
//! adapter's approximation of them ([`approximate`]). Both look for the
//! same files (`CONFIG_NAMES`) the same way, and differ where the rules
//! say.

pub mod approximate;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Project, Setting};
use aye_config::{ConfigError, Sourced, ValueSource, read_toml, split_args};
use aye_fs::common_ancestor;
use aye_python_stdlib::fnmatch::SetTranslation;
use aye_python_stdlib::glob;
use aye_python_stdlib::os_path::{self, normalize_absolute};
use aye_static::{EnvVars, ProcessEnv};

use crate::argv::{Lifted, MaxFail, option_values, without_addopts_override};
use crate::getini::toml_json;
use crate::ini::{self, ini_section};

/// Which rules find pytest's rootdir, its configuration file and what the
/// file says: pytest 9.1.1's, or the doctest adapter's approximation of
/// them, aye 0.0.1's, which it keeps until it switches to pytest's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigRules {
    /// pytest 9.1.1's (`_pytest/config/findpaths.py` and its option
    /// parsing): the pytest adapter's, [`PytestConfig`].
    Exact,
    /// The doctest adapter's, [`approximate::ApproximateConfig`]. They
    /// differ from pytest's in which `-c` wins (aye's own, then the last of
    /// `PYTEST_ADDOPTS` and the arguments after `--`), where the rootdir
    /// falls back to (the project root: no `setup.py` rule, no search from
    /// each path given), `--rootdir` (made absolute, its variables not
    /// expanded, not checked; an empty one counts), how the options are
    /// read (a scan of its own), how a file's values read (`load`: no error
    /// for a `setup.cfg` with `[pytest]`, nor for `[tool.pytest]` beside
    /// `[tool.pytest.ini_options]`, whose values are kept as TOML gives
    /// them) and the glob `testpaths` is expanded with ([`Self::testpaths`]).
    Approximate,
}

/// What a configuration file holds for pytest, as written, before either
/// rules reads its values.
enum FileValues {
    /// An INI file's section (`[pytest]`, `[tool:pytest]`).
    Ini(ini::Section),
    /// A table of native TOML (`[tool.pytest]`, `pytest.toml`'s
    /// `[pytest]`).
    Toml(toml::Table),
    /// `[tool.pytest.ini_options]`.
    IniOptions(toml::Table),
}

/// A configuration file [`ConfigRules::locate`] found.
struct Found {
    /// Its directory: pytest's rootdir.
    rootdir: PathBuf,
    file: PathBuf,
    /// Its pytest section and what it holds; `None` for a `pyproject.toml`
    /// that holds none, found when nothing else is.
    contents: Option<(&'static str, FileValues)>,
}

impl ConfigRules {
    /// `load_config_dict_from_file`: the section of `path` that holds
    /// pytest's configuration, and what it holds; `None` when it holds
    /// none. The exact rules refuse a `setup.cfg`'s `[pytest]`, and
    /// `[tool.pytest]` beside `[tool.pytest.ini_options]`; an `ini_options`
    /// that isn't a table is an empty one for them, and no configuration
    /// for the approximate rules.
    fn load(self, path: &Path) -> Result<Option<(&'static str, FileValues)>, AdapterError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        let config_error = |error: ConfigError| AdapterError::Config(error.to_string());
        match extension {
            "ini" => match ini_section(path, "pytest").map_err(config_error)? {
                Some(values) => Ok(Some(("[pytest]", FileValues::Ini(values)))),
                None if name == "pytest.ini" || name == ".pytest.ini" => {
                    Ok(Some(("[pytest]", FileValues::Ini(ini::Section::new()))))
                }
                None => Ok(None),
            },
            "cfg" => {
                let mut sections = ini::read_ini(path).map_err(config_error)?;
                if let Some(values) = sections.remove("tool:pytest") {
                    Ok(Some(("[tool:pytest]", FileValues::Ini(values))))
                } else if self == Self::Exact && sections.contains_key("pytest") {
                    Err(AdapterError::Config(format!(
                        "{}: [pytest] section in setup.cfg files is no longer supported, change to [tool:pytest] instead.",
                        path.display()
                    )))
                } else {
                    Ok(None)
                }
            }
            "toml" => {
                let doc = read_toml(path).map_err(config_error)?;
                if name == "pytest.toml" || name == ".pytest.toml" {
                    let table = doc
                        .get("pytest")
                        .and_then(toml::Value::as_table)
                        .cloned()
                        .unwrap_or_default();
                    return Ok(Some(("[pytest]", FileValues::Toml(table))));
                }
                let Some(tool) = doc
                    .get("tool")
                    .and_then(|t| t.get("pytest"))
                    .and_then(toml::Value::as_table)
                else {
                    return Ok(None);
                };
                let mut native = tool.clone();
                let ini_options = native.remove("ini_options");
                match (self, native.is_empty(), ini_options) {
                    (Self::Exact, false, Some(_)) => Err(AdapterError::Config(format!(
                        "{}: Cannot use both [tool.pytest] (native TOML types) and [tool.pytest.ini_options] (string-based INI format) simultaneously.",
                        path.display()
                    ))),
                    (_, false, _) => Ok(Some(("[tool.pytest]", FileValues::Toml(native)))),
                    (Self::Exact, true, Some(ini)) => Ok(Some((
                        "[tool.pytest.ini_options]",
                        FileValues::IniOptions(ini.as_table().cloned().unwrap_or_default()),
                    ))),
                    (Self::Approximate, true, Some(toml::Value::Table(ini))) => Ok(Some((
                        "[tool.pytest.ini_options]",
                        FileValues::IniOptions(ini),
                    ))),
                    (_, true, _) => Ok(None),
                }
            }
            _ => Ok(None),
        }
    }

    /// `locate_config`: the first file holding pytest's configuration,
    /// from each of `starts` upward, in each directory in [`CONFIG_NAMES`]'
    /// order; failing that, the first `pyproject.toml` met.
    fn locate(self, starts: &[PathBuf]) -> Result<Option<Found>, AdapterError> {
        let mut first_pyproject: Option<PathBuf> = None;
        for start in starts {
            for base in start.ancestors() {
                for name in CONFIG_NAMES {
                    let path = base.join(name);
                    if !path.is_file() {
                        continue;
                    }
                    if name == "pyproject.toml" && first_pyproject.is_none() {
                        first_pyproject = Some(path.clone());
                    }
                    if let Some(contents) = self.load(&path)? {
                        return Ok(Some(Found {
                            rootdir: base.to_path_buf(),
                            file: path,
                            contents: Some(contents),
                        }));
                    }
                }
            }
        }
        Ok(first_pyproject.map(|path| Found {
            rootdir: path.parent().expect("a file has a parent").to_path_buf(),
            file: path,
            contents: None,
        }))
    }

    /// The paths `testpaths` names, when pytest is run from the rootdir:
    /// each pattern glob-expanded from it (`[...]` sets read with `sets`),
    /// in order, each path once. The exact rules expand them as pytest's
    /// `glob`; the approximate rules with the doctest adapter's own glob,
    /// `glob_absolute`.
    pub fn testpaths(
        self,
        rootdir: &Path,
        testpaths: &[String],
        sets: SetTranslation,
    ) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        for pattern in testpaths {
            let found: Vec<PathBuf> = match self {
                Self::Exact => glob::glob(rootdir, pattern, sets)
                    .into_iter()
                    .map(|found| normalize_absolute(&rootdir.join(found)))
                    .collect(),
                Self::Approximate => glob::glob_absolute(rootdir, pattern, sets),
            };
            for path in found {
                if !roots.contains(&path) {
                    roots.push(path);
                }
            }
        }
        roots
    }
}

/// The files pytest looks for in each directory, in the order it tries
/// them (`locate_config`).
const CONFIG_NAMES: [&str; 7] = [
    "pytest.toml",
    ".pytest.toml",
    "pytest.ini",
    ".pytest.ini",
    "pyproject.toml",
    "tox.ini",
    "setup.cfg",
];

/// pytest's default `norecursedirs`.
pub(crate) const DEFAULT_NORECURSEDIRS: [&str; 9] = [
    "*.egg",
    ".*",
    "_darcs",
    "build",
    "CVS",
    "dist",
    "node_modules",
    "venv",
    "{arch}",
];

/// A raw option value, before pytest's type coercion.
#[derive(Debug, Clone, PartialEq)]
enum Raw {
    /// From an INI file, `[tool.pytest.ini_options]` scalars, or `-o`.
    Ini(String),
    /// A list in `[tool.pytest.ini_options]`.
    IniList(Vec<String>),
    /// From native TOML (`[tool.pytest]`, `pytest.toml`): no coercion.
    Toml(toml::Value),
}

impl Raw {
    /// The value as the front-end's `getini` gets it, to coerce by the
    /// option's type: a string, a list of strings, or the TOML value
    /// itself (dates as their text).
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Ini(text) => serde_json::json!(text),
            Self::IniList(items) => serde_json::json!(items),
            Self::Toml(value) => toml_json(value),
        }
    }
}

/// The options read from the config file.
#[derive(Debug, Default)]
struct RawConfig {
    values: BTreeMap<String, (Raw, ValueSource)>,
}

impl RawConfig {
    /// What the file at `path` holds in `section`, as the exact rules read
    /// it: `[tool.pytest.ini_options]`' values made Python's text.
    fn of_file(path: &Path, section: &str, values: FileValues) -> Self {
        let source = || ValueSource::file(path, section);
        let mut config = Self::default();
        match values {
            FileValues::Ini(values) => {
                for (key, value) in values {
                    config.values.insert(key, (Raw::Ini(value), source()));
                }
            }
            FileValues::Toml(table) => {
                for (key, value) in table {
                    config.values.insert(key, (Raw::Toml(value), source()));
                }
            }
            FileValues::IniOptions(table) => {
                for (key, value) in table {
                    let raw = match value {
                        toml::Value::Array(items) => {
                            Raw::IniList(items.iter().map(python_str).collect())
                        }
                        other => Raw::Ini(python_str(&other)),
                    };
                    config.values.insert(key, (raw, source()));
                }
            }
        }
        config
    }
}

/// The resolved pytest configuration for one project.
#[derive(Debug, Clone)]
pub struct PytestConfig {
    pub rootdir: PathBuf,
    pub inifile: Option<PathBuf>,
    /// Conftests in directories above this one are not loaded.
    pub confcutdir: PathBuf,
    pub testpaths: Sourced<Vec<String>>,
    pub python_files: Sourced<Vec<String>>,
    pub python_classes: Sourced<Vec<String>>,
    pub python_functions: Sourced<Vec<String>>,
    pub norecursedirs: Sourced<Vec<String>>,
    pub addopts: Sourced<Vec<String>>,
    pub markers: Sourced<Vec<String>>,
    /// Fixtures every test uses.
    pub usefixtures: Sourced<Vec<String>>,
    pub empty_parameter_set_mark: Sourced<String>,
    pub collect_imported_tests: Sourced<bool>,
    pub strict_parametrization_ids: Sourced<bool>,
    pub disable_test_id_escaping: Sourced<bool>,
    /// `consider_namespace_packages`: module names may then depend on
    /// `sys.path` at runtime.
    pub consider_namespace_packages: bool,
    /// `pythonpath`, absolute: put on `sys.path` before conftests load.
    pub pythonpath: Vec<PathBuf>,
    /// `--ignore` in addopts, `PYTEST_ADDOPTS` and the arguments after
    /// `--`, absolute.
    pub ignore: Vec<PathBuf>,
    /// `--ignore-glob` in the same, absolute patterns.
    pub ignore_glob: Vec<String>,
    /// `--deselect` in the same: node id prefixes, as given (relative to
    /// the rootdir).
    pub deselect: Vec<String>,
    /// `--collect-in-virtualenv` in the same.
    pub collect_in_virtualenv: bool,
    /// `--pyargs` in the same.
    pub pyargs: bool,
    /// `--noconftest` in the same: no `conftest.py` is loaded.
    pub noconftest: bool,
    /// `--import-mode`, the last one in the same.
    pub import_mode: ImportMode,
    /// `-p NAME` in the same, in order (`no:NAME` included), trimmed as
    /// pytest trims them.
    pub plugins: Vec<String>,
    /// The modules `PYTEST_PLUGINS` names (comma-separated), which pytest
    /// imports as plugins of the whole session after the `-p` ones, but
    /// those a `-p no:NAME` blocks (see [`Self::with_env_plugins`]).
    pub env_plugins: Vec<String>,
    /// `PYTEST_ADDOPTS` when set and not blank, as pytest reads it (after
    /// `addopts`, before the command line).
    pub env_addopts: Option<String>,
    /// pytest's options as aye's pytest front-end parses them: `addopts`,
    /// `PYTEST_ADDOPTS`, then the arguments after `--` (without a `-o
    /// addopts=…`, whose value `addopts` already is), less what aye takes
    /// over: pytest-xdist's options and `-x`, `--exitfirst` and
    /// `--maxfail` (the runner's workers and `maxfail`), the JUnit options
    /// (aye writes the report) and the doctest options (see
    /// `split_doctest`: the doctest adapter runs doctests; they stay when
    /// it doesn't run, see [`Self::keep_doctest_options`]).
    pub args: Vec<String>,
    /// The `-n` value: the last of addopts', `PYTEST_ADDOPTS`'s and the
    /// arguments after `--`.
    pub xdist_workers: Option<String>,
    /// The effective `-x` / `--maxfail N` from addopts, `PYTEST_ADDOPTS`
    /// then the arguments after `--` (the last one wins, as in pytest);
    /// `None` when none says or the limit is 0.
    pub maxfail: Option<u32>,
    /// The report `--junitxml`/`--junit-xml` asks for in the same (the last
    /// one wins), made absolute as pytest makes it; `None` when none does
    /// or the last is empty.
    pub junitxml: Option<PathBuf>,
    /// The last `--junitprefix`/`--junit-prefix` in the same.
    pub junit_prefix: Option<String>,
    /// pytest's JUnit settings, as `getini` gives them (the inifile, then
    /// `-o` from addopts, `PYTEST_ADDOPTS` and the arguments after `--`),
    /// with pytest's defaults, for the report aye writes.
    pub junit: JunitSettings,
    /// The last `-k` of addopts, `PYTEST_ADDOPTS` then the arguments after
    /// `--`. It stays in the arguments pytest gets.
    pub keyword: Option<String>,
    /// The last `-m` of the same, likewise.
    pub tags: Option<String>,
    /// pytest's doctest options in the same: what its doctest plugin would
    /// collect.
    pub doctest: DoctestOptions,
    /// [`Self::args`] with the doctest options in their places, for a run
    /// in which no doctest adapter runs (see
    /// [`Self::keep_doctest_options`]).
    args_with_doctest: Vec<String>,
    /// The configuration file's values as read, before `-o` and the
    /// `--strict` flag (see [`Self::file_ini`]).
    file_values: BTreeMap<String, Raw>,
}

/// pytest's doctest options, read from addopts, `PYTEST_ADDOPTS` and the
/// arguments after `--` (`split_doctest` finds them).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DoctestOptions {
    /// `--doctest-modules`: every `.py` file is a doctest module.
    pub modules: bool,
    /// The `--doctest-glob` patterns, in order (pytest appends them). None
    /// given means `test*.txt`.
    pub globs: Vec<String>,
}

impl DoctestOptions {
    /// The options `split_doctest` lifted out, in pytest's order.
    fn read<'a>(lifted: impl IntoIterator<Item = &'a String>) -> Self {
        let mut options = Self::default();
        let mut iter = lifted.into_iter();
        while let Some(arg) = iter.next() {
            if arg == "--doctest-modules" {
                options.modules = true;
            } else if let Some(glob) = arg.strip_prefix("--doctest-glob=") {
                options.globs.push(glob.to_string());
            } else if arg == "--doctest-glob" {
                options.globs.extend(iter.next().cloned());
            } else if arg == "--doctest-report" {
                iter.next();
            }
        }
        options
    }
}

/// pytest's `junit_*` ini options. A value pytest couldn't read is its
/// default: pytest reads them only when it writes a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JunitSettings {
    /// `junit_suite_name`, `pytest` by default.
    pub suite_name: String,
    /// `junit_logging`, `no` by default.
    pub logging: String,
    /// `junit_log_passing_tests`, true by default.
    pub log_passing_tests: bool,
    /// `junit_duration_report`, `total` by default.
    pub duration_report: String,
    /// `junit_family`, `xunit2` by default.
    pub family: String,
}

/// pytest's `--import-mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportMode {
    #[default]
    Prepend,
    Append,
    Importlib,
}

/// Python's `str()` of a TOML scalar, as `[tool.pytest.ini_options]`
/// stringifies values.
fn python_str(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        toml::Value::Float(f) => aye_python_stdlib::repr::float_repr(*f),
        other => other.to_string(),
    }
}

/// `load_config_dict_from_file` by the exact rules: the pytest options in
/// `path`, with the section they came from, or `None` when the file holds
/// no pytest configuration.
fn load_config(path: &Path) -> Result<Option<(RawConfig, String)>, AdapterError> {
    Ok(ConfigRules::Exact
        .load(path)?
        .map(|(section, values)| (RawConfig::of_file(path, section, values), section.into())))
}

struct Located {
    rootdir: PathBuf,
    inifile: PathBuf,
    config: RawConfig,
}

/// `locate_config` by the exact rules: the first directory, from each of
/// `args` upward, with a config file holding pytest configuration; failing
/// that, the first `pyproject.toml` seen.
fn locate_config(args: &[PathBuf]) -> Result<Option<Located>, AdapterError> {
    Ok(ConfigRules::Exact.locate(args)?.map(|found| {
        let config = found
            .contents
            .map_or_else(RawConfig::default, |(section, values)| {
                RawConfig::of_file(&found.file, section, values)
            });
        Located {
            rootdir: found.rootdir,
            inifile: found.file,
            config,
        }
    }))
}

fn is_fs_root(path: &Path) -> bool {
    path.parent().is_none()
}

/// `determine_setup` without `-o`: the rootdir, the inifile, and the
/// options in it. `config_file` is `-c` (the arguments after `--`, aye's,
/// then `PYTEST_ADDOPTS`'s), `rootdir_arg` the last `--rootdir`.
fn determine_setup(
    project: &Project<'_>,
    config_file: Option<&Path>,
    rootdir_arg: Option<&str>,
) -> Result<(PathBuf, Option<PathBuf>, RawConfig), AdapterError> {
    let (rootdir, inifile, config) = locate_setup(project, config_file, rootdir_arg.is_some())?;
    let Some(arg) = rootdir_arg else {
        return Ok((rootdir, inifile, config));
    };
    // `absolutepath(os.path.expandvars(rootdir_cmd_arg))`.
    let var = |name: &str| project.env.get(name).map(str::to_string);
    let rootdir = os_path::normalize_absolute(&project.cwd.join(os_path::expandvars(arg, var)));
    if !rootdir.is_dir() {
        return Err(AdapterError::Config(format!(
            "Directory '{}' not found. Check your '--rootdir' option.",
            rootdir.display()
        )));
    }
    Ok((rootdir, inifile, config))
}

/// `determine_setup` before `--rootdir` replaces the rootdir: with
/// `rootdir_given`, a directory found only by the fallbacks (`setup.py`,
/// the common ancestor) is not looked for.
fn locate_setup(
    project: &Project<'_>,
    config_file: Option<&Path>,
    rootdir_given: bool,
) -> Result<(PathBuf, Option<PathBuf>, RawConfig), AdapterError> {
    let cwd = project.cwd;
    let dirs: Vec<PathBuf> = project
        .args
        .iter()
        .map(|arg| cwd.join(arg))
        .filter(|p| p.exists())
        .map(|p| {
            if p.is_dir() {
                p
            } else {
                p.parent().map_or(p.clone(), Path::to_path_buf)
            }
        })
        .collect();

    if let Some(file) = config_file {
        let inifile = cwd.join(file);
        let config = load_config(&inifile)?.map(|(c, _)| c).unwrap_or_default();
        let rootdir = inifile.parent().unwrap_or(cwd).to_path_buf();
        return Ok((rootdir, Some(inifile), config));
    }

    let ancestor = common_ancestor(cwd, &dirs);
    if let Some(found) = locate_config(std::slice::from_ref(&ancestor))? {
        return Ok((found.rootdir, Some(found.inifile), found.config));
    }
    if rootdir_given {
        return Ok((ancestor, None, RawConfig::default()));
    }
    if let Some(dir) = ancestor.ancestors().find(|d| d.join("setup.py").is_file()) {
        return Ok((dir.to_path_buf(), None, RawConfig::default()));
    }
    if dirs != [ancestor.clone()] {
        let searched = if dirs.is_empty() {
            vec![cwd.to_path_buf()]
        } else {
            dirs.clone()
        };
        if let Some(found) = locate_config(&searched)? {
            return Ok((found.rootdir, Some(found.inifile), found.config));
        }
    }
    let rootdir = common_ancestor(cwd, &[cwd.to_path_buf(), ancestor.clone()]);
    if is_fs_root(&rootdir) {
        Ok((ancestor, None, RawConfig::default()))
    } else {
        Ok((rootdir, None, RawConfig::default()))
    }
}

/// pytest's `_strtobool`.
pub(crate) fn strtobool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "t" | "true" | "on" | "1" => Some(true),
        "n" | "no" | "f" | "false" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl RawConfig {
    fn type_error(&self, name: &str, expected: &str) -> AdapterError {
        let origin = self
            .values
            .get(name)
            .map(|(_, origin)| origin.to_string())
            .unwrap_or_default();
        AdapterError::Config(format!(
            "{origin}: config option '{name}' expects {expected}"
        ))
    }

    /// An `args` option: shell-split in INI mode, a list of strings in TOML.
    fn args(&self, name: &str, default: &[&str]) -> Result<Sourced<Vec<String>>, AdapterError> {
        let Some((raw, origin)) = self.values.get(name) else {
            return Ok(Sourced::default_value(
                default.iter().map(|s| (*s).to_string()).collect(),
            ));
        };
        let value = match raw {
            Raw::Ini(text) => split_args(text),
            Raw::IniList(items) => items.clone(),
            Raw::Toml(toml::Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| self.type_error(name, "a list of strings"))?,
            Raw::Toml(_) => return Err(self.type_error(name, "a list")),
        };
        Ok(Sourced::new(value, origin.clone()))
    }

    /// A `linelist` option: one entry per non-empty line in INI mode.
    fn linelist(&self, name: &str) -> Result<Sourced<Vec<String>>, AdapterError> {
        let Some((raw, origin)) = self.values.get(name) else {
            return Ok(Sourced::default_value(Vec::new()));
        };
        let value = match raw {
            Raw::Ini(text) => text
                .split('\n')
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            Raw::IniList(items) => items.clone(),
            Raw::Toml(toml::Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| self.type_error(name, "a list of strings"))?,
            Raw::Toml(_) => return Err(self.type_error(name, "a list")),
        };
        Ok(Sourced::new(value, origin.clone()))
    }

    fn boolean(&self, name: &str) -> Result<Option<Sourced<bool>>, AdapterError> {
        let Some((raw, origin)) = self.values.get(name) else {
            return Ok(None);
        };
        let value = match raw {
            Raw::Ini(text) => strtobool(text).ok_or_else(|| self.type_error(name, "a bool"))?,
            Raw::Toml(toml::Value::Boolean(b)) => *b,
            _ => return Err(self.type_error(name, "a bool")),
        };
        Ok(Some(Sourced::new(value, origin.clone())))
    }

    fn string(&self, name: &str) -> Result<Option<Sourced<String>>, AdapterError> {
        let Some((raw, origin)) = self.values.get(name) else {
            return Ok(None);
        };
        let value = match raw {
            Raw::Ini(text) => text.clone(),
            Raw::Toml(toml::Value::String(s)) => s.clone(),
            _ => return Err(self.type_error(name, "a string")),
        };
        Ok(Some(Sourced::new(value, origin.clone())))
    }
}

/// Where an option came from, in origins: `PYTEST_ADDOPTS`, or the
/// arguments after `--`.
const ENV: &str = EnvVars::PYTEST_ADDOPTS;
const CLI: &str = "after --";

/// The report `--junitxml VALUE` asks for, where pytest writes it:
/// `os.path.abspath(os.path.expanduser(os.path.expandvars(VALUE)))` from
/// the current directory, over the environment `env`; `None` when the
/// value is empty. A directory is pytest's usage error.
fn report_path(env: &ProcessEnv, cwd: &Path, value: &str) -> Result<Option<PathBuf>, AdapterError> {
    if value.is_empty() {
        return Ok(None);
    }
    if cwd.join(value).is_dir() {
        return Err(AdapterError::Config(format!(
            "--junitxml must be a filename, given: {value}"
        )));
    }
    let var = |name: &str| env.get(name).map(str::to_string);
    let expanded = os_path::expanduser(&os_path::expandvars(value, var), var);
    Ok(Some(os_path::normalize_absolute(&cwd.join(expanded))))
}

impl PytestConfig {
    /// Discover the configuration the way pytest would for `project`, with
    /// `PYTEST_ADDOPTS` from its environment.
    pub fn discover(project: &Project<'_>) -> Result<Self, AdapterError> {
        Self::discover_with(project, project.env.get(EnvVars::PYTEST_ADDOPTS))
    }

    /// [`Self::discover`] with `PYTEST_ADDOPTS` given.
    pub fn discover_with(
        project: &Project<'_>,
        env_addopts: Option<&str>,
    ) -> Result<Self, AdapterError> {
        // pytest reads `PYTEST_ADDOPTS` and the command line first (their
        // `-c`, `--rootdir` and `-o` take part in finding the config), then
        // puts `addopts` in front of them. The arguments after `--` come
        // after aye's own options on pytest's command line.
        let env_addopts = env_addopts
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string);
        let env_args: Vec<String> = env_addopts.as_deref().map(split_args).unwrap_or_default();
        let cli_args: &[String] = project.framework_args;
        let early: Vec<String> = env_args.iter().chain(cli_args).cloned().collect();
        let env_config = option_values(&env_args, "--config-file", Some("-c")).pop();
        let config_file = option_values(cli_args, "--config-file", Some("-c"))
            .pop()
            .map(PathBuf::from)
            .or_else(|| project.config_file.map(Path::to_path_buf))
            .or_else(|| env_config.map(PathBuf::from));
        let rootdir_arg = option_values(&early, "--rootdir", None)
            .pop()
            .filter(|dir| !dir.is_empty());
        let (rootdir, inifile, mut raw) =
            determine_setup(project, config_file.as_deref(), rootdir_arg.as_deref())?;
        let file_values = raw
            .values
            .iter()
            .map(|(name, (value, _))| (name.clone(), value.clone()))
            .collect();

        let overrides = |raw: &mut RawConfig, args: &[String], via: &'static str| {
            for item in option_values(args, "--override-ini", Some("-o")) {
                if let Some((name, value)) = item.split_once('=') {
                    raw.values.insert(
                        name.to_string(),
                        (
                            Raw::Ini(value.to_string()),
                            ValueSource::FrameworkOption {
                                option: format!("-o {name}").into(),
                                via,
                            },
                        ),
                    );
                }
            }
        };
        overrides(&mut raw, &env_args, ENV);
        overrides(&mut raw, cli_args, CLI);

        // addopts may override ini values (`-o name=value`, `--strict`);
        // `PYTEST_ADDOPTS` and the command line, which come after it, win.
        let addopts = raw.args("addopts", &[])?;
        overrides(&mut raw, &addopts.value, "addopts");
        overrides(&mut raw, &env_args, ENV);
        overrides(&mut raw, cli_args, CLI);
        let strict_from = [
            (cli_args, CLI),
            (&env_args, ENV),
            (&addopts.value, "addopts"),
        ]
        .into_iter()
        .find(|(args, _)| args.iter().any(|a| a == "--strict"))
        .map(|(_, via)| ValueSource::FrameworkOption {
            option: "--strict".into(),
            via,
        });
        if let Some(source) = strict_from {
            raw.values
                .insert("strict".into(), (Raw::Ini("true".into()), source));
        }
        // Everything pytest parses, in its order.
        let args: Vec<String> = addopts
            .value
            .iter()
            .chain(&env_args)
            .chain(cli_args)
            .cloned()
            .collect();

        let python_files = raw.args("python_files", &["test_*.py", "*_test.py"])?;
        let python_classes = raw.args("python_classes", &["Test"])?;
        let python_functions = raw.args("python_functions", &["test"])?;
        let norecursedirs = raw.args("norecursedirs", &DEFAULT_NORECURSEDIRS)?;
        let testpaths = raw.args("testpaths", &[])?;
        let markers = raw.linelist("markers")?;
        let usefixtures = raw.args("usefixtures", &[])?;
        let empty_parameter_set_mark = raw
            .string("empty_parameter_set_mark")?
            .unwrap_or_else(|| Sourced::default_value(String::new()));
        let collect_imported_tests = raw
            .boolean("collect_imported_tests")?
            .unwrap_or_else(|| Sourced::default_value(true));
        let strict = raw.boolean("strict")?;
        let strict_parametrization_ids = match raw.boolean("strict_parametrization_ids")? {
            Some(value) => value,
            None => strict.unwrap_or_else(|| Sourced::default_value(false)),
        };
        let disable_test_id_escaping = raw
            .boolean("disable_test_id_escaping_and_forfeit_all_rights_to_community_support")?
            .unwrap_or_else(|| Sourced::default_value(false));
        let consider_namespace_packages = raw
            .boolean("consider_namespace_packages")?
            .is_some_and(|value| value.value);
        let ini_dir = inifile
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| rootdir.clone(), Path::to_path_buf);
        let pythonpath = raw
            .args("pythonpath", &[])?
            .value
            .iter()
            .map(|p| ini_dir.join(p))
            .collect();

        let cwd = project.cwd;
        let ignore = option_values(&args, "--ignore", None)
            .into_iter()
            .map(|p| cwd.join(p))
            .collect();
        let ignore_glob = option_values(&args, "--ignore-glob", None)
            .into_iter()
            .map(|p| cwd.join(p).to_string_lossy().into_owned())
            .collect();
        let deselect = option_values(&args, "--deselect", None);
        let collect_in_virtualenv = args.iter().any(|a| a == "--collect-in-virtualenv");
        let pyargs = args.iter().any(|a| a == "--pyargs");
        let noconftest = args.iter().any(|a| a == "--noconftest");
        let import_mode = match option_values(&args, "--import-mode", None).pop().as_deref() {
            Some("append") => ImportMode::Append,
            Some("importlib") => ImportMode::Importlib,
            _ => ImportMode::Prepend,
        };
        let plugins: Vec<String> = option_values(&args, "-p", None)
            .into_iter()
            .map(|name| name.trim().to_string())
            .collect();
        let keyword = option_values(&args, "-k", None).pop();
        let tags = option_values(&args, "-m", None).pop();
        let confcutdir = option_values(&args, "--confcutdir", None)
            .pop()
            .map_or_else(
                || {
                    inifile
                        .as_deref()
                        .and_then(Path::parent)
                        .map_or_else(|| rootdir.clone(), Path::to_path_buf)
                },
                |dir| cwd.join(dir),
            );

        let from_addopts = Lifted::from(&addopts.value);
        let from_env = Lifted::from(&env_args);
        let from_cli = Lifted::from(cli_args);
        let doctest = DoctestOptions::read(
            from_addopts
                .doctest
                .iter()
                .chain(&from_env.doctest)
                .chain(&from_cli.doctest),
        );
        // What the front-end parses, in pytest's order.
        let joined = |part: fn(&Lifted) -> &Vec<String>| -> Vec<String> {
            part(&from_addopts)
                .iter()
                .chain(part(&from_env))
                .cloned()
                .chain(without_addopts_override(part(&from_cli)))
                .collect()
        };
        let kept = joined(|lifted| &lifted.kept);
        let kept_with_doctest = joined(|lifted| &lifted.with_doctest);
        let xdist_workers = from_cli
            .workers
            .or(from_env.workers)
            .or(from_addopts.workers);
        let maxfail = from_cli
            .maxfail
            .or(from_env.maxfail)
            .or(from_addopts.maxfail)
            .and_then(MaxFail::limit);
        let junitxml = match from_cli
            .junit
            .xml
            .or(from_env.junit.xml)
            .or(from_addopts.junit.xml)
        {
            Some(value) => report_path(project.env, cwd, &value)?,
            None => None,
        };
        let junit_prefix = from_cli
            .junit
            .prefix
            .or(from_env.junit.prefix)
            .or(from_addopts.junit.prefix);
        let text = |name: &str, default: &str| {
            raw.string(name)
                .ok()
                .flatten()
                .map_or_else(|| default.to_string(), |value| value.value)
        };
        let junit = JunitSettings {
            suite_name: text("junit_suite_name", "pytest"),
            logging: text("junit_logging", "no"),
            log_passing_tests: raw
                .boolean("junit_log_passing_tests")
                .ok()
                .flatten()
                .is_none_or(|value| value.value),
            duration_report: text("junit_duration_report", "total"),
            family: text("junit_family", "xunit2"),
        };

        Ok(Self {
            rootdir,
            inifile,
            confcutdir,
            testpaths,
            python_files,
            python_classes,
            python_functions,
            norecursedirs,
            addopts,
            markers,
            usefixtures,
            empty_parameter_set_mark,
            collect_imported_tests,
            strict_parametrization_ids,
            disable_test_id_escaping,
            consider_namespace_packages,
            pythonpath,
            ignore,
            ignore_glob,
            deselect,
            collect_in_virtualenv,
            pyargs,
            noconftest,
            import_mode,
            plugins,
            env_plugins: Vec::new(),
            env_addopts,
            args: kept,
            xdist_workers,
            maxfail,
            junitxml,
            junit_prefix,
            junit,
            keyword,
            tags,
            doctest,
            args_with_doctest: kept_with_doctest,
            file_values,
        })
    }

    /// The configuration file's values, by name, as read, before `-o` and
    /// the `--strict…` flags: what the native context's `ini` starts from
    /// (see [`crate::getini`]). The file is read once, by discovery.
    pub fn file_ini(&self) -> BTreeMap<String, serde_json::Value> {
        self.file_values
            .iter()
            .map(|(name, value)| (name.clone(), value.to_json()))
            .collect()
    }

    /// The configuration for a run in which no doctest adapter runs beside
    /// the pytest adapter: the front-end gets pytest's doctest options as
    /// given, as pytest's own plugins would see them.
    #[must_use]
    pub fn keep_doctest_options(mut self) -> Self {
        self.args = std::mem::take(&mut self.args_with_doctest);
        self
    }

    /// The configuration with `PYTEST_PLUGINS` read: pytest imports each
    /// module it names (comma-separated) as a plugin of the session after
    /// the `-p` ones (`consider_env`), unless a `-p no:NAME` blocks it.
    #[must_use]
    pub fn with_env_plugins(mut self, pytest_plugins: Option<&str>) -> Self {
        let blocked = |name: &str| {
            self.plugins
                .iter()
                .any(|p| p.strip_prefix("no:").is_some_and(|n| n.trim() == name))
        };
        let names: Vec<String> = pytest_plugins
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty() && !blocked(name))
            .map(str::to_string)
            .collect();
        self.env_plugins = names;
        self
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
        let join = |items: &[String]| {
            items
                .iter()
                .map(|item| shlex::try_quote(item).map_or_else(|_| item.clone(), Into::into))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut push = |name: &'static str, value: String, source: &ValueSource| {
            out.push(Setting {
                name: name.into(),
                value,
                source: source.clone(),
            });
        };
        for (name, sourced) in [
            ("testpaths", &self.testpaths),
            ("python_files", &self.python_files),
            ("python_classes", &self.python_classes),
            ("python_functions", &self.python_functions),
            ("norecursedirs", &self.norecursedirs),
            ("addopts", &self.addopts),
            ("usefixtures", &self.usefixtures),
        ] {
            push(name, join(&sourced.value), &sourced.source);
        }
        if let Some(env) = &self.env_addopts {
            push(
                EnvVars::PYTEST_ADDOPTS,
                env.clone(),
                &ValueSource::Env(EnvVars::PYTEST_ADDOPTS),
            );
        }
        push(
            "markers",
            self.markers.value.join("\n"),
            &self.markers.source,
        );
        push(
            "empty_parameter_set_mark",
            self.empty_parameter_set_mark.value.clone(),
            &self.empty_parameter_set_mark.source,
        );
        for (name, sourced) in [
            ("collect_imported_tests", &self.collect_imported_tests),
            (
                "strict_parametrization_ids",
                &self.strict_parametrization_ids,
            ),
            (
                "disable_test_id_escaping_and_forfeit_all_rights_to_community_support",
                &self.disable_test_id_escaping,
            ),
        ] {
            push(
                name,
                if sourced.value { "true" } else { "false" }.to_string(),
                &sourced.source,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::argv::{Junit, split_junit};

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn discover(cwd: &Path, args: &[&str]) -> PytestConfig {
        let args: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        PytestConfig::discover_with(
            &Project {
                args: &args,
                ..Project::new(cwd, cwd)
            },
            None,
        )
        .unwrap()
    }

    #[test]
    fn pyproject_ini_options() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\naddopts = \"-ra --strict-markers\"\npython_functions = [\"test_\", \"check_\"]\n",
        );
        let config = discover(dir.path(), &[]);
        assert_eq!(config.rootdir, dir.path());
        assert_eq!(config.inifile, Some(dir.path().join("pyproject.toml")));
        assert_eq!(config.testpaths.value, ["tests"]);
        assert_eq!(config.addopts.value, ["-ra", "--strict-markers"]);
        assert_eq!(config.python_functions.value, ["test_", "check_"]);
        assert_eq!(config.python_classes.value, ["Test"]);
        assert_eq!(config.python_classes.source, ValueSource::Default);
        assert_eq!(
            config.testpaths.source.display_row(dir.path()).to_string(),
            "pyproject.toml [tool.pytest.ini_options]"
        );
    }

    #[test]
    fn native_toml_is_typed() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest]\naddopts = [\"-ra\"]\ncollect_imported_tests = false\n",
        );
        let config = discover(dir.path(), &[]);
        assert_eq!(config.addopts.value, ["-ra"]);
        assert!(!config.collect_imported_tests.value);

        // pytest 9 rejects a string where it wants a list in native TOML.
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest]\naddopts = \"-ra\"\n",
        );
        let project = Project::new(dir.path(), dir.path());
        assert!(PytestConfig::discover_with(&project, None).is_err());

        write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest]\naddopts = [\"-ra\"]\n[tool.pytest.ini_options]\naddopts = \"-q\"\n",
        );
        assert!(PytestConfig::discover_with(&project, None).is_err());
    }

    #[test]
    fn pytest_toml_comes_first() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pytest.toml",
            "[pytest]\npython_files = [\"check_*.py\"]\n",
        );
        write(
            dir.path(),
            "pytest.ini",
            "[pytest]\npython_files = t_*.py\n",
        );
        let config = discover(dir.path(), &[]);
        assert_eq!(config.inifile, Some(dir.path().join("pytest.toml")));
        assert_eq!(config.python_files.value, ["check_*.py"]);
    }

    #[test]
    fn pyproject_without_pytest_table_yields_to_tox_ini_above() {
        // Verified with pytest 9.1.1: rootdir is the tox.ini directory.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/pyproject.toml", "[project]\nname = 'x'\n");
        write(dir.path(), "tox.ini", "[pytest]\ntestpaths = a b\n");
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        let config = discover(&dir.path().join("a/b"), &[]);
        assert_eq!(config.rootdir, dir.path());
        assert_eq!(config.inifile, Some(dir.path().join("tox.ini")));
        assert_eq!(config.testpaths.value, ["a", "b"]);
    }

    #[test]
    fn a_bare_pyproject_is_the_inifile_when_nothing_else_is_found() {
        // Verified with pytest 9.1.1: `configfile: pyproject.toml`.
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a/pyproject.toml", "[project]\nname = 'x'\n");
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        let config = discover(&dir.path().join("a/b"), &[]);
        assert_eq!(config.rootdir, dir.path().join("a"));
        assert_eq!(config.inifile, Some(dir.path().join("a/pyproject.toml")));
    }

    #[test]
    fn setup_py_then_common_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "setup.cfg", "[metadata]\nname = x\n");
        write(dir.path(), "setup.py", "");
        std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
        let config = discover(&dir.path().join("pkg"), &[]);
        assert_eq!(config.inifile, None);
        assert_eq!(config.rootdir, dir.path());

        let bare = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(bare.path().join("x")).unwrap();
        let config = discover(bare.path(), &["x"]);
        assert_eq!(config.rootdir, bare.path());
        assert_eq!(config.confcutdir, bare.path());
    }

    #[test]
    fn setup_cfg_plain_pytest_section_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "setup.cfg", "[pytest]\naddopts = -q\n");
        let project = Project::new(dir.path(), dir.path());
        let err = PytestConfig::discover_with(&project, None).unwrap_err();
        assert!(err.to_string().contains("[tool:pytest]"), "{err}");
    }

    #[test]
    fn rootdir_follows_the_args() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::create_dir_all(dir.path().join("a/c")).unwrap();
        write(dir.path(), "a/pytest.ini", "");
        let config = discover(dir.path(), &["a/b", "a/c"]);
        assert_eq!(config.rootdir, dir.path().join("a"));
        assert_eq!(config.inifile, Some(dir.path().join("a/pytest.ini")));

        // No config above the common ancestor: each arg is searched.
        let other = tempfile::tempdir().unwrap();
        write(other.path(), "x/pytest.ini", "[pytest]\n");
        std::fs::create_dir_all(other.path().join("y")).unwrap();
        let config = discover(other.path(), &["x", "y"]);
        assert_eq!(config.rootdir, other.path().join("x"));
    }

    #[test]
    fn config_file_flag() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "ci/custom.ini",
            "[pytest]\npython_classes = Check\n",
        );
        let project = Project {
            config_file: Some(Path::new("ci/custom.ini")),
            ..Project::new(dir.path(), dir.path())
        };
        let config = PytestConfig::discover_with(&project, None).unwrap();
        assert_eq!(config.rootdir, dir.path().join("ci"));
        assert_eq!(config.python_classes.value, ["Check"]);
    }

    #[test]
    fn ini_values_are_coerced_like_pytest() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pytest.ini",
            "[pytest]\npython_files = \"my tests.py\" check_*.py\nmarkers =\n    slow: slow tests\n    db\nempty_parameter_set_mark = xfail\nstrict = yes\n",
        );
        let config = discover(dir.path(), &[]);
        assert_eq!(config.python_files.value, ["my tests.py", "check_*.py"]);
        assert_eq!(config.markers.value, ["slow: slow tests", "db"]);
        assert_eq!(config.empty_parameter_set_mark.value, "xfail");
        assert!(config.strict_parametrization_ids.value);
    }

    #[test]
    fn addopts_overrides_and_collection_flags() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pytest.ini",
            "[pytest]\naddopts = -o python_classes=Suite --ignore=legacy --ignore-glob=*_old.py --collect-in-virtualenv -n 4 --dist loadfile -ra\n",
        );
        let config = discover(dir.path(), &[]);
        assert_eq!(config.python_classes.value, ["Suite"]);
        assert_eq!(config.ignore, [dir.path().join("legacy")]);
        assert_eq!(
            config.ignore_glob,
            [dir.path().join("*_old.py").to_string_lossy().into_owned()]
        );
        assert!(config.collect_in_virtualenv);
        assert_eq!(config.xdist_workers.as_deref(), Some("4"));
        assert_eq!(
            config.args,
            [
                "-o",
                "python_classes=Suite",
                "--ignore=legacy",
                "--ignore-glob=*_old.py",
                "--collect-in-virtualenv",
                "-ra"
            ]
        );
    }

    fn discover_env(cwd: &Path, env: &str) -> PytestConfig {
        PytestConfig::discover_with(&Project::new(cwd, cwd), Some(env)).unwrap()
    }

    #[test]
    fn maxfail_from_addopts_and_pytest_addopts() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "pytest.ini", "[pytest]\naddopts = -x -ra\n");
        let config = discover(dir.path(), &[]);
        assert_eq!(config.maxfail, Some(1));
        assert_eq!(config.args, ["-ra"]);
        // `PYTEST_ADDOPTS` comes after addopts: its `--maxfail 0` lifts the
        // limit, as pytest 9.1.1 does with addopts `-x`.
        let config = discover_env(dir.path(), "--maxfail 0 -n 3 --dist=load -v");
        assert_eq!(config.maxfail, None);
        assert_eq!(config.xdist_workers.as_deref(), Some("3"));
        assert_eq!(config.args, ["-ra", "-v"]);
        assert_eq!(
            config.env_addopts.as_deref(),
            Some("--maxfail 0 -n 3 --dist=load -v")
        );
        let config = discover_env(dir.path(), "--maxfail=5");
        assert_eq!(config.maxfail, Some(5));
        assert_eq!(config.args, ["-ra"]);
        let config = discover_env(dir.path(), "   ");
        assert_eq!(config.env_addopts, None);
        assert_eq!(config.args, ["-ra"]);
    }

    #[test]
    fn pytest_addopts_is_read_after_addopts() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pytest.ini",
            "[pytest]\naddopts = -o python_classes=Suite --ignore=legacy -p one\n",
        );
        let config = discover_env(
            dir.path(),
            "-o python_classes=Check -o disable_test_id_escaping_and_forfeit_all_rights_to_community_support=true --ignore=tests/b --import-mode=importlib -p no:two --pyargs --strict",
        );
        assert_eq!(config.python_classes.value, ["Check"]);
        assert_eq!(
            config.python_classes.source.to_string(),
            "-o python_classes (PYTEST_ADDOPTS)"
        );
        assert!(config.disable_test_id_escaping.value);
        assert!(config.strict_parametrization_ids.value);
        assert_eq!(
            config.ignore,
            [dir.path().join("legacy"), dir.path().join("tests/b")]
        );
        assert_eq!(config.import_mode, ImportMode::Importlib);
        assert_eq!(config.plugins, ["one", "no:two"]);
        assert_eq!(config.keyword, None);
        assert!(config.pyargs);
        let settings = config.settings();
        let env = settings
            .iter()
            .find(|s| s.name == "PYTEST_ADDOPTS")
            .unwrap();
        assert!(env.value.starts_with("-o python_classes=Check"));

        // `-c` and `-o addopts=` in `PYTEST_ADDOPTS` take part in finding
        // the config.
        write(dir.path(), "ci/other.ini", "[pytest]\naddopts = -x\n");
        let config = discover_env(dir.path(), "-c ci/other.ini");
        assert_eq!(config.inifile, Some(dir.path().join("ci/other.ini")));
        assert_eq!(config.maxfail, Some(1));
        let config = discover_env(dir.path(), "-o addopts=--maxfail=4");
        assert_eq!(config.maxfail, Some(4));
        assert_eq!(config.python_classes.value, ["Test"]);
    }

    #[test]
    fn settings_carry_their_sources() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "tox.ini", "[pytest]\ntestpaths = tests\n");
        let config = discover(dir.path(), &[]);
        let settings = config.settings();
        let row = |name: &str| {
            let setting = settings.iter().find(|s| s.name == name).unwrap();
            (
                setting.value.clone(),
                setting.source.display_row(dir.path()).to_string(),
            )
        };
        assert_eq!(
            row("testpaths"),
            ("tests".into(), "tox.ini [pytest]".into())
        );
        assert_eq!(row("python_classes").1, "default");
        assert_eq!(row("inifile").1, "tox.ini");
    }

    fn discover_args(cwd: &Path, env: Option<&str>, framework_args: &[&str]) -> PytestConfig {
        try_discover_args(cwd, env, framework_args).unwrap()
    }

    fn try_discover_args(
        cwd: &Path,
        env: Option<&str>,
        framework_args: &[&str],
    ) -> Result<PytestConfig, AdapterError> {
        let framework_args: Vec<String> = framework_args.iter().map(|s| (*s).to_string()).collect();
        PytestConfig::discover_with(
            &Project {
                framework_args: &framework_args,
                ..Project::new(cwd, cwd)
            },
            env,
        )
    }

    #[test]
    fn framework_args_are_read_after_pytest_addopts() {
        // pytest 9.1.1 parses addopts, then PYTEST_ADDOPTS, then the command
        // line, and the last `-o` wins: with `-o python_classes=Xc` in
        // addopts, `Yc` in PYTEST_ADDOPTS and `Zc` on the command line it
        // collects `ZcA` only.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = -o python_classes=Xc --ignore=a -p one -k first\n",
        );
        let config = discover_args(
            root,
            Some("-o python_classes=Yc --ignore=b -m db"),
            &[
                "-o",
                "python_classes=Zc",
                "--ignore=c",
                "--ignore-glob=*_old.py",
                "--deselect",
                "tests/test_a.py::test_x",
                "-p",
                " two ",
                "-k",
                "last",
                "--noconftest",
                "--import-mode=importlib",
                "--pyargs",
                "--strict",
            ],
        );
        assert_eq!(config.python_classes.value, ["Zc"]);
        assert_eq!(
            config.python_classes.source.to_string(),
            "-o python_classes (after --)"
        );
        assert_eq!(
            config.ignore,
            [root.join("a"), root.join("b"), root.join("c")]
        );
        assert_eq!(
            config.ignore_glob,
            [root.join("*_old.py").to_string_lossy().into_owned()]
        );
        assert_eq!(config.deselect, ["tests/test_a.py::test_x"]);
        assert_eq!(config.plugins, ["one", "two"]);
        assert_eq!(config.keyword.as_deref(), Some("last"));
        assert_eq!(config.tags.as_deref(), Some("db"));
        assert!(config.noconftest && config.pyargs);
        assert_eq!(config.import_mode, ImportMode::Importlib);
        assert!(config.strict_parametrization_ids.value);
        assert_eq!(
            config.strict_parametrization_ids.source.to_string(),
            "--strict (after --)"
        );

        // `-o addopts=` on the command line replaces the config's addopts
        // before pytest reads them (pytest: nothing collected, `Test` is
        // the class pattern again).
        let config = discover_args(root, None, &["-o", "addopts="]);
        assert_eq!(config.addopts.value, [] as [std::string::String; 0]);
        assert_eq!(config.python_classes.value, ["Test"]);
        assert_eq!(config.ignore, [] as [std::path::PathBuf; 0]);
        // Its value is the addopts the front-end's options start with,
        // filtered, and it isn't repeated after them.
        let config = discover_args(
            root,
            None,
            &[
                "-o",
                "addopts=-x -n 2 -ra",
                "-oaddopts=-q",
                "--override-ini=addopts=-v",
                "-o",
                "xfail_strict=true",
            ],
        );
        assert_eq!(config.addopts.value, ["-v"]);
        assert_eq!(config.args, ["-v", "-o", "xfail_strict=true"]);
        let config = discover_args(root, None, &["-o", "addopts=-x -n 2 -ra"]);
        assert_eq!(config.args, ["-ra"]);
        assert_eq!(config.maxfail, Some(1));
        assert_eq!(config.xdist_workers.as_deref(), Some("2"));

        // `-c` and `--rootdir` after `--` take part in finding the config.
        write(
            root,
            "ci/other.ini",
            "[pytest]\npython_files = check_*.py\n",
        );
        let config = discover_args(root, Some("-c pytest.ini"), &["-c", "ci/other.ini"]);
        assert_eq!(config.inifile, Some(root.join("ci/other.ini")));
        assert_eq!(config.rootdir, root.join("ci"));
        assert_eq!(config.python_files.value, ["check_*.py"]);
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        let config = discover_args(root, None, &["--rootdir=elsewhere"]);
        assert_eq!(config.rootdir, root.join("elsewhere"));
        assert_eq!(config.inifile, Some(root.join("pytest.ini")));
        let err = try_discover_args(root, Some("--rootdir missing"), &[]).unwrap_err();
        assert_eq!(
            err.to_string(),
            format!(
                "Directory '{}' not found. Check your '--rootdir' option.",
                root.join("missing").display()
            )
        );
    }

    #[test]
    fn framework_args_lift_maxfail_and_xdist() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "pytest.ini", "[pytest]\naddopts = --maxfail=3 -n 2\n");
        let config = discover_args(
            root,
            Some("-n 4"),
            &["-x", "-n", "auto", "--dist", "load", "-v", "--maxfail", "0"],
        );
        assert_eq!(config.maxfail, None);
        assert_eq!(config.xdist_workers.as_deref(), Some("auto"));
        assert_eq!(config.args, ["-v"]);
        let config = discover_args(root, None, &["-vx"]);
        assert_eq!(config.maxfail, Some(1));
        assert_eq!(config.xdist_workers.as_deref(), Some("2"));
        assert_eq!(config.args, ["-v"]);
    }

    #[test]
    fn junit_options_are_lifted_out() {
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_junit(&args(&[
                "--junitxml=a.xml",
                "-q",
                "--junit-xml",
                "b.xml",
                "--junit-prefix",
                "pre",
                "--junitprefix=fix",
            ])),
            (
                args(&["-q"]),
                Junit {
                    xml: Some("b.xml".into()),
                    prefix: Some("fix".into()),
                }
            )
        );
        // An empty value turns the report off; a missing one is pytest's
        // error to report.
        assert_eq!(
            split_junit(&args(&["--junitxml=", "--junitxml", "-x"])),
            (
                args(&["--junitxml", "-x"]),
                Junit {
                    xml: Some(String::new()),
                    prefix: None,
                }
            )
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = --junitxml=reports/addopts.xml -ra\n",
        );
        let config = discover_args(root, None, &[]);
        assert_eq!(config.junitxml, Some(root.join("reports/addopts.xml")));
        assert_eq!(config.args, ["-ra"]);
        // The last one wins: PYTEST_ADDOPTS over addopts, `--` over both.
        let config = discover_args(root, Some("--junit-xml=../env.xml -v"), &[]);
        assert_eq!(
            config.junitxml,
            Some(root.parent().unwrap().join("env.xml"))
        );
        assert_eq!(config.args, ["-ra", "-v"]);
        let config = discover_args(
            root,
            Some("--junit-xml=env.xml"),
            &["--junitxml", "./out/cli.xml", "--junit-prefix", "ci", "-q"],
        );
        assert_eq!(config.junitxml, Some(root.join("out/cli.xml")));
        assert_eq!(config.junit_prefix.as_deref(), Some("ci"));
        assert_eq!(config.args, ["-ra", "-q"]);
        let config = discover_args(root, None, &["--junitxml="]);
        assert_eq!(config.junitxml, None);
        // `os.path.expandvars` and `expanduser`, as pytest's `LogXML`, over
        // the project's environment: `ntpath`'s on Windows, where the home
        // is `USERPROFILE`.
        let (name, reference) = if cfg!(windows) {
            ("USERPROFILE", "%USERPROFILE%")
        } else {
            ("HOME", "${HOME}")
        };
        let home = root.join("home");
        let env = ProcessEnv::from_vars([(name, home.as_os_str())]);
        let expanded = |arg: &str| {
            let framework_args = [arg.to_string()];
            let project = Project {
                framework_args: &framework_args,
                env: &env,
                ..Project::new(root, root)
            };
            PytestConfig::discover_with(&project, None)
                .unwrap()
                .junitxml
        };
        assert_eq!(expanded("--junitxml=~/r.xml"), Some(home.join("r.xml")));
        assert_eq!(
            expanded(&format!("--junitxml={reference}/x/$AYE_UNSET_VAR.xml")),
            Some(home.join("x").join("$AYE_UNSET_VAR.xml"))
        );
        // A directory is pytest's usage error.
        std::fs::create_dir_all(root.join("reports")).unwrap();
        let err = try_discover_args(root, None, &["--junitxml", "reports"]).unwrap_err();
        assert_eq!(
            err.to_string(),
            "--junitxml must be a filename, given: reports"
        );
    }

    /// The doctest adapter runs doctests and reads pytest's doctest options
    /// itself: the front-end's options leave them out, unless no doctest
    /// adapter runs, when they stay in their places.
    #[test]
    fn doctest_options_are_lifted_unless_kept() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = --doctest-modules -ra\n",
        );
        let config = discover_args(
            root,
            Some("--doctest-glob '*.rst'"),
            &["-q", "--doctest-report", "ndiff"],
        );
        assert_eq!(config.args, ["-ra", "-q"]);
        assert_eq!(
            config.doctest,
            DoctestOptions {
                modules: true,
                globs: vec!["*.rst".into()],
            }
        );
        assert_eq!(
            config.keep_doctest_options().args,
            [
                "--doctest-modules",
                "-ra",
                "--doctest-glob",
                "*.rst",
                "-q",
                "--doctest-report",
                "ndiff"
            ]
        );
    }
}
