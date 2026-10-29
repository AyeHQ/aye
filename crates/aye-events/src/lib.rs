//! The wire types. Everything that crosses a process boundary or an engine
//! boundary is defined here and nowhere else: events, outcomes, structured
//! failures, captured output, the collected `Case` with its tags, requirements,
//! affinity keys and certainty, run metadata, and the schema version.
//!
//! Owns: the schema and its version. No traits, no logic, no I/O.
//!
//! Depends on: serde only. Must not depend on any other aye crate.
//!
//! Design notes (see docs/events.md once written):
//! - A run is its event log; summaries, history rows, run diffs and every
//!   renderer are folds over it. Events are self-contained and bounded.
//! - The outcome set is fixed; annotations such as *flaky* are separate.
//! - Sub-outcomes (subtests, doctest examples) and scope lifecycle events are
//!   first-class.
//! - Removing or renaming a field bumps `SCHEMA_VERSION`.
//!
//! Native features (October 2026): snapshots, test data and fakes are aye
//! features, not companion projects. They need not ship by 0.1.0, but the
//! schema leaves room for them now so that adding them is not a break.
//! - Additions are additive by construction: events are a tagged union and
//!   every consumer ignores kinds it does not know; every struct tolerates
//!   unknown fields and new fields are optional; only removals and renames
//!   bump `SCHEMA_VERSION`.
//! - Sub-outcomes are in the first schema: `subTest` blocks and doctest
//!   examples need them now, snapshots will add one per snapshot.
//! - A source span (file and byte range) exists separately from the test id,
//!   which deliberately has no line: snapshots, the source rewriter and
//!   failure locations all point at spans.
//! - Payloads these features will add: the snapshot (expected value, actual
//!   value, where the expected value lives), the fakes active during a test,
//!   and the seed and frozen time a test ran with.
//! - A hermetic verdict per test exists before the first release that caches
//!   results, even if it only comes from the sandbox tripwire.
//!
//! Open: whether a snapshot with no value yet maps onto an existing outcome
//! or onto an annotation (a new outcome after release would break every
//! consumer that matches on outcomes); whether the reproduce command is a
//! structured object from day one (command plus an open map of inputs such as
//! seed and frozen time) or a string accepted to change later.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Identity.** A [`TestId`] is the canonical path id: the file path
//!   relative to the project root with `/` separators, then `::`-separated
//!   names, then the parameter id in brackets
//!   (`tests/test_cart.py::TestCart::test_add[eur]`). Parameters stay opaque
//!   inside the id; structured parameters, when an adapter knows them, travel
//!   beside it on the [`Case`]. The [`TestKey`] is a stable 64-bit FNV-1a hash
//!   of the id, printed as 16 hex digits.
//! - **One outcome per test.** A test that passes its body and then fails in
//!   teardown is `error`; every failure is kept, each with its [`Phase`].
//! - **A failed setup is reported once.** The failure goes on the
//!   [`Event::LifetimeFinished`] of the lifetime that failed, and each test
//!   that could not run because of it is `error` with `cause` set to that
//!   lifetime instead of a copy of the failure. Adapters that cannot see
//!   lifetimes (pytest) attach the failure to each test, as pytest does.
//! - **The reproduce command is structured** from day one: an argument
//!   vector plus an open map of inputs (seed, frozen time, environment).
//! - **Envelope.** Workers stamp `batch` and a per-worker sequence and time;
//!   the runner stamps `run`, `environment`, `worker` and the run-wide `seq`
//!   as it merges worker streams into the run's log.
//! - **Counts are test results.** [`Counts`] folds `test_finished` alone. A
//!   module the framework skipped while collecting (pytest's
//!   `allow_module_level` skip, `importorskip`) has no test to finish: it is
//!   a `skipped` info [`Diagnostic`] from collection, and the renderers count
//!   it as one skipped item, as pytest's summary and JUnit report do. No
//!   field was needed for it. Nor is a failed lifetime that kept no test
//!   from running among them (a failed `tearDownClass`, `tearDownModule`
//!   or cleanup; a failed setup whose tests the run stopped before under
//!   `maxfail`), though the run fails on it: its `lifetime_finished`
//!   carries the failure, and the renderers count it from there, as a
//!   setup or teardown error (pytest's summary counts it as an error).
//! - **Lifetime events need not pair.** A worker that crashes, or that a
//!   stop kills, sends no `lifetime_finished` for the lifetimes it had
//!   started, so the log may hold none: a consumer must not depend on the
//!   pair. The same lifetime id starts again in each batch that needs it.
//! - **Properties tests record** travel in the stream, for the JUnit report
//!   and any later consumer: a test's own on its `test_finished`
//!   (`properties`, name and value pairs in order: pytest's
//!   `record_property`), and the run's as [`Event::RunProperty`] (pytest's
//!   `record_testsuite_property`), emitted when the test records it. Each
//!   batch is a framework session, so a session-scoped fixture that records
//!   one does so once per batch: consumers keep each pair once.
//! - **A test's body may be timed apart** (`body_duration_ms`, pytest's
//!   `call` phase), for reports that count the body alone (pytest's
//!   `junit_duration_report = call`); `duration_ms` stays setup, body and
//!   teardown.
//! - **A sub-outcome carries its `reason`** when it was skipped or expected
//!   to fail (unittest's `skipTest` inside `subTest`), as a test does.

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
pub const SCHEMA_VERSION: u32 = 1;
