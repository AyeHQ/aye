//! L2 and L3 of a bad outcome's line: its code, comparison, message, rows
//! and, at L3, its frames, output and locals ([`Detail`]).

use std::borrow::Cow;
use std::io::Write;
use std::time::Duration;

use aye_events::{Captured, Compare, Failure, TestResult};

use super::JsonLines;
use super::limits::{
    ALSO, CODE_MAX, DIFF_LINE_MAX, DIFF_LINES, DIFFERENCES, FIXTURE_MAX, FRAME_MAX, HINT_MAX,
    MESSAGE_LINES, MESSAGE_MAX, OPERAND_MAX, OUTPUT_LINES, OUTPUT_MAX, RERUN_MAX, SETUP,
    SOURCE_MAX, SUMMARY_MAX, VALUE_MAX,
};
use super::schema::{CompareLine, Detail, Diff, Entry, History, Locals, Output};
use crate::{failure, rerun, words};

/// The whole message, when it spans lines: its first line, then at most
/// 10 more, bounded.
fn message(failure: &Failure) -> Option<Cow<'_, str>> {
    let rest = failure::message_rest(failure);
    if rest.is_empty() {
        return None;
    }
    let mut lines = vec![failure::message(failure)];
    lines.extend(rest.iter().take(MESSAGE_LINES));
    if rest.len() > MESSAGE_LINES {
        lines.push("…");
    }
    Some(Cow::Owned(
        words::head(&lines.join("\n"), MESSAGE_MAX).into_owned(),
    ))
}

/// The structured comparison, bounded.
fn compare_line<'a>(
    assertion: &'a aye_events::Assertion,
    compare: &'a Compare,
    all: bool,
) -> CompareLine<'a> {
    let shown = if all {
        compare.differs.len()
    } else {
        DIFFERENCES.min(compare.differs.len())
    };
    let known = assertion.expected.is_some();
    let value = |value: Option<&'a str>| value.map(|value| words::head(value, VALUE_MAX));
    let differs = compare.differs[..shown]
        .iter()
        .map(|difference| {
            let at = difference
                .at
                .as_deref()
                .map(|at| words::head(at, VALUE_MAX));
            if known {
                let (expected, actual) = failure::entries(assertion, difference);
                Entry {
                    at,
                    expected: value(expected),
                    actual: value(actual),
                    left: None,
                    right: None,
                }
            } else {
                Entry {
                    at,
                    expected: None,
                    actual: None,
                    left: value(difference.left.as_deref()),
                    right: value(difference.right.as_deref()),
                }
            }
        })
        .collect();
    let omitted = u32::try_from(compare.differs.len() - shown)
        .unwrap_or(u32::MAX)
        .saturating_add(compare.omitted);
    CompareLine {
        kind: words::head(compare.kind.as_str(), VALUE_MAX),
        summary: words::head(&compare.summary, SUMMARY_MAX),
        same: compare.same,
        differs,
        omitted,
    }
}

/// The explanation, bounded; none when it only restates the operands,
/// which the line carries already.
fn bounded_diff(assertion: &aye_events::Assertion) -> Vec<Cow<'_, str>> {
    let mut lines = failure::explanation(assertion);
    if failure::only_restates(assertion, &lines) {
        lines.clear();
    }
    if lines.len() <= DIFF_LINES {
        return lines
            .into_iter()
            .map(|line| words::head(line, DIFF_LINE_MAX))
            .collect();
    }
    let more = lines.len() - (DIFF_LINES - 1);
    lines
        .into_iter()
        .take(DIFF_LINES - 1)
        .map(|line| words::head(line, DIFF_LINE_MAX))
        .chain([Cow::Owned(format!("… {more} more lines"))])
        .collect()
}

/// A failure's comparison, at L2: the structured one, or the operands and
/// the explanation.
fn comparison<'a>(detail: &mut Detail<'a>, failure: &'a Failure, all: bool) {
    let Some(assertion) = &failure.assertion else {
        return;
    };
    let source = |source: Option<&'a String>| {
        source.map(|source| {
            Cow::Owned(words::head(&failure::first_line(source), SOURCE_MAX).into_owned())
        })
    };
    let known = assertion.expected.is_some();
    let operands = if let Some(compare) = &assertion.compare {
        detail.diff = Some(Diff::Compare(compare_line(assertion, compare, all)));
        compare.differs.is_empty()
    } else {
        let lines = bounded_diff(assertion);
        let spans_lines = assertion.left.contains('\n') || assertion.right.contains('\n');
        if !lines.is_empty() {
            detail.diff = Some(Diff::Lines(lines));
        }
        // Not when the message holds both as written, nor when they span
        // lines and the explanation shows how they differ.
        let in_message =
            failure.message.contains(&assertion.left) && failure.message.contains(&assertion.right);
        !(in_message || spans_lines && detail.diff.is_some())
    };
    if !known {
        detail.left_source = source(assertion.left_source.as_ref());
        detail.right_source = source(assertion.right_source.as_ref());
    }
    if operands {
        if known {
            let (expected, actual) = failure::whole(assertion);
            detail.expected = Some(words::head(expected, OPERAND_MAX));
            detail.actual = Some(words::head(actual, OPERAND_MAX));
        } else {
            detail.left = Some(words::head(&assertion.left, OPERAND_MAX));
            detail.right = Some(words::head(&assertion.right, OPERAND_MAX));
        }
    }
}

/// L3's frames: each project frame, and each run of others as one entry.
fn frames(failure: &Failure) -> Vec<String> {
    failure::frame_items(&failure.frames)
        .into_iter()
        .map(|item| match item {
            failure::FrameItem::Project(frame) => {
                words::head(&failure::frame_line(frame), FRAME_MAX).into_owned()
            }
            failure::FrameItem::Others { count, package } => failure::others(count, &package),
        })
        .collect()
}

pub(super) fn output_tails(output: &Captured) -> Option<Output> {
    let tail = |captured: &str| {
        if captured.trim().is_empty() {
            return None;
        }
        let lines = words::last_lines(captured, OUTPUT_LINES);
        let mut shown = lines.lines.join("\n");
        if lines.total > lines.lines.len() {
            shown = format!(
                "[{} earlier lines omitted]\n{shown}",
                lines.total - lines.lines.len()
            );
        }
        Some(words::tail(&shown, OUTPUT_MAX).into_owned())
    };
    let output = Output {
        stdout: tail(&output.stdout),
        stderr: tail(&output.stderr),
        log: tail(&output.log),
    };
    (output.stdout.is_some() || output.stderr.is_some() || output.log.is_some()).then_some(output)
}

impl<W: Write> JsonLines<W> {
    /// L2 and L3 of a failure.
    pub(super) fn detail<'a>(
        &self,
        failure: &'a Failure,
        later: &'a [Failure],
        result: Option<&'a TestResult>,
        duration: Duration,
        rerun: Option<String>,
    ) -> Detail<'a> {
        let mut detail = Detail::default();
        if !self.detailed() {
            return detail;
        }
        if let Some(frame) = failure::pointed(failure) {
            // The failing line at its indentation relative to `before`.
            let indent = if frame.before.is_empty() {
                0
            } else {
                frame.indent.min(40)
            };
            detail.code = frame.source.as_deref().map(|source| {
                let source = words::head(source, CODE_MAX);
                if indent == 0 {
                    source
                } else {
                    Cow::Owned(format!("{:indent$}{source}", "", indent = indent as usize))
                }
            });
            detail.cols = frame
                .cols
                .filter(|_| frame.source.is_some())
                .map(|cols| [cols.start + indent, cols.end + indent]);
            detail.before = frame
                .before
                .iter()
                .map(|line| words::head(line, CODE_MAX))
                .collect();
        }
        comparison(&mut detail, failure, self.all());
        // A structured comparison, or the expected and actual values, say
        // what the message's lines would.
        let compared = matches!(detail.diff, Some(Diff::Compare(_))) || detail.expected.is_some();
        detail.message = if compared {
            // But the test's own message, when it gave one.
            failure
                .assertion
                .as_ref()
                .and_then(|assertion| failure::own_message(failure, assertion))
                .map(|own| words::head(own, MESSAGE_MAX))
        } else {
            message(failure)
        };
        detail.hint = failure
            .hint
            .as_deref()
            .map(|hint| words::head(hint, HINT_MAX));
        detail.fixture = failure
            .resource
            .as_deref()
            .map(|resource| failure::resource_name(resource, failure.phase))
            .filter(|(_, fixture)| *fixture)
            .map(|(name, _)| Cow::Owned(words::head(&name, FIXTURE_MAX).into_owned()));
        if let Some(result) = result {
            detail.setup = result
                .resources
                .iter()
                .take(SETUP)
                .map(|resource| {
                    let (name, _) = failure::resource_name(&resource.name, None);
                    words::head(&format!("{name}:{}", resource.scope), FIXTURE_MAX).into_owned()
                })
                .collect();
            detail.history = result.history.as_ref().and_then(|previous| {
                let total = previous.outcomes.len();
                let (passed, failed) = previous.outcomes.counts();
                (total > 0).then(|| History {
                    previous: total,
                    passed,
                    failed,
                    since: previous.since.map(|since| since.to_string()),
                })
            });
        }
        detail.also = later
            .iter()
            .take(ALSO)
            .map(|also| {
                let phase = also.phase.map_or("test", words::phase);
                words::head(&format!("{phase}: {}", failure::message(also)), MESSAGE_MAX)
                    .into_owned()
            })
            .collect();
        detail.rerun = rerun.map(|rerun| words::head(&rerun, RERUN_MAX).into_owned());
        detail.ms = Some(words::millis(duration));
        if self.all() {
            detail.frames = frames(failure);
            detail.output = result.and_then(|result| output_tails(&result.output));
            let locals = failure::locals(failure);
            detail.locals = (!locals.is_empty()).then_some(Locals(locals));
        }
        detail
    }

    /// The test's output and rerun, on the line of a bad outcome without a
    /// failure of its own, or on the first of its sub-outcomes'.
    pub(super) fn carry(&self, detail: &mut Detail<'_>, result: &TestResult) {
        if !self.detailed() {
            return;
        }
        detail.rerun = Some(
            words::head(
                &rerun::rerun(&result.id, result.reproduce.as_ref()),
                RERUN_MAX,
            )
            .into_owned(),
        );
        if self.all() {
            detail.output = output_tails(&result.output);
        }
    }
}
