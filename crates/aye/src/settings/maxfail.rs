//! Where a run stops ([`resolve`]).

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
