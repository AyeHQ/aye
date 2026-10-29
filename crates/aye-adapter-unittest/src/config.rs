//! `[tool.aye.unittest]`: where discovery starts, which files it takes and
//! where imports start, as `python -m unittest discover -s -p -t` take them.

use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Setting};
use aye_config::{Origin, Sourced};

/// The table's keys.
pub(crate) const START_DIRECTORY: &str = "start-directory";
pub(crate) const PATTERN: &str = "pattern";
pub(crate) const TOP_LEVEL_DIRECTORY: &str = "top-level-directory";

/// unittest's defaults.
pub(crate) const DEFAULT_START: &str = ".";
pub(crate) const DEFAULT_PATTERN: &str = "test*.py";

/// The unittest adapter's settings, resolved against the project root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnittestConfig {
    /// `start-directory`, as written (relative to the project root).
    pub start_directory: Sourced<String>,
    /// `pattern`: `fnmatch`ed against file names.
    pub pattern: Sourced<String>,
    /// `top-level-directory`, as written; `None` when it defaults to the
    /// start directory.
    pub top_level_directory: Sourced<Option<String>>,
    /// The start directory, absolute.
    pub start: PathBuf,
    /// The top-level directory, absolute: modules are imported by their
    /// dotted names from it.
    pub top: PathBuf,
}

impl UnittestConfig {
    /// Read `[tool.aye.unittest]` from `root/pyproject.toml`. Unknown keys,
    /// values that aren't strings, a start or top-level directory that
    /// isn't one, a start directory outside the top-level directory
    /// (unittest's "Path must be within the project"), and one below it
    /// that is no package (unittest's "Start directory is not importable")
    /// are errors.
    pub fn load(root: &Path) -> Result<Self, AdapterError> {
        let aye =
            aye_config::AyeConfig::load(root).map_err(|e| AdapterError::Config(e.to_string()))?;
        let path = root.join("pyproject.toml");
        let origin = Origin::file(&path, "[tool.aye.unittest]");
        let shown = || format!("invalid [tool.aye.unittest] in {}", path.display());
        let mut start_directory = Sourced::default_value(DEFAULT_START.to_string());
        let mut pattern = Sourced::default_value(DEFAULT_PATTERN.to_string());
        let mut top_level_directory = Sourced::default_value(None);
        for (key, value) in aye.settings.unittest.iter().flatten() {
            let Some(text) = value.as_str() else {
                return Err(AdapterError::Config(format!(
                    "{}: {key} must be a string, not {value}",
                    shown()
                )));
            };
            match key.as_str() {
                START_DIRECTORY => start_directory = Sourced::new(text.to_string(), origin.clone()),
                PATTERN => pattern = Sourced::new(text.to_string(), origin.clone()),
                TOP_LEVEL_DIRECTORY => {
                    top_level_directory = Sourced::new(Some(text.to_string()), origin.clone());
                }
                other => {
                    return Err(AdapterError::Config(format!(
                        "{}: unknown key {other:?} (expected {START_DIRECTORY}, {PATTERN} or \
                         {TOP_LEVEL_DIRECTORY})",
                        shown()
                    )));
                }
            }
        }
        if pattern.value.is_empty() {
            return Err(AdapterError::Config(format!(
                "{}: {PATTERN} is empty",
                shown()
            )));
        }
        let start = aye_fs::absolute(root, Path::new(&start_directory.value));
        let top = top_level_directory.value.as_deref().map_or_else(
            || start.clone(),
            |top| aye_fs::absolute(root, Path::new(top)),
        );
        for (key, dir) in [(START_DIRECTORY, &start), (TOP_LEVEL_DIRECTORY, &top)] {
            if !dir.is_dir() {
                return Err(AdapterError::Config(format!(
                    "{}: {key} {} is not a directory",
                    shown(),
                    dir.display()
                )));
            }
        }
        if !start.starts_with(&top) {
            return Err(AdapterError::Config(format!(
                "{}: the start directory {} is outside the top-level directory {}",
                shown(),
                start.display(),
                top.display()
            )));
        }
        // `TestLoader.discover`: below the top-level directory, the start
        // directory is imported as a package.
        if start != top && !start.join("__init__.py").is_file() {
            return Err(AdapterError::Config(format!(
                "{}: the start directory {} is below the top-level directory {} but is no \
                 package (no __init__.py), so unittest can't import it (\"Start directory is \
                 not importable\")",
                shown(),
                start.display(),
                top.display()
            )));
        }
        Ok(Self {
            start_directory,
            pattern,
            top_level_directory,
            start,
            top,
        })
    }

    /// The settings, with their origins, for `aye config`.
    pub fn settings(&self, root: &Path) -> Vec<Setting> {
        let top = self
            .top_level_directory
            .value
            .clone()
            .unwrap_or_else(|| self.start_directory.value.clone());
        vec![
            Setting {
                name: START_DIRECTORY.into(),
                value: self.start_directory.value.clone(),
                origin: self.start_directory.origin.display_relative(root),
            },
            Setting {
                name: PATTERN.into(),
                value: self.pattern.value.clone(),
                origin: self.pattern.origin.display_relative(root),
            },
            Setting {
                name: TOP_LEVEL_DIRECTORY.into(),
                value: top,
                origin: self.top_level_directory.origin.display_relative(root),
            },
        ]
    }
}
