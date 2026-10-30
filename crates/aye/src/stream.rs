//! The run's event stream, as the binary carries it ([`Stream`]): the
//! records it emits itself, stamped, and the runner's, each delivered to
//! the sink (`sink.rs`: the error tally, the reporters, history and the run
//! store), the runner's after the passes that rewrite some of them
//! (`passes.rs`).

use std::time::Instant;

use aye_events::{Counts, Event, ExitCode, Record, Stamper, StopReason};

use crate::commands::run::RunEnd;

mod passes;
mod sink;

pub(crate) use passes::{Passes, RerunFromHere};
pub(crate) use sink::{Remember, Sink, reporters};

/// The run's stream: the stamper of the run's records, which stamps the
/// binary's own here and the runner's as they come, and the sink every
/// record goes to.
pub(crate) struct Stream<'a> {
    pub(crate) stamper: Stamper,
    pub(crate) sink: Sink<'a>,
    /// When the run started, for `run_finished`'s duration.
    started: Instant,
}

impl<'a> Stream<'a> {
    pub(crate) const fn new(stamper: Stamper, sink: Sink<'a>, started: Instant) -> Self {
        Self {
            stamper,
            sink,
            started,
        }
    }

    /// A record of the binary's own, stamped, to the sink.
    pub(crate) fn emit(&mut self, event: Event) {
        let mut record = self.stamper.stamp(Record::new(event));
        self.sink.deliver(&mut record);
    }

    /// The run's end: `run_finished`, with the exit code `code`, the
    /// tests' `counts` and why the run `stopped` early, if it did, then the
    /// sink's end ([`Sink::finish`]).
    pub(crate) fn finish(
        mut self,
        code: ExitCode,
        counts: Counts,
        stopped: Option<StopReason>,
    ) -> RunEnd {
        self.emit(Event::RunFinished {
            exit_code: code,
            counts,
            duration: self.started.elapsed(),
            stopped,
        });
        RunEnd {
            code,
            report_failed: self.sink.finish(),
        }
    }
}

#[cfg(test)]
mod tests;
