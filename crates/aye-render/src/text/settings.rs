//! How text output is set up ([`TextOptions`]), and what each level shows.

use anstream::stream::{AsLockedWrite, RawStream};

use super::Text;
use super::limits::LINE_MAX;
use crate::{ColorChoice, DetailLevel, Ladder, Level};

#[derive(Debug, Clone, Copy)]
pub struct TextOptions {
    /// How much is shown (see the module's docs).
    pub verbosity: Level,
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
            verbosity: Level::Default,
            color: ColorChoice::Auto,
            live: false,
        }
    }
}

impl<W: RawStream + AsLockedWrite> Text<W> {
    /// `-q`: the summary line, and what never hides.
    pub(super) fn quiet(&self) -> bool {
        self.verbosity == Level::Quiet
    }

    /// From `-vv`: a line per test, every warning, infos, nothing cut.
    pub(super) fn full(&self) -> bool {
        self.verbosity >= Level::VeryVerbose
    }

    /// `-vvv`: what the engine did.
    pub(super) fn engine(&self) -> bool {
        self.verbosity >= Level::Engine
    }

    /// How much of a bad outcome is shown ([`Ladder::TEXT`]); at `-q`,
    /// which shows none, L1.
    pub(super) fn level(&self) -> DetailLevel {
        Ladder::TEXT
            .detail(self.verbosity)
            .unwrap_or(DetailLevel::L1)
    }

    pub(super) fn line_max(&self) -> usize {
        if self.full() { usize::MAX } else { LINE_MAX }
    }
}
