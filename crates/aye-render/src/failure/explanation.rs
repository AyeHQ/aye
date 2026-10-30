//! A comparison's explanation, the framework's lines ([`explanation`],
//! [`diff_lines`]), and whether its operands add to what the message and
//! the explanation say ([`operands_add`], [`only_restates`]).

use aye_events::{Assertion, Failure};

/// The comparison's explanation as worth showing: [`diff_lines`], without
/// a lone hunk header (`@@ -1 +1 @@`, a unified diff's line numbers, which
/// the failure's location gives).
pub(crate) fn explanation(assertion: &Assertion) -> Vec<&str> {
    let mut lines = diff_lines(&assertion.diff);
    let hunks = lines.iter().filter(|line| is_hunk_header(line)).count();
    if hunks == 1 && lines.first().is_some_and(|line| is_hunk_header(line)) {
        lines.remove(0);
    }
    lines
}

/// Whether the explanation only restates the operands, each one line
/// (`-3`, `+2`): it shows them one above the other, which helps a reader
/// but tells a program nothing the operands don't.
pub(crate) fn only_restates(assertion: &Assertion, explanation: &[&str]) -> bool {
    let one_line = !assertion.left.contains('\n') && !assertion.right.contains('\n');
    one_line
        && !explanation.is_empty()
        && explanation.iter().all(|line| {
            line.trim().is_empty()
                || line
                    .strip_prefix(['-', '+'])
                    .is_some_and(|value| restates(value, assertion))
        })
}

fn is_hunk_header(line: &str) -> bool {
    line.strip_prefix("@@ ")
        .is_some_and(|rest| rest.contains(" @@"))
}

fn restates(value: &str, assertion: &Assertion) -> bool {
    let operands = [assertion.left.as_str(), assertion.right.as_str()];
    operands.contains(&value) || operands.contains(&value.strip_prefix(' ').unwrap_or(value))
}

/// Whether the comparison's operands say more than the failure's message
/// and its explanation: not when the message holds both as they are
/// written, nor when the explanation shows them (it restates them, or they
/// span lines and it shows how they differ).
pub(crate) fn operands_add(failure: &Failure, assertion: &Assertion) -> bool {
    let explanation = explanation(assertion);
    let multi_line = assertion.left.contains('\n') || assertion.right.contains('\n');
    if multi_line {
        return explanation.is_empty();
    }
    let in_message =
        failure.message.contains(&assertion.left) && failure.message.contains(&assertion.right);
    !(in_message || only_restates(assertion, &explanation))
}

/// The assertion's explanation without pytest's advice about its own `-v`
/// flags, which is addressed to pytest's users, and without blank lines at
/// either end.
pub(crate) fn diff_lines(diff: &[String]) -> Vec<&str> {
    let mut lines: Vec<&str> = diff.iter().filter_map(|line| without_hint(line)).collect();
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines
}

fn without_hint(line: &str) -> Option<&str> {
    let lower = line.to_ascii_lowercase();
    let trimmed = lower.trim_start();
    if trimmed.starts_with("use -v") || trimmed.starts_with("use '-v") {
        return None;
    }
    match [", use -v", ", use '-v"]
        .iter()
        .filter_map(|hint| lower.find(hint))
        .min()
    {
        // ASCII lowercasing keeps byte offsets.
        Some(at) => {
            let kept = line[..at].trim_end();
            (!kept.trim().is_empty()).then_some(kept)
        }
        None => Some(line),
    }
}
