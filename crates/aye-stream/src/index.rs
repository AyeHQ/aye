//! A stored run's log, indexed ([`RunIndex`]): its results, as `aye show`
//! lists and selects them ([`RunItem`]), each with the places of its
//! records in the log, and the collected cases.
//!
//! A result is a test's `test_finished`, or what the run's summary counts
//! beside the tests (a failed lifetime, once however often it failed; an
//! error diagnostic, each once, as the renderers show them; a module
//! skipped while collecting, once) or shows whatever the level (a failed
//! worker).

use std::collections::{HashMap, HashSet};

use aye_events::{
    AdapterName, Case, Certainty, Diagnostic, Event, LifetimeId, Outcome, Record, Severity, TestId,
    TestResult, WorkerIndex,
};
use aye_index::{IndexSlice, IndexVec, newtype_index};

use crate::tally::module_skip;

newtype_index! {
    /// A record's place in a stored log.
    pub struct RecordIndex;
}

newtype_index! {
    /// A result's place among a run's results ([`RunIndex::items`]).
    pub struct ItemIndex;
}

/// A result of the run, as `aye show` lists and selects them.
#[derive(Debug)]
pub struct RunItem<'a> {
    /// Its record's place in the log.
    pub at: RecordIndex,
    pub kind: ItemKind<'a>,
    /// What selectors match: a test's id, a lifetime's, a diagnostic's
    /// path; empty for a failed worker.
    pub name: TestId,
    /// `error` for a failed lifetime, an error diagnostic or a failed
    /// worker; `skipped` for a module skipped while collecting.
    pub outcome: Outcome,
    /// A failed lifetime some test names as its `cause`.
    pub blocking: bool,
}

#[derive(Debug)]
pub enum ItemKind<'a> {
    /// A test's result, with the places of its `test_started` and
    /// `sub_outcome` records.
    Test(&'a TestResult, Vec<RecordIndex>),
    /// A failed lifetime: the first of its failures (a class split across
    /// batches tears down in each).
    Lifetime(&'a LifetimeId),
    /// An error diagnostic (each once), or a module skipped while
    /// collecting.
    Diagnostic,
    /// A failed worker.
    Worker,
}

/// A test's records before its `test_finished`: the worker that runs it,
/// and their places in the log.
type Open = (Option<WorkerIndex>, Vec<RecordIndex>);

/// A stored run's log, indexed; see the module's docs.
#[derive(Debug)]
pub struct RunIndex<'a> {
    records: IndexSlice<'a, RecordIndex, Record>,
    /// The run's first `run_started`.
    pub run_started: Option<&'a Record>,
    /// The run's last `run_finished`.
    pub run_finished: Option<&'a Record>,
    /// The run's results, in the order of the log.
    pub items: IndexVec<ItemIndex, RunItem<'a>>,
    /// Each failed lifetime's every failure, by place in the log.
    lifetimes: HashMap<&'a LifetimeId, Vec<RecordIndex>>,
    /// The collected cases, by id.
    cases: HashMap<&'a TestId, &'a Case>,
}

impl<'a> RunIndex<'a> {
    pub fn of(records: &'a [Record]) -> Self {
        let records = IndexSlice::from_raw(records);
        let mut index = RunIndex {
            records,
            run_started: None,
            run_finished: None,
            items: IndexVec::new(),
            lifetimes: HashMap::new(),
            cases: HashMap::new(),
        };
        // A test's `test_started` and `sub_outcome` records not yet
        // finished, by id and worker.
        let mut open: HashMap<&TestId, Vec<Open>> = HashMap::new();
        let mut causes: HashSet<&LifetimeId> = HashSet::new();
        let mut diagnostics: HashSet<(&str, &str, &str)> = HashSet::new();
        for (at, record) in records.iter_enumerated() {
            let worker = record.envelope.worker;
            match &record.event {
                Event::RunStarted { .. } if index.run_started.is_none() => {
                    index.run_started = Some(record);
                }
                Event::RunFinished { .. } => index.run_finished = Some(record),
                Event::CaseCollected(case) => {
                    index.cases.insert(&case.id, case);
                }
                Event::TestStarted { id } | Event::SubOutcome { id, .. } => {
                    let tests = open.entry(id).or_default();
                    match tests.iter_mut().find(|(w, _)| *w == worker) {
                        Some((_, parts)) => parts.push(at),
                        None => tests.push((worker, vec![at])),
                    }
                }
                Event::TestFinished(result) => {
                    // Its own worker's, else (a test the runner failed for
                    // a worker that crashed) the first open.
                    let parts = open.get_mut(&result.id).map_or_else(Vec::new, |tests| {
                        let found = tests.iter().position(|(w, _)| *w == worker);
                        match found.or_else(|| (!tests.is_empty()).then_some(0)) {
                            Some(at) => tests.remove(at).1,
                            None => Vec::new(),
                        }
                    });
                    if let Some(cause) = &result.cause {
                        causes.insert(cause);
                    }
                    index.items.push(RunItem {
                        at,
                        kind: ItemKind::Test(result, parts),
                        name: result.id.clone(),
                        outcome: result.outcome,
                        blocking: false,
                    });
                }
                Event::LifetimeFinished {
                    lifetime,
                    failure: Some(_),
                    ..
                } => {
                    let failures = index.lifetimes.entry(lifetime).or_default();
                    failures.push(at);
                    if failures.len() == 1 {
                        index.items.push(RunItem {
                            at,
                            kind: ItemKind::Lifetime(lifetime),
                            name: TestId::new(lifetime.as_str()),
                            outcome: Outcome::Error,
                            blocking: false,
                        });
                    }
                }
                Event::Diagnostic(diagnostic) => {
                    let outcome = if module_skip(record).is_some() {
                        Outcome::Skipped
                    } else if diagnostic.severity == Severity::Error {
                        Outcome::Error
                    } else {
                        continue;
                    };
                    let Diagnostic {
                        code,
                        path,
                        message,
                        ..
                    } = diagnostic;
                    // Each once, as the renderers show them; a module once.
                    let key = match outcome {
                        Outcome::Skipped => ("", path.as_str(), ""),
                        _ => (code.as_str(), path.as_str(), message.as_str()),
                    };
                    if diagnostics.insert(key) {
                        index.items.push(RunItem {
                            at,
                            kind: ItemKind::Diagnostic,
                            name: TestId::new(path.as_str()),
                            outcome,
                            blocking: false,
                        });
                    }
                }
                Event::WorkerFinished { error: Some(_), .. } => {
                    index.items.push(RunItem {
                        at,
                        kind: ItemKind::Worker,
                        name: TestId::new(""),
                        outcome: Outcome::Error,
                        blocking: false,
                    });
                }
                _ => {}
            }
        }
        for item in &mut index.items {
            if let ItemKind::Lifetime(lifetime) = item.kind {
                item.blocking = causes.contains(lifetime);
            }
        }
        index
    }

    /// The log.
    pub const fn records(&self) -> IndexSlice<'a, RecordIndex, Record> {
        self.records
    }

    /// The record at `at`.
    pub fn record(&self, at: RecordIndex) -> &'a Record {
        &self.records.as_raw()[at.as_usize()]
    }

    /// The places of every failure of `lifetime`; none when it never
    /// failed.
    pub fn failures(&self, lifetime: &LifetimeId) -> &[RecordIndex] {
        self.lifetimes.get(lifetime).map_or(&[], Vec::as_slice)
    }

    /// The names selectors match, each once, in the order they come.
    pub fn names(&self) -> Vec<TestId> {
        let mut seen = HashSet::new();
        self.items
            .iter()
            .filter(|item| !item.name.as_str().is_empty() && seen.insert(&item.name))
            .map(|item| item.name.clone())
            .collect()
    }

    /// The case of a name, as collection listed it: its own, or that of
    /// its id without parameters (a case collected without them finishes
    /// as one result per parameter), with its id; a bare one when
    /// collection listed neither (a lifetime, a file).
    pub fn case(&self, id: &TestId) -> Case {
        let found = self
            .cases
            .get(id)
            .copied()
            .or_else(|| self.cases.get(&TestId::new(id.without_params())).copied());
        let mut case = found
            .cloned()
            .unwrap_or_else(|| Case::new(id.as_str(), AdapterName::default()));
        case.id = id.clone();
        if case.certainty == Certainty::Partial {
            case.certainty = Certainty::Refined;
        }
        case
    }
}
