//! Where a run stops ([`resolve`]).
//!
//! Decided (implementation, October 2026):
//! - **`maxfail`** is the last `-x` or `--maxfail` given, counted across
//!   workers. One after `--` wins over the command line's, since those
//!   arguments come last in pytest's (`aye --maxfail 3 -- -x` stops at the
//!   first failure, as `pytest --maxfail 3 -x` does). Without any, the
//!   framework's own setting (pytest's `addopts`, `PYTEST_ADDOPTS`) is the
//!   run's.

use aye_adapter::FrameworkDefaults;
use aye_events::AdapterName;
use aye_protocol::MaxFail;

use super::first;
use crate::framework_args::{Maxfail, passed_maxfail};

/// The last of `-x` and `--maxfail` (`exitfirst`, `maxfail`): the arguments
/// after `--` (`framework_args`) come last in the framework's arguments, so
/// theirs wins over the command line's. Else the framework's own setting
/// (pytest's `addopts`, `PYTEST_ADDOPTS`); 0 means no limit.
pub(super) fn resolve(
    framework_args: &[String],
    exitfirst: bool,
    maxfail: Option<u32>,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Option<u32> {
    let given = || Maxfail::given(exitfirst, maxfail);
    match passed_maxfail(framework_args).unwrap_or_else(given) {
        Maxfail::After(n) => Some(n),
        Maxfail::Unlimited => None,
        Maxfail::Unset => first(defaults, |d| d.maxfail).map(MaxFail::get),
    }
}
