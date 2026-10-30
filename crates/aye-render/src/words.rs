//! The words the renderers share: text cut to a length ([`head`],
//! [`tail`], [`last_lines`]), counts of things ([`plural`]), durations
//! ([`spaced`], [`seconds`]), a test's history ([`history`]), and an
//! outcome's and a phase's names ([`outcome_words`], [`phase`]).

mod duration;
mod history;
mod plural;
mod truncate;

use aye_events::{Outcome, Phase};

pub(crate) use duration::{millis, seconds, secs, spaced};
pub(crate) use history::history;
pub(crate) use plural::plural;
pub(crate) use truncate::{head, last_lines, tail};

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

pub(crate) const fn phase(phase: Phase) -> &'static str {
    match phase {
        Phase::Setup => "setup",
        Phase::Test => "test",
        Phase::Teardown => "teardown",
    }
}
