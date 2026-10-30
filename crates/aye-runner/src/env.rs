//! The environment a lane's workers get ([`LaneEnv`]).

use std::ffi::OsString;

use aye_events::WorkerIndex;
use aye_static::EnvVars;

/// The environment of a lane's workers: lane `lane` of `lanes`. A worker
/// started ahead of a run (kept from refinement, or pre-listing) serves a
/// lane only if it was given that lane's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaneEnv {
    pub(crate) lane: WorkerIndex,
    pub(crate) lanes: u32,
}

impl LaneEnv {
    /// The first lane's, among `lanes`.
    pub(crate) fn first(lanes: u32) -> Self {
        Self {
            lane: WorkerIndex::FIRST,
            lanes,
        }
    }

    /// The variables: `AYE_WORKER`, and with several lanes pytest-xdist's,
    /// so that per-worker resources are reused.
    pub(crate) fn vars(self) -> Vec<(OsString, OsString)> {
        let mut env = vec![(EnvVars::AYE_WORKER.into(), self.lane.to_string().into())];
        if self.lanes > 1 {
            env.push((
                EnvVars::PYTEST_XDIST_WORKER.into(),
                self.lane.xdist_name().into(),
            ));
            env.push((
                EnvVars::PYTEST_XDIST_WORKER_COUNT.into(),
                self.lanes.to_string().into(),
            ));
        }
        env
    }
}
