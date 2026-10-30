//! The run's event stream, as the binary carries it ([`Stream`]): the
//! records it emits itself, stamped, and the runner's, each delivered in
//! turn to history, the error tally, the reporters and the run store
//! (`sink.rs`), the runner's after the passes that rewrite some of them
//! (`passes.rs`).

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aye_events::{Counts, DiagnosticOrigin, Event, ExitCode, Record, Stamper, StopReason};
use aye_render::{Reporter as _, Tee};

use crate::commands::run::{Errors, RunEnd};
use crate::printer::Printer;

mod passes;
mod sink;

pub(crate) use passes::{Here, OneConftestError, OneModuleErrors};
pub(crate) use sink::{Keep, Remember, reporters};

/// The reporters, history, the run store, the stamper for the binary's own
/// records, and the errors the stream has carried so far.
pub(crate) struct Stream<'a> {
    pub(crate) reporters: Tee<'a>,
    /// History: what each bad outcome says of the test's earlier runs, and
    /// a consumer of every record after the reporters.
    pub(crate) history: Option<Remember<'a>>,
    /// Keeps the run's log for `aye show`: the last consumer of each
    /// record.
    pub(crate) store: Keep,
    pub(crate) stamper: Stamper,
    pub(crate) errors: Errors,
    pub(crate) started: Instant,
    pub(crate) printer: Printer,
}

impl Stream<'_> {
    pub(crate) fn emit(&mut self, event: Event) {
        let mut record = self.stamper.stamp(Record::new(event));
        Self::deliver_to(
            &mut self.reporters,
            &mut self.history,
            &mut self.store,
            &mut self.errors,
            self.printer,
            &mut record,
        );
    }

    /// A bad outcome first gets the test's `history` (how it ended before
    /// this run). Then every reporter gets every record: one that fails
    /// (stdout, a closed pipe) stops writing, and the others carry on.
    /// Then history folds it, and the run store keeps it.
    pub(crate) fn deliver_to(
        reporters: &mut Tee<'_>,
        history: &mut Option<Remember<'_>>,
        store: &mut Keep,
        errors: &mut Errors,
        printer: Printer,
        record: &mut Record,
    ) {
        if let Some(history) = history.as_ref() {
            history.tell(record);
        }
        if let Event::Diagnostic(d) = &record.event {
            errors.see(d, DiagnosticOrigin::of(&record.envelope));
        }
        if let Err(err) = reporters.report(record)
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            printer.note(format_args!("cannot write output: {err}"));
        }
        if let Some(history) = history.as_mut() {
            history.history.record(record);
        }
        store.record(record);
    }

    pub(crate) fn finish(
        mut self,
        code: ExitCode,
        counts: Counts,
        stopped: Option<StopReason>,
        file_failed: &AtomicBool,
    ) -> RunEnd {
        self.emit(Event::RunFinished {
            exit_code: code,
            counts,
            duration: self.started.elapsed(),
            stopped,
        });
        if let Err(err) = self.reporters.finish()
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            self.printer
                .note(format_args!("cannot write output: {err}"));
        }
        if let Some(history) = &self.history {
            history.save(self.printer);
        }
        // Kept for `aye show`; never fails the run.
        let runs = self.store.dir().to_path_buf();
        if let Err(err) = self.store.finish() {
            self.printer.warning(format_args!(
                "could not keep this run for `aye show` in {}: {err}",
                runs.display()
            ));
        }
        RunEnd {
            code,
            report_failed: file_failed.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests;
