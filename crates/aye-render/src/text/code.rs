//! The pointed code, its carets, and locations, hyperlinked on a live
//! terminal.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};
use aye_events::{Columns, Failure};

use super::Text;
use super::stylesheet::{CYAN, DIM, RED, RED_BOLD, SPAN};
use crate::{failure, words};

/// Where in `source` the span `cols` is: its start and length in
/// characters, within the line, at least one long.
pub(super) fn span(source: &str, cols: Columns) -> (usize, usize) {
    let length = source.chars().count();
    let start = usize::try_from(cols.start)
        .unwrap_or(0)
        .min(length.saturating_sub(1));
    let end = usize::try_from(cols.end)
        .unwrap_or(0)
        .clamp(start + 1, length.max(start + 1));
    (start, end - start)
}

/// A `file://` URL for `path`, relative to `root` unless absolute.
pub(super) fn file_url(root: &str, path: &str) -> String {
    use std::fmt::Write as _;

    let absolute = path.starts_with('/') || path.as_bytes().get(1) == Some(&b':');
    let full = if absolute {
        path.to_owned()
    } else {
        format!("{}/{path}", root.trim_end_matches(['/', '\\']))
    };
    let full = full.replace('\\', "/");
    let full = if full.starts_with('/') {
        full
    } else {
        format!("/{full}")
    };
    let mut url = String::from("file://");
    for byte in full.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// The location, then the failing line with the two before it and the
    /// failing expression marked.
    pub(super) fn pointed_code(&mut self, failure: &Failure) -> io::Result<()> {
        let Some((path, line)) = failure::origin(failure) else {
            return Ok(());
        };
        writeln!(self.out, "  {}", self.location(path.as_str(), line))?;
        let Some(frame) = failure::pointed(failure) else {
            return Ok(());
        };
        let Some(source) = &frame.source else {
            return Ok(());
        };
        let width = line.to_string().len();
        let before = u32::try_from(frame.before.len()).unwrap_or(0);
        for (at, code) in (line.saturating_sub(before)..).zip(&frame.before) {
            writeln!(
                self.out,
                "     {DIM}{at:>width$} │{DIM:#} {}",
                words::head(code, self.line_max())
            )?;
        }
        // The failing line at its indentation relative to the ones before.
        let indent = if frame.before.is_empty() {
            0
        } else {
            usize::try_from(frame.indent).unwrap_or(0).min(40)
        };
        write!(
            self.out,
            "  {RED_BOLD}→{RED_BOLD:#}  {DIM}{line:>width$} │{DIM:#} {:indent$}",
            ""
        )?;
        let source = words::head(source, self.line_max());
        self.marked(&source, frame.cols)?;
        if let Some(cols) = frame.cols {
            let (start, len) = span(&source, cols);
            let start = indent + start;
            writeln!(
                self.out,
                "     {:width$} {DIM}│{DIM:#} {:start$}{RED}{}{RED:#}",
                "",
                "",
                "^".repeat(len)
            )?;
        }
        Ok(())
    }

    /// A source line, the span `cols` red and underlined.
    pub(super) fn marked(&mut self, source: &str, cols: Option<Columns>) -> io::Result<()> {
        let Some(cols) = cols else {
            return writeln!(self.out, "{source}");
        };
        let (start, len) = span(source, cols);
        let byte = |chars: usize| {
            source
                .char_indices()
                .nth(chars)
                .map_or(source.len(), |(at, _)| at)
        };
        let (from, to) = (byte(start), byte(start + len));
        writeln!(
            self.out,
            "{}{SPAN}{}{SPAN:#}{}",
            &source[..from],
            &source[from..to],
            &source[to..]
        )
    }

    /// `path:line`, cyan, and a hyperlink to the file on a live terminal
    /// with colour.
    pub(super) fn location(&self, path: &str, line: u32) -> String {
        let text = format!("{CYAN}{path}:{line}{CYAN:#}");
        match self.facts.root.as_deref().filter(|_| self.links) {
            Some(root) => format!(
                "\u{1b}]8;;{}\u{1b}\\{text}\u{1b}]8;;\u{1b}\\",
                file_url(root, path)
            ),
            None => text,
        }
    }
}
