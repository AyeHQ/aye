//! What the store keeps about a run beside its log, `r<N>.json`
//! ([`RunMeta`]), and how a file changed since the run.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use aye_events::{ContentDigest, Counts, ExitCode, ProjectPath, RunNumber, StopReason};
use aye_stream::FileChange;
use serde::{Deserialize, Serialize};

use super::StoredRunId;

/// The format of a run's summary file. One of another version is not
/// listed.
pub const VERSION: u32 = 1;

/// What the store keeps about a run beside its log: `r<N>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMeta {
    pub version: u32,
    /// The run's id in the store: `r81` (a run stored before runs were
    /// numbered has its records' 16 hex digits).
    pub run: StoredRunId,
    /// The project root, absolute, from `run_started`.
    pub root: String,
    /// When the store saw `run_started`, in milliseconds since the Unix
    /// epoch.
    pub started_ms: u64,
    /// When the store had the whole log, likewise.
    pub finished_ms: u64,
    /// From `run_finished`; absent when the log has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<ExitCode>,
    #[serde(default)]
    pub counts: Counts,
    /// The run's own duration, from `run_finished`, in whole milliseconds.
    #[serde(default)]
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StopReason>,
    /// What the run's summary counts beside `counts`, by name, the non-zero
    /// ones (`aye_stream::Extra::named`: `skipped_modules`, `setup_error`,
    /// `teardown_error`, `lifetime_error`, `collection_error`,
    /// `usage_error`, `internal_error`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, u32>,
    /// Each file holding a test the run collected, or that collection
    /// reported an error on or skipped, relative to the root as ids spell
    /// it, and the digest of its content when collection finished. A file
    /// that couldn't be read (a directory) is left out. A summary holding a
    /// digest that isn't 32 hex digits doesn't read; aye never wrote one.
    #[serde(default)]
    pub files: BTreeMap<ProjectPath, ContentDigest>,
}

impl RunMeta {
    /// How the file `path` (relative to `root`, as ids spell it) changed
    /// since the run; `None` when it didn't, or when the run recorded no
    /// digest for it.
    pub fn change(&self, root: &Path, path: &str) -> Option<FileChange> {
        let recorded = self.files.get(path)?;
        match fs::read(root.join(path)) {
            Ok(bytes) => (ContentDigest::of(&bytes) != *recorded).then_some(FileChange::Edited),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(FileChange::Gone),
            Err(_) => None,
        }
    }

    /// The run's number: 81 for `r81`; `None` for a run stored under a hex
    /// id.
    pub fn number(&self) -> Option<RunNumber> {
        self.run.number()
    }
}
