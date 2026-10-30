//! Tests a failed lifetime kept from running, and the lifetimes whose
//! failure was shown ([`BlockedTests`]).
//!
//! A test a failed setup kept from running finishes with the lifetime as
//! its `cause`. The engine reports the lifetime's failure after the tests
//! it blocked (except under `maxfail`), so a renderer holds those tests
//! until it comes, and shows them with it; one that comes after it is
//! shown on its own. A lifetime may fail in several batches (a class split
//! across them tears down in each): whether it is shown again is each
//! renderer's rule ([`Again`]). Lifetimes are not paired with their
//! `lifetime_started`: a crashed worker leaves them open, and a test whose
//! failed lifetime never reports is left over at the end.

use std::collections::HashMap;

use aye_events::{Failure, FailureSignature, LifetimeId};

/// Whether a lifetime that failed, and was shown, is shown again when it
/// fails again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Again {
    /// Never: once per lifetime (GitHub annotations, `aye show`'s list).
    Never,
    /// With a signature it wasn't shown with before (text).
    NewSignature,
    /// With a signature other than the one it was last shown with (JSON
    /// lines).
    OtherSignature,
}

/// The order of what is left: tests held for a lifetime that never
/// reported a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Left {
    /// By when the first test each lifetime kept from running came (text).
    Blocked,
    /// By the lifetimes' ids (JSON lines, GitHub annotations).
    Lifetime,
}

/// The tests held for each lifetime that kept them from running, each
/// held as a `T` (its id, its key), and the failures of lifetimes shown.
#[derive(Debug)]
pub struct BlockedTests<T> {
    /// The tests held, by the lifetime that kept them from running, with
    /// when the first of them came.
    held: HashMap<LifetimeId, (u64, Vec<T>)>,
    /// What the next lifetime held for comes as.
    next: u64,
    /// The signatures each lifetime's failures were shown with, in order.
    shown: HashMap<LifetimeId, Vec<Option<FailureSignature>>>,
}

impl<T> Default for BlockedTests<T> {
    fn default() -> Self {
        Self {
            held: HashMap::new(),
            next: 0,
            shown: HashMap::new(),
        }
    }
}

impl<T> BlockedTests<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Holds `test`, which `cause` kept from running, until the lifetime's
    /// failure comes.
    pub fn hold(&mut self, cause: &LifetimeId, test: T) {
        if let Some((_, tests)) = self.held.get_mut(cause) {
            tests.push(test);
        } else {
            self.held.insert(cause.clone(), (self.next, vec![test]));
            self.next += 1;
        }
    }

    /// The tests held for `lifetime`, in the order they came.
    pub fn held(&self, lifetime: &LifetimeId) -> &[T] {
        self.held
            .get(lifetime)
            .map_or(&[], |(_, tests)| tests.as_slice())
    }

    /// The tests held for `lifetime`, in the order they came, no longer
    /// held.
    pub fn take(&mut self, lifetime: &LifetimeId) -> Vec<T> {
        self.held
            .remove(lifetime)
            .map_or_else(Vec::new, |(_, tests)| tests)
    }

    /// Every test held, by lifetime, in the order `left` says, no longer
    /// held.
    pub fn take_left(&mut self, left: Left) -> Vec<(LifetimeId, Vec<T>)> {
        let mut held: Vec<(LifetimeId, (u64, Vec<T>))> = self.held.drain().collect();
        match left {
            Left::Blocked => held.sort_unstable_by_key(|(_, (first, _))| *first),
            Left::Lifetime => held.sort_unstable_by(|a, b| a.0.cmp(&b.0)),
        }
        held.into_iter()
            .map(|(lifetime, (_, tests))| (lifetime, tests))
            .collect()
    }

    /// Every test held, by lifetime, in the order of the lifetimes' ids.
    pub fn by_lifetime(&self) -> Vec<(&LifetimeId, &[T])> {
        let mut held: Vec<(&LifetimeId, &[T])> = self
            .held
            .iter()
            .map(|(lifetime, (_, tests))| (lifetime, tests.as_slice()))
            .collect();
        held.sort_unstable_by_key(|(lifetime, _)| *lifetime);
        held
    }

    /// Whether a failure of `lifetime` was shown.
    pub fn has_failed(&self, lifetime: &LifetimeId) -> bool {
        self.shown.contains_key(lifetime)
    }

    /// `lifetime` failed with `failure`: whether it is shown, as it is the
    /// first of its failures, or as `again` says of one shown before.
    pub fn failed(&mut self, lifetime: &LifetimeId, failure: &Failure, again: Again) -> bool {
        let signature = &failure.signature;
        let shown = self.shown.entry(lifetime.clone()).or_default();
        let fresh = match again {
            _ if shown.is_empty() => true,
            Again::Never => false,
            Again::NewSignature => !shown.contains(signature),
            Again::OtherSignature => shown.last() != Some(signature),
        };
        if fresh {
            shown.push(signature.clone());
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure(signature: &str) -> Failure {
        Failure {
            signature: FailureSignature::new(signature),
            ..Failure::default()
        }
    }

    #[test]
    fn a_lifetime_that_fails_again_is_shown_as_each_renderer_says() {
        let (a, b) = (failure("a"), failure("b"));
        let lifetime = LifetimeId::from("class:t.py::T");
        let shown = |again: Again| {
            let mut blocked = BlockedTests::<()>::new();
            [&a, &b, &a, &a]
                .into_iter()
                .map(|failure| blocked.failed(&lifetime, failure, again))
                .collect::<Vec<bool>>()
        };
        assert_eq!(shown(Again::Never), [true, false, false, false]);
        assert_eq!(shown(Again::NewSignature), [true, true, false, false]);
        assert_eq!(shown(Again::OtherSignature), [true, true, true, false]);
    }

    #[test]
    fn tests_are_held_until_their_lifetime_reports() {
        let (b, a) = (
            LifetimeId::from("class:t.py::B"),
            LifetimeId::from("class:t.py::A"),
        );
        let mut blocked = BlockedTests::new();
        blocked.hold(&b, 1);
        blocked.hold(&a, 2);
        blocked.hold(&b, 3);
        assert_eq!(blocked.held(&b), [1, 3]);
        assert!(!blocked.has_failed(&b));
        assert!(blocked.failed(&b, &Failure::default(), Again::Never));
        assert!(blocked.has_failed(&b));
        assert_eq!(blocked.take(&b), [1, 3]);
        assert!(blocked.take(&b).is_empty());
        // Held again after it was taken: it comes after the others.
        blocked.hold(&b, 4);
        assert_eq!(blocked.by_lifetime(), [(&a, &[2][..]), (&b, &[4][..])]);
        let mut left = BlockedTests::new();
        left.hold(&b, 1);
        left.hold(&a, 2);
        assert_eq!(
            left.take_left(Left::Blocked),
            [(b.clone(), vec![1]), (a.clone(), vec![2])]
        );
        left.hold(&b, 1);
        left.hold(&a, 2);
        assert_eq!(left.take_left(Left::Lifetime), [(a, vec![2]), (b, vec![1])]);
    }
}
