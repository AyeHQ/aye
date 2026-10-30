//! Configuration as a mechanism. Finds the project root, reads
//! `[tool.aye]`, and provides generic loaders (toml tables, ini sections, the
//! upward search) that engines use to read their own framework's settings. Every
//! resolved value remembers where it came from ([`ValueSource`]) for `aye
//! config`.
//!
//! Owns: loading and value sources, and the worker count a run asks for
//! ([`WorkerCount`]). Does not own: the meaning of any framework's options;
//! `python_files` means something to the pytest engine, not here.
//!
//! Depends on: aye-fs, for the lexical path algebra the root is found
//! with.

//!
//! Decided (implementation, October 2026):
//! - **The project root** is the nearest directory, from the common ancestor
//!   of the paths given (or the current directory) upward, holding a
//!   `pyproject.toml`; failing that, one holding `.git`; failing that, the
//!   starting directory. Test ids are relative to it. Frameworks keep their
//!   own notion of a root (pytest's rootdir) and translate. Paths are made
//!   absolute lexically, as `aye_fs` decides. Where the paths spell one
//!   directory two ways that share more than the filesystem's root (both
//!   under Linux's `/tmp`), their common ancestor stays lexical, as pytest's
//!   `get_common_ancestor` keeps it, but when no project holds it the root is
//!   searched for from the resolved one ([`project_root`]), so one command
//!   finds one root on every platform.
//! - **`[tool.aye]` rejects unknown keys and values**, so a typo is an error
//!   rather than a setting that silently does nothing: `output` must be
//!   `text` or `json`, in any case as on the command line (it is kept in
//!   lower case; `human` and `agent`, the formats' names until October 2026,
//!   are errors that say what to use instead), and `workers` a number or
//!   `"auto"`. `workers = 0` is one worker, as `-n 0` is on the command line
//!   and pytest-xdist's `-n 0` runs the tests in one process. `python` must
//!   not be blank, `timeout` must be a finite number of seconds, 0 or more
//!   (0 is none, as pytest-timeout reads it), and `adapters` must list at
//!   least one name, none twice; which names exist is the binary's to check,
//!   and each adapter checks its own table (`[tool.aye.unittest]`,
//!   `[tool.aye.doctest]`), even when it doesn't run. `keep-runs` (how many
//!   runs' logs `.aye/runs` keeps for `aye show`, 10 unless set) must be a
//!   whole number, 0 or more: 0 keeps none.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use serde::Deserialize;

pub mod ini;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {message}")]
    Parse { path: PathBuf, message: String },
    #[error("invalid [tool.aye] in {path}: {message}")]
    Aye { path: PathBuf, message: String },
}

/// Where a resolved value came from, for `aye config`. One source is shown
/// one way wherever it is (a row of `[tool.aye]`, of `[pytest]`, an
/// adapter's error), so that each spelling is decided here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueSource {
    /// Built in: nothing set it. Shown `default`.
    Default,
    /// aye's command line: a flag (`--python`), or `--` for the arguments
    /// after it, shown as it is.
    Cli(&'static str),
    /// An environment variable, by name; shown `$PYTEST_ADDOPTS`.
    Env(&'static str),
    /// A file, and where in it: a table or section (`[tool.aye]`,
    /// `[pytest]`), or `None` for the file as a whole (pytest's inifile).
    /// Shown `pyproject.toml [tool.aye]`.
    File {
        path: Arc<Path>,
        table: Option<Box<str>>,
    },
    /// An option of a framework's, and where the framework read it (its
    /// own words for its sources); shown `-o python_classes
    /// (PYTEST_ADDOPTS)`, `--strict (after --)`.
    FrameworkOption { option: Box<str>, via: &'static str },
    /// A rule of aye's, in words, shown as they are.
    Rule(&'static str),
}

impl ValueSource {
    /// The `table` of the file at `path` (`[tool.aye]`).
    pub fn file(path: impl AsRef<Path>, table: impl Into<Box<str>>) -> Self {
        Self::File {
            path: Arc::from(path.as_ref()),
            table: Some(table.into()),
        }
    }

    /// The file at `path` as a whole.
    pub fn whole_file(path: impl AsRef<Path>) -> Self {
        Self::File {
            path: Arc::from(path.as_ref()),
            table: None,
        }
    }

    /// The source as a row of `aye config` shows it (`pyproject.toml
    /// [tool.aye]`): a file relative to `root` when it is inside it.
    pub fn display_row<'a>(&'a self, root: &'a Path) -> impl fmt::Display + 'a {
        Row { source: self, root }
    }
}

/// [`ValueSource::display_row`].
struct Row<'a> {
    source: &'a ValueSource,
    root: &'a Path,
}

impl fmt::Display for Row<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.source {
            ValueSource::File { path, table } => {
                let shown = path.strip_prefix(self.root).unwrap_or(path).display();
                match table {
                    Some(table) => write!(f, "{shown} {table}"),
                    // A file as a whole is shown trimmed, as it always was.
                    None => f.write_str(shown.to_string().trim()),
                }
            }
            other => other.fmt(f),
        }
    }
}

/// The source in full: a file by its absolute path.
impl fmt::Display for ValueSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::Cli(flag) => f.write_str(flag),
            Self::Env(name) => write!(f, "${name}"),
            Self::File { path, table } => match table {
                Some(table) => write!(f, "{} {table}", path.display()),
                None => write!(f, "{}", path.display()),
            },
            Self::FrameworkOption { option, via } => write!(f, "{option} ({via})"),
            Self::Rule(words) => f.write_str(words),
        }
    }
}

/// A value and where it came from. Two are equal, and hash alike, when
/// their values are: where a value came from changes nothing it does.
#[derive(Debug, Clone)]
pub struct Sourced<T> {
    pub value: T,
    pub source: ValueSource,
}

impl<T> Sourced<T> {
    pub const fn new(value: T, source: ValueSource) -> Self {
        Self { value, source }
    }

    pub const fn default_value(value: T) -> Self {
        Self::new(value, ValueSource::Default)
    }
}

impl<T: PartialEq> PartialEq for Sourced<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T: Eq> Eq for Sourced<T> {}

impl<T: Hash> Hash for Sourced<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

// ---------------------------------------------------------------- searching

/// `start` and its ancestors, nearest first.
pub fn upward(start: &Path) -> impl Iterator<Item = &Path> {
    start.ancestors()
}

/// The first `dir/name` that is a file, searching `names` in order in each
/// directory from `start` upward.
pub fn find_upward(start: &Path, names: &[&str]) -> Option<PathBuf> {
    upward(start).find_map(|dir| {
        names
            .iter()
            .map(|name| dir.join(name))
            .find(|p| p.is_file())
    })
}

/// aye's project root: see the crate docs. `start` is normalized lexically
/// first, so `<root>/tests/..` gives `<root>`.
pub fn find_root(start: &Path) -> PathBuf {
    let start = aye_fs::normpath(start);
    marked(&start).unwrap_or(start)
}

/// aye's project root for `paths` given in `cwd`: [`find_root`] from their
/// [`aye_fs::common_ancestor`]. When nothing above that ancestor marks a
/// project, but the paths, resolved, share a directory inside it (one
/// directory spelled two ways under a shared `/tmp`, as on Linux), the
/// search starts from that directory instead, so the root doesn't depend on
/// how much two spellings happen to share.
pub fn project_root(cwd: &Path, paths: &[PathBuf]) -> PathBuf {
    let ancestor = aye_fs::normpath(&aye_fs::common_ancestor(cwd, paths));
    if let Some(root) = marked(&ancestor) {
        return root;
    }
    if let Some(resolved) = aye_fs::resolved_common_ancestor(cwd, paths)
        && let Ok(real) = fs::canonicalize(&ancestor)
        && resolved != real
        && resolved.starts_with(&real)
        && let Some(root) = marked(&resolved)
    {
        return root;
    }
    ancestor
}

/// The nearest directory from `start` upward holding a `pyproject.toml`,
/// failing that a `.git`.
fn marked(start: &Path) -> Option<PathBuf> {
    ["pyproject.toml", ".git"].into_iter().find_map(|marker| {
        upward(start)
            .find(|d| d.join(marker).exists())
            .map(Path::to_path_buf)
    })
}

// ---------------------------------------------------------------- loaders

pub fn read(path: &Path) -> Result<String, ConfigError> {
    fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Parse a TOML file.
pub fn read_toml(path: &Path) -> Result<toml::Table, ConfigError> {
    toml::from_str(&read(path)?).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// The table at `keys` in a TOML file (`["tool", "pytest", "ini_options"]`),
/// or `None` when any key is missing.
pub fn toml_table(path: &Path, keys: &[&str]) -> Result<Option<toml::Table>, ConfigError> {
    let doc = read_toml(path)?;
    let mut table = &doc;
    for key in keys {
        match table.get(*key).and_then(toml::Value::as_table) {
            Some(next) => table = next,
            None => return Ok(None),
        }
    }
    Ok(Some(table.clone()))
}

/// Parse an INI file into sections.
pub fn read_ini(path: &Path) -> Result<BTreeMap<String, ini::Section>, ConfigError> {
    ini::parse(&read(path)?).map_err(|message| ConfigError::Parse {
        path: path.to_path_buf(),
        message,
    })
}

/// One section of an INI file, or `None` when absent.
pub fn ini_section(path: &Path, section: &str) -> Result<Option<ini::Section>, ConfigError> {
    Ok(read_ini(path)?.remove(section))
}

/// Split a value the way INI-style list options are written: on whitespace
/// (and newlines).
pub fn split_list(value: &str) -> Vec<String> {
    value.split_whitespace().map(str::to_string).collect()
}

/// Split a value the way a shell would (`addopts`).
pub fn split_args(value: &str) -> Vec<String> {
    shlex::split(value).unwrap_or_else(|| vec![value.to_string()])
}

/// A TOML value as a list of strings: an array of strings, or a string split
/// with `split`.
pub fn toml_strings(value: &toml::Value, split: fn(&str) -> Vec<String>) -> Option<Vec<String>> {
    match value {
        toml::Value::String(s) => Some(split(s)),
        toml::Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        ),
        toml::Value::Integer(i) => Some(vec![i.to_string()]),
        toml::Value::Boolean(b) => Some(vec![b.to_string()]),
        toml::Value::Float(f) => Some(vec![f.to_string()]),
        _ => None,
    }
}

// ---------------------------------------------------------------- [tool.aye]

/// `[tool.aye]`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct AyeSettings {
    /// Worker processes: a number, or `"auto"` for one per CPU.
    pub workers: Option<Workers>,
    /// The interpreter that runs the tests.
    pub python: Option<String>,
    /// Default output format: `text` or `json` (see [`OUTPUTS`]), in lower
    /// case whatever the file's case; anything else fails
    /// [`AyeConfig::load`]. How much either shows is the command line's
    /// (`-v`, `-q`).
    pub output: Option<String>,
    /// Per-test timeout in seconds, finite and not negative; 0 is none
    /// (see [`AyeSettings::timeout`]).
    pub timeout: Option<f64>,
    /// Which adapters run, in priority order (`["pytest", "doctest"]`): at
    /// least one, none twice. Which names exist is the binary's business
    /// (its registry). Unset: chosen from the project (see the registry).
    pub adapters: Option<Vec<String>>,
    /// How many runs' logs `.aye/runs` keeps, for `aye show`: a whole
    /// number, 0 or more; 0 keeps none (see [`AyeSettings::keep_runs`]).
    pub keep_runs: Option<i64>,
    /// `[tool.aye.unittest]`: the unittest adapter's own settings, read by
    /// that adapter.
    pub unittest: Option<toml::Table>,
    /// `[tool.aye.doctest]`: the doctest adapter's own settings, read by
    /// that adapter.
    pub doctest: Option<toml::Table>,
}

impl AyeSettings {
    /// No `[tool.aye]`: nothing set, as [`AyeSettings::default`].
    pub const NONE: Self = Self {
        workers: None,
        python: None,
        output: None,
        timeout: None,
        adapters: None,
        keep_runs: None,
        unittest: None,
        doctest: None,
    };

    /// Check the values serde's types let through, and put `output` in
    /// lower case.
    fn check(&mut self) -> Result<(), String> {
        if let Some(output) = &mut self.output {
            let Some(known) = OUTPUTS.iter().find(|o| o.eq_ignore_ascii_case(output)) else {
                let instead = RENAMED_OUTPUTS
                    .iter()
                    .find(|(old, _)| old.eq_ignore_ascii_case(output))
                    .map_or_else(String::new, |(old, new)| format!("; {old:?} is now {new}"));
                return Err(format!(
                    "output = {output:?}: expected text or json{instead}"
                ));
            };
            *output = (*known).to_string();
        }
        if let Some(python) = &self.python
            && python.trim().is_empty()
        {
            return Err(format!(
                "python = {python:?}: expected the path or name of an interpreter"
            ));
        }
        if let Some(timeout) = self.timeout
            && !(timeout.is_finite() && timeout >= 0.0)
        {
            return Err(format!(
                "timeout = {timeout}: expected a number of seconds, 0 or more"
            ));
        }
        if let Some(keep) = self.keep_runs
            && keep < 0
        {
            return Err(format!(
                "keep-runs = {keep}: expected a number of runs, 0 or more"
            ));
        }
        if let Some(adapters) = &self.adapters {
            if adapters.is_empty() {
                return Err("adapters = []: list at least one adapter".to_string());
            }
            for (at, name) in adapters.iter().enumerate() {
                if name.trim().is_empty() {
                    return Err(format!("adapters: {name:?} is not an adapter's name"));
                }
                if adapters[..at].contains(name) {
                    return Err(format!("adapters: {name:?} is listed twice"));
                }
            }
        }
        Ok(())
    }

    /// The per-test timeout in seconds, if any: `timeout = 0` is none, as
    /// pytest-timeout reads it.
    pub fn timeout(&self) -> Option<f64> {
        self.timeout.filter(|t| *t > 0.0)
    }

    /// How many runs' logs to keep: `keep-runs`, else [`KEEP_RUNS`]. 0
    /// keeps none.
    pub fn keep_runs(&self) -> usize {
        self.keep_runs
            .map_or(KEEP_RUNS, |keep| usize::try_from(keep).unwrap_or(0))
    }

    /// An adapter's own table, `[tool.aye.<name>]`, when it is set.
    pub fn adapter_table(&self, name: &str) -> Option<&toml::Table> {
        match name {
            "unittest" => self.unittest.as_ref(),
            "doctest" => self.doctest.as_ref(),
            _ => None,
        }
    }
}

/// How many workers a run asks for, as `-n` and pytest-xdist's `-n` read
/// it: a number, or `auto` (and pytest-xdist's `logical`) for one per CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerCount {
    Auto,
    /// A number of workers; 0 is one, as `-n 0` is.
    Count(u32),
}

impl WorkerCount {
    /// The number of workers: a count as given, 0 being one; for `Auto`,
    /// the CPUs the process may use, else one.
    pub fn resolve(self) -> u32 {
        match self {
            Self::Count(n) => n.max(1),
            Self::Auto => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(1)),
        }
    }
}

impl FromStr for WorkerCount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" | "logical" => Ok(Self::Auto),
            n => n
                .parse()
                .map(Self::Count)
                .map_err(|_| format!("expected a number or `auto`, got `{n}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workers {
    /// A number of workers; 0 is one, as `-n 0` is.
    Count(u32),
    Keyword(WorkersKeyword),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkersKeyword {
    Auto,
}

/// What `workers` accepts, for its error message.
const WORKERS_EXPECTED: &str = "a number of workers or \"auto\"";

impl<'de> Deserialize<'de> for Workers {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;

        impl serde::de::Visitor<'_> for Visitor {
            type Value = Workers;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(WORKERS_EXPECTED)
            }

            fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Workers, E> {
                u32::try_from(n)
                    .map(Workers::Count)
                    .map_err(|_| E::custom(format!("workers = {n}: too many")))
            }

            fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Workers, E> {
                u64::try_from(n).map_or_else(
                    |_| {
                        Err(E::custom(format!(
                            "workers = {n}: expected {WORKERS_EXPECTED}"
                        )))
                    },
                    |n| self.visit_u64(n),
                )
            }

            fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Workers, E> {
                if s == "auto" {
                    Ok(Workers::Keyword(WorkersKeyword::Auto))
                } else {
                    Err(E::custom(format!(
                        "workers = {s:?}: expected {WORKERS_EXPECTED}"
                    )))
                }
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

/// How many runs' logs `.aye/runs` keeps when `keep-runs` isn't set.
pub const KEEP_RUNS: usize = 10;

/// The values `output` accepts, as the command line's `--output` does
/// (case aside).
pub const OUTPUTS: [&str; 2] = ["text", "json"];

/// The names the output formats had until October 2026 (never published),
/// with what replaces each, for the error that rejects them.
const RENAMED_OUTPUTS: [(&str, &str); 2] = [
    ("human", "\"text\""),
    (
        "agent",
        "\"json\" at -v (output = \"json\", and run aye -v)",
    ),
];

/// `[tool.aye]` with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct AyeConfig {
    pub settings: AyeSettings,
    /// Where the table was read from; `Default` when there is none.
    pub source: ValueSource,
}

impl AyeConfig {
    /// Read `[tool.aye]` from `root/pyproject.toml`.
    pub fn load(root: &Path) -> Result<Self, ConfigError> {
        let path = root.join("pyproject.toml");
        if !path.is_file() {
            return Ok(Self {
                settings: AyeSettings::default(),
                source: ValueSource::Default,
            });
        }
        let Some(table) = toml_table(&path, &["tool", "aye"])? else {
            return Ok(Self {
                settings: AyeSettings::default(),
                source: ValueSource::Default,
            });
        };
        let mut settings: AyeSettings =
            toml::Value::Table(table)
                .try_into()
                .map_err(|e: toml::de::Error| ConfigError::Aye {
                    path: path.clone(),
                    message: e.message().to_string(),
                })?;
        if let Err(message) = settings.check() {
            return Err(ConfigError::Aye { path, message });
        }
        Ok(Self {
            settings,
            source: ValueSource::file(path, "[tool.aye]"),
        })
    }

    /// Every setting that is set, as `(name, value, source)`, for `aye
    /// config`.
    pub fn entries(&self) -> Vec<(String, String, ValueSource)> {
        let s = &self.settings;
        let mut out = Vec::new();
        let mut push = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                out.push((name.to_string(), value, self.source.clone()));
            }
        };
        push(
            "workers",
            s.workers.map(|w| match w {
                Workers::Count(n) => n.to_string(),
                Workers::Keyword(WorkersKeyword::Auto) => "auto".to_string(),
            }),
        );
        push("python", s.python.clone());
        push("output", s.output.clone());
        push("timeout", s.timeout.map(|t| t.to_string()));
        push("adapters", s.adapters.as_ref().map(|a| a.join(", ")));
        push("keep-runs", s.keep_runs.map(|keep| keep.to_string()));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn root_is_the_nearest_pyproject_then_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(find_root(&root.join("a/b")), root);
        write(root, "a/pyproject.toml", "[project]\nname='x'\n");
        assert_eq!(find_root(&root.join("a/b")), root.join("a"));
    }

    /// Finding 23: `..` in a path argument made the root `<root>/sub/..`.
    #[test]
    fn the_root_of_dotted_paths_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "pyproject.toml", "[project]\nname='x'\n");
        write(root, "other/test_o.py", "");
        write(root, "sub/test_s.py", "");
        let sub = root.join("sub");
        let find = |paths: &[&str]| {
            let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
            find_root(&aye_fs::common_ancestor(&sub, &paths))
        };
        assert_eq!(find(&[".."]), root);
        assert_eq!(find(&["../other"]), root);
        assert_eq!(find(&["../other/test_o.py"]), root);
        assert_eq!(find(&["../other", "test_s.py"]), root);
        assert_eq!(find(&["../sub/../other/test_o.py"]), root);
        assert_eq!(aye_fs::common_ancestor(&sub, &[PathBuf::from("..")]), root);
        assert_eq!(
            aye_fs::common_ancestor(&sub, &[PathBuf::from("../other/./test_o.py")]),
            root.join("other")
        );
        assert_eq!(find_root(&sub.join("..")), root);
        assert_eq!(find_root(&root.join("other/../sub/..")), root);
        assert_eq!(aye_fs::common_ancestor(&sub.join("../sub"), &[]), sub);
    }

    #[cfg(unix)]
    #[test]
    fn one_directory_spelled_two_ways_has_one_root() {
        let dir = tempfile::tempdir().unwrap();
        let real = fs::canonicalize(dir.path()).unwrap();
        write(&real, "proj/pyproject.toml", "");
        write(&real, "proj/tests/test_a.py", "");
        write(&real, "proj/tests/test_b.py", "");
        // A second spelling of `proj`, as macOS's /tmp is /private/tmp.
        let alias_dir = tempfile::tempdir().unwrap();
        let alias = alias_dir.path().join("alias");
        std::os::unix::fs::symlink(real.join("proj"), &alias).unwrap();
        let cwd = real.join("proj");
        // Whether the two spellings share only the filesystem's root (macOS)
        // or more (/tmp on Linux), the root is the project's.
        let paths = [
            alias.join("tests/test_a.py"),
            PathBuf::from("tests/test_b.py"),
        ];
        assert_eq!(project_root(&cwd, &paths), real.join("proj"));
        // A symlinked directory inside the project keeps its own name.
        std::os::unix::fs::symlink(real.join("proj/tests"), real.join("proj/linked")).unwrap();
        assert_eq!(
            project_root(&cwd, &[PathBuf::from("linked"), PathBuf::from("tests")]),
            real.join("proj")
        );
    }

    #[test]
    fn toml_tables_and_ini_sections() {
        let dir = tempfile::tempdir().unwrap();
        let toml_path = write(
            dir.path(),
            "pyproject.toml",
            "[tool.pytest.ini_options]\ntestpaths = [\"tests\"]\naddopts = \"-ra -q\"\n",
        );
        let table = toml_table(&toml_path, &["tool", "pytest", "ini_options"])
            .unwrap()
            .unwrap();
        assert_eq!(
            toml_strings(&table["testpaths"], split_list).unwrap(),
            ["tests"]
        );
        assert_eq!(
            toml_strings(&table["addopts"], split_args).unwrap(),
            ["-ra", "-q"]
        );
        assert!(toml_table(&toml_path, &["tool", "nope"]).unwrap().is_none());

        let ini_path = write(dir.path(), "tox.ini", "[pytest]\ntestpaths = a\n    b\n");
        let section = ini_section(&ini_path, "pytest").unwrap().unwrap();
        assert_eq!(split_list(&section["testpaths"]), ["a", "b"]);
        assert!(ini_section(&ini_path, "tool:pytest").unwrap().is_none());
    }

    #[test]
    fn find_upward_checks_names_in_order_per_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "setup.cfg", "");
        write(root, "a/tox.ini", "");
        fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(
            find_upward(&root.join("a/b"), &["pytest.ini", "tox.ini", "setup.cfg"]),
            Some(root.join("a/tox.ini"))
        );
    }

    #[test]
    fn tool_aye_with_its_source_and_strict_keys() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "pyproject.toml",
            "[tool.aye]\nworkers = \"auto\"\ntimeout = 5.0\n",
        );
        let config = AyeConfig::load(dir.path()).unwrap();
        assert_eq!(
            config.settings.workers,
            Some(Workers::Keyword(WorkersKeyword::Auto))
        );
        let entries = config.entries();
        assert_eq!(entries[0].0, "workers");
        assert_eq!(
            entries[0].2.display_row(dir.path()).to_string(),
            "pyproject.toml [tool.aye]"
        );

        write(dir.path(), "pyproject.toml", "[tool.aye]\nwrokers = 4\n");
        let err = AyeConfig::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("wrokers"), "{err}");
    }

    /// Finding 82: a bad `output` was ignored, a bad `workers` named serde's
    /// untagged enum.
    #[test]
    fn tool_aye_values_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let load = |body: &str| {
            write(
                dir.path(),
                "pyproject.toml",
                &format!("[tool.aye]\n{body}\n"),
            );
            AyeConfig::load(dir.path())
        };
        for output in ["text", "json"] {
            let config = load(&format!("output = {output:?}")).unwrap();
            assert_eq!(config.settings.output.as_deref(), Some(output));
        }
        // Any case, as `--output` takes it, kept in lower case.
        for (output, kept) in [("JSON", "json"), ("Text", "text"), ("tEXT", "text")] {
            let config = load(&format!("output = {output:?}")).unwrap();
            assert_eq!(config.settings.output.as_deref(), Some(kept));
            let entries = config.entries();
            assert_eq!(
                entries[0],
                (
                    "output".to_string(),
                    kept.to_string(),
                    config.source.clone()
                )
            );
        }
        let err = load("output = \"xml\"").unwrap_err().to_string();
        assert!(
            err.ends_with("output = \"xml\": expected text or json"),
            "{err}"
        );
        assert!(err.contains("pyproject.toml"), "{err}");
        // The formats' old names say what to use instead, in any case.
        for (output, says) in [
            (
                "human",
                "output = \"human\": expected text or json; \"human\" is now \"text\"",
            ),
            (
                "Human",
                "output = \"Human\": expected text or json; \"human\" is now \"text\"",
            ),
            (
                "agent",
                "output = \"agent\": expected text or json; \"agent\" is now \"json\" at -v \
                 (output = \"json\", and run aye -v)",
            ),
            (
                "AGENT",
                "output = \"AGENT\": expected text or json; \"agent\" is now \"json\" at -v \
                 (output = \"json\", and run aye -v)",
            ),
        ] {
            let err = load(&format!("output = {output:?}"))
                .unwrap_err()
                .to_string();
            assert!(err.ends_with(says), "{err}");
        }

        assert_eq!(
            load("workers = 4").unwrap().settings.workers,
            Some(Workers::Count(4))
        );
        // 0 loads as given; it means one worker, as `-n 0` does.
        assert_eq!(
            load("workers = 0").unwrap().settings.workers,
            Some(Workers::Count(0))
        );
        assert_eq!(
            load("workers = \"auto\"").unwrap().settings.workers,
            Some(Workers::Keyword(WorkersKeyword::Auto))
        );
        for (bad, shown) in [
            ("\"lots\"", "workers = \"lots\""),
            ("-1", "workers = -1"),
            ("2.5", "invalid type"),
            ("99999999999", "workers = 99999999999: too many"),
        ] {
            let err = load(&format!("workers = {bad}")).unwrap_err().to_string();
            assert!(err.contains(shown), "{bad}: {err}");
            assert!(!err.contains("untagged"), "{bad}: {err}");
            if bad != "99999999999" {
                assert!(
                    err.contains("a number of workers or \"auto\""),
                    "{bad}: {err}"
                );
            }
        }
    }

    /// Finding C34: `timeout`, `python` and `adapters` took any value.
    #[test]
    fn every_tool_aye_value_is_checked() {
        let dir = tempfile::tempdir().unwrap();
        let load = |body: &str| {
            write(
                dir.path(),
                "pyproject.toml",
                &format!("[tool.aye]\n{body}\n"),
            );
            AyeConfig::load(dir.path())
        };
        for (bad, shown) in [
            (
                "timeout = -5",
                "timeout = -5: expected a number of seconds, 0 or more",
            ),
            ("timeout = nan", "timeout = NaN"),
            ("timeout = inf", "timeout = inf"),
            ("timeout = \"5\"", "invalid type"),
            ("python = \"\"", "python = \"\": expected the path or name"),
            ("python = \"  \"", "python = \"  \""),
            ("python = 3", "invalid type"),
            ("adapters = []", "adapters = []: list at least one adapter"),
            (
                "adapters = [\"pytest\", \"pytest\"]",
                "\"pytest\" is listed twice",
            ),
            ("adapters = [\"\"]", "\"\" is not an adapter's name"),
            ("adapters = \"pytest\"", "invalid type"),
            ("unittest = 1", "invalid type"),
            ("doctest = \"x\"", "invalid type"),
            (
                "keep-runs = -1",
                "keep-runs = -1: expected a number of runs, 0 or more",
            ),
            ("keep-runs = 2.5", "invalid type"),
            ("keep-runs = \"10\"", "invalid type"),
            ("keep_runs = 3", "keep_runs"),
        ] {
            let err = load(bad).unwrap_err().to_string();
            assert!(err.contains(shown), "{bad}: {err}");
            assert!(err.starts_with("invalid [tool.aye] in "), "{bad}: {err}");
        }
        let config = load("timeout = 0\npython = \"python3\"\nadapters = [\"unittest\"]").unwrap();
        assert_eq!(config.settings.timeout, Some(0.0));
        assert_eq!(config.settings.timeout(), None);
        assert_eq!(load("timeout = 5").unwrap().settings.timeout(), Some(5.0));
        assert_eq!(load("timeout = 2.5").unwrap().settings.timeout(), Some(2.5));
        assert_eq!(load("").unwrap().settings.keep_runs(), KEEP_RUNS);
        assert_eq!(load("keep-runs = 0").unwrap().settings.keep_runs(), 0);
        let config = load("keep-runs = 3").unwrap();
        assert_eq!(config.settings.keep_runs(), 3);
        assert_eq!(
            config.entries()[0],
            (
                "keep-runs".to_string(),
                "3".to_string(),
                config.source.clone()
            )
        );
        let config = load("[tool.aye.unittest]\npattern = \"*_test.py\"").unwrap();
        assert!(config.settings.adapter_table("unittest").is_some());
        assert!(config.settings.adapter_table("doctest").is_none());
        assert!(config.settings.adapter_table("pytest").is_none());
    }

    /// Every source as `aye config`'s rows show it, and in full, as 0.0.1
    /// spelt each (`aye config`'s snapshots pin them in place).
    #[test]
    fn a_source_is_shown_as_aye_config_shows_it() {
        let root = Path::new("/repo");
        let shown =
            |source: ValueSource| (source.display_row(root).to_string(), source.to_string());
        let both = |text: &str| (text.to_string(), text.to_string());
        assert_eq!(shown(ValueSource::Default), both("default"));
        assert_eq!(shown(ValueSource::Cli("--")), both("--"));
        assert_eq!(
            shown(ValueSource::Env("PYTEST_ADDOPTS")),
            both("$PYTEST_ADDOPTS")
        );
        assert_eq!(
            shown(ValueSource::file("/repo/pyproject.toml", "[tool.aye]")),
            (
                "pyproject.toml [tool.aye]".to_string(),
                "/repo/pyproject.toml [tool.aye]".to_string()
            )
        );
        assert_eq!(
            shown(ValueSource::file("/elsewhere/tox.ini", "[pytest]")).0,
            "/elsewhere/tox.ini [pytest]"
        );
        // A section with no name keeps its space, as 0.0.1's did.
        assert_eq!(
            shown(ValueSource::file("/repo/pyproject.toml", "")).0,
            "pyproject.toml "
        );
        assert_eq!(
            shown(ValueSource::whole_file("/repo/pytest.ini")),
            ("pytest.ini".to_string(), "/repo/pytest.ini".to_string())
        );
        assert_eq!(
            shown(ValueSource::FrameworkOption {
                option: "-o python_classes".into(),
                via: "PYTEST_ADDOPTS",
            }),
            both("-o python_classes (PYTEST_ADDOPTS)")
        );
        assert_eq!(
            shown(ValueSource::FrameworkOption {
                option: "--strict".into(),
                via: "after --",
            }),
            both("--strict (after --)")
        );
        assert_eq!(
            shown(ValueSource::Rule(
                "the doctest adapter isn't among the run's adapters"
            )),
            both("the doctest adapter isn't among the run's adapters")
        );
    }

    /// Where a value came from changes nothing it does.
    #[test]
    fn sourced_values_are_equal_by_their_values() {
        let from_file = Sourced::new(3, ValueSource::file("/repo/pyproject.toml", "[tool.aye]"));
        assert_eq!(from_file, Sourced::default_value(3));
        assert_ne!(from_file, Sourced::default_value(4));
    }

    #[test]
    fn missing_tool_aye_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = AyeConfig::load(dir.path()).unwrap();
        assert_eq!(config.source, ValueSource::Default);
        assert_eq!(
            config.entries(),
            [] as [(std::string::String, std::string::String, ValueSource); 0]
        );
    }
}
