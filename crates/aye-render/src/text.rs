//! `--output text`, the default: for people at a terminal. A short run,
//! with the details one command away (`aye show <key>`).
//!
//! # What a bad outcome shows
//!
//! A bad outcome is a test that failed or errored, a failed sub-outcome
//! (a subtest, a doctest example) shown under its test's key, or a failed
//! lifetime (`setup of class:tests/t.py::TestX`). Each is shown when it
//! finishes, at one of three levels of detail; the key is bold magenta,
//! `✗` (red) marks a failure and `!` (yellow) an error.
//!
//! **L1, the line** (two lines):
//! ```text
//! ✗ 3fae6  tests/test_cart.py::TestCart::test_summary
//!          cart.summary()['pear']: expected 3, actual 2 at tests/test_cart.py:34
//! ```
//! The key and the id (with ` (setup)` after it for a setup failure the
//! second line doesn't name), then the one line saying why (see
//! [`failure::why`]) and where: `at <path>:<line>`, the innermost project
//! frame, cyan, and on a live terminal with colour an OSC 8 hyperlink to
//! the file. A failure whose signature an earlier one had says `same as
//! <key>` instead (and its own why after ` · ` when its message differs).
//! A failed lifetime adds `kept 12 tests from running: aye show <key>`; a
//! test it kept from running that finishes after it is shown says `not
//! run: setup of <lifetime> failed`. A bad outcome without a failure says
//! the last line its test wrote to stderr (else stdout), or its outcome.
//!
//! **L2, the detail** (a block, set apart by blank lines):
//! ```text
//! ✗ 3fae6  tests/test_cart.py::TestCart::test_summary
//!   failed in the test · 2 ms · python 3.13.1 · run r81, 2 min ago · file unchanged
//!
//!   tests/test_cart.py:34
//!      33 │ cart.add("pear", price_list["pear"], 2)
//!   →  34 │ assert cart.summary() == {"apple": 4, "pear": 3}
//!         │        ^^^^^^^^^^^^^^
//!   dict differs in 1 of 2 keys
//!     ['pear']   expected 3   actual 2
//!
//!   hint     did you mean cart?
//!   fixture  cart
//!   setup    price_list (session) → cart (function)
//!   history  passed the previous 12 runs
//!   rerun    aye tests/test_cart.py::TestCart::test_summary
//!   more     aye show 3fae6 -vv   (every frame, output, locals)
//! ```
//! - The second line: the outcome and where it happened (`in setup`, `in
//!   the test`, `in teardown`), the duration; for `aye show` only, the
//!   Python of the test's worker (from its `worker_started`, when fed),
//!   the run (`run r81, 2 min ago`) and whether the test's file changed
//!   since (`file unchanged`, `file changed since`, `file gone`).
//! - The pointed code: the location, the two lines before the failing
//!   one, the failing line, and carets under the failing expression's
//!   span (`cols`; with colour, the span is also red and underlined).
//!   The lines keep their indentation relative to each other (the
//!   failing one indented by the frame's `indent` beyond the ones before).
//! - The comparison, structured (`compare`): its summary, then a row per
//!   difference, `expected` and `actual` when the expected side is known,
//!   else the operands' sources (cut to 30) as the columns' names, at most
//!   10 rows then `… N more` (an entry only one side has is `missing` on
//!   the other); text (`str`, `bytes`) as `strings differ at line 2,
//!   column 5` and the two lines, labelled. A message of its own (`assert
//!   a == b, "why"`) comes first. Without `compare`, the message, the
//!   operands when they add to it (`expected:`/`actual:`, else
//!   `left:`/`right:`), and the explanation's lines, at most 15. For an
//!   exception, its message and up to 10 more lines of it. Later failures
//!   of the same test follow, one line each (`also in teardown: …`).
//! - Rows: `hint`, `fixture` (the one that broke), `setup` (the setup
//!   chain), `history`, `not run` (a lifetime's blocked tests with their
//!   keys, 10 then `… N more`), `rerun`, and `more` (how to see L3).
//! - A bad outcome without a failure shows its captured output instead.
//!
//! A test's first three bad sub-outcomes get blocks, the rest their L1
//! lines; the test's own failures that only repeat theirs aren't shown
//! again, and the test gets a block of its own only for what theirs lack
//! (its output at L3, its rerun when no block showed it).
//!
//! **L3, everything**: L2 without `more`, and between the comparison and
//! the rows: `frames` (every project frame, `path:line in function` and
//! its pointed source; each run of other frames as `12 frames in
//! requests`), each captured stream (`stdout`, `stderr`, `log`: its last
//! 50 lines, a cut marked `… N earlier lines`), and `locals` (the
//! innermost frame's, `name = value`, one per line). No failure is grouped
//! with an earlier one, and nothing else is cut.
//!
//! # The levels
//!
//! The [`Level`] (`-v` count minus `-q` count, clamped to -1..=3), with
//! what each shows of a bad outcome in [`Ladder::TEXT`]:
//! - **-1** (`-q`): the summary line alone, with what never hides: error
//!   diagnostics, failed workers, why a run stopped early. A collect-only
//!   run still lists its cases.
//! - **0**: the header, each bad outcome at L1 as it finishes, then the
//!   closing lines: warnings grouped by code (`warnings  3 of
//!   plugin-not-loaded, 1 of class-has-init`, three codes then `…`),
//!   skips grouped by reason, most first (`skipped  needs GPU (8) · Linux
//!   only (4)`, three reasons then `…`; modules skipped while collecting
//!   count with them), why the run stopped, the summary line and the
//!   details hint.
//! - **1** (`-v`): bad outcomes at L2 instead.
//! - **2** (`-vv`): bad outcomes at L3; a line per test result that isn't
//!   bad (`✓ passed  tests/t.py::test_x  1 ms`, `  skipped tests/t.py::
//!   test_y  0 ms — no network`), the `collected` line, info diagnostics as
//!   they come, and every warning in full instead of their grouped line.
//! - **3** (`-vvv`): what the engine did: each worker and batch starting
//!   and finishing, each lifetime starting and finishing, each sub-outcome
//!   that didn't fail, and what each test that didn't fail printed.
//!
//! The header, dimmed, once, when the first worker starts (not when only
//! collecting): `aye · <root's directory> · python 3.13.1 · 15 tests · 1
//! worker`. The summary line: `✗ 2 failed · ! 3 errors · ✓ 11 passed · 1
//! skipped   0.12 s`, symbols paired with words and colours (skipped,
//! expected failures and unexpected passes as words alone; every kind of
//! error, `! 1 teardown error`, `! 1 collection error`, with `!`), after
//! the verdict when the run didn't end on its tests (`interrupted`,
//! `usage error`, `internal error`, `no tests ran`). The details hint,
//! when something failed: `details  aye show 3fae6 4d4f6 c3d4e  ·  every
//! failure: aye show --failed` (the first three keys of the bad outcomes
//! shown on their own; with one bad outcome alone, `details  aye show
//! 3fae6`). Error diagnostics are shown when they come
//! (`error[syntax-error] tests/t.py:3: …`), a failed worker as a block
//! with the last 10 lines of its error (all from `-vv`).
//!
//! # `aye show`
//!
//! [`Text::with_mode`]: [`ShowMode::List`] writes a line per result (`✗
//! failed  3fae6  tests/t.py::test_x  5 ms — why`, the key on bad ones; a
//! failed lifetime's once; a module skipped while collecting as a skipped
//! line named after its file), the error diagnostics and failed workers as
//! a run shows them, then the summary line. [`ShowMode::Details`] shows the
//! bad outcomes as a run does at the level asked for, with the `aye show`
//! parts of the second line, and a result line for each test that wasn't
//! bad, and no summary line after tests (only after what has no count: a
//! failed lifetime, a diagnostic). Neither has the header, grouped skips or
//! the details hint; with nothing to count and the run's exit code 0 or 1,
//! the summary says `no tests shown`. [`Text::stale`] warns of a file that
//! changed since the run, before anything else.

use std::collections::HashSet;
use std::io::{self, Write};
use std::time::Duration;

use anstream::AutoStream;
use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Certainty, Diagnostic, Event, Failure, Outcome, Record, TestResult, WorkerIndex};
use aye_stream::{
    BlockedTests, Consumer, FirstBySignature, Keys, Repeats, RunFacts, SubOutcomes, Subject, Tally,
};

use crate::{ColorChoice, Level, ShowContext, ShowMode, rerun, words};

mod bad;
mod block;
mod code;
mod diagnostics;
mod engine;
mod explain;
mod header;
mod limits;
mod out;
mod progress;
mod settings;
mod show;
mod stylesheet;
mod summary;

use bad::{Bad, NotRun};
use limits::OUTPUT_LINES;
use out::{Group, Out};
use stylesheet::{DIM, KEY, symbol};

pub use settings::TextOptions;
pub use summary::summary_words;

pub struct Text<W: RawStream> {
    out: Out<W>,
    verbosity: Level,
    live: bool,
    /// Locations are OSC 8 hyperlinks: a live terminal, with colour.
    links: bool,
    /// The width of the progress line on screen, 0 when none is.
    progress: usize,
    /// From `run_started`, `collection_finished` and each
    /// `worker_started`.
    facts: RunFacts,
    header_shown: bool,
    /// From `-vv`, the `collected` line, held to follow the header.
    collected: Option<String>,
    /// The first bad outcome shown with each signature: its key and
    /// message.
    signatures: FirstBySignature<(String, String)>,
    warnings: Vec<Diagnostic>,
    /// Skip reasons, with how many tests (or modules) each, in order.
    skips: Vec<(String, u32)>,
    /// Tests kept from running by a lifetime whose failure hasn't arrived
    /// (the engine reports it after them, except under `maxfail`), in the
    /// order they finished, to be listed under it; and the lifetimes whose
    /// failure was shown, each again with a signature it wasn't shown with.
    blocked: BlockedTests<NotRun>,
    /// Bad sub-outcomes held until their test finishes, by test.
    subs: SubOutcomes,
    /// What the summary counts beside the tests' counts: modules skipped
    /// while collecting, failed lifetimes that kept no test from running,
    /// error diagnostics (each shown and counted once, however many
    /// batches repeat it).
    tally: Tally,
    keys: Keys,
    /// The keys of every bad outcome, each once, in order.
    bad_keys: Vec<String>,
    bad_seen: HashSet<String>,
    /// The keys the details hint names: those of the bad outcomes shown
    /// on their own, in order.
    details: Vec<String>,
    /// What `aye show` asks for; `None` in a run.
    show: Option<ShowMode>,
    context: ShowContext,
    group: Group,
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    pub fn new(out: W, options: TextOptions) -> Self {
        let verbosity = options.verbosity;
        let stream = AutoStream::new(out, options.color);
        let links = options.live && stream.current_choice() == ColorChoice::AlwaysAnsi;
        Self {
            out: Out {
                stream,
                tail: *b"\n\n",
            },
            verbosity,
            live: options.live,
            links,
            progress: 0,
            facts: RunFacts::new(),
            header_shown: false,
            collected: None,
            signatures: FirstBySignature::new(),
            warnings: Vec::new(),
            skips: Vec::new(),
            blocked: BlockedTests::new(),
            subs: SubOutcomes::new(),
            tally: Tally::default(),
            keys: Keys::new(),
            bad_keys: Vec::new(),
            bad_seen: HashSet::new(),
            details: Vec::new(),
            show: None,
            context: ShowContext::default(),
            group: Group::Start,
        }
    }

    pub fn into_inner(self) -> W {
        self.out.stream.into_inner()
    }

    /// The key of a test or a lifetime.
    fn key<'s>(&self, subject: impl Into<Subject<'s>>) -> String {
        self.keys.shown(subject).to_string()
    }

    /// `key` is a bad outcome's: the details hint counts it.
    fn note_bad(&mut self, key: &str) {
        if self.bad_seen.insert(key.to_owned()) {
            self.bad_keys.push(key.to_owned());
        }
    }

    /// A test's line: `✓ passed  tests/t.py::test_x  1 ms`, the key after
    /// the outcome when there is one, and why it was skipped, expected to
    /// fail, or went bad.
    fn result_line(
        &mut self,
        outcome: Outcome,
        key: Option<&str>,
        name: &str,
        duration: Option<Duration>,
        note: Option<&str>,
    ) -> io::Result<()> {
        let (symbol, style) = symbol(outcome);
        let word = words::outcome_words(outcome);
        write!(self.out, "{style}{symbol} {word:<7}{style:#} ")?;
        if let Some(key) = key {
            write!(self.out, "{KEY}{key}{KEY:#}  ")?;
        }
        write!(self.out, "{name}")?;
        if let Some(duration) = duration {
            write!(self.out, "  {DIM}{}{DIM:#}", words::spaced(duration))?;
        }
        if let Some(note) = note {
            let note = note.lines().next().unwrap_or_default();
            write!(self.out, " — {}", words::head(note, self.line_max()))?;
        }
        writeln!(self.out)
    }

    fn test(&mut self, result: &TestResult, worker: Option<WorkerIndex>) -> io::Result<()> {
        let outcome = result.outcome;
        let id = result.id.as_str();
        let key = self.key(&result.id);
        let rerun = rerun::rerun(&result.id, result.reproduce.as_ref());
        let subs = self.subs.take(&result.id);
        let blocks = self.show_subs(&result.id, &key, &subs, Some(&rerun))?;
        if !outcome.is_bad() {
            if outcome == Outcome::Skipped && self.show.is_none() {
                self.skipped(result.reason.as_deref().unwrap_or_default());
            }
            if self.full() || self.show == Some(ShowMode::Details) {
                self.lines()?;
                self.result_line(
                    outcome,
                    None,
                    id,
                    Some(result.duration),
                    result.reason.as_deref(),
                )?;
                if self.engine() {
                    self.output(&result.output, OUTPUT_LINES)?;
                }
            }
            return Ok(());
        }
        self.note_bad(&key);
        if let Some(lifetime) = &result.cause {
            return self.blocked_test(lifetime, result, key);
        }
        // What the test's bad sub-outcomes already showed isn't shown
        // again: a test failed only by them gets a block of its own only
        // for what theirs lack.
        let own: Vec<Failure> = aye_stream::own_failures(result, &subs, Repeats::SameOrBare)
            .cloned()
            .collect();
        if own.is_empty() && !subs.is_empty() {
            return self.after_subs(result, &key, &rerun, subs.len(), blocks);
        }
        self.bad(&Bad {
            outcome,
            key: &key,
            name: id,
            path: result.id.path().as_str(),
            failures: &own,
            test: true,
            duration: Some(result.duration),
            result: Some(result),
            rerun: Some(rerun.clone()),
            worker,
            not_run: &[],
            brief: false,
            sub: None,
        })?;
        Ok(())
    }
}

impl<W: RawStream + AsLockedWrite> Consumer for Text<W> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        let noted = self.tally.count(record);
        self.keys.note(record);
        self.facts.note(record);
        if let Event::CollectionProgress {
            walked,
            test_files,
            refining,
        } = &record.event
        {
            return self.show_progress(*walked, *test_files, *refining);
        }
        self.clear_progress()?;
        let worker = record.envelope.worker;
        if let Event::WorkerStarted { .. } = &record.event {
            self.header(worker)?;
        }
        if self.engine() {
            self.engine_detail(record)?;
        }
        // `-q`: no failure is shown, only counted.
        if self.quiet()
            && matches!(
                record.event,
                Event::TestFinished(_) | Event::SubOutcome { .. } | Event::LifetimeFinished { .. }
            )
        {
            return Ok(());
        }
        match &record.event {
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic, noted),
            Event::CaseCollected(case) if self.facts.collect_only => {
                let partial = if case.certainty == Certainty::Partial {
                    "[…]"
                } else {
                    ""
                };
                self.lines()?;
                writeln!(self.out, "{}{DIM}{partial}{DIM:#}", case.id)
            }
            Event::CollectionFinished {
                selected,
                deselected,
                refined_files,
                duration,
            } => self.collection_finished(*selected, *deselected, *refined_files, *duration),
            // `aye show`'s list: a line per failed lifetime, and no block.
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                duration,
            } if self.show == Some(ShowMode::List) => {
                self.lifetime_list_line(lifetime, failure, *duration)
            }
            Event::LifetimeFinished { .. } | Event::SubOutcome { .. }
                if self.show == Some(ShowMode::List) =>
            {
                Ok(())
            }
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                duration,
            } => self.lifetime(lifetime, failure, *duration, worker),
            Event::LifetimeFinished { lifetime, .. } => self.flush_pending(Some(lifetime)),
            Event::SubOutcome { id, outcome, .. } if outcome.is_bad() => {
                self.sub_outcome(id, record);
                Ok(())
            }
            Event::TestFinished(result) if self.show == Some(ShowMode::List) => {
                self.list_line(result)
            }
            Event::TestFinished(result) => self.test(result, worker),
            Event::WorkerFinished {
                exit_status,
                error: Some(error),
            } => self.worker_error(worker, *exit_status, error),
            Event::RunFinished {
                exit_code,
                counts,
                duration,
                stopped,
            } => self.run_finished(*exit_code, counts, *duration, stopped.as_ref()),
            _ => Ok(()),
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        self.clear_progress()?;
        self.flush_subs()?;
        self.flush_pending(None)?;
        self.out.flush()
    }
}
