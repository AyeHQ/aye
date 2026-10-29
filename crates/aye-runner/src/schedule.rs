//! What runs together, and in what order.
//!
//! Cases are grouped by their outermost affinity key (the file, for the
//! unittest and doctest adapters; a costly module or class lifetime, for
//! the pytest adapter's front-end), so that a
//! lifetime is set up once per group rather than once per test. Groups are
//! weighted by historical durations and go into a queue longest-first; each
//! worker pulls the next batch when it finishes one, which balances like LPT
//! scheduling while adapting to durations that history got wrong.
//!
//! A case that declares no affinity is *free*: it shares no costly lifetime
//! (the native engine's tests without module or class setup). A file's free
//! cases go with its first lifetime group when it has one, so that they keep
//! their place among its other cases; otherwise they are grouped by file,
//! and with several workers a file heavier than an even share of the work is
//! split into chunks of about that weight, in order, so that a big file
//! spreads across workers.
//!
//! With several workers, groups lighter than an even share of the work
//! (a quarter of a worker's part) are merged, so that a batch's fixed cost
//! (a round trip, a module or class lifetime) is paid less often while
//! every worker still gets a few batches to balance with. With one worker
//! there is nothing to balance: each adapter's cases form one batch, in
//! plan order.
//!
//! The batches of an adapter whose executor lists the session (the native
//! engine's) keep plan order instead, as pytest-xdist's workers do: only
//! consecutive groups are merged, and the batches take their places in the
//! hand-out in plan order, so each worker runs its tests in plan order (a
//! test that changes what a later one reads, rich's tables changing a box
//! its box tests read, runs after it as under plain pytest).
//!
//! Tests to run first (`--ff`) run first within their batch, in plan order,
//! and the batches holding them are handed out before the others, as
//! pytest runs last run's failures before everything else.
//!
//! A worker runs batches of one adapter only (see `run.rs`), so with
//! several lanes a worker that asks for its next batch prefers one of its
//! own adapter's, after those holding tests to run first: switching
//! adapters costs a new worker. With one lane, batches go in plan order.
//! A batch handed out when nothing else of its adapter is left for its
//! lane, and nothing can be pinned to it, ends its worker's session
//! ([`Assigned::ends_session`]); [`Queue::closing`] makes one with no ids
//! for a worker whose last batch didn't.
//!
//! A batch may be pinned to the first lane ([`Queue::pin`]): the ids of the
//! files whose tests a lane lists otherwise than the first lane does (see
//! `run.rs`). While other lanes may still pin some, the first lane waits for
//! them rather than finishing.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use aye_events::{BatchId, Case, TestId, WorkerId};

use crate::worker::lock;

/// Assumed duration of a test with no history.
pub const DEFAULT_WEIGHT: Duration = Duration::from_millis(10);
/// With several workers, groups lighter than the total over
/// `workers × BATCHES_PER_WORKER` are merged.
const BATCHES_PER_WORKER: u32 = 4;

/// A batch before it is handed out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Planned {
    pub adapter: String,
    pub ids: Vec<TestId>,
    pub weight: Duration,
    /// Holds a case that must not run alongside other serial cases.
    pub serial: bool,
    /// Plan position of its first case.
    first: usize,
    /// Holds a test to run first.
    priority: bool,
}

#[derive(Debug)]
struct Group<'a> {
    adapter: &'a str,
    cases: Vec<(usize, &'a TestId)>,
    weight: Duration,
    serial: bool,
    /// Its cases declare no affinity: it may be split.
    free: bool,
}

/// What a case is grouped by: its outermost affinity key, or its file for
/// a free case (kept apart from affinity keys, which may be file names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key<'a> {
    Affinity(&'a str),
    Free(&'a str),
}

/// Split `cases` into batches, in the order they should be handed out.
/// `first` holds the tests to run first; the batches of the adapters in
/// `ordered` keep plan order (see the module's docs).
pub(crate) fn plan(
    cases: &[Case],
    durations: &HashMap<TestId, Duration>,
    first: &HashSet<TestId>,
    workers: u32,
    ordered: &HashSet<&str>,
) -> Vec<Planned> {
    let weigh = Weights::new(durations);
    // A file's free cases go with its first lifetime group, if it has one,
    // so that they keep their place among its other cases.
    let mut lifetimes: HashMap<(&str, &str), &str> = HashMap::new();
    for case in cases {
        if let Some(key) = case.affinity.first() {
            lifetimes
                .entry((case.adapter.as_str(), case.id.path()))
                .or_insert(key);
        }
    }
    let mut index: HashMap<(&str, Key<'_>), usize> = HashMap::new();
    let mut groups: Vec<Group<'_>> = Vec::new();
    for (position, case) in cases.iter().enumerate() {
        let adapter = case.adapter.as_str();
        let key = match case.affinity.first() {
            Some(key) => Key::Affinity(key),
            None => lifetimes
                .get(&(adapter, case.id.path()))
                .map_or_else(|| Key::Free(case.id.path()), |key| Key::Affinity(key)),
        };
        let at = *index.entry((adapter, key)).or_insert_with(|| {
            groups.push(Group {
                adapter,
                cases: Vec::new(),
                weight: Duration::ZERO,
                serial: false,
                free: matches!(key, Key::Free(_)),
            });
            groups.len() - 1
        });
        let group = &mut groups[at];
        group.cases.push((position, &case.id));
        group.weight += weigh.of(&case.id);
        group.serial |= case.serial;
    }

    let mut batches = if workers <= 1 {
        let mut by_adapter: Vec<Vec<Group<'_>>> = Vec::new();
        for group in groups {
            match by_adapter
                .iter_mut()
                .find(|b| b[0].adapter == group.adapter)
            {
                Some(batch) => batch.push(group),
                None => by_adapter.push(vec![group]),
            }
        }
        by_adapter
    } else {
        let total: Duration = groups.iter().map(|g| g.weight).sum();
        let target = total / workers.saturating_mul(BATCHES_PER_WORKER);
        let (in_order, mut groups): (Vec<Group<'_>>, Vec<Group<'_>>) = groups
            .into_iter()
            .flat_map(|group| split(group, target, &weigh, cases))
            .partition(|group| ordered.contains(group.adapter));
        groups.sort_by(|a, b| {
            b.weight
                .cmp(&a.weight)
                .then(a.cases[0].0.cmp(&b.cases[0].0))
        });
        let mut batches = in_plan_order(in_order, target);
        // The batch still being filled, per adapter and seriality.
        let mut open: HashMap<(&str, bool), usize> = HashMap::new();
        for group in groups {
            if group.weight >= target {
                batches.push(vec![group]);
                continue;
            }
            let slot = (group.adapter, group.serial);
            match open.get(&slot) {
                Some(&at) if weight(&batches[at]) + group.weight <= target => {
                    batches[at].push(group);
                }
                _ => {
                    open.insert(slot, batches.len());
                    batches.push(vec![group]);
                }
            }
        }
        batches
    }
    .into_iter()
    .map(|mut batch| {
        batch.sort_by_key(|g| g.cases[0].0);
        let mut ids: Vec<(usize, &TestId)> =
            batch.iter().flat_map(|g| g.cases.iter().copied()).collect();
        // Tests to run first go first, in plan order; the rest keep their
        // groups.
        let runs_first = |id: &TestId| first.contains(id);
        let priority = ids.iter().any(|(_, id)| runs_first(id));
        if priority {
            let mut at_front: Vec<(usize, &TestId)> = ids
                .iter()
                .copied()
                .filter(|(_, id)| runs_first(id))
                .collect();
            at_front.sort_by_key(|(position, _)| *position);
            at_front.extend(ids.iter().copied().filter(|(_, id)| !runs_first(id)));
            ids = at_front;
        }
        Planned {
            adapter: batch[0].adapter.to_string(),
            weight: weight(&batch),
            serial: batch.iter().any(|g| g.serial),
            first: batch[0].cases[0].0,
            priority,
            ids: ids.into_iter().map(|(_, id)| id.clone()).collect(),
        }
    })
    .collect::<Vec<_>>();

    if workers <= 1 {
        batches.sort_by_key(|b| (!b.priority, b.first));
    } else {
        batches.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(b.weight.cmp(&a.weight))
                .then(a.first.cmp(&b.first))
        });
        // The ordered adapters' batches take their places in plan order.
        let slots: Vec<usize> = (0..batches.len())
            .filter(|&at| ordered.contains(batches[at].adapter.as_str()))
            .collect();
        let mut in_order: Vec<Planned> = slots.iter().map(|&at| batches[at].clone()).collect();
        in_order.sort_by_key(|b| (!b.priority, b.first));
        for (at, batch) in slots.into_iter().zip(in_order) {
            batches[at] = batch;
        }
    }
    batches
}

/// `groups` as batches of consecutive groups (by their first case's plan
/// position) of one adapter and seriality, each up to `target` in weight,
/// a group of `target` or more alone: handed out in plan order, each lane
/// runs its tests in plan order, as pytest-xdist's workers run theirs.
fn in_plan_order<'a>(mut groups: Vec<Group<'a>>, target: Duration) -> Vec<Vec<Group<'a>>> {
    groups.sort_by_key(|group| group.cases[0].0);
    let mut batches: Vec<Vec<Group<'a>>> = Vec::new();
    let mut open = false;
    for group in groups {
        let joins = open
            && group.weight < target
            && batches.last().is_some_and(|batch| {
                batch[0].adapter == group.adapter
                    && batch[0].serial == group.serial
                    && weight(batch) + group.weight <= target
            });
        open = group.weight < target;
        match batches.last_mut() {
            Some(batch) if joins => batch.push(group),
            _ => batches.push(vec![group]),
        }
    }
    batches
}

fn weight(groups: &[Group<'_>]) -> Duration {
    groups.iter().map(|g| g.weight).sum()
}

/// A free group heavier than `target`, as chunks of about that weight, in
/// order: each closes once it weighs `target` or more. Other groups, and
/// every group when `target` is zero, stay whole.
fn split<'a>(
    group: Group<'a>,
    target: Duration,
    weigh: &Weights<'_>,
    cases: &[Case],
) -> Vec<Group<'a>> {
    if !group.free || target.is_zero() || group.weight <= target {
        return vec![group];
    }
    let mut chunks: Vec<Group<'a>> = Vec::new();
    let mut open: Option<Group<'a>> = None;
    for (position, id) in group.cases {
        let chunk = open.get_or_insert_with(|| Group {
            adapter: group.adapter,
            cases: Vec::new(),
            weight: Duration::ZERO,
            serial: false,
            free: true,
        });
        chunk.cases.push((position, id));
        chunk.weight += weigh.of(id);
        chunk.serial |= cases[position].serial;
        if chunk.weight >= target {
            chunks.extend(open.take());
        }
    }
    chunks.extend(open);
    chunks
}

/// Historical durations, looked up by id. A case collected without its
/// parameters (`t.py::test_x`) weighs what its parametrized tests weighed.
struct Weights<'a> {
    exact: &'a HashMap<TestId, Duration>,
    unparametrized: HashMap<&'a str, Duration>,
}

impl<'a> Weights<'a> {
    fn new(exact: &'a HashMap<TestId, Duration>) -> Self {
        let mut unparametrized: HashMap<&str, Duration> = HashMap::new();
        for (id, duration) in exact {
            let base = id.without_params();
            if base.len() != id.as_str().len() {
                *unparametrized.entry(base).or_default() += *duration;
            }
        }
        Self {
            exact,
            unparametrized,
        }
    }

    fn of(&self, id: &TestId) -> Duration {
        self.exact
            .get(id)
            .or_else(|| self.unparametrized.get(id.as_str()))
            .copied()
            .unwrap_or(DEFAULT_WEIGHT)
    }
}

/// A batch as handed to a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Assigned {
    pub id: BatchId,
    pub adapter: String,
    pub ids: Vec<TestId>,
    pub serial: bool,
    /// Already re-queued once after a crash.
    pub retry: bool,
    /// For a re-queued batch: the tests that ran in the batch that crashed,
    /// which the adapter must not run again (see `run.rs`).
    pub ran: Vec<TestId>,
    /// The only lane that may run it: the first, for the tests of files
    /// another lane lists otherwise ([`Queue::pin`]).
    pub lane: Option<WorkerId>,
    /// Holds a test to run first.
    priority: bool,
    /// When it was handed out, nothing else of its adapter was left for
    /// its lane, and nothing could be pinned to it: its worker's last batch
    /// ([`aye_engine::Batch::ends_session`]), unless a crashed batch comes
    /// back. Not kept when it goes back to the queue.
    pub ends_session: bool,
}

#[derive(Debug)]
struct Pending {
    /// Kept when a batch is put back unstarted, so ids stay dense.
    id: Option<BatchId>,
    adapter: String,
    ids: Vec<TestId>,
    serial: bool,
    retry: bool,
    ran: Vec<TestId>,
    lane: Option<WorkerId>,
    priority: bool,
}

impl Pending {
    fn of(batch: Assigned, id: Option<BatchId>) -> Self {
        Self {
            id,
            adapter: batch.adapter,
            ids: batch.ids,
            serial: batch.serial,
            retry: batch.retry,
            ran: batch.ran,
            lane: batch.lane,
            priority: batch.priority,
        }
    }

    fn eligible(&self, lane: WorkerId) -> bool {
        self.lane.is_none_or(|only| only == lane)
    }
}

/// The batches still to run. Workers pull from it; a serial batch is not
/// handed out while another one runs.
#[derive(Debug)]
pub(crate) struct Queue {
    state: std::sync::Mutex<State>,
    changed: std::sync::Condvar,
}

#[derive(Debug, Default)]
struct State {
    pending: VecDeque<Pending>,
    next_id: BatchId,
    serial_running: bool,
    halted: bool,
    /// Lanes other than the first that may still pin batches to it
    /// ([`Queue::pin`]): until none is left, the first lane waits for work
    /// rather than finishing.
    pinning: HashSet<WorkerId>,
    /// The first lane is done taking batches: nothing is pinned to it any
    /// more.
    first_done: bool,
}

impl Queue {
    pub(crate) fn new(planned: Vec<Planned>) -> Self {
        let pending = planned
            .into_iter()
            .map(|p| Pending {
                id: None,
                adapter: p.adapter,
                ids: p.ids,
                serial: p.serial,
                retry: false,
                ran: Vec::new(),
                lane: None,
                priority: p.priority,
            })
            .collect();
        Self {
            state: std::sync::Mutex::new(State {
                pending,
                ..State::default()
            }),
            changed: std::sync::Condvar::new(),
        }
    }

    /// The next batch `lane` may run, waiting while only serial batches are
    /// left and one is running, and, for the first lane, while other lanes
    /// may still pin batches to it. `None` when nothing is left for `lane`
    /// or the queue was halted. Those holding tests to run first go first;
    /// among the rest, the first of the adapter `prefer` (the asking
    /// worker's), else the first.
    pub(crate) fn take(&self, lane: WorkerId, prefer: Option<&str>) -> Option<Assigned> {
        let mut state = lock(&self.state);
        loop {
            if state.halted {
                state.first_done |= lane == 0;
                return None;
            }
            let blocked = state.serial_running;
            let chosen = state
                .pending
                .iter()
                .enumerate()
                .filter(|(_, p)| p.eligible(lane) && !(p.serial && blocked))
                .min_by_key(|(at, p)| (!p.priority, Some(p.adapter.as_str()) != prefer, *at))
                .map(|(at, _)| at);
            if let Some(at) = chosen {
                let pending = state.pending.remove(at).expect("position is in range");
                let id = pending.id.unwrap_or_else(|| {
                    state.next_id += 1;
                    state.next_id - 1
                });
                state.serial_running |= pending.serial;
                let ends_session = (lane != 0 || state.pinning.is_empty())
                    && !state
                        .pending
                        .iter()
                        .any(|p| p.adapter == pending.adapter && p.eligible(lane));
                return Some(Assigned {
                    id,
                    adapter: pending.adapter,
                    ids: pending.ids,
                    serial: pending.serial,
                    retry: pending.retry,
                    ran: pending.ran,
                    lane: pending.lane,
                    priority: pending.priority,
                    ends_session,
                });
            }
            let more = state.pending.iter().any(|p| p.eligible(lane))
                || (lane == 0 && !state.pinning.is_empty());
            if !more {
                // The first lane finishes: nothing can be pinned to it now.
                state.first_done |= lane == 0;
                return None;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// A batch of `adapter` with no ids that only ends its worker's session
    /// ([`aye_engine::Batch::ends_session`]): for a worker whose last batch
    /// didn't, as its lane finishes or leaves the adapter.
    pub(crate) fn closing(&self, adapter: &str) -> Assigned {
        let mut state = lock(&self.state);
        state.next_id += 1;
        Assigned {
            id: state.next_id - 1,
            adapter: adapter.to_string(),
            ids: Vec::new(),
            serial: false,
            retry: false,
            ran: Vec::new(),
            lane: None,
            priority: false,
            ends_session: true,
        }
    }

    /// Return a batch that never started, to the front.
    pub(crate) fn put_back(&self, batch: Assigned) {
        let mut state = lock(&self.state);
        state.serial_running &= !batch.serial;
        let id = Some(batch.id);
        state.pending.push_front(Pending::of(batch, id));
        self.changed.notify_all();
    }

    /// Every lane but the first among `lanes` may pin batches to it: the
    /// first lane waits for work until each has said it won't
    /// ([`Queue::may_pin`]) or is done ([`Queue::lane_done`]).
    pub(crate) fn expect_pins(&self, lanes: u32) {
        lock(&self.state).pinning = (1..lanes).collect();
    }

    /// Whether `lane` (not the first) may pin batches from now on: its new
    /// worker lists some files otherwise than the first lane, or none.
    pub(crate) fn may_pin(&self, lane: WorkerId, may: bool) {
        if lane == 0 {
            return;
        }
        let mut state = lock(&self.state);
        if may {
            state.pinning.insert(lane);
        } else {
            state.pinning.remove(&lane);
        }
        self.changed.notify_all();
    }

    /// A lane is done taking batches: if it isn't the first, it pins no
    /// more; if it is, nothing can be pinned to it any more.
    pub(crate) fn lane_done(&self, lane: WorkerId) {
        let mut state = lock(&self.state);
        if lane == 0 {
            state.first_done = true;
        } else {
            state.pinning.remove(&lane);
        }
        self.changed.notify_all();
    }

    /// Leave the ids `pinned` says must run on the first lane to it, as a
    /// batch at the front that only it may take, and return the rest of
    /// `batch`, `None` when nothing is left (the whole batch goes back).
    /// Once the first lane is done, nothing is pinned: the batch comes back
    /// whole, for the asking lane to run.
    pub(crate) fn pin(
        &self,
        mut batch: Assigned,
        pinned: impl Fn(&TestId) -> bool,
    ) -> Option<Assigned> {
        let mut state = lock(&self.state);
        if state.first_done {
            return Some(batch);
        }
        let (moved, kept): (Vec<TestId>, Vec<TestId>) = std::mem::take(&mut batch.ids)
            .into_iter()
            .partition(|id| pinned(id));
        if moved.is_empty() {
            batch.ids = kept;
            return Some(batch);
        }
        if kept.is_empty() {
            batch.ids = moved;
            batch.lane = Some(0);
            state.serial_running &= !batch.serial;
            let id = Some(batch.id);
            state.pending.push_front(Pending::of(batch, id));
            self.changed.notify_all();
            return None;
        }
        state.pending.push_front(Pending {
            id: None,
            adapter: batch.adapter.clone(),
            ids: moved,
            serial: batch.serial,
            retry: batch.retry,
            ran: batch.ran.clone(),
            lane: Some(0),
            priority: batch.priority,
        });
        self.changed.notify_all();
        batch.ids = kept;
        Some(batch)
    }

    /// Re-queue the ids of a crashed batch that never started, to the front,
    /// as a new batch that will not be retried again, with the tests that
    /// ran (`ran`), which must not run again.
    pub(crate) fn retry(&self, batch: &Assigned, ids: Vec<TestId>, ran: Vec<TestId>) {
        let mut state = lock(&self.state);
        state.pending.push_front(Pending {
            id: None,
            adapter: batch.adapter.clone(),
            ids,
            serial: batch.serial,
            retry: true,
            ran,
            lane: batch.lane,
            priority: batch.priority,
        });
        self.changed.notify_all();
    }

    /// A batch is over, however it ended.
    pub(crate) fn finished(&self, batch: &Assigned) {
        if batch.serial {
            lock(&self.state).serial_running = false;
            self.changed.notify_all();
        }
    }

    /// Hand nothing more out.
    pub(crate) fn halt(&self) {
        lock(&self.state).halted = true;
        self.changed.notify_all();
    }

    pub(crate) fn is_halted(&self) -> bool {
        lock(&self.state).halted
    }

    /// Take every batch still pending: the ids that were never handed out.
    pub(crate) fn drain(&self) -> Vec<TestId> {
        let mut state = lock(&self.state);
        state.pending.drain(..).flat_map(|p| p.ids).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    /// A case whose file is its lifetime, as the adapters but the native
    /// engine declare.
    fn case(id: &str) -> Case {
        let mut case = free(id);
        case.affinity = vec![case.id.path().to_string()];
        case
    }

    /// A case that shares no lifetime.
    fn free(id: &str) -> Case {
        Case::new(id, "pytest")
    }

    /// No adapter whose batches keep plan order.
    fn none() -> HashSet<&'static str> {
        HashSet::new()
    }

    fn ids(batch: &Planned) -> Vec<&str> {
        batch.ids.iter().map(TestId::as_str).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn history(entries: &[(&str, u64)]) -> HashMap<TestId, Duration> {
        entries
            .iter()
            .map(|(id, n)| (TestId::new(*id), ms(*n)))
            .collect()
    }

    #[test]
    fn keeps_files_together_longest_first() {
        let cases = [
            case("a.py::t1"),
            case("b.py::t1"),
            case("a.py::t2"),
            case("c.py::t1"),
        ];
        let durations = history(&[("c.py::t1", 5000), ("a.py::t1", 1500), ("b.py::t1", 1000)]);
        let batches = plan(&cases, &durations, &HashSet::new(), 2, &none());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(
            got,
            [
                vec!["c.py::t1"],
                vec!["a.py::t1", "a.py::t2"],
                vec!["b.py::t1"]
            ]
        );
        assert_eq!(batches[0].weight, ms(5000));
        assert_eq!(batches[1].weight, ms(1510));
    }

    #[test]
    fn groups_by_the_outermost_affinity_key() {
        let mut a = case("a.py::A::t1");
        a.affinity = vec!["a.py".into(), "a.py::A".into()];
        let mut b = case("a.py::B::t1");
        b.affinity = vec!["a.py".into(), "a.py::B".into()];
        let mut odd = case("b.py::t1");
        odd.affinity = vec!["shared-db".into()];
        let mut odd2 = case("c.py::t1");
        odd2.affinity = vec!["shared-db".into()];
        let batches = plan(
            &[a, odd, b, odd2],
            &HashMap::new(),
            &HashSet::new(),
            8,
            &none(),
        );
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(
            got,
            [
                vec!["a.py::A::t1", "a.py::B::t1"],
                vec!["b.py::t1", "c.py::t1"]
            ]
        );
    }

    #[test]
    fn adapters_never_share_a_batch() {
        let cases = [case("a.py::t1"), Case::new("a.py::t1", "doctest")];
        let batches = plan(&cases, &HashMap::new(), &HashSet::new(), 1, &none());
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].adapter, "pytest");
        assert_eq!(batches[1].adapter, "doctest");
    }

    #[test]
    fn one_worker_gets_one_batch_per_adapter_in_plan_order() {
        let cases = [case("b.py::t1"), case("a.py::t1"), case("b.py::t2")];
        let durations = history(&[("a.py::t1", 900)]);
        let batches = plan(&cases, &durations, &HashSet::new(), 1, &none());
        assert_eq!(batches.len(), 1);
        assert_eq!(ids(&batches[0]), ["b.py::t1", "b.py::t2", "a.py::t1"]);
    }

    fn first(ids: &[&str]) -> HashSet<TestId> {
        ids.iter().map(|id| TestId::new(*id)).collect()
    }

    #[test]
    fn tests_to_run_first_lead_their_batch_across_files() {
        let cases: Vec<Case> = ["a1", "a2", "a3", "a4"]
            .iter()
            .map(|n| case(&format!("a.py::{n}")))
            .chain(
                ["b1", "b2", "b3"]
                    .iter()
                    .map(|n| case(&format!("b.py::{n}"))),
            )
            .collect();
        let batches = plan(
            &cases,
            &HashMap::new(),
            &first(&["b.py::b3", "a.py::a3"]),
            1,
            &none(),
        );
        assert_eq!(batches.len(), 1);
        assert_eq!(
            ids(&batches[0]),
            [
                "a.py::a3", "b.py::b3", "a.py::a1", "a.py::a2", "a.py::a4", "b.py::b1", "b.py::b2"
            ]
        );
    }

    #[test]
    fn batches_with_tests_to_run_first_are_handed_out_first() {
        let cases = [case("a.py::t1"), case("b.py::t1"), case("c.py::t1")];
        let durations = history(&[("a.py::t1", 5000), ("b.py::t1", 1000), ("c.py::t1", 3000)]);
        let batches = plan(&cases, &durations, &first(&["b.py::t1"]), 2, &none());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(got, [vec!["b.py::t1"], vec!["a.py::t1"], vec!["c.py::t1"]]);
        let none = plan(&cases, &durations, &HashSet::new(), 2, &none());
        assert_eq!(ids(&none[0]), ["a.py::t1"]);
    }

    #[test]
    fn small_groups_are_merged_with_several_workers() {
        // 1000ms in all, over 2 workers: groups under 125ms are merged.
        let durations = history(&[
            ("big.py::t", 700),
            ("s1.py::t", 60),
            ("s2.py::t", 60),
            ("s3.py::t", 60),
            ("s4.py::t", 60),
            ("s5.py::t", 60),
        ]);
        let cases: Vec<Case> = ["s5", "s1", "big", "s2", "s3", "s4"]
            .iter()
            .map(|name| case(&format!("{name}.py::t")))
            .collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 2, &none());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(
            got,
            [
                vec!["big.py::t"],
                vec!["s5.py::t", "s1.py::t"],
                vec!["s2.py::t", "s3.py::t"],
                vec!["s4.py::t"],
            ]
        );
    }

    #[test]
    fn serial_groups_are_not_merged_with_others() {
        let mut serial = case("s.py::t");
        serial.serial = true;
        let cases = [case("a.py::t"), serial, case("b.py::t"), case("c.py::t")];
        let durations = history(&[("big.py::t", 10_000)]);
        let mut all = cases.to_vec();
        all.push(case("big.py::t"));
        let batches = plan(&all, &durations, &HashSet::new(), 2, &none());
        let serial: Vec<&Planned> = batches.iter().filter(|b| b.serial).collect();
        assert_eq!(serial.len(), 1);
        assert_eq!(ids(serial[0]), ["s.py::t"]);
    }

    #[test]
    fn unparametrized_cases_weigh_their_parametrized_history() {
        let durations = history(&[("a.py::t[1]", 300), ("a.py::t[2]", 400), ("b.py::u", 50)]);
        let batches = plan(
            &[case("b.py::u"), case("a.py::t")],
            &durations,
            &HashSet::new(),
            2,
            &none(),
        );
        assert_eq!(ids(&batches[0]), ["a.py::t"]);
        assert_eq!(batches[0].weight, ms(700));
    }

    #[test]
    fn a_heavy_files_free_cases_are_split_by_duration_share() {
        // 1400ms over 2 workers: chunks close at 175ms or more.
        let mut cases: Vec<Case> = vec![case("a.py::t0"), case("a.py::t1")];
        cases.extend((0..12).map(|n| free(&format!("big.py::t{n}"))));
        let durations: HashMap<TestId, Duration> =
            cases.iter().map(|c| (c.id.clone(), ms(100))).collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 2, &none());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        let mut want = vec![vec!["a.py::t0", "a.py::t1"]];
        for n in (0..12).step_by(2) {
            want.push(vec![cases[2 + n].id.as_str(), cases[3 + n].id.as_str()]);
        }
        assert_eq!(got, want);
        assert!(batches.iter().all(|b| b.weight == ms(200)));
    }

    #[test]
    fn groups_with_affinity_are_never_split() {
        let cases: Vec<Case> = (0..12).map(|n| case(&format!("big.py::t{n}"))).collect();
        let mut all = cases.clone();
        all.push(free("small.py::t"));
        let durations: HashMap<TestId, Duration> =
            cases.iter().map(|c| (c.id.clone(), ms(100))).collect();
        let batches = plan(&all, &durations, &HashSet::new(), 4, &none());
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].ids.len(), 12);
        assert_eq!(ids(&batches[1]), ["small.py::t"]);
    }

    #[test]
    fn free_cases_without_weight_are_not_split() {
        let cases: Vec<Case> = (0..6).map(|n| free(&format!("big.py::t{n}"))).collect();
        let durations: HashMap<TestId, Duration> = cases
            .iter()
            .map(|c| (c.id.clone(), Duration::ZERO))
            .collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 4, &none());
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].ids.len(), 6);
    }

    #[test]
    fn a_chunk_is_serial_only_when_it_holds_a_serial_case() {
        let mut cases: Vec<Case> = (0..4).map(|n| free(&format!("big.py::t{n}"))).collect();
        cases[3].serial = true;
        let durations: HashMap<TestId, Duration> =
            cases.iter().map(|c| (c.id.clone(), ms(100))).collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 2, &none());
        let serial: Vec<(Vec<&str>, bool)> = batches.iter().map(|b| (ids(b), b.serial)).collect();
        assert_eq!(
            serial,
            [
                (vec!["big.py::t0"], false),
                (vec!["big.py::t1"], false),
                (vec!["big.py::t2"], false),
                (vec!["big.py::t3"], true),
            ]
        );
    }

    #[test]
    fn with_one_worker_free_cases_keep_their_place_among_their_files_cases() {
        let module = |id: &str| {
            let mut case = free(id);
            case.affinity = vec![format!("module:{}", case.id.path())];
            case
        };
        let cases = [
            free("a.py::t1"),
            module("a.py::t2"),
            free("a.py::t3"),
            module("a.py::t4"),
            free("b.py::t1"),
            free("c.py::t1"),
            module("c.py::t2"),
        ];
        let batches = plan(&cases, &HashMap::new(), &HashSet::new(), 1, &none());
        assert_eq!(batches.len(), 1);
        let plan_order: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids(&batches[0]), plan_order);
    }

    fn ordered() -> HashSet<&'static str> {
        ["pytest"].into_iter().collect()
    }

    #[test]
    fn an_ordered_adapters_batches_are_consecutive_and_handed_out_in_plan_order() {
        // 2000ms over 2 workers: batches up to 250ms.
        let mut cases = vec![free("a.py::t0"), free("a.py::t1"), free("a.py::t2")];
        cases.extend((0..6).map(|n| free(&format!("b.py::t{n}"))));
        cases.push(case("c.py::heavy"));
        cases.extend([free("d.py::t0"), free("e.py::t0")]);
        let mut entries: Vec<(String, u64)> = (0..3).map(|n| (format!("a.py::t{n}"), 50)).collect();
        entries.extend((0..6).map(|n| (format!("b.py::t{n}"), 100)));
        entries.extend([
            ("c.py::heavy".to_string(), 1000),
            ("d.py::t0".to_string(), 100),
            ("e.py::t0".to_string(), 150),
        ]);
        let durations: HashMap<TestId, Duration> = entries
            .iter()
            .map(|(id, n)| (TestId::new(id.as_str()), ms(*n)))
            .collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 2, &ordered());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(
            got,
            [
                vec!["a.py::t0", "a.py::t1", "a.py::t2"],
                vec!["b.py::t0", "b.py::t1", "b.py::t2"],
                vec!["b.py::t3", "b.py::t4", "b.py::t5"],
                vec!["c.py::heavy"],
                vec!["d.py::t0", "e.py::t0"],
            ]
        );
        // Without the order, longest first.
        let lpt = plan(&cases, &durations, &HashSet::new(), 2, &none());
        assert_eq!(ids(&lpt[0]), ["c.py::heavy"]);
    }

    #[test]
    fn a_files_free_cases_go_with_its_lifetime_group_whatever_the_workers() {
        let mut module = free("a.py::t2");
        module.affinity = vec!["module:a.py".into()];
        let cases = [
            free("a.py::t1"),
            module,
            free("a.py::t3"),
            free("b.py::t1"),
            free("b.py::t2"),
        ];
        let durations: HashMap<TestId, Duration> =
            cases.iter().map(|c| (c.id.clone(), ms(100))).collect();
        let batches = plan(&cases, &durations, &HashSet::new(), 4, &ordered());
        let got: Vec<Vec<&str>> = batches.iter().map(ids).collect();
        assert_eq!(
            got,
            [
                vec!["a.py::t1", "a.py::t2", "a.py::t3"],
                vec!["b.py::t1"],
                vec!["b.py::t2"],
            ]
        );
    }

    #[test]
    fn ordered_batches_keep_plan_order_among_the_others() {
        let mut cases: Vec<Case> = ["a.py::t", "b.py::t", "c.py::t"]
            .iter()
            .map(|id| free(id))
            .collect();
        cases.extend(["x.txt::d", "y.txt::d"].iter().map(|id| {
            let mut case = Case::new(*id, "doctest");
            case.affinity = vec![case.id.path().to_string()];
            case
        }));
        let durations = history(&[
            ("a.py::t", 100),
            ("b.py::t", 300),
            ("c.py::t", 500),
            ("x.txt::d", 200),
            ("y.txt::d", 400),
        ]);
        let batches = plan(&cases, &durations, &HashSet::new(), 4, &ordered());
        let got: Vec<&str> = batches.iter().map(|b| b.ids[0].as_str()).collect();
        // By weight, the slots are c, y, b, x, a; pytest's take theirs in
        // plan order.
        assert_eq!(
            got,
            ["a.py::t", "y.txt::d", "b.py::t", "x.txt::d", "c.py::t"]
        );
    }

    #[test]
    fn nothing_to_run_is_no_batches() {
        assert_eq!(
            plan(&[], &HashMap::new(), &HashSet::new(), 4, &none()).len(),
            0
        );
    }

    fn queue(names: &[(&str, bool)]) -> Queue {
        queue_of(
            &names
                .iter()
                .map(|(name, serial)| ("pytest", *name, *serial, false))
                .collect::<Vec<_>>(),
        )
    }

    /// Batches of one test each: (adapter, id, serial, priority).
    fn queue_of(batches: &[(&str, &str, bool, bool)]) -> Queue {
        Queue::new(
            batches
                .iter()
                .map(|(adapter, name, serial, priority)| Planned {
                    adapter: (*adapter).into(),
                    ids: vec![TestId::new(*name)],
                    weight: DEFAULT_WEIGHT,
                    serial: *serial,
                    first: 0,
                    priority: *priority,
                })
                .collect(),
        )
    }

    fn taken(q: &Queue, prefer: Option<&str>) -> String {
        q.take(0, prefer).unwrap().ids[0].0.clone()
    }

    #[test]
    fn a_worker_gets_its_own_adapters_batches_first() {
        let q = queue_of(&[
            ("pytest", "p1", false, false),
            ("doctest", "d1", false, false),
            ("pytest", "p2", false, false),
            ("doctest", "d2", false, false),
        ]);
        assert_eq!(taken(&q, Some("doctest")), "d1");
        assert_eq!(taken(&q, Some("doctest")), "d2");
        // None of its own left: the next in line, whatever its adapter.
        assert_eq!(taken(&q, Some("doctest")), "p1");
        assert_eq!(taken(&q, None), "p2");
        assert_eq!(q.take(0, Some("pytest")), None);
    }

    #[test]
    fn a_lanes_last_batch_of_an_adapter_ends_its_workers_session() {
        let q = queue_of(&[
            ("pytest", "p1", false, false),
            ("doctest", "d1", false, false),
            ("pytest", "p2", false, false),
        ]);
        let p1 = q.take(0, Some("pytest")).unwrap();
        assert!(!p1.ends_session, "p2 is left");
        let p2 = q.take(0, Some("pytest")).unwrap();
        assert!(p2.ends_session);
        // A crashed batch that comes back goes to another worker.
        q.retry(&p2, vec![TestId::new("p2")], Vec::new());
        let d1 = q.take(0, Some("doctest")).unwrap();
        assert!(d1.ends_session);
        assert!(q.take(0, Some("pytest")).unwrap().ends_session);
        assert_eq!(q.take(0, None), None);

        // The first lane's never does while other lanes may pin batches to
        // it; a batch pinned to it is no other lane's.
        let q = queue_of(&[
            ("pytest", "p1", false, false),
            ("pytest", "p2", false, false),
        ]);
        q.expect_pins(2);
        let p1 = q.take(1, None).unwrap();
        assert!(!p1.ends_session);
        assert_eq!(q.pin(p1, |_| true), None);
        assert!(
            q.take(1, None).unwrap().ends_session,
            "p1 is the first lane's"
        );
        q.lane_done(1);
        assert!(q.take(0, None).unwrap().ends_session);

        // A batch that only ends a session: no ids, an id of its own.
        let closing = q.closing("pytest");
        assert!(closing.ends_session && closing.ids.is_empty());
        assert_eq!((closing.id, closing.adapter.as_str()), (2, "pytest"));
    }

    #[test]
    fn tests_to_run_first_go_first_whatever_the_workers_adapter() {
        let q = queue_of(&[
            ("doctest", "d1", false, true),
            ("pytest", "p1", false, false),
            ("pytest", "p2", false, true),
        ]);
        assert_eq!(taken(&q, Some("pytest")), "p2");
        assert_eq!(taken(&q, Some("pytest")), "d1");
        assert_eq!(taken(&q, Some("pytest")), "p1");
    }

    #[test]
    fn a_retry_keeps_its_priority_and_adapter() {
        let q = queue_of(&[
            ("doctest", "d1", false, true),
            ("pytest", "p1", false, false),
        ]);
        let first = q.take(0, None).unwrap();
        q.retry(&first, vec![TestId::new("d1b")], Vec::new());
        assert_eq!(taken(&q, Some("pytest")), "d1b");
    }

    #[test]
    fn hands_out_in_order_with_dense_ids() {
        let q = queue(&[("a", false), ("b", false), ("c", false)]);
        let a = q.take(0, None).unwrap();
        let b = q.take(0, None).unwrap();
        assert_eq!((a.id, a.ids[0].as_str()), (0, "a"));
        assert_eq!((b.id, b.ids[0].as_str()), (1, "b"));
        q.put_back(b);
        let again = q.take(0, None).unwrap();
        assert_eq!((again.id, again.ids[0].as_str()), (1, "b"));
        q.retry(&a, vec![TestId::new("a2")], vec![TestId::new("a1")]);
        let retry = q.take(0, None).unwrap();
        assert_eq!(
            (retry.id, retry.ids[0].as_str(), retry.retry),
            (2, "a2", true)
        );
        assert_eq!(retry.ran, [TestId::new("a1")]);
        assert_eq!(q.take(0, None).unwrap().id, 3);
        assert_eq!(q.take(0, None), None);
    }

    #[test]
    fn pinned_ids_go_to_the_first_lane_only() {
        let q = Queue::new(vec![Planned {
            adapter: "pytest".into(),
            ids: ["a.py::t1", "b.py::t1", "a.py::t2"]
                .into_iter()
                .map(TestId::new)
                .collect(),
            weight: DEFAULT_WEIGHT,
            serial: false,
            first: 0,
            priority: false,
        }]);
        let batch = q.take(1, None).unwrap();
        let kept = q.pin(batch, |id| id.path() == "a.py").unwrap();
        assert_eq!(
            (kept.id, kept.ids.clone()),
            (0, vec![TestId::new("b.py::t1")])
        );
        assert_eq!(q.take(1, None), None, "pinned to the first lane");
        let pinned = q.take(0, None).unwrap();
        assert_eq!(pinned.id, 1);
        assert_eq!(pinned.lane, Some(0));
        let names: Vec<&str> = pinned.ids.iter().map(TestId::as_str).collect();
        assert_eq!(names, ["a.py::t1", "a.py::t2"]);
        // Its retry stays on the first lane.
        q.retry(&pinned, vec![TestId::new("a.py::t2")], Vec::new());
        assert_eq!(q.take(1, None), None);
        assert_eq!(q.take(0, None).unwrap().lane, Some(0));
    }

    #[test]
    fn a_batch_pinned_whole_goes_back_with_its_id() {
        let q = queue(&[("a.py::t", false), ("b.py::t", false)]);
        let batch = q.take(1, None).unwrap();
        assert_eq!(q.pin(batch, |_| true), None);
        assert_eq!(taken(&q, None), "a.py::t");
        assert_eq!(q.take(1, None).unwrap().ids[0].as_str(), "b.py::t");
        let unpinned = q.take(1, None);
        assert_eq!(unpinned, None);
        let q = queue(&[("a.py::t", false)]);
        let batch = q.take(1, None).unwrap();
        let kept = q.pin(batch, |_| false).unwrap();
        assert_eq!(kept.lane, None);
        assert_eq!(kept.ids.len(), 1);
    }

    #[test]
    fn the_first_lane_waits_while_other_lanes_may_pin() {
        let q = Arc::new(queue(&[("a.py::t", false), ("b.py::t", false)]));
        q.expect_pins(3);
        let other = q.take(1, None).unwrap();
        let _third = q.take(2, None).unwrap();
        let first = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(0, None))
        };
        thread::sleep(ms(50));
        assert!(!first.is_finished());
        assert_eq!(q.pin(other, |_| true), None);
        assert_eq!(first.join().unwrap().unwrap().ids[0].as_str(), "a.py::t");
        let first = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(0, None))
        };
        q.lane_done(1);
        thread::sleep(ms(50));
        assert!(!first.is_finished(), "lane 2 may still pin");
        q.may_pin(2, true);
        thread::sleep(ms(20));
        assert!(!first.is_finished(), "lane 2 may still pin");
        // Its worker lists the session as the first lane's does.
        q.may_pin(2, false);
        assert_eq!(first.join().unwrap(), None);
        // Without pins expected, the first lane finishes as the others do.
        let q = queue(&[]);
        assert_eq!(q.take(0, None), None);
    }

    #[test]
    fn nothing_is_pinned_once_the_first_lane_is_done() {
        let q = queue(&[("a.py::t", false)]);
        q.expect_pins(2);
        let batch = q.take(1, None).unwrap();
        q.lane_done(0);
        let kept = q.pin(batch, |_| true).unwrap();
        assert_eq!((kept.ids.len(), kept.lane), (1, None));
        assert_eq!(q.take(1, None), None);
    }

    #[test]
    fn a_serial_batch_waits_for_the_running_one() {
        let q = Arc::new(queue(&[("s1", true), ("s2", true), ("x", false)]));
        let s1 = q.take(0, None).unwrap();
        assert_eq!(q.take(0, None).unwrap().ids[0].as_str(), "x");
        let waiter = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(0, None))
        };
        thread::sleep(ms(50));
        assert!(!waiter.is_finished());
        q.finished(&s1);
        let s2 = waiter.join().unwrap().unwrap();
        assert_eq!(s2.ids[0].as_str(), "s2");
    }

    #[test]
    fn draining_returns_what_was_never_handed_out() {
        let q = queue(&[("a", false), ("b", false), ("c", false)]);
        let _a = q.take(0, None).unwrap();
        q.halt();
        let left: Vec<String> = q.drain().into_iter().map(|id| id.0).collect();
        assert_eq!(left, ["b", "c"]);
        assert_eq!(q.drain().len(), 0);
    }

    #[test]
    fn halting_wakes_waiters_and_stops_hand_out() {
        let q = Arc::new(queue(&[("s1", true), ("s2", true)]));
        let _s1 = q.take(0, None).unwrap();
        let waiter = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(0, None))
        };
        thread::sleep(ms(20));
        q.halt();
        assert_eq!(waiter.join().unwrap(), None);
        assert!(q.is_halted());
    }
}
