//! `.aye/history.json`: what history knows of each test ([`TestStats`],
//! `stats.rs`), the file it keeps (`file.rs`), and the fold of a run's
//! records into it ([`History::record`]), which `--lf`, `--ff`, the
//! scheduler's durations and each bad outcome's `history` read.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **One file**, `<dir>/history.json`, carrying its own `version`. A file
//!   that is missing, unreadable, malformed or of another version loads as
//!   empty history, never as an error: history is advisory and must not stop
//!   a run. Saving writes a temporary file beside it and renames it over, so
//!   a reader sees the old file or the new one, never part of either. Two
//!   runs saving at once race, and the last one wins. There is no fsync: a
//!   file damaged by a power loss reads as empty.
//! - **Keyed by test id alone** for now. The environment joins the key when
//!   the matrix lands, under a new `version`.
//! - **Durations** are a moving average (alpha 0.3, the first run taken as
//!   is), the maximum, the count and the last, over the `test_finished` of
//!   tests that ran. A test that failed without running, because a lifetime
//!   failed (`cause` set) or without a `test_started` in its run (the
//!   runner fails the tests a crashed worker never started), updates its
//!   outcome but not its durations.
//! - **Last failed follows pytest's `--lf`**: a test that finishes `failed`
//!   or `error` joins the set, a test that finishes any other way leaves it,
//!   and a test that did not run stays as it was, outcome and durations
//!   included. A test did not run when it has no `test_finished` (the run
//!   was stopped first), or when it finished neither `failed` nor `error`
//!   without a `test_started` in its run. Every skip leaves the set,
//!   whatever its reason, as pytest's does. (No adapter reports a test it
//!   never ran as skipped: pytest's `--setup-plan` and `--setup-only`,
//!   which run no test, are a usage error.) Records outside a run (no
//!   `run_started`) count as run. Collect-only runs record nothing at all.
//!   Collection errors are not recorded yet (pytest records the file that
//!   failed to collect).
//! - **Every adapter's tests alike.** History knows tests by id, whatever
//!   adapter ran them (pytest, unittest, doctest), so `--lf` and `--ff`
//!   work across adapters. Sub-outcomes (subtests, doctest examples) and
//!   lifetimes are not recorded: a test with a failed subtest or example
//!   fails as a whole, and a failed `setUpClass` or `setUpModule` joins the
//!   last-failed set through the tests it blocked (`cause` set), which
//!   record their outcome but no duration. Only `test_started` and
//!   `test_finished` are read, so lifetimes a crashed or stopped worker
//!   left without a `lifetime_finished` change nothing.
//! - **`flips` is reserved** for flake rates and stays 0 until they land.
//! - **Recent outcomes** (October 2026): each test keeps `recent`, a letter
//!   per run that recorded it, oldest first, the last 20 (`p` passed, `f`
//!   failed, `e` error, `s` skipped, `x` expected failure, `u` unexpected
//!   pass: an [`OutcomeTrail`]), and `bad_since`, the number of the run
//!   (`r81`, from its `run_started`) in which its current streak of bad
//!   outcomes began: kept while it stays bad, the run's own number when it
//!   turns bad (none when the run has none: it isn't stored), cleared when
//!   it isn't bad. Both are optional in the file, so its `version` stays 1
//!   and an older file keeps its durations. A test that finishes twice in
//!   a run has one letter for it, the last result's. [`History::previous`]
//!   gives them as they were before the run being recorded, which the
//!   binary puts in each bad outcome's `history` before anything else sees
//!   it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::time::Duration;

use aye_events::{Event, OutcomeTrail, Previous, Record, RunId, RunNumber, TestId, TestResult};

mod file;
mod stats;

pub use file::{FILE, VERSION};
pub use stats::TestStats;
use stats::duration;

/// Per-test statistics and last-failed, folded from run logs.
#[derive(Debug, Clone, Default)]
pub struct History {
    tests: BTreeMap<TestId, TestStats>,
    last_failed: BTreeSet<TestId>,
    run: Run,
    changed: bool,
}

/// The run being recorded.
#[derive(Debug, Clone, Default)]
struct Run {
    id: Option<RunId>,
    /// Its number in the run store, from its `run_started`.
    number: Option<RunNumber>,
    collect_only: bool,
    /// Whether a `run_started` opened it: only then does a `test_finished`
    /// without a `test_started` mean the test never started.
    opened: bool,
    /// The tests that started in it.
    started: HashSet<TestId>,
    /// What history knew of each test it recorded before it.
    before: HashMap<TestId, Before>,
}

/// What history knew of a test's outcomes before the run being recorded.
#[derive(Debug, Clone, Default)]
struct Before {
    recent: OutcomeTrail,
    bad_since: Option<RunNumber>,
    /// Whether its last outcome was bad.
    bad: bool,
}

impl Before {
    fn of(stats: Option<&TestStats>) -> Self {
        stats.map_or_else(Self::default, |stats| Self {
            recent: stats.recent.clone(),
            bad_since: stats.bad_since,
            bad: stats.last_outcome.is_bad(),
        })
    }

    /// What it says of the test, when a run recorded its outcome.
    fn previous(&self) -> Option<Previous> {
        (!self.recent.is_empty()).then(|| Previous {
            outcomes: self.recent.clone(),
            since: self.bad_since.filter(|_| self.bad),
        })
    }
}

impl History {
    /// Folds one record of a run's log into the history. Kinds history has
    /// no use for are ignored, and so is everything in a collect-only run.
    /// A run's number, which `bad_since` takes, is its `run_started`'s.
    pub fn record(&mut self, record: &Record) {
        match &record.event {
            Event::RunStarted {
                collect_only, run, ..
            } => {
                self.run = Run {
                    id: record.envelope.run.clone(),
                    number: *run,
                    collect_only: *collect_only,
                    opened: true,
                    ..Run::default()
                };
            }
            Event::RunFinished { .. } => self.run = Run::default(),
            Event::TestStarted { id } if self.run.opened => {
                self.run.started.insert(id.clone());
            }
            Event::TestFinished(result) if !self.run.collect_only => {
                let run = record
                    .envelope
                    .run
                    .as_ref()
                    .or(self.run.id.as_ref())
                    .cloned();
                self.finish(result, run);
            }
            _ => {}
        }
    }

    fn finish(&mut self, result: &TestResult, run: Option<RunId>) {
        let started = !self.run.opened || self.run.started.contains(&result.id);
        // A bad outcome counts whether the test ran or not: the run failed
        // on it, and `--lf` should run it again.
        if !started && !result.outcome.is_bad() {
            return;
        }
        // What it was before this run, kept for its next result in this
        // run (a duplicate id), which replaces this one.
        let before = self
            .run
            .before
            .entry(result.id.clone())
            .or_insert_with(|| Before::of(self.tests.get(&result.id)));
        let stats = self
            .tests
            .entry(result.id.clone())
            .or_insert_with(|| TestStats::new(result.outcome));
        stats.last_outcome = result.outcome;
        stats.last_run = run;
        stats.recent.clone_from(&before.recent);
        stats.recent.push(result.outcome);
        stats.bad_since = match (result.outcome.is_bad(), before.bad) {
            (false, _) => None,
            (true, true) => before.bad_since,
            (true, false) => self.run.number,
        };
        if result.cause.is_none() && started {
            stats.add(result.duration.as_secs_f64() * 1000.0);
        }
        if result.outcome.is_bad() {
            self.last_failed.insert(result.id.clone());
        } else {
            self.last_failed.remove(&result.id);
        }
        self.changed = true;
    }

    /// Whether any test was recorded since the history was loaded. A run
    /// that recorded nothing (collect-only, nothing selected) need not save.
    pub fn changed(&self) -> bool {
        self.changed
    }

    /// No test is known.
    pub fn is_empty(&self) -> bool {
        self.tests.is_empty() && self.last_failed.is_empty()
    }

    /// The tests whose last result was `failed` or `error`.
    pub fn last_failed(&self) -> HashSet<TestId> {
        self.last_failed.iter().cloned().collect()
    }

    /// Whether `id` was `failed` or `error` when it last ran.
    pub fn is_last_failed(&self, id: &TestId) -> bool {
        self.last_failed.contains(id)
    }

    /// The moving-average duration of every test that has one.
    pub fn durations(&self) -> HashMap<TestId, Duration> {
        self.tests
            .iter()
            .filter_map(|(id, stats)| Some((id.clone(), stats.ewma()?)))
            .collect()
    }

    /// The moving-average duration of `id`.
    pub fn duration(&self, id: &TestId) -> Option<Duration> {
        self.tests.get(id)?.ewma()
    }

    /// What `id` is expected to take: its own moving average or, for an id
    /// without parameters (`t.py::test_x`, as collection predicts a case it
    /// can only partly see), the sum over its recorded parameter ids
    /// (`t.py::test_x[…]`).
    pub fn estimate(&self, id: &TestId) -> Option<Duration> {
        if let Some(own) = self.duration(id) {
            return Some(own);
        }
        let name = id.as_str();
        if id.is_file() || id.without_params() != name {
            return None;
        }
        // Ids that start with `name[` sort between `name[` and `name\`.
        let from = TestId::new(format!("{name}["));
        let to = TestId::new(format!("{name}\\"));
        let mut found = false;
        let mut total = 0.0;
        for (_, stats) in self
            .tests
            .range(from..to)
            .filter(|(param, stats)| param.without_params() == name && stats.count > 0)
        {
            found = true;
            total += stats.ewma_ms;
        }
        found.then(|| duration(total))
    }

    /// Moving-average durations summed per group, `group` naming the group
    /// of each test: `durations_by_group(|id| id.path().to_project_path())` weighs
    /// files.
    pub fn durations_by_group<K, F>(&self, mut group: F) -> HashMap<K, Duration>
    where
        K: Eq + Hash,
        F: FnMut(&TestId) -> K,
    {
        let mut totals: HashMap<K, f64> = HashMap::new();
        for (id, stats) in self.tests.iter().filter(|(_, stats)| stats.count > 0) {
            *totals.entry(group(id)).or_default() += stats.ewma_ms;
        }
        totals
            .into_iter()
            .map(|(key, ms)| (key, duration(ms)))
            .collect()
    }

    /// Everything known about `id`.
    pub fn stats(&self, id: &TestId) -> Option<&TestStats> {
        self.tests.get(id)
    }

    /// How `id` ended in the runs before the one being recorded, for a bad
    /// outcome's `history`: its `recent` outcomes, and, when it was bad
    /// already, the run its streak of bad outcomes began in. `None` when
    /// no run recorded it.
    pub fn previous(&self, id: &TestId) -> Option<Previous> {
        match self.run.before.get(id) {
            Some(before) => before.previous(),
            None => Before::of(self.tests.get(id)).previous(),
        }
    }
}

#[cfg(test)]
mod tests;
