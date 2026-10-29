//! Configuration as a mechanism. Finds the project root, reads
//! `[tool.aye]`, and provides generic loaders (toml tables, ini sections, the
//! upward search) that engines use to read their own framework's settings. Every
//! resolved value remembers its origin for `aye config`.
//!
//! Owns: loading and origins. Does not own: the meaning of any framework's
//! options; `python_files` means something to the pytest engine, not here.
//!
//! Depends on: nothing from aye yet (loaders are pure).

//!
//! Decided (implementation, October 2026):
//! - **The project root** is the nearest directory, from the common ancestor
//!   of the paths given (or the current directory) upward, holding a
//!   `pyproject.toml`; failing that, one holding `.git`; failing that, the
//!   starting directory. Test ids are relative to it. Frameworks keep their
//!   own notion of a root (pytest's rootdir) and translate.
//! - **Paths are made absolute lexically**, as Python's `os.path.abspath`
//!   does and pytest relies on: joined to the current directory, with `.`
//!   and `..` folded away ([`absolute`]), never resolved through symlinks, so
//!   a symlinked test directory keeps its name. The filesystem is consulted
//!   only to reconcile two spellings of one directory (macOS's `/tmp` is
//!   `/private/tmp`, which is what the current directory reads as): when the
//!   given directories share nothing lexically but do once resolved, their
//!   common ancestor is the resolved one, and [`relative_to`] falls back to
//!   resolved paths the same way. Where the two spellings share more than
//!   the filesystem's root (both under Linux's `/tmp`), the common ancestor
//!   stays lexical, as pytest's `get_common_ancestor` keeps it, but when no
//!   project holds it the root is searched for from the resolved one
//!   ([`project_root`]), so one command finds one root on every platform.
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
use std::path::{Component, Path, PathBuf};

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

/// Where a resolved value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Built-in default.
    Default,
    /// A command-line flag, e.g. `--workers`.
    Flag(String),
    /// An environment variable.
    Env(String),
    /// A file, and where in it (`[tool.aye]`, `[pytest]`).
    File { path: PathBuf, location: String },
}

impl Origin {
    pub fn file(path: impl Into<PathBuf>, location: impl Into<String>) -> Self {
        Self::File {
            path: path.into(),
            location: location.into(),
        }
    }

    /// The origin with its file shown relative to `root` when inside it.
    pub fn display_relative(&self, root: &Path) -> String {
        match self {
            Self::File { path, location } => {
                let shown = path.strip_prefix(root).unwrap_or(path);
                format!("{} {location}", shown.display())
            }
            other => other.to_string(),
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::Flag(flag) => write!(f, "{flag}"),
            Self::Env(var) => write!(f, "${var}"),
            Self::File { path, location } => write!(f, "{} {location}", path.display()),
        }
    }
}

/// A value and its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub origin: Origin,
}

impl<T> Sourced<T> {
    pub const fn new(value: T, origin: Origin) -> Self {
        Self { value, origin }
    }

    pub fn default_value(value: T) -> Self {
        Self::new(value, Origin::Default)
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

/// `path` without `.` and `..` components, folded lexically as Python's
/// `os.path.normpath` does: `..` at the top of an absolute path stays at the
/// top, and leading `..` of a relative path are kept.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `path` made absolute against `cwd` and normalized, as Python's
/// `os.path.abspath` does: no filesystem access, symlinks untouched.
pub fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    normalize(&cwd.join(path))
}

/// `path` relative to `root` (both absolute): lexically when `path` is
/// spelled under `root`, otherwise through both resolved on the filesystem
/// (the deepest existing ancestor of `path`, then the rest as written), so
/// `/tmp/x/t.py` is under `/private/tmp/x`. `None` when it is not under
/// `root` either way.
pub fn relative_to(root: &Path, path: &Path) -> Option<PathBuf> {
    let (root, path) = (normalize(root), normalize(path));
    if let Ok(rest) = path.strip_prefix(&root) {
        return Some(rest.to_path_buf());
    }
    let root = fs::canonicalize(&root).ok()?;
    resolve_existing(&path)?
        .strip_prefix(&root)
        .ok()
        .map(Path::to_path_buf)
}

/// `path` with its deepest existing ancestor resolved on the filesystem and
/// the rest appended as written; `None` when nothing of it exists.
fn resolve_existing(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    for ancestor in path.ancestors() {
        if let Ok(resolved) = fs::canonicalize(ancestor) {
            return Some(rest.iter().rev().fold(resolved, |acc, part| acc.join(part)));
        }
        rest.push(ancestor.file_name()?);
    }
    None
}

/// The deepest directory containing every existing path in `paths` (files
/// count as their parent directory), or `cwd` when none exist. Paths are
/// relative to `cwd` or absolute, and are normalized lexically first (see
/// the crate docs); so is the result.
pub fn common_ancestor(cwd: &Path, paths: &[PathBuf]) -> PathBuf {
    let cwd = normalize(cwd);
    let dirs = existing_dirs(&cwd, paths);
    let Some(lexical) = deepest_common(&dirs) else {
        return cwd;
    };
    if lexical.parent().is_none() && dirs.len() > 1 {
        // Nothing in common but the filesystem's root: perhaps one directory
        // spelled two ways.
        if let Some(common) = resolved_common(&dirs)
            && common.parent().is_some()
        {
            return common;
        }
    }
    lexical
}

/// The existing paths of `paths` made absolute against `cwd` (already
/// normalized), files as their parent directory.
fn existing_dirs(cwd: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| absolute(cwd, p))
        .filter(|p| p.exists())
        .map(|p| {
            if p.is_file() {
                p.parent().map_or(p.clone(), Path::to_path_buf)
            } else {
                p
            }
        })
        .collect()
}

/// The deepest common ancestor of `dirs` resolved on the filesystem;
/// `None` when one of them does not resolve.
fn resolved_common(dirs: &[PathBuf]) -> Option<PathBuf> {
    let resolved: Option<Vec<PathBuf>> = dirs.iter().map(|d| fs::canonicalize(d).ok()).collect();
    deepest_common(&resolved?)
}

/// The deepest common ancestor of absolute paths, by whole components.
fn deepest_common(dirs: &[PathBuf]) -> Option<PathBuf> {
    let (first, rest) = dirs.split_first()?;
    Some(rest.iter().fold(first.clone(), |acc, dir| {
        acc.ancestors()
            .find(|a| dir.starts_with(a))
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
    }))
}

/// aye's project root: see the crate docs. `start` is normalized lexically
/// first, so `<root>/tests/..` gives `<root>`.
pub fn find_root(start: &Path) -> PathBuf {
    let start = normalize(start);
    marked(&start).unwrap_or(start)
}

/// aye's project root for `paths` given in `cwd`: [`find_root`] from their
/// [`common_ancestor`]. When nothing above that ancestor marks a project,
/// but the paths, resolved, share a directory inside it (one directory
/// spelled two ways under a shared `/tmp`, as on Linux), the search starts
/// from that directory instead, so the root doesn't depend on how much two
/// spellings happen to share.
pub fn project_root(cwd: &Path, paths: &[PathBuf]) -> PathBuf {
    let ancestor = normalize(&common_ancestor(cwd, paths));
    if let Some(root) = marked(&ancestor) {
        return root;
    }
    let dirs = existing_dirs(&normalize(cwd), paths);
    if dirs.len() > 1
        && let Some(resolved) = resolved_common(&dirs)
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

/// `[tool.aye]` with its origin.
#[derive(Debug, Clone, PartialEq)]
pub struct AyeConfig {
    pub settings: AyeSettings,
    /// Where the table was read from; `Default` when there is none.
    pub origin: Origin,
}

impl AyeConfig {
    /// Read `[tool.aye]` from `root/pyproject.toml`.
    pub fn load(root: &Path) -> Result<Self, ConfigError> {
        let path = root.join("pyproject.toml");
        if !path.is_file() {
            return Ok(Self {
                settings: AyeSettings::default(),
                origin: Origin::Default,
            });
        }
        let Some(table) = toml_table(&path, &["tool", "aye"])? else {
            return Ok(Self {
                settings: AyeSettings::default(),
                origin: Origin::Default,
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
            origin: Origin::file(path, "[tool.aye]"),
        })
    }

    /// Every setting that is set, as `(name, value, origin)`, for `aye config`.
    pub fn entries(&self) -> Vec<(String, String, Origin)> {
        let s = &self.settings;
        let mut out = Vec::new();
        let mut push = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                out.push((name.to_string(), value, self.origin.clone()));
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

    #[test]
    fn common_ancestor_of_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a/b/t1.py", "");
        write(root, "a/c/t2.py", "");
        let paths = [PathBuf::from("a/b/t1.py"), PathBuf::from("a/c")];
        assert_eq!(common_ancestor(root, &paths), root.join("a"));
        assert_eq!(common_ancestor(root, &[PathBuf::from("missing")]), root);
    }

    #[test]
    fn normalize_folds_dots_lexically() {
        let cases = [
            ("/a/b/../c", "/a/c"),
            ("/a/./b/", "/a/b"),
            ("/a/b/../../..", "/"),
            ("/..", "/"),
            ("a/../..", ".."),
            ("../a/./b", "../a/b"),
            ("a/..", "."),
            ("", "."),
        ];
        for (path, expected) in cases {
            assert_eq!(normalize(Path::new(path)), Path::new(expected), "{path}");
        }
        assert_eq!(
            absolute(Path::new("/r/sub"), Path::new("../other")),
            Path::new("/r/other")
        );
        assert_eq!(
            absolute(Path::new("/r/sub"), Path::new("/x/./y/..")),
            Path::new("/x")
        );
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
            find_root(&common_ancestor(&sub, &paths))
        };
        assert_eq!(find(&[".."]), root);
        assert_eq!(find(&["../other"]), root);
        assert_eq!(find(&["../other/test_o.py"]), root);
        assert_eq!(find(&["../other", "test_s.py"]), root);
        assert_eq!(find(&["../sub/../other/test_o.py"]), root);
        assert_eq!(common_ancestor(&sub, &[PathBuf::from("..")]), root);
        assert_eq!(
            common_ancestor(&sub, &[PathBuf::from("../other/./test_o.py")]),
            root.join("other")
        );
        assert_eq!(find_root(&sub.join("..")), root);
        assert_eq!(find_root(&root.join("other/../sub/..")), root);
        assert_eq!(common_ancestor(&sub.join("../sub"), &[]), sub);
    }

    #[cfg(unix)]
    #[test]
    fn one_directory_spelled_two_ways_is_one_ancestor() {
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
        let paths = [
            alias.join("tests/test_a.py"),
            PathBuf::from("tests/test_b.py"),
        ];
        // The two spellings are resolved only when they share nothing but
        // the filesystem's root (macOS's /private/var against /var); where
        // both temporary directories share more (/tmp on Linux), that
        // ancestor is kept, as pytest's own `get_common_ancestor` does.
        let lexical =
            super::deepest_common(&[alias.join("tests"), real.join("proj/tests")]).unwrap();
        if lexical.parent().is_none() {
            assert_eq!(common_ancestor(&cwd, &paths), real.join("proj/tests"));
            assert_eq!(find_root(&common_ancestor(&cwd, &paths)), real.join("proj"));
        } else {
            assert_eq!(common_ancestor(&cwd, &paths), lexical);
        }
        // Either way, the root is the project's.
        assert_eq!(project_root(&cwd, &paths), real.join("proj"));
        // One spelling alone is kept as given, symlinks unresolved.
        assert_eq!(
            common_ancestor(&cwd, &[alias.join("tests")]),
            alias.join("tests")
        );
        // A symlinked directory inside the project keeps its own name.
        std::os::unix::fs::symlink(real.join("proj/tests"), real.join("proj/linked")).unwrap();
        assert_eq!(
            common_ancestor(&cwd, &[PathBuf::from("linked"), PathBuf::from("tests")]),
            real.join("proj")
        );
        assert_eq!(
            project_root(&cwd, &[PathBuf::from("linked"), PathBuf::from("tests")]),
            real.join("proj")
        );

        let root = real.join("proj");
        assert_eq!(
            relative_to(&root, &alias.join("tests/test_a.py")),
            Some(PathBuf::from("tests/test_a.py"))
        );
        assert_eq!(
            relative_to(&root, &alias.join("tests/missing.py")),
            Some(PathBuf::from("tests/missing.py"))
        );
        assert_eq!(
            relative_to(&root, &root.join("linked/test_a.py")),
            Some(PathBuf::from("linked/test_a.py"))
        );
        assert_eq!(relative_to(&root, &real), None);
        assert_eq!(relative_to(&root, &root), Some(PathBuf::new()));
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
    fn tool_aye_with_origin_and_strict_keys() {
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
            entries[0].2.display_relative(dir.path()),
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
                    config.origin.clone()
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
                config.origin.clone()
            )
        );
        let config = load("[tool.aye.unittest]\npattern = \"*_test.py\"").unwrap();
        assert!(config.settings.adapter_table("unittest").is_some());
        assert!(config.settings.adapter_table("doctest").is_none());
        assert!(config.settings.adapter_table("pytest").is_none());
    }

    #[test]
    fn missing_tool_aye_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let config = AyeConfig::load(dir.path()).unwrap();
        assert_eq!(config.origin, Origin::Default);
        assert_eq!(
            config.entries(),
            [] as [(std::string::String, std::string::String, Origin); 0]
        );
    }
}
