//! What text, JSON lines, JUnit and GitHub say of a failure alike: why it
//! went bad, in one line ([`why()`]); which operand was expected
//! ([`whole`], [`entries`]); its message and the rest of it, and its
//! explanation ([`message()`], [`message_rest`], [`explanation()`]); the
//! resource that failed ([`resource_name`]); where it is shown and its
//! frames ([`origin`], [`frame_items`]); and where it happened.
//!
//! The adapters' text rules are read here, each where it is documented:
//! the rest of a message after pytest's `E` marker or Python's `Type: `
//! ([`message_rest`]), unittest's `first != second : msg` and the doctest
//! adapter's `<source>: expected …` ([`own_message`]), pytest's advice
//! about its `-v` flags ([`diff_lines`]), a doctest's lone hunk header
//! ([`explanation()`]), and the fixture pytest makes for `setup_module`
//! ([`resource_name`]). They stay in the renderers for 0.1.0, so that a
//! log 0.0.1 stored reads the same.

mod explanation;
mod frames;
mod message;
mod oriented;
mod resource;
mod why;

use aye_events::{Failure, Phase};

pub(crate) use explanation::{diff_lines, explanation, only_restates, operands_add};
pub(crate) use frames::{FrameItem, frame_items, frame_line, locals, origin, others, pointed};
pub(crate) use message::{message, message_rest, own_message};
pub(crate) use oriented::{entries, first_line, sources, whole};
pub(crate) use resource::{resource_name, setup_chain};
pub(crate) use why::{bare_why, why};

use crate::words;

/// Where a failure happened, in words: `setup`, `the test`, `teardown`.
pub(crate) fn phase_words(failure: Option<&Failure>) -> Option<&'static str> {
    failure.map(|failure| match failure.phase {
        Some(Phase::Setup) => "setup",
        Some(Phase::Teardown) => "teardown",
        Some(Phase::Test) | None => "the test",
    })
}

/// The phase worth naming: setup or teardown, not the test itself.
pub(crate) fn notable_phase(failure: &Failure) -> Option<&'static str> {
    failure
        .phase
        .filter(|phase| *phase != Phase::Test)
        .map(words::phase)
}

#[cfg(test)]
mod tests;
