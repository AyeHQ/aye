//! The wire types. Everything that crosses a process boundary or an engine
//! boundary is defined here and nowhere else: events, outcomes, structured
//! failures, captured output, the collected `Case` with its tags, requirements,
//! affinity keys and certainty, run metadata, and the schema version; the
//! identities and paths they hold, each taken apart in one place: the test id
//! and its parts, its key, an adapter's name, a lifetime's id and the scope it
//! opens, a case's affinity keys, the project-relative path, and what a
//! diagnostic and a frame point at; and the diagnostic codes, with what an
//! error means for the exit code, the summary and the run store.
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
//! Owns: the schema and its version, and what an error diagnostic means for
//! a run (`diagnostic/classify.rs`). No traits, no I/O.
//!
//! Depends on: serde, and aye-index for the index types. Must not depend on
//! any other aye crate.

mod affinity;
mod case;
mod diagnostic;
mod event;
mod failure;
mod id;
mod lifetime;
mod outcome;
mod path;
mod result;
mod run;
mod serde_ext;

pub use affinity::{AffinityKey, AffinityKind};
pub use case::{Case, Certainty};
pub use diagnostic::{
    Diagnostic, DiagnosticCode, DiagnosticOrigin, ErrorClass, Severity, SummaryKind,
};
pub use event::{Envelope, Event, Record};
pub use failure::{Assertion, Columns, Compare, Difference, Failure, Frame, Local, Side};
pub use id::{AdapterName, InvalidKey, KeyPrefix, TestId, TestIdParts, TestKey};
pub use lifetime::{LifetimeId, Scope};
pub use outcome::{ExitCode, Outcome, Phase};
pub use path::{
    DiagnosticPath, FramePath, InvalidProjectPath, LineNumber, Location, ProjectPath,
    ProjectPathRef, Span,
};
pub use result::{Captured, Counts, Previous, Reproduce, Resource, TestResult};
pub use run::{
    BatchIndex, EnvironmentId, InvalidRunNumber, RunId, RunNumber, Stamper, WorkerIndex,
};

/// Bumped only when a field is removed or renamed, or its meaning changes.
///
/// Additions are additive by construction: events are a tagged union and
/// every consumer ignores kinds it doesn't know; every struct tolerates
/// unknown fields, and new fields are optional.
pub const SCHEMA_VERSION: u32 = 1;
