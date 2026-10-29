//! History as another consumer of the stream. Per-test duration statistics
//! (ewma, max, count, last run), outcomes and last-failed, flake rates, the
//! per-test coverage map, run metadata, and the queries built on them: run
//! diffs, tests-for, parallel floor. Same schema locally and in the cloud;
//! durations are advisory and separate from result caching, which has stricter
//! invalidation.
//!
//! Depends on: aye-events, and aye-fs for the state directory.
//!
//! Native features (October 2026): the fingerprint that decides whether a
//! cached result holds carries its own version, so changing its inputs
//! invalidates caches instead of misreading them. Its inputs grow to include
//! the snapshot values a test compares against and the test's hermetic
//! verdict; a test that was not hermetic is never served from cache.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **One file**, `<dir>/history.json`, carrying its own `version`. A file
//!   that is missing, unreadable, malformed or of another version loads as
//!   empty history, never as an error: history is advisory and must not stop
//!   a run. Saving writes a temporary file beside it and renames it over, so
//!   a reader sees the old file or the new one, never part of either. Two
//!   runs saving at once race, and the last one wins. There is no fsync: a
//!   file damaged by a power loss reads as empty.
//! - **The directory is marked as a cache** when aye creates it, whichever
//!   writer comes first: history and the run store create theirs, as the
//!   refinement cache does, through `aye_fs::prepare_dir`, which says how.
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
//!   per run that recorded it, oldest first, the last [`RECENT`] (`p`
//!   passed, `f` failed, `e` error, `s` skipped, `x` expected failure, `u`
//!   unexpected pass: [`letter`]), and `bad_since`, the number of the run
//!   (`r81`, from its `run_started`) in which its current streak of bad
//!   outcomes began: kept while it stays bad, the run's own number when it
//!   turns bad (none when the run has none: it isn't stored), cleared when
//!   it isn't bad. Both are optional in the file, so its `version` stays 1
//!   and an older file keeps its durations. A test that finishes twice in
//!   a run has one letter for it, the last result's. [`History::previous`]
//!   gives them as they were before the run being recorded, which the
//!   binary puts in each bad outcome's `history` before anything else sees
//!   it.
//! - **Every run's log is kept** for `aye show` ([`runs`]): the run store,
//!   one more consumer of the stream, writes each run's records (all of
//!   them, as `aye show --log` prints them) gzip-compressed to
//!   `<dir>/runs/r<N>.jsonl.gz` as they arrive (serialized where they
//!   arrive, compressed on a thread of its own), then a summary beside it
//!   (`r<N>.json`: when it ran, its exit code and counts, what its summary
//!   counts beside them, and the digest of each file holding a test it
//!   collected) and `<dir>/runs/last`, each written to a temporary file and
//!   renamed over. Runs are numbered, `r1`, `r2`, …, each claiming its
//!   number before its `run_started`, which says it. It keeps the runs that
//!   finished last (`[tool.aye] keep-runs`, 10 by default; 0 keeps none),
//!   never stores a collect-only run, nor one with nothing to show (no test
//!   ran, nothing failed but the command line's names), so `last` stays
//!   the run before it, and its number is free again.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::hash::Hash;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use aye_events::{Event, Outcome, Previous, Record, TestId, TestResult};
use aye_fs::{HISTORY_TEMPORARY, StateBucket, atomic_write, prepare_dir};
use serde::{Deserialize, Serialize};

pub mod runs;

pub use runs::{Change, RunMeta, RunStore, RunWriter};

/// The history file, inside the directory history is given.
pub const FILE: &str = StateBucket::History.name();

/// The format of the history file. A file of any other version loads as
/// empty history.
pub const VERSION: u32 = 1;

/// The weight of the newest duration in the moving average.
const ALPHA: f64 = 0.3;

/// How many outcomes a test's `recent` keeps.
pub const RECENT: usize = 20;

/// What history knows about one test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestStats {
    /// Moving average of the duration, in milliseconds.
    #[serde(serialize_with = "micros")]
    pub ewma_ms: f64,
    #[serde(serialize_with = "micros")]
    pub max_ms: f64,
    /// How many durations the statistics hold.
    pub count: u32,
    #[serde(serialize_with = "micros")]
    pub last_ms: f64,
    pub last_outcome: Outcome,
    /// The run that last finished this test, when it had an id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    /// Reserved for flake rates; 0 until they land.
    #[serde(default)]
    pub flips: u32,
    /// The test's outcomes in the runs that recorded it, a [`letter`] per
    /// run, oldest first, the last [`RECENT`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub recent: String,
    /// The number of the run in which the test's current streak of bad
    /// outcomes began; `None` when its last outcome isn't bad, or when that
    /// run had no number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bad_since: Option<u64>,
}

impl TestStats {
    fn new(outcome: Outcome) -> Self {
        Self {
            ewma_ms: 0.0,
            max_ms: 0.0,
            count: 0,
            last_ms: 0.0,
            last_outcome: outcome,
            last_run: None,
            flips: 0,
            recent: String::new(),
            bad_since: None,
        }
    }

    fn add(&mut self, ms: f64) {
        self.ewma_ms = if self.count == 0 {
            ms
        } else {
            ALPHA * ms + (1.0 - ALPHA) * self.ewma_ms
        };
        self.max_ms = self.max_ms.max(ms);
        self.last_ms = ms;
        self.count = self.count.saturating_add(1);
    }

    /// The moving average, or `None` when no duration was recorded.
    pub fn ewma(&self) -> Option<Duration> {
        (self.count > 0).then(|| duration(self.ewma_ms))
    }

    fn is_valid(&self) -> bool {
        [self.ewma_ms, self.max_ms, self.last_ms]
            .iter()
            .all(|ms| ms.is_finite() && *ms >= 0.0)
    }
}

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
    id: Option<String>,
    /// Its number in the run store, from its `run_started`.
    number: Option<u64>,
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
    recent: String,
    bad_since: Option<u64>,
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

/// An outcome's letter in a test's `recent`: `p` passed, `f` failed, `e`
/// error, `s` skipped, `x` expected failure, `u` unexpected pass.
pub const fn letter(outcome: Outcome) -> char {
    match outcome {
        Outcome::Passed => 'p',
        Outcome::Failed => 'f',
        Outcome::Error => 'e',
        Outcome::Skipped => 's',
        Outcome::ExpectedFailure => 'x',
        Outcome::UnexpectedPass => 'u',
    }
}

/// `recent` as history keeps it: letters only, the last [`RECENT`].
fn tidy(recent: &mut String) {
    recent.retain(|c| "pfesxu".contains(c));
    let extra = recent.len().saturating_sub(RECENT);
    recent.drain(..extra);
}

#[derive(Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    tests: BTreeMap<TestId, TestStats>,
    #[serde(default)]
    last_failed: BTreeSet<TestId>,
}

#[derive(Serialize)]
struct StoredRef<'a> {
    version: u32,
    tests: &'a BTreeMap<TestId, TestStats>,
    last_failed: &'a BTreeSet<TestId>,
}

impl History {
    /// Reads `<dir>/history.json`. Anything short of a valid file of this
    /// version gives empty history.
    pub fn load(dir: &Path) -> Self {
        fs::read(dir.join(FILE))
            .ok()
            .and_then(|bytes| Self::parse(&bytes))
            .unwrap_or_default()
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut stored: Stored = serde_json::from_slice(bytes).ok()?;
        if stored.version != VERSION || !stored.tests.values().all(TestStats::is_valid) {
            return None;
        }
        for stats in stored.tests.values_mut() {
            tidy(&mut stats.recent);
        }
        Some(Self {
            tests: stored.tests,
            last_failed: stored.last_failed,
            ..Self::default()
        })
    }

    /// Writes `<dir>/history.json` atomically, creating `dir` (see
    /// `aye_fs::prepare_dir`) if needed.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        prepare_dir(dir)?;
        atomic_write(dir, FILE, HISTORY_TEMPORARY, |file| {
            let mut out = io::BufWriter::new(file);
            serde_json::to_writer(
                &mut out,
                &StoredRef {
                    version: VERSION,
                    tests: &self.tests,
                    last_failed: &self.last_failed,
                },
            )?;
            out.write_all(b"\n")?;
            out.flush()
        })
    }

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

    fn finish(&mut self, result: &TestResult, run: Option<String>) {
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
        stats.recent.push(letter(result.outcome));
        tidy(&mut stats.recent);
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
        if !name.contains("::") || id.without_params() != name {
            return None;
        }
        // Ids that start with `name[` sort between `name[` and `name\`.
        let from = TestId(format!("{name}["));
        let to = TestId(format!("{name}\\"));
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
    /// of each test: `durations_by_group(|id| id.path().to_owned())` weighs
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

/// Milliseconds as a duration, saturating.
fn duration(ms: f64) -> Duration {
    Duration::try_from_secs_f64(ms / 1000.0).unwrap_or(Duration::MAX)
}

/// Milliseconds on disk keep microseconds, as on the wire.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature fixed by serde"
)]
fn micros<S: serde::Serializer>(ms: &f64, s: S) -> Result<S::Ok, S::Error> {
    let rounded = (ms * 1000.0).round() / 1000.0;
    s.serialize_f64(if rounded.is_finite() { rounded } else { *ms })
}

#[cfg(test)]
mod tests;
