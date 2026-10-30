//! The raw values of pytest's configuration, as JSON, for the front-end's
//! `getini` to coerce by each option's type: what the pytest adapter's
//! native context's `ini` holds.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

/// Python's `str()` of a TOML scalar, as `[tool.pytest.ini_options]`
/// stringifies values.
fn python_str(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        toml::Value::Float(f) => aye_python_stdlib::repr::float_repr(*f),
        other => other.to_string(),
    }
}

/// A native TOML value as JSON (dates as their text).
fn toml_json(value: &toml::Value) -> Value {
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

/// The raw values of pytest's configuration in `inifile` (see the pytest
/// adapter's native context's `ini`). The file was read once already,
/// with its errors reported; here an unreadable one gives nothing.
pub fn read_ini(inifile: Option<&Path>) -> BTreeMap<String, Value> {
    let Some(path) = inifile else {
        return BTreeMap::new();
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let texts = |section: Option<crate::ini::Section>| -> BTreeMap<String, Value> {
        section
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, json!(v)))
            .collect()
    };
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "ini" => texts(crate::ini::ini_section(path, "pytest").ok().flatten()),
        "cfg" => texts(crate::ini::ini_section(path, "tool:pytest").ok().flatten()),
        "toml" => {
            let Ok(doc) = aye_config::read_toml(path) else {
                return BTreeMap::new();
            };
            let native = |table: &toml::Table| -> BTreeMap<String, Value> {
                table
                    .iter()
                    .map(|(k, v)| (k.clone(), toml_json(v)))
                    .collect()
            };
            if name == "pytest.toml" || name == ".pytest.toml" {
                return doc
                    .get("pytest")
                    .and_then(toml::Value::as_table)
                    .map(native)
                    .unwrap_or_default();
            }
            let Some(tool) = doc
                .get("tool")
                .and_then(|t| t.get("pytest"))
                .and_then(toml::Value::as_table)
            else {
                return BTreeMap::new();
            };
            let mut table = tool.clone();
            match table.remove("ini_options") {
                Some(toml::Value::Table(ini)) if table.is_empty() => ini
                    .iter()
                    .map(|(k, v)| {
                        let value = match v {
                            toml::Value::Array(items) => {
                                json!(items.iter().map(python_str).collect::<Vec<_>>())
                            }
                            other => json!(python_str(other)),
                        };
                        (k.clone(), value)
                    })
                    .collect(),
                _ => native(&table),
            }
        }
        _ => BTreeMap::new(),
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
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "t" | "true" | "on" | "1" => Some(true),
            "n" | "no" | "f" | "false" | "off" | "0" => Some(false),
            _ => None,
        },
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
