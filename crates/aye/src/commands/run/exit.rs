//! Where a run's exit code is decided ([`RunVerdict`]), from the facts the
//! run gathers: the error diagnostics the stream carried ([`Errors`]), the
//! runner's summary, the cases selected and the tests' counts. The values
//! are pytest's, and `docs/events.md` ("Exit codes") says what each means.
//!
//! Decided (implementation, October 2026):
//! - **Exit codes** come from the stream as well as the counts: pytest's
//!   `usage-error` (it rejected its arguments, or an initial conftest failed
//!   to load) exits 4 and `internal-error` 3, whichever batch reports it,
//!   or collection (aye's pytest front-end runs pytest's collection hooks
//!   while it lists, where a failing one ends the run before anything runs);
//!   other error diagnostics, collection's or the run's, exit 2, and 1
//!   under `--continue-on-collection-errors`, as pytest counts each
//!   collection error as a failure: also when nothing else runs, and under
//!   `--co`. A run the framework stopped as interrupted exits 2, and
//!   `run_finished.stopped` says why a run ended early. A batch the
//!   framework failed although none of its tests failed (the runner's
//!   `framework_failed`: a conftest's `session.exitstatus`, pytest-cov's
//!   `--cov-fail-under`) fails the run (1), as pytest exits 1.

use aye_events::{Counts, Diagnostic, DiagnosticOrigin, ExitCode};
use aye_runner::RunSummary;

/// Error diagnostics seen in the stream, by what they mean for the exit code
/// ([`aye_events::ErrorClass::exit_code`]).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Errors {
    pub(crate) any: bool,
    pub(crate) usage: bool,
    pub(crate) internal: bool,
}

impl Errors {
    pub(crate) fn see(&mut self, diagnostic: &Diagnostic, origin: DiagnosticOrigin) {
        let Some(class) = diagnostic.error_class(origin) else {
            return;
        };
        self.any = true;
        match class.exit_code() {
            Some(ExitCode::UsageError) => self.usage = true,
            Some(ExitCode::InternalError) => self.internal = true,
            _ => {}
        }
    }
}

/// How a run ended: the exit code `run_finished` carried, and whether a
/// report file that could be written when the run started couldn't be at
/// its end, which only the process's exit code can say ([`crate::main`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunEnd {
    pub(crate) code: ExitCode,
    pub(crate) report_failed: bool,
}

/// When a run's exit code is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunStage {
    /// When collection ends, before anything runs.
    Collection,
    /// When the run ends.
    Run,
}

/// What a run's exit code is decided on, at either stage. Collection gives
/// all of it but the runner's facts and the tests' counts
/// ([`RunVerdict::with_run`] adds those).
#[derive(Debug, Clone, Default)]
pub(crate) struct RunVerdict {
    /// Ctrl-C, or the framework stopping a batch as interrupted
    /// (`pytest.exit`, a test raising `KeyboardInterrupt`).
    pub(crate) interrupted: bool,
    /// An internal error: pytest's `INTERNALERROR` (a hook that failed
    /// while aye's pytest front-end collected), a worker that died outside
    /// any test.
    pub(crate) internal: bool,
    /// A usage error: what the command line names that names nothing, the
    /// framework rejecting its arguments or failing to load an initial
    /// conftest (which pytest exits 4 for).
    pub(crate) usage: bool,
    /// Any error diagnostic: collection's, then the run's too.
    pub(crate) errors: bool,
    /// A batch the framework failed although none of its tests did (a
    /// conftest's `session.exitstatus`, pytest-cov's `--cov-fail-under`).
    pub(crate) framework_failed: bool,
    /// Cases selected when collection ended.
    pub(crate) selected: u32,
    /// The tests' counts, and the deselected.
    pub(crate) counts: Counts,
    pub(crate) collect_only: bool,
    pub(crate) continue_on_collection_errors: bool,
}

impl RunVerdict {
    /// Collection's verdict (`self`) once the run ran: the runner's
    /// `summary`, the error diagnostics the run's records carried
    /// (`errors`), and the tests' `counts`. Collection's errors, which
    /// `--continue-on-collection-errors` let the run go past, still fail
    /// it.
    pub(crate) fn with_run(self, summary: &RunSummary, errors: Errors, counts: Counts) -> Self {
        Self {
            interrupted: summary.interrupted,
            internal: summary.internal_error || errors.internal,
            usage: errors.usage,
            errors: errors.any || self.errors,
            framework_failed: summary.framework_failed,
            counts,
            ..self
        }
    }

    /// Whether the run ends when collection does: an interruption, an
    /// internal or usage error, collection errors without
    /// `--continue-on-collection-errors` (pytest counts each collection
    /// error as a failure, which stops it), `--co`, or nothing selected.
    pub(crate) const fn ends_after_collection(&self) -> bool {
        self.interrupted
            || self.internal
            || self.usage
            || (self.errors && !self.continue_on_collection_errors)
            || self.collect_only
            || self.selected == 0
    }

    /// The exit code of a run that ends at `stage`. An interruption decides
    /// first, then an internal error, then a usage error, then collection
    /// errors the run can't go past (2), then bad outcomes, other error
    /// diagnostics (collection's under `--continue-on-collection-errors`,
    /// `--co` included) or a framework that failed a clean batch (1), then
    /// nothing to run (5: no case selected when collection ends, no test
    /// counted when the run does).
    pub(crate) const fn exit_code(&self, stage: RunStage) -> ExitCode {
        let nothing = match stage {
            RunStage::Collection => self.selected == 0,
            RunStage::Run => self.counts.total() == 0,
        };
        if self.interrupted {
            ExitCode::Interrupted
        } else if self.internal {
            ExitCode::InternalError
        } else if self.usage {
            ExitCode::UsageError
        } else if self.errors && !self.continue_on_collection_errors {
            ExitCode::Interrupted
        } else if self.counts.bad() > 0 || self.errors || self.framework_failed {
            ExitCode::TestsFailed
        } else if nothing {
            ExitCode::NoTests
        } else {
            ExitCode::Ok
        }
    }
}

#[cfg(test)]
mod tests;
