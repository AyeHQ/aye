//! Bad outcomes: a test's, its bad sub-outcomes', a failed lifetime's and
//! the tests it kept from running; which are shown, at what level, and
//! L1, their line ([`Bad`]).

use std::io::{self, Write};
use std::time::Duration;

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Failure, LifetimeId, Outcome, Phase, Record, TestId, TestResult, WorkerIndex};
use aye_stream::{Again, HeldSub, Left};

use super::Text;
use super::limits::{OUTPUT_LINES, SUB_BLOCKS};
use super::settings::Level;
use super::stylesheet::{BOLD, DIM, KEY, symbol};
use crate::{failure, rerun, words};

/// A test a failed lifetime kept from running, and its key.
pub(super) type NotRun = (TestId, String);

/// A bad outcome to show.
pub(super) struct Bad<'a> {
    pub(super) outcome: Outcome,
    pub(super) key: &'a str,
    /// The id, `<id> › <sub-outcome>`, or `setup of <lifetime>`.
    pub(super) name: &'a str,
    /// The file the name is in, as ids spell it.
    pub(super) path: &'a str,
    /// The first is shown; the rest each in a line.
    pub(super) failures: &'a [Failure],
    /// A test's own failures: a failed teardown says it passed.
    pub(super) test: bool,
    pub(super) duration: Option<Duration>,
    /// The test's result: its resources, history and output.
    pub(super) result: Option<&'a TestResult>,
    pub(super) rerun: Option<String>,
    pub(super) worker: Option<WorkerIndex>,
    /// Tests a failed lifetime kept from running.
    pub(super) not_run: &'a [NotRun],
    /// At most L1 (a test's fourth bad sub-outcome and later, below L3).
    pub(super) brief: bool,
    /// A sub-outcome's name: a later one failing the same way names it
    /// beside the key it shares.
    pub(super) sub: Option<&'a str>,
}

/// The indent of the line under a bad outcome's: past its symbol and key.
pub(super) fn indent(key: &str) -> String {
    " ".repeat(key.chars().count() + 4)
}

/// What failed of a lifetime: `setup of <lifetime>`, `teardown of
/// <lifetime>`, or the lifetime when its failure names no phase.
pub(super) fn lifetime_name(lifetime: &LifetimeId, failure: &Failure) -> String {
    match failure.phase {
        Some(Phase::Setup) => format!("setup of {lifetime}"),
        Some(Phase::Teardown) => format!("teardown of {lifetime}"),
        Some(Phase::Test) | None => lifetime.to_string(),
    }
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// A test a failed lifetime kept from running: listed under the
    /// lifetime's failure, which the engine reports after the tests it
    /// blocked; a line of its own when that failure was shown already.
    pub(super) fn blocked_test(
        &mut self,
        lifetime: &LifetimeId,
        result: &TestResult,
        key: String,
    ) -> io::Result<()> {
        if self.blocked.has_failed(lifetime) {
            return self.blocked_line(lifetime, &result.id, &key);
        }
        self.blocked.hold(lifetime, (result.id.clone(), key));
        Ok(())
    }

    fn blocked_line(&mut self, lifetime: &LifetimeId, id: &TestId, key: &str) -> io::Result<()> {
        self.lines()?;
        self.head(Outcome::Error, key, id.as_str(), None, false)?;
        let indent = indent(key);
        writeln!(
            self.out,
            "{indent}{DIM}not run: setup of {lifetime} failed{DIM:#}"
        )
    }

    /// Blocked tests whose lifetime reported no failure (or none yet, when
    /// `lifetime` is `None`): a line each.
    pub(super) fn flush_pending(&mut self, lifetime: Option<&LifetimeId>) -> io::Result<()> {
        let flushed = match lifetime {
            Some(lifetime) => vec![(lifetime.clone(), self.blocked.take(lifetime))],
            None => self.blocked.take_left(Left::Blocked),
        };
        for (held, ids) in flushed {
            for (id, key) in &ids {
                self.blocked_line(&held, id, key)?;
            }
        }
        Ok(())
    }

    /// What a test failed only by its sub-outcomes adds to theirs, as
    /// pytest's own section for it does ("contains 1 failed subtest",
    /// then its captured output): its output, from L3, and its rerun,
    /// from L2, when no block of theirs showed it. Nothing when it adds
    /// nothing.
    pub(super) fn after_subs(
        &mut self,
        result: &TestResult,
        key: &str,
        rerun: &str,
        count: usize,
        blocks: u32,
    ) -> io::Result<()> {
        let level = self.level();
        let has_output = level == Level::L3 && !result.output.is_empty();
        let has_rerun =
            level >= Level::L2 && blocks == 0 && rerun != rerun::rerun(&result.id, None);
        if !(has_output || has_rerun) {
            return Ok(());
        }
        // The adapter's own word for it ("contains 1 failed subtest"), or
        // how many were shown above.
        let note = match result
            .failure()
            .filter(|failure| aye_stream::is_bare(failure))
        {
            Some(failure) => failure::message(failure).to_owned(),
            None => format!("{} above", words::plural(count, "failure", "failures")),
        };
        self.block()?;
        self.head(result.outcome, key, result.id.as_str(), Some(&note), true)?;
        if has_output {
            self.output(&result.output, OUTPUT_LINES)?;
        }
        self.row("rerun", rerun)?;
        writeln!(self.out)
    }

    /// The bad sub-outcomes of test `id`, held until now, with `rerun`, its
    /// rerun, which arrives when it finishes: how many were shown as
    /// blocks.
    pub(super) fn show_subs(
        &mut self,
        id: &TestId,
        key: &str,
        subs: &[HeldSub],
        rerun: Option<&str>,
    ) -> io::Result<u32> {
        let mut blocks = 0;
        for sub in subs {
            let name = format!("{id} › {}", sub.name);
            let brief = blocks >= SUB_BLOCKS && self.level() < Level::L3;
            let fresh = self.bad(&Bad {
                outcome: sub.outcome,
                key,
                name: &name,
                path: id.path().as_str(),
                failures: sub.failure.as_slice(),
                test: false,
                duration: Some(sub.duration),
                result: None,
                rerun: Some(rerun.map_or_else(|| rerun::rerun(id, None), str::to_owned)),
                worker: sub.worker,
                not_run: &[],
                brief,
                sub: Some(&sub.name),
            })?;
            if fresh && !brief {
                blocks += 1;
            }
        }
        Ok(blocks)
    }

    /// Bad sub-outcomes whose test never finished (the run stopped), with
    /// the plain rerun.
    pub(super) fn flush_subs(&mut self) -> io::Result<()> {
        for (id, subs) in self.subs.take_left() {
            let key = self.key(&id);
            self.show_subs(&id, &key, &subs, None)?;
        }
        Ok(())
    }

    /// A bad sub-outcome of test `id` (a failed subtest or doctest
    /// example), `record`, held until its test finishes, whose rerun its
    /// block shows.
    pub(super) fn sub_outcome(&mut self, id: &TestId, record: &Record) {
        let key = self.key(id);
        self.note_bad(&key);
        self.subs.hold(record);
    }

    pub(super) fn lifetime(
        &mut self,
        lifetime: &LifetimeId,
        failure: &Failure,
        duration: Duration,
        worker: Option<WorkerIndex>,
    ) -> io::Result<()> {
        if !self.blocked.failed(lifetime, failure, Again::NewSignature) {
            return Ok(());
        }
        let not_run = self.blocked.take(lifetime);
        let key = self.key(lifetime);
        self.note_bad(&key);
        let name = lifetime_name(lifetime, failure);
        let rerun = not_run.first().map(|(first, _)| rerun::rerun(first, None));
        self.bad(&Bad {
            outcome: Outcome::Error,
            key: &key,
            name: &name,
            path: lifetime.file().as_str(),
            failures: std::slice::from_ref(failure),
            test: false,
            duration: Some(duration),
            result: None,
            rerun,
            worker,
            not_run: &not_run,
            brief: false,
            sub: None,
        })?;
        Ok(())
    }

    /// A bad outcome, at the level asked for; below L3, one naming the
    /// first with its signature when that was shown already. Whether it
    /// was shown on its own.
    pub(super) fn bad(&mut self, bad: &Bad<'_>) -> io::Result<bool> {
        let level = if bad.brief { Level::L1 } else { self.level() };
        if let Some(first) = bad.failures.first()
            && let Some(signature) = &first.signature
            && self.level() < Level::L3
        {
            let label = || {
                let label = match bad.sub {
                    Some(sub) => format!("{} › {sub}", bad.key),
                    None => bad.key.to_owned(),
                };
                (label, failure::message(first).to_owned())
            };
            if let Some((same, message)) = self.signatures.first_or_keep(signature, label).cloned()
            {
                self.same_as(bad, first, &same, &message)?;
                return Ok(false);
            }
        }
        if !self.details.iter().any(|key| key == bad.key) {
            self.details.push(bad.key.to_owned());
        }
        if level == Level::L1 {
            self.line_l1(bad)?;
        } else {
            self.block_l2(bad, level)?;
        }
        Ok(true)
    }

    /// `✗ 4d4f6  <name>` / `same as 3fae6`, and its own why when its
    /// message differs from the first's.
    fn same_as(
        &mut self,
        bad: &Bad<'_>,
        failure: &Failure,
        same: &str,
        message: &str,
    ) -> io::Result<()> {
        self.lines()?;
        self.head(bad.outcome, bad.key, bad.name, None, false)?;
        let indent = indent(bad.key);
        write!(self.out, "{indent}{DIM}same as{DIM:#} {KEY}{same}{KEY:#}")?;
        if message != failure::message(failure) {
            write!(self.out, " · {}", failure::why(failure, bad.test))?;
        }
        writeln!(self.out)?;
        self.kept(bad)
    }

    /// L1: two lines.
    fn line_l1(&mut self, bad: &Bad<'_>) -> io::Result<()> {
        self.lines()?;
        let first = bad.failures.first();
        let note = first
            .filter(|failure| failure.phase == Some(Phase::Setup) && failure.resource.is_none())
            .filter(|_| !bad.name.starts_with("setup of "))
            .map(|_| "setup");
        self.head(bad.outcome, bad.key, bad.name, note, false)?;
        let indent = indent(bad.key);
        if let Some(failure) = first {
            let why = failure::why(failure, bad.test);
            write!(self.out, "{indent}{why}")?;
            if let Some((path, line)) = failure::origin(failure) {
                write!(self.out, " at {}", self.location(path.as_str(), line))?;
            }
            writeln!(self.out)?;
        } else {
            let why = bad
                .result
                .and_then(|result| failure::bare_why(&result.output))
                .unwrap_or_else(|| words::outcome_words(bad.outcome).to_owned());
            writeln!(self.out, "{indent}{why}")?;
        }
        self.kept(bad)
    }

    /// Under a failed lifetime's line: how many tests it kept from running.
    fn kept(&mut self, bad: &Bad<'_>) -> io::Result<()> {
        if bad.not_run.is_empty() {
            return Ok(());
        }
        let indent = indent(bad.key);
        writeln!(
            self.out,
            "{indent}{DIM}kept {} from running:{DIM:#} aye show {KEY}{}{KEY:#}",
            words::plural(bad.not_run.len(), "test", "tests"),
            bad.key
        )
    }

    /// `✗ 3fae6  <name>`, the name bold in a block, a note after it.
    pub(super) fn head(
        &mut self,
        outcome: Outcome,
        key: &str,
        name: &str,
        note: Option<&str>,
        bold: bool,
    ) -> io::Result<()> {
        let (symbol, style) = symbol(outcome);
        write!(self.out, "{style}{symbol}{style:#} {KEY}{key}{KEY:#}  ")?;
        if bold {
            write!(self.out, "{BOLD}{name}{BOLD:#}")?;
        } else {
            write!(self.out, "{name}")?;
        }
        if let Some(note) = note {
            write!(self.out, " {DIM}({note}){DIM:#}")?;
        }
        writeln!(self.out)
    }
}
