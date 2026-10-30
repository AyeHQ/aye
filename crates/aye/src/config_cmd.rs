//! `aye config`: the resolved configuration, and where each value came from.

use std::io::{self, Write};

use anyhow::Result;

use aye_config::ValueSource;

use crate::settings::RunSettings;

pub(crate) fn print(settings: &RunSettings) -> Result<()> {
    let mut out = io::stdout().lock();
    let root = &settings.root;
    writeln!(out, "root     {}", root.display())?;
    let python = &settings.interpreter;
    writeln!(
        out,
        "python   {}  ({})",
        python.value.executable.display(),
        python.source.display_for("python")
    )?;
    writeln!(out, "workers  {}", settings.workers)?;
    // The format at its verbosity, as the command line would ask for it.
    let output = &settings.output;
    writeln!(
        out,
        "output   {}  ({})",
        output.shown(),
        output.format.source.display_for("output")
    )?;
    let adapters = settings.adapter_names();
    writeln!(
        out,
        "adapters {}  ({})",
        adapters.join(", "),
        why(&settings.adapters_source)
    )?;
    // Which engine runs the tests: aye's own, whatever the framework.
    writeln!(out, "engine   native")?;
    let entries = settings.tool_aye.entries();
    writeln!(out)?;
    writeln!(out, "[tool.aye]")?;
    if entries.is_empty() {
        writeln!(out, "  (not set)")?;
    }
    for (name, value, source) in entries {
        writeln!(out, "  {name} = {value}    # {}", source.display_row(root))?;
    }
    for adapter in adapters {
        writeln!(out)?;
        writeln!(out, "[{adapter}]")?;
        let Some((_, recognizer)) = settings.prepared.iter().find(|(name, _)| *name == adapter)
        else {
            // Its `prepare` found nothing for it to collect here (doctest
            // without doctests turned on).
            writeln!(out, "  (nothing to collect in this project)")?;
            continue;
        };
        for setting in recognizer.settings() {
            writeln!(
                out,
                "  {} = {}    # {}",
                setting.name,
                setting.value,
                setting.source.display_row(root)
            )?;
        }
    }
    Ok(())
}

/// Why the run uses the adapters it does: a list `[tool.aye]` gives is
/// "from" it, the default its rule.
fn why(source: &ValueSource) -> String {
    match source {
        ValueSource::File { .. } => format!("from {}", source.display_for("adapters")),
        other => other.display_for("adapters").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_adapters_reason_is_shown_in_words() {
        assert_eq!(
            why(&ValueSource::file("/repo/pyproject.toml", "[tool.aye]")),
            "from [tool.aye] adapters"
        );
        assert_eq!(
            why(&crate::registry::DEFAULT_ADAPTERS),
            "default: pytest's rules, which run unittest's TestCase classes too"
        );
    }
}
