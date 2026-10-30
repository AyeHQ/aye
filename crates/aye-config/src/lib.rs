//! Configuration as a mechanism. Finds the project root, reads
//! `[tool.aye]`, and provides generic loaders (toml tables, ini sections, the
//! upward search) that engines use to read their own framework's settings. Every
//! resolved value remembers where it came from ([`ValueSource`]) for `aye
//! config`, which shows a source in a table's row (`pyproject.toml
//! [tool.aye]`) or beside a setting's name on a header line (`[tool.aye]
//! output`).
//!
//! `[tool.aye]` is read as written, every key optional and each value as
//! TOML gives it ([`AyeOptions`]), then resolved ([`AyeSettings::resolve`])
//! into values of types that hold only what each setting accepts
//! ([`WorkerCount`], [`OutputFormat`], [`Timeout`], [`KeepRuns`],
//! [`AdapterList`]), each [`Sourced`] from the table. The command line
//! shares those types where it takes the same value, each surface with its
//! own words.
//!
//! Owns: loading and value sources, and the values `[tool.aye]` holds. Does
//! not own: the meaning of any framework's options; `python_files` means
//! something to the pytest engine, not here.
//!
//! Depends on: aye-fs, for the lexical path algebra the root is found
//! with, and aye-events, for the adapters' names.

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
//!   `text` or `json`, in any case as on the command line (`human` and
//!   `agent`, the formats' names until October 2026, are errors that say
//!   what to use instead), and `workers` a number or `"auto"`. `workers = 0`
//!   is one worker, as `-n 0` is on the command line and pytest-xdist's `-n
//!   0` runs the tests in one process. `python` must not be blank, `timeout`
//!   must be a finite number of seconds, 0 or more (0 is none, as
//!   pytest-timeout reads it), and `adapters` must list at least one name,
//!   none twice; which names exist is the binary's to check, and each
//!   adapter checks its own table (`[tool.aye.unittest]`,
//!   `[tool.aye.doctest]`), even when it doesn't run. `keep-runs` (how many
//!   runs' logs `.aye/runs` keeps for `aye show`, 10 unless set) must be a
//!   whole number, 0 or more: 0 keeps none. With several bad values the
//!   error names one: an unknown key or a value of the wrong type first,
//!   then the first bad one in that list.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub mod ini;
mod options;
mod settings;
mod source;
mod values;

pub use options::AyeOptions;
pub use settings::AyeSettings;
pub use source::{Sourced, ValueSource};
pub use values::{AdapterList, InvalidAdapterList, KeepRuns, OutputFormat, Timeout, WorkerCount};

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
}
