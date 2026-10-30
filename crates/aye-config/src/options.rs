//! `[tool.aye]` as written ([`AyeOptions`]).

use std::path::Path;

use serde::Deserialize;

use crate::values::WorkerCount;
use crate::{ConfigError, toml_table};

/// `[tool.aye]` as written: every key optional, each value as TOML gives
/// it. A key aye doesn't know, or a value of the wrong type, is an error
/// here; whether a value is one the setting accepts is
/// [`crate::AyeSettings::resolve`]'s to say. The keys are in the order an
/// unknown one's error lists them.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct AyeOptions {
    /// Worker processes: a number, or `"auto"` for one per CPU.
    pub workers: Option<WorkerCount>,
    /// The interpreter that runs the tests.
    pub python: Option<String>,
    /// Default output format: `text` or `json`, in any case.
    pub output: Option<String>,
    /// Per-test timeout in seconds.
    pub timeout: Option<f64>,
    /// Which adapters run, in priority order (`["pytest", "doctest"]`).
    pub adapters: Option<Vec<String>>,
    /// How many runs' logs `.aye/runs` keeps, for `aye show`.
    pub keep_runs: Option<i64>,
    /// `[tool.aye.unittest]`: the unittest adapter's own settings, read by
    /// that adapter.
    pub unittest: Option<toml::Table>,
    /// `[tool.aye.doctest]`: the doctest adapter's own settings, read by
    /// that adapter.
    pub doctest: Option<toml::Table>,
}

impl AyeOptions {
    /// The `[tool.aye]` table of the `pyproject.toml` at `path`, or `None`
    /// when it has none.
    pub fn read(path: &Path) -> Result<Option<Self>, ConfigError> {
        let Some(table) = toml_table(path, &["tool", "aye"])? else {
            return Ok(None);
        };
        toml::Value::Table(table)
            .try_into()
            .map(Some)
            .map_err(|e: toml::de::Error| ConfigError::Aye {
                path: path.to_path_buf(),
                message: e.message().to_string(),
            })
    }
}
