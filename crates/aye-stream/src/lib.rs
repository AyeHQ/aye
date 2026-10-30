//! Folds over the event stream that aren't presentation: what consumes the
//! log and how one stream reaches several, and what the renderers, the
//! run store and `aye show` each derive from it the same way, so that each
//! keeps only what it shows and how.
//!
//! Owns: the [`Consumer`] trait, [`Tee`] and the two policies for a
//! consumer that fails ([`StopAfterError`], [`FailOnFinish`]); test keys
//! ([`Keys`], [`ShortKey`]); what the summaries count beside the tests
//! ([`Tally`], [`Extra`]); a test's bad sub-outcomes held until it finishes
//! ([`SubOutcomes`], with which of its own failures repeat theirs:
//! [`Repeats`]); tests a failed lifetime kept from running
//! ([`BlockedTests`], with whether a lifetime is shown again: [`Again`]);
//! the first failure of each signature ([`FirstBySignature`]); the line
//! each collected test is defined on ([`DefinitionLines`]); and the run's
//! facts ([`RunFacts`]). Where the renderers differ (JUnit keeps a test's bare
//! failure, text and JSON lines show a lifetime that fails again in their
//! own ways, the order of what is left at the end), each says which rule it
//! takes. Does not own: words, layout or limits, which are aye-render's,
//! nor history or the run store, which are aye-history's.
//!
//! Depends on: aye-events.
//!
//! Decided (implementation, October 2026):
//! - **The fold.** A [`Consumer`] receives every [`Record`](aye_events::Record) in log order and
//!   is told when the stream ends ([`Consumer::finish`]); [`Tee`] fans one
//!   stream out to several and keeps going when one of them fails, returning
//!   the first error. Records of kinds a consumer does not know are
//!   ignored. A consumer on stdout stops writing at its first error (a
//!   closed pipe), so it is reported once ([`StopAfterError`]); a report
//!   written to a file at the end says its error, naming the file, and
//!   fails the run, though the stream's end goes on ([`FailOnFinish`]).
//! - **Keys** ([`Keys`]): every test and lifetime is named by its key, the
//!   start of its `TestKey` (5 hex digits, more when an earlier id starts
//!   the same way), a fold over the log alone, so a run's lines and `aye
//!   show` later, given the whole log's keys, name a test alike. Tests and
//!   lifetimes share one key space ([`Subject`]).
//! - **What the summaries count beside the tests** (modules skipped while
//!   collecting, failed lifetimes that kept no test from running, error
//!   diagnostics by kind) is one fold, [`Tally`], which both summaries
//!   keep, and which the binary keeps beside a stored run ([`Extra`], which
//!   aye-render words for `aye show --runs`).

mod blocked;
mod consumer;
mod facts;
mod keys;
mod lines;
mod signatures;
mod subs;
mod tally;

pub use blocked::{Again, BlockedTests, Left};
pub use consumer::{Consumer, FailOnFinish, StopAfterError, Tee};
pub use facts::RunFacts;
pub use keys::{Keys, ShortKey, Subject};
pub use lines::DefinitionLines;
pub use signatures::FirstBySignature;
pub use subs::{HeldSub, Repeats, SubOutcomes, is_bare, own_failures};
pub use tally::{Extra, ModuleSkips, Noted, Tally, module_skip};
