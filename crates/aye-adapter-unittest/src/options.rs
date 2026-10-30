//! `[tool.aye.unittest]` as written ([`UnittestOptions`]): where discovery
//! starts, which files it takes and where imports start, as `python -m
//! unittest discover -s -p -t` take them.

use std::path::Path;

use aye_adapter::AdapterError;
use aye_config::{AyeSettings, Sourced, ValueSource};

/// The table's keys.
pub(crate) const START_DIRECTORY: &str = "start-directory";
pub(crate) const PATTERN: &str = "pattern";
pub(crate) const TOP_LEVEL_DIRECTORY: &str = "top-level-directory";

/// unittest's defaults.
pub(crate) const DEFAULT_START: &str = ".";
pub(crate) const DEFAULT_PATTERN: &str = "test*.py";

/// `[tool.aye.unittest]` as written, each value with where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnittestOptions {
    /// `start-directory`, relative to the project root.
    pub(crate) start_directory: Sourced<String>,
    /// `pattern`, never empty.
    pub(crate) pattern: Sourced<String>,
    /// `top-level-directory`; `None` when it defaults to the start
    /// directory.
    pub(crate) top_level_directory: Sourced<Option<String>>,
}

impl UnittestOptions {
    /// Read the table from `tool_aye`, `[tool.aye]` as the binary loaded it
    /// from `path`. Unknown keys, values that aren't strings and an empty
    /// pattern are errors.
    pub(crate) fn read(path: &Path, tool_aye: &AyeSettings) -> Result<Self, AdapterError> {
        let source = ValueSource::file(path, "[tool.aye.unittest]");
        let shown = || invalid(path);
        let mut start_directory = Sourced::default_value(DEFAULT_START.to_string());
        let mut pattern = Sourced::default_value(DEFAULT_PATTERN.to_string());
        let mut top_level_directory = Sourced::default_value(None);
        for (key, value) in tool_aye.unittest.iter().flatten() {
            let Some(text) = value.as_str() else {
                return Err(AdapterError::Config(format!(
                    "{}: {key} must be a string, not {value}",
                    shown()
                )));
            };
            match key.as_str() {
                START_DIRECTORY => start_directory = Sourced::new(text.to_string(), source.clone()),
                PATTERN => pattern = Sourced::new(text.to_string(), source.clone()),
                TOP_LEVEL_DIRECTORY => {
                    top_level_directory = Sourced::new(Some(text.to_string()), source.clone());
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
        Ok(Self {
            start_directory,
            pattern,
            top_level_directory,
        })
    }
}

/// How every error in the table, or in what it resolves to, begins.
pub(crate) fn invalid(path: &Path) -> String {
    format!("invalid [tool.aye.unittest] in {}", path.display())
}
