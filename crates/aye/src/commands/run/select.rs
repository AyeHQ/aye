//! What a run selects, beside what static selection keeps: the words of a
//! `not-found` for a selector that names nothing ([`not_found_message`]),
//! and which `-k` and `-m` count for each adapter's cases ([`Pick`]).

use aye_adapter::FrameworkDefaults;
use aye_events::AdapterName;

use crate::framework_args::Passed;

/// pytest's `not found:`, with a word for those who expected a command.
pub(crate) fn not_found_message(selector: &str) -> String {
    if matches!(selector, "run" | "test" | "tests") && !selector.contains('/') {
        format!(
            "not found: {selector} (aye has no `{selector}` command: `aye` alone runs the tests)"
        )
    } else if matches!(selector, "show" | "config") {
        format!(
            "not found: {selector} (for `aye {selector}`, put `{selector}` first, before any \
             option: `aye {selector} …`)"
        )
    } else {
        format!("not found: {selector}")
    }
}

/// The `-k` or `-m` expressions (each adapter's `own`) static selection
/// applies, by adapter, when the arguments after `--` may hold one aye can't
/// read ([`Pick::Unknown`]): the command line's (`given`), else the
/// adapter's own. No adapter applies an expression at runtime (aye's
/// pytest front-end lists every test, and nothing re-reads those
/// arguments' `-k` or `-m` in a batch), so static selection is the last
/// word on every case.
pub(crate) fn unread<'d>(
    given: Option<&str>,
    defaults: &'d [(AdapterName, FrameworkDefaults)],
    own: impl Fn(&FrameworkDefaults) -> Option<String>,
) -> Vec<(&'d AdapterName, String)> {
    defaults
        .iter()
        .filter_map(|(name, defaults)| {
            let expr = given.map(str::to_string).or_else(|| own(defaults))?;
            Some((name, expr))
        })
        .collect()
}

/// Where a `-k` or `-m` expression for static selection comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pick {
    /// One for every case: the arguments after `--` give it (the framework
    /// sees them last), else the command line (an empty one turns
    /// selection off, the framework's own included, as in pytest).
    Given(String),
    /// The arguments after `--` may hold one aye can't read: every
    /// adapter's cases get the command line's, else their own ([`unread`]).
    Unknown,
    /// Neither gives one: each adapter's own (pytest's from `addopts` or
    /// `PYTEST_ADDOPTS`, in its context) for its own cases.
    Own,
}

/// See [`Pick`].
pub(crate) fn pick(given: Option<&str>, framework: Passed) -> Pick {
    match framework {
        Passed::Set(expr) => Pick::Given(expr),
        Passed::Unknown => Pick::Unknown,
        Passed::Unset => given.map_or(Pick::Own, |expr| Pick::Given(expr.to_string())),
    }
}
