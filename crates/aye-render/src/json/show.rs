//! What `aye show` asks of JSON lines: the list's `test` and
//! `lifetime_fail` lines, the `stale` lines, and what the details add to a
//! bad outcome's line ([`Shown`]).

use std::io::{self, Write};

use aye_events::{Diagnostic, Failure, LifetimeId, Outcome, TestResult, WorkerIndex};
use aye_stream::{Again, FileChange};

use super::limits::{ID_MAX, PATH_MAX, REASON_MAX, RERUN_MAX};
use super::schema::{Detail, LifetimeFail, Line, Shown};
use super::{JsonLines, at};
use crate::{RenderMode, ShowMode, failure, rerun, words};

impl<W: Write> JsonLines<W> {
    /// Fed as `mode` says: a run's stream ([`RenderMode::Run`], as
    /// [`Self::new`] is), or part of a stored log for `aye show`.
    #[must_use]
    pub fn with_mode(mut self, mode: RenderMode) -> Self {
        (self.keys, self.show, self.context) = mode.into_parts();
        self
    }

    /// Before anything else, for `aye show`: a `stale` line, `path` having
    /// changed since the run (or being `gone`). None at `-q`.
    pub fn stale(&mut self, path: &str, change: FileChange) -> io::Result<()> {
        if self.quiet() {
            return Ok(());
        }
        let gone = change == FileChange::Gone;
        self.line(&Line::Stale {
            path: words::head(path, PATH_MAX),
            rerun: (!gone).then(|| words::head(&rerun::rerun_path(path), RERUN_MAX).into_owned()),
            gone,
        })
    }

    /// What `aye show`'s details add to a bad outcome's line about `path`,
    /// run by `worker`.
    pub(super) fn shown(&self, path: &str, worker: Option<WorkerIndex>) -> Option<Shown> {
        if self.show != Some(ShowMode::Details) {
            return None;
        }
        Some(Shown {
            run: self
                .context
                .run
                .or(self.facts.number)
                .map(|run| run.to_string()),
            python: self.facts.python(worker).map(str::to_owned),
            stale: self.context.stale.contains_key(path),
        })
    }

    /// A failed lifetime's line in `aye show`'s list, once per lifetime,
    /// at L1 whatever the level.
    pub(super) fn lifetime_list_line(
        &mut self,
        lifetime: &LifetimeId,
        failure: &Failure,
    ) -> io::Result<()> {
        if !self.blocked.failed(lifetime, failure, Again::Never) {
            return Ok(());
        }
        self.line(&Line::LifetimeFail(Box::new(LifetimeFail {
            key: self.key(lifetime),
            lifetime: words::head(lifetime.as_str(), ID_MAX),
            phase: failure.phase.map(words::phase),
            same_as: None,
            at: at(failure),
            why: Some(failure::why(failure, false)),
            shown: None,
            detail: Detail::default(),
            blocked: Vec::new(),
            blocked_omitted: 0,
        })))
    }

    /// A test's line in `aye show`'s list.
    pub(super) fn list_line(&mut self, result: &TestResult) -> io::Result<()> {
        let why = if result.outcome.is_bad() && result.cause.is_none() {
            match result.failure() {
                Some(failure) => Some(failure::why(failure, true)),
                None => failure::bare_why(&result.output),
            }
        } else {
            None
        };
        self.line(&Line::Test {
            key: Some(self.key(&result.id)),
            id: words::head(result.id.as_str(), ID_MAX),
            outcome: result.outcome.as_str(),
            ms: words::millis(result.duration),
            why,
            reason: result
                .reason
                .as_deref()
                .map(|reason| words::head(reason, REASON_MAX)),
            cause: result
                .cause
                .as_ref()
                .map(|cause| words::head(cause.as_str(), ID_MAX)),
        })
    }

    /// A module skipped while collecting, in `aye show`'s list: a `test`
    /// line, named after its file, as pytest counts it as a skipped item.
    pub(super) fn module_skip_line(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        self.line(&Line::Test {
            key: None,
            id: words::head(diagnostic.path.as_str(), ID_MAX),
            outcome: Outcome::Skipped.as_str(),
            ms: 0,
            why: None,
            reason: Some(words::head(&diagnostic.message, REASON_MAX)),
            cause: None,
        })
    }
}
