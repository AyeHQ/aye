//! The unittest adapter's settings ([`UnittestConfig`]): `[tool.aye.unittest]`
//! resolved against the project root, and shown with where each value came
//! from.

use std::path::{Path, PathBuf};

use aye_adapter::{AdapterError, Setting};
use aye_config::{AyeSettings, Sourced};
use aye_python_stdlib::os_path;

use crate::options::{PATTERN, START_DIRECTORY, TOP_LEVEL_DIRECTORY, UnittestOptions, invalid};

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
    /// Read `[tool.aye.unittest]` from `tool_aye`, `[tool.aye]` as the
    /// binary loaded it from `root/pyproject.toml`, and resolve it. Unknown
    /// keys, values that aren't strings, an empty pattern, a start or
    /// top-level directory that isn't one, a start directory outside the
    /// top-level directory (unittest's "Path must be within the project"),
    /// and one below it that is no package (unittest's "Start directory is
    /// not importable") are errors.
    pub fn load(root: &Path, tool_aye: &AyeSettings) -> Result<Self, AdapterError> {
        let path = root.join("pyproject.toml");
        let shown = || invalid(&path);
        let UnittestOptions {
            start_directory,
            pattern,
            top_level_directory,
        } = UnittestOptions::read(&path, tool_aye)?;
        let start = os_path::abspath(root, Path::new(&start_directory.value));
        let top = top_level_directory.value.as_deref().map_or_else(
            || start.clone(),
            |top| os_path::abspath(root, Path::new(top)),
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

    /// The settings, with where each came from, for `aye config`.
    pub fn settings(&self) -> Vec<Setting> {
        let top = self
            .top_level_directory
            .value
            .clone()
            .unwrap_or_else(|| self.start_directory.value.clone());
        vec![
            Setting {
                name: START_DIRECTORY.into(),
                value: self.start_directory.value.clone(),
                source: self.start_directory.source.clone(),
            },
            Setting {
                name: PATTERN.into(),
                value: self.pattern.value.clone(),
                source: self.pattern.source.clone(),
            },
            Setting {
                name: TOP_LEVEL_DIRECTORY.into(),
                value: top,
                source: self.top_level_directory.source.clone(),
            },
        ]
    }
}
