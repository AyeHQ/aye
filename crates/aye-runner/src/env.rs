//! The environment variables a lane's workers get.

use std::ffi::OsString;

use aye_events::WorkerIndex;
use aye_static::EnvVars;

/// The environment variables of lane `lane`'s workers among `lanes`:
/// `AYE_WORKER`, and with several lanes pytest-xdist's, so that per-worker
/// resources are reused.
pub(crate) fn lane_env(lane: WorkerIndex, lanes: u32) -> Vec<(OsString, OsString)> {
    let mut env = vec![(EnvVars::AYE_WORKER.into(), lane.to_string().into())];
    if lanes > 1 {
        env.push((
            EnvVars::PYTEST_XDIST_WORKER.into(),
            lane.xdist_name().into(),
        ));
        env.push((
            EnvVars::PYTEST_XDIST_WORKER_COUNT.into(),
            lanes.to_string().into(),
        ));
    }
    env
}
