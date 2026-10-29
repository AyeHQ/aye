//! pytest's configuration: rootdir and inifile discovery
//! (`_pytest/config/findpaths.py`) and the ini options collection depends
//! on, read with `aye-config`'s loaders, with the options `addopts`,
//! `PYTEST_ADDOPTS` and the arguments after `--` add, in pytest's order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Project, Setting};
use aye_config::{Origin, Sourced, ini_section, read_toml, split_args};
use aye_fs::common_ancestor;
use aye_static::{EnvVars, ProcessEnv};

/// Config file names, in the order pytest 9 tries them in each directory.
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

/// The options read from the config file.
#[derive(Debug, Default)]
struct RawConfig {
    values: BTreeMap<String, (Raw, Origin)>,
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

fn config_error(error: impl std::fmt::Display) -> AdapterError {
    AdapterError::Config(error.to_string())
}

/// Python's `str()` of a TOML scalar, as `[tool.pytest.ini_options]`
/// stringifies values.
fn python_str(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        toml::Value::Float(f) => crate::ids::float_repr(*f),
        other => other.to_string(),
    }
}

/// `load_config_dict_from_file`: the pytest options in `path`, with the
/// section they came from, or `None` when the file holds no pytest
/// configuration.
fn load_config(path: &Path) -> Result<Option<(RawConfig, String)>, AdapterError> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let from_ini = |section: &str, values: aye_config::ini::Section| {
        let mut config = RawConfig::default();
        for (key, value) in values {
            config.values.insert(
                key,
                (Raw::Ini(value), Origin::file(path, format!("[{section}]"))),
            );
        }
        (config, format!("[{section}]"))
    };
    match extension {
        "ini" => match ini_section(path, "pytest").map_err(config_error)? {
            Some(values) => Ok(Some(from_ini("pytest", values))),
            None if name == "pytest.ini" || name == ".pytest.ini" => {
                Ok(Some((RawConfig::default(), "[pytest]".into())))
            }
            None => Ok(None),
        },
        "cfg" => {
            let sections = aye_config::read_ini(path).map_err(config_error)?;
            if let Some(values) = sections.get("tool:pytest") {
                Ok(Some(from_ini("tool:pytest", values.clone())))
            } else if sections.contains_key("pytest") {
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
            let native = |table: &toml::Table, location: &str| {
                let mut config = RawConfig::default();
                for (key, value) in table {
                    config.values.insert(
                        key.clone(),
                        (Raw::Toml(value.clone()), Origin::file(path, location)),
                    );
                }
                config
            };
            if name == "pytest.toml" || name == ".pytest.toml" {
                let table = doc
                    .get("pytest")
                    .and_then(toml::Value::as_table)
                    .cloned()
                    .unwrap_or_default();
                return Ok(Some((native(&table, "[pytest]"), "[pytest]".into())));
            }
            let Some(tool) = doc
                .get("tool")
                .and_then(|t| t.get("pytest"))
                .and_then(toml::Value::as_table)
            else {
                return Ok(None);
            };
            let mut toml_mode = tool.clone();
            let ini_mode = toml_mode.remove("ini_options");
            match (toml_mode.is_empty(), ini_mode) {
                (false, Some(_)) => Err(AdapterError::Config(format!(
                    "{}: Cannot use both [tool.pytest] (native TOML types) and [tool.pytest.ini_options] (string-based INI format) simultaneously.",
                    path.display()
                ))),
                (false, None) => Ok(Some((
                    native(&toml_mode, "[tool.pytest]"),
                    "[tool.pytest]".into(),
                ))),
                (true, Some(ini)) => {
                    let location = "[tool.pytest.ini_options]";
                    let mut config = RawConfig::default();
                    for (key, value) in ini.as_table().into_iter().flatten() {
                        let raw = match value {
                            toml::Value::Array(items) => {
                                Raw::IniList(items.iter().map(python_str).collect())
                            }
                            other => Raw::Ini(python_str(other)),
                        };
                        config
                            .values
                            .insert(key.clone(), (raw, Origin::file(path, location)));
                    }
                    Ok(Some((config, location.into())))
                }
                (true, None) => Ok(None),
            }
        }
        _ => Ok(None),
    }
}

struct Located {
    rootdir: PathBuf,
    inifile: PathBuf,
    config: RawConfig,
}

/// `locate_config`: the first directory, from each of `args` upward, with a
/// config file holding pytest configuration; failing that, the first
/// `pyproject.toml` seen.
fn locate_config(args: &[PathBuf]) -> Result<Option<Located>, AdapterError> {
    let mut first_pyproject: Option<PathBuf> = None;
    for arg in args {
        for base in arg.ancestors() {
            for name in CONFIG_NAMES {
                let path = base.join(name);
                if !path.is_file() {
                    continue;
                }
                if name == "pyproject.toml" && first_pyproject.is_none() {
                    first_pyproject = Some(path.clone());
                }
                if let Some((config, _)) = load_config(&path)? {
                    return Ok(Some(Located {
                        rootdir: base.to_path_buf(),
                        inifile: path,
                        config,
                    }));
                }
            }
        }
    }
    Ok(first_pyproject.map(|path| Located {
        rootdir: path.parent().expect("a file has a parent").to_path_buf(),
        inifile: path,
        config: RawConfig::default(),
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
    let rootdir = aye_fs::normalize_absolute(&project.cwd.join(expandvars(project.env, arg)));
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
fn strtobool(value: &str) -> Option<bool> {
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

/// pytest-xdist's options: those taking a value, and flags. With
/// `-p no:xdist` in workers, any of them left in `addopts` would be an
/// unknown argument.
const XDIST_VALUED: [&str; 11] = [
    "-n",
    "--numprocesses",
    "--maxprocesses",
    "--max-worker-restart",
    "--dist",
    "--tx",
    "--px",
    "--rsyncdir",
    "--rsyncignore",
    "--testrunuid",
    "--maxschedchunk",
];
const XDIST_FLAGS: [&str; 5] = [
    "-d",
    "--loadscope-reorder",
    "--no-loadscope-reorder",
    "-f",
    "--looponfail",
];

/// `addopts` without pytest-xdist's options, and the `-n` value.
pub fn split_xdist(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut kept = Vec::new();
    let mut workers = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if XDIST_FLAGS.contains(&arg.as_str()) {
            continue;
        }
        if XDIST_VALUED.contains(&arg.as_str()) {
            let value = iter.next().cloned();
            if matches!(arg.as_str(), "-n" | "--numprocesses") {
                workers = value.or(workers);
            }
            continue;
        }
        if let Some((option, value)) = arg.split_once('=')
            && XDIST_VALUED.contains(&option)
        {
            if matches!(option, "-n" | "--numprocesses") {
                workers = Some(value.to_string());
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("-n")
            && !value.is_empty()
            && !arg.starts_with("--")
        {
            workers = Some(value.to_string());
            continue;
        }
        kept.push(arg.clone());
    }
    (kept, workers)
}

/// pytest's short options that take no value, which argparse lets a
/// cluster combine (`-vx`); pytest-xdist adds `-d` and `-f`.
const SHORT_FLAGS: &str = "xvqslhVdf";
/// pytest's short options that take a value: the rest of a cluster, or the
/// next argument, is it (`-rx` is `-r x`, not `-x`).
const SHORT_VALUED: &str = "kmcpoWrn";

/// A cluster of short options (`-vxs`) with `-x` taken out: whether it held
/// `-x`, and what is left of it (`None` when nothing is). An argument
/// that isn't such a cluster, or holds an option pytest doesn't know, is
/// left alone.
fn take_exitfirst(arg: &str) -> (bool, Option<String>) {
    let Some(cluster) = arg.strip_prefix('-') else {
        return (false, Some(arg.to_string()));
    };
    if cluster.is_empty() || cluster.starts_with('-') {
        return (false, Some(arg.to_string()));
    }
    let mut kept = String::from("-");
    let mut found = false;
    for (at, c) in cluster.char_indices() {
        if c == 'x' {
            found = true;
        } else if SHORT_FLAGS.contains(c) {
            kept.push(c);
        } else if SHORT_VALUED.contains(c) {
            kept.push_str(&cluster[at..]);
            break;
        } else {
            return (false, Some(arg.to_string()));
        }
    }
    if !found {
        return (false, Some(arg.to_string()));
    }
    (true, (kept.len() > 1).then_some(kept))
}

/// What `-x` or `--maxfail N` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaxFail {
    /// `--maxfail 0`: no limit.
    Unlimited,
    /// Stop after this many failures.
    After(u32),
}

impl MaxFail {
    /// The limit, if any.
    pub(crate) fn limit(self) -> Option<u32> {
        match self {
            Self::Unlimited => None,
            Self::After(n) => Some(n),
        }
    }
}

/// `args` without `-x`, `--exitfirst` and `--maxfail`, and what the last
/// of them asks for, as argparse lets the last one win (`None` when none
/// appears). A `--maxfail` value pytest would reject is left in place, for
/// the front-end to report, as pytest does.
pub(crate) fn split_maxfail(args: &[String]) -> (Vec<String>, Option<MaxFail>) {
    fn limit(value: &str) -> Option<MaxFail> {
        let n: i64 = value.trim().parse().ok()?;
        // pytest stops once the failures reach the limit, checked after
        // each failure: a negative limit stops at the first, like 1.
        Some(match n {
            0 => MaxFail::Unlimited,
            n if n < 0 => MaxFail::After(1),
            n => MaxFail::After(u32::try_from(n).unwrap_or(u32::MAX)),
        })
    }
    let mut kept = Vec::new();
    let mut maxfail = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--exitfirst" {
            maxfail = Some(MaxFail::After(1));
            continue;
        }
        if arg == "--maxfail" {
            if let Some(found) = iter.peek().and_then(|v| limit(v)) {
                iter.next();
                maxfail = Some(found);
                continue;
            }
            kept.push(arg.clone());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--maxfail=") {
            if let Some(found) = limit(value) {
                maxfail = Some(found);
                continue;
            }
            kept.push(arg.clone());
            continue;
        }
        let (exitfirst, rest) = take_exitfirst(arg);
        if exitfirst {
            maxfail = Some(MaxFail::After(1));
        }
        // A cluster ending in an option that takes a value takes the next
        // argument too.
        let takes_next = rest.as_deref().is_some_and(|r| {
            !r.starts_with("--")
                && r.len() >= 2
                && r.chars()
                    .skip(1)
                    .position(|c| SHORT_VALUED.contains(c))
                    .is_some_and(|at| at + 2 == r.len())
        });
        kept.extend(rest);
        if takes_next && let Some(value) = iter.next() {
            kept.push(value.clone());
        }
    }
    (kept, maxfail)
}

/// Where an option came from, in origins: `PYTEST_ADDOPTS`, or the
/// arguments after `--`.
const ENV: &str = EnvVars::PYTEST_ADDOPTS;
const CLI: &str = "after --";

/// pytest's JUnit options: the report's path, and the prefix of its class
/// names.
const JUNIT_XML: [&str; 2] = ["--junitxml", "--junit-xml"];
const JUNIT_PREFIX: [&str; 2] = ["--junitprefix", "--junit-prefix"];

/// Whether argparse takes `arg` as an option's value rather than as an
/// option: it doesn't start with `-`, or it looks like a negative number
/// (pytest has no option that does), or it holds a space.
fn is_value(arg: &str) -> bool {
    !arg.starts_with('-') || arg == "-" || arg.contains(' ') || arg.parse::<f64>().is_ok()
}

/// What the JUnit options in an argument list ask for: the last value of
/// each, `Some("")` for an empty one (which turns the report off).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Junit {
    pub xml: Option<String>,
    pub prefix: Option<String>,
}

/// `args` without pytest's JUnit options (`--junitxml`/`--junit-xml PATH`,
/// `--junitprefix`/`--junit-prefix STR`, each also with `=`), and the last
/// value of each, as argparse lets the last one win. An option missing its
/// value is left in place, for the front-end to reject, as pytest does.
pub(crate) fn split_junit(args: &[String]) -> (Vec<String>, Junit) {
    let mut kept = Vec::new();
    let mut found = Junit::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if arg.starts_with("--") => (name, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let slot = if JUNIT_XML.contains(&name) {
            &mut found.xml
        } else if JUNIT_PREFIX.contains(&name) {
            &mut found.prefix
        } else {
            kept.push(arg.clone());
            continue;
        };
        let value = if let Some(value) = inline {
            value
        } else if let Some(value) = iter.next_if(|next| is_value(next)) {
            value.clone()
        } else {
            kept.push(arg.clone());
            continue;
        };
        *slot = Some(value);
    }
    (kept, found)
}

/// Python's `os.path.expandvars`: `ntpath`'s on Windows, else `posixpath`'s,
/// over the environment, whichever variable `text` names.
fn expandvars(env: &ProcessEnv, text: &str) -> String {
    let var = |name: &str| env.get(name).map(str::to_string);
    if cfg!(windows) {
        nt_expandvars(text, var)
    } else {
        posix_expandvars(text, var)
    }
}

/// Python's `os.path.expanduser`: `ntpath`'s on Windows, else
/// `posixpath`'s, over the environment.
fn expanduser(env: &ProcessEnv, text: &str) -> String {
    let var = |name: &str| env.get(name).map(str::to_string);
    if cfg!(windows) {
        nt_expanduser(text, var)
    } else {
        posix_expanduser(text, var(EnvVars::HOME))
    }
}

/// `posixpath.expandvars`: `$name` and `${name}` replaced by the variable
/// `var` finds, when it finds one.
fn posix_expandvars(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, len) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after.find(|c: char| !word(c)).unwrap_or(after.len());
            (&after[..end], end)
        };
        match var(name) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..=at + len]),
        }
        rest = &rest[at + 1 + len..];
    }
    out.push_str(rest);
    out
}

/// `ntpath.expandvars`: `%name%`, `$name` and `${name}` replaced by the
/// variable `var` finds, when it finds one; `%%` and `$$` become `%` and
/// `$`, and nothing within single quotes is expanded. An unclosed `%`,
/// `${` or `'` leaves the rest as it is.
fn nt_expandvars(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(['\'', '%', '$']) {
        out.push_str(&rest[..at]);
        let token = &rest[at..];
        let after = &token[1..];
        // How long the token is, and what it becomes, if not itself.
        let (len, value) = if token.starts_with('\'') {
            (after.find('\'').map_or(token.len(), |end| end + 2), None)
        } else if token.starts_with('%') {
            if after.starts_with('%') {
                (2, Some("%".to_string()))
            } else {
                match after.find('%') {
                    Some(end) => (end + 2, var(&after[..end])),
                    None => (token.len(), None),
                }
            }
        } else if after.starts_with('$') {
            (2, Some("$".to_string()))
        } else if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (end + 3, var(&braced[..end])),
                None => (token.len(), None),
            }
        } else {
            let end = after.find(|c: char| !word(c)).unwrap_or(after.len());
            (end + 1, if end == 0 { None } else { var(&after[..end]) })
        };
        out.push_str(value.as_deref().unwrap_or(&token[..len]));
        rest = &token[len..];
    }
    out.push_str(rest);
    out
}

/// `posixpath.expanduser` for the current user, whose home is `home`: a
/// leading `~` becomes it. `~user` is left alone.
fn posix_expanduser(text: &str, home: Option<String>) -> String {
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    if !(rest.is_empty() || rest.starts_with('/')) {
        return text.to_string();
    }
    match home {
        Some(home) => {
            let home = home.trim_end_matches('/');
            let joined = format!("{home}{rest}");
            if joined.is_empty() {
                "/".to_string()
            } else {
                joined
            }
        }
        None => text.to_string(),
    }
}

/// `ntpath.expanduser`, with the variables `var` finds: a leading `~`
/// becomes `USERPROFILE` (else `HOMEDRIVE` and `HOMEPATH`); `~user`, that
/// user's home, guessed beside the current user's (`USERNAME`) as Python
/// guesses it.
fn nt_expanduser(text: &str, var: impl Fn(&str) -> Option<String>) -> String {
    let Some(rest) = text.strip_prefix('~') else {
        return text.to_string();
    };
    let (user, tail) = rest.split_at(rest.find(['\\', '/']).unwrap_or(rest.len()));
    let mut home = match (var(EnvVars::USERPROFILE), var(EnvVars::HOMEPATH)) {
        (Some(profile), _) => profile,
        (None, Some(path)) => nt_join(&var(EnvVars::HOMEDRIVE).unwrap_or_default(), &path),
        (None, None) => return text.to_string(),
    };
    if !user.is_empty() {
        let current = var(EnvVars::USERNAME);
        if current.as_deref() != Some(user) {
            let (parent, name) = nt_split(&home);
            if current.as_deref() != Some(name) {
                return text.to_string();
            }
            home = nt_join(parent, user);
        }
    }
    format!("{home}{tail}")
}

/// `ntpath.split` of a home directory: its parent (keeping the root, `C:\`)
/// and its name.
fn nt_split(path: &str) -> (&str, &str) {
    let Some(at) = path.rfind(['\\', '/']) else {
        let drive = if path.as_bytes().get(1) == Some(&b':') {
            2
        } else {
            0
        };
        return path.split_at(drive);
    };
    let head = path[..at].trim_end_matches(['\\', '/']);
    if head.is_empty() || head.ends_with(':') {
        (&path[..=at], &path[at + 1..])
    } else {
        (head, &path[at + 1..])
    }
}

/// `ntpath.join` of a drive or a directory and a name or a rooted path.
fn nt_join(head: &str, tail: &str) -> String {
    if head.is_empty() || head.ends_with(['\\', '/', ':']) || tail.starts_with(['\\', '/']) {
        format!("{head}{tail}")
    } else {
        format!("{head}\\{tail}")
    }
}

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
    let expanded = expanduser(env, &expandvars(env, value));
    Ok(Some(aye_fs::normalize_absolute(&cwd.join(expanded))))
}

/// pytest's doctest options that take no value, and those that take one.
const DOCTEST_FLAGS: [&str; 3] = [
    "--doctest-modules",
    "--doctest-continue-on-failure",
    "--doctest-ignore-import-errors",
];
const DOCTEST_VALUED: [&str; 2] = ["--doctest-glob", "--doctest-report"];

/// `args` without pytest's doctest options (`--doctest-modules`,
/// `--doctest-continue-on-failure`, `--doctest-ignore-import-errors`, and
/// `--doctest-glob PAT` and `--doctest-report STYLE`, each also with `=`,
/// the glob repeatable), and those options, as given: the doctest adapter
/// runs doctests and reads them itself. A valued option missing its value
/// is left in place, for the front-end to reject, as pytest does. pytest's parser takes no abbreviations, so
/// only the full names count.
pub(crate) fn split_doctest(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut lifted = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if DOCTEST_FLAGS.contains(&arg.as_str()) {
            lifted.push(arg.clone());
            continue;
        }
        if let Some((name, _)) = arg.split_once('=')
            && DOCTEST_VALUED.contains(&name)
        {
            lifted.push(arg.clone());
            continue;
        }
        if DOCTEST_VALUED.contains(&arg.as_str())
            && let Some(value) = iter.next_if(|next| is_value(next))
        {
            lifted.push(arg.clone());
            lifted.push(value.clone());
            continue;
        }
        kept.push(arg.clone());
    }
    (kept, lifted)
}

/// One argument list with what the runner and aye take over lifted out:
/// pytest-xdist's options, `-x`/`--maxfail`, the JUnit options, and the
/// doctest options (the doctest adapter's, when it runs).
struct Lifted {
    kept: Vec<String>,
    /// `kept` with the doctest options in their places.
    with_doctest: Vec<String>,
    workers: Option<String>,
    maxfail: Option<MaxFail>,
    junit: Junit,
    doctest: Vec<String>,
}

impl Lifted {
    fn from(args: &[String]) -> Self {
        let (kept, workers) = split_xdist(args);
        let (kept, maxfail) = split_maxfail(&kept);
        let (with_doctest, junit) = split_junit(&kept);
        let (kept, doctest) = split_doctest(&with_doctest);
        Self {
            kept,
            with_doctest,
            workers,
            maxfail,
            junit,
            doctest,
        }
    }
}

/// `args` without `-o addopts=…` (`--override-ini` too, in each spelling):
/// its value is the `addopts` already at the front of the options.
fn without_addopts_override(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "-o" | "--override-ini")
            && iter.peek().is_some_and(|next| next.starts_with("addopts="))
        {
            iter.next();
            continue;
        }
        let inline = arg
            .strip_prefix("--override-ini=")
            .or_else(|| arg.strip_prefix("-o=").or_else(|| arg.strip_prefix("-o")));
        if inline.is_some_and(|value| value.starts_with("addopts=")) {
            continue;
        }
        kept.push(arg.clone());
    }
    kept
}

/// The values of an option in an argument list: `--opt v`, `--opt=v`, and
/// `-ov` for short options.
fn option_values(args: &[String], long: &str, short: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let names = std::iter::once(long).chain(short);
        for name in names {
            if arg == name {
                if let Some(value) = iter.next() {
                    out.push(value.clone());
                }
                break;
            }
            if let Some(value) = arg.strip_prefix(name) {
                if let Some(value) = value.strip_prefix('=') {
                    out.push(value.to_string());
                    break;
                }
                if name.len() == 2 && !value.is_empty() {
                    out.push(value.to_string());
                    break;
                }
            }
        }
    }
    out
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

        let overrides = |raw: &mut RawConfig, args: &[String], from: &str| {
            for item in option_values(args, "--override-ini", Some("-o")) {
                if let Some((name, value)) = item.split_once('=') {
                    raw.values.insert(
                        name.to_string(),
                        (
                            Raw::Ini(value.to_string()),
                            Origin::Flag(format!("-o {name} ({from})")),
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
        .map(|(_, from)| format!("--strict ({from})"));
        if let Some(origin) = strict_from {
            raw.values.insert(
                "strict".into(),
                (Raw::Ini("true".into()), Origin::Flag(origin)),
            );
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
        })
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

    /// The resolved settings, with origins shown relative to `root`.
    pub fn settings(&self, root: &Path) -> Vec<Setting> {
        let origin_of_files = match &self.inifile {
            Some(file) => Origin::file(file, "")
                .display_relative(root)
                .trim()
                .to_string(),
            None => "default".to_string(),
        };
        let mut out = vec![
            Setting {
                name: "rootdir".into(),
                value: self.rootdir.display().to_string(),
                origin: origin_of_files.clone(),
            },
            Setting {
                name: "inifile".into(),
                value: self
                    .inifile
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                origin: origin_of_files,
            },
        ];
        let join = |items: &[String]| {
            items
                .iter()
                .map(|item| shlex::try_quote(item).map_or_else(|_| item.clone(), Into::into))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut push = |name: &str, value: String, origin: &Origin| {
            out.push(Setting {
                name: name.to_string(),
                value,
                origin: origin.display_relative(root),
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
            push(name, join(&sourced.value), &sourced.origin);
        }
        if let Some(env) = &self.env_addopts {
            push(
                EnvVars::PYTEST_ADDOPTS,
                env.clone(),
                &Origin::Env(EnvVars::PYTEST_ADDOPTS.into()),
            );
        }
        push(
            "markers",
            self.markers.value.join("\n"),
            &self.markers.origin,
        );
        push(
            "empty_parameter_set_mark",
            self.empty_parameter_set_mark.value.clone(),
            &self.empty_parameter_set_mark.origin,
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
                &sourced.origin,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config.python_classes.origin, Origin::Default);
        assert_eq!(
            config.testpaths.origin.display_relative(dir.path()),
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

    #[test]
    fn xdist_flags_are_lifted_out() {
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_xdist(&args(&["-nauto", "-ra"])),
            (args(&["-ra"]), Some("auto".into()))
        );
        assert_eq!(
            split_xdist(&args(&["-n=3", "--maxprocesses=2", "-d", "--tx", "popen"])),
            (Vec::new(), Some("3".into()))
        );
        assert_eq!(
            split_xdist(&args(&["--numprocesses", "logical", "-q"])),
            (args(&["-q"]), Some("logical".into()))
        );
        assert_eq!(
            split_xdist(&args(&["--dist=load", "--looponfail", "-x"])),
            (args(&["-x"]), None)
        );
    }

    #[test]
    fn maxfail_is_lifted_out() {
        // Each verified with pytest 9.1.1: `-vx` stops at the first
        // failure, `-rx` doesn't, and the last of `-x`/`--maxfail` wins.
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_maxfail(&args(&["-x", "-ra"])),
            (args(&["-ra"]), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["--exitfirst", "--maxfail", "3", "-q"])),
            (args(&["-q"]), Some(MaxFail::After(3)))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=2", "-x"])),
            (Vec::new(), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=0"])),
            (Vec::new(), Some(MaxFail::Unlimited))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=-4"])),
            (Vec::new(), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["-vxs", "-rx", "-k", "-x"])),
            (args(&["-vs", "-rx", "-k", "-x"]), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["-xk", "slow", "-vm", "x"])),
            (args(&["-k", "slow", "-vm", "x"]), Some(MaxFail::After(1)))
        );
        // Left for pytest to reject.
        assert_eq!(
            split_maxfail(&args(&["--maxfail", "many", "-Zx"])),
            (args(&["--maxfail", "many", "-Zx"]), None)
        );
        assert_eq!(split_maxfail(&args(&["-q"])), (args(&["-q"]), None));
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
            config.python_classes.origin,
            Origin::Flag("-o python_classes (PYTEST_ADDOPTS)".into())
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
        let settings = config.settings(dir.path());
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
    fn settings_carry_origins() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "tox.ini", "[pytest]\ntestpaths = tests\n");
        let config = discover(dir.path(), &[]);
        let settings = config.settings(dir.path());
        let testpaths = settings.iter().find(|s| s.name == "testpaths").unwrap();
        assert_eq!(testpaths.value, "tests");
        assert_eq!(testpaths.origin, "tox.ini [pytest]");
        let classes = settings
            .iter()
            .find(|s| s.name == "python_classes")
            .unwrap();
        assert_eq!(classes.origin, "default");
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
            config.python_classes.origin,
            Origin::Flag("-o python_classes (after --)".into())
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
            config.strict_parametrization_ids.origin,
            Origin::Flag("--strict (after --)".into())
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

    #[test]
    fn expandvars_follows_python() {
        // Each checked against `posixpath.expandvars` and `ntpath.expandvars`
        // (Python 3.11 and 3.14) with this environment, whose names Windows
        // reads whatever their case.
        let env = [("HOME", "/home/me"), ("MY-VAR", "dash"), ("1x", "num")];
        let posix = |name: &str| {
            let found = env.iter().find(|(key, _)| *key == name);
            found.map(|(_, value)| (*value).to_string())
        };
        let nt = |name: &str| {
            let found = env.iter().find(|(key, _)| key.eq_ignore_ascii_case(name));
            found.map(|(_, value)| (*value).to_string())
        };
        for (text, on_posix, on_nt) in [
            ("$HOME", "/home/me", "/home/me"),
            ("${HOME}/x", "/home/me/x", "/home/me/x"),
            ("a$HOME.b", "a/home/me.b", "a/home/me.b"),
            ("$AYE_UNSET", "$AYE_UNSET", "$AYE_UNSET"),
            ("${AYE_UNSET}", "${AYE_UNSET}", "${AYE_UNSET}"),
            ("${}", "${}", "${}"),
            ("${a=b}", "${a=b}", "${a=b}"),
            ("${unclosed", "${unclosed", "${unclosed"),
            ("${HOME", "${HOME", "${HOME"),
            ("$", "$", "$"),
            ("$$", "$$", "$"),
            ("$$HOME", "$/home/me", "$HOME"),
            ("cost $5", "cost $5", "cost $5"),
            ("$1x", "num", "num"),
            ("$home", "$home", "/home/me"),
            ("$HOME-x", "/home/me-x", "$HOME-x"),
            ("$MY-VAR/x", "$MY-VAR/x", "dash/x"),
            ("${MY-VAR}", "dash", "dash"),
            ("%HOME%", "%HOME%", "/home/me"),
            ("%home%\\x", "%home%\\x", "/home/me\\x"),
            ("%MY-VAR%", "%MY-VAR%", "dash"),
            ("%AYE_UNSET%", "%AYE_UNSET%", "%AYE_UNSET%"),
            ("%=a%", "%=a%", "%=a%"),
            ("%", "%", "%"),
            ("%%", "%%", "%"),
            ("100%", "100%", "100%"),
            ("%HOME", "%HOME", "%HOME"),
            ("a%%b%HOME%", "a%%b%HOME%", "a%b/home/me"),
            ("%%HOME%%", "%%HOME%%", "%HOME%"),
            ("$HOME%HOME%", "/home/me%HOME%", "/home/me/home/me"),
            ("'$HOME'", "'/home/me'", "'$HOME'"),
            ("'%HOME%", "'%HOME%", "'%HOME%"),
            ("it's $HOME", "it's /home/me", "it's $HOME"),
            ("'", "'", "'"),
            ("''$HOME", "''/home/me", "''/home/me"),
            (
                "x%HOME%y'%HOME%'%HOME%",
                "x%HOME%y'%HOME%'%HOME%",
                "x/home/mey'%HOME%'/home/me",
            ),
        ] {
            assert_eq!(posix_expandvars(text, posix), on_posix, "posix: {text}");
            assert_eq!(nt_expandvars(text, nt), on_nt, "nt: {text}");
        }
    }

    #[test]
    fn expanduser_follows_python() {
        type Env<'a> = &'a [(&'a str, &'a str)];
        // Checked against `posixpath.expanduser` with this HOME.
        for (text, home, expected) in [
            ("~", "/home/me", "/home/me"),
            ("~/r.xml", "/home/me/", "/home/me/r.xml"),
            ("~", "/", "/"),
            ("~/x", "/", "/x"),
            ("x~", "/home/me", "x~"),
            ("~aye-no-such-user/x", "/home/me", "~aye-no-such-user/x"),
        ] {
            assert_eq!(
                posix_expanduser(text, Some(home.to_string())),
                expected,
                "{text}"
            );
        }
        // Checked against `ntpath.expanduser` with each environment.
        let texts = [
            "~", "~/r.xml", "~\\r.xml", "~me\\x", "~other/x", "~other", "x~", "~me",
        ];
        let cases: [(Env<'_>, [&str; 8]); 6] = [
            (
                &[("USERPROFILE", "C:\\Users\\me"), ("USERNAME", "me")],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "C:\\Users\\me\\x",
                    "C:\\Users\\other/x",
                    "C:\\Users\\other",
                    "x~",
                    "C:\\Users\\me",
                ],
            ),
            (
                &[("USERPROFILE", "C:\\Users\\me")],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "~me\\x",
                    "~other/x",
                    "~other",
                    "x~",
                    "~me",
                ],
            ),
            (
                &[
                    ("HOMEDRIVE", "C:"),
                    ("HOMEPATH", "\\Users\\me"),
                    ("USERNAME", "me"),
                ],
                [
                    "C:\\Users\\me",
                    "C:\\Users\\me/r.xml",
                    "C:\\Users\\me\\r.xml",
                    "C:\\Users\\me\\x",
                    "C:\\Users\\other/x",
                    "C:\\Users\\other",
                    "x~",
                    "C:\\Users\\me",
                ],
            ),
            (
                &[("HOMEPATH", "\\Users\\me"), ("USERNAME", "me")],
                [
                    "\\Users\\me",
                    "\\Users\\me/r.xml",
                    "\\Users\\me\\r.xml",
                    "\\Users\\me\\x",
                    "\\Users\\other/x",
                    "\\Users\\other",
                    "x~",
                    "\\Users\\me",
                ],
            ),
            (
                &[("USERPROFILE", "C:\\me"), ("USERNAME", "me")],
                [
                    "C:\\me",
                    "C:\\me/r.xml",
                    "C:\\me\\r.xml",
                    "C:\\me\\x",
                    "C:\\other/x",
                    "C:\\other",
                    "x~",
                    "C:\\me",
                ],
            ),
            (&[("USERNAME", "me")], texts),
        ];
        for (env, expected) in cases {
            let var = |name: &str| {
                let found = env.iter().find(|(key, _)| *key == name);
                found.map(|(_, value)| (*value).to_string())
            };
            for (text, expected) in texts.iter().zip(expected) {
                assert_eq!(nt_expanduser(text, var), expected, "{text} with {env:?}");
            }
        }
    }
}
