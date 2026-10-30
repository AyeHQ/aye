//! The files collection looks at ([`FileId`]), and what each recognizer
//! ([`AdapterId`]) says about them.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use aye_adapter::{Claim, Requested};
use aye_events::ProjectPath;
use aye_index::{IndexSlice, IndexVec, newtype_index};

use crate::Prepared;
use crate::diagnostic::CollectDiagnostic;

newtype_index! {
    /// A file collection knows: its place in the table of files, which is
    /// the order collection found them in.
    pub(crate) struct FileId;
}

newtype_index! {
    /// A prepared recognizer: its place among them, which is its adapter's
    /// priority. The lowest comes first: when two adapters give a test the
    /// same id, the first one's case is kept.
    pub(crate) struct AdapterId;
}

/// A set of the prepared recognizers, at most [`RecognizerSet::CAPACITY`]
/// of them, as one bit each; it goes through them in priority order.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RecognizerSet(u64);

impl RecognizerSet {
    /// How many recognizers a set can hold.
    pub(crate) const CAPACITY: usize = u64::BITS as usize;

    /// The set of `recognizer` alone.
    pub(crate) const fn of(recognizer: AdapterId) -> Self {
        Self(1 << recognizer.as_u32())
    }

    pub(crate) const fn contains(self, recognizer: AdapterId) -> bool {
        self.0 & Self::of(recognizer).0 != 0
    }

    pub(crate) const fn insert(&mut self, recognizer: AdapterId) {
        self.0 |= Self::of(recognizer).0;
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The recognizers in either set.
    pub(crate) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The recognizers in this set but not in `other`.
    pub(crate) const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    /// The recognizers in it, highest priority first.
    pub(crate) fn iter(self) -> impl Iterator<Item = AdapterId> {
        (0..u64::BITS)
            .filter(move |&i| self.0 & (1 << i) != 0)
            .map(AdapterId::from_u32)
    }
}

impl FromIterator<AdapterId> for RecognizerSet {
    fn from_iter<I: IntoIterator<Item = AdapterId>>(recognizers: I) -> Self {
        recognizers
            .into_iter()
            .fold(Self::default(), |set, recognizer| {
                set.union(Self::of(recognizer))
            })
    }
}

impl fmt::Debug for RecognizerSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

pub(crate) struct Entry {
    pub abs: PathBuf,
    /// Relative to the project root, `/` separators.
    pub rel: ProjectPath,
    /// Found by the walk, rather than while looking for support files above
    /// it. Only walked files yield tests.
    pub walked: bool,
    /// Named on the command line (or as a root): see [`Entry::requested`].
    pub explicit: bool,
    /// The recognizers asked about this file, and their answers.
    pub asked: RecognizerSet,
    pub tests: RecognizerSet,
    pub support: RecognizerSet,
}

pub(crate) struct Tree<'r> {
    pub(super) root: &'r Path,
    pub(super) recognizers: IndexSlice<'r, AdapterId, Prepared>,
    /// In discovery order.
    pub entries: IndexVec<FileId, Entry>,
    index: HashMap<PathBuf, FileId>,
    /// The files directly in a directory, sorted by name.
    listings: HashMap<PathBuf, Vec<FileId>>,
    /// One recognizer's support files directly in a directory.
    support_in: HashMap<(PathBuf, AdapterId), Vec<FileId>>,
    pub diagnostics: Vec<CollectDiagnostic>,
    /// Files and directories the walks went through, for progress.
    pub walked: u64,
    /// Files some recognizer claimed for tests, for progress.
    pub test_files: u64,
}

impl<'r> Tree<'r> {
    pub(crate) fn new(root: &'r Path, recognizers: IndexSlice<'r, AdapterId, Prepared>) -> Self {
        Self {
            root,
            recognizers,
            entries: IndexVec::new(),
            index: HashMap::new(),
            listings: HashMap::new(),
            support_in: HashMap::new(),
            diagnostics: Vec::new(),
            walked: 0,
            test_files: 0,
        }
    }

    /// Show the file `abs` to the recognizers in `mask`. A file seen again
    /// keeps its first place; a file named explicitly after being walked is
    /// asked about again, as [`Requested::Named`].
    pub(crate) fn visit(&mut self, abs: PathBuf, mask: RecognizerSet, explicit: bool) -> FileId {
        let at = self.insert(abs);
        let recognizers = self.recognizers;
        let entry = &mut self.entries[at];
        entry.walked = true;
        if explicit && !entry.explicit {
            entry.explicit = true;
            entry.asked = RecognizerSet::default();
            entry.tests = RecognizerSet::default();
            entry.support = RecognizerSet::default();
        }
        let had_tests = !entry.tests.is_empty();
        for i in mask.without(entry.asked).iter() {
            entry.claim(
                i,
                recognizers[i]
                    .1
                    .claim(entry.rel.as_str(), entry.requested()),
            );
        }
        entry.asked = entry.asked.union(mask);
        if !had_tests && !entry.tests.is_empty() {
            self.test_files += 1;
        }
        at
    }

    fn insert(&mut self, abs: PathBuf) -> FileId {
        if let Some(&at) = self.index.get(&abs) {
            return at;
        }
        let at = self.entries.push(Entry {
            rel: ProjectPath::as_written(aye_fs::relative_through_root(&abs, self.root)),
            abs: abs.clone(),
            walked: false,
            explicit: false,
            asked: RecognizerSet::default(),
            tests: RecognizerSet::default(),
            support: RecognizerSet::default(),
        });
        self.index.insert(abs, at);
        at
    }

    /// Walked files some recognizer claims for tests, in discovery order.
    pub(crate) fn tests(&self) -> Vec<FileId> {
        self.entries
            .iter_enumerated()
            .filter(|(_, entry)| entry.walked && !entry.tests.is_empty())
            .map(|(at, _)| at)
            .collect()
    }

    /// Files some recognizer claims as support, in discovery order.
    pub(crate) fn support_files(&self) -> Vec<FileId> {
        self.entries
            .iter_enumerated()
            .filter(|(_, entry)| !entry.support.is_empty())
            .map(|(at, _)| at)
            .collect()
    }

    /// The support files `recognizer` hands along with the file at `at`: its
    /// support files in the file's directory and each one above it up to the
    /// project root, outermost first. A file that is itself a support file
    /// is in its own list (in its place by name), as the framework loads it
    /// as support before collecting it. Directories above the walked roots
    /// are listed here, and their files asked about as found unless they
    /// were named.
    pub(crate) fn support(&mut self, at: FileId, recognizer: AdapterId) -> Vec<FileId> {
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

    fn support_in(&mut self, dir: PathBuf, recognizer: AdapterId) -> Vec<FileId> {
        let key = (dir, recognizer);
        if let Some(found) = self.support_in.get(&key) {
            return found.clone();
        }
        let mut found = Vec::new();
        for at in self.listing(&key.0) {
            let entry = &mut self.entries[at];
            if !entry.asked.contains(recognizer) {
                let claim = self.recognizers[recognizer]
                    .1
                    .claim(entry.rel.as_str(), entry.requested());
                // Asked here, a file is only support: its tests belong to
                // the recognizers whose walk reached it.
                if claim.is_support() {
                    entry.support.insert(recognizer);
                }
                entry.asked.insert(recognizer);
            }
            if entry.support.contains(recognizer) {
                found.push(at);
            }
        }
        self.support_in.insert(key, found.clone());
        found
    }

    fn listing(&mut self, dir: &Path) -> Vec<FileId> {
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
        let listing: Vec<FileId> = files.into_iter().map(|abs| self.insert(abs)).collect();
        self.listings.insert(dir.to_path_buf(), listing.clone());
        listing
    }
}

impl Entry {
    /// How the recognizers are asked about the file, and told about it when
    /// they recognize it: named once, always named.
    pub(crate) const fn requested(&self) -> Requested {
        if self.explicit {
            Requested::Named
        } else {
            Requested::Found
        }
    }

    /// Record `recognizer`'s answer. A file claimed as both tests and
    /// support is in both sets: its tests are collected when it is walked,
    /// and it is support for the files in its directory and below, itself
    /// included.
    fn claim(&mut self, recognizer: AdapterId, claim: Claim) {
        if claim.has_tests() {
            self.tests.insert(recognizer);
        }
        if claim.is_support() {
            self.support.insert(recognizer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AdapterId, RecognizerSet};

    #[test]
    fn a_set_of_recognizers_goes_through_them_in_priority_order() {
        let [first, sixth, last] = [0, 5, 63].map(AdapterId::from_u32);
        let set: RecognizerSet = [last, first, sixth, first].into_iter().collect();
        assert_eq!(set.iter().collect::<Vec<_>>(), [first, sixth, last]);
        assert!(set.contains(sixth));
        assert!(!set.contains(AdapterId::from_u32(1)));
        assert_eq!(
            set.without(RecognizerSet::of(sixth))
                .iter()
                .collect::<Vec<_>>(),
            [first, last]
        );
        let mut other = RecognizerSet::default();
        assert!(other.is_empty());
        other.insert(AdapterId::from_u32(1));
        assert_eq!(set.union(other).iter().count(), 4);
        assert_eq!(
            format!("{set:?}"),
            "{AdapterId(0), AdapterId(5), AdapterId(63)}"
        );
    }
}
