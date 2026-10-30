//! Where every record of a run goes once stamped ([`Sink`]): the error
//! tally, the reporters (the output format's on stdout, GitHub's
//! annotations, the JUnit report: [`reporters`]), history ([`Remember`])
//! and the run store ([`Keep`]).
//!
//! Decided (implementation, October 2026):
//! - **Output.** A stdout reporter that fails (a closed pipe) stops writing
//!   (`aye_stream::StopAfterError`), and the JUnit report and history still
//!   see every record. With `-s`,
//!   workers' output goes to stdout beside text output and to stderr beside
//!   `--output json`, so JSON lines stay whole. GitHub annotations go
//!   beside text output, at every verbosity, never beside JSON. The
//!   JUnit report goes to `--junitxml` (with `$VARIABLES` and `~` expanded,
//!   as pytest does), else to the framework's own: the `junitxml` path its
//!   adapter read from pytest's `addopts`, `PYTEST_ADDOPTS` or the
//!   arguments after `--`. Relative paths are taken from the current
//!   directory. Its `junit_prefix` (pytest's `--junitprefix`) goes before
//!   the report's classnames either way, and its JUnit settings
//!   (`junit_suite_name`, `junit_logging`, `junit_log_passing_tests`,
//!   `junit_duration_report`, `junit_family`, from the first adapter that
//!   gives each) shape the report, which is aye's own layout without them.
//!   The path is checked before anything runs (exit 4); if the report
//!   still can't be written at the end, the process exits 1 although
//!   `run_finished` said 0 or 5 (`aye_stream::FailOnFinish`). GitHub annotation paths are relative to
//!   `GITHUB_WORKSPACE` (else the directory holding `.git`).
//! - **Every run is kept for `aye show`** (October 2026), numbered (`r81`,
//!   claimed before `run_started`, which says it): the run store
//!   (`aye_history::store`) is the stream's last consumer, serializing each
//!   record once every reporter has it and compressing the log on a thread
//!   of its own, so the run doesn't wait on it (on `packaging`'s 62,000
//!   tests, 57 MB of records, the run takes as long). Beside it the binary
//!   folds what the summaries count beside the tests (`aye_stream::Tally`)
//!   and hands it to the store with the end of the stream, for `aye show
//!   --runs`. A run with nothing to show (no test ran and nothing failed
//!   but the command line's names: a typo in a path) isn't kept, so `last`
//!   stays the run before it, and its number is free again. `[tool.aye]
//!   keep-runs` (10) says how many runs are kept. A store that can't be
//!   written is a warning, never a failed run. History, folded after the
//!   reporters, first gives each bad outcome its `history` (how the test
//!   ended before this run), which reporters and the store see.

use std::io::{self, IsTerminal as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aye_config::OutputFormat;
use aye_events::{DiagnosticOrigin, Event, Record};
use aye_history::{History, RunWriter, StoredRunId};
use aye_render::{GithubAnnotations, JsonLines, Junit, Text, TextOptions};
use aye_stream::{Consumer, FailOnFinish, StopAfterError, Tally, Tee};

use super::passes::PreviousOutcome;
use crate::commands::run::Errors;
use crate::printer::Printer;
use crate::settings::{JunitTarget, RunSettings};

/// What consumes every record of the run, the binary's own and the
/// runner's (those after the passes), in this order: history's word on a
/// bad outcome ([`PreviousOutcome`]), the error tally, every reporter,
/// history's fold, then the run store. The stamper stays apart, with the
/// stream, so the runner can stamp its records while they come here.
pub(crate) struct Sink<'a> {
    reporters: Tee<'a>,
    /// Whether a report file could not be written at the end, which its
    /// reporter ([`FailOnFinish`]) sets.
    report_failed: Arc<AtomicBool>,
    /// History: what each bad outcome says of the test's earlier runs, and
    /// a consumer of every record after the reporters.
    history: Option<Remember<'a>>,
    /// Keeps the run's log for `aye show`: the last consumer of each
    /// record.
    store: Keep,
    /// The error diagnostics carried since the tally was last taken.
    errors: Errors,
    printer: Printer,
}

impl<'a> Sink<'a> {
    pub(crate) fn new(
        reporters: Tee<'a>,
        report_failed: Arc<AtomicBool>,
        history: Option<Remember<'a>>,
        store: Keep,
        printer: Printer,
    ) -> Self {
        Self {
            reporters,
            report_failed,
            history,
            store,
            errors: Errors::default(),
            printer,
        }
    }

    /// A bad outcome first gets the test's `history` (how it ended before
    /// this run). Then every reporter gets every record: one that fails
    /// (stdout, a closed pipe) stops writing, and the others carry on.
    /// Then history folds it, and the run store keeps it.
    pub(crate) fn deliver(&mut self, record: &mut Record) {
        if let Some(history) = &self.history {
            PreviousOutcome(history.history).apply(record);
        }
        if let Event::Diagnostic(d) = &record.event {
            self.errors.see(d, DiagnosticOrigin::of(&record.envelope));
        }
        if let Err(err) = self.reporters.consume(record)
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            self.printer
                .note(format_args!("cannot write output: {err}"));
        }
        if let Some(history) = self.history.as_mut() {
            history.history.record(record);
        }
        self.store.record(record);
    }

    /// The error diagnostics carried since the tally was last taken, or
    /// since the stream started; the tally starts again from none.
    pub(crate) fn take_errors(&mut self) -> Errors {
        std::mem::take(&mut self.errors)
    }

    /// The stream is over: the reporters finish, history is saved and the
    /// run kept (neither fails the run). Whether a report file could not
    /// be written.
    pub(crate) fn finish(mut self) -> bool {
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
        self.report_failed.load(Ordering::Relaxed)
    }
}

/// The run's reporters: its output format's on stdout (and GitHub's
/// annotations beside text, in a step of GitHub Actions), and the JUnit
/// report, whose failure at the end `printer` says and `file_failed`
/// remembers.
pub(crate) fn reporters<'a>(
    settings: &RunSettings,
    junit: Option<JunitTarget>,
    file_failed: &Arc<AtomicBool>,
    printer: Printer,
) -> Tee<'a> {
    let mut tee = Tee::new();
    let verbosity = settings.output.verbosity.level();
    match settings.output.format.value {
        OutputFormat::Text => {
            tee.push(StopAfterError::new(Text::new(
                io::stdout(),
                TextOptions {
                    verbosity,
                    color: settings.output.color,
                    live: io::stdout().is_terminal(),
                },
            )));
            if settings.env.github_actions {
                let prefix =
                    repository_prefix(&settings.root, settings.env.github_workspace.as_deref());
                tee.push(StopAfterError::new(
                    GithubAnnotations::new(io::stdout()).with_path_prefix(prefix),
                ));
            }
        }
        OutputFormat::Json => {
            tee.push(StopAfterError::new(JsonLines::new(io::stdout(), verbosity)));
        }
    }
    if let Some(junit) = junit {
        tee.push(FailOnFinish::new(
            Junit::new(junit.path)
                .with_prefix(junit.prefix)
                .with_options(junit.options),
            Arc::clone(file_failed),
            move |err| printer.error(err),
        ));
    }
    tee
}

/// The run store's writer, and what the run's summary counts beside the
/// tests' counts, which it keeps beside the log (`aye show --runs` words
/// them as the summary did).
pub(crate) struct Keep {
    writer: RunWriter,
    tally: Tally,
}

impl Keep {
    pub(crate) fn new(writer: RunWriter) -> Self {
        Self {
            writer,
            tally: Tally::default(),
        }
    }

    fn record(&mut self, record: &Record) {
        self.tally.note(record);
        self.writer.record(record);
    }

    fn dir(&self) -> &Path {
        self.writer.dir()
    }

    fn finish(self) -> io::Result<Option<StoredRunId>> {
        self.writer.finish(self.tally.extra().named())
    }
}

/// History as a consumer of the stream, saved in `dir` at the end; and,
/// before anything else sees a bad outcome, what it says of the test's
/// earlier runs ([`PreviousOutcome`]).
pub(crate) struct Remember<'a> {
    pub(crate) history: &'a mut History,
    pub(crate) dir: PathBuf,
}

impl Remember<'_> {
    fn save(&self, printer: Printer) {
        if self.history.changed()
            && let Err(err) = self.history.save(&self.dir)
        {
            printer.warning(format_args!(
                "could not save history to {}: {err}",
                self.dir.display()
            ));
        }
    }
}

/// Where the project root lies in the repository, for GitHub annotations:
/// relative to `GITHUB_WORKSPACE` when the root is inside it, else to the
/// nearest directory above the root holding `.git`; empty at the top.
fn repository_prefix(root: &Path, workspace: Option<&Path>) -> String {
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = real(root);
    let inside = |top: &Path| {
        root.strip_prefix(real(top)).ok().map(|rest| {
            rest.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
    };
    workspace
        .filter(|w| !w.as_os_str().is_empty())
        .and_then(inside)
        .or_else(|| {
            root.ancestors()
                .find(|dir| dir.join(".git").exists())
                .and_then(inside)
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn annotation_paths_are_relative_to_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path();
        fs::create_dir_all(top.join(".git")).unwrap();
        fs::create_dir_all(top.join("sub/project")).unwrap();
        let root = top.join("sub/project");
        assert_eq!(repository_prefix(&root, Some(top)), "sub/project");
        // Without GITHUB_WORKSPACE, the directory holding `.git`.
        assert_eq!(repository_prefix(&root, None), "sub/project");
        assert_eq!(repository_prefix(top, Some(top)), "");
        // A workspace that doesn't hold the project is no help.
        let other = tempfile::tempdir().unwrap();
        assert_eq!(repository_prefix(&root, Some(other.path())), "sub/project");
        let lone = tempfile::tempdir().unwrap();
        assert_eq!(repository_prefix(lone.path(), Some(other.path())), "");
    }
}
