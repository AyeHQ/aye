//! `[tool.aye.unittest]`: where discovery starts, which files it takes and
//! where imports start, as `python -m unittest discover -s -p -t` take them.

/// The table's keys.
pub(crate) const START_DIRECTORY: &str = "start-directory";
pub(crate) const PATTERN: &str = "pattern";
pub(crate) const TOP_LEVEL_DIRECTORY: &str = "top-level-directory";

/// unittest's defaults.
pub(crate) const DEFAULT_START: &str = ".";
pub(crate) const DEFAULT_PATTERN: &str = "test*.py";
