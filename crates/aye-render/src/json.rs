//! `--output json`: JSON lines, for programs and coding agents.
//!
//! No colour, no banners. Each line is one JSON object whose `type` says
//! what it is. Keys are stable: new keys may appear, existing ones keep
//! their meaning. Optional keys are left out rather than set to null, an
//! absent count is zero, and an absent list is empty.
//!
//! # The levels
//!
//! The [`Level`] (`-v` count minus `-q` count, clamped to -1..=3) says
//! which lines are written and how much a bad outcome's line holds (its
//! level of detail, L1 to L3, below), each level adding to the one below:
//! - **-1** (`-q`): the `summary` alone, without `failures` or `more`.
//! - **0**: the `summary`, with the keys of what failed (`failures`) and
//!   how to see more (`more`); in a collect-only run, a `case` line per
//!   case before it.
//! - **1** (`-v`): a line per bad outcome at L1 (`fail`, `lifetime_fail`),
//!   each error diagnostic (`collection_error`) and failed worker
//!   (`worker_error`).
//! - **2** (`-vv`): bad outcomes at L2; a `diagnostic` line per warning
//!   and info.
//! - **3** (`-vvv`): bad outcomes at L3. (The run's raw log is `aye show
//!   --log`.)
//!
//! # A bad outcome's line
//!
//! `fail` is a test with a bad outcome, or a failed sub-outcome of it
//! (`sub`); `lifetime_fail` a lifetime whose setup or teardown failed. A
//! sub-outcome's line is written when its test finishes; a test that
//! failed only through sub-outcomes gets no line of its own (the first of
//! theirs carries its `output` and `rerun`), and the test's own failures
//! that repeat theirs (the unittest and doctest adapters fail the test
//! with them) aren't reported again. A test's fourth bad sub-outcome and
//! later ones are at L1 below level 3.
//!
//! **L1**: `key` (the test's, or the lifetime's: see [`Keys`]; `aye show
//! <key>` shows it again), `id`, `sub`, `outcome`, `phase` (`setup` or
//! `teardown`; absent for the test itself), `at` (`path:line` where it was
//! raised, or the innermost project frame when that was outside the
//! project), `why` (the one line saying why: see [`failure::why`]; a bad
//! outcome without a failure has the last line its test wrote to stderr,
//! else stdout). Below level 3, a failure whose signature an earlier one
//! had is `same_as` (the earlier one's key) instead of `why` and the rest,
//! with `why` too when its message differs. A `lifetime_fail` has `key`,
//! `lifetime`, `phase`, `at`, `why`, and `blocked` (the keys of the tests
//! it kept from running, at most 20, then `blocked_omitted`); it is
//! written once, after the tests it blocked (under `maxfail` the engine
//! reports it before them, and they follow as `fail` lines naming it in
//! `cause`, as does a test whose lifetime never reported).
//!
//! **L2** adds: `code` (the failing line, without its indentation),
//! `cols` (the failing expression's span in it, 0-based characters),
//! `before` (the two lines before it); `diff`: the structured comparison,
//! `{kind, summary, same, differs: [{at, expected, actual}], omitted}`
//! (`expected` and `actual` when the expected side is known, else `left`
//! and `right`, with `left_source` and `right_source` beside `diff`; an
//! entry only one side has leaves the other out; at most 20 entries, the
//! rest counted in `omitted`), or without one the framework's explanation
//! lines (none when they only restate the operands); the operands
//! themselves when there is no structured comparison and they add to the
//! message, or when it has no entries (`expected` and `actual`, else
//! `left` and `right`); `message` (the whole message, at most 10 lines
//! after the first, when it spans lines and neither a structured
//! comparison nor `expected` and `actual` say it already; with them, the
//! message the test gave, if it gave one); `hint`; `fixture` (the one that
//! broke); `setup` (`["price_list:session", "cart:function"]`); `history`
//! (`{previous, passed, failed, since}`: how many earlier runs the history
//! holds, how many passed and failed, and `r78`, the run its current
//! streak of bad outcomes began in; zero counts left out); `also` (later
//! failures of the same test, `phase: message`, at most 3); `rerun`; `ms`.
//!
//! **L3** adds: `frames` (`path:line in function: source` for each
//! project frame, and `12 frames in requests` for each run of other
//! frames), `output` (`{stdout, stderr, log}`: each stream's last 50
//! lines, at most 4000 characters, a cut marked `[N earlier lines
//! omitted]`), `locals` (the innermost frame's, `{name: value}`).
//!
//! # Other lines
//!
//! - `case`: `id`, `partial` (true when the id stands for tests whose
//!   parameters are only known at runtime). Only when collecting without
//!   running, from level 0.
//! - `collection_error`: `path`, `line`, `code`, `error`: an error
//!   diagnostic, reported once however many batches repeat it.
//! - `worker_error`: `worker`, `error`.
//! - `diagnostic`: a warning or info: `severity`, `path`, `line`, `code`,
//!   `message`; once however many batches repeat it.
//! - `summary`, last: `run` (`r81`, when the run is stored), `exit`,
//!   `tests` (how many were selected), the non-zero counts (`failed`,
//!   `errors`, `passed`, `skipped`, `expected_failure`, `unexpected_pass`,
//!   `deselected`), `s` (seconds, two decimals), `stopped` when the run
//!   stopped before every selected test ran (`maxfail`, `interrupted` or
//!   `framework`), `failures` (the keys of every bad outcome, tests and
//!   lifetimes, each once in the order of the log, at most 50, then
//!   `failures_omitted`) and `more` (`aye show <key> --output json`) when
//!   something failed. `skipped` counts a module skipped while collecting
//!   as one, as pytest does. The run also fails on a failed lifetime that
//!   kept no test from running (a failed `tearDownClass` or
//!   `tearDownModule`, a cleanup that raised, a failed setup whose tests
//!   the run stopped before), which no test's count holds:
//!   `setup_error`, `teardown_error` and `lifetime_error` (no phase) count
//!   them, once per lifetime.
//! - `stale` (`aye show`, before every other line, at every level but
//!   `-q`): `path`, a file of a test shown that changed since the stored
//!   run, and `rerun` (`aye <path>`); `gone` instead of `rerun` when the
//!   file no longer exists.
//!
//! # `aye show`
//!
//! [`JsonLines::for_show`]: [`ShowMode::List`] writes, whatever the level
//! from 0, a `test` line per `test_finished` fed (`key`, `id`, `outcome`,
//! `ms`, and `why` for a bad one, `reason` for one skipped or expected to
//! fail, `cause` for one a failed lifetime kept from running; one per
//! module skipped while collecting, its `id` the file's path, without a
//! key), an L1
//! `lifetime_fail` line per failed lifetime, once each, the
//! `collection_error` and `worker_error` lines a run writes, then the
//! summary; [`ShowMode::Details`] writes what a run would of the records
//! fed, a bad outcome's line adding `run` (`r81`), `python` (its worker's,
//! when its `worker_started` is fed) and `stale` (whether its file changed
//! since the run). Either way the keys are the whole log's.
//!
//! Limits, in characters unless stated, with cuts marked by `…`: ids and
//! lifetimes 1000; `why` 160; paths and `at` 500; `code` and `before` 300
//! each; `diff`'s `summary` 200, its values 200 each, its explanation
//! lines 30 of 200; operands 500 each, their sources 200; `message` 500;
//! `hint` 500; `fixture` 200; `setup` 30 entries of 200; `also` 3 entries
//! of 500; `rerun` 1000; `frames` 300 each; `locals` 200 each; a
//! collection error's `error` and a diagnostic's `message` 1000; `reason`
//! 500; a worker's `error` its last 2000.

use std::collections::HashSet;
use std::io::{self, Write};
use std::time::Duration;

use aye_events::{
    Certainty, Diagnostic, DiagnosticCode, Event, Failure, LifetimeId, Outcome, Record, Severity,
    TestId, TestResult, WorkerIndex,
};
use aye_stream::{
    Again, BlockedTests, Consumer, FirstBySignature, HeldSub, Keys, Left, Noted, Repeats, RunFacts,
    SubOutcomes, Subject, Tally,
};

use crate::{Level, ShowContext, ShowMode, failure, rerun, words};

mod detail;
mod limits;
mod schema;
mod show;
mod summary;

use detail::output_tails;
use limits::{
    BLOCKED, COLLECTION_ERROR_MAX, DIAGNOSTIC_CODE_MAX, ID_MAX, PATH_MAX, SUB_LINES,
    WORKER_ERROR_MAX,
};
use schema::{Detail, Fail, LifetimeFail, Line};

/// `-q`: the summary alone.
const QUIET: i8 = -1;
/// `-v`: a line per bad outcome, at L1.
const LINES: i8 = 1;
/// `-vv`: L2, and every diagnostic.
const DETAIL: i8 = 2;
/// `-vvv`: L3.
const ALL: i8 = 3;

pub struct JsonLines<W: Write> {
    out: W,
    /// `-v` count minus `-q` count, clamped to -1..=3.
    verbosity: i8,
    /// From `run_started`, `collection_finished` and each
    /// `worker_started`.
    facts: RunFacts,
    /// The first bad outcome reported with each signature: its key and
    /// message.
    signatures: FirstBySignature<(String, String)>,
    /// Tests blocked by a lifetime whose `lifetime_finished` has not
    /// arrived, with their keys; and the lifetimes reported, each again
    /// with a signature other than the one it was last reported with.
    blocked: BlockedTests<(TestId, String)>,
    /// Bad sub-outcomes of tests that haven't finished, held so that the
    /// first can carry the test's output and rerun.
    subs: SubOutcomes,
    /// Diagnostics reported, by severity, code, path and message, so that
    /// one every batch repeats (a framework rejecting its arguments) is
    /// reported once.
    diagnostics: HashSet<(&'static str, DiagnosticCode, String, String)>,
    /// What the summary counts beside the tests' counts: modules skipped
    /// while collecting, failed lifetimes that kept no test from running.
    tally: Tally,
    keys: Keys,
    /// The keys of every bad outcome, each once, in order.
    failures: Vec<String>,
    failures_seen: HashSet<String>,
    /// What `aye show` asks for; `None` in a run.
    show: Option<ShowMode>,
    context: ShowContext,
}

/// `at`: where the failure is shown.
fn at(failure: &Failure) -> Option<String> {
    failure::origin(failure)
        .map(|(path, line)| words::head(&format!("{path}:{line}"), PATH_MAX).into_owned())
}

impl<W: Write> JsonLines<W> {
    /// At `level` (see the module's docs).
    pub fn new(out: W, level: Level) -> Self {
        Self {
            out,
            verbosity: level.count(),
            facts: RunFacts::new(),
            signatures: FirstBySignature::new(),
            blocked: BlockedTests::new(),
            subs: SubOutcomes::new(),
            diagnostics: HashSet::new(),
            tally: Tally::default(),
            keys: Keys::new(),
            failures: Vec::new(),
            failures_seen: HashSet::new(),
            show: None,
            context: ShowContext::default(),
        }
    }

    pub fn into_inner(self) -> W {
        self.out
    }

    /// Bad outcomes get lines.
    const fn lines(&self) -> bool {
        self.verbosity >= LINES
    }

    /// Their lines hold L2.
    const fn detailed(&self) -> bool {
        self.verbosity >= DETAIL
    }

    /// Their lines hold L3, and none is grouped with an earlier one.
    const fn all(&self) -> bool {
        self.verbosity >= ALL
    }

    fn line(&mut self, line: &Line<'_>) -> io::Result<()> {
        serde_json::to_writer(&mut self.out, line)?;
        self.out.write_all(b"\n")
    }

    /// The key of a test or a lifetime.
    fn key<'s>(&self, subject: impl Into<Subject<'s>>) -> String {
        self.keys.shown(subject).to_string()
    }

    /// The first reported with this failure's signature (its key, or a
    /// sub-outcome's `<key> › <name>`), and its message; records `key` as
    /// the first when there is none yet (none is grouped at L3).
    fn first_with(&mut self, failure: &Failure, key: &str) -> Option<(String, String)> {
        let signature = failure.signature.as_ref()?;
        if self.all() {
            return None;
        }
        self.signatures
            .first_or_keep(signature, || {
                (key.to_owned(), failure::message(failure).to_owned())
            })
            .cloned()
    }

    fn test(&mut self, result: &TestResult, worker: Option<WorkerIndex>) -> io::Result<()> {
        // A test that failed only through its sub-outcomes gets no line of
        // its own: the first of theirs carries its output and rerun. What
        // they reported (the unittest and doctest adapters fail the test
        // with the same failures) isn't reported again.
        let subs = self.subs.take(&result.id);
        let own: Vec<Failure> = aye_stream::own_failures(result, &subs, Repeats::SameOrBare)
            .cloned()
            .collect();
        let only_subs =
            !subs.is_empty() && result.outcome.is_bad() && result.cause.is_none() && own.is_empty();
        for (at, sub) in subs.iter().enumerate() {
            let carried = (only_subs && at == 0).then_some(result);
            self.sub(&result.id, sub, carried, at >= SUB_LINES && !self.all())?;
        }
        if !only_subs && result.outcome.is_bad() {
            self.own_failures(result, &own, worker)?;
        }
        Ok(())
    }

    /// The line of a test with a bad outcome, `own` its failures that its
    /// sub-outcomes didn't report; held when a lifetime that hasn't
    /// reported kept it from running.
    fn own_failures(
        &mut self,
        result: &TestResult,
        own: &[Failure],
        worker: Option<WorkerIndex>,
    ) -> io::Result<()> {
        let key = self.key(&result.id);
        if let Some(lifetime) = &result.cause {
            if self.blocked.has_failed(lifetime) {
                return self.caused(&result.id, key, result.outcome, lifetime);
            }
            self.blocked.hold(lifetime, (result.id.clone(), key));
            return Ok(());
        }
        let Some(first) = own.first() else {
            let mut fail = Fail::new(key, &result.id, result.outcome);
            fail.why = failure::bare_why(&result.output);
            fail.shown = self.shown(result.id.path().as_str(), worker);
            self.carry(&mut fail.detail, result);
            if self.detailed() {
                fail.detail.ms = Some(words::millis(result.duration));
                if !self.all() {
                    fail.detail.output = output_tails(&result.output);
                }
            }
            return self.line(&Line::Fail(Box::new(fail)));
        };
        let mut fail = Fail::new(key.clone(), &result.id, result.outcome);
        fail.phase = failure::notable_phase(first);
        fail.at = at(first);
        fail.shown = self.shown(result.id.path().as_str(), worker);
        if let Some((same, message)) = self.first_with(first, &key) {
            fail.same_as = Some(same);
            if message != failure::message(first) {
                fail.why = Some(failure::why(first, true));
            }
        } else {
            fail.why = Some(failure::why(first, true));
            let rerun = rerun::rerun(&result.id, result.reproduce.as_ref());
            fail.detail = self.detail(first, &own[1..], Some(result), result.duration, Some(rerun));
        }
        self.line(&Line::Fail(Box::new(fail)))
    }

    /// A bad sub-outcome's line; `carried`, its test's output and rerun.
    /// A `brief` one (a test's fourth bad sub-outcome and later, below
    /// L3, as a doctest whose every example fails after a broken first one
    /// gives) is at L1.
    fn sub(
        &mut self,
        id: &TestId,
        sub: &HeldSub,
        carried: Option<&TestResult>,
        brief: bool,
    ) -> io::Result<()> {
        let key = self.key(id);
        let mut fail = Fail::new(key.clone(), id, sub.outcome);
        fail.sub = Some(words::head(&sub.name, ID_MAX));
        fail.shown = self.shown(id.path().as_str(), sub.worker);
        if let Some(failure) = &sub.failure {
            fail.phase = failure::notable_phase(failure);
            fail.at = at(failure);
            // A sub-outcome is named by its test's key and its name.
            let label = format!("{key} › {}", sub.name);
            if let Some((same, message)) = self.first_with(failure, &label) {
                fail.same_as = Some(same);
                if message != failure::message(failure) {
                    fail.why = Some(failure::why(failure, false));
                }
            } else {
                fail.why = Some(failure::why(failure, false));
                if !brief {
                    let rerun = rerun::rerun(id, carried.and_then(|r| r.reproduce.as_ref()));
                    fail.detail = self.detail(failure, &[], None, sub.duration, Some(rerun));
                }
            }
        }
        if let Some(result) = carried
            && fail.same_as.is_none()
            && !brief
        {
            self.carry(&mut fail.detail, result);
        }
        self.line(&Line::Fail(Box::new(fail)))
    }

    /// Bad sub-outcomes whose test never finished (its worker was killed).
    fn flush_held(&mut self) -> io::Result<()> {
        for (id, subs) in self.subs.take_left() {
            if !self.lines() {
                continue;
            }
            for (at, sub) in subs.iter().enumerate() {
                self.sub(&id, sub, None, at >= SUB_LINES && !self.all())?;
            }
        }
        Ok(())
    }

    fn caused(
        &mut self,
        id: &TestId,
        key: String,
        outcome: Outcome,
        lifetime: &LifetimeId,
    ) -> io::Result<()> {
        let mut fail = Fail::new(key, id, outcome);
        fail.cause = Some(words::head(lifetime.as_str(), ID_MAX));
        self.line(&Line::Fail(Box::new(fail)))
    }

    fn lifetime(
        &mut self,
        lifetime: &LifetimeId,
        failure: Option<&Failure>,
        duration: Duration,
        worker: Option<WorkerIndex>,
    ) -> io::Result<()> {
        let blocked = self.blocked.take(lifetime);
        let fresh = failure.filter(|failure| {
            self.blocked
                .failed(lifetime, failure, Again::OtherSignature)
        });
        let Some(failure) = fresh else {
            for (id, key) in blocked {
                self.caused(&id, key, Outcome::Error, lifetime)?;
            }
            return Ok(());
        };
        let key = self.key(lifetime);
        let mut line = LifetimeFail {
            key: key.clone(),
            lifetime: words::head(lifetime.as_str(), ID_MAX),
            phase: failure.phase.map(words::phase),
            same_as: None,
            at: at(failure),
            why: None,
            shown: self.shown(lifetime.file().as_str(), worker),
            detail: Detail::default(),
            blocked: blocked
                .iter()
                .take(BLOCKED)
                .map(|(_, key)| key.clone())
                .collect(),
            blocked_omitted: blocked.len().saturating_sub(BLOCKED),
        };
        if let Some((same, message)) = self.first_with(failure, &key) {
            line.same_as = Some(same);
            if message != failure::message(failure) {
                line.why = Some(failure::why(failure, false));
            }
        } else {
            line.why = Some(failure::why(failure, false));
            let rerun = blocked.first().map(|(id, _)| rerun::rerun(id, None));
            line.detail = self.detail(failure, &[], None, duration, rerun);
        }
        self.line(&Line::LifetimeFail(Box::new(line)))
    }

    /// Tests blocked by lifetimes that never reported a failure.
    fn flush_pending(&mut self) -> io::Result<()> {
        for (lifetime, ids) in self.blocked.take_left(Left::Lifetime) {
            for (id, key) in ids {
                self.caused(&id, key, Outcome::Error, &lifetime)?;
            }
        }
        Ok(())
    }

    /// An error diagnostic's `collection_error`; from level 2, a warning's
    /// or info's `diagnostic`. Each once.
    fn diagnostic(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        let severity = diagnostic.severity.as_str();
        if (diagnostic.severity != Severity::Error && !self.detailed())
            || !self.diagnostics.insert((
                severity,
                diagnostic.code.clone(),
                diagnostic.path.to_string(),
                diagnostic.message.clone(),
            ))
        {
            return Ok(());
        }
        let path = words::head(diagnostic.path.as_str(), PATH_MAX);
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        let code = words::head(diagnostic.code.as_str(), DIAGNOSTIC_CODE_MAX);
        let message = words::head(&diagnostic.message, COLLECTION_ERROR_MAX);
        if diagnostic.severity == Severity::Error {
            return self.line(&Line::CollectionError {
                path,
                line,
                code,
                error: message,
            });
        }
        self.line(&Line::Diagnostic {
            severity,
            path,
            line,
            code,
            message,
        })
    }

    fn worker_error(&mut self, worker: Option<WorkerIndex>, error: &str) -> io::Result<()> {
        self.line(&Line::WorkerError {
            worker,
            error: words::tail(error.trim_end(), WORKER_ERROR_MAX),
        })
    }
}

impl<W: Write> Consumer for JsonLines<W> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        let noted = self.tally.count(record);
        self.keys.note(record);
        self.fold(record);
        if let Event::RunFinished {
            exit_code,
            counts,
            duration,
            stopped,
        } = &record.event
        {
            return self.summary(*exit_code, counts, *duration, stopped.as_ref());
        }
        if self.verbosity <= QUIET {
            return Ok(());
        }
        let worker = record.envelope.worker;
        if self.show == Some(ShowMode::List) {
            return match &record.event {
                Event::TestFinished(result) => self.list_line(result),
                Event::Diagnostic(diagnostic) => match noted {
                    Some(Noted::ModuleSkip(_)) => self.module_skip_line(diagnostic),
                    _ if diagnostic.severity == Severity::Error => self.diagnostic(diagnostic),
                    _ => Ok(()),
                },
                Event::LifetimeFinished {
                    lifetime,
                    failure: Some(failure),
                    ..
                } => self.lifetime_list_line(lifetime, failure),
                Event::WorkerFinished {
                    error: Some(error), ..
                } => self.worker_error(worker, error),
                _ => Ok(()),
            };
        }
        match &record.event {
            Event::CaseCollected(case) if self.facts.collect_only => self.line(&Line::Case {
                id: words::head(case.id.as_str(), ID_MAX),
                partial: case.certainty == Certainty::Partial,
            }),
            _ if !self.lines() => Ok(()),
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic),
            Event::LifetimeFinished {
                lifetime,
                failure,
                duration,
            } => self.lifetime(lifetime, failure.as_ref(), *duration, worker),
            Event::SubOutcome { outcome, .. } if outcome.is_bad() => {
                self.subs.hold(record);
                Ok(())
            }
            Event::TestFinished(result) => self.test(result, worker),
            Event::WorkerFinished {
                error: Some(error), ..
            } => self.worker_error(worker, error),
            _ => Ok(()),
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.lines() {
            self.flush_held()?;
            self.flush_pending()?;
        }
        self.out.flush()
    }
}
