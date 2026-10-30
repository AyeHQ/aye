//! The order a run's cases run in ([`Order`]): the arguments', as
//! selection keeps them, or, under `--ff`, what failed last time first.

use std::collections::HashSet;

use aye_events::{Case, TestId};

use crate::LastFailed;

/// The order a run's cases run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order<'a> {
    /// The arguments', as selection keeps them: pytest's collection order.
    Arguments,
    /// `--ff`: the cases that failed last time (by their own ids) first,
    /// the others after, each part in the arguments' order.
    FailedFirst(&'a LastFailed),
}

impl Order<'_> {
    /// Put `cases`, in the arguments' order, in this one: a stable sort.
    pub fn sort(self, cases: &mut [Case]) {
        if let Self::FailedFirst(failed) = self
            && !failed.is_empty()
        {
            cases.sort_by_key(|case| !failed.contains(&case.id));
        }
    }

    /// The tests the runner runs first, within their batches and in the
    /// batches it hands out first: what failed last time, under `--ff`.
    pub fn first(self) -> HashSet<TestId> {
        match self {
            Self::Arguments => HashSet::new(),
            Self::FailedFirst(failed) => failed.ids(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(cases: &[Case]) -> Vec<&str> {
        cases.iter().map(|case| case.id.as_str()).collect()
    }

    #[test]
    fn failed_first_moves_what_failed_last_and_keeps_each_parts_order() {
        let mut cases: Vec<Case> = ["b.py::t1", "a.py::t1", "b.py::t2", "a.py::t2", "c.txt"]
            .into_iter()
            .map(|id| Case::new(id, "pytest"))
            .collect();
        let last: LastFailed = ["a.py::t2", "b.py::t2", "c.txt", "gone.py::t"]
            .into_iter()
            .map(TestId::new)
            .collect();
        Order::Arguments.sort(&mut cases);
        assert_eq!(
            ids(&cases),
            ["b.py::t1", "a.py::t1", "b.py::t2", "a.py::t2", "c.txt"]
        );
        assert!(Order::Arguments.first().is_empty());
        let order = Order::FailedFirst(&last);
        order.sort(&mut cases);
        assert_eq!(
            ids(&cases),
            ["b.py::t2", "a.py::t2", "c.txt", "b.py::t1", "a.py::t1"]
        );
        let mut first: Vec<String> = order.first().into_iter().map(TestId::into_string).collect();
        first.sort();
        assert_eq!(first, ["a.py::t2", "b.py::t2", "c.txt", "gone.py::t"]);
    }
}
