//! The raw values of pytest's configuration, as JSON, for the front-end's
//! `getini` to coerce by each option's type: what the pytest adapter's
//! native context's `ini` holds. They start from the configuration file's
//! values, as discovery read them ([`PytestConfig::file_ini`]), then
//! [`apply_overrides`] applies `-o` and the `--strict…` flags of the
//! front-end's arguments again, as pytest's `OverrideIniAction` does.
//!
//! [`PytestConfig::file_ini`]: crate::config::PytestConfig::file_ini

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::config::strtobool;

/// A native TOML value as JSON (dates as their text).
pub(crate) fn toml_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), toml_json(v)))
                .collect(),
        ),
    }
}

/// `-o name=value` and the `--strict…` flags (pytest's
/// `OverrideIniAction`), in the order pytest reads `args`: the last one
/// wins.
pub fn apply_overrides(ini: &mut BTreeMap<String, Value>, args: &[String]) {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        let value = match arg.as_str() {
            "--strict-config" => Some("strict_config=true".to_string()),
            "--strict-markers" => Some("strict_markers=true".to_string()),
            "--strict" => Some("strict=true".to_string()),
            "-o" | "--override-ini" => iter.next().cloned(),
            _ => arg
                .strip_prefix("--override-ini=")
                .or_else(|| arg.strip_prefix("-o"))
                .map(str::to_string),
        };
        if let Some((name, value)) = value.as_deref().and_then(|v| v.split_once('=')) {
            ini.insert(name.trim().to_string(), json!(value));
        }
    }
}

/// pytest's `_strtobool`, or a native bool.
pub fn ini_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(text) => strtobool(text),
        _ => None,
    }
}

/// A `linelist` value: one entry per non-empty line of a string, or a
/// list's strings.
pub fn ini_lines(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => text
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}
