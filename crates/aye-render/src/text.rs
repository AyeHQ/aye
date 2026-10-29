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
//! [`shared::why`]) and where: `at <path>:<line>`, the innermost project
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
//! The verbosity (`-v` count minus `-q` count, clamped to -1..=3):
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
//! [`Text::for_show`]: [`ShowMode::List`] writes a line per result (`✗
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

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Duration;

use anstream::AutoStream;
use anstream::stream::{AsLockedWrite, RawStream};
use anstyle::{AnsiColor, Style};
use aye_events::{
    Assertion, Captured, Certainty, Columns, Compare, Counts, Diagnostic, Event, ExitCode, Failure,
    Frame, LifetimeId, Local, Outcome, Phase, Record, Severity, TestId, TestResult,
};

use crate::tally::{self, Noted, Tally, Tone};
use crate::{ColorChoice, Keys, Reporter, ShowContext, ShowMode, shared};

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const RED: Style = AnsiColor::Red.on_default();
const RED_BOLD: Style = AnsiColor::Red.on_default().bold();
const GREEN: Style = AnsiColor::Green.on_default();
const YELLOW: Style = AnsiColor::Yellow.on_default();
const YELLOW_BOLD: Style = AnsiColor::Yellow.on_default().bold();
const CYAN: Style = AnsiColor::Cyan.on_default();
const KEY: Style = AnsiColor::Magenta.on_default().bold();
const SPAN: Style = AnsiColor::Red.on_default().underline();

/// `-q`: the summary line, and what never hides.
const QUIET: i8 = -1;
/// `-v`: bad outcomes at L2.
const VERBOSE: i8 = 1;
/// `-vv`: bad outcomes at L3, a line per test.
const FULL: i8 = 2;
/// `-vvv`: what the engine did.
const ENGINE: i8 = 3;

/// Characters shown of a value, a source line or a line of output, below
/// L3.
const LINE_MAX: usize = 300;
/// Lines of an explanation shown at L2.
const DIFF_LINES: usize = 15;
/// Rows of a structured comparison shown at L2.
const DIFFERENCES: usize = 10;
/// Lines of a message after its first, at L2.
const MESSAGE_LINES: usize = 10;
/// Lines of each captured stream at L3.
const OUTPUT_LINES: usize = 50;
/// Lines of a worker's error, below `-vv`.
const WORKER_LINES: usize = 10;
/// Bad sub-outcomes of one test shown as blocks; the rest are lines.
const SUB_BLOCKS: u32 = 3;
/// Tests a failed lifetime kept from running, listed under it at L2.
const NOT_RUN: usize = 10;
/// Keys the details hint lists.
const DETAILS: usize = 3;
/// Codes on the warnings line, reasons on the skipped line.
const GROUPS: usize = 3;
/// Characters of a skip's reason on the skipped line.
const REASON_MAX: usize = 100;
/// Characters of an operand's source naming a column.
const SOURCE_LABEL: usize = 30;
/// The width of a row's label (`history`).
const LABEL: usize = 7;

#[derive(Debug, Clone, Copy)]
pub struct TextOptions {
    /// `-v` count minus `-q` count, clamped to -1..=3: how much is shown
    /// (see the module's docs).
    pub verbosity: i8,
    pub color: ColorChoice,
    /// The output is a terminal a person watches: a long collection shows
    /// a line that says how far it got, rewritten in place and cleared
    /// before anything else is written, and locations are hyperlinks when
    /// colour is on.
    pub live: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            verbosity: 0,
            color: ColorChoice::Auto,
            live: false,
        }
    }
}

/// How much of a bad outcome is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Level {
    L1,
    L2,
    L3,
}

/// What was written last, so that paragraphs are set apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Start,
    Header,
    Lines,
    Block,
}

pub struct Text<W: RawStream> {
    out: Out<W>,
    verbosity: i8,
    collect_only: bool,
    live: bool,
    /// Locations are OSC 8 hyperlinks: a live terminal, with colour.
    links: bool,
    /// The width of the progress line on screen, 0 when none is.
    progress: usize,
    /// From `run_started`: the project root, the workers, the run's
    /// number.
    root: Option<String>,
    workers: u32,
    run: Option<u64>,
    /// From `collection_finished`.
    selected: Option<u32>,
    header_shown: bool,
    /// From `-vv`, the `collected` line, held to follow the header.
    collected: Option<String>,
    /// Each worker's Python version, from its `worker_started`.
    pythons: HashMap<Option<u32>, String>,
    /// The first bad outcome shown with each signature: its key and
    /// message.
    signatures: HashMap<String, (String, String)>,
    warnings: Vec<Diagnostic>,
    /// Skip reasons, with how many tests (or modules) each, in order.
    skips: Vec<(String, u32)>,
    /// Tests kept from running by a lifetime whose failure hasn't arrived
    /// (the engine reports it after them, except under `maxfail`), in the
    /// order they finished, to be listed under it.
    pending: Vec<(LifetimeId, Vec<NotRun>)>,
    /// Lifetime failures already shown, by lifetime and signature.
    lifetimes: HashSet<(LifetimeId, String)>,
    /// Lifetimes whose failure was shown.
    shown_lifetimes: HashSet<LifetimeId>,
    /// Bad sub-outcomes held until their test finishes, by test.
    subs: HashMap<TestId, Subs>,
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

/// A test a failed lifetime kept from running, and its key.
type NotRun = (TestId, String);

/// The bad sub-outcomes of one test.
#[derive(Default)]
struct Subs {
    /// How many there were.
    count: u32,
    /// How many were shown as blocks.
    blocks: u32,
    /// Their failures, which the test's own may repeat.
    failures: Vec<Failure>,
    /// Those not shown yet, held until their test finishes.
    held: Vec<HeldSub>,
}

struct HeldSub {
    name: String,
    outcome: Outcome,
    failure: Option<Failure>,
    duration: Duration,
    worker: Option<u32>,
}

/// A bad outcome to show.
struct Bad<'a> {
    outcome: Outcome,
    key: &'a str,
    /// The id, `<id> › <sub-outcome>`, or `setup of <lifetime>`.
    name: &'a str,
    /// The file the name is in, as ids spell it.
    path: &'a str,
    /// The first is shown; the rest each in a line.
    failures: &'a [Failure],
    /// A test's own failures: a failed teardown says it passed.
    test: bool,
    duration: Option<Duration>,
    /// The test's result: its resources, history and output.
    result: Option<&'a TestResult>,
    rerun: Option<String>,
    worker: Option<u32>,
    /// Tests a failed lifetime kept from running.
    not_run: &'a [NotRun],
    /// At most L1 (a test's fourth bad sub-outcome and later, below L3).
    brief: bool,
    /// A sub-outcome's name: a later one failing the same way names it
    /// beside the key it shares.
    sub: Option<&'a str>,
}

/// The output stream, remembering whether it is at the start of a paragraph.
struct Out<W: RawStream> {
    stream: AutoStream<W>,
    tail: [u8; 2],
}

impl<W: RawStream + AsLockedWrite> Out<W> {
    /// Starts a paragraph: a blank line, unless at the start already.
    fn paragraph(&mut self) -> io::Result<()> {
        if self.tail == *b"\n\n" {
            Ok(())
        } else {
            writeln!(self)
        }
    }
}

impl<W: RawStream + AsLockedWrite> Write for Out<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.stream.write(buf)?;
        match written {
            0 => {}
            1 => self.tail = [self.tail[1], buf[0]],
            n => self.tail = [buf[n - 2], buf[n - 1]],
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    pub fn new(out: W, options: TextOptions) -> Self {
        let verbosity = options.verbosity.clamp(QUIET, ENGINE);
        let stream = AutoStream::new(out, options.color);
        let links = options.live && stream.current_choice() == ColorChoice::AlwaysAnsi;
        Self {
            out: Out {
                stream,
                tail: *b"\n\n",
            },
            verbosity,
            collect_only: false,
            live: options.live,
            links,
            progress: 0,
            root: None,
            workers: 0,
            run: None,
            selected: None,
            header_shown: false,
            collected: None,
            pythons: HashMap::new(),
            signatures: HashMap::new(),
            warnings: Vec::new(),
            skips: Vec::new(),
            pending: Vec::new(),
            lifetimes: HashSet::new(),
            shown_lifetimes: HashSet::new(),
            subs: HashMap::new(),
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

    /// For `aye show`: `mode`, the keys of the whole log the records fed
    /// come from, and what the run store knows of the run beside it.
    #[must_use]
    pub fn for_show(mut self, mode: ShowMode, keys: Keys, context: ShowContext) -> Self {
        self.show = Some(mode);
        self.keys = keys;
        self.context = context;
        self
    }

    /// Before anything else, for `aye show`: `path` changed since the run
    /// (or is `gone`), so what is shown of its tests may be out of date. A
    /// warning, shown at every level.
    pub fn stale(&mut self, path: &str, gone: bool) -> io::Result<()> {
        if gone {
            writeln!(self.out, "{YELLOW}{path} is gone since this run{YELLOW:#}")
        } else {
            let rerun = shared::rerun_path(path);
            writeln!(
                self.out,
                "{YELLOW}{path} changed since this run; rerun: {rerun}{YELLOW:#}"
            )
        }
    }

    pub fn into_inner(self) -> W {
        self.out.stream.into_inner()
    }

    /// Rewrite the progress line in place. Only spaces and `\r`, so that it
    /// clears the same whether escapes are passed or stripped. It bypasses
    /// the paragraph tracking: it is gone before anything else is written.
    fn show_progress(&mut self, walked: u64, test_files: u64, refining: u64) -> io::Result<()> {
        if !self.live {
            return Ok(());
        }
        let line = if refining > 0 {
            format!(
                "collecting: importing {} to find their tests",
                shared::plural(refining, "file", "files")
            )
        } else {
            format!(
                "collecting: {} walked, {} found",
                shared::plural(walked, "file", "files"),
                shared::plural(test_files, "test file", "test files")
            )
        };
        let width = line.chars().count();
        let pad = self.progress.saturating_sub(width);
        write!(self.out.stream, "\r{line}{:pad$}", "")?;
        self.out.stream.flush()?;
        self.progress = width;
        Ok(())
    }

    fn clear_progress(&mut self) -> io::Result<()> {
        if self.progress == 0 {
            return Ok(());
        }
        let width = self.progress;
        write!(self.out.stream, "\r{:width$}\r", "")?;
        self.progress = 0;
        self.out.stream.flush()
    }

    const fn quiet(&self) -> bool {
        self.verbosity <= QUIET
    }

    /// From `-vv`: a line per test, every warning, infos, nothing cut.
    const fn full(&self) -> bool {
        self.verbosity >= FULL
    }

    const fn engine(&self) -> bool {
        self.verbosity >= ENGINE
    }

    /// How much of a bad outcome is shown.
    const fn level(&self) -> Level {
        if self.verbosity >= FULL {
            Level::L3
        } else if self.verbosity >= VERBOSE {
            Level::L2
        } else {
            Level::L1
        }
    }

    const fn line_max(&self) -> usize {
        if self.full() { usize::MAX } else { LINE_MAX }
    }

    /// Lines follow: a blank line first unless lines came last.
    fn lines(&mut self) -> io::Result<()> {
        self.flush_collected()?;
        if !matches!(self.group, Group::Lines | Group::Start) {
            self.out.paragraph()?;
        }
        self.group = Group::Lines;
        Ok(())
    }

    /// A block follows, set apart by blank lines.
    fn block(&mut self) -> io::Result<()> {
        self.flush_collected()?;
        self.out.paragraph()?;
        self.group = Group::Block;
        Ok(())
    }

    /// The key of a test's or a lifetime's id.
    fn key(&self, id: &str) -> String {
        self.keys.key(id).map_or_else(
            || aye_events::TestKey::of(id).0[..crate::keys::MIN].to_owned(),
            str::to_owned,
        )
    }

    /// `key` is a bad outcome's: the details hint counts it.
    fn note_bad(&mut self, key: &str) {
        if self.bad_seen.insert(key.to_owned()) {
            self.bad_keys.push(key.to_owned());
        }
    }

    /// The header, once, at the first worker: `aye · <root> · python 3.13.1
    /// · 15 tests · 1 worker`.
    fn header(&mut self, worker: Option<u32>) -> io::Result<()> {
        if self.header_shown || self.show.is_some() || self.collect_only || self.quiet() {
            return Ok(());
        }
        self.header_shown = true;
        let mut parts = vec!["aye".to_owned()];
        if let Some(root) = &self.root {
            let trimmed = root.trim_end_matches(['/', '\\']);
            let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
            if !name.is_empty() {
                parts.push(name.to_owned());
            }
        }
        if let Some(python) = self.pythons.get(&worker) {
            parts.push(format!("python {python}"));
        }
        if let Some(selected) = self.selected {
            parts.push(shared::plural(selected, "test", "tests"));
        }
        parts.push(shared::plural(self.workers.max(1), "worker", "workers"));
        self.out.paragraph()?;
        writeln!(self.out, "{DIM}{}{DIM:#}", parts.join(" · "))?;
        self.group = Group::Header;
        if let Some(collected) = self.collected.take() {
            writeln!(self.out, "{collected}")?;
        }
        Ok(())
    }

    /// A diagnostic, `noted` what the tally made of it: an error is shown
    /// the first time it comes, a warning in the closing lines, an info
    /// from `-vv`. A module skipped while collecting counts with the skips
    /// (for `aye show`, it is a line as a skipped test's), and `aye show`'s
    /// list shows no other warning or info.
    fn diagnostic(&mut self, diagnostic: &Diagnostic, noted: Option<Noted<'_>>) -> io::Result<()> {
        if matches!(noted, Some(Noted::ModuleSkip(_))) {
            if self.show.is_some() {
                if self.quiet() {
                    return Ok(());
                }
                self.lines()?;
                return self.result_line(
                    Outcome::Skipped,
                    None,
                    diagnostic.path.as_str(),
                    None,
                    Some(&diagnostic.message),
                );
            }
            self.skipped(&diagnostic.message);
        }
        match diagnostic.severity {
            Severity::Error if matches!(noted, Some(Noted::Error(_))) => {
                self.lines()?;
                self.diagnostic_line(diagnostic)
            }
            Severity::Error => Ok(()),
            _ if self.show == Some(ShowMode::List) => Ok(()),
            Severity::Warning => {
                self.warnings.push(diagnostic.clone());
                Ok(())
            }
            Severity::Info if self.full() => {
                self.lines()?;
                self.diagnostic_line(diagnostic)
            }
            Severity::Info => Ok(()),
        }
    }

    fn diagnostic_line(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        let (label, style) = match diagnostic.severity {
            Severity::Error => ("error", RED_BOLD),
            Severity::Warning => ("warning", YELLOW),
            Severity::Info => ("info", DIM),
        };
        let at = match diagnostic.span.as_ref().and_then(|span| span.line) {
            Some(line) => format!(" {}", self.location(diagnostic.path.as_str(), line)),
            None if diagnostic.path.is_nowhere() => String::new(),
            None => format!(" {}", diagnostic.path),
        };
        let mut lines = diagnostic.message.lines();
        let first = lines.next().unwrap_or_default();
        writeln!(
            self.out,
            "{style}{label}[{}]{style:#}{at}: {first}",
            diagnostic.code
        )?;
        for line in lines {
            writeln!(self.out, "  {line}")?;
        }
        Ok(())
    }

    /// A skip's reason, for the grouped line.
    fn skipped(&mut self, reason: &str) {
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
        let word = shared::outcome_words(outcome);
        write!(self.out, "{style}{symbol} {word:<7}{style:#} ")?;
        if let Some(key) = key {
            write!(self.out, "{KEY}{key}{KEY:#}  ")?;
        }
        write!(self.out, "{name}")?;
        if let Some(duration) = duration {
            write!(self.out, "  {DIM}{}{DIM:#}", shared::spaced(duration))?;
        }
        if let Some(note) = note {
            let note = note.lines().next().unwrap_or_default();
            write!(self.out, " — {}", shared::head(note, self.line_max()))?;
        }
        writeln!(self.out)
    }

    /// What the engine did (`-vvv`): a line for each worker and batch
    /// starting and finishing, each lifetime starting and finishing (a
    /// failed one's block follows), and each sub-outcome that didn't fail.
    fn engine_detail(&mut self, record: &Record) -> io::Result<()> {
        let worker = record
            .envelope
            .worker
            .map_or_else(|| "a worker".to_owned(), |w| format!("worker {w}"));
        let batch = record
            .envelope
            .batch
            .map_or_else(|| "a batch".to_owned(), |b| format!("batch {b}"));
        let line = match &record.event {
            Event::WorkerStarted { pid, python, .. } => {
                format!("{worker} started (pid {pid}, {python})")
            }
            Event::BatchStarted { adapter, cases } => format!(
                "{batch} started on {worker} ({adapter}, {})",
                shared::plural(*cases, "test", "tests")
            ),
            Event::LifetimeStarted { lifetime, kind } => {
                format!("lifetime {lifetime} started ({kind})")
            }
            Event::LifetimeFinished {
                lifetime,
                failure,
                duration,
            } => {
                let finished = if failure.is_some() {
                    "failed"
                } else {
                    "finished"
                };
                format!(
                    "lifetime {lifetime} {finished} ({})",
                    shared::spaced(*duration)
                )
            }
            Event::BatchFinished { status } => match status {
                Some(status) => format!("{batch} finished (status {status})"),
                None => format!("{batch} finished"),
            },
            // A failed worker's block says it.
            Event::WorkerFinished {
                exit_status,
                error: None,
            } => match exit_status {
                Some(status) => format!("{worker} finished (exit status {status})"),
                None => format!("{worker} finished"),
            },
            Event::SubOutcome {
                id,
                name,
                outcome,
                reason,
                duration,
                ..
            } if !outcome.is_bad() => {
                self.lines()?;
                return self.result_line(
                    *outcome,
                    None,
                    &format!("{id} › {name}"),
                    Some(*duration),
                    reason.as_deref(),
                );
            }
            _ => return Ok(()),
        };
        self.lines()?;
        writeln!(self.out, "{DIM}{line}{DIM:#}")
    }

    fn collection_finished(
        &mut self,
        selected: u32,
        deselected: u32,
        refined_files: u32,
        duration: Duration,
    ) -> io::Result<()> {
        self.selected = Some(selected);
        if !(self.full() || self.collect_only) {
            return Ok(());
        }
        let mut notes = Vec::new();
        if deselected > 0 {
            notes.push(format!("{deselected} deselected"));
        }
        if refined_files > 0 && self.full() {
            notes.push(shared::plural(
                refined_files,
                "file imported",
                "files imported",
            ));
        }
        let notes = if notes.is_empty() {
            String::new()
        } else {
            format!(" ({})", notes.join(", "))
        };
        let line = format!(
            "collected {}{notes} in {}",
            shared::plural(selected, "test", "tests"),
            shared::spaced(duration)
        );
        // In a run, it follows the header (when a worker starts).
        if !self.collect_only && self.show.is_none() && !self.quiet() {
            self.collected = Some(line);
            return Ok(());
        }
        self.lines()?;
        writeln!(self.out, "{line}")
    }

    /// The `collected` line held for the header, when something else comes
    /// first (a log without `worker_started`).
    fn flush_collected(&mut self) -> io::Result<()> {
        if let Some(line) = self.collected.take() {
            self.lines()?;
            writeln!(self.out, "{line}")?;
        }
        Ok(())
    }

    /// A test's line in `aye show`'s list: its outcome, its key when bad,
    /// its id, its duration, and why it went bad or was skipped.
    fn list_line(&mut self, result: &TestResult) -> io::Result<()> {
        let bad = result.outcome.is_bad();
        let key = bad.then(|| self.key(result.id.as_str()));
        let note = if bad {
            match (&result.cause, result.failure()) {
                (Some(lifetime), _) => Some(format!("setup of {lifetime} failed")),
                (None, Some(failure)) => Some(shared::why(failure, true)),
                (None, None) => shared::bare_why(&result.output),
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

    fn test(&mut self, result: &TestResult, worker: Option<u32>) -> io::Result<()> {
        let outcome = result.outcome;
        let id = result.id.as_str();
        let key = self.key(id);
        let rerun = shared::rerun(&result.id, result.reproduce.as_ref());
        let mut subs = self.subs.remove(&result.id);
        if let Some(subs) = &mut subs {
            self.show_subs(&result.id, &key, subs, Some(&rerun))?;
        }
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
        let own: Vec<Failure> = match &subs {
            Some(subs) => result
                .failures
                .iter()
                .filter(|failure| !shared::repeats_subs(failure, &subs.failures))
                .cloned()
                .collect(),
            None => result.failures.clone(),
        };
        if own.is_empty()
            && let Some(subs) = &subs
        {
            return self.after_subs(result, &key, &rerun, subs);
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

    /// A test a failed lifetime kept from running: listed under the
    /// lifetime's failure, which the engine reports after the tests it
    /// blocked; a line of its own when that failure was shown already.
    fn blocked_test(
        &mut self,
        lifetime: &LifetimeId,
        result: &TestResult,
        key: String,
    ) -> io::Result<()> {
        if self.shown_lifetimes.contains(lifetime) {
            return self.blocked_line(lifetime, &result.id, &key);
        }
        let entry = (result.id.clone(), key);
        match self.pending.iter_mut().find(|(held, _)| held == lifetime) {
            Some((_, ids)) => ids.push(entry),
            None => self.pending.push((lifetime.clone(), vec![entry])),
        }
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
    fn flush_pending(&mut self, lifetime: Option<&LifetimeId>) -> io::Result<()> {
        let pending = std::mem::take(&mut self.pending);
        let (flushed, kept): (Vec<_>, Vec<_>) = pending
            .into_iter()
            .partition(|(held, _)| lifetime.is_none_or(|lifetime| held == lifetime));
        self.pending = kept;
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
    fn after_subs(
        &mut self,
        result: &TestResult,
        key: &str,
        rerun: &str,
        subs: &Subs,
    ) -> io::Result<()> {
        let level = self.level();
        let has_output = level == Level::L3 && !result.output.is_empty();
        let has_rerun =
            level >= Level::L2 && subs.blocks == 0 && rerun != shared::rerun(&result.id, None);
        if !(has_output || has_rerun) {
            return Ok(());
        }
        // The adapter's own word for it ("contains 1 failed subtest"), or
        // how many were shown above.
        let note = match result.failure().filter(|failure| shared::is_bare(failure)) {
            Some(failure) => shared::message(failure).to_owned(),
            None => format!(
                "{} above",
                shared::plural(subs.count, "failure", "failures")
            ),
        };
        self.block()?;
        self.head(result.outcome, key, result.id.as_str(), Some(&note), true)?;
        if has_output {
            self.output(&result.output, OUTPUT_LINES)?;
        }
        self.row("rerun", rerun)?;
        writeln!(self.out)
    }

    /// The bad sub-outcomes of test `id` not shown yet, with `rerun`, its
    /// rerun, which arrives when it finishes.
    fn show_subs(
        &mut self,
        id: &TestId,
        key: &str,
        subs: &mut Subs,
        rerun: Option<&str>,
    ) -> io::Result<()> {
        for sub in std::mem::take(&mut subs.held) {
            let name = format!("{id} › {}", sub.name);
            let brief = subs.blocks >= SUB_BLOCKS && self.level() < Level::L3;
            let fresh = self.bad(&Bad {
                outcome: sub.outcome,
                key,
                name: &name,
                path: id.path().as_str(),
                failures: sub.failure.as_slice(),
                test: false,
                duration: Some(sub.duration),
                result: None,
                rerun: Some(rerun.map_or_else(|| shared::rerun(id, None), str::to_owned)),
                worker: sub.worker,
                not_run: &[],
                brief,
                sub: Some(&sub.name),
            })?;
            if fresh && !brief {
                subs.blocks += 1;
            }
        }
        Ok(())
    }

    /// Bad sub-outcomes whose test never finished (the run stopped), with
    /// the plain rerun.
    fn flush_subs(&mut self) -> io::Result<()> {
        let mut held: Vec<(TestId, Subs)> = self
            .subs
            .iter_mut()
            .filter(|(_, subs)| !subs.held.is_empty())
            .map(|(id, subs)| {
                let held = Subs {
                    held: std::mem::take(&mut subs.held),
                    ..Subs::default()
                };
                (id.clone(), held)
            })
            .collect();
        held.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, mut subs) in held {
            let key = self.key(id.as_str());
            self.show_subs(&id, &key, &mut subs, None)?;
        }
        Ok(())
    }

    /// A bad sub-outcome (a failed subtest or doctest example), held until
    /// its test finishes, whose rerun its block shows.
    fn sub_outcome(&mut self, id: &TestId, sub: HeldSub) {
        let key = self.key(id.as_str());
        self.note_bad(&key);
        let subs = self.subs.entry(id.clone()).or_default();
        subs.count += 1;
        if let Some(failure) = &sub.failure {
            subs.failures.push(failure.clone());
        }
        subs.held.push(sub);
    }

    fn lifetime(
        &mut self,
        lifetime: &LifetimeId,
        failure: &Failure,
        duration: Duration,
        worker: Option<u32>,
    ) -> io::Result<()> {
        if !self
            .lifetimes
            .insert((lifetime.clone(), failure.signature.clone()))
        {
            return Ok(());
        }
        self.shown_lifetimes.insert(lifetime.clone());
        let not_run = match self.pending.iter().position(|(held, _)| held == lifetime) {
            Some(at) => self.pending.remove(at).1,
            None => Vec::new(),
        };
        let key = self.key(lifetime);
        self.note_bad(&key);
        let name = lifetime_name(lifetime, failure);
        let rerun = not_run.first().map(|(first, _)| shared::rerun(first, None));
        self.bad(&Bad {
            outcome: Outcome::Error,
            key: &key,
            name: &name,
            path: shared::file_of(lifetime),
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

    /// A failed lifetime's line in `aye show`'s list, once per lifetime.
    fn lifetime_list_line(
        &mut self,
        lifetime: &LifetimeId,
        failure: &Failure,
        duration: Duration,
    ) -> io::Result<()> {
        if !self.shown_lifetimes.insert(lifetime.clone()) {
            return Ok(());
        }
        let key = self.key(lifetime);
        let name = lifetime_name(lifetime, failure);
        let why = shared::why(failure, false);
        self.lines()?;
        self.result_line(
            Outcome::Error,
            Some(&key),
            &name,
            Some(duration),
            Some(&why),
        )
    }

    /// A bad outcome, at the level asked for; below L3, one naming the
    /// first with its signature when that was shown already. Whether it
    /// was shown on its own.
    fn bad(&mut self, bad: &Bad<'_>) -> io::Result<bool> {
        let level = if bad.brief { Level::L1 } else { self.level() };
        if let Some(first) = bad.failures.first()
            && !first.signature.is_empty()
            && self.level() < Level::L3
        {
            match self.signatures.entry(first.signature.clone()) {
                Entry::Occupied(seen) => {
                    let (same, message) = seen.get().clone();
                    self.same_as(bad, first, &same, &message)?;
                    return Ok(false);
                }
                Entry::Vacant(slot) => {
                    let label = match bad.sub {
                        Some(sub) => format!("{} › {sub}", bad.key),
                        None => bad.key.to_owned(),
                    };
                    slot.insert((label, shared::message(first).to_owned()));
                }
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
        if message != shared::message(failure) {
            write!(self.out, " · {}", shared::why(failure, bad.test))?;
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
            let why = shared::why(failure, bad.test);
            write!(self.out, "{indent}{why}")?;
            if let Some((path, line)) = shared::origin(failure) {
                write!(self.out, " at {}", self.location(path.as_str(), line))?;
            }
            writeln!(self.out)?;
        } else {
            let why = bad
                .result
                .and_then(|result| shared::bare_why(&result.output))
                .unwrap_or_else(|| shared::outcome_words(bad.outcome).to_owned());
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
            shared::plural(bad.not_run.len(), "test", "tests"),
            bad.key
        )
    }

    /// L2 and L3: a block.
    fn block_l2(&mut self, bad: &Bad<'_>, level: Level) -> io::Result<()> {
        self.block()?;
        self.head(bad.outcome, bad.key, bad.name, None, true)?;
        self.second_line(bad)?;
        let first = bad.failures.first();
        if let Some(failure) = first {
            writeln!(self.out)?;
            self.pointed_code(failure)?;
            self.explanation(failure, level)?;
            for also in &bad.failures[1..] {
                let phase = also.phase.map_or("test", shared::phase);
                writeln!(
                    self.out,
                    "  {DIM}also in {phase}:{DIM:#} {RED}{}{RED:#}",
                    shared::head(shared::message(also), self.line_max())
                )?;
            }
        }
        // Without a failure, the output is all there is to say: shown
        // from L2.
        let output = bad
            .result
            .map(|result| &result.output)
            .filter(|output| !output.is_empty())
            .filter(|_| level == Level::L3 || first.is_none());
        let (frames, locals) = match (level, first) {
            (Level::L3, Some(failure)) => (failure.frames.as_slice(), shared::locals(failure)),
            _ => (&[][..], &[][..]),
        };
        if !frames.is_empty() || output.is_some() || !locals.is_empty() {
            writeln!(self.out)?;
            self.frames(frames)?;
            if let Some(output) = output {
                self.output(output, OUTPUT_LINES)?;
            }
            self.locals(locals)?;
        }
        self.rows(bad, level)?;
        writeln!(self.out)?;
        self.group = Group::Block;
        Ok(())
    }

    /// `  failed in the test · 2 ms`, then for `aye show` the Python, the
    /// run and the file.
    fn second_line(&mut self, bad: &Bad<'_>) -> io::Result<()> {
        let mut parts = Vec::new();
        let word = shared::outcome_words(bad.outcome);
        parts.push(match shared::phase_words(bad.failures.first()) {
            Some(phase) => format!("{word} in {phase}"),
            None => word.to_owned(),
        });
        if let Some(duration) = bad.duration {
            parts.push(shared::spaced(duration));
        }
        if self.show.is_some() {
            if let Some(python) = self.pythons.get(&bad.worker) {
                parts.push(format!("python {python}"));
            }
            let run = self
                .context
                .run
                .or(self.run)
                .map(|run| format!("run r{run}"));
            match (run, &self.context.ago) {
                (Some(run), Some(ago)) => parts.push(format!("{run}, {ago}")),
                (Some(run), None) => parts.push(run),
                (None, Some(ago)) => parts.push(ago.clone()),
                (None, None) => {}
            }
            parts.push(
                match self.context.stale.get(bad.path) {
                    None => "file unchanged",
                    Some(false) => "file changed since",
                    Some(true) => "file gone",
                }
                .to_owned(),
            );
        }
        writeln!(self.out, "  {DIM}{}{DIM:#}", parts.join(" · "))
    }

    /// The location, then the failing line with the two before it and the
    /// failing expression marked.
    fn pointed_code(&mut self, failure: &Failure) -> io::Result<()> {
        let Some((path, line)) = shared::origin(failure) else {
            return Ok(());
        };
        writeln!(self.out, "  {}", self.location(path.as_str(), line))?;
        let Some(frame) = shared::pointed(failure) else {
            return Ok(());
        };
        let Some(source) = &frame.source else {
            return Ok(());
        };
        let width = line.to_string().len();
        let before = u32::try_from(frame.before.len()).unwrap_or(0);
        for (at, code) in (line.saturating_sub(before)..).zip(&frame.before) {
            writeln!(
                self.out,
                "     {DIM}{at:>width$} │{DIM:#} {}",
                shared::head(code, self.line_max())
            )?;
        }
        // The failing line at its indentation relative to the ones before.
        let indent = if frame.before.is_empty() {
            0
        } else {
            usize::try_from(frame.indent).unwrap_or(0).min(40)
        };
        write!(
            self.out,
            "  {RED_BOLD}→{RED_BOLD:#}  {DIM}{line:>width$} │{DIM:#} {:indent$}",
            ""
        )?;
        let source = shared::head(source, self.line_max());
        self.marked(&source, frame.cols)?;
        if let Some(cols) = frame.cols {
            let (start, len) = span(&source, cols);
            let start = indent + start;
            writeln!(
                self.out,
                "     {:width$} {DIM}│{DIM:#} {:start$}{RED}{}{RED:#}",
                "",
                "",
                "^".repeat(len)
            )?;
        }
        Ok(())
    }

    /// A source line, the span `cols` red and underlined.
    fn marked(&mut self, source: &str, cols: Option<Columns>) -> io::Result<()> {
        let Some(cols) = cols else {
            return writeln!(self.out, "{source}");
        };
        let (start, len) = span(source, cols);
        let byte = |chars: usize| {
            source
                .char_indices()
                .nth(chars)
                .map_or(source.len(), |(at, _)| at)
        };
        let (from, to) = (byte(start), byte(start + len));
        writeln!(
            self.out,
            "{}{SPAN}{}{SPAN:#}{}",
            &source[..from],
            &source[from..to],
            &source[to..]
        )
    }

    /// How the failure explains itself: a structured comparison, or the
    /// message, the operands and the explanation, or an exception's
    /// message.
    fn explanation(&mut self, failure: &Failure, level: Level) -> io::Result<()> {
        let Some(assertion) = &failure.assertion else {
            return self.message(failure, level);
        };
        if let Some(compare) = &assertion.compare {
            // The test's own message (`assert a == b, "why"`) first.
            if let Some(own) = shared::own_message(failure, assertion) {
                writeln!(
                    self.out,
                    "  {RED_BOLD}{}{RED_BOLD:#}",
                    shared::head(own, self.line_max())
                )?;
            }
            return self.compare(assertion, compare, level);
        }
        self.message(failure, level)?;
        if shared::operands_add(failure, assertion) {
            let (expected, actual) = shared::whole(assertion);
            if assertion.expected.is_some() {
                self.operand("expected:", expected, level)?;
                self.operand("actual:  ", actual, level)?;
            } else {
                self.operand("left: ", &assertion.left, level)?;
                self.operand("right:", &assertion.right, level)?;
            }
        }
        self.diff(assertion, level)
    }

    /// The message, and up to 10 more lines of it (all at L3).
    fn message(&mut self, failure: &Failure, level: Level) -> io::Result<()> {
        writeln!(
            self.out,
            "  {RED_BOLD}{}{RED_BOLD:#}",
            shared::head(shared::message(failure), self.line_max())
        )?;
        let rest = shared::message_rest(failure);
        let shown = if level == Level::L3 {
            rest.len()
        } else {
            MESSAGE_LINES.min(rest.len())
        };
        for line in &rest[..shown] {
            writeln!(
                self.out,
                "  {RED}{}{RED:#}",
                shared::head(line, self.line_max())
            )?;
        }
        self.more_lines(rest.len() - shown, "  ")
    }

    /// `… N more lines`, when some are.
    fn more_lines(&mut self, more: usize, indent: &str) -> io::Result<()> {
        if more == 0 {
            return Ok(());
        }
        let more = shared::plural(more, "more line", "more lines");
        writeln!(self.out, "{indent}{DIM}… {more}{DIM:#}")
    }

    /// A compared value; one that spans lines continues under its first.
    fn operand(&mut self, label: &str, value: &str, level: Level) -> io::Result<()> {
        let mut lines = value.lines();
        let first = lines.next().unwrap_or_default();
        let pad = " ".repeat(label.chars().count() + 5);
        writeln!(
            self.out,
            "    {DIM}{label}{DIM:#} {}",
            shared::head(first, self.line_max())
        )?;
        let rest: Vec<&str> = lines.collect();
        let shown = if level == Level::L3 {
            rest.len()
        } else {
            DIFF_LINES.min(rest.len())
        };
        for line in &rest[..shown] {
            writeln!(self.out, "{pad}{}", shared::head(line, self.line_max()))?;
        }
        self.more_lines(rest.len() - shown, &pad)
    }

    /// The framework's explanation, `-` lines red and `+` lines green.
    fn diff(&mut self, assertion: &Assertion, level: Level) -> io::Result<()> {
        let diff = shared::explanation(assertion);
        let shown = if level == Level::L3 {
            diff.len()
        } else {
            DIFF_LINES.min(diff.len())
        };
        for line in &diff[..shown] {
            let style = if line.starts_with('-') {
                RED
            } else if line.starts_with('+') {
                GREEN
            } else {
                Style::new()
            };
            writeln!(
                self.out,
                "    {style}{}{style:#}",
                shared::head(line, self.line_max())
            )?;
        }
        self.more_lines(diff.len() - shown, "    ")
    }

    /// A structured comparison: its summary, then a row per difference.
    fn compare(
        &mut self,
        assertion: &Assertion,
        compare: &Compare,
        level: Level,
    ) -> io::Result<()> {
        let text = matches!(compare.kind.as_str(), "str" | "bytes");
        let first_at = compare
            .differs
            .first()
            .and_then(|difference| difference.at.as_deref());
        let summary = match first_at {
            Some(at) if text && compare.differs.len() == 1 => {
                let what = if compare.kind == "bytes" {
                    "bytes"
                } else {
                    "strings"
                };
                format!("{what} differ at {at}")
            }
            _ => compare.summary.clone(),
        };
        if !summary.is_empty() {
            writeln!(self.out, "  {}", shared::head(&summary, self.line_max()))?;
        }
        let known = assertion.expected.is_some();
        let (first_name, second_name) = if known {
            ("expected".to_owned(), "actual".to_owned())
        } else {
            let (left, right) = shared::sources(assertion);
            (
                shared::head(&left, SOURCE_LABEL).into_owned(),
                shared::head(&right, SOURCE_LABEL).into_owned(),
            )
        };
        let (first_style, second_style) = if known {
            (GREEN, RED)
        } else {
            (Style::new(), Style::new())
        };
        let shown = if level == Level::L3 {
            compare.differs.len()
        } else {
            DIFFERENCES.min(compare.differs.len())
        };
        let line_max = self.line_max();
        let value = |value: Option<&str>| {
            value.map_or_else(
                || "missing".to_owned(),
                |value| shared::head(value, line_max).into_owned(),
            )
        };
        let rows: Vec<(String, String, String)> = if compare.differs.is_empty() {
            let (first, second) = if known {
                shared::whole(assertion)
            } else {
                (assertion.left.as_str(), assertion.right.as_str())
            };
            vec![(String::new(), value(Some(first)), value(Some(second)))]
        } else {
            compare.differs[..shown]
                .iter()
                .map(|difference| {
                    let (first, second) = shared::entries(assertion, difference);
                    (
                        difference.at.clone().unwrap_or_default(),
                        value(first),
                        value(second),
                    )
                })
                .collect()
        };
        if text {
            let width = first_name.chars().count().max(second_name.chars().count());
            for (_, first, second) in &rows {
                writeln!(
                    self.out,
                    "    {DIM}{first_name:<width$}{DIM:#}  {first_style}{first}{first_style:#}"
                )?;
                writeln!(
                    self.out,
                    "    {DIM}{second_name:<width$}{DIM:#}  {second_style}{second}{second_style:#}"
                )?;
            }
        } else {
            let at_width = rows.iter().map(|row| row.0.chars().count()).max();
            let at_width = at_width.unwrap_or(0);
            let first_width = rows.iter().map(|row| row.1.chars().count()).max();
            let first_width = first_width.unwrap_or(0);
            for (at, first, second) in &rows {
                let at = if at_width == 0 {
                    String::new()
                } else {
                    format!("{at:<at_width$}   ")
                };
                writeln!(
                    self.out,
                    "    {at}{DIM}{first_name}{DIM:#} {first_style}{first:<first_width$}{first_style:#}   {DIM}{second_name}{DIM:#} {second_style}{second}{second_style:#}"
                )?;
            }
        }
        let more = compare.differs.len() - shown + usize::try_from(compare.omitted).unwrap_or(0);
        if more > 0 {
            writeln!(self.out, "    {DIM}… {more} more{DIM:#}")?;
        }
        Ok(())
    }

    /// L3: every project frame, with its source, and each run of other
    /// frames as a line.
    fn frames(&mut self, frames: &[Frame]) -> io::Result<()> {
        let mut label = "frames";
        for item in shared::frame_items(frames) {
            match item {
                shared::FrameItem::Project(frame) => {
                    let mut line = self.location(frame.path.as_str(), frame.line);
                    if let Some(function) = &frame.function {
                        line = format!("{line} {DIM}in {function}{DIM:#}");
                    }
                    self.row(label, &line)?;
                    if let Some(source) = &frame.source {
                        write!(self.out, "{:pad$}", "", pad = LABEL + 6)?;
                        self.marked(source, frame.cols)?;
                    }
                }
                shared::FrameItem::Others { count, package } => {
                    let others = shared::others(count, &package);
                    self.row(label, &format!("{DIM}{others}{DIM:#}"))?;
                }
            }
            label = "";
        }
        Ok(())
    }

    /// L3: the innermost frame's local variables.
    fn locals(&mut self, locals: &[Local]) -> io::Result<()> {
        let mut label = "locals";
        for Local { name, value } in locals {
            self.row(label, &format!("{name} = {value}"))?;
            label = "";
        }
        Ok(())
    }

    /// The rows closing a block: hint, fixture, setup, history, not run,
    /// rerun, more.
    fn rows(&mut self, bad: &Bad<'_>, level: Level) -> io::Result<()> {
        let first = bad.failures.first();
        let mut rows: Vec<(&str, String)> = Vec::new();
        if let Some(hint) = first.and_then(|failure| failure.hint.as_deref()) {
            rows.push(("hint", hint.to_owned()));
        }
        if let Some(failure) = first
            && let Some(resource) = failure.resource.as_deref()
            && let (name, true) = shared::resource_name(resource, failure.phase)
        {
            rows.push(("fixture", name));
        }
        if let Some(result) = bad.result {
            if !result.resources.is_empty() {
                rows.push(("setup", shared::setup_chain(&result.resources)));
            }
            if let Some(history) = result.history.as_ref().and_then(shared::history) {
                rows.push(("history", history));
            }
        }
        let any = !rows.is_empty()
            || !bad.not_run.is_empty()
            || bad.rerun.is_some()
            || level == Level::L2;
        if !any {
            return Ok(());
        }
        writeln!(self.out)?;
        for (label, value) in &rows {
            self.row(label, value)?;
        }
        let shown = if level == Level::L3 {
            bad.not_run.len()
        } else {
            NOT_RUN.min(bad.not_run.len())
        };
        let mut label = "not run";
        for (id, key) in &bad.not_run[..shown] {
            self.row(label, &format!("{KEY}{key}{KEY:#}  {id}"))?;
            label = "";
        }
        if bad.not_run.len() > shown {
            let more = bad.not_run.len() - shown;
            self.row("", &format!("{DIM}… {more} more{DIM:#}"))?;
        }
        if let Some(rerun) = &bad.rerun {
            self.row("rerun", rerun)?;
        }
        if level == Level::L2 {
            self.row(
                "more",
                &format!(
                    "aye show {KEY}{}{KEY:#} -vv   {DIM}(every frame, output, locals){DIM:#}",
                    bad.key
                ),
            )?;
        }
        Ok(())
    }

    /// `  label    value`.
    fn row(&mut self, label: &str, value: &str) -> io::Result<()> {
        writeln!(self.out, "  {DIM}{label:<LABEL$}{DIM:#}  {value}")
    }

    /// The tail of each captured stream, `lines` lines each, a cut marked.
    fn output(&mut self, output: &Captured, lines: usize) -> io::Result<()> {
        for (name, captured) in [
            ("stdout", &output.stdout),
            ("stderr", &output.stderr),
            ("log", &output.log),
        ] {
            if captured.trim().is_empty() {
                continue;
            }
            let tail = shared::last_lines(captured, lines);
            let mut label = name;
            let earlier = tail.total - tail.lines.len();
            if earlier > 0 {
                let earlier = shared::plural(earlier, "earlier line", "earlier lines");
                self.row(label, &format!("{DIM}… {earlier}{DIM:#}"))?;
                label = "";
            } else if output.truncated {
                self.row(label, &format!("{DIM}… (cut){DIM:#}"))?;
                label = "";
            }
            for line in tail.lines {
                self.row(label, &shared::head(line, self.line_max()))?;
                label = "";
            }
        }
        Ok(())
    }

    /// `✗ 3fae6  <name>`, the name bold in a block, a note after it.
    fn head(
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

    /// `path:line`, cyan, and a hyperlink to the file on a live terminal
    /// with colour.
    fn location(&self, path: &str, line: u32) -> String {
        let text = format!("{CYAN}{path}:{line}{CYAN:#}");
        match self.root.as_deref().filter(|_| self.links) {
            Some(root) => format!(
                "\u{1b}]8;;{}\u{1b}\\{text}\u{1b}]8;;\u{1b}\\",
                file_url(root, path)
            ),
            None => text,
        }
    }

    fn worker_error(
        &mut self,
        worker: Option<u32>,
        status: Option<i32>,
        error: &str,
    ) -> io::Result<()> {
        let who = worker.map_or_else(|| "a worker".to_owned(), |w| format!("worker {w}"));
        let status = status
            .map(|status| format!(" (exit status {status})"))
            .unwrap_or_default();
        self.block()?;
        writeln!(
            self.out,
            "{RED_BOLD}{who} failed{RED_BOLD:#}{DIM}{status}{DIM:#}"
        )?;
        let lines = if self.full() {
            usize::MAX
        } else {
            WORKER_LINES
        };
        for line in shared::last_lines(error, lines).lines {
            writeln!(self.out, "  {}", shared::head(line, self.line_max()))?;
        }
        writeln!(self.out)
    }

    fn run_finished(
        &mut self,
        exit_code: ExitCode,
        counts: &Counts,
        duration: Duration,
        stopped: Option<&str>,
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
        let summary = !(tests_shown || (self.collect_only && exit_code == ExitCode::Ok));
        let grouped = !self.quiet()
            && ((!self.full() && !self.warnings.is_empty())
                || (self.show.is_none() && !self.collect_only && !self.skips.is_empty()));
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
            let mut codes: Vec<(&str, usize)> = Vec::new();
            for warning in &self.warnings {
                match codes.iter_mut().find(|(code, _)| *code == warning.code) {
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
        if self.show.is_none() && !self.collect_only && !self.skips.is_empty() {
            let mut skips = self.skips.clone();
            // Most first; a stable sort keeps ties in the order they came.
            skips.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let mut words: Vec<String> = skips
                .iter()
                .take(GROUPS)
                .map(|(reason, count)| format!("{} ({count})", shared::head(reason, REASON_MAX)))
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
        stopped: &str,
        exit_code: ExitCode,
        counts: &Counts,
    ) -> io::Result<()> {
        let why = match stopped {
            "maxfail" => format!(
                "stopped after {} (-x/--maxfail)",
                shared::plural(counts.bad(), "failure", "failures")
            ),
            "interrupted" if exit_code == ExitCode::Interrupted => return Ok(()),
            "interrupted" => "interrupted".to_owned(),
            "framework" => "stopped early by the framework".to_owned(),
            other => format!("stopped early: {other}"),
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
        let parts: Vec<String> = tally::parts(counts, &self.tally.extra())
            .into_iter()
            .map(|(n, one, many, tone)| {
                let words = shared::plural(n, one, many);
                match tone {
                    Tone::Bad if one == "failed" => format!("{RED_BOLD}✗ {words}{RED_BOLD:#}"),
                    Tone::Bad => format!("{YELLOW_BOLD}! {words}{YELLOW_BOLD:#}"),
                    Tone::Good => format!("{GREEN}✓ {words}{GREEN:#}"),
                    Tone::Warn => format!("{YELLOW}{words}{YELLOW:#}"),
                    Tone::Dim => format!("{DIM}{words}{DIM:#}"),
                }
            })
            .collect();
        let none = if self.collect_only {
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
            shared::seconds(duration)
        )
    }

    /// `details  aye show 3fae6 4d4f6 c3d4e  ·  every failure: aye show
    /// --failed`, in a run where something failed.
    fn details_line(&mut self) -> io::Result<()> {
        if self.show.is_some() || self.collect_only || self.bad_keys.is_empty() {
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

/// The indent of the line under a bad outcome's: past its symbol and key.
fn indent(key: &str) -> String {
    " ".repeat(key.chars().count() + 4)
}

/// Where in `source` the span `cols` is: its start and length in
/// characters, within the line, at least one long.
fn span(source: &str, cols: Columns) -> (usize, usize) {
    let length = source.chars().count();
    let start = usize::try_from(cols.start)
        .unwrap_or(0)
        .min(length.saturating_sub(1));
    let end = usize::try_from(cols.end)
        .unwrap_or(0)
        .clamp(start + 1, length.max(start + 1));
    (start, end - start)
}

/// A `file://` URL for `path`, relative to `root` unless absolute.
fn file_url(root: &str, path: &str) -> String {
    use std::fmt::Write as _;

    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    let full = if absolute {
        path.to_owned()
    } else {
        format!("{}/{path}", root.trim_end_matches(['/', '\\']))
    };
    let full = full.replace('\\', "/");
    let full = if full.starts_with('/') {
        full
    } else {
        format!("/{full}")
    };
    let mut url = String::from("file://");
    for byte in full.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

/// What failed of a lifetime: `setup of <lifetime>`, `teardown of
/// <lifetime>`, or the lifetime when its failure names no phase.
fn lifetime_name(lifetime: &str, failure: &Failure) -> String {
    match failure.phase {
        Some(Phase::Setup) => format!("setup of {lifetime}"),
        Some(Phase::Teardown) => format!("teardown of {lifetime}"),
        Some(Phase::Test) | None => lifetime.to_owned(),
    }
}

/// An outcome's symbol and colour: `✗` failed, `!` error, `✓` passed, and
/// none for the rest, which are words alone.
const fn symbol(outcome: Outcome) -> (&'static str, Style) {
    match outcome {
        Outcome::Passed => ("✓", GREEN),
        Outcome::Failed => ("✗", RED_BOLD),
        Outcome::Error => ("!", YELLOW_BOLD),
        Outcome::Skipped | Outcome::ExpectedFailure | Outcome::UnexpectedPass => (" ", YELLOW),
    }
}

impl<W: RawStream + AsLockedWrite> Reporter for Text<W> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        let noted = self.tally.count(record);
        self.keys.note(record);
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
        if let Event::WorkerStarted {
            python, version, ..
        } = &record.event
        {
            if let Some(python) = shared::python_version(python, version.as_deref()) {
                self.pythons.insert(worker, python);
            }
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
            Event::RunStarted {
                root,
                collect_only,
                workers,
                run,
                ..
            } => {
                self.collect_only = *collect_only;
                self.root = Some(root.clone());
                self.workers = *workers;
                self.run = *run;
                Ok(())
            }
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic, noted),
            Event::CaseCollected(case) if self.collect_only => {
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
            Event::SubOutcome {
                id,
                name,
                outcome,
                failure,
                duration,
                ..
            } if outcome.is_bad() => {
                let sub = HeldSub {
                    name: name.clone(),
                    outcome: *outcome,
                    failure: failure.clone(),
                    duration: *duration,
                    worker,
                };
                self.sub_outcome(id, sub);
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
            } => self.run_finished(*exit_code, counts, *duration, stopped.as_deref()),
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
