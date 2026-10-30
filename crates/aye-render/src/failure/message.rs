//! A failure's message: its one line ([`message`]), the lines after it
//! that the framework's text holds ([`message_rest`]), and the message the
//! test gave beside a comparison ([`own_message`]). Each reads the
//! adapters' text as they write it.

use aye_events::{Assertion, ComparisonOp, Failure};

/// What a failed comparison's message says beside the comparison: the
/// message the test gave (`assert a == b, "why"`, `assertEqual(a, b,
/// "why")`), not the one the framework made of the operands (`assert …`,
/// unittest's `a != b`, the doctest adapter's `<source>: expected …`).
pub(crate) fn own_message<'a>(failure: &'a Failure, assertion: &Assertion) -> Option<&'a str> {
    let message = failure.message.trim();
    if message.is_empty() || message.starts_with("assert ") {
        return None;
    }
    if assertion
        .left_source
        .as_deref()
        .is_some_and(|source| message.starts_with(&format!("{source}: expected ")))
    {
        return None;
    }
    if assertion.op == ComparisonOp::Eq && message.contains(" != ") {
        return message
            .split_once(" : ")
            .map(|(_, own)| own.trim())
            .filter(|own| !own.is_empty());
    }
    Some(message)
}

/// The failure's one line, never empty.
pub(crate) fn message(failure: &Failure) -> &str {
    if !failure.message.is_empty() {
        &failure.message
    } else if let Some(exception) = &failure.exception {
        exception
    } else {
        "failed"
    }
}

/// The lines of a failure's message after the first, which its one-line
/// `message` leaves out (`self.fail("a\nb")`, an exception whose text spans
/// lines, pytest's `+  where` lines): read from the framework's text,
/// after the last line holding the message, as Python's traceback writes
/// it (`Type: message`, then the rest) and as pytest's does (`E   Type:
/// message`, then more `E` lines). None when a comparison's explanation
/// holds them (its `diff`), nor for an unknown fixture (pytest's text lists
/// every fixture there is; the hint names the close ones).
pub(crate) fn message_rest(failure: &Failure) -> Vec<&str> {
    let explained = failure
        .assertion
        .as_ref()
        .is_some_and(|assertion| !assertion.diff.is_empty())
        || failure.exception.as_deref() == Some("FixtureLookupError");
    let first = failure.message.trim();
    let Some(detail) = failure.detail.as_deref().filter(|_| !explained) else {
        return Vec::new();
    };
    if first.is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = detail.lines().collect();
    let Some((at, marker)) = lines
        .iter()
        .enumerate()
        .rev()
        .find_map(|(at, line)| holds_message(line, first).map(|marker| (at, marker)))
    else {
        return Vec::new();
    };
    let mut rest = Vec::new();
    for line in &lines[at + 1..] {
        match marker {
            Marker::Pytest(width) => {
                let Some(after) = line.strip_prefix('E') else {
                    break;
                };
                if !(after.is_empty() || after.starts_with(char::is_whitespace)) {
                    break;
                }
                rest.push(strip_spaces(after, width));
            }
            Marker::Indent(width) => rest.push(strip_spaces(line, width)),
        }
    }
    while rest.last().is_some_and(|line| line.trim().is_empty()) {
        rest.pop();
    }
    while rest.first().is_some_and(|line| line.trim().is_empty()) {
        rest.remove(0);
    }
    rest
}

/// How the line holding a failure's message is written: after pytest's `E`
/// and this many spaces, or indented by this many.
#[derive(Clone, Copy)]
enum Marker {
    Pytest(usize),
    Indent(usize),
}

/// Whether `line` holds the message `first`: as it is, or after an
/// exception's (dotted) name and `: `.
fn holds_message(line: &str, first: &str) -> Option<Marker> {
    let (marker, content) = match line.strip_prefix('E') {
        Some(after) if after.starts_with(char::is_whitespace) => {
            let content = after.trim_start();
            (Marker::Pytest(after.len() - content.len()), content)
        }
        _ => {
            let content = line.trim_start();
            (Marker::Indent(line.len() - content.len()), content)
        }
    };
    let content = content.trim_end();
    if content == first {
        return Some(marker);
    }
    // `AssertionError: ` before an assertion's message, or the module
    // Python names an exception by (`json.decoder.`) before the
    // `JSONDecodeError: …` the message starts with.
    let before = content.strip_suffix(first)?;
    let name = before
        .strip_suffix(": ")
        .or_else(|| before.ends_with('.').then_some(before))?;
    let is_name = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    is_name.then_some(marker)
}

/// `line` without up to `width` leading spaces.
fn strip_spaces(line: &str, width: usize) -> &str {
    let spaces = line.len() - line.trim_start_matches(' ').len();
    &line[spaces.min(width)..]
}
