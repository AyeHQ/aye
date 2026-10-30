//! What the store keeps of the runs it holds: the runs that finished last,
//! and what runs that never completed left, until it is a day old.

use std::fs;
use std::io;
use std::time::{Duration, SystemTime};

use super::layout::{LAST, LOG, META, TEMPORARY};
use super::{RunStore, StoredRunId};

/// How old a log without a summary, or a temporary file, must be before
/// it is deleted: older than any run still writing it.
const LEFT_OVER: Duration = Duration::from_hours(24);

impl RunStore {
    /// Keeps the `keep` runs that finished last, and `current`, deleting
    /// the others, and what is left over from runs that never completed
    /// (logs without a summary, temporary files) once a day old. With
    /// `keep` 0, `last` goes too.
    pub(super) fn retain(&self, keep: usize, current: Option<&StoredRunId>) -> io::Result<()> {
        if !self.dir.is_dir() {
            return Ok(());
        }
        for meta in self.runs().iter().skip(keep) {
            if Some(&meta.run) == current {
                continue;
            }
            // The log first: a summary without its log is deleted next
            // time, a log without its summary only once it is old.
            match fs::remove_file(self.dir.join(format!("{}{LOG}", meta.run))) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            let _ = fs::remove_file(self.dir.join(format!("{}{META}", meta.run)));
        }
        let now = SystemTime::now();
        for entry in fs::read_dir(&self.dir)?.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let left_over = match name.strip_suffix(LOG) {
                Some(id) => !self.dir.join(format!("{id}{META}")).exists(),
                None => name.starts_with(TEMPORARY),
            };
            let old = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > LEFT_OVER);
            if left_over && old {
                let _ = fs::remove_file(entry.path());
            }
        }
        if keep == 0 {
            match fs::remove_file(self.dir.join(LAST)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}
