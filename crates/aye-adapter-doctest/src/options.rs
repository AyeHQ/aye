//! `[tool.aye.doctest]` as written: its keys, each value checked.

use std::path::Path;

use aye_adapter::AdapterError;

use crate::parser::OptionFlag;

/// The keys `[tool.aye.doctest]` accepts.
const AYE_KEYS: [&str; 4] = ["modules", "glob", "optionflags", "encoding"];

/// `[tool.aye.doctest]`, checked.
#[derive(Debug, Clone, Default)]
pub(crate) struct AyeDoctest {
    pub(crate) modules: Option<bool>,
    pub(crate) glob: Option<Vec<String>>,
    pub(crate) optionflags: Option<Vec<OptionFlag>>,
    pub(crate) encoding: Option<String>,
}

pub(crate) fn aye_doctest(
    table: Option<&toml::Table>,
    path: &Path,
) -> Result<AyeDoctest, AdapterError> {
    let mut out = AyeDoctest::default();
    let Some(table) = table else {
        return Ok(out);
    };
    let error = |message: String| {
        AdapterError::Config(format!(
            "invalid [tool.aye.doctest] in {}: {message}",
            path.display()
        ))
    };
    let strings = |key: &str, value: &toml::Value| -> Result<Vec<String>, AdapterError> {
        match value {
            toml::Value::String(s) => Ok(vec![s.clone()]),
            toml::Value::Array(items) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| error(format!("{key}: expected a list of strings")))
                })
                .collect(),
            _ => Err(error(format!("{key}: expected a list of strings"))),
        }
    };
    for (key, value) in table {
        match key.as_str() {
            "modules" => {
                out.modules = Some(
                    value
                        .as_bool()
                        .ok_or_else(|| error("modules: expected true or false".into()))?,
                );
            }
            "glob" => out.glob = Some(strings(key, value)?),
            "optionflags" => {
                let flags = strings(key, value)?
                    .iter()
                    .map(|name| {
                        OptionFlag::from_name(name).ok_or_else(|| {
                            error(format!(
                                "optionflags: unknown flag {name:?} (expected one of {})",
                                OptionFlag::ALL.map(OptionFlag::name).join(", ")
                            ))
                        })
                    })
                    .collect::<Result<_, _>>()?;
                out.optionflags = Some(flags);
            }
            "encoding" => {
                out.encoding = Some(
                    value
                        .as_str()
                        .ok_or_else(|| error("encoding: expected a string".into()))?
                        .to_string(),
                );
            }
            other => {
                return Err(error(format!(
                    "unknown key {other:?} (expected one of {})",
                    AYE_KEYS.join(", ")
                )));
            }
        }
    }
    Ok(out)
}
