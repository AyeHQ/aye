//! The batches still to run, handed out to lanes as they ask.

use std::collections::{HashSet, VecDeque};

use aye_events::{AdapterName, BatchIndex, TestId, WorkerIndex};

use crate::sync::lock;

use super::batch::{Assigned, Pending, Planned};

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
    /// How many batches were given an index.
    indexed: u32,
    serial_running: bool,
    halted: bool,
    /// Lanes other than the first that may still pin batches to it
    /// ([`Queue::pin`]): until none is left, the first lane waits for work
    /// rather than finishing.
    pinning: HashSet<WorkerIndex>,
    /// The first lane is done taking batches: nothing is pinned to it any
    /// more.
    first_done: bool,
}

impl State {
    /// The index of the next batch handed out for the first time.
    fn next_index(&mut self) -> BatchIndex {
        self.indexed += 1;
        BatchIndex::from_u32(self.indexed - 1)
    }
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
    pub(crate) fn take(&self, lane: WorkerIndex, prefer: Option<&AdapterName>) -> Option<Assigned> {
        let mut state = lock(&self.state);
        loop {
            if state.halted {
                state.first_done |= lane.is_first();
                return None;
            }
            let blocked = state.serial_running;
            let chosen = state
                .pending
                .iter()
                .enumerate()
                .filter(|(_, p)| p.eligible(lane) && !(p.serial && blocked))
                .min_by_key(|(at, p)| (!p.priority, Some(&p.adapter) != prefer, *at))
                .map(|(at, _)| at);
            if let Some(at) = chosen {
                let pending = state.pending.remove(at).expect("position is in range");
                let id = pending.id.unwrap_or_else(|| state.next_index());
                state.serial_running |= pending.serial;
                let ends_session = (!lane.is_first() || state.pinning.is_empty())
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
                || (lane.is_first() && !state.pinning.is_empty());
            if !more {
                // The first lane finishes: nothing can be pinned to it now.
                state.first_done |= lane.is_first();
                return None;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// A batch of `adapter` with no ids that only ends its worker's session
    /// ([`aye_protocol::Batch::ends_session`]): for a worker whose last batch
    /// didn't, as its lane finishes or leaves the adapter.
    pub(crate) fn closing(&self, adapter: &AdapterName) -> Assigned {
        let mut state = lock(&self.state);
        Assigned {
            id: state.next_index(),
            adapter: adapter.clone(),
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
        lock(&self.state).pinning = (1..lanes).map(WorkerIndex::from_u32).collect();
    }

    /// Whether `lane` (not the first) may pin batches from now on: its new
    /// worker lists some files otherwise than the first lane, or none.
    pub(crate) fn may_pin(&self, lane: WorkerIndex, may: bool) {
        if lane.is_first() {
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
    pub(crate) fn lane_done(&self, lane: WorkerIndex) {
        let mut state = lock(&self.state);
        if lane.is_first() {
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
            batch.lane = Some(WorkerIndex::FIRST);
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
            lane: Some(WorkerIndex::FIRST),
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
    use std::time::Duration;

    use crate::schedule::DEFAULT_WEIGHT;

    use super::*;

    fn lane(n: u32) -> WorkerIndex {
        WorkerIndex::from_u32(n)
    }

    fn index(n: u32) -> BatchIndex {
        BatchIndex::from_u32(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
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
    fn queue_of(batches: &[(&'static str, &str, bool, bool)]) -> Queue {
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

    const PYTEST: AdapterName = AdapterName::new_static("pytest");
    const DOCTEST: AdapterName = AdapterName::new_static("doctest");

    fn taken(q: &Queue, prefer: Option<&'static str>) -> String {
        let prefer = prefer.map(AdapterName::from);
        q.take(lane(0), prefer.as_ref()).unwrap().ids[0]
            .as_str()
            .to_owned()
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
        assert_eq!(q.take(lane(0), Some(&PYTEST)), None);
    }

    #[test]
    fn a_lanes_last_batch_of_an_adapter_ends_its_workers_session() {
        let q = queue_of(&[
            ("pytest", "p1", false, false),
            ("doctest", "d1", false, false),
            ("pytest", "p2", false, false),
        ]);
        let p1 = q.take(lane(0), Some(&PYTEST)).unwrap();
        assert!(!p1.ends_session, "p2 is left");
        let p2 = q.take(lane(0), Some(&PYTEST)).unwrap();
        assert!(p2.ends_session);
        // A crashed batch that comes back goes to another worker.
        q.retry(&p2, vec![TestId::new("p2")], Vec::new());
        let d1 = q.take(lane(0), Some(&DOCTEST)).unwrap();
        assert!(d1.ends_session);
        assert!(q.take(lane(0), Some(&PYTEST)).unwrap().ends_session);
        assert_eq!(q.take(lane(0), None), None);

        // The first lane's never does while other lanes may pin batches to
        // it; a batch pinned to it is no other lane's.
        let q = queue_of(&[
            ("pytest", "p1", false, false),
            ("pytest", "p2", false, false),
        ]);
        q.expect_pins(2);
        let p1 = q.take(lane(1), None).unwrap();
        assert!(!p1.ends_session);
        assert_eq!(q.pin(p1, |_| true), None);
        assert!(
            q.take(lane(1), None).unwrap().ends_session,
            "p1 is the first lane's"
        );
        q.lane_done(lane(1));
        assert!(q.take(lane(0), None).unwrap().ends_session);

        // A batch that only ends a session: no ids, an id of its own.
        let closing = q.closing(&PYTEST);
        assert!(closing.ends_session && closing.ids.is_empty());
        assert_eq!((closing.id, closing.adapter.as_str()), (index(2), "pytest"));
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
        let first = q.take(lane(0), None).unwrap();
        q.retry(&first, vec![TestId::new("d1b")], Vec::new());
        assert_eq!(taken(&q, Some("pytest")), "d1b");
    }

    #[test]
    fn hands_out_in_order_with_dense_ids() {
        let q = queue(&[("a", false), ("b", false), ("c", false)]);
        let a = q.take(lane(0), None).unwrap();
        let b = q.take(lane(0), None).unwrap();
        assert_eq!((a.id, a.ids[0].as_str()), (index(0), "a"));
        assert_eq!((b.id, b.ids[0].as_str()), (index(1), "b"));
        q.put_back(b);
        let again = q.take(lane(0), None).unwrap();
        assert_eq!((again.id, again.ids[0].as_str()), (index(1), "b"));
        q.retry(&a, vec![TestId::new("a2")], vec![TestId::new("a1")]);
        let retry = q.take(lane(0), None).unwrap();
        assert_eq!(
            (retry.id, retry.ids[0].as_str(), retry.retry),
            (index(2), "a2", true)
        );
        assert_eq!(retry.ran, [TestId::new("a1")]);
        assert_eq!(q.take(lane(0), None).unwrap().id, index(3));
        assert_eq!(q.take(lane(0), None), None);
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
        let batch = q.take(lane(1), None).unwrap();
        let kept = q.pin(batch, |id| id.path() == "a.py").unwrap();
        assert_eq!(
            (kept.id, kept.ids.clone()),
            (index(0), vec![TestId::new("b.py::t1")])
        );
        assert_eq!(q.take(lane(1), None), None, "pinned to the first lane");
        let pinned = q.take(lane(0), None).unwrap();
        assert_eq!(pinned.id, index(1));
        assert_eq!(pinned.lane, Some(WorkerIndex::FIRST));
        let names: Vec<&str> = pinned.ids.iter().map(TestId::as_str).collect();
        assert_eq!(names, ["a.py::t1", "a.py::t2"]);
        // Its retry stays on the first lane.
        q.retry(&pinned, vec![TestId::new("a.py::t2")], Vec::new());
        assert_eq!(q.take(lane(1), None), None);
        assert_eq!(
            q.take(lane(0), None).unwrap().lane,
            Some(WorkerIndex::FIRST)
        );
    }

    #[test]
    fn a_batch_pinned_whole_goes_back_with_its_id() {
        let q = queue(&[("a.py::t", false), ("b.py::t", false)]);
        let batch = q.take(lane(1), None).unwrap();
        assert_eq!(q.pin(batch, |_| true), None);
        assert_eq!(taken(&q, None), "a.py::t");
        assert_eq!(q.take(lane(1), None).unwrap().ids[0].as_str(), "b.py::t");
        let unpinned = q.take(lane(1), None);
        assert_eq!(unpinned, None);
        let q = queue(&[("a.py::t", false)]);
        let batch = q.take(lane(1), None).unwrap();
        let kept = q.pin(batch, |_| false).unwrap();
        assert_eq!(kept.lane, None);
        assert_eq!(kept.ids.len(), 1);
    }

    #[test]
    fn the_first_lane_waits_while_other_lanes_may_pin() {
        let q = Arc::new(queue(&[("a.py::t", false), ("b.py::t", false)]));
        q.expect_pins(3);
        let other = q.take(lane(1), None).unwrap();
        let _third = q.take(lane(2), None).unwrap();
        let first = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(lane(0), None))
        };
        thread::sleep(ms(50));
        assert!(!first.is_finished());
        assert_eq!(q.pin(other, |_| true), None);
        assert_eq!(first.join().unwrap().unwrap().ids[0].as_str(), "a.py::t");
        let first = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(lane(0), None))
        };
        q.lane_done(lane(1));
        thread::sleep(ms(50));
        assert!(!first.is_finished(), "lane 2 may still pin");
        q.may_pin(lane(2), true);
        thread::sleep(ms(20));
        assert!(!first.is_finished(), "lane 2 may still pin");
        // Its worker lists the session as the first lane's does.
        q.may_pin(lane(2), false);
        assert_eq!(first.join().unwrap(), None);
        // Without pins expected, the first lane finishes as the others do.
        let q = queue(&[]);
        assert_eq!(q.take(lane(0), None), None);
    }

    #[test]
    fn nothing_is_pinned_once_the_first_lane_is_done() {
        let q = queue(&[("a.py::t", false)]);
        q.expect_pins(2);
        let batch = q.take(lane(1), None).unwrap();
        q.lane_done(lane(0));
        let kept = q.pin(batch, |_| true).unwrap();
        assert_eq!((kept.ids.len(), kept.lane), (1, None));
        assert_eq!(q.take(lane(1), None), None);
    }

    #[test]
    fn a_serial_batch_waits_for_the_running_one() {
        let q = Arc::new(queue(&[("s1", true), ("s2", true), ("x", false)]));
        let s1 = q.take(lane(0), None).unwrap();
        assert_eq!(q.take(lane(0), None).unwrap().ids[0].as_str(), "x");
        let waiter = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(lane(0), None))
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
        let _a = q.take(lane(0), None).unwrap();
        q.halt();
        let left: Vec<String> = q.drain().into_iter().map(TestId::into_string).collect();
        assert_eq!(left, ["b", "c"]);
        assert_eq!(q.drain().len(), 0);
    }

    #[test]
    fn halting_wakes_waiters_and_stops_hand_out() {
        let q = Arc::new(queue(&[("s1", true), ("s2", true)]));
        let _s1 = q.take(lane(0), None).unwrap();
        let waiter = {
            let q = Arc::clone(&q);
            thread::spawn(move || q.take(lane(0), None))
        };
        thread::sleep(ms(20));
        q.halt();
        assert_eq!(waiter.join().unwrap(), None);
        assert!(q.is_halted());
    }
}
