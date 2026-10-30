//! What the store's directory holds, by name: each run's log and summary,
//! `last`, and the temporary files they are written through.

use std::io::{self, Write};
use std::path::Path;

/// The store's directory, inside aye's state directory.
pub const DIR: &str = aye_fs::StateBucket::Runs.name();

/// The file naming the run that finished last.
pub const LAST: &str = "last";

/// After a run's id: its log.
pub(super) const LOG: &str = ".jsonl.gz";
/// After a run's id: its summary.
pub(super) const META: &str = ".json";
/// Before a temporary file's name.
pub(super) const TEMPORARY: &str = ".tmp-";

/// Writes `name` in `dir` whole or not at all (`aye_fs::atomic_write`).
pub(super) fn replace(dir: &Path, name: &str, content: &[u8]) -> io::Result<()> {
    aye_fs::atomic_write(dir, name, TEMPORARY, |file| {
        file.write_all(content)?;
        file.flush()
    })
}

/// A run's id as the runner makes them (16 hex digits), or anything as
/// harmless in a file name: letters, digits, `-` and `_`.
pub(super) fn is_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
