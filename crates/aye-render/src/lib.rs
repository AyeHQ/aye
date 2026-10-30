//! Renderers. Each is a fold over the event stream and sees nothing else:
//! text (a short run: a line per bad outcome, the details one command
//! away), JSON lines (from the summary and the keys of what failed to
//! everything about each bad outcome, bounded), JUnit XML, GitHub
//! annotations, and later the TUI.
//!
//! Owns: presentation: what each format shows of a record, in what words,
//! layout and limits. If a renderer needs more, the schema grows; runner
//! state never passes around it. What the renderers derive from the log
//! alike (keys, the tally, held sub-outcomes and blocked tests, the first of
//! each signature, definition lines, the run's facts) are aye-stream's
//! folds, and each renderer is an `aye_stream::Consumer`.
//!
//! Depends on: aye-stream and aye-events. It reads no environment: the
//! binary says what it found there (whether to annotate for GitHub, the
//! colour choice, a live terminal).
//!
//! Decided (implementation, October 2026):
//! - **Two formats, one ladder.** `--output text` ([`Text`]) and
//!   `--output json` ([`JsonLines`]) take a [`Level`] (`-v` count minus
//!   `-q` count, clamped to -1..=3) and show a bad outcome (a failed or
//!   errored test, a failed sub-outcome, a failed lifetime) at one of
//!   three levels of detail ([`DetailLevel`]), the same in both: L1, its
//!   line (key, id, outcome, phase, where, and why in one line); L2, the
//!   detail (the pointed code, the structured comparison or the
//!   exception's message, the hint, the fixture that broke, the setup
//!   chain, history, the rerun, the duration); L3, everything (every
//!   frame, the captured output, the innermost frame's locals). Text shows
//!   L1 by default, L2 at `-v`, L3 at `-vv`; JSON lines the summary alone
//!   by default (with the keys of what failed), L1 lines at `-v`, L2 at
//!   `-vv`, L3 at `-vvv`: a table each ([`Ladder::TEXT`],
//!   [`Ladder::JSON`]), tested against `docs/architecture.md`'s. `aye
//!   show` takes the same flags with the same meaning, its details at L2
//!   without them ([`Ladder::first_at`]). Each module's docs give its
//!   format exactly.
//! - **Keys** (`aye_stream::Keys`): every test and lifetime is named by its
//!   key, the same in every format and in `aye show` later.
//! - **Why, in one line** (`failure::why`), the same in both formats: a
//!   test's failed teardown says it passed, a fixture's failed setup names
//!   it, a comparison says what was expected and what was got, and
//!   otherwise the message speaks, with its hint.
//! - **A long collection shows its progress** in text output on a terminal
//!   (`TextOptions::live`): each `collection_progress` rewrites one line,
//!   padded with spaces rather than cleared with escapes so it reads the same
//!   with colour off, and the line is blanked before anything else is
//!   written. Off a terminal, and in every other renderer, the event changes
//!   nothing.
//! - **The fold.** Each renderer receives every record in log order and is
//!   told when the stream ends (`aye_stream::Consumer`). Records of kinds a
//!   renderer does not know are ignored.
//! - **Colour** goes through `anstream`, so [`ColorChoice::Auto`] honours the
//!   terminal, `NO_COLOR` and `CLICOLOR_FORCE`, and `Never` strips every
//!   escape, including any in captured output.
//! - **A failure's text.** The message is one line on the wire; the rest of
//!   a message that spans lines (`self.fail("a\nb")`, an exception's text,
//!   pytest's `+  where` lines) is read from the framework's `detail`, after
//!   the last line holding the message (Python's `Type: message`, pytest's
//!   `E   Type: message`), unless a comparison's `diff` explains it. A
//!   comparison's `left` and `right` are shown only when they add to the
//!   message: not when it holds both as written, nor when the explanation
//!   restates them (a doctest's `-3`, `+2`) or shows how values that span
//!   lines differ. A lone unified-diff hunk header (`@@ -1 +1 @@`, from a
//!   doctest's output) is dropped. When the engine sends a structured
//!   comparison (`compare`), the source of each operand and which is the
//!   expected one, the renderers show those instead, and fall back to the
//!   above for logs without them (older logs, the unittest adapter, a
//!   crash).
//! - **Modules skipped while collecting** (pytest's `skip(...,
//!   allow_module_level=True)`, `importorskip`) reach the stream only as a
//!   `skipped` info diagnostic from collection: no test of the module runs,
//!   and `run_finished`'s counts hold test results only. pytest counts each
//!   as one skipped item, so the text and JSON summaries add one skipped
//!   per such module (once per path), and JUnit has a skipped test case for
//!   it, named after the module. A collect-only run counts none, as `pytest
//!   --co` doesn't.
//! - **Failed lifetimes that kept no test from running** (a failed
//!   `tearDownClass` or `tearDownModule`, a class or module cleanup that
//!   raised, a failed setup whose tests the run stopped before under
//!   `maxfail`) fail the run, but no test result holds them and
//!   `run_finished`'s counts hold test results only. So the summaries
//!   count them from the stream, by the phase that failed, once per
//!   lifetime: the text line as `1 setup error` and `1 teardown error`
//!   (`1 lifetime error` for a failure that names no phase), after the
//!   test counts as the diagnostics' kinds are (`✓ 1 passed · ! 1
//!   teardown error`, where pytest says `1 passed, 1 error` and unittest
//!   `FAILED (errors=1)`), and the JSON summary as `setup_error`,
//!   `teardown_error` and `lifetime_error`. A lifetime some test names as
//!   its `cause` counts through that test instead. JUnit has a test case
//!   for each, and GitHub an annotation.
//! - **Lifetimes are not paired.** A worker that crashes, or that a stop
//!   kills, leaves the lifetimes it started without a
//!   `lifetime_finished`; no renderer pairs it with its `lifetime_started`
//!   (only text at `-vvv` reads that, as a line of its own), and a test
//!   whose failed lifetime never reports is shown on its own.
//! - **Error diagnostics** are shown once (`aye_stream::SeenDiagnostics`):
//!   one that every batch repeats (the framework rejecting its arguments)
//!   is one line, one annotation, one count.
//! - **Failure signatures** come from the engine; failures without one are
//!   never grouped, and none is at L3: below it, a failure whose signature
//!   an earlier one had names that one's key (`same as 3fae6`, `same_as`).
//!   Where a failure is shown at one place, it is where it was raised when
//!   that is in the project, else the innermost project frame (paths
//!   outside the project are absolute or start with `..`).
//! - **JUnit XML** ([`Junit`]): written on `finish`. `line` is 1-based (pytest
//!   writes 0-based); `expected_failure` counts as skipped and
//!   `unexpected_pass` as passed; collection errors, and failed lifetimes that
//!   kept no test from running, become test cases with an `<error>`; each
//!   test a failed lifetime kept from running gets an `<error>` naming the
//!   lifetime and its message (`setup of <lifetime> failed: <message>`) with
//!   its text, as pytest reports each test a failed fixture kept from
//!   running; each bad sub-outcome is a `<failure>` of its test (named
//!   `<name>: <message>`, so a doctest's names its failing example), before
//!   the test's own, as in pytest's report, and the test's own failures
//!   that repeat theirs are left out, so a doctest is one test case with one
//!   `<failure>` per failing example; a prefix (pytest's `--junitprefix`)
//!   goes before every `classname`; characters XML cannot carry are shown
//!   as `#xNN`, as pytest does. A report that can't be written is an error
//!   naming the file. What tests record goes in as pytest puts it: a test
//!   case's `properties` first in it, the run's (`run_property`, each pair
//!   once, however many batches' sessions recorded it) first in the suite.
//!   The framework's settings ([`JunitOptions`], pytest's `junit_*`, which
//!   the binary reads from the adapters' contexts) shape the rest as they
//!   shape pytest's report: the suite's name; which captured output a test
//!   case carries, each stream under pytest's header (written even when
//!   empty, as pytest does); whether a test that didn't fail carries any
//!   (pytest's rule: not one that only failed to set up); whether `time`
//!   is the body's alone (`body_duration_ms`); and whether test cases have
//!   `file` and `line` (xunit1) or not (xunit2). Without them, aye's own
//!   layout: the suite `aye`, `file` and `line`, stdout then the log in
//!   `<system-out>`, stderr in `<system-err>`, only when there is some.
//! - **What the summaries count beside the tests** (modules skipped while
//!   collecting, failed lifetimes that kept no test from running, error
//!   diagnostics by kind; see above) is one fold, `aye_stream::Tally`,
//!   which both summaries keep, and which the binary keeps beside a stored
//!   run (`aye_stream::Extra`, worded by [`summary_words`] for `aye show
//!   --runs`).
//! - **`aye show`** ([`RenderMode::Show`], [`ShowMode`]): the binary feeds
//!   a renderer ([`Text::with_mode`], [`JsonLines::with_mode`]) part of a
//!   stored log with the whole log's keys and what the run store knows
//!   beside it ([`ShowContext`]: the run's number, how long ago it
//!   finished, the files changed since), so a test shows the key it had
//!   in the run. [`ShowMode::List`] makes a line per result then the
//!   summary; [`ShowMode::Details`] shows the records as a run does,
//!   adding to a bad outcome the run, its worker's Python and whether its
//!   file changed. `stale` warns, before anything else, of a file that
//!   changed since the run.
//! - **GitHub annotations** ([`GithubAnnotations`]): one `::error` per failure
//!   signature (the first test or bad sub-outcome names it, the rest are
//!   counted), one per failed lifetime, error diagnostic and failed worker.
//!   A bad sub-outcome is annotated where it failed, as `<id> › <name>`,
//!   and its test's failures that repeat it aren't. Failures are written when
//!   the run finishes, capped at 50; paths can be prefixed for projects below
//!   the repository's root (the binary prefixes the root's path relative to
//!   `GITHUB_WORKSPACE`). A diagnostic about no file is annotated on none.
//!   The binary enables them in a GitHub Actions step (`GITHUB_ACTIONS`
//!   is `true` in the environment it read).

mod failure;
mod github;
mod json;
mod junit;
mod level;
mod rerun;
mod show;
mod text;
pub mod words;

pub use anstream::ColorChoice;
pub use github::GithubAnnotations;
pub use json::JsonLines;
pub use junit::{Junit, JunitDuration, JunitFamily, JunitLogging, JunitOptions};
pub use level::{DetailLevel, Ladder, Level};
pub use show::{RenderMode, ShowContext, ShowMode};
pub use text::{Text, TextOptions, summary_words};

#[cfg(test)]
mod tests;
