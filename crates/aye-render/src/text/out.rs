//! The output stream, and how paragraphs are set apart ([`Out`],
//! [`Group`]).

use std::io::{self, Write};

use anstream::AutoStream;
use anstream::stream::{AsLockedWrite, RawStream};

use super::Text;

/// What was written last, so that paragraphs are set apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Start,
    Header,
    Lines,
    Block,
}

/// The output stream, remembering whether it is at the start of a paragraph.
pub(super) struct Out<W: RawStream> {
    pub(super) stream: AutoStream<W>,
    pub(super) tail: [u8; 2],
}

impl<W: RawStream + AsLockedWrite> Out<W> {
    /// Starts a paragraph: a blank line, unless at the start already.
    pub(super) fn paragraph(&mut self) -> io::Result<()> {
        if self.tail == *b"\n\n" {
            Ok(())
        } else {
            writeln!(self)
        }
    }
}

impl<W: RawStream + AsLockedWrite> Write for Out<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.stream.write(buf)?;
        match written {
            0 => {}
            1 => self.tail = [self.tail[1], buf[0]],
            n => self.tail = [buf[n - 2], buf[n - 1]],
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// Lines follow: a blank line first unless lines came last.
    pub(super) fn lines(&mut self) -> io::Result<()> {
        self.flush_collected()?;
        if !matches!(self.group, Group::Lines | Group::Start) {
            self.out.paragraph()?;
        }
        self.group = Group::Lines;
        Ok(())
    }

    /// A block follows, set apart by blank lines.
    pub(super) fn block(&mut self) -> io::Result<()> {
        self.flush_collected()?;
        self.out.paragraph()?;
        self.group = Group::Block;
        Ok(())
    }
}
