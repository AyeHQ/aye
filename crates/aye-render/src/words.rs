//! The words the renderers share: text cut to a length (`head`, `tail`,
//! `last_lines`), counts of things (`plural`), durations (`spaced`,
//! `seconds`), a test's history (`history`), an outcome's and a phase's
//! names (`outcome_words`, `phase`), and the times `aye show` says, which
//! the binary's printer reuses ([`utc`], [`rfc3339`], [`ago`]).

mod duration;
mod history;
mod plural;
mod time;
mod truncate;

use aye_events::{Outcome, Phase};

pub(crate) use duration::{millis, seconds, secs, spaced};
pub(crate) use history::history;
pub(crate) use plural::plural;
pub use time::{ago, rfc3339, utc};
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
