//! Text shared by the renderers: bounds, tails, durations, frames, the
//! rerun command, and what text and JSON lines both say of a bad outcome
//! (its one line, [`why`]; its history; its frames, grouped).

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use aye_events::{
    Assertion, Diagnostic, Difference, Event, Failure, Frame, FramePath, LifetimeId, Local,
    Outcome, Phase, Previous, Record, Reproduce, Resource, Side, TestId,
};

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
            None if assertion.op == "==" && assertion.expected.is_some() => {
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
    if assertion.op == "==" && message.contains(" != ") {
        return message
            .split_once(" : ")
            .map(|(_, own)| own.trim())
            .filter(|own| !own.is_empty());
    }
    Some(message)
}

/// The first line of an operand's source (a literal may span lines), `…`
/// marking the cut.
pub(crate) fn first_line(source: &str) -> Cow<'_, str> {
    match source.split_once('\n') {
        Some((first, _)) => Cow::Owned(format!("{}…", first.trim_end())),
        None => Cow::Borrowed(source),
    }
}

/// What to call a resource that failed in `phase`, and whether it is a
/// fixture the project wrote: pytest's own for `setup_module` and its
/// kind (`_xunit_setup_module_fixture_<module>`) is called for what it
/// runs, `setup_module` or `teardown_module`.
pub(crate) fn resource_name(resource: &str, phase: Option<Phase>) -> (String, bool) {
    let Some(rest) = resource.strip_prefix("_xunit_setup_") else {
        return (resource.to_owned(), true);
    };
    let kind = rest.split_once("_fixture").map_or(rest, |(kind, _)| kind);
    let verb = if phase == Some(Phase::Teardown) {
        "teardown"
    } else {
        "setup"
    };
    (format!("{verb}_{kind}"), false)
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

/// Whether `at` is written as an access on a value (`['pear']`, `.total`)
/// rather than a place in text (`line 2, column 5`).
pub(crate) fn is_access(at: &str) -> bool {
    at.starts_with(['[', '.'])
}

/// A compared value in [`why`]: cut to 40 characters, or `missing` for an
/// entry the operand doesn't have.
fn value(value: Option<&str>) -> Cow<'_, str> {
    value.map_or(Cow::Borrowed("missing"), |value| head(value, WHY_VALUE))
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

/// A test's earlier outcomes in words: `passed the previous 12 runs`,
/// `failed the previous 3 runs too (since r78)`, `failed 2 of the last 10
/// runs`, `didn't fail in the previous 4 runs`. None without any.
pub(crate) fn history(previous: &Previous) -> Option<String> {
    let letters: Vec<char> = previous.outcomes.chars().collect();
    let total = letters.len();
    let runs = |n: usize| {
        if n == 1 {
            "run".to_owned()
        } else {
            format!("{n} runs")
        }
    };
    let bad = |letter: &char| matches!(letter, 'f' | 'e');
    if total == 0 {
        return None;
    }
    if letters.iter().all(|letter| *letter == 'p') {
        return Some(format!("passed the previous {}", runs(total)));
    }
    if letters.last().is_some_and(bad) {
        let streak = letters
            .iter()
            .rev()
            .take_while(|letter| bad(letter))
            .count();
        let since = previous
            .since
            .map(|since| format!(" (since {since})"))
            .unwrap_or_default();
        return Some(format!("failed the previous {} too{since}", runs(streak)));
    }
    match letters.iter().filter(|letter| bad(letter)).count() {
        0 => Some(format!("didn't fail in the previous {}", runs(total))),
        failed => Some(format!("failed {failed} of the last {}", runs(total))),
    }
}

/// How many of the earlier outcomes passed and how many failed (or
/// errored).
pub(crate) fn history_counts(previous: &Previous) -> (u32, u32) {
    previous
        .outcomes
        .chars()
        .fold((0, 0), |(passed, failed), letter| match letter {
            'p' => (passed + 1, failed),
            'f' | 'e' => (passed, failed + 1),
            _ => (passed, failed),
        })
}

/// The setup chain: `price_list (session) → cart (function)`.
pub(crate) fn setup_chain(resources: &[Resource]) -> String {
    resources
        .iter()
        .map(|resource| {
            let (name, _) = resource_name(&resource.name, None);
            format!("{name} ({})", resource.scope)
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

/// A failure's frames as L3 lists them: the project's each, and each run
/// of other frames as one entry per package.
pub(crate) enum FrameItem<'a> {
    Project(&'a Frame),
    /// `count` frames in a row outside the project, in `package`.
    Others {
        count: usize,
        package: String,
    },
}

pub(crate) fn frame_items(frames: &[Frame]) -> Vec<FrameItem<'_>> {
    let mut items: Vec<FrameItem<'_>> = Vec::new();
    for frame in frames {
        if frame.path.is_project() {
            items.push(FrameItem::Project(frame));
            continue;
        }
        let package = package_of(frame.path.as_str());
        match items.last_mut() {
            Some(FrameItem::Others {
                count,
                package: last,
            }) if *last == package => *count += 1,
            _ => items.push(FrameItem::Others { count: 1, package }),
        }
    }
    items
}

/// `12 frames in requests`.
pub(crate) fn others(count: usize, package: &str) -> String {
    format!("{} in {package}", plural(count, "frame", "frames"))
}

/// The package a frame outside the project belongs to: the first part
/// after `site-packages` (or `dist-packages`), the standard library's
/// module after `pythonX.Y` (or `Lib`), else the file's directory.
pub(crate) fn package_of(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with('<') {
        return path.trim_matches(['<', '>']).to_owned();
    }
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let after = parts.iter().rposition(|part| {
        matches!(*part, "site-packages" | "dist-packages" | "Lib")
            || (part.starts_with("python") && part[6..].starts_with(|c: char| c.is_ascii_digit()))
    });
    let part = match after {
        Some(at) if at + 1 < parts.len() => parts[at + 1],
        _ if parts.len() >= 2 => parts[parts.len() - 2],
        _ => parts.last().copied().unwrap_or_default(),
    };
    part.strip_suffix(".py").unwrap_or(part).to_owned()
}

/// The frame a failure is shown at ([`origin`]), with its source.
pub(crate) fn pointed(failure: &Failure) -> Option<&Frame> {
    let (path, line) = origin(failure)?;
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| frame.path == *path && frame.line == line)
}

/// The local variables a failure's innermost frame had.
pub(crate) fn locals(failure: &Failure) -> &[Local] {
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| !frame.locals.is_empty())
        .map_or(&[], |frame| frame.locals.as_slice())
}

/// An outcome as words: `passed`, `expected failure`.
pub(crate) const fn outcome_words(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Passed => "passed",
        Outcome::Failed => "failed",
        Outcome::Error => "error",
        Outcome::Skipped => "skipped",
        Outcome::ExpectedFailure => "expected failure",
        Outcome::UnexpectedPass => "unexpected pass",
    }
}

/// Where a failure happened, in words: `setup`, `the test`, `teardown`.
pub(crate) fn phase_words(failure: Option<&Failure>) -> Option<&'static str> {
    failure.map(|failure| match failure.phase {
        Some(Phase::Setup) => "setup",
        Some(Phase::Teardown) => "teardown",
        Some(Phase::Test) | None => "the test",
    })
}

/// The Python version a `worker_started` names: its `version`'s first
/// word (`3.13.1`), else its `python` when that is a version itself.
pub(crate) fn python_version(python: &str, version: Option<&str>) -> Option<String> {
    match version.and_then(|version| version.split_whitespace().next()) {
        Some(version) => Some(version.to_owned()),
        None => python
            .starts_with(|c: char| c.is_ascii_digit())
            .then(|| python.to_owned()),
    }
}

/// `2 ms` below a second, then as [`seconds`].
pub(crate) fn spaced(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{} ms", duration.as_millis())
    } else {
        seconds(duration)
    }
}

/// `0.12 s` below a minute, then `1 min 5 s`.
pub(crate) fn seconds(duration: Duration) -> String {
    if duration < Duration::from_secs(60) {
        format!("{:.2} s", duration.as_secs_f64())
    } else {
        let whole = duration.as_secs();
        format!("{} min {} s", whole / 60, whole % 60)
    }
}

/// Seconds with two decimals, as a number.
pub(crate) fn secs(duration: Duration) -> f64 {
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations are far below 2^52 ms"
    )]
    let ms = millis(duration) as f64;
    (ms / 10.0).round() / 100.0
}

/// At most `max` characters of `text`; a cut ends in `…`.
pub(crate) fn head(text: &str, max: usize) -> Cow<'_, str> {
    if text.chars().nth(max).is_none() {
        return Cow::Borrowed(text);
    }
    let end = text
        .char_indices()
        .nth(max.saturating_sub(1))
        .map_or(text.len(), |(at, _)| at);
    Cow::Owned(format!("{}…", &text[..end]))
}

/// The last `max` characters of `text`; a cut starts with `…`.
pub(crate) fn tail(text: &str, max: usize) -> Cow<'_, str> {
    let count = text.chars().count();
    if count <= max {
        return Cow::Borrowed(text);
    }
    let start = text
        .char_indices()
        .nth(count - max.saturating_sub(1))
        .map_or(text.len(), |(at, _)| at);
    Cow::Owned(format!("…{}", &text[start..]))
}

/// The end of some captured text.
pub(crate) struct Lines<'a> {
    pub lines: Vec<&'a str>,
    /// How many lines the text has.
    pub total: usize,
}

/// The last `n` lines of `text`, trailing blank lines dropped.
pub(crate) fn last_lines(text: &str, n: usize) -> Lines<'_> {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let total = all.len();
    Lines {
        lines: all[total.saturating_sub(n)..].to_vec(),
        total,
    }
}

/// Whole milliseconds.
pub(crate) fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(crate) fn plural<N: Copy + std::fmt::Display + PartialEq + From<u8>>(
    n: N,
    one: &str,
    many: &str,
) -> String {
    let word = if n == N::from(1) { one } else { many };
    format!("{n} {word}")
}

pub(crate) const fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Setup => "setup",
        Phase::Test => "test",
        Phase::Teardown => "teardown",
    }
}

/// The phase worth naming: setup or teardown, not the test itself.
pub(crate) fn notable_phase(failure: &Failure) -> Option<&'static str> {
    failure
        .phase
        .filter(|phase| *phase != Phase::Test)
        .map(phase)
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

/// A failure that says nothing but its message: no exception, place,
/// frames or comparison, and no framework text beyond the message. The
/// engine makes one for a test that failed only through its sub-outcomes.
pub(crate) fn is_bare(failure: &Failure) -> bool {
    failure.exception.is_none()
        && failure.location.is_none()
        && failure.frames.is_empty()
        && failure.assertion.is_none()
        && failure
            .detail
            .as_deref()
            .is_none_or(|detail| detail.trim() == failure.message.trim())
}

/// The same failure, reported twice: equal, or with the same signature and
/// message. The unittest and doctest adapters fail a test with the very
/// failures its subtests or examples reported.
pub(crate) fn same_failure(a: &Failure, b: &Failure) -> bool {
    a == b || (!a.signature.is_empty() && a.signature == b.signature && a.message == b.message)
}

/// Whether a test's own failure only repeats what its bad sub-outcomes
/// (`subs`, their failures) already said: one of theirs again, or a bare
/// one (the pytest adapter's "contains 1 failed subtest").
pub(crate) fn repeats_subs(failure: &Failure, subs: &[Failure]) -> bool {
    is_bare(failure) || subs.iter().any(|sub| same_failure(sub, failure))
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

/// Modules the framework skipped while collecting them (pytest's
/// `skip(..., allow_module_level=True)` and `importorskip`): collection
/// reports each as a `skipped` info diagnostic, outside any batch, and no
/// test of the module reaches the run. pytest counts each as one skipped
/// item in its summary and its JUnit report, so the renderers do too;
/// `run_finished`'s counts hold test results only. A collect-only run counts
/// none, as `pytest --co` doesn't.
#[derive(Debug, Default)]
pub(crate) struct ModuleSkips {
    paths: std::collections::HashSet<String>,
    collect_only: bool,
}

impl ModuleSkips {
    /// The diagnostic, when `record` reports a module skipped and that
    /// module wasn't counted yet.
    pub(crate) fn note<'a>(&mut self, record: &'a Record) -> Option<&'a Diagnostic> {
        if let Event::RunStarted { collect_only, .. } = &record.event {
            self.collect_only = *collect_only;
            return None;
        }
        let diagnostic = crate::tally::module_skip(record).filter(|_| !self.collect_only)?;
        self.paths
            .insert(diagnostic.path.to_string())
            .then_some(diagnostic)
    }

    pub(crate) fn count(&self) -> u32 {
        u32::try_from(self.paths.len()).unwrap_or(u32::MAX)
    }
}

/// Failed lifetimes that kept no test from running: a failed
/// `tearDownClass` or `tearDownModule`, a class or module cleanup that
/// raised, or a failed setup whose tests the run stopped before (under
/// `maxfail` the engine reports a failed setup before the tests it blocks,
/// and the runner keeps what closes a lifetime but drops the tests after
/// the bad outcome that reaches `maxfail`). The run fails on them, but
/// `run_finished`'s counts hold test results only, so the summaries count
/// them from the stream, by the phase that failed: pytest's summary counts
/// each as an error (`1 passed, 1 error`), unittest's too (`FAILED
/// (errors=1)`). Each lifetime counts once, however often it failed (a
/// class split across batches tears down in each), as it is one test case
/// in the JUnit report; one that some test names as its `cause` counts
/// through that test instead. Only `lifetime_finished` and `test_finished`
/// matter: a `lifetime_started` without its `lifetime_finished` (a worker
/// that crashed or was stopped) counts nothing.
#[derive(Debug, Default)]
pub(crate) struct LifetimeErrors {
    /// The phase each failed lifetime first failed in.
    failed: HashMap<LifetimeId, Option<Phase>>,
    /// Lifetimes some test's result names as its `cause`.
    blocking: HashSet<LifetimeId>,
}

/// [`LifetimeErrors`], by the phase that failed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LifetimeCounts {
    pub setup: u32,
    pub teardown: u32,
    /// A failure that names no phase (or the test's own, which a lifetime
    /// has none of): no adapter reports one today.
    pub other: u32,
}

impl LifetimeErrors {
    pub(crate) fn note(&mut self, record: &Record) {
        match &record.event {
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                ..
            } => {
                self.failed.entry(lifetime.clone()).or_insert(failure.phase);
            }
            Event::TestFinished(result) => {
                if let Some(cause) = &result.cause {
                    self.blocking.insert(cause.clone());
                }
            }
            _ => {}
        }
    }

    /// The failed lifetimes that kept no test from running, so far.
    pub(crate) fn counts(&self) -> LifetimeCounts {
        let mut counts = LifetimeCounts::default();
        #[expect(clippy::iter_over_hash_type, reason = "counts, in any order")]
        for (lifetime, phase) in &self.failed {
            if self.blocking.contains(lifetime) {
                continue;
            }
            let slot = match phase {
                Some(Phase::Setup) => &mut counts.setup,
                Some(Phase::Teardown) => &mut counts.teardown,
                Some(Phase::Test) | None => &mut counts.other,
            };
            *slot = slot.saturating_add(1);
        }
        counts
    }
}

/// Where to look: where the failure was raised when that is in the project,
/// else the innermost project frame, else wherever it was raised.
pub(crate) fn origin(failure: &Failure) -> Option<(&FramePath, u32)> {
    let raised = failure
        .location
        .as_ref()
        .map(|location| (&location.path, location.line))
        .or_else(|| failure.frames.last().map(|frame| (&frame.path, frame.line)));
    if let Some(raised) = raised.filter(|(path, _)| path.is_project()) {
        return Some(raised);
    }
    failure
        .frames
        .iter()
        .rev()
        .find(|frame| frame.path.is_project())
        .map(|frame| (&frame.path, frame.line))
        .or(raised)
}

/// `path:line in function: source`.
pub(crate) fn frame_line(frame: &Frame) -> String {
    let mut line = format!("{}:{}", frame.path, frame.line);
    if let Some(function) = &frame.function {
        line.push_str(" in ");
        line.push_str(function);
    }
    if let Some(source) = &frame.source {
        line.push_str(": ");
        line.push_str(source);
    }
    line
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

/// The command that reruns a test: the recorded one, or `aye <id>`.
pub(crate) fn rerun(id: &TestId, reproduce: Option<&Reproduce>) -> String {
    match reproduce {
        Some(reproduce) if !reproduce.command.is_empty() => join(&reproduce.command),
        _ => join(&["aye", id.as_str()]),
    }
}

/// The command that reruns a file's tests: `aye <path>`.
pub(crate) fn rerun_path(path: &str) -> String {
    join(&["aye", path])
}

/// Words joined into one shell command, quoted where a shell needs it.
fn join<S: AsRef<str>>(words: &[S]) -> String {
    shlex::try_join(words.iter().map(AsRef::as_ref)).unwrap_or_else(|_| {
        words
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(" ")
    })
}

#[cfg(test)]
mod tests {
    use aye_events::{RunNumber, Scope};

    use super::*;

    #[test]
    fn head_and_tail_cut_on_characters() {
        assert_eq!(head("abcdef", 6), "abcdef");
        assert_eq!(head("abcdefg", 6), "abcde…");
        assert_eq!(head("ééééééé", 3), "éé…");
        assert_eq!(tail("abcdef", 6), "abcdef");
        assert_eq!(tail("abcdefg", 6), "…cdefg");
        assert_eq!(tail("ééééééé", 3), "…éé");
    }

    #[test]
    fn last_lines_counts_every_line() {
        let lines = last_lines("a\nb\nc\nd\n\n", 2);
        assert_eq!(lines.lines, ["c", "d"]);
        assert_eq!(lines.total, 4);
        assert_eq!(last_lines("", 3).total, 0);
    }

    #[test]
    fn durations() {
        assert_eq!(spaced(Duration::from_micros(12_400)), "12 ms");
        assert_eq!(spaced(Duration::from_millis(840)), "840 ms");
        assert_eq!(spaced(Duration::from_millis(1234)), "1.23 s");
        assert_eq!(spaced(Duration::from_secs(65)), "1 min 5 s");
        assert_eq!(seconds(Duration::from_millis(120)), "0.12 s");
        assert!((secs(Duration::from_millis(1234)) - 1.23).abs() < f64::EPSILON);
        assert!((secs(Duration::from_millis(5)) - 0.01).abs() < f64::EPSILON);
    }

    #[test]
    fn frames_outside_the_project_are_grouped_by_package() {
        let frame = |path: &str| Frame {
            path: path.into(),
            line: 1,
            ..Default::default()
        };
        let frames = [
            frame("tests/a.py"),
            frame("/venv/lib/python3.13/site-packages/requests/api.py"),
            frame("/venv/lib/python3.13/site-packages/requests/sessions.py"),
            frame("/venv/lib/python3.13/site-packages/urllib3/pool.py"),
            frame("src/b.py"),
            frame("/usr/lib/python3.13/socket.py"),
            frame("C:\\Python313\\Lib\\json\\decoder.py"),
            frame("<frozen importlib._bootstrap>"),
        ];
        let items: Vec<String> = frame_items(&frames)
            .into_iter()
            .map(|item| match item {
                FrameItem::Project(frame) => frame.path.to_string(),
                FrameItem::Others { count, package } => others(count, &package),
            })
            .collect();
        assert_eq!(
            items,
            [
                "tests/a.py",
                "2 frames in requests",
                "1 frame in urllib3",
                "src/b.py",
                "1 frame in socket",
                "1 frame in json",
                "1 frame in frozen importlib._bootstrap",
            ]
        );
        assert_eq!(package_of("/opt/tools/helper.py"), "tools");
    }

    #[test]
    fn hints_about_pytest_flags_are_removed() {
        let diff: Vec<String> = [
            "",
            "At index 2 diff: 3 != 4",
            "Use -v to get more diff",
            "Omitting 3 identical items, use -vv to show",
            "Skipping 12 identical leading characters in diff, use -v to show",
            "- spam",
            "+ eggs",
            "",
            "...Full output truncated (3 lines hidden), use '-vv' to show",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            diff_lines(&diff),
            [
                "At index 2 diff: 3 != 4",
                "Omitting 3 identical items",
                "Skipping 12 identical leading characters in diff",
                "- spam",
                "+ eggs",
                "",
                "...Full output truncated (3 lines hidden)",
            ]
        );
    }

    fn failure(message: &str, detail: &str) -> Failure {
        Failure {
            message: message.into(),
            detail: Some(detail.into()),
            ..Failure::default()
        }
    }

    #[test]
    fn the_rest_of_a_message_comes_from_the_frameworks_text() {
        // Python's traceback, as unittest writes it.
        let python = "Traceback (most recent call last):\n  File \"t.py\", line 5, in test_x\n    self.fail(\"a\\nb\")\nAssertionError: first\nsecond\n  third\n\n";
        assert_eq!(
            message_rest(&failure("first", python)),
            ["second", "  third"]
        );
        // An exception named by its module, as Python prints it.
        let qualified = "Traceback (most recent call last):\njson.decoder.JSONDecodeError: Expecting value\nat 1\n";
        assert_eq!(
            message_rest(&failure("JSONDecodeError: Expecting value", qualified)),
            ["at 1"]
        );
        // pytest's: the `E` lines, then the location, which isn't part of it.
        let pytest = "def test_x():\n>       assert f(2) == 5\nE       assert 4 == 5\nE        +  where 4 = f(2)\n\ntests/t.py:2: AssertionError";
        assert_eq!(
            message_rest(&failure("assert 4 == 5", pytest)),
            [" +  where 4 = f(2)"]
        );
        // Indented, as a doctest's "Exception raised:" is.
        let doctest = "Failed example:\n    boom()\nException raised:\n    Traceback (most recent call last):\n    ValueError: a\n    b\n";
        assert_eq!(message_rest(&failure("ValueError: a", doctest)), ["b"]);
    }

    #[test]
    fn a_message_has_no_rest_when_nothing_follows_it_or_it_isnt_found() {
        assert_eq!(
            message_rest(&failure("boom", "ValueError: boom\n")),
            [] as [&str; 0]
        );
        // A doctest mismatch's message isn't in its text.
        let doctest = "Failed example:\n    f()\nExpected:\n    1\nGot:\n    2\n";
        assert_eq!(
            message_rest(&failure("f(): expected '1', got '2'", doctest)),
            [] as [&str; 0]
        );
        // pytest's location line ends with the exception's name, but isn't
        // the message.
        assert_eq!(
            message_rest(&failure(
                "AssertionError",
                "x\n\ntests/t.py:3: AssertionError"
            )),
            [] as [&str; 0]
        );
        // A source line that ends with the message isn't it either.
        assert_eq!(
            message_rest(&failure(
                "boom",
                "    raise X('boom')\n    print(boom)\nmore"
            )),
            [] as [&str; 0]
        );
        // An explained comparison's lines are its diff.
        let mut explained = failure("a != b", "AssertionError: a != b\n- a\n+ b\n");
        explained.assertion = Some(Assertion {
            op: "==".into(),
            left: "a".into(),
            right: "b".into(),
            diff: vec!["- a".into(), "+ b".into()],
            ..Default::default()
        });
        assert_eq!(message_rest(&explained), [] as [&str; 0]);
        assert_eq!(message_rest(&Failure::default()), [] as [&str; 0]);
    }

    fn assertion(left: &str, right: &str, diff: &[&str]) -> Assertion {
        Assertion {
            op: "==".into(),
            left: left.into(),
            right: right.into(),
            diff: diff.iter().map(|line| (*line).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn explanations_lose_a_lone_hunk_header() {
        let doctest = assertion("2", "3", &["@@ -1 +1 @@", "-3", "+2"]);
        assert_eq!(explanation(&doctest), ["-3", "+2"]);
        assert!(only_restates(&doctest, &explanation(&doctest)));
        let hunks = assertion("a\nb", "a\nc", &["@@ -1 +1 @@", "-a", "@@ -9 +9 @@", "+c"]);
        assert_eq!(explanation(&hunks).len(), 4);
        // pytest's string diff shows the text without its quotes: more
        // than the operands.
        let strings = assertion("'spam'", "'eggs'", &["- eggs", "+ spam"]);
        assert!(!only_restates(&strings, &explanation(&strings)));
        let restated = assertion("1", "2", &["- 1", "+ 2"]);
        assert!(only_restates(&restated, &explanation(&restated)));
    }

    #[test]
    fn operands_are_shown_when_they_add_to_the_message() {
        let with = |message: &str| Failure {
            message: message.into(),
            ..Failure::default()
        };
        // Already in the message.
        assert!(!operands_add(
            &with("assert 4 == 5"),
            &assertion("4", "5", &[])
        ));
        // Cut from it.
        assert!(operands_add(
            &with("{'a': 1, [41 chars]} != {...}"),
            &assertion("{'a': 1, 'b': 2}", "{'a': 1, 'b': 3}", &[])
        ));
        // Restated by the explanation.
        assert!(!operands_add(
            &with("f(): expected '3', got 'two'"),
            &assertion("2", "3", &["@@ -1 +1 @@", "-3", "+2"])
        ));
        // Spanning lines, with or without an explanation of them.
        assert!(!operands_add(
            &with("x"),
            &assertion("a\nb", "a\nc", &[" a", "-b", "+c"])
        ));
        assert!(operands_add(&with("x"), &assertion("a\nb", "a\nc", &[])));
    }

    #[test]
    fn failed_lifetimes_count_once_and_only_when_they_blocked_no_test() {
        use aye_events::{Outcome, TestResult};

        let lifetime = |id: &str, phase: Option<Phase>| {
            Record::new(Event::LifetimeFinished {
                lifetime: id.into(),
                failure: Some(Failure {
                    phase,
                    message: "boom".into(),
                    ..Failure::default()
                }),
                duration: Duration::ZERO,
            })
        };
        let blocked = |cause: &str| {
            let mut result = TestResult::new(TestId::new("t.py::T::test_x"), Outcome::Error);
            result.cause = Some(cause.into());
            Record::new(Event::TestFinished(Box::new(result)))
        };
        let mut errors = LifetimeErrors::default();
        for record in [
            // A class split across batches tears down in each: one count.
            lifetime("t.py::A", Some(Phase::Teardown)),
            lifetime("t.py::A", Some(Phase::Teardown)),
            // Blocking tests before its failure arrives (the engine's
            // order) or after it (under `maxfail`): no count of its own.
            blocked("t.py::B"),
            lifetime("t.py::B", Some(Phase::Setup)),
            lifetime("t.py::C", Some(Phase::Setup)),
            blocked("t.py::C"),
            // A setup whose tests the run dropped.
            lifetime("t.py::D", Some(Phase::Setup)),
            lifetime("t.py", None),
            // Started, never finished (a crash): nothing.
            Record::new(Event::LifetimeStarted {
                lifetime: "t.py::E".into(),
                kind: Scope::Class,
            }),
            Record::new(Event::LifetimeFinished {
                lifetime: "t.py::F".into(),
                failure: None,
                duration: Duration::ZERO,
            }),
        ] {
            errors.note(&record);
        }
        assert_eq!(
            errors.counts(),
            LifetimeCounts {
                setup: 1,
                teardown: 1,
                other: 1,
            }
        );
    }

    #[test]
    fn rerun_quotes_for_the_shell() {
        let id = TestId::new("tests/t.py::test_x[1-2]");
        assert_eq!(rerun(&id, None), "aye 'tests/t.py::test_x[1-2]'");
        let reproduce = Reproduce {
            command: vec!["aye".into(), "tests/t.py::test_y".into(), "--seed=7".into()],
            inputs: std::collections::BTreeMap::new(),
        };
        assert_eq!(
            rerun(&id, Some(&reproduce)),
            "aye tests/t.py::test_y '--seed=7'"
        );
    }

    fn compared(
        expected: Option<Side>,
        sources: (Option<&str>, Option<&str>),
        compare: Option<aye_events::Compare>,
    ) -> Failure {
        Failure {
            phase: Some(Phase::Test),
            message: "assert 2 == 3".into(),
            assertion: Some(Assertion {
                op: "==".into(),
                left: "2".into(),
                right: "3".into(),
                expected,
                left_source: sources.0.map(str::to_owned),
                right_source: sources.1.map(str::to_owned),
                compare,
                ..Default::default()
            }),
            ..Failure::default()
        }
    }

    fn compare(kind: &str, summary: &str, differs: Vec<Difference>) -> aye_events::Compare {
        aye_events::Compare {
            kind: kind.into(),
            summary: summary.into(),
            differs,
            ..aye_events::Compare::default()
        }
    }

    fn entry(at: Option<&str>, left: Option<&str>, right: Option<&str>) -> Difference {
        Difference {
            at: at.map(str::to_owned),
            left: left.map(str::to_owned),
            right: right.map(str::to_owned),
        }
    }

    #[test]
    fn a_comparisons_own_message_is_the_one_the_test_gave() {
        let with = |message: &str, left_source: Option<&str>| {
            let mut failure = compared(None, (left_source, None), None);
            failure.message = message.into();
            failure
        };
        let own = |failure: &Failure| {
            own_message(failure, failure.assertion.as_ref().unwrap()).map(str::to_owned)
        };
        // pytest's own text, and pytest's with the test's message first.
        assert_eq!(own(&with("assert 2 == 3", None)), None);
        assert_eq!(
            own(&with("totals differ", None)).as_deref(),
            Some("totals differ")
        );
        // unittest's `first != second`, with `msg` after ` : `.
        assert_eq!(own(&with("{'a': 1} != {'a': 2}", None)), None);
        assert_eq!(
            own(&with("{'a': 1} != {'a': 2} : totals differ", None)).as_deref(),
            Some("totals differ")
        );
        // The doctest adapter's, which names the example.
        assert_eq!(
            own(&with(
                "cart.total(): expected '1', got '2'",
                Some("cart.total()")
            )),
            None
        );
    }

    #[test]
    fn why_says_what_was_expected_and_what_came() {
        let dict = compare(
            "dict",
            "dict differs in 1 of 2 keys",
            vec![entry(Some("['pear']"), Some("2"), Some("3"))],
        );
        // The expected side known: the actual operand's source, where.
        let failure = compared(
            Some(Side::Right),
            (Some("cart.summary()"), Some("{'pear': 3}")),
            Some(dict.clone()),
        );
        assert_eq!(
            why(&failure, true),
            "cart.summary()['pear']: expected 3, actual 2"
        );
        // On the left, the right operand is the actual one.
        let failure = compared(
            Some(Side::Left),
            (Some("{'pear': 2}"), Some("cart.summary()")),
            Some(dict.clone()),
        );
        assert_eq!(
            why(&failure, true),
            "cart.summary()['pear']: expected 2, actual 3"
        );
        // A source too long, or spanning lines, is left out or cut.
        let long = "inventory.levels_for_every_warehouse_in(region)";
        let failure = compared(Some(Side::Right), (Some(long), None), Some(dict.clone()));
        assert_eq!(why(&failure, true), "['pear']: expected 3, actual 2");
        let failure = compared(
            Some(Side::Right),
            (Some("build(\n    1,\n)"), None),
            Some(dict.clone()),
        );
        assert_eq!(why(&failure, true), "build(…['pear']: expected 3, actual 2");
        // Neither side literal: each named by its source.
        let failure = compared(None, (Some("a.total()"), Some("b.total()")), Some(dict));
        assert_eq!(
            why(&failure, true),
            "['pear']: a.total() is 2, b.total() is 3"
        );
        // No entries (numbers, dates): the operands themselves.
        let failure = compared(
            None,
            (Some("cart.total()"), None),
            Some(compare("number", "differ by 1", Vec::new())),
        );
        assert_eq!(why(&failure, true), "cart.total() is 2, right is 3");
        // Several differences: the summary.
        let failure = compared(
            Some(Side::Right),
            (Some("xs"), None),
            Some(compare(
                "list",
                "list differs at 2 of 5 positions",
                vec![
                    entry(Some("[1]"), Some("2"), Some("3")),
                    entry(Some("[3]"), Some("4"), Some("5")),
                ],
            )),
        );
        assert_eq!(
            why(&failure, true),
            "list differs at 2 of 5 positions: [1], [3]"
        );
        // An item of a set only one side has: the summary too.
        let failure = compared(
            Some(Side::Right),
            (Some("tags"), None),
            Some(compare(
                "set",
                "set: 1 missing",
                vec![entry(None, None, Some("'b'"))],
            )),
        );
        assert_eq!(why(&failure, true), "set: 1 missing");
        // Text: where it differs, in brackets, or the summary.
        let text = compare(
            "str",
            "strings differ at line 2, column 5",
            vec![entry(
                Some("line 2, column 5"),
                Some("'wrld'"),
                Some("'world'"),
            )],
        );
        let failure = compared(
            Some(Side::Right),
            (Some("greeting"), None),
            Some(text.clone()),
        );
        assert_eq!(
            why(&failure, true),
            "greeting (line 2, column 5): expected 'world', actual 'wrld'"
        );
        let failure = compared(Some(Side::Right), (None, None), Some(text));
        assert_eq!(
            why(&failure, true),
            "strings differ at line 2, column 5: expected 'world', actual 'wrld'"
        );
        // One line of text, both shown whole: the place says nothing more.
        let line = compare(
            "str",
            "strings differ at line 1, column 2",
            vec![entry(
                Some("line 1, column 2"),
                Some("'wrld'"),
                Some("'world'"),
            )],
        );
        let failure = compared(
            Some(Side::Right),
            (Some("greeting"), None),
            Some(line.clone()),
        );
        assert_eq!(
            why(&failure, true),
            "greeting: expected 'world', actual 'wrld'"
        );
        let failure = compared(Some(Side::Right), (None, None), Some(line));
        assert_eq!(why(&failure, true), "expected 'world', actual 'wrld'");
        // `==` with an expected side, without a comparison.
        let failure = compared(Some(Side::Right), (None, None), None);
        assert_eq!(why(&failure, true), "expected 3, actual 2");
        // Values are cut to 40 characters.
        let mut failure = compared(Some(Side::Right), (None, None), None);
        failure.assertion.as_mut().unwrap().right = "x".repeat(100);
        assert_eq!(
            why(&failure, true),
            format!("expected {}…, actual 2", "x".repeat(39))
        );
    }

    #[test]
    fn why_blames_the_fixture() {
        let failed = |phase, resource: Option<&str>| Failure {
            phase: Some(phase),
            message: "RuntimeError: boom".into(),
            resource: resource.map(str::to_owned),
            ..Failure::default()
        };
        assert_eq!(
            why(&failed(Phase::Setup, Some("db")), true),
            "fixture db: RuntimeError: boom"
        );
        assert_eq!(
            why(&failed(Phase::Teardown, Some("db")), true),
            "passed; fixture db failed in teardown: RuntimeError: boom"
        );
        assert_eq!(
            why(&failed(Phase::Teardown, None), true),
            "teardown failed: RuntimeError: boom"
        );
        // A lifetime's teardown is no test's.
        assert_eq!(
            why(&failed(Phase::Teardown, None), false),
            "RuntimeError: boom"
        );
        assert_eq!(why(&failed(Phase::Setup, None), true), "RuntimeError: boom");
        // pytest's own fixture for `setup_module` is named for what it runs.
        let xunit = Some("_xunit_setup_module_fixture_tests.test_a");
        assert_eq!(
            why(&failed(Phase::Setup, xunit), true),
            "setup_module: RuntimeError: boom"
        );
        assert_eq!(
            why(&failed(Phase::Teardown, xunit), true),
            "passed; teardown_module failed: RuntimeError: boom"
        );
        assert_eq!(
            setup_chain(&[
                Resource {
                    name: "_xunit_setup_class_fixture_TestA".into(),
                    scope: Scope::Class,
                },
                Resource {
                    name: "cart".into(),
                    scope: Scope::Function,
                },
            ]),
            "setup_class (class) → cart (function)"
        );
    }

    #[test]
    fn why_adds_the_hint_to_the_message() {
        let failure = Failure {
            phase: Some(Phase::Setup),
            message: "fixture 'kart' not found".into(),
            hint: Some("did you mean 'cart'?".into()),
            ..Failure::default()
        };
        assert_eq!(
            why(&failure, true),
            "fixture 'kart' not found; did you mean 'cart'?"
        );
        // The whole line is cut to 160 characters.
        let long = Failure {
            message: "m".repeat(400),
            ..Failure::default()
        };
        assert_eq!(why(&long, true).chars().count(), 160);
    }

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
        assert_eq!(
            history_counts(&Previous {
                outcomes: "pfpes".into(),
                since: None,
            }),
            (2, 2)
        );
    }
}
