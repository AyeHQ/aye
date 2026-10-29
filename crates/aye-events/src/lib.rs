//! The wire types. Everything that crosses a process boundary or an engine
//! boundary is defined here and nowhere else: events, outcomes, structured
//! failures, captured output, the collected `Case` with its tags, requirements,
//! affinity keys and certainty, run metadata, and the schema version.
//!
//! A run is its event log: summaries, history rows, run diffs and every
//! renderer are folds over it. Events are self-contained and bounded, and
//! sub-outcomes (subtests, doctest examples) and lifetimes are first-class.
//! `docs/events.md` describes the log on the wire; each type here says what
//! it decides.
//!
//! Native features (October 2026): snapshots, test data and fakes are aye
//! features, not companion projects. They need not ship by 0.1.0, but the
//! schema leaves room for them now so that adding them is not a break. They
//! will add the snapshot (expected value, actual value, where the expected
//! value lives), the fakes active during a test, and the seed and frozen
//! time a test ran with. A hermetic verdict per test exists before the first
//! release that caches results, even if it only comes from the sandbox
//! tripwire.
//!
//! Owns: the schema and its version. No traits, no logic, no I/O.
//!
//! Depends on: serde only. Must not depend on any other aye crate.

mod case;
mod diagnostic;
mod event;
mod failure;
mod id;
mod outcome;
mod path;
mod result;
mod serde_ext;

pub use case::{Case, Certainty};
pub use diagnostic::{Diagnostic, Severity};
pub use event::{Envelope, Event, Record};
pub use failure::{Assertion, Compare, Difference, Failure, Frame, Side};
pub use id::{BatchId, EnvironmentId, LifetimeId, TestId, TestKey, WorkerId};
pub use outcome::{ExitCode, Outcome, Phase};
pub use path::{Location, Span};
pub use result::{Captured, Counts, Previous, Reproduce, Resource, TestResult};

/// Bumped only when a field is removed or renamed, or its meaning changes.
///
/// Additions are additive by construction: events are a tagged union and
/// every consumer ignores kinds it doesn't know; every struct tolerates
/// unknown fields, and new fields are optional.
pub const SCHEMA_VERSION: u32 = 1;
