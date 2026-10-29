//! `--output json`: JSON lines, for programs and coding agents.
//!
//! No colour, no banners. Each line is one JSON object whose `type` says
//! what it is. Keys are stable: new keys may appear, existing ones keep
//! their meaning. Optional keys are left out rather than set to null, an
//! absent count is zero, and an absent list is empty.
//!
//! # The levels
//!
//! The verbosity (`-v` count minus `-q` count, clamped to -1..=3) says
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
//! project), `why` (the one line saying why: see [`shared::why`]; a bad
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

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::time::Duration;

use aye_events::{
    Captured, Certainty, Compare, Counts, Diagnostic, DiagnosticCode, Event, ExitCode, Failure,
    LifetimeId, Local, Outcome, Record, RunNumber, Severity, TestId, TestResult, WorkerIndex,
};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::tally::{Noted, Tally};
use crate::{Keys, Reporter, ShowContext, ShowMode, shared};

const ID_MAX: usize = 1000;
const PATH_MAX: usize = 500;
const CODE_MAX: usize = 300;
const SUMMARY_MAX: usize = 200;
const VALUE_MAX: usize = 200;
const OPERAND_MAX: usize = 500;
const SOURCE_MAX: usize = 200;
const MESSAGE_MAX: usize = 500;
const MESSAGE_LINES: usize = 10;
const HINT_MAX: usize = 500;
const FIXTURE_MAX: usize = 200;
const SETUP: usize = 30;
const DIFF_LINES: usize = 30;
const DIFF_LINE_MAX: usize = 200;
/// Entries of a structured comparison at L2.
const DIFFERENCES: usize = 20;
const FRAME_MAX: usize = 300;
const OUTPUT_LINES: usize = 50;
const OUTPUT_MAX: usize = 4000;
const LOCAL_MAX: usize = 200;
const ALSO: usize = 3;
const BLOCKED: usize = 20;
const RERUN_MAX: usize = 1000;
const COLLECTION_ERROR_MAX: usize = 1000;
const DIAGNOSTIC_CODE_MAX: usize = 100;
const WORKER_ERROR_MAX: usize = 2000;
const REASON_MAX: usize = 500;
/// Keys in the summary's `failures`.
const FAILURES: usize = 50;
/// Bad sub-outcomes of one test at their level; later ones are at L1.
const SUB_LINES: usize = 3;

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
    collect_only: bool,
    /// From `run_started`.
    run: Option<RunNumber>,
    /// From `collection_finished`.
    selected: Option<u32>,
    /// Each worker's Python version, from its `worker_started`.
    pythons: HashMap<Option<WorkerIndex>, String>,
    /// The first bad outcome reported with each signature: its key and
    /// message.
    signatures: HashMap<String, (String, String)>,
    /// Tests blocked by a lifetime whose `lifetime_finished` has not
    /// arrived, with their keys.
    pending: BTreeMap<LifetimeId, Vec<(TestId, String)>>,
    /// Lifetimes reported, and the signature they were reported with.
    reported: HashMap<LifetimeId, String>,
    /// Bad sub-outcomes of tests that haven't finished, held so that the
    /// first can carry the test's output and rerun.
    held: BTreeMap<TestId, Vec<HeldSub>>,
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

/// A bad sub-outcome, until its test finishes.
struct HeldSub {
    name: String,
    outcome: Outcome,
    failure: Option<Failure>,
    duration: Duration,
    worker: Option<WorkerIndex>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line<'a> {
    Case {
        id: Cow<'a, str>,
        #[serde(skip_serializing_if = "is_false")]
        partial: bool,
    },
    Fail(Box<Fail<'a>>),
    LifetimeFail(Box<LifetimeFail<'a>>),
    CollectionError {
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        code: Cow<'a, str>,
        error: Cow<'a, str>,
    },
    WorkerError {
        #[serde(skip_serializing_if = "Option::is_none")]
        worker: Option<WorkerIndex>,
        error: Cow<'a, str>,
    },
    Test {
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        id: Cow<'a, str>,
        outcome: &'static str,
        ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Cow<'a, str>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<Cow<'a, str>>,
    },
    Diagnostic {
        severity: &'static str,
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        code: Cow<'a, str>,
        message: Cow<'a, str>,
    },
    Stale {
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rerun: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        gone: bool,
    },
    Summary(Summary),
}

#[derive(Serialize)]
struct Fail<'a> {
    key: String,
    id: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<Cow<'a, str>>,
    outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cause: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
    #[serde(flatten)]
    shown: Option<Shown>,
    #[serde(flatten)]
    detail: Detail<'a>,
}

impl<'a> Fail<'a> {
    fn new(key: String, id: &'a TestId, outcome: Outcome) -> Self {
        Self {
            key,
            id: shared::head(id.as_str(), ID_MAX),
            sub: None,
            outcome: outcome.as_str(),
            phase: None,
            cause: None,
            same_as: None,
            at: None,
            why: None,
            shown: None,
            detail: Detail::default(),
        }
    }
}

#[derive(Serialize)]
struct LifetimeFail<'a> {
    key: String,
    lifetime: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    same_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    why: Option<String>,
    #[serde(flatten)]
    shown: Option<Shown>,
    #[serde(flatten)]
    detail: Detail<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    blocked: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    blocked_omitted: usize,
}

/// What `aye show` adds to a bad outcome's line.
#[derive(Serialize)]
struct Shown {
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    python: Option<String>,
    stale: bool,
}

/// L2 and L3.
#[derive(Serialize, Default)]
struct Detail<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cols: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    before: Vec<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    diff: Option<Diff<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left_source: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    right_source: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    right: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hint: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fixture: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    setup: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    history: Option<History>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    also: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rerun: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    frames: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<Output>,
    #[serde(skip_serializing_if = "Option::is_none")]
    locals: Option<Locals<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Diff<'a> {
    Compare(CompareLine<'a>),
    Lines(Vec<Cow<'a, str>>),
}

#[derive(Serialize)]
struct CompareLine<'a> {
    kind: Cow<'a, str>,
    #[serde(skip_serializing_if = "str::is_empty")]
    summary: Cow<'a, str>,
    #[serde(skip_serializing_if = "is_zero")]
    same: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    differs: Vec<Entry<'a>>,
    #[serde(skip_serializing_if = "is_zero")]
    omitted: u32,
}

#[derive(Serialize)]
struct Entry<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actual: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    right: Option<Cow<'a, str>>,
}

#[derive(Serialize)]
struct History {
    previous: usize,
    #[serde(skip_serializing_if = "is_zero")]
    passed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    failed: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
}

/// The innermost frame's locals, as an object in their order.
struct Locals<'a>(&'a [Local]);

impl Serialize for Locals<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for local in self.0 {
            map.serialize_entry(
                &shared::head(&local.name, LOCAL_MAX),
                &shared::head(&local.value, LOCAL_MAX),
            )?;
        }
        map.end()
    }
}

#[derive(Serialize)]
struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log: Option<String>,
}

#[derive(Serialize)]
struct Summary {
    #[serde(skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    exit: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    tests: Option<u32>,
    #[serde(skip_serializing_if = "is_zero")]
    failed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    errors: u32,
    #[serde(skip_serializing_if = "is_zero")]
    passed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    skipped: u32,
    #[serde(skip_serializing_if = "is_zero")]
    expected_failure: u32,
    #[serde(skip_serializing_if = "is_zero")]
    unexpected_pass: u32,
    #[serde(skip_serializing_if = "is_zero")]
    deselected: u32,
    #[serde(skip_serializing_if = "is_zero")]
    setup_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    teardown_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    lifetime_error: u32,
    s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failures: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    failures_omitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    more: Option<&'static str>,
}

fn is_zero<T: Default + PartialEq>(n: &T) -> bool {
    *n == T::default()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature fixed by serde"
)]
const fn is_false(b: &bool) -> bool {
    !*b
}

/// `at`: where the failure is shown.
fn at(failure: &Failure) -> Option<String> {
    shared::origin(failure)
        .map(|(path, line)| shared::head(&format!("{path}:{line}"), PATH_MAX).into_owned())
}

/// The whole message, when it spans lines: its first line, then at most
/// 10 more, bounded.
fn message(failure: &Failure) -> Option<Cow<'_, str>> {
    let rest = shared::message_rest(failure);
    if rest.is_empty() {
        return None;
    }
    let mut lines = vec![shared::message(failure)];
    lines.extend(rest.iter().take(MESSAGE_LINES));
    if rest.len() > MESSAGE_LINES {
        lines.push("…");
    }
    Some(Cow::Owned(
        shared::head(&lines.join("\n"), MESSAGE_MAX).into_owned(),
    ))
}

/// The structured comparison, bounded.
fn compare_line<'a>(
    assertion: &'a aye_events::Assertion,
    compare: &'a Compare,
    all: bool,
) -> CompareLine<'a> {
    let shown = if all {
        compare.differs.len()
    } else {
        DIFFERENCES.min(compare.differs.len())
    };
    let known = assertion.expected.is_some();
    let value = |value: Option<&'a str>| value.map(|value| shared::head(value, VALUE_MAX));
    let differs = compare.differs[..shown]
        .iter()
        .map(|difference| {
            let at = difference
                .at
                .as_deref()
                .map(|at| shared::head(at, VALUE_MAX));
            if known {
                let (expected, actual) = shared::entries(assertion, difference);
                Entry {
                    at,
                    expected: value(expected),
                    actual: value(actual),
                    left: None,
                    right: None,
                }
            } else {
                Entry {
                    at,
                    expected: None,
                    actual: None,
                    left: value(difference.left.as_deref()),
                    right: value(difference.right.as_deref()),
                }
            }
        })
        .collect();
    let omitted = u32::try_from(compare.differs.len() - shown)
        .unwrap_or(u32::MAX)
        .saturating_add(compare.omitted);
    CompareLine {
        kind: shared::head(&compare.kind, VALUE_MAX),
        summary: shared::head(&compare.summary, SUMMARY_MAX),
        same: compare.same,
        differs,
        omitted,
    }
}

/// The explanation, bounded; none when it only restates the operands,
/// which the line carries already.
fn bounded_diff(assertion: &aye_events::Assertion) -> Vec<Cow<'_, str>> {
    let mut lines = shared::explanation(assertion);
    if shared::only_restates(assertion, &lines) {
        lines.clear();
    }
    if lines.len() <= DIFF_LINES {
        return lines
            .into_iter()
            .map(|line| shared::head(line, DIFF_LINE_MAX))
            .collect();
    }
    let more = lines.len() - (DIFF_LINES - 1);
    lines
        .into_iter()
        .take(DIFF_LINES - 1)
        .map(|line| shared::head(line, DIFF_LINE_MAX))
        .chain([Cow::Owned(format!("… {more} more lines"))])
        .collect()
}

/// A failure's comparison, at L2: the structured one, or the operands and
/// the explanation.
fn comparison<'a>(detail: &mut Detail<'a>, failure: &'a Failure, all: bool) {
    let Some(assertion) = &failure.assertion else {
        return;
    };
    let source = |source: Option<&'a String>| {
        source.map(|source| {
            Cow::Owned(shared::head(&shared::first_line(source), SOURCE_MAX).into_owned())
        })
    };
    let known = assertion.expected.is_some();
    let operands = if let Some(compare) = &assertion.compare {
        detail.diff = Some(Diff::Compare(compare_line(assertion, compare, all)));
        compare.differs.is_empty()
    } else {
        let lines = bounded_diff(assertion);
        let spans_lines = assertion.left.contains('\n') || assertion.right.contains('\n');
        if !lines.is_empty() {
            detail.diff = Some(Diff::Lines(lines));
        }
        // Not when the message holds both as written, nor when they span
        // lines and the explanation shows how they differ.
        let in_message =
            failure.message.contains(&assertion.left) && failure.message.contains(&assertion.right);
        !(in_message || spans_lines && detail.diff.is_some())
    };
    if !known {
        detail.left_source = source(assertion.left_source.as_ref());
        detail.right_source = source(assertion.right_source.as_ref());
    }
    if operands {
        if known {
            let (expected, actual) = shared::whole(assertion);
            detail.expected = Some(shared::head(expected, OPERAND_MAX));
            detail.actual = Some(shared::head(actual, OPERAND_MAX));
        } else {
            detail.left = Some(shared::head(&assertion.left, OPERAND_MAX));
            detail.right = Some(shared::head(&assertion.right, OPERAND_MAX));
        }
    }
}

/// L3's frames: each project frame, and each run of others as one entry.
fn frames(failure: &Failure) -> Vec<String> {
    shared::frame_items(&failure.frames)
        .into_iter()
        .map(|item| match item {
            shared::FrameItem::Project(frame) => {
                shared::head(&shared::frame_line(frame), FRAME_MAX).into_owned()
            }
            shared::FrameItem::Others { count, package } => shared::others(count, &package),
        })
        .collect()
}

fn output_tails(output: &Captured) -> Option<Output> {
    let tail = |captured: &str| {
        if captured.trim().is_empty() {
            return None;
        }
        let lines = shared::last_lines(captured, OUTPUT_LINES);
        let mut shown = lines.lines.join("\n");
        if lines.total > lines.lines.len() {
            shown = format!(
                "[{} earlier lines omitted]\n{shown}",
                lines.total - lines.lines.len()
            );
        }
        Some(shared::tail(&shown, OUTPUT_MAX).into_owned())
    };
    let output = Output {
        stdout: tail(&output.stdout),
        stderr: tail(&output.stderr),
        log: tail(&output.log),
    };
    (output.stdout.is_some() || output.stderr.is_some() || output.log.is_some()).then_some(output)
}

impl<W: Write> JsonLines<W> {
    /// `verbosity`: `-v` count minus `-q` count; clamped to -1..=3 (see the
    /// module's docs).
    pub fn new(out: W, verbosity: i8) -> Self {
        Self {
            out,
            verbosity: verbosity.clamp(QUIET, ALL),
            collect_only: false,
            run: None,
            selected: None,
            pythons: HashMap::new(),
            signatures: HashMap::new(),
            pending: BTreeMap::new(),
            reported: HashMap::new(),
            held: BTreeMap::new(),
            diagnostics: HashSet::new(),
            tally: Tally::default(),
            keys: Keys::new(),
            failures: Vec::new(),
            failures_seen: HashSet::new(),
            show: None,
            context: ShowContext::default(),
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

    /// Before anything else, for `aye show`: a `stale` line, `path` having
    /// changed since the run (or being `gone`). None at `-q`.
    pub fn stale(&mut self, path: &str, gone: bool) -> io::Result<()> {
        if self.verbosity <= QUIET {
            return Ok(());
        }
        self.line(&Line::Stale {
            path: shared::head(path, PATH_MAX),
            rerun: (!gone).then(|| shared::head(&shared::rerun_path(path), RERUN_MAX).into_owned()),
            gone,
        })
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

    /// The key of a test's or a lifetime's id.
    fn key(&self, id: &str) -> String {
        self.keys.key(id).map_or_else(
            || aye_events::TestKey::of(id).prefix(crate::keys::MIN),
            str::to_owned,
        )
    }

    /// The summary's `failures` counts `id`'s key.
    fn note_bad(&mut self, id: &str) {
        let key = self.key(id);
        if self.failures_seen.insert(key.clone()) {
            self.failures.push(key);
        }
    }

    /// What `aye show`'s details add to a bad outcome's line about `path`,
    /// run by `worker`.
    fn shown(&self, path: &str, worker: Option<WorkerIndex>) -> Option<Shown> {
        if self.show != Some(ShowMode::Details) {
            return None;
        }
        Some(Shown {
            run: self.context.run.or(self.run).map(|run| run.to_string()),
            python: self.pythons.get(&worker).cloned(),
            stale: self.context.stale.contains_key(path),
        })
    }

    /// The first reported with this failure's signature (its key, or a
    /// sub-outcome's `<key> › <name>`), and its message; records `key` as
    /// the first when there is none yet (none is grouped at L3).
    fn first_with(&mut self, failure: &Failure, key: &str) -> Option<(String, String)> {
        if failure.signature.is_empty() || self.all() {
            return None;
        }
        if let Some(first) = self.signatures.get(&failure.signature) {
            return Some(first.clone());
        }
        self.signatures.insert(
            failure.signature.clone(),
            (key.to_owned(), shared::message(failure).to_owned()),
        );
        None
    }

    /// L2 and L3 of a failure.
    fn detail<'a>(
        &self,
        failure: &'a Failure,
        later: &'a [Failure],
        result: Option<&'a TestResult>,
        duration: Duration,
        rerun: Option<String>,
    ) -> Detail<'a> {
        let mut detail = Detail::default();
        if !self.detailed() {
            return detail;
        }
        if let Some(frame) = shared::pointed(failure) {
            // The failing line at its indentation relative to `before`.
            let indent = if frame.before.is_empty() {
                0
            } else {
                frame.indent.min(40)
            };
            detail.code = frame.source.as_deref().map(|source| {
                let source = shared::head(source, CODE_MAX);
                if indent == 0 {
                    source
                } else {
                    Cow::Owned(format!("{:indent$}{source}", "", indent = indent as usize))
                }
            });
            detail.cols = frame
                .cols
                .filter(|_| frame.source.is_some())
                .map(|cols| [cols.start + indent, cols.end + indent]);
            detail.before = frame
                .before
                .iter()
                .map(|line| shared::head(line, CODE_MAX))
                .collect();
        }
        comparison(&mut detail, failure, self.all());
        // A structured comparison, or the expected and actual values, say
        // what the message's lines would.
        let compared = matches!(detail.diff, Some(Diff::Compare(_))) || detail.expected.is_some();
        detail.message = if compared {
            // But the test's own message, when it gave one.
            failure
                .assertion
                .as_ref()
                .and_then(|assertion| shared::own_message(failure, assertion))
                .map(|own| shared::head(own, MESSAGE_MAX))
        } else {
            message(failure)
        };
        detail.hint = failure
            .hint
            .as_deref()
            .map(|hint| shared::head(hint, HINT_MAX));
        detail.fixture = failure
            .resource
            .as_deref()
            .map(|resource| shared::resource_name(resource, failure.phase))
            .filter(|(_, fixture)| *fixture)
            .map(|(name, _)| Cow::Owned(shared::head(&name, FIXTURE_MAX).into_owned()));
        if let Some(result) = result {
            detail.setup = result
                .resources
                .iter()
                .take(SETUP)
                .map(|resource| {
                    let (name, _) = shared::resource_name(&resource.name, None);
                    shared::head(&format!("{name}:{}", resource.scope), FIXTURE_MAX).into_owned()
                })
                .collect();
            detail.history = result.history.as_ref().and_then(|previous| {
                let total = previous.outcomes.chars().count();
                let (passed, failed) = shared::history_counts(previous);
                (total > 0).then(|| History {
                    previous: total,
                    passed,
                    failed,
                    since: previous.since.map(|since| since.to_string()),
                })
            });
        }
        detail.also = later
            .iter()
            .take(ALSO)
            .map(|also| {
                let phase = also.phase.map_or("test", shared::phase);
                shared::head(&format!("{phase}: {}", shared::message(also)), MESSAGE_MAX)
                    .into_owned()
            })
            .collect();
        detail.rerun = rerun.map(|rerun| shared::head(&rerun, RERUN_MAX).into_owned());
        detail.ms = Some(shared::millis(duration));
        if self.all() {
            detail.frames = frames(failure);
            detail.output = result.and_then(|result| output_tails(&result.output));
            let locals = shared::locals(failure);
            detail.locals = (!locals.is_empty()).then_some(Locals(locals));
        }
        detail
    }

    /// The test's output and rerun, on the line of a bad outcome without a
    /// failure of its own, or on the first of its sub-outcomes'.
    fn carry(&self, detail: &mut Detail<'_>, result: &TestResult) {
        if !self.detailed() {
            return;
        }
        detail.rerun = Some(
            shared::head(
                &shared::rerun(&result.id, result.reproduce.as_ref()),
                RERUN_MAX,
            )
            .into_owned(),
        );
        if self.all() {
            detail.output = output_tails(&result.output);
        }
    }

    fn test(&mut self, result: &TestResult, worker: Option<WorkerIndex>) -> io::Result<()> {
        // A test that failed only through its sub-outcomes gets no line of
        // its own: the first of theirs carries its output and rerun. What
        // they reported (the unittest and doctest adapters fail the test
        // with the same failures) isn't reported again.
        let subs = self.held.remove(&result.id).unwrap_or_default();
        let reported: Vec<Failure> = subs.iter().filter_map(|sub| sub.failure.clone()).collect();
        let own: Vec<Failure> = if subs.is_empty() {
            result.failures.clone()
        } else {
            result
                .failures
                .iter()
                .filter(|failure| !shared::repeats_subs(failure, &reported))
                .cloned()
                .collect()
        };
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
        let key = self.key(result.id.as_str());
        if let Some(lifetime) = &result.cause {
            if self.reported.contains_key(lifetime) {
                return self.caused(&result.id, key, result.outcome, lifetime);
            }
            self.pending
                .entry(lifetime.clone())
                .or_default()
                .push((result.id.clone(), key));
            return Ok(());
        }
        let Some(first) = own.first() else {
            let mut fail = Fail::new(key, &result.id, result.outcome);
            fail.why = shared::bare_why(&result.output);
            fail.shown = self.shown(result.id.path().as_str(), worker);
            self.carry(&mut fail.detail, result);
            if self.detailed() {
                fail.detail.ms = Some(shared::millis(result.duration));
                if !self.all() {
                    fail.detail.output = output_tails(&result.output);
                }
            }
            return self.line(&Line::Fail(Box::new(fail)));
        };
        let mut fail = Fail::new(key.clone(), &result.id, result.outcome);
        fail.phase = shared::notable_phase(first);
        fail.at = at(first);
        fail.shown = self.shown(result.id.path().as_str(), worker);
        if let Some((same, message)) = self.first_with(first, &key) {
            fail.same_as = Some(same);
            if message != shared::message(first) {
                fail.why = Some(shared::why(first, true));
            }
        } else {
            fail.why = Some(shared::why(first, true));
            let rerun = shared::rerun(&result.id, result.reproduce.as_ref());
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
        let key = self.key(id.as_str());
        let mut fail = Fail::new(key.clone(), id, sub.outcome);
        fail.sub = Some(shared::head(&sub.name, ID_MAX));
        fail.shown = self.shown(id.path().as_str(), sub.worker);
        if let Some(failure) = &sub.failure {
            fail.phase = shared::notable_phase(failure);
            fail.at = at(failure);
            // A sub-outcome is named by its test's key and its name.
            let label = format!("{key} › {}", sub.name);
            if let Some((same, message)) = self.first_with(failure, &label) {
                fail.same_as = Some(same);
                if message != shared::message(failure) {
                    fail.why = Some(shared::why(failure, false));
                }
            } else {
                fail.why = Some(shared::why(failure, false));
                if !brief {
                    let rerun = shared::rerun(id, carried.and_then(|r| r.reproduce.as_ref()));
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
        for (id, subs) in std::mem::take(&mut self.held) {
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
        fail.cause = Some(shared::head(lifetime.as_str(), ID_MAX));
        self.line(&Line::Fail(Box::new(fail)))
    }

    fn lifetime(
        &mut self,
        lifetime: &LifetimeId,
        failure: Option<&Failure>,
        duration: Duration,
        worker: Option<WorkerIndex>,
    ) -> io::Result<()> {
        let blocked = self.pending.remove(lifetime).unwrap_or_default();
        let fresh = failure.filter(|failure| {
            self.reported.get(lifetime).map(String::as_str) != Some(failure.signature.as_str())
        });
        let Some(failure) = fresh else {
            for (id, key) in blocked {
                self.caused(&id, key, Outcome::Error, lifetime)?;
            }
            return Ok(());
        };
        self.reported
            .insert(lifetime.clone(), failure.signature.clone());
        let key = self.key(lifetime.as_str());
        let mut line = LifetimeFail {
            key: key.clone(),
            lifetime: shared::head(lifetime.as_str(), ID_MAX),
            phase: failure.phase.map(shared::phase),
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
            if message != shared::message(failure) {
                line.why = Some(shared::why(failure, false));
            }
        } else {
            line.why = Some(shared::why(failure, false));
            let rerun = blocked.first().map(|(id, _)| shared::rerun(id, None));
            line.detail = self.detail(failure, &[], None, duration, rerun);
        }
        self.line(&Line::LifetimeFail(Box::new(line)))
    }

    /// A failed lifetime's line in `aye show`'s list, once per lifetime,
    /// at L1 whatever the level.
    fn lifetime_list_line(&mut self, lifetime: &LifetimeId, failure: &Failure) -> io::Result<()> {
        if self.reported.contains_key(lifetime) {
            return Ok(());
        }
        self.reported
            .insert(lifetime.clone(), failure.signature.clone());
        self.line(&Line::LifetimeFail(Box::new(LifetimeFail {
            key: self.key(lifetime.as_str()),
            lifetime: shared::head(lifetime.as_str(), ID_MAX),
            phase: failure.phase.map(shared::phase),
            same_as: None,
            at: at(failure),
            why: Some(shared::why(failure, false)),
            shown: None,
            detail: Detail::default(),
            blocked: Vec::new(),
            blocked_omitted: 0,
        })))
    }

    /// A test's line in `aye show`'s list.
    fn list_line(&mut self, result: &TestResult) -> io::Result<()> {
        let why = if result.outcome.is_bad() && result.cause.is_none() {
            match result.failure() {
                Some(failure) => Some(shared::why(failure, true)),
                None => shared::bare_why(&result.output),
            }
        } else {
            None
        };
        self.line(&Line::Test {
            key: Some(self.key(result.id.as_str())),
            id: shared::head(result.id.as_str(), ID_MAX),
            outcome: result.outcome.as_str(),
            ms: shared::millis(result.duration),
            why,
            reason: result
                .reason
                .as_deref()
                .map(|reason| shared::head(reason, REASON_MAX)),
            cause: result
                .cause
                .as_ref()
                .map(|cause| shared::head(cause.as_str(), ID_MAX)),
        })
    }

    /// A module skipped while collecting, in `aye show`'s list: a `test`
    /// line, named after its file, as pytest counts it as a skipped item.
    fn module_skip_line(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        self.line(&Line::Test {
            key: None,
            id: shared::head(diagnostic.path.as_str(), ID_MAX),
            outcome: Outcome::Skipped.as_str(),
            ms: 0,
            why: None,
            reason: Some(shared::head(&diagnostic.message, REASON_MAX)),
            cause: None,
        })
    }

    /// Tests blocked by lifetimes that never reported a failure.
    fn flush_pending(&mut self) -> io::Result<()> {
        for (lifetime, ids) in std::mem::take(&mut self.pending) {
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
        let path = shared::head(diagnostic.path.as_str(), PATH_MAX);
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        let code = shared::head(diagnostic.code.as_str(), DIAGNOSTIC_CODE_MAX);
        let message = shared::head(&diagnostic.message, COLLECTION_ERROR_MAX);
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
            error: shared::tail(error.trim_end(), WORKER_ERROR_MAX),
        })
    }

    fn summary(
        &mut self,
        exit_code: ExitCode,
        counts: &Counts,
        duration: Duration,
        stopped: Option<&str>,
    ) -> io::Result<()> {
        if self.lines() {
            self.flush_held()?;
            self.flush_pending()?;
        }
        let extra = self.tally.extra();
        let failures = if self.verbosity > QUIET {
            std::mem::take(&mut self.failures)
        } else {
            Vec::new()
        };
        // Details are what `more` would show.
        let more = (!failures.is_empty() && self.show != Some(ShowMode::Details))
            .then_some("aye show <key> --output json");
        let omitted = failures.len().saturating_sub(FAILURES);
        self.line(&Line::Summary(Summary {
            run: self.context.run.or(self.run).map(|run| run.to_string()),
            exit: u8::from(exit_code),
            tests: self.selected,
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
            s: shared::secs(duration),
            stopped: stopped.map(str::to_owned),
            failures: failures.into_iter().take(FAILURES).collect(),
            failures_omitted: omitted,
            more,
        }))?;
        self.out.flush()
    }

    /// What every level folds, whatever it writes: the run's number, the
    /// selected count, the workers' Pythons, and the keys of what failed.
    fn fold(&mut self, record: &Record) {
        match &record.event {
            Event::RunStarted {
                collect_only, run, ..
            } => {
                self.collect_only = *collect_only;
                self.run = *run;
            }
            Event::CollectionFinished { selected, .. } => self.selected = Some(*selected),
            Event::WorkerStarted {
                python, version, ..
            } => {
                if let Some(python) = shared::python_version(python, version.as_deref()) {
                    self.pythons.insert(record.envelope.worker, python);
                }
            }
            Event::TestFinished(result) if result.outcome.is_bad() => {
                self.note_bad(result.id.as_str());
            }
            Event::SubOutcome { id, outcome, .. } if outcome.is_bad() => {
                self.note_bad(id.as_str());
            }
            Event::LifetimeFinished {
                lifetime,
                failure: Some(_),
                ..
            } => self.note_bad(lifetime.as_str()),
            _ => {}
        }
    }
}

impl<W: Write> Reporter for JsonLines<W> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
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
            return self.summary(*exit_code, counts, *duration, stopped.as_deref());
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
            Event::CaseCollected(case) if self.collect_only => self.line(&Line::Case {
                id: shared::head(case.id.as_str(), ID_MAX),
                partial: case.certainty == Certainty::Partial,
            }),
            _ if !self.lines() => Ok(()),
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic),
            Event::LifetimeFinished {
                lifetime,
                failure,
                duration,
            } => self.lifetime(lifetime, failure.as_ref(), *duration, worker),
            Event::SubOutcome {
                id,
                name,
                outcome,
                failure,
                duration,
                ..
            } if outcome.is_bad() => {
                self.held.entry(id.clone()).or_default().push(HeldSub {
                    name: name.clone(),
                    outcome: *outcome,
                    failure: failure.clone(),
                    duration: *duration,
                    worker,
                });
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
