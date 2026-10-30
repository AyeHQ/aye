//! What `aye show` asks of [`Text`](crate::Text) and
//! [`JsonLines`](crate::JsonLines) ([`ShowMode`]), and what it tells them of
//! the stored run beside its log ([`ShowContext`]).

use std::collections::BTreeMap;

use aye_events::{ProjectPath, RunNumber};

/// What [`Text`](crate::Text) and [`JsonLines`](crate::JsonLines) make of a
/// stored run's records, for `aye show`, which feeds them part of a log
/// with the whole log's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowMode {
    /// One line per `test_finished` and the summary: text's result line
    /// (a bad outcome's with its key and why), JSON's `test` line.
    List,
    /// The tests as the run showed them, at the verbosity asked for; in
    /// text, a test that didn't fail has its result line.
    Details,
}

/// What `aye show` knows of a stored run beside its log, for the details'
/// headers: which run it is, how long ago it finished, and which of its
/// files changed since.
#[derive(Debug, Clone, Default)]
pub struct ShowContext {
    /// The run's number (`r81`).
    pub run: Option<RunNumber>,
    /// How long ago the run finished: `2 min ago`.
    pub ago: Option<String>,
    /// The files, as ids spell them, that changed since the run, each with
    /// whether it is gone.
    pub stale: BTreeMap<ProjectPath, bool>,
}
