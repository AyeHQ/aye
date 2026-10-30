//! How many workers a run starts ([`resolve`]).

use aye_adapter::FrameworkDefaults;
use aye_config::{AyeSettings, WorkerCount};
use aye_events::AdapterName;

use super::first;

/// Worker processes: `-n` (`flag`), else `[tool.aye] workers`, else the
/// first adapter's framework that gives a count (pytest-xdist's `-n` in
/// pytest's `addopts`), else 1; `auto` is one per CPU, and 0 is 1.
pub(super) fn resolve(
    flag: Option<WorkerCount>,
    tool_aye: &AyeSettings,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> u32 {
    flag.or_else(|| tool_aye.workers.as_ref().map(|workers| workers.value))
        .or_else(|| first(defaults, |d| d.workers))
        .map_or(1, WorkerCount::resolve)
}
