//! The `summary` line, and what every level folds for it: the keys of
//! what failed.

use std::io::{self, Write};
use std::time::Duration;

use aye_events::{Counts, Event, ExitCode, Record, StopReason};
use aye_stream::Subject;

use super::JsonLines;
use super::limits::FAILURES;
use super::schema::{Line, Summary};
use crate::{ShowMode, words};

impl<W: Write> JsonLines<W> {
    /// The summary's `failures` counts the key of a test or a lifetime.
    fn note_bad<'s>(&mut self, subject: impl Into<Subject<'s>>) {
        let key = self.key(subject);
        if self.failures_seen.insert(key.clone()) {
            self.failures.push(key);
        }
    }

    pub(super) fn summary(
        &mut self,
        exit_code: ExitCode,
        counts: &Counts,
        duration: Duration,
        stopped: Option<&StopReason>,
    ) -> io::Result<()> {
        if self.lines() {
            self.flush_held()?;
            self.flush_pending()?;
        }
        let extra = self.tally.extra();
        let failures = if self.quiet() {
            Vec::new()
        } else {
            std::mem::take(&mut self.failures)
        };
        // Details are what `more` would show.
        let more = (!failures.is_empty() && self.show != Some(ShowMode::Details))
            .then_some("aye show <key> --output json");
        let omitted = failures.len().saturating_sub(FAILURES);
        self.line(&Line::Summary(Summary {
            run: self
                .context
                .run
                .or(self.facts.number)
                .map(|run| run.to_string()),
            exit: u8::from(exit_code),
            tests: self.facts.selected,
            failed: counts.failed,
            errors: counts.error,
            passed: counts.passed,
            skipped: counts.skipped + extra.skipped_modules,
            expected_failure: counts.expected_failure,
            unexpected_pass: counts.unexpected_pass,
            deselected: counts.deselected,
            setup_error: extra.setup_error,
            teardown_error: extra.teardown_error,
            lifetime_error: extra.lifetime_error,
            s: words::secs(duration),
            stopped: stopped.cloned(),
            failures: failures.into_iter().take(FAILURES).collect(),
            failures_omitted: omitted,
            more,
        }))?;
        self.out.flush()
    }

    /// What every level folds, whatever it writes: the run's number, the
    /// selected count, the workers' Pythons, and the keys of what failed.
    pub(super) fn fold(&mut self, record: &Record) {
        self.facts.note(record);
        match &record.event {
            Event::TestFinished(result) if result.outcome.is_bad() => {
                self.note_bad(&result.id);
            }
            Event::SubOutcome { id, outcome, .. } if outcome.is_bad() => {
                self.note_bad(id);
            }
            Event::LifetimeFinished {
                lifetime,
                failure: Some(_),
                ..
            } => self.note_bad(lifetime),
            _ => {}
        }
    }
}
