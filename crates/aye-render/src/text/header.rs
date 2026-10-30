//! The header (`aye · <root> · python 3.13.1 · 15 tests · 1 worker`), and
//! the `collected` line that follows it.

use std::io::{self, Write};
use std::time::Duration;

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::WorkerIndex;

use super::Text;
use super::out::Group;
use super::stylesheet::DIM;
use crate::words;

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// The header, once, at the first worker: `aye · <root> · python 3.13.1
    /// · 15 tests · 1 worker`.
    pub(super) fn header(&mut self, worker: Option<WorkerIndex>) -> io::Result<()> {
        if self.header_shown || self.show.is_some() || self.facts.collect_only || self.quiet() {
            return Ok(());
        }
        self.header_shown = true;
        let mut parts = vec!["aye".to_owned()];
        if let Some(root) = &self.facts.root {
            let trimmed = root.trim_end_matches(['/', '\\']);
            let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
            if !name.is_empty() {
                parts.push(name.to_owned());
            }
        }
        if let Some(python) = self.facts.python(worker) {
            parts.push(format!("python {python}"));
        }
        if let Some(selected) = self.facts.selected {
            parts.push(words::plural(selected, "test", "tests"));
        }
        parts.push(words::plural(
            self.facts.workers.max(1),
            "worker",
            "workers",
        ));
        self.out.paragraph()?;
        writeln!(self.out, "{DIM}{}{DIM:#}", parts.join(" · "))?;
        self.group = Group::Header;
        if let Some(collected) = self.collected.take() {
            writeln!(self.out, "{collected}")?;
        }
        Ok(())
    }

    pub(super) fn collection_finished(
        &mut self,
        selected: u32,
        deselected: u32,
        refined_files: u32,
        duration: Duration,
    ) -> io::Result<()> {
        if !(self.full() || self.facts.collect_only) {
            return Ok(());
        }
        let mut notes = Vec::new();
        if deselected > 0 {
            notes.push(format!("{deselected} deselected"));
        }
        if refined_files > 0 && self.full() {
            notes.push(words::plural(
                refined_files,
                "file imported",
                "files imported",
            ));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };
        let line = format!(
            "collected {}{notes} in {}",
            words::plural(selected, "test", "tests"),
            words::spaced(duration)
        );
        // In a run, it follows the header (when a worker starts).
        if !self.facts.collect_only && self.show.is_none() && !self.quiet() {
            self.collected = Some(line);
            return Ok(());
        }
        self.lines()?;
        writeln!(self.out, "{line}")
    }

    /// The `collected` line held for the header, when something else comes
    /// first (a log without `worker_started`).
    pub(super) fn flush_collected(&mut self) -> io::Result<()> {
        if let Some(line) = self.collected.take() {
            self.lines()?;
            writeln!(self.out, "{line}")?;
        }
        Ok(())
    }
}
