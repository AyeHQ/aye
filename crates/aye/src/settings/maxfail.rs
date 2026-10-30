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
use aye_pytest::argv::{self, ArgValue, passed_maxfail};

use super::first;

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
    let asked = match passed_maxfail(framework_args) {
        ArgValue::Set(limit) => Maxfail::from(limit),
        // What they can't say (an `@file`) leaves the command line's.
        ArgValue::Unset | ArgValue::Unreadable => Maxfail::given(exitfirst, maxfail),
    };
    match asked {
        Maxfail::After(n) => Some(n),
        Maxfail::Unlimited => None,
        Maxfail::Unset => first(defaults, |d| d.maxfail).map(MaxFail::get),
    }
}

/// What the command line says about `maxfail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Maxfail {
    /// Neither `-x` nor `--maxfail`.
    Unset,
    /// `--maxfail 0`.
    Unlimited,
    /// Stop after this many bad outcomes.
    After(u32),
}

impl Maxfail {
    /// What aye's own `-x` and `--maxfail N` ask for: the last given, as
    /// clap keeps it.
    pub(super) const fn given(exitfirst: bool, maxfail: Option<u32>) -> Self {
        match (exitfirst, maxfail) {
            (true, _) => Self::After(1),
            (false, None) => Self::Unset,
            (false, Some(0)) => Self::Unlimited,
            (false, Some(n)) => Self::After(n),
        }
    }
}

impl From<argv::MaxFail> for Maxfail {
    fn from(limit: argv::MaxFail) -> Self {
        match limit {
            argv::MaxFail::Unlimited => Self::Unlimited,
            argv::MaxFail::After(n) => Self::After(n),
        }
    }
}
