//! Where a resolved value came from ([`ValueSource`]), and a value with
//! its source ([`Sourced`]).

use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

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

    /// The source of the setting `name` as a header line of `aye config`
    /// shows it (`[tool.aye] output`): a file by its table and the key,
    /// since the line names no file; any other source in full.
    pub fn display_for<'a>(&'a self, name: &'a str) -> impl fmt::Display + 'a {
        Header { source: self, name }
    }
}

/// [`ValueSource::display_for`].
struct Header<'a> {
    source: &'a ValueSource,
    name: &'a str,
}

impl fmt::Display for Header<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.source {
            ValueSource::File {
                table: Some(table), ..
            } => write!(f, "{table} {}", self.name),
            ValueSource::File { path, table: None } => {
                write!(f, "{} {}", path.display(), self.name)
            }
            other => other.fmt(f),
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Every source as `aye config`'s header lines show it, beside the
    /// setting's name, as 0.0.1 spelt the ones it shows (`aye config`'s
    /// snapshots and `aye_config_says_where_the_interpreter_came_from` pin
    /// them in place).
    #[test]
    fn a_source_is_shown_as_a_header_shows_it() {
        let shown = |source: ValueSource, name: &str| source.display_for(name).to_string();
        assert_eq!(
            shown(
                ValueSource::file("/repo/pyproject.toml", "[tool.aye]"),
                "output"
            ),
            "[tool.aye] output"
        );
        assert_eq!(
            shown(
                ValueSource::file("/repo/pyproject.toml", "[tool.aye]"),
                "python"
            ),
            "[tool.aye] python"
        );
        assert_eq!(
            shown(ValueSource::whole_file("/repo/pytest.ini"), "addopts"),
            "/repo/pytest.ini addopts"
        );
        assert_eq!(shown(ValueSource::Default, "output"), "default");
        assert_eq!(shown(ValueSource::Cli("--output"), "output"), "--output");
        assert_eq!(
            shown(ValueSource::Env("AYE_PYTHON"), "python"),
            "$AYE_PYTHON"
        );
        assert_eq!(shown(ValueSource::Rule(".venv"), "python"), ".venv");
        assert_eq!(
            shown(
                ValueSource::FrameworkOption {
                    option: "-o python_classes".into(),
                    via: "addopts",
                },
                "python_classes"
            ),
            "-o python_classes (addopts)"
        );
    }

    /// Where a value came from changes nothing it does.
    #[test]
    fn sourced_values_are_equal_by_their_values() {
        let from_file = Sourced::new(3, ValueSource::file("/repo/pyproject.toml", "[tool.aye]"));
        assert_eq!(from_file, Sourced::default_value(3));
        assert_ne!(from_file, Sourced::default_value(4));
    }
}
