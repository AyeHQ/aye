//! pytest's collection order, followed for one question: which
//! `conftest.py` pytest holds as `conftest` when it collects another one
//! as a doctest module, which is its "import file mismatch".

use std::path::{Path, PathBuf};

use aye_adapter::Recognizer;
use aye_pytest::conftest;

use crate::{DoctestRecognizer, ImportMode};

/// How far pytest's collection gets when [`Walk`] emulates it.
enum Reached {
    Not,
    /// It collects the target, `conftest` being then the file held.
    Target(Option<PathBuf>),
}

/// pytest's collection order, for [`DoctestRecognizer::conftest_mismatch`]:
/// which `conftest.py` it has imported as `conftest` last.
struct Walk<'r> {
    recognizer: &'r DoctestRecognizer,
    target: &'r Path,
    loaded: std::collections::HashSet<PathBuf>,
    current: Option<PathBuf>,
}

impl Walk<'_> {
    /// pytest's `_importconftest`: once per file; one outside a package is
    /// `conftest` from then on.
    fn load(&mut self, conftest: PathBuf) {
        if self.loaded.insert(conftest.clone()) && !conftest::in_package(&conftest) {
            self.current = Some(conftest);
        }
    }

    /// Collect `dir`: its conftests, then its entries by name, depth first.
    fn dir(&mut self, dir: &Path) -> Reached {
        for conftest in self.recognizer.conftests.chain(dir) {
            self.load(conftest);
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Reached::Not;
        };
        let mut entries: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
        for entry in entries {
            if entry.is_dir() {
                if self.recognizer.descend(&entry)
                    && let reached @ Reached::Target(_) = self.dir(&entry)
                {
                    return reached;
                }
            } else if entry == self.target {
                return Reached::Target(self.current.clone());
            }
        }
        Reached::Not
    }
}

impl DoctestRecognizer {
    /// For a `conftest.py` outside a package, collected as a doctest
    /// module: the other `conftest.py` pytest has imported as `conftest`
    /// (`_importconftest`) by the time it collects it, if any, which makes
    /// it pytest's "import file mismatch". pytest imports the initial
    /// conftests first, then collects depth first, entries sorted by name,
    /// importing each directory's conftests before collecting it; a
    /// conftest already imported isn't imported again.
    pub(crate) fn conftest_mismatch(&self, target: &Path) -> Option<PathBuf> {
        if self.conftests.disabled
            || self.ignore_import_mismatch
            || self.config.import_mode == ImportMode::Importlib
            || self.config.consider_namespace_packages
            || conftest::in_package(target)
        {
            return None;
        }
        let mut walk = Walk {
            recognizer: self,
            target,
            loaded: std::collections::HashSet::new(),
            current: None,
        };
        for conftest in self.conftests.initial(&self.roots, &self.cwd) {
            walk.load(conftest);
        }
        for root in &self.roots {
            let reached = if root.is_dir() {
                walk.dir(root)
            } else if root == target {
                for conftest in self.conftests.chain(root.parent().unwrap_or(root)) {
                    walk.load(conftest);
                }
                Reached::Target(walk.current.clone())
            } else {
                Reached::Not
            };
            if let Reached::Target(current) = reached {
                return current.filter(|other| other != target);
            }
        }
        None
    }
}
