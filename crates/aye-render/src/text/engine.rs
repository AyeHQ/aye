//! What the engine did, at `-vvv`.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Event, Record};

use super::Text;
use super::stylesheet::DIM;
use crate::words;

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// What the engine did (`-vvv`): a line for each worker and batch
    /// starting and finishing, each lifetime starting and finishing (a
    /// failed one's block follows), and each sub-outcome that didn't fail.
    pub(super) fn engine_detail(&mut self, record: &Record) -> io::Result<()> {
        let worker = record
            .envelope
            .worker
            .map_or_else(|| "a worker".to_owned(), |w| format!("worker {w}"));
        let batch = record
            .envelope
            .batch
            .map_or_else(|| "a batch".to_owned(), |b| format!("batch {b}"));
        let line = match &record.event {
            Event::WorkerStarted { pid, python, .. } => {
                format!("{worker} started (pid {pid}, {python})")
            }
            Event::BatchStarted { adapter, cases } => format!(
                "{batch} started on {worker} ({adapter}, {})",
                words::plural(*cases, "test", "tests")
            ),
            Event::LifetimeStarted { lifetime, kind } => {
                format!("lifetime {lifetime} started ({kind})")
            }
            Event::LifetimeFinished {
                lifetime,
                failure,
                duration,
            } => {
                let finished = if failure.is_some() {
                    "failed"
                } else {
                    "finished"
                };
                format!(
                    "lifetime {lifetime} {finished} ({})",
                    words::spaced(*duration)
                )
            }
            Event::BatchFinished { status } => match status {
                Some(status) => format!("{batch} finished (status {status})"),
                None => format!("{batch} finished"),
            },
            // A failed worker's block says it.
            Event::WorkerFinished {
                exit_status,
                error: None,
            } => match exit_status {
                Some(status) => format!("{worker} finished (exit status {status})"),
                None => format!("{worker} finished"),
            },
            Event::SubOutcome {
                id,
                name,
                outcome,
                reason,
                duration,
                ..
            } if !outcome.is_bad() => {
                self.lines()?;
                return self.result_line(
                    *outcome,
                    None,
                    &format!("{id} › {name}"),
                    Some(*duration),
                    reason.as_deref(),
                );
            }
            _ => return Ok(()),
        };
        self.lines()?;
        writeln!(self.out, "{DIM}{line}{DIM:#}")
    }
}
