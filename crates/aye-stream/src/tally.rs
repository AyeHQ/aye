//! What a run's summary counts beside `run_finished`'s counts, which hold
//! test results only ([`Tally`], [`Extra`]): modules skipped while
//! collecting ([`ModuleSkips`]), failed lifetimes that kept no test from
//! running ([`LifetimeErrors`]), and error diagnostics by kind.

use std::collections::{BTreeMap, HashMap, HashSet};

use aye_events::{
    Diagnostic, DiagnosticCode, DiagnosticOrigin, Event, LifetimeId, Phase, Record, Severity,
    SummaryKind,
};

use crate::SeenDiagnostics;

/// Folds what the summaries count beside `run_finished`'s test counts, as
/// text and JSON lines count it: modules skipped while collecting (each
/// once), failed lifetimes that kept no test from running (each once, by
/// the phase that failed), and error diagnostics (each once, by kind). The
/// renderers fold their own; one on its own keeps a run's summary without
/// them (the run store, for `aye show --runs`).
#[derive(Debug, Default)]
pub struct Tally {
    module_skips: ModuleSkips,
    lifetime_errors: LifetimeErrors,
    errors: ErrorKinds,
}

/// What a record added to a [`Tally`] that a renderer shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noted<'a> {
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
    pub fn count<'a>(&mut self, record: &'a Record) -> Option<Noted<'a>> {
        if let Some(skipped) = self.module_skips.note(record) {
            return Some(Noted::ModuleSkip(skipped));
        }
        self.lifetime_errors.note(record);
        match &record.event {
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    && self
                        .errors
                        .count(diagnostic, DiagnosticOrigin::of(&record.envelope)) =>
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
            if diagnostic.code == DiagnosticCode::Skipped
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

/// Error diagnostics, by what the text summary line calls them
/// ([`aye_events::ErrorClass::summary`]), each once however many batches
/// repeat it (a framework rejecting its arguments).
#[derive(Debug, Default)]
struct ErrorKinds {
    collection: u32,
    usage: u32,
    internal: u32,
    /// Counted, each once.
    seen: SeenDiagnostics,
}

impl ErrorKinds {
    /// Counts an error diagnostic the first time it comes from `origin`:
    /// whether it was. A batch stopped early is seen, but not counted: the
    /// summary's verdict and the stop notice say so.
    fn count(&mut self, diagnostic: &Diagnostic, origin: DiagnosticOrigin) -> bool {
        if !self.seen.first(diagnostic) {
            return false;
        }
        match diagnostic.code.class(origin).summary() {
            Some(SummaryKind::Collection) => self.collection += 1,
            Some(SummaryKind::Usage) => self.usage += 1,
            Some(SummaryKind::Internal) => self.internal += 1,
            None => {}
        }
        true
    }
}

/// Modules the framework skipped while collecting them (pytest's
/// `skip(..., allow_module_level=True)` and `importorskip`): collection
/// reports each as a `skipped` info diagnostic, outside any batch, and no
/// test of the module reaches the run. pytest counts each as one skipped
/// item in its summary and its JUnit report, so the renderers do too;
/// `run_finished`'s counts hold test results only. A collect-only run counts
/// none, as `pytest --co` doesn't.
#[derive(Debug, Default)]
pub struct ModuleSkips {
    paths: HashSet<String>,
    collect_only: bool,
}

impl ModuleSkips {
    /// The diagnostic, when `record` reports a module skipped and that
    /// module wasn't counted yet.
    pub fn note<'a>(&mut self, record: &'a Record) -> Option<&'a Diagnostic> {
        if let Event::RunStarted { collect_only, .. } = &record.event {
            self.collect_only = *collect_only;
            return None;
        }
        let diagnostic = module_skip(record).filter(|_| !self.collect_only)?;
        self.paths
            .insert(diagnostic.path.to_string())
            .then_some(diagnostic)
    }

    pub fn count(&self) -> u32 {
        u32::try_from(self.paths.len()).unwrap_or(u32::MAX)
    }
}

/// Failed lifetimes that kept no test from running: a failed
/// `tearDownClass` or `tearDownModule`, a class or module cleanup that
/// raised, or a failed setup whose tests the run stopped before (under
/// `maxfail` the engine reports a failed setup before the tests it blocks,
/// and the runner keeps what closes a lifetime but drops the tests after
/// the bad outcome that reaches `maxfail`). The run fails on them, but
/// `run_finished`'s counts hold test results only, so the summaries count
/// them from the stream, by the phase that failed: pytest's summary counts
/// each as an error (`1 passed, 1 error`), unittest's too (`FAILED
/// (errors=1)`). Each lifetime counts once, however often it failed (a
/// class split across batches tears down in each), as it is one test case
/// in the JUnit report; one that some test names as its `cause` counts
/// through that test instead. Only `lifetime_finished` and `test_finished`
/// matter: a `lifetime_started` without its `lifetime_finished` (a worker
/// that crashed or was stopped) counts nothing.
#[derive(Debug, Default)]
struct LifetimeErrors {
    /// The phase each failed lifetime first failed in.
    failed: HashMap<LifetimeId, Option<Phase>>,
    /// Lifetimes some test's result names as its `cause`.
    blocking: HashSet<LifetimeId>,
}

/// [`LifetimeErrors`], by the phase that failed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct LifetimeCounts {
    setup: u32,
    teardown: u32,
    /// A failure that names no phase (or the test's own, which a lifetime
    /// has none of): no adapter reports one today.
    other: u32,
}

impl LifetimeErrors {
    fn note(&mut self, record: &Record) {
        match &record.event {
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                ..
            } => {
                self.failed.entry(lifetime.clone()).or_insert(failure.phase);
            }
            Event::TestFinished(result) => {
                if let Some(cause) = &result.cause {
                    self.blocking.insert(cause.clone());
                }
            }
            _ => {}
        }
    }

    /// The failed lifetimes that kept no test from running, so far.
    fn counts(&self) -> LifetimeCounts {
        let mut counts = LifetimeCounts::default();
        #[expect(clippy::iter_over_hash_type, reason = "counts, in any order")]
        for (lifetime, phase) in &self.failed {
            if self.blocking.contains(lifetime) {
                continue;
            }
            let slot = match phase {
                Some(Phase::Setup) => &mut counts.setup,
                Some(Phase::Teardown) => &mut counts.teardown,
                Some(Phase::Test) | None => &mut counts.other,
            };
            *slot = slot.saturating_add(1);
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use aye_events::{BatchIndex, DiagnosticPath, Failure, Outcome, Scope, TestId, TestResult};

    use super::*;

    /// Every diagnostic code aye 0.0.1 knows, and one it doesn't, with what
    /// the summary counts an error with it as, before any batch and from
    /// one: a `collection`, `usage` or `internal` error, or `none`.
    const SUMMARY: [(&str, &str, &str); 26] = [
        ("syntax-error", "collection", "collection"),
        ("unreadable", "collection", "collection"),
        ("not-found", "usage", "collection"),
        ("unclaimed", "usage", "usage"),
        ("duplicate-id", "collection", "collection"),
        ("no-tests", "collection", "collection"),
        ("not-refined", "collection", "collection"),
        ("refine-failed", "collection", "collection"),
        ("class-has-init", "collection", "collection"),
        ("duplicate-definition", "collection", "collection"),
        ("collection-error", "collection", "collection"),
        ("usage-error", "usage", "usage"),
        ("unknown-option", "collection", "collection"),
        ("plugin-not-loaded", "collection", "collection"),
        ("internal-error", "internal", "internal"),
        ("skipped", "collection", "collection"),
        ("ignored", "collection", "collection"),
        ("batch-stopped", "none", "none"),
        ("session-failed", "collection", "collection"),
        ("protocol-error", "internal", "internal"),
        ("batch-failed", "internal", "internal"),
        ("no-executor", "internal", "internal"),
        ("listing-differs", "collection", "collection"),
        ("stand-in", "collection", "collection"),
        ("adapter-error", "internal", "internal"),
        ("some-other-code", "collection", "collection"),
    ];

    /// What the summary counts an error with `code` from `origin` as.
    fn counted(code: &str, origin: DiagnosticOrigin) -> &'static str {
        let mut kinds = ErrorKinds::default();
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            path: DiagnosticPath::Nowhere,
            span: None,
            message: String::new(),
        };
        assert!(kinds.count(&diagnostic, origin), "{code}");
        match (kinds.collection, kinds.usage, kinds.internal) {
            (1, 0, 0) => "collection",
            (0, 1, 0) => "usage",
            (0, 0, 1) => "internal",
            (0, 0, 0) => "none",
            counts => panic!("{code}: {counts:?}"),
        }
    }

    #[test]
    fn every_code_is_counted_as_before() {
        let known: Vec<&str> = SUMMARY[..25].iter().map(|(code, ..)| *code).collect();
        let all: Vec<&str> = DiagnosticCode::ALL
            .iter()
            .map(DiagnosticCode::as_str)
            .collect();
        assert_eq!(known, all);
        for (code, before, from_batch) in SUMMARY {
            assert_eq!(
                (
                    counted(code, DiagnosticOrigin::BeforeBatches),
                    counted(code, DiagnosticOrigin::Batch(BatchIndex::from_u32(0)))
                ),
                (before, from_batch),
                "{code}"
            );
        }
    }

    #[test]
    fn failed_lifetimes_count_once_and_only_when_they_blocked_no_test() {
        let lifetime = |id: &str, phase: Option<Phase>| {
            Record::new(Event::LifetimeFinished {
                lifetime: id.into(),
                failure: Some(Failure {
                    phase,
                    message: "boom".into(),
                    ..Failure::default()
                }),
                duration: Duration::ZERO,
            })
        };
        let blocked = |cause: &str| {
            let mut result = TestResult::new(TestId::new("t.py::T::test_x"), Outcome::Error);
            result.cause = Some(cause.into());
            Record::new(Event::TestFinished(Box::new(result)))
        };
        let mut errors = LifetimeErrors::default();
        for record in [
            // A class split across batches tears down in each: one count.
            lifetime("t.py::A", Some(Phase::Teardown)),
            lifetime("t.py::A", Some(Phase::Teardown)),
            // Blocking tests before its failure arrives (the engine's
            // order) or after it (under `maxfail`): no count of its own.
            blocked("t.py::B"),
            lifetime("t.py::B", Some(Phase::Setup)),
            lifetime("t.py::C", Some(Phase::Setup)),
            blocked("t.py::C"),
            // A setup whose tests the run dropped.
            lifetime("t.py::D", Some(Phase::Setup)),
            lifetime("t.py", None),
            // Started, never finished (a crash): nothing.
            Record::new(Event::LifetimeStarted {
                lifetime: "t.py::E".into(),
                kind: Scope::Class,
            }),
            Record::new(Event::LifetimeFinished {
                lifetime: "t.py::F".into(),
                failure: None,
                duration: Duration::ZERO,
            }),
        ] {
            errors.note(&record);
        }
        assert_eq!(
            errors.counts(),
            LifetimeCounts {
                setup: 1,
                teardown: 1,
                other: 1,
            }
        );
    }
}
