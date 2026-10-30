//! The closing lines: warnings and skips grouped, why the run stopped, the
//! summary line ([`summary_words`] words its counts) and the details hint.

use std::io::{self, Write};
use std::time::Duration;

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Counts, DiagnosticCode, ExitCode, StopReason};
use aye_stream::Extra;

use super::Text;
use super::limits::{DETAILS, GROUPS, REASON_MAX};
use super::out::Group;
use super::stylesheet::{BOLD, DIM, GREEN, KEY, RED_BOLD, YELLOW, YELLOW_BOLD};
use crate::{ShowMode, words};

/// How the text summary line colours a count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Bad,
    Good,
    Warn,
    Dim,
}

/// One count of the text summary line: how many, its words for one and
/// for more, and its tone.
pub(crate) type Part = (u32, &'static str, &'static str, Tone);

/// The non-zero counts of the text summary line, in its order: the tests'
/// outcomes (`skipped` with the modules skipped while collecting), then
/// the failed lifetimes and the error diagnostics.
pub(crate) fn parts(counts: &Counts, extra: &Extra) -> Vec<Part> {
    [
        (counts.failed, "failed", "failed", Tone::Bad),
        (counts.error, "error", "errors", Tone::Bad),
        (counts.passed, "passed", "passed", Tone::Good),
        (
            counts.skipped + extra.skipped_modules,
            "skipped",
            "skipped",
            Tone::Warn,
        ),
        (
            counts.expected_failure,
            "expected failure",
            "expected failures",
            Tone::Warn,
        ),
        (
            counts.unexpected_pass,
            "unexpected pass",
            "unexpected passes",
            Tone::Warn,
        ),
        (counts.deselected, "deselected", "deselected", Tone::Dim),
        (extra.setup_error, "setup error", "setup errors", Tone::Bad),
        (
            extra.teardown_error,
            "teardown error",
            "teardown errors",
            Tone::Bad,
        ),
        (
            extra.lifetime_error,
            "lifetime error",
            "lifetime errors",
            Tone::Bad,
        ),
        (
            extra.collection_error,
            "collection error",
            "collection errors",
            Tone::Bad,
        ),
        (extra.usage_error, "usage error", "usage errors", Tone::Bad),
        (
            extra.internal_error,
            "internal error",
            "internal errors",
            Tone::Bad,
        ),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .collect()
}

/// The counts as the text summary line words them, without colour
/// (`1 failed`, `2 passed`, `1 teardown error`): the non-zero ones, in its
/// order.
pub fn summary_words(counts: &Counts, extra: &Extra) -> Vec<String> {
    parts(counts, extra)
        .into_iter()
        .map(|(n, one, many, _)| words::plural(n, one, many))
        .collect()
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// A skip's reason, for the grouped line.
    pub(super) fn skipped(&mut self, reason: &str) {
        let reason = reason.lines().next().unwrap_or_default().trim();
        let reason = if reason.is_empty() {
            "no reason given"
        } else {
            reason
        };
        match self.skips.iter_mut().find(|(seen, _)| seen == reason) {
            Some((_, count)) => *count += 1,
            None => self.skips.push((reason.to_owned(), 1)),
        }
    }

    pub(super) fn run_finished(
        &mut self,
        exit_code: ExitCode,
        counts: &Counts,
        duration: Duration,
        stopped: Option<&StopReason>,
    ) -> io::Result<()> {
        self.flush_collected()?;
        self.flush_subs()?;
        self.flush_pending(None)?;
        if !self.quiet() && self.full() {
            self.warnings_in_full()?;
        }
        // `aye show`'s details of tests are the tests alone: the counts of
        // what was asked for say nothing more.
        let tests_shown =
            self.show == Some(ShowMode::Details) && !self.quiet() && counts.total() > 0;
        let summary = !(tests_shown || (self.facts.collect_only && exit_code == ExitCode::Ok));
        let grouped = !self.quiet()
            && ((!self.full() && !self.warnings.is_empty())
                || (self.show.is_none() && !self.facts.collect_only && !self.skips.is_empty()));
        if summary || grouped || stopped.is_some() {
            self.out.paragraph()?;
        }
        if grouped {
            self.grouped()?;
        }
        if let Some(stopped) = stopped {
            self.stopped_line(stopped, exit_code, counts)?;
        }
        if summary {
            self.summary_line(exit_code, counts, duration)?;
        }
        if !self.quiet() {
            self.details_line()?;
        }
        self.group = Group::Lines;
        self.out.flush()
    }

    /// Below `-vv`: the warnings by code, and the skips by reason, on a
    /// line each.
    fn grouped(&mut self) -> io::Result<()> {
        if !self.full() && !self.warnings.is_empty() {
            let mut codes: Vec<(&DiagnosticCode, usize)> = Vec::new();
            for warning in &self.warnings {
                match codes.iter_mut().find(|(code, _)| **code == warning.code) {
                    Some((_, count)) => *count += 1,
                    None => codes.push((&warning.code, 1)),
                }
            }
            let mut words: Vec<String> = codes
                .iter()
                .take(GROUPS)
                .map(|(code, count)| format!("{count} of {code}"))
                .collect();
            if codes.len() > GROUPS {
                words.push("…".to_owned());
            }
            writeln!(self.out, "{YELLOW}warnings{YELLOW:#}  {}", words.join(", "))?;
        }
        if self.show.is_none() && !self.facts.collect_only && !self.skips.is_empty() {
            let mut skips = self.skips.clone();
            // Most first; a stable sort keeps ties in the order they came.
            skips.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let mut words: Vec<String> = skips
                .iter()
                .take(GROUPS)
                .map(|(reason, count)| format!("{} ({count})", words::head(reason, REASON_MAX)))
                .collect();
            if skips.len() > GROUPS {
                words.push("…".to_owned());
            }
            writeln!(self.out, "{YELLOW}skipped{YELLOW:#}  {}", words.join(" · "))?;
        }
        Ok(())
    }

    /// Why the run stopped before every selected test ran, unless the
    /// summary line's verdict says it already.
    fn stopped_line(
        &mut self,
        stopped: &StopReason,
        exit_code: ExitCode,
        counts: &Counts,
    ) -> io::Result<()> {
        let why = match stopped {
            StopReason::Maxfail => format!(
                "stopped after {} (-x/--maxfail)",
                words::plural(counts.bad(), "failure", "failures")
            ),
            StopReason::Interrupted if exit_code == ExitCode::Interrupted => return Ok(()),
            StopReason::Interrupted => "interrupted".to_owned(),
            StopReason::Framework => "stopped early by the framework".to_owned(),
            StopReason::Other(other) => format!("stopped early: {other}"),
        };
        writeln!(self.out, "{YELLOW}{why}{YELLOW:#}")
    }

    /// From `-vv`: every warning, in full.
    fn warnings_in_full(&mut self) -> io::Result<()> {
        let warnings = std::mem::take(&mut self.warnings);
        if warnings.is_empty() {
            return Ok(());
        }
        self.out.paragraph()?;
        for warning in &warnings {
            self.diagnostic_line(warning)?;
        }
        Ok(())
    }

    /// `✗ 2 failed · ! 3 errors · ✓ 11 passed · 1 skipped   0.12 s`.
    fn summary_line(
        &mut self,
        exit_code: ExitCode,
        counts: &Counts,
        duration: Duration,
    ) -> io::Result<()> {
        // A failed teardown blocks no test, so the counts don't hold it,
        // though the run fails on it: the line says so, as it counts the
        // modules skipped while collecting and the error diagnostics.
        let parts: Vec<String> = parts(counts, &self.tally.extra())
            .into_iter()
            .map(|(n, one, many, tone)| {
                let words = words::plural(n, one, many);
                match tone {
                    Tone::Bad if one == "failed" => format!("{RED_BOLD}✗ {words}{RED_BOLD:#}"),
                    Tone::Bad => format!("{YELLOW_BOLD}! {words}{YELLOW_BOLD:#}"),
                    Tone::Good => format!("{GREEN}✓ {words}{GREEN:#}"),
                    Tone::Warn => format!("{YELLOW}{words}{YELLOW:#}"),
                    Tone::Dim => format!("{DIM}{words}{DIM:#}"),
                }
            })
            .collect();
        let none = if self.facts.collect_only {
            "no tests collected"
        } else if self.show.is_some() {
            "no tests shown"
        } else {
            "no tests ran"
        };
        let verdict = match exit_code {
            ExitCode::Ok | ExitCode::TestsFailed if parts.is_empty() => {
                Some(format!("{BOLD}{none}{BOLD:#}"))
            }
            ExitCode::Ok | ExitCode::TestsFailed => None,
            ExitCode::Interrupted => Some(format!("{RED_BOLD}interrupted{RED_BOLD:#}")),
            ExitCode::InternalError => Some(format!("{RED_BOLD}internal error{RED_BOLD:#}")),
            ExitCode::UsageError => Some(format!("{RED_BOLD}usage error{RED_BOLD:#}")),
            ExitCode::NoTests => Some(format!("{YELLOW}{none}{YELLOW:#}")),
        };
        let line: Vec<String> = verdict.into_iter().chain(parts).collect();
        writeln!(
            self.out,
            "{}   {DIM}{}{DIM:#}",
            line.join(" · "),
            words::seconds(duration)
        )
    }

    /// `details  aye show 3fae6 4d4f6 c3d4e  ·  every failure: aye show
    /// --failed`, in a run where something failed.
    fn details_line(&mut self) -> io::Result<()> {
        if self.show.is_some() || self.facts.collect_only || self.bad_keys.is_empty() {
            return Ok(());
        }
        let source = if self.details.is_empty() {
            &self.bad_keys
        } else {
            &self.details
        };
        let keys: Vec<String> = source
            .iter()
            .take(DETAILS)
            .map(|key| format!("{KEY}{key}{KEY:#}"))
            .collect();
        write!(self.out, "{DIM}details{DIM:#}  aye show {}", keys.join(" "))?;
        if self.bad_keys.len() > 1 {
            write!(
                self.out,
                "  {DIM}·  every failure:{DIM:#} aye show --failed"
            )?;
        }
        writeln!(self.out)
    }
}
