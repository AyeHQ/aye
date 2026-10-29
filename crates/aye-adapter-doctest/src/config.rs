//! The doctest adapter's configuration: pytest's doctest options, read from
//! pytest's configuration the way pytest would find it (approximately, see
//! the crate docs), and `[tool.aye.doctest]`, which wins over them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Project, Setting};
use aye_config::{Origin, Sourced};
use aye_static::EnvVars;

use crate::parser::known_option;

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

/// The option flags pytest's `doctest_optionflags` accepts
/// (`_get_flag_lookup`); any other name fails pytest's collection.
pub(crate) const PYTEST_OPTIONFLAGS: [&str; 9] = [
    "DONT_ACCEPT_TRUE_FOR_1",
    "DONT_ACCEPT_BLANKLINE",
    "NORMALIZE_WHITESPACE",
    "ELLIPSIS",
    "IGNORE_EXCEPTION_DETAIL",
    "COMPARISON_FLAGS",
    "ALLOW_UNICODE",
    "ALLOW_BYTES",
    "NUMBER",
];

/// `--doctest-report`'s choices.
const REPORT_CHOICES: [&str; 5] = ["none", "cdiff", "ndiff", "udiff", "only_first_failure"];

/// The keys `[tool.aye.doctest]` accepts.
const AYE_KEYS: [&str; 4] = ["modules", "glob", "optionflags", "encoding"];

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
            let sections = aye_config::read_ini(path).map_err(bad)?;
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
            let sections = aye_config::read_ini(path).map_err(bad)?;
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
    doctest_modules: Option<Origin>,
    globs: Vec<(String, Origin)>,
    report: Option<String>,
    ignore_import_errors: bool,
    blocked: bool,
    import_mode: Option<String>,
    ignore: Vec<String>,
    ignore_glob: Vec<String>,
    collect_in_virtualenv: bool,
    deselect: Vec<String>,
    overrides: BTreeMap<String, (String, Origin)>,
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

/// pytest's short options that take no value, which a cluster may combine
/// (`-vx`); and those that take one, the rest of the cluster or the next
/// argument (`-Werror`, `-W error`).
const SHORT_FLAGS: &str = "xvqslhVdf";
const SHORT_VALUED: &str = "kmcpoWrn";

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
    let n: i64 = value.trim().parse().ok()?;
    Some(match n {
        0 => 0,
        n if n < 0 => 1,
        n => u32::try_from(n).unwrap_or(u32::MAX),
    })
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
        let mut exitfirst = false;
        for (index, c) in cluster.char_indices() {
            if c == 'x' {
                exitfirst = true;
            } else if SHORT_VALUED.contains(c) {
                let rest = &cluster[index + c.len_utf8()..];
                let value = if rest.is_empty() {
                    *at += 1;
                    args.get(*at).cloned().unwrap_or_default()
                } else {
                    rest.strip_prefix('=').unwrap_or(rest).to_string()
                };
                return Some((exitfirst, Some((c, value))));
            } else if !SHORT_FLAGS.contains(c) {
                return None;
            }
        }
        Some((exitfirst, None))
    }

    /// A valued short option met in a cluster.
    fn short(&mut self, option: char, value: String, origin: &Origin) {
        match option {
            'W' => self.pythonwarnings.push(value),
            'c' => self.config_file = Some(value),
            'p' => self.plugin(&value),
            'o' => {
                if let Some((name, value)) = value.split_once('=') {
                    self.overrides
                        .insert(name.to_string(), (value.to_string(), origin.clone()));
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
    fn scan(&mut self, args: &[String], origin: &Origin) {
        let mut at = 0;
        while at < args.len() {
            let arg = args[at].as_str();
            match arg {
                "--doctest-modules" => self.doctest_modules = Some(origin.clone()),
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
                            self.short(option, value, origin);
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
                        self.globs.push((glob, origin.clone()));
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
                            .insert(name.to_string(), (value.to_string(), origin.clone()));
                    }
                }
            }
            at += 1;
        }
    }
}

/// `[tool.aye.doctest]`, checked.
#[derive(Debug, Clone, Default)]
struct AyeDoctest {
    modules: Option<bool>,
    glob: Option<Vec<String>>,
    optionflags: Option<Vec<String>>,
    encoding: Option<String>,
}

fn aye_doctest(table: Option<&toml::Table>, path: &Path) -> Result<AyeDoctest, AdapterError> {
    let mut out = AyeDoctest::default();
    let Some(table) = table else {
        return Ok(out);
    };
    let error = |message: String| {
        AdapterError::Config(format!(
            "invalid [tool.aye.doctest] in {}: {message}",
            path.display()
        ))
    };
    let strings = |key: &str, value: &toml::Value| -> Result<Vec<String>, AdapterError> {
        match value {
            toml::Value::String(s) => Ok(vec![s.clone()]),
            toml::Value::Array(items) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| error(format!("{key}: expected a list of strings")))
                })
                .collect(),
            _ => Err(error(format!("{key}: expected a list of strings"))),
        }
    };
    for (key, value) in table {
        match key.as_str() {
            "modules" => {
                out.modules = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| error("modules: expected true or false".into()))?,
                );
            }
            "glob" => out.glob = Some(strings(key, value)?),
            "optionflags" => {
                let flags = strings(key, value)?;
                if let Some(bad) = flags.iter().find(|f| !known_option(f)) {
                    return Err(error(format!(
                        "optionflags: unknown flag {bad:?} (expected one of {})",
                        crate::parser::KNOWN_OPTIONS.join(", ")
                    )));
                }
                out.optionflags = Some(flags);
            }
            "encoding" => {
                out.encoding = Some(
                    value
                        .as_str()
                        .ok_or_else(|| error("encoding: expected a string".into()))?
                        .to_string(),
                );
            }
            other => {
                return Err(error(format!(
                    "unknown key {other:?} (expected one of {})",
                    AYE_KEYS.join(", ")
                )));
            }
        }
    }
    Ok(out)
}

/// How pytest imports a module (`--import-mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    Prepend,
    Append,
    Importlib,
}

impl ImportMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepend => "prepend",
            Self::Append => "append",
            Self::Importlib => "importlib",
        }
    }
}

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

/// pytest's JUnit settings the binary reads from an adapter's context.
const JUNIT_SETTINGS: [&str; 5] = [
    "junit_suite_name",
    "junit_logging",
    "junit_log_passing_tests",
    "junit_duration_report",
    "junit_family",
];

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
        let env_args = pytest_addopts
            .map(aye_config::split_args)
            .unwrap_or_default();
        let env_origin = Origin::Env(EnvVars::PYTEST_ADDOPTS.into());
        let args_origin = Origin::Flag("--".into());
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
            let file = aye_fs::absolute(project.cwd, &file);
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
                .map(|a| aye_fs::absolute(project.cwd, a))
                .collect();
            locate(&aye_fs::common_ancestor(project.cwd, &paths))?
        };
        let rootdir = early
            .rootdir
            .as_ref()
            .map(|dir| aye_fs::absolute(project.cwd, Path::new(dir)))
            .or_else(|| ini.rootdir.clone())
            .unwrap_or_else(|| project.root.to_path_buf());
        let ini_origin = match &ini.file {
            Some(file) => Origin::file(file, ini.section.as_str()),
            None => Origin::Default,
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
        let origin_of = |name: &str| -> Origin {
            options
                .overrides
                .get(name)
                .map_or_else(|| ini_origin.clone(), |(_, origin)| origin.clone())
        };

        let aye_path = aye.map(|(_, path)| path.to_path_buf()).unwrap_or_default();
        let aye_settings = aye_doctest(aye.map(|(table, _)| table), &aye_path)?;
        let aye_origin = Origin::file(&aye_path, "[tool.aye.doctest]");

        let modules = match aye_settings.modules {
            Some(on) => Sourced::new(on, aye_origin.clone()),
            None => match (&options.doctest_modules, options.blocked) {
                (Some(origin), false) => Sourced::new(true, origin.clone()),
                _ => Sourced::default_value(false),
            },
        };
        let globs = if let Some(globs) = aye_settings.glob {
            Sourced::new(globs, aye_origin.clone())
        } else if options.blocked {
            Sourced::default_value(Vec::new())
        } else if let Some((_, origin)) = options.globs.last() {
            Sourced::new(
                options.globs.iter().map(|(g, _)| g.clone()).collect(),
                origin.clone(),
            )
        } else if ini.configured || beside_pytest {
            // pytest's default `--doctest-glob`: wherever pytest runs.
            Sourced::new(vec!["test*.txt".to_string()], Origin::Default)
        } else {
            Sourced::default_value(Vec::new())
        };
        let explicit_text = project.args.iter().any(|arg| {
            arg.extension().is_some_and(|e| e == "txt" || e == "rst")
                && aye_fs::absolute(project.cwd, arg).is_file()
        });
        let active = modules.value || !globs.value.is_empty() || explicit_text;

        let mut optionflags = match ini.values.get("doctest_optionflags") {
            Some(value) => Sourced::new(value.list(), origin_of("doctest_optionflags")),
            None => Sourced::default_value(vec!["ELLIPSIS".to_string()]),
        };
        if active
            && let Some(bad) = optionflags
                .value
                .iter()
                .find(|f| !PYTEST_OPTIONFLAGS.contains(&f.as_str()))
        {
            return Err(AdapterError::Config(format!(
                "doctest_optionflags: unknown flag {bad:?} in {} (pytest accepts {})",
                optionflags.origin,
                PYTEST_OPTIONFLAGS.join(", ")
            )));
        }
        if let Some(extra) = aye_settings.optionflags {
            for flag in extra {
                if !optionflags.value.contains(&flag) {
                    optionflags.value.push(flag);
                }
            }
            optionflags.origin = aye_origin.clone();
        }
        let encoding = match (aye_settings.encoding, ini.values.get("doctest_encoding")) {
            (Some(encoding), _) => Sourced::new(encoding, aye_origin),
            (None, Some(value)) => Sourced::new(value.text(), origin_of("doctest_encoding")),
            (None, None) => Sourced::default_value("utf-8".to_string()),
        };
        let report = options.report.unwrap_or_else(|| "udiff".to_string());
        if active && !REPORT_CHOICES.contains(&report.as_str()) {
            return Err(AdapterError::Config(format!(
                "--doctest-report: invalid choice {report:?} (choose from {})",
                REPORT_CHOICES.join(", ")
            )));
        }
        let import_mode = match options.import_mode.as_deref() {
            None | Some("prepend") => ImportMode::Prepend,
            Some("append") => ImportMode::Append,
            Some("importlib") => ImportMode::Importlib,
            Some(other) if active => {
                return Err(AdapterError::Config(format!(
                    "--import-mode: invalid choice {other:?} (choose from prepend, append, importlib)"
                )));
            }
            Some(_) => ImportMode::Prepend,
        };
        let inidir = ini
            .file
            .as_ref()
            .and_then(|f| f.parent())
            .map_or_else(|| rootdir.clone(), Path::to_path_buf);
        let confcutdir = options.confcutdir.as_ref().map_or_else(
            || inidir.clone(),
            |dir| aye_fs::absolute(project.cwd, Path::new(dir)),
        );
        let junitxml = options
            .junitxml
            .as_ref()
            .filter(|path| !path.is_empty())
            .map(|path| aye_fs::absolute(project.cwd, Path::new(path)));
        let junit = JUNIT_SETTINGS
            .iter()
            .filter_map(|name| {
                ini.values
                    .get(*name)
                    .map(|value| ((*name).to_string(), value.text()))
            })
            .collect();
        Ok(Self {
            explicit_text,
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
                .map(|p| aye_fs::absolute(project.cwd, Path::new(p)))
                .collect(),
            ignore_glob: options
                .ignore_glob
                .iter()
                .map(|g| {
                    aye_fs::absolute(project.cwd, Path::new(g))
                        .to_string_lossy()
                        .into_owned()
                })
                .collect(),
            collect_in_virtualenv: options.collect_in_virtualenv,
            deselect: options.deselect.clone(),
            import_mode,
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
                .map(|p| aye_fs::absolute(&inidir, Path::new(p)))
                .collect(),
            report,
            ignore_import_errors: options.ignore_import_errors,
            beside_pytest,
            python_files: ini.values.get("python_files").map_or_else(
                || vec!["test_*.py".to_string(), "*_test.py".to_string()],
                Value::list,
            ),
            rootdir,
            inifile: ini.file.clone().filter(|_| ini.configured),
            pytest_configured: ini.configured,
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
        let mut push = |name: &str, value: String, origin: &Origin| {
            out.push(Setting {
                name: name.to_string(),
                value,
                origin: origin.display_relative(root),
            });
        };
        push(
            "modules",
            self.modules.value.to_string(),
            &self.modules.origin,
        );
        push("glob", self.globs.value.join(" "), &self.globs.origin);
        push(
            "optionflags",
            self.optionflags.value.join(" "),
            &self.optionflags.origin,
        );
        push(
            "encoding",
            self.encoding.value.clone(),
            &self.encoding.origin,
        );
        out
    }
}
