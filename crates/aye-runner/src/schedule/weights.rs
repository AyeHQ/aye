//! How long a test is expected to take: what history says, by id.

use std::collections::HashMap;
use std::time::Duration;

use aye_events::TestId;

/// Assumed duration of a test with no history.
pub const DEFAULT_WEIGHT: Duration = Duration::from_millis(10);

/// Historical durations, looked up by id. A case collected without its
/// parameters (`t.py::test_x`) weighs what its parametrized tests weighed.
pub(super) struct Weights<'a> {
    exact: &'a HashMap<TestId, Duration>,
    unparametrized: HashMap<&'a str, Duration>,
}

impl<'a> Weights<'a> {
    pub(super) fn new(exact: &'a HashMap<TestId, Duration>) -> Self {
        let mut unparametrized: HashMap<&str, Duration> = HashMap::new();
        #[expect(clippy::iter_over_hash_type, reason = "sums, in any order")]
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

    pub(super) fn of(&self, id: &TestId) -> Duration {
        self.exact
            .get(id)
            .or_else(|| self.unparametrized.get(id.as_str()))
            .copied()
            .unwrap_or(DEFAULT_WEIGHT)
    }
}
