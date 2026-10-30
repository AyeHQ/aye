//! Where unittest's discovery goes: where it starts ([`Starts`]), which
//! directories it enters, which files it imports, and which packages decide
//! the tests below them.

use std::path::{Path, PathBuf};

use aye_adapter::Project;
use aye_python_stdlib::fnmatch::fnmatch;
use aye_python_stdlib::os_path::normalize_absolute;
use aye_python_stdlib::unittest::{may_define_load_tests, valid_module_name};

use crate::{SETS, UnittestRecognizer};

/// One start of discovery: the directory it walks (`discover -s`) and the
/// top-level directory its modules are imported from (`discover -t`), both
/// absolute.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveryStart {
    pub(crate) start: PathBuf,
    pub(crate) top: PathBuf,
}

/// Where discovery starts: the configured start directory, and the
/// directories named on the command line outside it, each a start of its
/// own (its own top-level directory when outside the configured one).
#[derive(Debug)]
pub(crate) struct Starts {
    pub(crate) configured: DiscoveryStart,
    pub(crate) named: Vec<DiscoveryStart>,
}

impl Starts {
    /// The configured start, and the starts `project`'s paths add: a
    /// directory outside it, unless it is inside one already added (one
    /// that holds added ones takes their place).
    pub(crate) fn new(configured: DiscoveryStart, project: &Project<'_>) -> Self {
        let mut named: Vec<DiscoveryStart> = Vec::new();
        for arg in project.args {
            let path = normalize_absolute(&project.cwd.join(arg));
            // A directory holding the start directory walks it; one inside
            // it narrows discovery from it.
            if !path.is_dir()
                || configured.start.starts_with(&path)
                || path.starts_with(&configured.start)
            {
                continue;
            }
            if named.iter().any(|s| path.starts_with(&s.start)) {
                continue;
            }
            named.retain(|s| !s.start.starts_with(&path));
            let top = if path.starts_with(&configured.top) {
                configured.top.clone()
            } else {
                path.clone()
            };
            named.push(DiscoveryStart { start: path, top });
        }
        Self { configured, named }
    }

    /// Every start, the configured one first.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &DiscoveryStart> {
        std::iter::once(&self.configured).chain(&self.named)
    }

    /// The start discovery reaches `path` from: the configured one, else
    /// the outermost named directory outside it.
    pub(crate) fn start_for(&self, path: &Path) -> Option<&DiscoveryStart> {
        let configured = &self.configured;
        if path.starts_with(&configured.start) && path != configured.start {
            return Some(configured);
        }
        self.named
            .iter()
            .filter(|s| path.starts_with(&s.start) && path != s.start)
            .min_by_key(|s| s.start.components().count())
    }
}

pub(crate) fn is_package(dir: &Path) -> bool {
    dir.join("__init__.py").is_file()
}

impl UnittestRecognizer {
    /// The files named on the command line that a package that may define
    /// `load_tests` would otherwise answer for: the executor loads each as
    /// a module alone.
    pub(crate) fn named_below_load_tests(&self) -> Vec<&PathBuf> {
        self.named_files
            .iter()
            .filter(|path| self.taken_over_by(path).is_some())
            .collect()
    }

    /// Whether discovery from `start` imports `path`: a valid module name
    /// matching the pattern, or a package's `__init__.py` (not the top
    /// level's), in a directory reached through packages.
    fn discoverable(&self, path: &Path, start: &Path, top: &Path) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if name == "__init__.py" {
            if dir == top || !dir.starts_with(start) {
                return false;
            }
        } else if !(valid_module_name(&name) && fnmatch(&name, &self.config.pattern.value, SETS)) {
            return false;
        }
        dir.starts_with(start) && dir.ancestors().take_while(|d| *d != start).all(is_package)
    }

    fn takes_over(&self, package: &Path) -> bool {
        if let Some(known) = self.takes_over.lock().expect("lock").get(package) {
            return *known;
        }
        let found = std::fs::read(package.join("__init__.py"))
            .is_ok_and(|bytes| may_define_load_tests(&bytes));
        self.takes_over
            .lock()
            .expect("lock")
            .insert(package.to_path_buf(), found);
        found
    }

    /// The outermost package between the start directory and `path`'s
    /// directory whose `__init__.py` may define `load_tests`: the package
    /// whose discovery decides `path`'s tests.
    pub(crate) fn taken_over_by(&self, path: &Path) -> Option<PathBuf> {
        let DiscoveryStart { start, top } = self.starts.start_for(path)?;
        if !self.discoverable(path, start, top) {
            return None;
        }
        let dir = path.parent()?;
        let mut chain: Vec<&Path> = dir
            .ancestors()
            .take_while(|d| d.starts_with(start))
            .collect();
        chain.reverse();
        chain
            .into_iter()
            .find(|d| *d != top && is_package(d) && self.takes_over(d))
            .map(Path::to_path_buf)
    }

    /// The outermost package between the start directory and `path` whose
    /// name has a dot: discovery can't import it by its dotted name, so it
    /// reports one import error and loads nothing below it.
    pub(crate) fn unimportable(&self, path: &Path) -> Option<PathBuf> {
        let DiscoveryStart { start, top } = self.starts.start_for(path)?;
        if !self.discoverable(path, start, top) {
            return None;
        }
        let dir = path.parent()?;
        let mut chain: Vec<&Path> = dir
            .ancestors()
            .take_while(|d| d.starts_with(start))
            .collect();
        chain.reverse();
        chain
            .into_iter()
            .find(|d| {
                *d != top
                    && d.file_name()
                        .is_some_and(|name| name.to_string_lossy().contains('.'))
            })
            .map(Path::to_path_buf)
    }
}

/// Whether a package's `__init__.py` may bind a test class: it defines a
/// class, imports, or may define `load_tests`. Unreadable files are claimed,
/// for collection to report.
pub(crate) fn may_hold_tests(init: &Path) -> bool {
    let Ok(bytes) = std::fs::read(init) else {
        return true;
    };
    let has = |word: &[u8]| bytes.windows(word.len()).any(|w| w == word);
    has(b"class") || has(b"import") || may_define_load_tests(&bytes)
}
