//! What `aye show` reads of a stored log ([`Log`]: the run's results, as
//! [`Item`]s) and the records it feeds the renderers for those shown
//! ([`feed`]), with what it tells them of the run beside the log
//! ([`context`], [`stale`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aye_events::{
    AdapterName, Case, Certainty, Counts, Diagnostic, Event, ExitCode, LifetimeId, Outcome,
    ProjectPath, Record, Severity, TestId, TestResult, WorkerIndex,
};
use aye_history::{Change, RunMeta};
use aye_render::{ShowContext, ShowMode};

/// A result of the run, as `aye show` lists and selects them.
pub(super) struct Item<'a> {
    /// Its record's place in the log.
    pub(super) at: usize,
    pub(super) what: What<'a>,
    /// What selectors match: a test's id, a lifetime's, a diagnostic's
    /// path; empty for a failed worker.
    pub(super) name: TestId,
    /// `error` for a failed lifetime, an error diagnostic or a failed
    /// worker; `skipped` for a module skipped while collecting.
    pub(super) outcome: Outcome,
    /// A failed lifetime some test names as its `cause`.
    pub(super) blocking: bool,
}

pub(super) enum What<'a> {
    /// A test's result, with the places of its `test_started` and
    /// `sub_outcome` records.
    Test(&'a TestResult, Vec<usize>),
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
pub(super) type Open = (Option<WorkerIndex>, Vec<usize>);

/// What `aye show` reads of a stored log.
pub(super) struct Log<'a> {
    pub(super) run_started: Option<&'a Record>,
    pub(super) run_finished: Option<&'a Record>,
    /// The run's results, in the order of the log.
    pub(super) items: Vec<Item<'a>>,
    /// Each failed lifetime's every failure, by place in the log.
    pub(super) lifetimes: HashMap<&'a LifetimeId, Vec<usize>>,
    /// The collected cases, by id.
    pub(super) cases: HashMap<&'a TestId, &'a Case>,
}

impl<'a> Log<'a> {
    pub(super) fn of(records: &'a [Record]) -> Self {
        let mut log = Log {
            run_started: None,
            run_finished: None,
            items: Vec::new(),
            lifetimes: HashMap::new(),
            cases: HashMap::new(),
        };
        // A test's `test_started` and `sub_outcome` records not yet
        // finished, by id and worker.
        let mut open: HashMap<&TestId, Vec<Open>> = HashMap::new();
        let mut causes: HashSet<&LifetimeId> = HashSet::new();
        let mut diagnostics: HashSet<(&str, &str, &str)> = HashSet::new();
        for (at, record) in records.iter().enumerate() {
            let worker = record.envelope.worker;
            match &record.event {
                Event::RunStarted { .. } if log.run_started.is_none() => {
                    log.run_started = Some(record);
                }
                Event::RunFinished { .. } => log.run_finished = Some(record),
                Event::CaseCollected(case) => {
                    log.cases.insert(&case.id, case);
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
                    log.items.push(Item {
                        at,
                        what: What::Test(result, parts),
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
                    let failures = log.lifetimes.entry(lifetime).or_default();
                    failures.push(at);
                    if failures.len() == 1 {
                        log.items.push(Item {
                            at,
                            what: What::Lifetime(lifetime),
                            name: TestId::new(lifetime.as_str()),
                            outcome: Outcome::Error,
                            blocking: false,
                        });
                    }
                }
                Event::Diagnostic(diagnostic) => {
                    let outcome = if aye_render::module_skip(record).is_some() {
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
                        log.items.push(Item {
                            at,
                            what: What::Diagnostic,
                            name: TestId::new(path.as_str()),
                            outcome,
                            blocking: false,
                        });
                    }
                }
                Event::WorkerFinished { error: Some(_), .. } => log.items.push(Item {
                    at,
                    what: What::Worker,
                    name: TestId::new(""),
                    outcome: Outcome::Error,
                    blocking: false,
                }),
                _ => {}
            }
        }
        for item in &mut log.items {
            if let What::Lifetime(lifetime) = item.what {
                item.blocking = causes.contains(lifetime);
            }
        }
        log
    }

    /// The names selectors match, each once, in the order they come.
    pub(super) fn names(&self) -> Vec<TestId> {
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
    pub(super) fn case(&self, id: &TestId) -> Case {
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

/// The records the renderer gets: the run's `run_started`, what `mode`
/// shows of the results `shown` (in details, with the `worker_started` of
/// the workers that ran them), and a `run_finished`: the run's own when
/// the list is the whole run (nothing `filtered`), else one counting the
/// tests shown, with the run's exit code (`exit_code`, the stored one when
/// the log has no `run_finished`).
pub(super) fn feed(
    records: &[Record],
    log: &Log<'_>,
    shown: &[&Item<'_>],
    mode: ShowMode,
    filtered: bool,
    exit_code: Option<ExitCode>,
) -> Vec<Record> {
    let mut places = BTreeSet::new();
    for item in shown {
        places.insert(item.at);
        if mode == ShowMode::List {
            continue;
        }
        match &item.what {
            What::Test(result, parts) => {
                places.extend(parts);
                if let Some(cause) = &result.cause {
                    places.extend(log.lifetimes.get(cause).into_iter().flatten());
                }
            }
            What::Lifetime(lifetime) => {
                places.extend(log.lifetimes.get(lifetime).into_iter().flatten());
            }
            What::Diagnostic | What::Worker => {}
        }
    }
    if mode == ShowMode::Details {
        // The `worker_started` of each worker that ran a result shown: it
        // says which Python ran it.
        let workers: HashSet<WorkerIndex> = places
            .iter()
            .filter_map(|at| records[*at].envelope.worker)
            .collect();
        places.extend(records.iter().enumerate().filter_map(|(at, record)| {
            (matches!(record.event, Event::WorkerStarted { .. })
                && record
                    .envelope
                    .worker
                    .is_some_and(|worker| workers.contains(&worker)))
            .then_some(at)
        }));
    }
    let mut feed: Vec<Record> = log.run_started.into_iter().cloned().collect();
    feed.extend(places.into_iter().map(|at| records[at].clone()));
    let whole = mode == ShowMode::List && !filtered;
    match log.run_finished {
        Some(finished) if whole => feed.push(finished.clone()),
        _ => {
            let mut counts = Counts::default();
            let mut duration = Duration::ZERO;
            for item in shown {
                if let What::Test(result, _) = &item.what {
                    counts.record(result.outcome);
                    duration = duration.saturating_add(result.duration);
                }
            }
            let exit_code = log
                .run_finished
                .and_then(|finished| match &finished.event {
                    Event::RunFinished { exit_code, .. } => Some(*exit_code),
                    _ => None,
                })
                .or(exit_code)
                .unwrap_or(if counts.bad() > 0 {
                    ExitCode::TestsFailed
                } else {
                    ExitCode::Ok
                });
            let mut record = Record::new(Event::RunFinished {
                exit_code,
                counts,
                duration,
                stopped: None,
            });
            record.envelope.run = log
                .run_started
                .and_then(|started| started.envelope.run.clone());
            feed.push(record);
        }
    }
    feed
}

/// The files of the results shown that changed since the run, each once,
/// in the order their results come.
pub(super) fn stale(
    meta: &RunMeta,
    root: &Path,
    shown: &[&Item<'_>],
) -> Vec<(ProjectPath, Change)> {
    let mut seen = HashSet::new();
    shown
        .iter()
        .map(|item| item.name.path())
        .filter(|path| !path.as_str().is_empty() && seen.insert(*path))
        .filter_map(|path| Some((path.to_project_path(), meta.change(root, path.as_str())?)))
        .collect()
}

/// What the renderers are told of the run `meta` beside its log, at
/// `now_ms`: its number, how long ago it finished, and the files of the
/// results shown that changed since (`stale`), each with whether it is
/// gone.
pub(super) fn context(meta: &RunMeta, stale: &[(ProjectPath, Change)], now_ms: u64) -> ShowContext {
    ShowContext {
        run: meta.number(),
        ago: (meta.finished_ms > 0).then(|| ago(now_ms, meta.finished_ms)),
        stale: stale
            .iter()
            .map(|(path, change)| (path.clone(), *change == Change::Gone))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// How long before `now_ms` the time `then_ms` was (both in milliseconds
/// since the Unix epoch): `just now` (under a minute, or later than now),
/// `2 min ago`, `3 h ago`, `1 day ago`, `2 days ago`.
pub(super) fn ago(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        ..60 => "just now".to_owned(),
        60..3_600 => format!("{} min ago", secs / 60),
        3_600..86_400 => format!("{} h ago", secs / 3_600),
        86_400..172_800 => "1 day ago".to_owned(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
