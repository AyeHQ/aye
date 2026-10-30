//! A comparison's operands as expected and actual, when the expected side
//! is known, else as left and right ([`whole`], [`entries`],
//! [`sources`]).

use std::borrow::Cow;

use aye_events::{Assertion, Difference, Side};

/// The first line of an operand's source (a literal may span lines), `…`
/// marking the cut.
pub(crate) fn first_line(source: &str) -> Cow<'_, str> {
    match source.split_once('\n') {
        Some((first, _)) => Cow::Owned(format!("{}…", first.trim_end())),
        None => Cow::Borrowed(source),
    }
}

/// Whether `at` is written as an access on a value (`['pear']`, `.total`)
/// rather than a place in text (`line 2, column 5`).
pub(crate) fn is_access(at: &str) -> bool {
    at.starts_with(['[', '.'])
}

/// The expected and the actual operand as a whole, when the expected side
/// is known (else left and right).
pub(crate) fn whole(assertion: &Assertion) -> (&str, &str) {
    match assertion.expected {
        Some(Side::Left) => (&assertion.left, &assertion.right),
        _ => (&assertion.right, &assertion.left),
    }
}

/// A difference's entries as (expected, actual), when the expected side is
/// known, else (left, right).
pub(crate) fn entries<'a>(
    assertion: &Assertion,
    difference: &'a Difference,
) -> (Option<&'a str>, Option<&'a str>) {
    let (left, right) = (difference.left.as_deref(), difference.right.as_deref());
    if assertion.expected == Some(Side::Right) {
        (right, left)
    } else {
        (left, right)
    }
}

/// What names each operand without an expected side: the first line of
/// its source, else `left` and `right`.
pub(crate) fn sources(assertion: &Assertion) -> (Cow<'_, str>, Cow<'_, str>) {
    (
        assertion
            .left_source
            .as_deref()
            .map_or(Cow::Borrowed("left"), first_line),
        assertion
            .right_source
            .as_deref()
            .map_or(Cow::Borrowed("right"), first_line),
    )
}
