//! What consumes the run's records, after the passes: the reporters (the
//! output format's on stdout, GitHub's annotations, the JUnit report),
//! history ([`Remember`]) and the run store ([`Keep`]).

use std::io::{self, IsTerminal as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aye_config::OutputFormat;
use aye_events::{Event, Record};
use aye_history::{History, RunWriter, StoredRunId};
use aye_render::{GithubAnnotations, JsonLines, Junit, Reporter, Tally, Tee, Text, TextOptions};

use crate::printer::Printer;
use crate::settings::{JunitTarget, RunSettings};

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
            tee.push(Stdout::new(Text::new(
                io::stdout(),
                TextOptions {
                    verbosity,
                    color: settings.output.color,
                    live: io::stdout().is_terminal(),
                },
            )));
            if aye_render::github_actions_enabled(&settings.env.vars) {
                let prefix =
                    repository_prefix(&settings.root, settings.env.github_workspace.as_deref());
                tee.push(Stdout::new(
                    GithubAnnotations::new(io::stdout()).with_path_prefix(prefix),
                ));
            }
        }
        OutputFormat::Json => tee.push(Stdout::new(JsonLines::new(io::stdout(), verbosity))),
    }
    if let Some(junit) = junit {
        tee.push(File {
            inner: Junit::new(junit.path)
                .with_prefix(junit.prefix)
                .with_options(junit.options),
            failed: Arc::clone(file_failed),
            printer,
        });
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

    pub(crate) fn record(&mut self, record: &Record) {
        self.tally.note(record);
        self.writer.record(record);
    }

    pub(crate) fn dir(&self) -> &Path {
        self.writer.dir()
    }

    pub(crate) fn finish(self) -> io::Result<Option<StoredRunId>> {
        self.writer.finish(self.tally.extra().named())
    }
}

/// A reporter writing to stdout. After its first error (a closed pipe) it
/// writes nothing more, so the error is reported once and every other
/// reporter still sees the whole stream.
pub(crate) struct Stdout<R> {
    inner: R,
    failed: bool,
}

impl<R> Stdout<R> {
    pub(crate) const fn new(inner: R) -> Self {
        Self {
            inner,
            failed: false,
        }
    }
}

impl<R: Reporter> Reporter for Stdout<R> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner
            .report(record)
            .inspect_err(|_| self.failed = true)
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner.finish().inspect_err(|_| self.failed = true)
    }
}

/// A reporter writing a file when the stream ends: its error is reported
/// here, naming the file, and remembered for the exit code.
pub(crate) struct File<R> {
    pub(crate) inner: R,
    pub(crate) failed: Arc<AtomicBool>,
    pub(crate) printer: Printer,
}

impl<R: Reporter> Reporter for File<R> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        self.inner.report(record)
    }

    fn finish(&mut self) -> io::Result<()> {
        if let Err(err) = self.inner.finish() {
            self.printer.error(err);
            self.failed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// History as a consumer of the stream, saved in `dir` at the end; and,
/// before anything else sees a bad outcome, what it says of the test's
/// earlier runs.
pub(crate) struct Remember<'a> {
    pub(crate) history: &'a mut History,
    pub(crate) dir: PathBuf,
}

impl Remember<'_> {
    /// Sets a bad outcome's `history`, from history as it was before this
    /// run, when the test has some.
    pub(crate) fn tell(&self, record: &mut Record) {
        if let Event::TestFinished(result) = &mut record.event
            && result.outcome.is_bad()
            && result.history.is_none()
        {
            result.history = self.history.previous(&result.id);
        }
    }

    pub(crate) fn save(&self, printer: Printer) {
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
