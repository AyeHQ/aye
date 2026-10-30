//! The line a long collection rewrites in place, on a live terminal.

use std::io::{self, Write};

use anstream::stream::{AsLockedWrite, RawStream};

use super::Text;
use crate::words;

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// Rewrite the progress line in place. Only spaces and `\r`, so that it
    /// clears the same whether escapes are passed or stripped. It bypasses
    /// the paragraph tracking: it is gone before anything else is written.
    pub(super) fn show_progress(
        &mut self,
        walked: u64,
        test_files: u64,
        refining: u64,
    ) -> io::Result<()> {
        if !self.live {
            return Ok(());
        }
        let line = if refining > 0 {
            format!(
                "collecting: importing {} to find their tests",
                words::plural(refining, "file", "files")
            )
        } else {
            format!(
                "collecting: {} walked, {} found",
                words::plural(walked, "file", "files"),
                words::plural(test_files, "test file", "test files")
            )
        };
        let width = line.chars().count();
        let pad = self.progress.saturating_sub(width);
        write!(self.out.stream, "\r{line}{:pad$}", "")?;
        self.out.stream.flush()?;
        self.progress = width;
        Ok(())
    }

    pub(super) fn clear_progress(&mut self) -> io::Result<()> {
        if self.progress == 0 {
            return Ok(());
        }
        let width = self.progress;
        write!(self.out.stream, "\r{:width$}\r", "")?;
        self.progress = 0;
        self.out.stream.flush()
    }
}
