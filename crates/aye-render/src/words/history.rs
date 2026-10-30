//! A test's earlier outcomes in words ([`history`]).

use aye_events::{Outcome, Previous};

/// A test's earlier outcomes in words: `passed the previous 12 runs`,
/// `failed the previous 3 runs too (since r78)`, `failed 2 of the last 10
/// runs`, `didn't fail in the previous 4 runs`. None without any.
pub(crate) fn history(previous: &Previous) -> Option<String> {
    let outcomes = &previous.outcomes;
    let total = outcomes.len();
    let runs = |n: usize| {
        if n == 1 {
            "run".to_owned()
        } else {
            format!("{n} runs")
        }
    };
    if total == 0 {
        return None;
    }
    if outcomes.iter().all(|outcome| outcome == Outcome::Passed) {
        return Some(format!("passed the previous {}", runs(total)));
    }
    if outcomes.iter().next_back().is_some_and(Outcome::is_bad) {
        let streak = outcomes.iter().rev().take_while(|o| o.is_bad()).count();
        let since = previous
            .since
            .map(|since| format!(" (since {since})"))
            .unwrap_or_default();
        return Some(format!("failed the previous {} too{since}", runs(streak)));
    }
    match outcomes.iter().filter(|outcome| outcome.is_bad()).count() {
        0 => Some(format!("didn't fail in the previous {}", runs(total))),
        failed => Some(format!("failed {failed} of the last {}", runs(total))),
    }
}

#[cfg(test)]
mod tests {
    use aye_events::RunNumber;

    use super::*;

    #[test]
    fn history_in_words() {
        let words = |outcomes: &str, since: Option<u64>| {
            history(&Previous {
                outcomes: outcomes.into(),
                since: since.and_then(RunNumber::new),
            })
        };
        assert_eq!(words("", None), None);
        assert_eq!(words("p", None).unwrap(), "passed the previous run");
        assert_eq!(words("pppp", None).unwrap(), "passed the previous 4 runs");
        assert_eq!(
            words("ppfef", Some(78)).unwrap(),
            "failed the previous 3 runs too (since r78)"
        );
        assert_eq!(words("pf", None).unwrap(), "failed the previous run too");
        assert_eq!(
            words("pfppfp", None).unwrap(),
            "failed 2 of the last 6 runs"
        );
        assert_eq!(
            words("psxp", None).unwrap(),
            "didn't fail in the previous 4 runs"
        );
    }
}
