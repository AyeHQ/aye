//! How the text summary line words a run's counts, `run_finished`'s and
//! what `aye_stream::Tally` counts beside them ([`summary_words`]).

use aye_events::Counts;
use aye_stream::Extra;

use crate::shared;

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
        .map(|(n, one, many, _)| shared::plural(n, one, many))
        .collect()
}
