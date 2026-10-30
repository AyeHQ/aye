//! What [`Text`](crate::Text) and [`JsonLines`](crate::JsonLines) are fed
//! ([`RenderMode`]): a run's stream, or, for `aye show`, part of a stored
//! log, with what `aye show` asks of them ([`ShowMode`]) and what it tells
//! them of the stored run beside its log ([`ShowContext`]).

use std::collections::BTreeMap;

use aye_events::{ProjectPath, RunNumber};
use aye_stream::{FileChange, Keys};

/// What [`Text`](crate::Text) and [`JsonLines`](crate::JsonLines) are fed,
/// and so what they make of it.
#[derive(Debug, Clone, Default)]
pub enum RenderMode {
    /// A run's stream, as it comes: the renderer folds the keys from it.
    #[default]
    Run,
    /// For `aye show`: part of a stored log, `mode` saying what to make of
    /// it, with the whole log's `keys` (so a test shows the key it had in
    /// the run) and what the run store knows of the run beside it
    /// (`context`).
    Show {
        mode: ShowMode,
        keys: Keys,
        context: ShowContext,
    },
}

impl RenderMode {
    /// The keys a renderer starts from (a run's fold from none), what
    /// `aye show` asks for (`None` in a run), and what it knows of the run.
    pub(crate) fn into_parts(self) -> (Keys, Option<ShowMode>, ShowContext) {
        match self {
            Self::Run => (Keys::new(), None, ShowContext::default()),
            Self::Show {
                mode,
                keys,
                context,
            } => (keys, Some(mode), context),
        }
    }
}

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
    /// how.
    pub stale: BTreeMap<ProjectPath, FileChange>,
}
