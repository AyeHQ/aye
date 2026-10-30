//! How collection reports its progress while it runs ([`Pace`]).

use std::time::{Duration, Instant};

/// When collection reports its progress: not before it has taken half a
/// second, so that a quick collection sends nothing, then at most four times
/// a second.
pub(crate) struct Pace {
    started: Instant,
    last: Option<Instant>,
}

impl Pace {
    const AFTER: Duration = Duration::from_millis(500);
    const EVERY: Duration = Duration::from_millis(250);

    pub(crate) const fn new(started: Instant) -> Self {
        Self {
            started,
            last: None,
        }
    }

    pub(crate) fn due(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.started) < Self::AFTER
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < Self::EVERY)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}
