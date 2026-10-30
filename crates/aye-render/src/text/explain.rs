//! How a failure explains itself in a block: a structured comparison, or
//! the message, the operands and the framework's explanation.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};
use anstyle::Style;
use aye_events::{Assertion, Compare, CompareKind, Failure};

use super::Text;
use super::limits::{DIFF_LINES, DIFFERENCES, MESSAGE_LINES, SOURCE_LABEL};
use super::stylesheet::{DIM, GREEN, RED, RED_BOLD};
use crate::{DetailLevel, failure, words};

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// How the failure explains itself: a structured comparison, or the
    /// message, the operands and the explanation, or an exception's
    /// message.
    pub(super) fn explanation(&mut self, failure: &Failure, level: DetailLevel) -> io::Result<()> {
        let Some(assertion) = &failure.assertion else {
            return self.message(failure, level);
        };
        if let Some(compare) = &assertion.compare {
            // The test's own message (`assert a == b, "why"`) first.
            if let Some(own) = failure::own_message(failure, assertion) {
                writeln!(
                    self.out,
                    "  {RED_BOLD}{}{RED_BOLD:#}",
                    words::head(own, self.line_max())
                )?;
            }
            return self.compare(assertion, compare, level);
        }
        self.message(failure, level)?;
        if failure::operands_add(failure, assertion) {
            let (expected, actual) = failure::whole(assertion);
            if assertion.expected.is_some() {
                self.operand("expected:", expected, level)?;
                self.operand("actual:  ", actual, level)?;
            } else {
                self.operand("left: ", &assertion.left, level)?;
                self.operand("right:", &assertion.right, level)?;
            }
        }
        self.diff(assertion, level)
    }

    /// The message, and up to 10 more lines of it (all at L3).
    fn message(&mut self, failure: &Failure, level: DetailLevel) -> io::Result<()> {
        writeln!(
            self.out,
            "  {RED_BOLD}{}{RED_BOLD:#}",
            words::head(failure::message(failure), self.line_max())
        )?;
        let rest = failure::message_rest(failure);
        let shown = if level == DetailLevel::L3 {
            rest.len()
        } else {
            MESSAGE_LINES.min(rest.len())
        };
        for line in &rest[..shown] {
            writeln!(
                self.out,
                "  {RED}{}{RED:#}",
                words::head(line, self.line_max())
            )?;
        }
        self.more_lines(rest.len() - shown, "  ")
    }

    /// `… N more lines`, when some are.
    fn more_lines(&mut self, more: usize, indent: &str) -> io::Result<()> {
        if more == 0 {
            return Ok(());
        }
        let more = words::plural(more, "more line", "more lines");
        writeln!(self.out, "{indent}{DIM}… {more}{DIM:#}")
    }

    /// A compared value; one that spans lines continues under its first.
    fn operand(&mut self, label: &str, value: &str, level: DetailLevel) -> io::Result<()> {
        let mut lines = value.lines();
        let first = lines.next().unwrap_or_default();
        let pad = " ".repeat(label.chars().count() + 5);
        writeln!(
            self.out,
            "    {DIM}{label}{DIM:#} {}",
            words::head(first, self.line_max())
        )?;
        let rest: Vec<&str> = lines.collect();
        let shown = if level == DetailLevel::L3 {
            rest.len()
        } else {
            DIFF_LINES.min(rest.len())
        };
        for line in &rest[..shown] {
            writeln!(self.out, "{pad}{}", words::head(line, self.line_max()))?;
        }
        self.more_lines(rest.len() - shown, &pad)
    }

    /// The framework's explanation, `-` lines red and `+` lines green.
    fn diff(&mut self, assertion: &Assertion, level: DetailLevel) -> io::Result<()> {
        let diff = failure::explanation(assertion);
        let shown = if level == DetailLevel::L3 {
            diff.len()
        } else {
            DIFF_LINES.min(diff.len())
        };
        for line in &diff[..shown] {
            let style = if line.starts_with('-') {
                RED
            } else if line.starts_with('+') {
                GREEN
            } else {
                Style::new()
            };
            writeln!(
                self.out,
                "    {style}{}{style:#}",
                words::head(line, self.line_max())
            )?;
        }
        self.more_lines(diff.len() - shown, "    ")
    }

    /// A structured comparison: its summary, then a row per difference.
    fn compare(
        &mut self,
        assertion: &Assertion,
        compare: &Compare,
        level: DetailLevel,
    ) -> io::Result<()> {
        let text = compare.kind.is_text();
        let first_at = compare
            .differs
            .first()
            .and_then(|difference| difference.at.as_deref());
        let summary = match first_at {
            Some(at) if text && compare.differs.len() == 1 => {
                let what = if compare.kind == CompareKind::Bytes {
                    "bytes"
                } else {
                    "strings"
                };
                format!("{what} differ at {at}")
            }
            _ => compare.summary.clone(),
        };
        if !summary.is_empty() {
            writeln!(self.out, "  {}", words::head(&summary, self.line_max()))?;
        }
        let known = assertion.expected.is_some();
        let (first_name, second_name) = if known {
            ("expected".to_owned(), "actual".to_owned())
        } else {
            let (left, right) = failure::sources(assertion);
            (
                words::head(&left, SOURCE_LABEL).into_owned(),
                words::head(&right, SOURCE_LABEL).into_owned(),
            )
        };
        let (first_style, second_style) = if known {
            (GREEN, RED)
        } else {
            (Style::new(), Style::new())
        };
        let shown = if level == DetailLevel::L3 {
            compare.differs.len()
        } else {
            DIFFERENCES.min(compare.differs.len())
        };
        let line_max = self.line_max();
        let value = |value: Option<&str>| {
            value.map_or_else(
                || "missing".to_owned(),
                |value| words::head(value, line_max).into_owned(),
            )
        };
        let rows: Vec<(String, String, String)> = if compare.differs.is_empty() {
            let (first, second) = if known {
                failure::whole(assertion)
            } else {
                (assertion.left.as_str(), assertion.right.as_str())
            };
            vec![(String::new(), value(Some(first)), value(Some(second)))]
        } else {
            compare.differs[..shown]
                .iter()
                .map(|difference| {
                    let (first, second) = failure::entries(assertion, difference);
                    (
                        difference.at.clone().unwrap_or_default(),
                        value(first),
                        value(second),
                    )
                })
                .collect()
        };
        if text {
            let width = first_name.chars().count().max(second_name.chars().count());
            for (_, first, second) in &rows {
                writeln!(
                    self.out,
                    "    {DIM}{first_name:<width$}{DIM:#}  {first_style}{first}{first_style:#}"
                )?;
                writeln!(
                    self.out,
                    "    {DIM}{second_name:<width$}{DIM:#}  {second_style}{second}{second_style:#}"
                )?;
            }
        } else {
            let at_width = rows.iter().map(|row| row.0.chars().count()).max();
            let at_width = at_width.unwrap_or(0);
            let first_width = rows.iter().map(|row| row.1.chars().count()).max();
            let first_width = first_width.unwrap_or(0);
            for (at, first, second) in &rows {
                let at = if at_width == 0 {
                    String::new()
                } else {
                    format!("{at:<at_width$}   ")
                };
                writeln!(
                    self.out,
                    "    {at}{DIM}{first_name}{DIM:#} {first_style}{first:<first_width$}{first_style:#}   {DIM}{second_name}{DIM:#} {second_style}{second}{second_style:#}"
                )?;
            }
        }
        let more = compare.differs.len() - shown + usize::try_from(compare.omitted).unwrap_or(0);
        if more > 0 {
            writeln!(self.out, "    {DIM}… {more} more{DIM:#}")?;
        }
        Ok(())
    }
}
