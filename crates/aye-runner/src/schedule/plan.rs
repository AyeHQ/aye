//! A plan's cases as batches: grouped, weighed, split and merged, in the
//! order they are handed out.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aye_events::{AdapterName, AffinityKey, Case, TestId};

use super::batch::Planned;
use super::weights::Weights;

/// With several workers, groups lighter than the total over
/// `workers × BATCHES_PER_WORKER` are merged.
const BATCHES_PER_WORKER: u32 = 4;

#[derive(Debug)]
struct Group<'a> {
    adapter: &'a AdapterName,
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
    Affinity(&'a AffinityKey),
    Free(&'a str),
}

/// Split `cases` into batches, in the order they should be handed out.
/// `first` holds the tests to run first; the batches of the adapters in
/// `ordered` keep plan order (see the `schedule` module's docs).
pub(crate) fn plan(
    cases: &[Case],
    durations: &HashMap<TestId, Duration>,
    first: &HashSet<TestId>,
    workers: u32,
    ordered: &HashSet<&AdapterName>,
) -> Vec<Planned> {
    let weigh = Weights::new(durations);
    // A file's free cases go with its first lifetime group, if it has one,
    // so that they keep their place among its other cases.
    let mut lifetimes: HashMap<(&AdapterName, &str), &AffinityKey> = HashMap::new();
    for case in cases {
        if let Some(key) = case.affinity.first() {
            lifetimes
                .entry((&case.adapter, case.id.path().as_str()))
                .or_insert(key);
        }
    }
    let mut index: HashMap<(&AdapterName, Key<'_>), usize> = HashMap::new();
    let mut groups: Vec<Group<'_>> = Vec::new();
    for (position, case) in cases.iter().enumerate() {
        let adapter = &case.adapter;
        let key = match case.affinity.first() {
            Some(key) => Key::Affinity(key),
            None => lifetimes
                .get(&(adapter, case.id.path().as_str()))
                .map_or_else(
                    || Key::Free(case.id.path().as_str()),
                    |key| Key::Affinity(key),
                ),
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
        let mut open: HashMap<(&AdapterName, bool), usize> = HashMap::new();
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
            adapter: batch[0].adapter.clone(),
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
            .filter(|&at| ordered.contains(&batches[at].adapter))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A case whose file is its lifetime, as the adapters but the native
    /// engine declare.
    fn case(id: &str) -> Case {
        let mut case = free(id);
        case.affinity = vec![AffinityKey::file(case.id.path())];
        case
    }

    /// A case that shares no lifetime.
    fn free(id: &str) -> Case {
        Case::new(id, "pytest")
    }

    /// No adapter whose batches keep plan order.
    fn none() -> HashSet<&'static AdapterName> {
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
            case.affinity = vec![AffinityKey::module(case.id.path())];
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

    const PYTEST: AdapterName = AdapterName::new_static("pytest");

    /// pytest's batches keep plan order.
    fn ordered() -> HashSet<&'static AdapterName> {
        static ORDERED: AdapterName = PYTEST;
        HashSet::from([&ORDERED])
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
            case.affinity = vec![AffinityKey::file(case.id.path())];
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
}
