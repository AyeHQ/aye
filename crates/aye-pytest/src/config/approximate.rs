//! pytest's configuration as the doctest adapter reads it: approximately
//! ([`ApproximateConfig`]). The first file holding pytest settings, from the
//! common directory of the paths given upward, as pytest's `locate_config`
//! finds it; `-c` and `--rootdir` from `PYTEST_ADDOPTS` or the arguments
//! after `--`; `-o`; then what the doctest adapter reads of the options
//! `addopts`, `PYTEST_ADDOPTS` and the arguments after `--` give.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Project};
use aye_config::{Sourced, ValueSource};
use aye_python_stdlib::os_path;
use aye_static::EnvVars;

use crate::argv::{MaxFail, ShortOptions};

/// The files pytest looks for in each directory, in its order
/// (`locate_config`).
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

/// A value from a configuration file.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Text(String),
    List(Vec<String>),
    Bool(bool),
}

impl Value {
    fn from_toml(value: &toml::Value) -> Self {
        match value {
            toml::Value::String(s) => Self::Text(s.clone()),
            toml::Value::Boolean(b) => Self::Bool(*b),
            toml::Value::Array(items) => Self::List(
                items
                    .iter()
                    .map(|item| match item {
                        toml::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect(),
            ),
            other => Self::Text(other.to_string()),
        }
    }

    /// pytest's `args` and `paths` types: a string is split as a shell
    /// would.
    fn list(&self) -> Vec<String> {
        match self {
            Self::Text(text) => aye_config::split_args(text),
            Self::List(items) => items.clone(),
            Self::Bool(b) => vec![if *b { "True" } else { "False" }.to_string()],
        }
    }

    fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::List(items) => items.join(" "),
            Self::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        }
    }

    /// pytest's `linelist` type (`filterwarnings`): a string is one entry
    /// per non-blank line.
    fn lines(&self) -> Vec<String> {
        match self {
            Self::Text(text) => text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect(),
            Self::List(items) => items.clone(),
            Self::Bool(_) => vec![self.text()],
        }
    }

    /// pytest's `bool` type (`_strtobool`).
    fn truth(&self) -> bool {
        match self {
            Self::Bool(b) => *b,
            other => matches!(
                other.text().trim().to_lowercase().as_str(),
                "y" | "yes" | "t" | "true" | "on" | "1"
            ),
        }
    }
}

/// pytest's configuration file: where it is and what it says.
#[derive(Debug, Clone, Default)]
struct Ini {
    rootdir: Option<PathBuf>,
    file: Option<PathBuf>,
    /// The section holding pytest's settings (`[pytest]`, `[tool:pytest]`,
    /// `[tool.pytest.ini_options]`, …), for origins.
    section: String,
    values: BTreeMap<String, Value>,
    /// The file holds pytest configuration (not just a bare
    /// `pyproject.toml`).
    configured: bool,
}

/// A configuration file's pytest section (for origins) and its values.
type Section = (String, BTreeMap<String, Value>);

/// The pytest settings in `path`, `None` when it holds none (pytest's
/// `load_config_dict_from_file`).
fn load(path: &Path) -> Result<Option<Section>, AdapterError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let bad = |e: aye_config::ConfigError| AdapterError::Config(e.to_string());
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    match extension.as_deref() {
        Some("ini") => {
            let sections = crate::ini::read_ini(path).map_err(bad)?;
            if let Some(section) = sections.get("pytest") {
                let values = section
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Text(v.clone())))
                    .collect();
                return Ok(Some(("[pytest]".into(), values)));
            }
            if name == "pytest.ini" || name == ".pytest.ini" {
                return Ok(Some(("[pytest]".into(), BTreeMap::new())));
            }
            Ok(None)
        }
        Some("cfg") => {
            let sections = crate::ini::read_ini(path).map_err(bad)?;
            Ok(sections.get("tool:pytest").map(|section| {
                let values = section
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Text(v.clone())))
                    .collect();
                ("[tool:pytest]".to_string(), values)
            }))
        }
        Some("toml") => {
            let doc = aye_config::read_toml(path).map_err(bad)?;
            let table = |t: &toml::Table| -> BTreeMap<String, Value> {
                t.iter()
                    .map(|(k, v)| (k.clone(), Value::from_toml(v)))
                    .collect()
            };
            if name == "pytest.toml" || name == ".pytest.toml" {
                let values = doc
                    .get("pytest")
                    .and_then(toml::Value::as_table)
                    .map(table)
                    .unwrap_or_default();
                return Ok(Some(("[pytest]".into(), values)));
            }
            let Some(tool_pytest) = doc
                .get("tool")
                .and_then(|t| t.get("pytest"))
                .and_then(toml::Value::as_table)
            else {
                return Ok(None);
            };
            let mut native = tool_pytest.clone();
            let ini_options = native.remove("ini_options");
            if !native.is_empty() {
                return Ok(Some(("[tool.pytest]".into(), table(&native))));
            }
            Ok(ini_options
                .as_ref()
                .and_then(toml::Value::as_table)
                .map(|t| ("[tool.pytest.ini_options]".to_string(), table(t))))
        }
        _ => Ok(None),
    }
}

/// pytest's `locate_config` from `start` upward: the first file holding
/// pytest configuration, else the first `pyproject.toml` met.
fn locate(start: &Path) -> Result<Ini, AdapterError> {
    let mut bare: Option<PathBuf> = None;
    for dir in start.ancestors() {
        for name in CONFIG_NAMES {
            let path = dir.join(name);
            if !path.is_file() {
                continue;
            }
            if name == "pyproject.toml" && bare.is_none() {
                bare = Some(path.clone());
            }
            if let Some((section, values)) = load(&path)? {
                return Ok(Ini {
                    rootdir: Some(dir.to_path_buf()),
                    file: Some(path),
                    section,
                    values,
                    configured: true,
                });
            }
        }
    }
    Ok(bare.map_or_else(Ini::default, |path| Ini {
        rootdir: path.parent().map(Path::to_path_buf),
        file: Some(path),
        ..Ini::default()
    }))
}

/// The options pytest's command line (addopts, `PYTEST_ADDOPTS`, the
/// arguments after `--`) sets that matter here.
#[derive(Debug, Clone, Default)]
struct Options {
    doctest_modules: Option<ValueSource>,
    globs: Vec<(String, ValueSource)>,
    report: Option<String>,
    ignore_import_errors: bool,
    blocked: bool,
    import_mode: Option<String>,
    ignore: Vec<String>,
    ignore_glob: Vec<String>,
    collect_in_virtualenv: bool,
    deselect: Vec<String>,
    overrides: BTreeMap<String, (String, ValueSource)>,
    config_file: Option<String>,
    rootdir: Option<String>,
    /// `--doctest-continue-on-failure`.
    continue_on_failure: bool,
    /// `-W`/`--pythonwarnings`, in order.
    pythonwarnings: Vec<String>,
    /// `--noconftest`.
    noconftest: bool,
    /// `--confcutdir`, as given.
    confcutdir: Option<String>,
    /// `-x`/`--exitfirst`/`--maxfail`: the limit, 0 for none.
    maxfail: Option<u32>,
    /// `--junitxml`/`--junit-xml`, `--junitprefix`/`--junit-prefix`.
    junitxml: Option<String>,
    junit_prefix: Option<String>,
    /// `-p NAME` plugins (not `no:`), which may mark doctest items.
    plugins: Vec<String>,
}

/// The value of option `long` (or short option `short`) at `args[*at]`,
/// in its `--long=value`, `--long value`, `-svalue` and `-s value` forms;
/// advances `at` past a separate value.
fn value_of(args: &[String], at: &mut usize, long: &str, short: Option<&str>) -> Option<String> {
    let arg = &args[*at];
    if let Some(value) = arg.strip_prefix(long) {
        if let Some(value) = value.strip_prefix('=') {
            return Some(value.to_string());
        }
        if value.is_empty() {
            *at += 1;
            return args.get(*at).cloned();
        }
        return None;
    }
    let short = short?;
    let value = arg.strip_prefix(short)?;
    if value.is_empty() {
        *at += 1;
        return args.get(*at).cloned();
    }
    Some(value.strip_prefix('=').unwrap_or(value).to_string())
}

/// A `--maxfail` value as pytest takes it: 0 is no limit, a negative one
/// stops at the first failure.
fn maxfail_value(value: &str) -> Option<u32> {
    MaxFail::parse(value).map(|limit| limit.limit().unwrap_or(0))
}

impl Options {
    /// A cluster of short options (`-x`, `-vx`, `-Werror`, `-vW error`):
    /// whether it holds `-x`, and the valued option ending it with its value
    /// (advancing `at` past a separate one). `None` for anything else.
    fn cluster(args: &[String], at: &mut usize) -> Option<(bool, Option<(char, String)>)> {
        let cluster = args[*at].strip_prefix('-')?;
        if cluster.is_empty() || cluster.starts_with('-') {
            return None;
        }
        let ShortOptions { flags, valued } = ShortOptions::WITH_XDIST;
        let mut exitfirst = false;
        for (index, c) in cluster.char_indices() {
            if c == 'x' {
                exitfirst = true;
            } else if valued.contains(c) {
                let rest = &cluster[index + c.len_utf8()..];
                let value = if rest.is_empty() {
                    *at += 1;
                    args.get(*at).cloned().unwrap_or_default()
                } else {
                    rest.strip_prefix('=').unwrap_or(rest).to_string()
                };
                return Some((exitfirst, Some((c, value))));
            } else if !flags.contains(c) {
                return None;
            }
        }
        Some((exitfirst, None))
    }

    /// A valued short option met in a cluster.
    fn short(&mut self, option: char, value: String, source: &ValueSource) {
        match option {
            'W' => self.pythonwarnings.push(value),
            'c' => self.config_file = Some(value),
            'p' => self.plugin(&value),
            'o' => {
                if let Some((name, value)) = value.split_once('=') {
                    self.overrides
                        .insert(name.to_string(), (value.to_string(), source.clone()));
                }
            }
            _ => {}
        }
    }

    fn plugin(&mut self, plugin: &str) {
        let plugin = plugin.trim();
        if plugin == "no:doctest" {
            self.blocked = true;
        } else if plugin == "doctest" {
            self.blocked = false;
        } else if !plugin.starts_with("no:") {
            self.plugins.push(plugin.to_string());
        }
    }

    /// Read `args`, in order (the last value wins, as in argparse).
    fn scan(&mut self, args: &[String], source: &ValueSource) {
        let mut at = 0;
        while at < args.len() {
            let arg = args[at].as_str();
            match arg {
                "--doctest-modules" => self.doctest_modules = Some(source.clone()),
                "--doctest-ignore-import-errors" => self.ignore_import_errors = true,
                "--doctest-continue-on-failure" => self.continue_on_failure = true,
                "--collect-in-virtualenv" => self.collect_in_virtualenv = true,
                "--noconftest" => self.noconftest = true,
                "--exitfirst" => self.maxfail = Some(1),
                // Options whose value must not be read as an option.
                "--basetemp" => {
                    at += 1;
                }
                _ => {
                    let mut next = at;
                    if let Some((exitfirst, valued)) = Self::cluster(args, &mut next) {
                        at = next;
                        if exitfirst {
                            self.maxfail = Some(1);
                        }
                        if let Some((option, value)) = valued {
                            self.short(option, value, source);
                        }
                    } else if let Some(value) = value_of(args, &mut at, "--maxfail", None) {
                        if let Some(limit) = maxfail_value(&value) {
                            self.maxfail = Some(limit);
                        }
                    } else if let Some(filter) = value_of(args, &mut at, "--pythonwarnings", None) {
                        self.pythonwarnings.push(filter);
                    } else if let Some(dir) = value_of(args, &mut at, "--confcutdir", None) {
                        self.confcutdir = Some(dir);
                    } else if let Some(path) = value_of(args, &mut at, "--junitxml", None)
                        .or_else(|| value_of(args, &mut at, "--junit-xml", None))
                    {
                        self.junitxml = Some(path);
                    } else if let Some(prefix) = value_of(args, &mut at, "--junitprefix", None)
                        .or_else(|| value_of(args, &mut at, "--junit-prefix", None))
                    {
                        self.junit_prefix = Some(prefix);
                    } else if let Some(glob) = value_of(args, &mut at, "--doctest-glob", None) {
                        self.globs.push((glob, source.clone()));
                    } else if let Some(report) = value_of(args, &mut at, "--doctest-report", None) {
                        self.report = Some(report.to_lowercase());
                    } else if let Some(mode) = value_of(args, &mut at, "--import-mode", None) {
                        self.import_mode = Some(mode);
                    } else if let Some(path) = value_of(args, &mut at, "--ignore-glob", None) {
                        self.ignore_glob.push(path);
                    } else if let Some(path) = value_of(args, &mut at, "--ignore", None) {
                        self.ignore.push(path);
                    } else if let Some(prefix) = value_of(args, &mut at, "--deselect", None) {
                        self.deselect.push(prefix);
                    } else if let Some(file) = value_of(args, &mut at, "--config-file", Some("-c"))
                    {
                        self.config_file = Some(file);
                    } else if let Some(dir) = value_of(args, &mut at, "--rootdir", None) {
                        self.rootdir = Some(dir);
                    } else if let Some(plugin) = value_of(args, &mut at, "--plugin", Some("-p")) {
                        self.plugin(&plugin);
                    } else if let Some(pair) = value_of(args, &mut at, "--override-ini", Some("-o"))
                        && let Some((name, value)) = pair.split_once('=')
                    {
                        self.overrides
                            .insert(name.to_string(), (value.to_string(), source.clone()));
                    }
                }
            }
            at += 1;
        }
    }
}

/// pytest's JUnit settings the binary reads from an adapter's context.
const JUNIT_SETTINGS: [&str; 5] = [
    "junit_suite_name",
    "junit_logging",
    "junit_log_passing_tests",
    "junit_duration_report",
    "junit_family",
];

/// pytest's configuration, as the doctest adapter reads it.
#[derive(Debug, Clone)]
pub struct ApproximateConfig {
    /// pytest's rootdir: `--rootdir`, else the configuration file's
    /// directory, else the project root.
    pub rootdir: PathBuf,
    /// pytest's configuration file, if any.
    pub inifile: Option<PathBuf>,
    /// A pytest configuration exists (not just a bare `pyproject.toml`).
    pub configured: bool,
    /// Where `--doctest-modules` was given, if it was.
    pub doctest_modules: Option<ValueSource>,
    /// The `--doctest-glob` patterns, in order, each with where it was
    /// given.
    pub doctest_globs: Vec<(String, ValueSource)>,
    /// `-p no:doctest` blocks pytest's doctest plugin (a later `-p
    /// doctest` unblocks it).
    pub doctest_blocked: bool,
    /// `doctest_optionflags`, as a list, with where it came from.
    pub doctest_optionflags: Option<Sourced<Vec<String>>>,
    /// `doctest_encoding`, with where it came from.
    pub doctest_encoding: Option<Sourced<String>>,
    /// `--doctest-report`, lower-cased.
    pub doctest_report: Option<String>,
    /// `--doctest-ignore-import-errors`.
    pub ignore_import_errors: bool,
    /// `--doctest-continue-on-failure`.
    pub continue_on_failure: bool,
    pub testpaths: Vec<String>,
    pub norecursedirs: Vec<String>,
    /// `--ignore`, absolute.
    pub ignore: Vec<PathBuf>,
    /// `--ignore-glob`, absolute.
    pub ignore_glob: Vec<String>,
    pub collect_in_virtualenv: bool,
    /// `--deselect`: node id prefixes, as given (relative to the rootdir).
    pub deselect: Vec<String>,
    /// `--import-mode`, as given.
    pub import_mode: Option<String>,
    pub consider_namespace_packages: bool,
    /// `pythonpath`, absolute.
    pub pythonpath: Vec<PathBuf>,
    /// pytest's `python_files`.
    pub python_files: Vec<String>,
    /// pytest's `filterwarnings`.
    pub filterwarnings: Vec<String>,
    /// `-W`/`--pythonwarnings`, in order.
    pub pythonwarnings: Vec<String>,
    /// `--noconftest`.
    pub noconftest: bool,
    /// pytest's confcutdir: `--confcutdir`, else the configuration file's
    /// directory, else the rootdir.
    pub confcutdir: PathBuf,
    /// `-x`/`--exitfirst`/`--maxfail`: `None` when none says, 0 for no
    /// limit.
    pub maxfail: Option<u32>,
    /// `--junitxml`/`--junit-xml`, absolute (`None` when empty or unset),
    /// and `--junitprefix`/`--junit-prefix`.
    pub junitxml: Option<PathBuf>,
    pub junit_prefix: Option<String>,
    /// pytest's `junit_*` settings that the configuration sets, by name.
    pub junit: BTreeMap<String, String>,
    /// `-p NAME` plugins (not `no:`), which may mark doctest items.
    pub plugins: Vec<String>,
}

impl ApproximateConfig {
    /// Read pytest's configuration for `project`. `pytest_addopts` is
    /// `PYTEST_ADDOPTS`.
    pub fn discover(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
    ) -> Result<Self, AdapterError> {
        let env_args = pytest_addopts
            .map(aye_config::split_args)
            .unwrap_or_default();
        let env_origin = ValueSource::Env(EnvVars::PYTEST_ADDOPTS);
        let args_origin = ValueSource::Cli("--");
        // `-c`, `--rootdir` and `-o` from the environment and the command
        // line take part in finding the configuration.
        let mut early = Options::default();
        early.scan(&env_args, &env_origin);
        early.scan(project.framework_args, &args_origin);
        let given = project
            .config_file
            .map(Path::to_path_buf)
            .or_else(|| early.config_file.as_ref().map(|f| project.cwd.join(f)));
        let mut ini = if let Some(file) = given {
            let file = os_path::abspath(project.cwd, &file);
            let (section, values) = load(&file)?.unwrap_or_default();
            Ini {
                rootdir: file.parent().map(Path::to_path_buf),
                file: Some(file),
                section,
                values,
                configured: true,
            }
        } else {
            let paths: Vec<PathBuf> = project
                .args
                .iter()
                .map(|a| os_path::abspath(project.cwd, a))
                .collect();
            locate(&aye_fs::common_ancestor(project.cwd, &paths))?
        };
        let rootdir = early
            .rootdir
            .as_ref()
            .map(|dir| os_path::abspath(project.cwd, Path::new(dir)))
            .or_else(|| ini.rootdir.clone())
            .unwrap_or_else(|| project.root.to_path_buf());
        let ini_origin = match &ini.file {
            Some(file) => ValueSource::file(file, ini.section.as_str()),
            None => ValueSource::Default,
        };
        // `-o` from any of the three overrides the file; `addopts` itself
        // only from the environment and the command line.
        let mut addopts = ini
            .values
            .get("addopts")
            .map(Value::list)
            .unwrap_or_default();
        if let Some((value, _)) = early.overrides.get("addopts") {
            addopts = aye_config::split_args(value);
        }
        let mut options = Options::default();
        options.scan(&addopts, &ini_origin);
        options.scan(&env_args, &env_origin);
        options.scan(project.framework_args, &args_origin);
        for (name, (value, _)) in &options.overrides {
            ini.values.insert(name.clone(), Value::Text(value.clone()));
        }
        let origin_of = |name: &str| -> ValueSource {
            options
                .overrides
                .get(name)
                .map_or_else(|| ini_origin.clone(), |(_, origin)| origin.clone())
        };

        let inidir = ini
            .file
            .as_ref()
            .and_then(|f| f.parent())
            .map_or_else(|| rootdir.clone(), Path::to_path_buf);
        let confcutdir = options.confcutdir.as_ref().map_or_else(
            || inidir.clone(),
            |dir| os_path::abspath(project.cwd, Path::new(dir)),
        );
        let junitxml = options
            .junitxml
            .as_ref()
            .filter(|path| !path.is_empty())
            .map(|path| os_path::abspath(project.cwd, Path::new(path)));
        let junit = JUNIT_SETTINGS
            .iter()
            .filter_map(|name| {
                ini.values
                    .get(*name)
                    .map(|value| ((*name).to_string(), value.text()))
            })
            .collect();
        Ok(Self {
            continue_on_failure: options.continue_on_failure,
            filterwarnings: ini
                .values
                .get("filterwarnings")
                .map(Value::lines)
                .unwrap_or_default(),
            pythonwarnings: options.pythonwarnings.clone(),
            noconftest: options.noconftest,
            confcutdir,
            maxfail: options.maxfail,
            junitxml,
            junit_prefix: options.junit_prefix.clone(),
            junit,
            plugins: options.plugins.clone(),
            testpaths: ini
                .values
                .get("testpaths")
                .map(Value::list)
                .unwrap_or_default(),
            norecursedirs: ini.values.get("norecursedirs").map_or_else(
                || DEFAULT_NORECURSEDIRS.map(str::to_string).to_vec(),
                Value::list,
            ),
            ignore: options
                .ignore
                .iter()
                .map(|p| os_path::abspath(project.cwd, Path::new(p)))
                .collect(),
            ignore_glob: options
                .ignore_glob
                .iter()
                .map(|g| {
                    os_path::abspath(project.cwd, Path::new(g))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            collect_in_virtualenv: options.collect_in_virtualenv,
            deselect: options.deselect.clone(),
            consider_namespace_packages: ini
                .values
                .get("consider_namespace_packages")
                .is_some_and(Value::truth),
            pythonpath: ini
                .values
                .get("pythonpath")
                .map(Value::list)
                .unwrap_or_default()
                .iter()
                .map(|p| os_path::abspath(&inidir, Path::new(p)))
                .collect(),
            ignore_import_errors: options.ignore_import_errors,
            python_files: ini.values.get("python_files").map_or_else(
                || vec!["test_*.py".to_string(), "*_test.py".to_string()],
                Value::list,
            ),
            rootdir,
            inifile: ini.file.clone().filter(|_| ini.configured),
            configured: ini.configured,
            doctest_modules: options.doctest_modules.clone(),
            doctest_globs: options.globs.clone(),
            doctest_blocked: options.blocked,
            doctest_optionflags: ini
                .values
                .get("doctest_optionflags")
                .map(|value| Sourced::new(value.list(), origin_of("doctest_optionflags"))),
            doctest_encoding: ini
                .values
                .get("doctest_encoding")
                .map(|value| Sourced::new(value.text(), origin_of("doctest_encoding"))),
            doctest_report: options.report.clone(),
            import_mode: options.import_mode.clone(),
        })
    }
}

/// Pinned where paths are written with `/`.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::config::{RawConfig, load_config, locate_config};

    /// The configuration files the pins read, one each, alone in their
    /// directory.
    const FILES: &[(&str, &str)] = &[
        (
            "pytest.ini",
            "[pytest]\naddopts = -ra -q\nmarkers =\n    slow: slow\n    db\nxfail_strict = true\n",
        ),
        ("pytest.ini", "[other]\nx = 1\n"),
        (".pytest.ini", "[pytest]\ntestpaths = a b\n"),
        ("tox.ini", "[pytest]\npython_files = check_*.py\n"),
        ("tox.ini", "[tox]\nenvlist = py3\n"),
        ("custom.ini", "[pytest]\npython_classes = Check\n"),
        ("pytest.ini", "x = 1\n"),
        ("setup.cfg", "[tool:pytest]\naddopts = --doctest-modules\n"),
        ("setup.cfg", "[pytest]\naddopts = -q\n"),
        ("setup.cfg", "[metadata]\nname = x\n"),
        (
            "pytest.toml",
            "[pytest]\naddopts = [\"-ra\"]\nstrict = true\ntimeout = 5\nratio = 1.5\nwhen = 1979-05-27\n[pytest.sub]\nkey = \"v\"\n",
        ),
        ("pytest.toml", "[other]\nx = 1\n"),
        (".pytest.toml", "[pytest]\nminversion = \"9.0\"\n"),
        (
            "pyproject.toml",
            "[tool.pytest]\naddopts = [\"-ra\"]\ncollect_imported_tests = false\n",
        ),
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\naddopts = \"-ra\"\nxfail_strict = true\ntimeout = 5\nratio = 1.0\nbig = 1e10\nneg = -0.5\nmarkers = [\"slow\", 1, true, 2.5]\nwhen = 1979-05-27\n",
        ),
        (
            "pyproject.toml",
            "[tool.pytest]\naddopts = [\"-ra\"]\n[tool.pytest.ini_options]\naddopts = \"-q\"\n",
        ),
        ("pyproject.toml", "[tool.pytest]\nini_options = 1\n"),
        ("pyproject.toml", "[tool.pytest]\n"),
        ("pyproject.toml", "[project]\nname = \"x\"\n"),
        ("pyproject.toml", "[tool]\npytest = 1\n"),
        ("pyproject.toml", "[tool.pytest\n"),
        ("notes.txt", "[pytest]\nx = 1\n"),
    ];

    /// Directory trees the pins locate a configuration in, from a
    /// directory of each (`start`).
    const TREES: &[(&[(&str, &str)], &str)] = &[
        (
            &[
                ("a/pyproject.toml", "[project]\nname = 'x'\n"),
                ("tox.ini", "[pytest]\ntestpaths = a\n"),
            ],
            "a/b",
        ),
        (&[("a/pyproject.toml", "[project]\nname = 'x'\n")], "a/b"),
        (
            &[
                ("a/setup.cfg", "[pytest]\naddopts = -q\n"),
                ("pytest.ini", "[pytest]\n"),
            ],
            "a",
        ),
        (
            &[
                (
                    "a/pyproject.toml",
                    "[tool.pytest.ini_options]\naddopts = '-q'\n",
                ),
                ("a/pytest.ini", "[pytest]\nx = 1\n"),
            ],
            "a",
        ),
        (&[], "a"),
    ];

    fn exact_values(config: &RawConfig) -> String {
        config
            .values
            .iter()
            .map(|(key, (raw, source))| format!("{key}={raw:?} <- {source}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn approximate_values(values: &BTreeMap<String, Value>) -> String {
        values
            .iter()
            .map(|(key, value)| format!("{key}={value:?}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// What each rules makes of each of [`FILES`] (`load_config` and
    /// `load`), and where each locates a configuration in each of
    /// [`TREES`] (`locate_config` and `locate`), the root written `<root>`.
    fn readings() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (name, body) in FILES {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            let root = dir.path().to_string_lossy().into_owned();
            let exact = match load_config(&path) {
                Ok(Some((config, section))) => format!("{section} {}", exact_values(&config)),
                Ok(None) => "none".to_string(),
                Err(err) => format!("error: {err}"),
            };
            let approximate = match load(&path) {
                Ok(Some((section, values))) => {
                    format!("{section} {}", approximate_values(&values))
                }
                Ok(None) => "none".to_string(),
                Err(err) => format!("error: {err}"),
            };
            out.push((
                exact.replace(&root, "<root>"),
                approximate.replace(&root, "<root>"),
            ));
        }
        for (files, start) in TREES {
            let dir = tempfile::tempdir().unwrap();
            for (rel, body) in *files {
                let path = dir.path().join(rel);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, body).unwrap();
            }
            let start = dir.path().join(start);
            std::fs::create_dir_all(&start).unwrap();
            let root = dir.path().to_string_lossy().into_owned();
            let exact = match locate_config(std::slice::from_ref(&start)) {
                Ok(Some(found)) => format!(
                    "{} {} {}",
                    found.rootdir.display(),
                    found.inifile.display(),
                    exact_values(&found.config)
                ),
                Ok(None) => "none".to_string(),
                Err(err) => format!("error: {err}"),
            };
            let approximate = match locate(&start) {
                Ok(ini) => format!(
                    "{:?} {:?} {} configured={} {}",
                    ini.rootdir,
                    ini.file,
                    ini.section,
                    ini.configured,
                    approximate_values(&ini.values)
                ),
                Err(err) => format!("error: {err}"),
            };
            out.push((
                exact.replace(&root, "<root>"),
                approximate.replace(&root, "<root>"),
            ));
        }
        out
    }

    /// Each rules' reading of each of [`FILES`], then where each locates
    /// a configuration in each of [`TREES`], recorded from the pytest
    /// adapter's and the doctest adapter's readers before they shared any
    /// code: `(exact, approximate)`.
    const READINGS: &[(&str, &str)] = &[
        (
            "[pytest] addopts=Ini(\"-ra -q\") <- <root>/pytest.ini [pytest]; markers=Ini(\"slow: slow\\ndb\") <- <root>/pytest.ini [pytest]; xfail_strict=Ini(\"true\") <- <root>/pytest.ini [pytest]",
            "[pytest] addopts=Text(\"-ra -q\"); markers=Text(\"slow: slow\\ndb\"); xfail_strict=Text(\"true\")",
        ),
        ("[pytest] ", "[pytest] "),
        (
            "[pytest] testpaths=Ini(\"a b\") <- <root>/.pytest.ini [pytest]",
            "[pytest] testpaths=Text(\"a b\")",
        ),
        (
            "[pytest] python_files=Ini(\"check_*.py\") <- <root>/tox.ini [pytest]",
            "[pytest] python_files=Text(\"check_*.py\")",
        ),
        ("none", "none"),
        (
            "[pytest] python_classes=Ini(\"Check\") <- <root>/custom.ini [pytest]",
            "[pytest] python_classes=Text(\"Check\")",
        ),
        (
            "error: failed to parse <root>/pytest.ini: line 1: key outside of a section",
            "error: failed to parse <root>/pytest.ini: line 1: key outside of a section",
        ),
        (
            "[tool:pytest] addopts=Ini(\"--doctest-modules\") <- <root>/setup.cfg [tool:pytest]",
            "[tool:pytest] addopts=Text(\"--doctest-modules\")",
        ),
        (
            "error: <root>/setup.cfg: [pytest] section in setup.cfg files is no longer supported, change to [tool:pytest] instead.",
            "none",
        ),
        ("none", "none"),
        (
            "[pytest] addopts=Toml(Array([String(\"-ra\")])) <- <root>/pytest.toml [pytest]; ratio=Toml(Float(1.5)) <- <root>/pytest.toml [pytest]; strict=Toml(Boolean(true)) <- <root>/pytest.toml [pytest]; sub=Toml(Table({\"key\": String(\"v\")})) <- <root>/pytest.toml [pytest]; timeout=Toml(Integer(5)) <- <root>/pytest.toml [pytest]; when=Toml(Datetime(Datetime { date: Some(Date { year: 1979, month: 5, day: 27 }), time: None, offset: None })) <- <root>/pytest.toml [pytest]",
            "[pytest] addopts=List([\"-ra\"]); ratio=Text(\"1.5\"); strict=Bool(true); sub=Text(\"{ key = \\\"v\\\" }\"); timeout=Text(\"5\"); when=Text(\"1979-05-27\")",
        ),
        ("[pytest] ", "[pytest] "),
        (
            "[pytest] minversion=Toml(String(\"9.0\")) <- <root>/.pytest.toml [pytest]",
            "[pytest] minversion=Text(\"9.0\")",
        ),
        (
            "[tool.pytest] addopts=Toml(Array([String(\"-ra\")])) <- <root>/pyproject.toml [tool.pytest]; collect_imported_tests=Toml(Boolean(false)) <- <root>/pyproject.toml [tool.pytest]",
            "[tool.pytest] addopts=List([\"-ra\"]); collect_imported_tests=Bool(false)",
        ),
        (
            "[tool.pytest.ini_options] addopts=Ini(\"-ra\") <- <root>/pyproject.toml [tool.pytest.ini_options]; big=Ini(\"10000000000.0\") <- <root>/pyproject.toml [tool.pytest.ini_options]; markers=IniList([\"slow\", \"1\", \"True\", \"2.5\"]) <- <root>/pyproject.toml [tool.pytest.ini_options]; neg=Ini(\"-0.5\") <- <root>/pyproject.toml [tool.pytest.ini_options]; ratio=Ini(\"1.0\") <- <root>/pyproject.toml [tool.pytest.ini_options]; timeout=Ini(\"5\") <- <root>/pyproject.toml [tool.pytest.ini_options]; when=Ini(\"1979-05-27\") <- <root>/pyproject.toml [tool.pytest.ini_options]; xfail_strict=Ini(\"True\") <- <root>/pyproject.toml [tool.pytest.ini_options]",
            "[tool.pytest.ini_options] addopts=Text(\"-ra\"); big=Text(\"10000000000.0\"); markers=List([\"slow\", \"1\", \"true\", \"2.5\"]); neg=Text(\"-0.5\"); ratio=Text(\"1.0\"); timeout=Text(\"5\"); when=Text(\"1979-05-27\"); xfail_strict=Bool(true)",
        ),
        (
            "error: <root>/pyproject.toml: Cannot use both [tool.pytest] (native TOML types) and [tool.pytest.ini_options] (string-based INI format) simultaneously.",
            "[tool.pytest] addopts=List([\"-ra\"])",
        ),
        ("[tool.pytest.ini_options] ", "none"),
        ("none", "none"),
        ("none", "none"),
        ("none", "none"),
        (
            "error: failed to parse <root>/pyproject.toml: TOML parse error at line 1, column 13\n  |\n1 | [tool.pytest\n  |             ^\nunclosed table, expected `]`\n",
            "error: failed to parse <root>/pyproject.toml: TOML parse error at line 1, column 13\n  |\n1 | [tool.pytest\n  |             ^\nunclosed table, expected `]`\n",
        ),
        ("none", "none"),
        (
            "<root> <root>/tox.ini testpaths=Ini(\"a\") <- <root>/tox.ini [pytest]",
            "Some(\"<root>\") Some(\"<root>/tox.ini\") [pytest] configured=true testpaths=Text(\"a\")",
        ),
        (
            "<root>/a <root>/a/pyproject.toml ",
            "Some(\"<root>/a\") Some(\"<root>/a/pyproject.toml\")  configured=false ",
        ),
        (
            "error: <root>/a/setup.cfg: [pytest] section in setup.cfg files is no longer supported, change to [tool:pytest] instead.",
            "Some(\"<root>\") Some(\"<root>/pytest.ini\") [pytest] configured=true ",
        ),
        (
            "<root>/a <root>/a/pytest.ini x=Ini(\"1\") <- <root>/a/pytest.ini [pytest]",
            "Some(\"<root>/a\") Some(\"<root>/a/pytest.ini\") [pytest] configured=true x=Text(\"1\")",
        ),
        ("none", "None None  configured=false "),
    ];

    /// The two rules read and locate configuration files as their two
    /// readers always have, differences included: what the exact rules
    /// refuse and the approximation reads on from (a `setup.cfg` with
    /// `[pytest]`, both TOML forms at once), TOML values coerced to text
    /// or kept, an `ini_options` that isn't a table.
    #[test]
    fn each_rules_reads_configuration_files_as_it_always_has() {
        let readings = readings();
        assert_eq!(readings.len(), READINGS.len());
        for ((exact, approximate), (pinned_exact, pinned_approximate)) in
            readings.iter().zip(READINGS)
        {
            assert_eq!(exact, pinned_exact);
            assert_eq!(approximate, pinned_approximate);
        }
    }
}
