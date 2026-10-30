//! Diagnostics as they come, and failed workers.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Diagnostic, Outcome, Severity, WorkerIndex};
use aye_stream::Noted;

use super::Text;
use super::limits::WORKER_LINES;
use super::stylesheet::{DIM, RED_BOLD, YELLOW};
use crate::{ShowMode, words};

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// A diagnostic, `noted` what the tally made of it: an error is shown
    /// the first time it comes, a warning in the closing lines, an info
    /// from `-vv`. A module skipped while collecting counts with the skips
    /// (for `aye show`, it is a line as a skipped test's), and `aye show`'s
    /// list shows no other warning or info.
    pub(super) fn diagnostic(
        &mut self,
        diagnostic: &Diagnostic,
        noted: Option<Noted<'_>>,
    ) -> io::Result<()> {
        if matches!(noted, Some(Noted::ModuleSkip(_))) {
            if self.show.is_some() {
                if self.quiet() {
                    return Ok(());
                }
                self.lines()?;
                return self.result_line(
                    Outcome::Skipped,
                    None,
                    diagnostic.path.as_str(),
                    None,
                    Some(&diagnostic.message),
                );
            }
            self.skipped(&diagnostic.message);
        }
        match diagnostic.severity {
            Severity::Error if matches!(noted, Some(Noted::Error(_))) => {
                self.lines()?;
                self.diagnostic_line(diagnostic)
            }
            Severity::Error => Ok(()),
            _ if self.show == Some(ShowMode::List) => Ok(()),
            Severity::Warning => {
                self.warnings.push(diagnostic.clone());
                Ok(())
            }
            Severity::Info if self.full() => {
                self.lines()?;
                self.diagnostic_line(diagnostic)
            }
            Severity::Info => Ok(()),
        }
    }

    pub(super) fn diagnostic_line(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        let (label, style) = match diagnostic.severity {
            Severity::Error => ("error", RED_BOLD),
            Severity::Warning => ("warning", YELLOW),
            Severity::Info => ("info", DIM),
        };
        let at = match diagnostic.span.as_ref().and_then(|span| span.line) {
            Some(line) => format!(" {}", self.location(diagnostic.path.as_str(), line)),
            None if diagnostic.path.is_nowhere() => String::new(),
            None => format!(" {}", diagnostic.path),
        };
        let mut lines = diagnostic.message.lines();
        let first = lines.next().unwrap_or_default();
        writeln!(
            self.out,
            "{style}{label}[{}]{style:#}{at}: {first}",
            diagnostic.code
        )?;
        for line in lines {
            writeln!(self.out, "  {line}")?;
        }
        Ok(())
    }

    pub(super) fn worker_error(
        &mut self,
        worker: Option<WorkerIndex>,
        status: Option<i32>,
        error: &str,
    ) -> io::Result<()> {
        let who = worker.map_or_else(|| "a worker".to_owned(), |w| format!("worker {w}"));
        let status = status
            .map(|status| format!(" (exit status {status})"))
            .unwrap_or_default();
        self.block()?;
        writeln!(
            self.out,
            "{RED_BOLD}{who} failed{RED_BOLD:#}{DIM}{status}{DIM:#}"
        )?;
        let lines = if self.full() {
            usize::MAX
        } else {
            WORKER_LINES
        };
        for line in words::last_lines(error, lines).lines {
            writeln!(self.out, "  {}", words::head(line, self.line_max()))?;
        }
        writeln!(self.out)
    }
}
