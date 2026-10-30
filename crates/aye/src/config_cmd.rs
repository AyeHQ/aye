//! `aye config`: the resolved configuration, and where each value came from.

use std::io::{self, Write};

use anyhow::Result;

use crate::session::Session;

pub(crate) fn print(session: &Session) -> Result<()> {
    let mut out = io::stdout().lock();
    let root = session.root();
    writeln!(out, "root     {}", root.display())?;
    writeln!(
        out,
        "python   {}  ({})",
        session.python().display(),
        session.python_source()
    )?;
    writeln!(out, "workers  {}", session.workers())?;
    // The format at its verbosity, as the command line would ask for it.
    let (output, origin) = session.output_shown();
    writeln!(out, "output   {output}  ({origin})")?;
    let (adapters, why) = session.adapters();
    writeln!(out, "adapters {}  ({why})", adapters.join(", "))?;
    // Which engine runs the tests: aye's own, whatever the framework.
    writeln!(out, "engine   native")?;
    let entries = session.config().entries();
    writeln!(out)?;
    writeln!(out, "[tool.aye]")?;
    if entries.is_empty() {
        writeln!(out, "  (not set)")?;
    }
    for (name, value, origin) in entries {
        writeln!(
            out,
            "  {name} = {value}    # {}",
            origin.display_relative(root)
        )?;
    }
    for adapter in adapters {
        writeln!(out)?;
        writeln!(out, "[{adapter}]")?;
        let Some((_, recognizer)) = session.prepared().iter().find(|(name, _)| *name == adapter)
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
                setting.name, setting.value, setting.origin
            )?;
        }
    }
    Ok(())
}
