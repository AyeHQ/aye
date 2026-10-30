//! What `aye show` asks of text output: the list's lines, and the warnings
//! of files changed since the run.

use std::io::{self, Write};
use std::time::Duration;

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Failure, LifetimeId, Outcome, TestResult};
use aye_stream::{Again, FileChange};

use super::Text;
use super::bad::lifetime_name;
use super::stylesheet::YELLOW;
use crate::{RenderMode, failure, rerun};

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// Fed as `mode` says: a run's stream ([`RenderMode::Run`], as
    /// [`Self::new`] is), or part of a stored log for `aye show`.
    #[must_use]
    pub fn with_mode(mut self, mode: RenderMode) -> Self {
        (self.keys, self.show, self.context) = mode.into_parts();
        self
    }

    /// Before anything else, for `aye show`: `path` changed since the run
    /// (or is `gone`), so what is shown of its tests may be out of date. A
    /// warning, shown at every level.
    pub fn stale(&mut self, path: &str, change: FileChange) -> io::Result<()> {
        if change == FileChange::Gone {
            writeln!(self.out, "{YELLOW}{path} is gone since this run{YELLOW:#}")
        } else {
            let rerun = rerun::rerun_path(path);
            writeln!(
                self.out,
                "{YELLOW}{path} changed since this run; rerun: {rerun}{YELLOW:#}"
            )
        }
    }

    /// A test's line in `aye show`'s list: its outcome, its key when bad,
    /// its id, its duration, and why it went bad or was skipped.
    pub(super) fn list_line(&mut self, result: &TestResult) -> io::Result<()> {
        let bad = result.outcome.is_bad();
        let key = bad.then(|| self.key(&result.id));
        let note = if bad {
            match (&result.cause, result.failure()) {
                (Some(lifetime), _) => Some(format!("setup of {lifetime} failed")),
                (None, Some(failure)) => Some(failure::why(failure, true)),
                (None, None) => failure::bare_why(&result.output),
            }
        } else {
            result.reason.clone()
        };
        self.lines()?;
        self.result_line(
            result.outcome,
            key.as_deref(),
            result.id.as_str(),
            Some(result.duration),
            note.as_deref(),
        )
    }

    /// A failed lifetime's line in `aye show`'s list, once per lifetime.
    pub(super) fn lifetime_list_line(
        &mut self,
        lifetime: &LifetimeId,
        failure: &Failure,
        duration: Duration,
    ) -> io::Result<()> {
        if !self.blocked.failed(lifetime, failure, Again::Never) {
            return Ok(());
        }
        let key = self.key(lifetime);
        let name = lifetime_name(lifetime, failure);
        let why = failure::why(failure, false);
        self.lines()?;
        self.result_line(
            Outcome::Error,
            Some(&key),
            &name,
            Some(duration),
            Some(&why),
        )
    }
}
