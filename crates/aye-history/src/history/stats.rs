//! What history knows of one test ([`TestStats`]): its durations, its
//! last outcome and the run it came in, and its recent outcomes.

use std::time::Duration;

use aye_events::{Outcome, OutcomeTrail, RunId, RunNumber};
use serde::{Deserialize, Serialize};

/// The weight of the newest duration in the moving average.
const ALPHA: f64 = 0.3;

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
    pub last_run: Option<RunId>,
    /// Reserved for flake rates; 0 until they land.
    #[serde(default)]
    pub flips: u32,
    /// The test's outcomes in the runs that recorded it, a letter per run,
    /// oldest first, the last [`OutcomeTrail::MAX`].
    #[serde(default, skip_serializing_if = "OutcomeTrail::is_empty")]
    pub recent: OutcomeTrail,
    /// The number of the run in which the test's current streak of bad
    /// outcomes began; `None` when its last outcome isn't bad, or when that
    /// run had no number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bad_since: Option<RunNumber>,
}

impl TestStats {
    pub(super) fn new(outcome: Outcome) -> Self {
        Self {
            ewma_ms: 0.0,
            max_ms: 0.0,
            count: 0,
            last_ms: 0.0,
            last_outcome: outcome,
            last_run: None,
            flips: 0,
            recent: OutcomeTrail::default(),
            bad_since: None,
        }
    }

    pub(super) fn add(&mut self, ms: f64) {
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

    pub(super) fn is_valid(&self) -> bool {
        [self.ewma_ms, self.max_ms, self.last_ms]
            .iter()
            .all(|ms| ms.is_finite() && *ms >= 0.0)
    }
}

/// Milliseconds as a duration, saturating.
pub(super) fn duration(ms: f64) -> Duration {
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
