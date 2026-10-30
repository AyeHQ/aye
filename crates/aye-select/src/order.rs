//! The order a run's cases run in: the arguments', as selection keeps them,
//! or, under `--ff`, what failed last time first ([`failed_first`]).

use std::collections::HashSet;
use std::hash::BuildHasher;

use aye_events::{Case, TestId};

/// Put the cases that failed last time (`last_failed`, by their own ids)
/// first, the others after, each part in the order it came: a stable sort,
/// so the arguments' order holds within each.
pub fn failed_first<S: BuildHasher>(cases: &mut [Case], last_failed: &HashSet<TestId, S>) {
    if !last_failed.is_empty() {
        cases.sort_by_key(|case| !last_failed.contains(&case.id));
    }
}
