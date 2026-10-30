//! The walk: the directories collection enters and the files it shows
//! each recognizer, in the walk's order (`order.rs`), into the table of
//! files it knows (`tree.rs`).

use std::path::Path;

use aye_events::DiagnosticPath;
use walkdir::WalkDir;

use crate::diagnostic::CollectDiagnostic;

mod order;
mod tree;

pub(crate) use order::{WalkKey, walk_order};
pub(crate) use tree::{AdapterId, FileId, RecognizerSet, Tree};

/// How often a walk reports and asks whether to go on, in entries.
const TICK: u64 = 256;

impl Tree<'_> {
    /// Walk the directory `top` for the recognizers in `mask`, sorted by file
    /// name, a package's `__init__.py` first (see [`walk_order`]). `top`
    /// itself is always entered; below it, a directory is entered for the
    /// recognizers that want it, and a file is shown only to the recognizers
    /// that entered its directory. Every so many entries it calls `tick`,
    /// which reports progress and says whether to go on: `false` ends the
    /// walk there, and `walk` returns `false` too.
    pub(crate) fn walk(
        &mut self,
        top: &Path,
        mask: RecognizerSet,
        tick: &mut dyn FnMut(&Self) -> bool,
    ) -> bool {
        let mut masks = vec![mask];
        let mut walker = WalkDir::new(top)
            .follow_links(true)
            .sort_by(|a, b| walk_order(a.file_name(), b.file_name()))
            .into_iter();
        while let Some(entry) = walker.next() {
            self.walked += 1;
            if self.walked.is_multiple_of(TICK) && !tick(self) {
                return false;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    let missing = err
                        .io_error()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                    // Broken symlinks and symlink loops are skipped quietly.
                    // A directory a recognizer wanted to enter but that can't
                    // be read is an error: pytest fails its collection
                    // (`PermissionError`), and so the run, rather than run
                    // the rest as if it held no tests.
                    if !missing && err.loop_ancestor().is_none() {
                        let path = err.path().unwrap_or(top);
                        self.diagnostics.push(CollectDiagnostic::Unreadable {
                            path: DiagnosticPath::as_written(aye_fs::relative_through_root(
                                path, self.root,
                            )),
                            error: err.to_string(),
                        });
                    }
                    continue;
                }
            };
            let depth = entry.depth();
            if depth == 0 {
                continue;
            }
            let parent = masks[depth - 1];
            if entry.file_type().is_dir() {
                let wanted = self.descend(entry.path(), parent);
                if wanted.is_empty() {
                    walker.skip_current_dir();
                    continue;
                }
                masks.truncate(depth);
                masks.push(wanted);
            } else if entry.file_type().is_file() {
                self.visit(entry.into_path(), parent, false);
            }
        }
        true
    }

    fn descend(&self, dir: &Path, mask: RecognizerSet) -> RecognizerSet {
        mask.iter()
            .filter(|&i| self.recognizers[i].1.descend(dir))
            .collect()
    }
}
