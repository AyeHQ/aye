//! The one line that says why a bad outcome went bad ([`why`], and
//! [`bare_why`] for one without a failure).

use std::borrow::Cow;

use aye_events::{Assertion, ComparisonOp, Difference, Failure, Phase, Side};

use super::oriented::{first_line, is_access, sources, whole};
use super::{message, resource_name};
use crate::words::head;

/// Characters of a value (an operand, a source, a resource) in [`why`].
const WHY_VALUE: usize = 40;
/// Characters of the whole of [`why`].
pub(crate) const WHY_MAX: usize = 160;

/// The one line that says why a test, a sub-outcome or a lifetime went
/// bad, from its first failure; `test` when it is a test's own (a failed
/// teardown then says the test itself passed). The first that applies:
/// 1. a test's failed teardown with no earlier failure: `passed; fixture
///    <resource> failed in teardown: <message>` (`teardown failed:
///    <message>` without a resource);
/// 2. a failed setup with a resource: `fixture <resource>: <message>`;
/// 3. a comparison with one difference (or none: two numbers, two dates):
///    `<actual source><at>: expected <e>, actual <a>` when the expected
///    side is known (the source left out when longer than 40 characters;
///    a place in text in brackets, `greeting (line 2, column 5)`, or the
///    summary without a source, `strings differ at line 2, column 5`; no
///    place on the first line of texts both shown whole), else `<at>:
///    <left source> is <l>, <right source> is <r>`;
/// 4. a comparison with several differences, or one an operand lacks
///    with nothing to say where (an item of a set): its summary, then the
///    first three places (`dict differs in 2 of 4 keys: ['discount'],
///    ['total']`, `, …` after them when there are more);
/// 5. `==` with the expected side known and no comparison: `expected <e>,
///    actual <a>`;
/// 6. the message, then `; <hint>` when there is one.
///
/// Values are cut to 40 characters, the line to 160; an operand's source
/// to its first line. A resource pytest makes for `setup_module` and its
/// kind (`_xunit_setup_module_fixture_…`) is named for what it runs:
/// `setup_module: <message>`, `passed; teardown_module failed: <message>`.
pub(crate) fn why(failure: &Failure, test: bool) -> String {
    let line = why_line(failure, test);
    head(&line, WHY_MAX).into_owned()
}

fn why_line(failure: &Failure, test: bool) -> String {
    let message = message(failure);
    let resource = failure
        .resource
        .as_deref()
        .map(|resource| resource_name(resource, failure.phase));
    if test && failure.phase == Some(Phase::Teardown) {
        return match resource {
            Some((name, true)) => format!(
                "passed; fixture {} failed in teardown: {message}",
                head(&name, WHY_VALUE)
            ),
            Some((name, false)) => format!("passed; {name} failed: {message}"),
            None => format!("teardown failed: {message}"),
        };
    }
    if failure.phase == Some(Phase::Setup) {
        match resource {
            Some((name, true)) => return format!("fixture {}: {message}", head(&name, WHY_VALUE)),
            Some((name, false)) => return format!("{name}: {message}"),
            None => {}
        }
    }
    if let Some(assertion) = &failure.assertion {
        match &assertion.compare {
            Some(compare) => match compare.differs.as_slice() {
                [one] if one.at.is_some() || (one.left.is_some() && one.right.is_some()) => {
                    return one_difference(assertion, compare, Some(one));
                }
                [] => return one_difference(assertion, compare, None),
                _ if !compare.summary.is_empty() => return several(compare),
                _ => {}
            },
            None if assertion.op == ComparisonOp::Eq && assertion.expected.is_some() => {
                let (expected, actual) = whole(assertion);
                return format!(
                    "expected {}, actual {}",
                    value(Some(expected)),
                    value(Some(actual))
                );
            }
            None => {}
        }
    }
    match &failure.hint {
        Some(hint) => format!("{message}; {hint}"),
        None => message.to_owned(),
    }
}

/// [`why`] of a comparison that differs in several entries: its summary,
/// then where the first three differ (`dict differs in 2 of 4 keys:
/// ['discount'], ['total']`).
fn several(compare: &aye_events::Compare) -> String {
    let ats: Vec<&str> = compare
        .differs
        .iter()
        .filter_map(|difference| difference.at.as_deref())
        .filter(|at| is_access(at))
        .collect();
    if ats.is_empty() {
        return compare.summary.clone();
    }
    let mut named = ats
        .iter()
        .take(3)
        .map(|at| head(at, WHY_VALUE))
        .collect::<Vec<_>>()
        .join(", ");
    if ats.len() > 3 || compare.omitted > 0 {
        named.push_str(", …");
    }
    format!("{}: {named}", compare.summary)
}

/// [`why`] of a comparison that differs in `difference` (`None`: as a
/// whole).
fn one_difference(
    assertion: &Assertion,
    compare: &aye_events::Compare,
    difference: Option<&Difference>,
) -> String {
    let (left, right) = match difference {
        Some(difference) => (difference.left.as_deref(), difference.right.as_deref()),
        None => (
            Some(assertion.left.as_str()),
            Some(assertion.right.as_str()),
        ),
    };
    let at = difference.and_then(|difference| difference.at.as_deref());
    let Some(side) = assertion.expected else {
        let (left_name, right_name) = sources(assertion);
        let at = at.map(|at| format!("{}: ", head(at, WHY_VALUE)));
        return format!(
            "{}{} is {}, {} is {}",
            at.unwrap_or_default(),
            head(&left_name, WHY_VALUE),
            value(left),
            head(&right_name, WHY_VALUE),
            value(right)
        );
    };
    let (expected, actual, source) = match side {
        Side::Left => (left, right, assertion.right_source.as_deref()),
        Side::Right => (right, left, assertion.left_source.as_deref()),
    };
    let source = source
        .map(first_line)
        .filter(|source| source.chars().count() <= WHY_VALUE);
    // Where two texts of one line part says nothing the values shown whole
    // don't.
    let whole = |value: Option<&str>| value.is_some_and(|value| value.chars().count() <= WHY_VALUE);
    let at = at.filter(|at| {
        is_access(at) || !(at.starts_with("line 1,") && whole(expected) && whole(actual))
    });
    let subject = match (source, at) {
        (Some(source), Some(at)) if is_access(at) => {
            format!("{source}{}", head(at, WHY_VALUE))
        }
        (Some(source), Some(at)) => format!("{source} ({})", head(at, WHY_VALUE)),
        (Some(source), None) => source.into_owned(),
        (None, Some(at)) if is_access(at) => head(at, WHY_VALUE).into_owned(),
        // Text: the summary says where it differs.
        (None, Some(at)) if compare.summary.is_empty() => {
            format!("at {}", head(at, WHY_VALUE))
        }
        (None, Some(_)) => head(&compare.summary, WHY_VALUE).into_owned(),
        (None, None) => String::new(),
    };
    let subject = if subject.is_empty() {
        subject
    } else {
        format!("{subject}: ")
    };
    format!(
        "{subject}expected {}, actual {}",
        value(expected),
        value(actual)
    )
}

/// The one line a bad outcome without a failure gets: the last its test
/// wrote to stderr (a crash's), else to stdout.
pub(crate) fn bare_why(output: &aye_events::Captured) -> Option<String> {
    [&output.stderr, &output.stdout]
        .into_iter()
        .find_map(|captured| {
            captured
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .map(|line| head(line.trim(), WHY_MAX).into_owned())
        })
}

/// A compared value in [`why`]: cut to 40 characters, or `missing` for an
/// entry the operand doesn't have.
fn value(value: Option<&str>) -> Cow<'_, str> {
    value.map_or(Cow::Borrowed("missing"), |value| head(value, WHY_VALUE))
}
