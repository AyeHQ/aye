//! How text output is set up ([`TextOptions`]), and what each level shows.

use anstream::stream::{AsLockedWrite, RawStream};

use super::Text;
use super::limits::LINE_MAX;
use crate::ColorChoice;

/// `-q`: the summary line, and what never hides.
pub(super) const QUIET: i8 = -1;
/// `-v`: bad outcomes at L2.
pub(super) const VERBOSE: i8 = 1;
/// `-vv`: bad outcomes at L3, a line per test.
pub(super) const FULL: i8 = 2;
/// `-vvv`: what the engine did.
pub(super) const ENGINE: i8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct TextOptions {
    /// How much is shown (see the module's docs).
    pub verbosity: crate::Level,
    pub color: ColorChoice,
    /// The output is a terminal a person watches: a long collection shows
    /// a line that says how far it got, rewritten in place and cleared
    /// before anything else is written, and locations are hyperlinks when
    /// colour is on.
    pub live: bool,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            verbosity: crate::Level::Default,
            color: ColorChoice::Auto,
            live: false,
        }
    }
}

/// How much of a bad outcome is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Level {
    L1,
    L2,
    L3,
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    pub(super) const fn quiet(&self) -> bool {
        self.verbosity <= QUIET
    }

    /// From `-vv`: a line per test, every warning, infos, nothing cut.
    pub(super) const fn full(&self) -> bool {
        self.verbosity >= FULL
    }

    pub(super) const fn engine(&self) -> bool {
        self.verbosity >= ENGINE
    }

    /// How much of a bad outcome is shown.
    pub(super) const fn level(&self) -> Level {
        if self.verbosity >= FULL {
            Level::L3
        } else if self.verbosity >= VERBOSE {
            Level::L2
        } else {
            Level::L1
        }
    }

    pub(super) const fn line_max(&self) -> usize {
        if self.full() { usize::MAX } else { LINE_MAX }
    }
}
