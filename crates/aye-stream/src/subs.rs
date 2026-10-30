//! A test's bad sub-outcomes (failed subtests, doctest examples), held
//! until the test finishes ([`SubOutcomes`]), and which of the test's own
//! failures only repeat theirs ([`Repeats`], [`own_failures`]).
//!
//! A sub-outcome names its test by id, and comes before the test's
//! `test_finished`, which a renderer needs to show the sub-outcome with
//! (the test's rerun, its output), so each renderer holds them until then.
//! A test whose worker died, or that a stop cut short, never finishes: its
//! sub-outcomes are what is left at the end of the stream.

use std::collections::HashMap;
use std::time::Duration;

use aye_events::{Event, Failure, Outcome, Record, TestId, TestResult, WorkerIndex};

/// A bad sub-outcome, until its test finishes.
#[derive(Debug, Clone)]
pub struct HeldSub {
    /// Its name: the subtest's, the example's.
    pub name: String,
    pub outcome: Outcome,
    pub failure: Option<Failure>,
    pub duration: Duration,
    /// The worker that ran it.
    pub worker: Option<WorkerIndex>,
}

/// The bad sub-outcomes of the tests that haven't finished, by test.
#[derive(Debug, Default)]
pub struct SubOutcomes {
    held: HashMap<TestId, Vec<HeldSub>>,
}

impl SubOutcomes {
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds `record` when it is a bad sub-outcome: whether it was.
    pub fn hold(&mut self, record: &Record) -> bool {
        let Event::SubOutcome {
            id,
            name,
            outcome,
            failure,
            duration,
            ..
        } = &record.event
        else {
            return false;
        };
        if !outcome.is_bad() {
            return false;
        }
        self.held.entry(id.clone()).or_default().push(HeldSub {
            name: name.clone(),
            outcome: *outcome,
            failure: failure.clone(),
            duration: *duration,
            worker: record.envelope.worker,
        });
        true
    }

    /// The bad sub-outcomes held for the test `id`, which finished, in the
    /// order they came; none when it had none.
    pub fn take(&mut self, id: &TestId) -> Vec<HeldSub> {
        self.held.remove(id).unwrap_or_default()
    }

    /// The bad sub-outcomes of the tests that never finished, by test, in
    /// the order of their ids.
    pub fn take_left(&mut self) -> Vec<(TestId, Vec<HeldSub>)> {
        let mut left: Vec<(TestId, Vec<HeldSub>)> = self.held.drain().collect();
        left.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        left
    }
}

/// Which of a test's own failures only repeat what its bad sub-outcomes
/// said: the unittest and doctest adapters fail a test with the very
/// failures of its subtests or examples, and the pytest adapter with a bare
/// one ("contains 1 failed subtest"). Each renderer says which it leaves
/// out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeats {
    /// One of theirs again, or a bare failure: text, JSON lines and GitHub
    /// annotations.
    SameOrBare,
    /// One of theirs again only: the JUnit report keeps a bare one, as a
    /// `<failure>` of the test's own.
    Same,
}

impl Repeats {
    /// Whether `failure`, a test's own, repeats what `subs`, its bad
    /// sub-outcomes, said.
    pub fn repeats(self, failure: &Failure, subs: &[HeldSub]) -> bool {
        (self == Self::SameOrBare && is_bare(failure))
            || subs
                .iter()
                .filter_map(|sub| sub.failure.as_ref())
                .any(|sub| same_failure(sub, failure))
    }
}

/// The test's own failures, less those that only repeat `subs`, its bad
/// sub-outcomes, by `rule`; every one of them when it had none.
pub fn own_failures<'a>(
    result: &'a TestResult,
    subs: &'a [HeldSub],
    rule: Repeats,
) -> impl Iterator<Item = &'a Failure> {
    result
        .failures
        .iter()
        .filter(move |failure| subs.is_empty() || !rule.repeats(failure, subs))
}

/// A failure that says nothing but its message: no exception, place,
/// frames or comparison, and no framework text beyond the message. The
/// engine makes one for a test that failed only through its sub-outcomes.
pub fn is_bare(failure: &Failure) -> bool {
    failure.exception.is_none()
        && failure.location.is_none()
        && failure.frames.is_empty()
        && failure.assertion.is_none()
        && failure
            .detail
            .as_deref()
            .is_none_or(|detail| detail.trim() == failure.message.trim())
}

/// The same failure, reported twice: equal, or with the same signature and
/// message.
fn same_failure(a: &Failure, b: &Failure) -> bool {
    a == b || (a.signature.is_some() && a.signature == b.signature && a.message == b.message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str, name: &str, outcome: Outcome, message: &str) -> Record {
        Record::new(Event::SubOutcome {
            id: TestId::new(id),
            name: name.into(),
            outcome,
            reason: None,
            failure: Some(Failure {
                message: message.into(),
                exception: Some("AssertionError".into()),
                ..Failure::default()
            }),
            duration: Duration::ZERO,
        })
    }

    #[test]
    fn bad_sub_outcomes_are_held_until_their_test_finishes() {
        let mut subs = SubOutcomes::new();
        assert!(subs.hold(&sub("t.py::test_b", "x=1", Outcome::Failed, "one")));
        assert!(!subs.hold(&sub("t.py::test_b", "x=2", Outcome::Passed, "")));
        assert!(subs.hold(&sub("t.py::test_b", "x=3", Outcome::Error, "three")));
        assert!(subs.hold(&sub("t.py::test_c", "y", Outcome::Failed, "c")));
        assert!(subs.hold(&sub("t.py::test_a", "z", Outcome::Failed, "a")));
        let names = |held: &[HeldSub]| held.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(&subs.take(&TestId::new("t.py::test_b"))),
            ["x=1", "x=3"]
        );
        assert_eq!(
            names(&subs.take(&TestId::new("t.py::test_b"))),
            [] as [String; 0]
        );
        // What is left, by id.
        let left = |subs: &mut SubOutcomes| {
            subs.take_left()
                .into_iter()
                .map(|(id, _)| id.into_string())
                .collect::<Vec<String>>()
        };
        assert_eq!(left(&mut subs), ["t.py::test_a", "t.py::test_c"]);
        assert_eq!(left(&mut subs), [] as [String; 0]);
    }

    #[test]
    fn each_renderer_says_which_repeats_it_leaves_out() {
        let failure = |message: &str| Failure {
            message: message.into(),
            exception: Some("AssertionError".into()),
            ..Failure::default()
        };
        let bare = Failure {
            message: "contains 1 failed subtest".into(),
            ..Failure::default()
        };
        let held = |failure: Failure| HeldSub {
            name: "x".into(),
            outcome: Outcome::Failed,
            failure: Some(failure),
            duration: Duration::ZERO,
            worker: None,
        };
        let mut result = TestResult::new(TestId::new("t.py::test_x"), Outcome::Failed);
        result.failures = vec![failure("one"), bare.clone(), failure("own")];
        let subs = [held(failure("one"))];
        let own = |rule| {
            own_failures(&result, &subs, rule)
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(own(Repeats::SameOrBare), ["own"]);
        assert_eq!(own(Repeats::Same), ["contains 1 failed subtest", "own"]);
        // Without sub-outcomes, a test's failures are its own, bare or not.
        result.failures = vec![bare];
        assert_eq!(own_failures(&result, &[], Repeats::SameOrBare).count(), 1);
    }
}
