//! The styles text output is written in, and each outcome's symbol
//! ([`symbol`]).

use anstyle::{AnsiColor, Style};
use aye_events::Outcome;

pub(super) const BOLD: Style = Style::new().bold();
pub(super) const DIM: Style = Style::new().dimmed();
pub(super) const RED: Style = AnsiColor::Red.on_default();
pub(super) const RED_BOLD: Style = AnsiColor::Red.on_default().bold();
pub(super) const GREEN: Style = AnsiColor::Green.on_default();
pub(super) const YELLOW: Style = AnsiColor::Yellow.on_default();
pub(super) const YELLOW_BOLD: Style = AnsiColor::Yellow.on_default().bold();
pub(super) const CYAN: Style = AnsiColor::Cyan.on_default();
pub(super) const KEY: Style = AnsiColor::Magenta.on_default().bold();
pub(super) const SPAN: Style = AnsiColor::Red.on_default().underline();

/// An outcome's symbol and colour: `✗` failed, `!` error, `✓` passed, and
/// none for the rest, which are words alone.
pub(super) const fn symbol(outcome: Outcome) -> (&'static str, Style) {
    match outcome {
        Outcome::Passed => ("✓", GREEN),
        Outcome::Failed => ("✗", RED_BOLD),
        Outcome::Error => ("!", YELLOW_BOLD),
        Outcome::Skipped | Outcome::ExpectedFailure | Outcome::UnexpectedPass => (" ", YELLOW),
    }
}
