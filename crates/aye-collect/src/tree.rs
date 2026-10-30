//! The files collection looks at, and what each recognizer says about them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use aye_adapter::Claim;
use aye_events::{Diagnostic, DiagnosticCode, DiagnosticPath, ProjectPath, Severity};
use walkdir::WalkDir;

use crate::Prepared;

/// A set of recognizers, by priority index.
pub(crate) type Mask = u64;

/// The recognizers in `mask`, highest priority first.
pub(crate) fn bits(mask: Mask) -> impl Iterator<Item = usize> {
    (0..Mask::BITS as usize).filter(move |i| mask & (1 << i) != 0)
}

pub(crate) struct Entry {
    pub abs: PathBuf,
    /// Relative to the project root, `/` separators.
    pub rel: ProjectPath,
    /// Found by the walk, rather than while looking for support files above
    /// it. Only walked files yield tests.
    pub walked: bool,
    /// Named on the command line (or as a root).
    pub explicit: bool,
    /// The recognizers asked about this file, and their answers.
    pub asked: Mask,
    pub tests: Mask,
    pub support: Mask,
}

pub(crate) struct Tree<'r> {
    root: &'r Path,
    recognizers: &'r [Prepared],
    /// In discovery order.
    pub entries: Vec<Entry>,
    index: HashMap<PathBuf, usize>,
    /// The files directly in a directory, sorted by name.
    listings: HashMap<PathBuf, Vec<usize>>,
    /// One recognizer's support files directly in a directory.
    support_in: HashMap<(PathBuf, usize), Vec<usize>>,
    pub diagnostics: Vec<Diagnostic>,
    /// Files and directories the walks went through, for progress.
    pub walked: u64,
    /// Files some recognizer claimed for tests, for progress.
    pub test_files: u64,
}

/// How often a walk reports and asks whether to go on, in entries.
const TICK: u64 = 256;

impl<'r> Tree<'r> {
    pub(crate) fn new(root: &'r Path, recognizers: &'r [Prepared]) -> Self {
        Self {
            root,
            recognizers,
            entries: Vec::new(),
            index: HashMap::new(),
            listings: HashMap::new(),
            support_in: HashMap::new(),
            diagnostics: Vec::new(),
            walked: 0,
            test_files: 0,
        }
    }

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
        mask: Mask,
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
                        self.diagnostics.push(Diagnostic {
                            severity: Severity::Error,
                            code: DiagnosticCode::Unreadable,
                            path: DiagnosticPath::as_written(aye_fs::relative_through_root(
                                path, self.root,
                            )),
                            span: None,
                            message: format!("cannot read: {err}"),
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
                if wanted == 0 {
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

    fn descend(&self, dir: &Path, mask: Mask) -> Mask {
        bits(mask)
            .filter(|&i| self.recognizers[i].1.descend(dir))
            .fold(0, |wanted, i| wanted | 1 << i)
    }

    /// Show the file `abs` to the recognizers in `mask`. A file seen again
    /// keeps its first place; a file named explicitly after being walked is
    /// asked about again with `explicit` set.
    pub(crate) fn visit(&mut self, abs: PathBuf, mask: Mask, explicit: bool) -> usize {
        let at = self.insert(abs);
        let recognizers = self.recognizers;
        let entry = &mut self.entries[at];
        entry.walked = true;
        if explicit && !entry.explicit {
            entry.explicit = true;
            entry.asked = 0;
            entry.tests = 0;
            entry.support = 0;
        }
        let had_tests = entry.tests != 0;
        for i in bits(mask & !entry.asked) {
            entry.claim(
                i,
                recognizers[i].1.claim(entry.rel.as_str(), entry.explicit),
            );
        }
        entry.asked |= mask;
        if !had_tests && entry.tests != 0 {
            self.test_files += 1;
        }
        at
    }

    fn insert(&mut self, abs: PathBuf) -> usize {
        if let Some(&at) = self.index.get(&abs) {
            return at;
        }
        let at = self.entries.len();
        self.entries.push(Entry {
            rel: ProjectPath::as_written(aye_fs::relative_through_root(&abs, self.root)),
            abs: abs.clone(),
            walked: false,
            explicit: false,
            asked: 0,
            tests: 0,
            support: 0,
        });
        self.index.insert(abs, at);
        at
    }

    /// Walked files some recognizer claims for tests, in discovery order.
    pub(crate) fn tests(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&at| self.entries[at].walked && self.entries[at].tests != 0)
            .collect()
    }

    /// Files some recognizer claims as support, in discovery order.
    pub(crate) fn support_files(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&at| self.entries[at].support != 0)
            .collect()
    }

    /// The support files `recognizer` hands along with the file at `at`: its
    /// support files in the file's directory and each one above it up to the
    /// project root, outermost first. A file that is itself a support file
    /// is in its own list (in its place by name), as the framework loads it
    /// as support before collecting it. Directories above the walked roots
    /// are listed here, and their files asked about with `explicit` unset.
    pub(crate) fn support(&mut self, at: usize, recognizer: usize) -> Vec<usize> {
        let Some(dir) = self.entries[at].abs.parent() else {
            return Vec::new();
        };
        let dirs: Vec<PathBuf> = dir
            .ancestors()
            .take_while(|dir| dir.starts_with(self.root))
            .map(Path::to_path_buf)
            .collect();
        let mut chain = Vec::new();
        for dir in dirs.into_iter().rev() {
            chain.extend(self.support_in(dir, recognizer));
        }
        chain
    }

    fn support_in(&mut self, dir: PathBuf, recognizer: usize) -> Vec<usize> {
        let key = (dir, recognizer);
        if let Some(found) = self.support_in.get(&key) {
            return found.clone();
        }
        let bit = 1 << recognizer;
        let mut found = Vec::new();
        for at in self.listing(&key.0) {
            let entry = &mut self.entries[at];
            if entry.asked & bit == 0 {
                let claim = self.recognizers[recognizer]
                    .1
                    .claim(entry.rel.as_str(), entry.explicit);
                // Asked here, a file is only support: its tests belong to
                // the recognizers whose walk reached it.
                if claim.is_support() {
                    entry.support |= bit;
                }
                entry.asked |= bit;
            }
            if entry.support & bit != 0 {
                found.push(at);
            }
        }
        self.support_in.insert(key, found.clone());
        found
    }

    fn listing(&mut self, dir: &Path) -> Vec<usize> {
        if let Some(listing) = self.listings.get(dir) {
            return listing.clone();
        }
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| {
                    kind.is_file() || (kind.is_symlink() && entry.path().is_file())
                })
            })
            .map(|entry| entry.path())
            .collect();
        files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        let listing: Vec<usize> = files.into_iter().map(|abs| self.insert(abs)).collect();
        self.listings.insert(dir.to_path_buf(), listing.clone());
        listing
    }
}

/// The order of a directory's entries in the walk: by name, as pytest's
/// `scandir` sorts them, except that `__init__.py` comes first, as pytest's
/// `Package` collects it (and unittest's discovery loads a package before
/// its contents).
pub(crate) fn walk_order(a: &std::ffi::OsStr, b: &std::ffi::OsStr) -> std::cmp::Ordering {
    let init = |name: &std::ffi::OsStr| name != "__init__.py";
    init(a).cmp(&init(b)).then_with(|| a.cmp(b))
}

impl Entry {
    /// Record `recognizer`'s answer. A file claimed as both tests and
    /// support is in both sets: its tests are collected when it is walked,
    /// and it is support for the files in its directory and below, itself
    /// included.
    fn claim(&mut self, recognizer: usize, claim: Claim) {
        let bit = 1 << recognizer;
        if claim.has_tests() {
            self.tests |= bit;
        }
        if claim.is_support() {
            self.support |= bit;
        }
    }
}
