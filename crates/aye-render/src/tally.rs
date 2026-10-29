//! What a run's summary counts beside `run_finished`'s counts, which hold
//! test results only ([`Tally`], [`Extra`]), and how the summary line
//! words them all ([`summary_words`]).

use std::collections::{BTreeMap, HashSet};

use aye_events::{Counts, Diagnostic, Event, Record, Severity};

use crate::shared;

/// Folds what the summaries count beside `run_finished`'s test counts, as
/// [`Text`](crate::Text) and [`JsonLines`](crate::JsonLines) count it:
/// modules skipped while collecting (each once), failed lifetimes that
/// kept no test from running (each once, by the phase that failed), and
/// error diagnostics (each once, by kind). See the crate's docs for each
/// rule. The renderers fold their own; one on its own keeps a run's
/// summary without them (the run store, for `aye show --runs`).
#[derive(Debug, Default)]
pub struct Tally {
    module_skips: shared::ModuleSkips,
    lifetime_errors: shared::LifetimeErrors,
    errors: ErrorKinds,
}

/// What a record added to a [`Tally`] that a renderer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Noted<'a> {
    /// A module skipped while collecting, counted for the first time.
    ModuleSkip(&'a Diagnostic),
    /// An error diagnostic, counted for the first time.
    Error(&'a Diagnostic),
}

impl Tally {
    pub fn note(&mut self, record: &Record) {
        self.count(record);
    }

    /// Folds `record`, and says what it added that is shown once.
    pub(crate) fn count<'a>(&mut self, record: &'a Record) -> Option<Noted<'a>> {
        if let Some(skipped) = self.module_skips.note(record) {
            return Some(Noted::ModuleSkip(skipped));
        }
        self.lifetime_errors.note(record);
        match &record.event {
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    && self
                        .errors
                        .count(diagnostic, record.envelope.batch.is_some()) =>
            {
                Some(Noted::Error(diagnostic))
            }
            _ => None,
        }
    }

    /// What was counted so far.
    pub fn extra(&self) -> Extra {
        let lifetimes = self.lifetime_errors.counts();
        Extra {
            skipped_modules: self.module_skips.count(),
            setup_error: lifetimes.setup,
            teardown_error: lifetimes.teardown,
            lifetime_error: lifetimes.other,
            collection_error: self.errors.collection,
            usage_error: self.errors.usage,
            internal_error: self.errors.internal,
        }
    }
}

/// The diagnostic of a module skipped while collecting (pytest's
/// `skip(..., allow_module_level=True)`, `importorskip`), which the
/// summaries count as a skipped test: a `skipped` info from collection,
/// outside any batch. (A collect-only run counts none.)
pub fn module_skip(record: &Record) -> Option<&Diagnostic> {
    match &record.event {
        Event::Diagnostic(diagnostic)
            if diagnostic.code == "skipped"
                && diagnostic.severity == Severity::Info
                && record.envelope.batch.is_none() =>
        {
            Some(diagnostic)
        }
        _ => None,
    }
}

/// What a run's summary counts beside `run_finished`'s test counts
/// ([`Tally`]).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Extra {
    /// Modules skipped while collecting: the summaries add them to
    /// `skipped`.
    pub skipped_modules: u32,
    /// Failed lifetimes that kept no test from running, by the phase that
    /// failed (`lifetime_error`: none named).
    pub setup_error: u32,
    pub teardown_error: u32,
    pub lifetime_error: u32,
    /// Error diagnostics, by what the text summary calls them.
    pub collection_error: u32,
    pub usage_error: u32,
    pub internal_error: u32,
}

impl Extra {
    /// The fields by name, in order.
    fn fields(&self) -> [(&'static str, u32); 7] {
        [
            ("skipped_modules", self.skipped_modules),
            ("setup_error", self.setup_error),
            ("teardown_error", self.teardown_error),
            ("lifetime_error", self.lifetime_error),
            ("collection_error", self.collection_error),
            ("usage_error", self.usage_error),
            ("internal_error", self.internal_error),
        ]
    }

    /// The non-zero counts by their fields' names, to keep beside a run.
    pub fn named(&self) -> BTreeMap<String, u32> {
        self.fields()
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(name, n)| (name.to_owned(), n))
            .collect()
    }

    /// Read back from [`Extra::named`]; a name it doesn't know is left out.
    pub fn from_named(named: &BTreeMap<String, u32>) -> Self {
        let get = |name: &str| named.get(name).copied().unwrap_or(0);
        Self {
            skipped_modules: get("skipped_modules"),
            setup_error: get("setup_error"),
            teardown_error: get("teardown_error"),
            lifetime_error: get("lifetime_error"),
            collection_error: get("collection_error"),
            usage_error: get("usage_error"),
            internal_error: get("internal_error"),
        }
    }
}

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

/// Error diagnostics, by what the text summary line calls them, each once
/// however many batches repeat it (a framework rejecting its arguments).
#[derive(Debug, Default)]
pub(crate) struct ErrorKinds {
    pub collection: u32,
    pub usage: u32,
    pub internal: u32,
    /// Counted, by code, path and message.
    seen: HashSet<(String, String, String)>,
}

impl ErrorKinds {
    /// Counts an error diagnostic the first time it comes: whether it was.
    /// `from_batch`: whether a batch reported it, rather than the runner
    /// before anything ran.
    pub(crate) fn count(&mut self, diagnostic: &Diagnostic, from_batch: bool) -> bool {
        let key = (
            diagnostic.code.clone(),
            diagnostic.path.clone(),
            diagnostic.message.clone(),
        );
        if !self.seen.insert(key) {
            return false;
        }
        match diagnostic.code.as_str() {
            // A batch the framework stopped early: the summary's verdict and
            // the stop notice say so.
            "batch-stopped" => {}
            "usage-error" | "unclaimed" => self.usage += 1,
            // What the command line names doesn't exist: pytest's usage
            // error. A requested id the framework can't find in a batch is
            // a collection error.
            "not-found" if !from_batch => self.usage += 1,
            "internal-error" | "adapter-error" | "protocol-error" | "batch-failed"
            | "no-executor" => self.internal += 1,
            _ => self.collection += 1,
        }
        true
    }
}
