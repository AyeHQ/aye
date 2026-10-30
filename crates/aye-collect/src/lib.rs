//! Discovery infrastructure. Walks the tree, parses files with Ruff's parser,
//! hands each file and its AST to every registered recognizer, merges the
//! results, runs the refinement protocol for files any adapter marked
//! runtime-dependent, caches refinement by file content hash, and emits
//! diagnostics (duplicate definitions, uncollected tests, files with no tests)
//! with the strict-mode policy.
//!
//! Owns: the mechanism of discovery. Does not own: what counts as a test, which
//! is each adapter's recognizer. If a framework's name pattern or id algorithm
//! appears in this crate, the boundary has leaked.
//!
//! Depends on: aye-adapter (the recognizers it drives, and Ruff's syntax
//! tree), aye-events, aye-fs (paths, and marking the state directory),
//! aye-protocol (refinement's messages and the `Refiner` it asks),
//! aye-provision (the environment's fingerprint, which the cache's key
//! holds: collection reads the interpreter only through it). Must not
//! depend on any `aye-adapter-*`.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Stopping and progress.** [`Collector::with_stop`] takes a flag the
//!   binary's Ctrl-C handler sets: the walk checks it every 256 entries,
//!   parsing before each file, and collection again after refinement (whose
//!   refiner stops on its own), returning a [`Collection`] marked
//!   `interrupted`. [`Collector::collect_watching`] reports [`Progress`] to
//!   the caller's thread at the same ticks, when the walk ends, and every
//!   50ms while refinement runs on another thread (so a refiner must be
//!   `Sync`); pacing what is shown is the caller's business.
//! - **Roots** are the explicit paths when any are given (relative ones are
//!   taken from the project's cwd, and `.` and `..` are resolved lexically,
//!   as Python's `os.path.abspath` does; one spelled through another name of
//!   a directory in the project, as macOS's `/tmp` is `/private/tmp`, is
//!   spelled from the root), otherwise the union of every recognizer's
//!   roots, each walked only for the recognizers that listed it. Like
//!   pytest's `normalize_collection_arguments`, an existing explicit path
//!   below another explicit directory, or repeating an earlier one, is
//!   dropped, so the files keep the order of the path that subsumes them.
//!   A root is always entered, whatever `descend` says. A missing explicit
//!   path is a `not-found` error; a missing recognizer root is skipped. A file
//!   given as a root is claimed, and recognized, as `Requested::Named`; an
//!   explicit file no recognizer claims is an `unclaimed` error, as
//!   pytest's "found no collectors" is.
//! - **The walk** is depth-first and sorted by file name, directories and
//!   files interleaved, as pytest's `scandir` is, except that a package's
//!   `__init__.py` comes first in its directory, as pytest's `Package`
//!   collects it (so its doctests come first, as in `--doctest-modules`) and
//!   unittest's discovery loads a package before its contents. Symlinks are
//!   followed; broken links and loops are skipped. A directory a
//!   recognizer wants to enter but that can't be read is an `unreadable`
//!   error, as pytest fails its collection with a `PermissionError` (exit
//!   2); one no recognizer enters is never read. Below a root, a directory is entered
//!   for the recognizers whose `descend` accepts it and its parents, and a
//!   file is shown only to those. A file reached twice keeps its first place.
//!   A file yields tests only for the recognizers whose walk reached it (or
//!   that it was named for).
//! - **Every file is offered to `claim`**, but only `.py` files are parsed. A
//!   claimed file of any other kind (a doctest text file) reaches `recognize`
//!   with its text and an empty module.
//! - **Support files** for a test file are its recognizer's support files
//!   (claimed `Support` or `TestsAndSupport`) in the file's directory and
//!   each directory above it up to the project root, outermost first, by
//!   name within a directory. Directories above the walked roots are listed
//!   for this; a file found only that way is support, never a test file. A
//!   file outside the project root gets none. Every claimed support file is
//!   parsed once, even when no test file uses it, so its syntax errors
//!   surface as they would when the framework loads it.
//! - **A file claimed `TestsAndSupport`** (pytest's `conftest.py` when it
//!   also matches `python_files`) is both. Parsed once, with the support
//!   files, it is support for the files in its directory and below and,
//!   when walked or named, a test file recognized like any other. It is in
//!   its own support files (in its place by name), since the framework
//!   loads it as support before collecting its tests, whose collection its
//!   hooks then shape too.
//! - **Parsing** runs on every core in an order-preserving map: support files
//!   first, then each test file parsed and recognized in one task, so only
//!   support files' ASTs are held at once. A file that doesn't parse is a
//!   `syntax-error` (an error, with `line:col`) and yields no cases; a support
//!   file that doesn't parse is left out of the support it would give. A file
//!   that isn't UTF-8 is decoded lossily and always refined, since only the
//!   framework knows its encoding. A claimed file that can't be read is an
//!   `unreadable` error, and so is a directory the walk can't read.
//! - **Merging** keeps discovery order by file, then adapter priority, then
//!   each recognizer's own order. Collection never re-sorts cases within a
//!   file: an adapter's order is its framework's (inherited methods,
//!   parametrize). An id produced twice keeps the higher-priority adapter's
//!   case (the first, within one adapter). Another adapter's case for it is
//!   dropped quietly: two frameworks collecting the same test is how a
//!   project chooses which runs it (unittest listed before pytest runs the
//!   `TestCase` methods both collect natively, pytest the rest). An adapter
//!   that produces one id twice gets a `duplicate-id` warning, saying that
//!   running the id runs every test with it, as pytest selects by id: the
//!   run then reports more results for it than collection lists.
//! - **Framework-level diagnostics stay with recognizers.** Duplicate
//!   definitions and tests that will never be collected are framework rules,
//!   so recognizers report them and collection passes them through.
//!   Collection adds `no-tests` (info) for a test file no adapter finds a
//!   test in, unless every adapter claiming it claims such files whatever
//!   their names (pytest's doctest modules: every module), as
//!   `Recognizer::claims_any_name` says for the file's directory and
//!   extension: a file without tests is no surprise then. The rule knows no
//!   framework's patterns: it only asks the recognizers. Nor is it added for a file an adapter
//!   already reported about (a module the framework skipped or ignores, a
//!   class it won't collect) or, in refinement, about a directory it is
//!   in (a conftest that skipped the directory), which says why already.
//! - **Refinement** sends one request per adapter, with all its flagged files
//!   and its recognizer's `context()`. Refined cases replace that adapter's
//!   static cases for the file; other adapters' cases stay. The refined
//!   diagnostics likewise replace that recognizer's errors for the file
//!   (its warnings and infos stay): the framework's import has the final
//!   word on whether the file collects and reports its own error, so a
//!   static error and the framework's collection error are one error, and a
//!   file the framework collects isn't failed by a static guess. Refined cases
//!   whose path is not a requested file are kept. Those in the package of a
//!   requested `__init__.py` (the innermost) are that package's: importing
//!   a package can make tests in files of it the walk didn't claim, as
//!   unittest's `load_tests` does when it discovers another pattern. They
//!   go where their files fall in the walk among the package's files, and
//!   the `__init__.py` gets no `no-tests` note. The others go after every
//!   file. When refinement fails, the static guess stays and each file gets
//!   a `refine-failed` warning; with no refiner (refinement is off), a
//!   `not-refined` info. `refined_files` counts the files whose cases came
//!   from refinement, fresh or cached.
//! - **The refinement cache** is `<cache dir>/refine/<adapter>/<key>.json`.
//!   It must never answer differently from a fresh import, since a stale
//!   answer drops tests pytest would run, so it caches only what its key
//!   covers. The key is a 128-bit FNV-1a hash of the cache's format, aye's
//!   version, the project root and cwd, the environment's digest
//!   (`aye_provision::EnvironmentFingerprint`: the interpreter and its
//!   installed packages, the environment variables but a few shell ones,
//!   its framework arguments), the project's `framework_args` (the
//!   arguments after `--`, which adapters pass to refinement in their
//!   context), the adapter's name, its recognizer's `context()` (with the
//!   content of every existing file the context names, such as pytest's
//!   inifile) and `settings()`, and the path and content of the file, of
//!   each of its support files and of each package `__init__.py` from its
//!   directory up to the root (importing the file runs them). The binary
//!   turns the cache on with [`Options::cache_dir`] and
//!   [`Collector::with_environment`], giving the interpreter workers run;
//!   without an interpreter (the default) there is no cache. A
//!   file is read from or written to the cache only when it, its support
//!   files and those `__init__.py` files are standalone (`standalone.rs`):
//!   they import only pure standard-library modules and the adapter's
//!   framework, never touch files, processes, the clock or randomness, never
//!   run code by name (`__import__`, `import_module`, `importorskip`), and
//!   name no module of the project and no installed one in a string (a
//!   plugin list); and when the context and the framework arguments load no
//!   plugin by name (`-p NAME`, unless it names the framework or a pure
//!   standard-library module) and name no module of the project or
//!   installed one. Installed modules are what the interpreter's paths list
//!   (its `site-packages` entries, the directories its `.pth` files and
//!   `PYTHONPATH` add, its distributions' names and `top_level.txt`), since
//!   the key holds their names but not their content, which an editable
//!   install outside the root or an edit in place changes. Only clean
//!   results are written: none with a diagnostic (an import error or a
//!   module-level skip may be the environment's), none without cases, and
//!   nothing from a request whose reply had diagnostics or cases outside the
//!   requested files (a broken conftest, a usage error). A hit skips the
//!   worker, even when no refiner is given. Known gaps, until workers report
//!   what an import read: a file that imports a project module is always
//!   refined, even when that module didn't change; conftests outside the
//!   file's directory chain (session-wide hooks in a sibling directory,
//!   conftests above the project root), installed plugins acting without an
//!   import, plugins an environment variable names (pytest's
//!   `PYTEST_PLUGINS`: its value is in the key, the plugin's content isn't)
//!   and a module named in a string that only an editable install's import
//!   hook maps, under a name that is neither its distribution's nor in its
//!   `top_level.txt`, are not in the key. Entries are never pruned. The
//!   state directory the cache creates is marked as a cache
//!   (`aye_fs::prepare_dir`).
//! - **Strict mode** turns every warning into an error, recognizers' and
//!   workers' included; infos stay infos. [`Collection::has_errors`] is what
//!   fails the run.
//! - At most 64 recognizers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::{Project, Recognizer};
use aye_events::{AdapterName, Case, Diagnostic, Severity};
use aye_protocol::Refiner;
use aye_provision::{EnvironmentFingerprint, InstalledModules};

mod assemble;
mod parallel;
mod recognize;
mod refine;
mod source;
mod targets;
mod walk;

use recognize::{Job, Recognized};
use refine::standalone::Scope;
use walk::{Mask, Tree, bits};

/// A prepared recognizer and its adapter's name.
pub type Prepared = (AdapterName, Box<dyn Recognizer>);

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Report every warning as an error.
    pub strict: bool,
    /// Where refinement results are cached. `None` turns the cache off, and
    /// so does a collector without an environment (see
    /// [`Collector::with_environment`]).
    pub cache_dir: Option<PathBuf>,
    /// aye's version, part of every cache key.
    pub version: String,
}

/// What collection found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Collection {
    /// By file in discovery order, then adapter priority, then source order.
    pub cases: Vec<Case>,
    pub diagnostics: Vec<Diagnostic>,
    /// Files whose tests were known by importing them.
    pub refined_files: u32,
    /// Collection was stopped (see [`Collector::with_stop`]) before it was
    /// done: `cases` and `diagnostics` are whatever it had, and mean
    /// nothing.
    pub interrupted: bool,
}

/// How far collection has got, for a live display: reported while the walk
/// goes on and before refinement starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// Files and directories the walk went through.
    pub walked: u64,
    /// Files a recognizer claimed for tests.
    pub test_files: u64,
    /// Files being imported to know their tests (0 until refinement).
    pub refining: u64,
}

impl Collection {
    /// Whether any diagnostic is an error (in strict mode, any warning too).
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Finds the tests in a project with the recognizers prepared for it.
pub struct Collector<'a> {
    project: Project<'a>,
    recognizers: &'a [Prepared],
    options: Options,
    /// What refinement depends on outside the project's files; without
    /// it, no cache.
    environment: Option<EnvironmentFingerprint>,
    /// The environment's installed modules, listed when the cache first
    /// needs them.
    installed: OnceLock<InstalledModules>,
    /// Set from another thread (Ctrl-C) to stop collection.
    stop: Option<Arc<AtomicBool>>,
}

impl<'a> Collector<'a> {
    /// `recognizers` are in priority order: when two produce the same id, the
    /// first one's case is kept.
    pub fn new(project: Project<'a>, recognizers: &'a [Prepared], options: Options) -> Self {
        assert!(
            recognizers.len() <= Mask::BITS as usize,
            "at most {} recognizers",
            Mask::BITS
        );
        Self {
            project,
            recognizers,
            options,
            environment: None,
            installed: OnceLock::new(),
            stop: None,
        }
    }

    /// A flag another thread sets to stop collection (the binary's Ctrl-C
    /// handler): the walk, parsing and refinement each give up within
    /// moments, and the [`Collection`] says it was interrupted. Refinement
    /// stops through the refiner, which is told separately.
    #[must_use]
    pub fn with_stop(mut self, stop: Arc<AtomicBool>) -> Self {
        self.stop = Some(stop);
        self
    }

    fn stopped(&self) -> bool {
        self.stop
            .as_ref()
            .is_some_and(|stop| stop.load(Ordering::SeqCst))
    }

    /// What refinement depends on outside the project's files: the
    /// interpreter workers run, the framework's arguments and the
    /// environment variables, as the cache's key holds them. Without one,
    /// the default, the refinement cache is neither read nor written.
    #[must_use]
    pub fn with_environment(mut self, environment: EnvironmentFingerprint) -> Self {
        self.environment = Some(environment);
        self
    }

    /// The environment, when the refinement cache may be used at all.
    fn caching(&self) -> Option<&EnvironmentFingerprint> {
        self.environment
            .as_ref()
            .filter(|_| self.options.cache_dir.is_some())
    }

    fn scope<'s>(
        &'s self,
        framework: &'s str,
        seen: &'s Mutex<HashMap<PathBuf, bool>>,
    ) -> Scope<'s> {
        Scope {
            root: self.project.root,
            cwd: self.project.cwd,
            framework,
            seen,
            installed: self.installed.get_or_init(|| {
                self.caching()
                    .map(|environment| environment.installed_modules(self.project.cwd))
                    .unwrap_or_default()
            }),
        }
    }

    /// Collect the tests under `targets` (absolute, or relative to the
    /// project's cwd), or under the recognizers' roots when there are none.
    /// Files that can only be known by importing them go to `refiner`.
    pub fn collect(
        &self,
        targets: &[PathBuf],
        refiner: Option<&(dyn Refiner + Sync)>,
    ) -> Collection {
        self.collect_watching(targets, refiner, &mut |_| {})
    }

    /// [`Collector::collect`], reporting its [`Progress`] to `progress` from
    /// the calling thread: every few hundred entries the walk goes through,
    /// once it is done, and every 50ms while refinement imports files (it
    /// runs on another thread meanwhile).
    pub fn collect_watching(
        &self,
        targets: &[PathBuf],
        refiner: Option<&(dyn Refiner + Sync)>,
        progress: &mut dyn FnMut(Progress),
    ) -> Collection {
        let interrupted = || Collection {
            cases: Vec::new(),
            diagnostics: Vec::new(),
            refined_files: 0,
            interrupted: true,
        };
        let mut tree = Tree::new(self.project.root, self.recognizers);
        let mut tick = |tree: &Tree<'_>| {
            progress(Progress {
                walked: tree.walked,
                test_files: tree.test_files,
                refining: 0,
            });
            !self.stopped()
        };
        if !self.discover(&mut tree, targets, &mut tick) || self.stopped() {
            return interrupted();
        }
        tick(&tree);

        let mut jobs = Vec::new();
        for at in tree.tests() {
            let claimants = bits(tree.entries[at].tests)
                .map(|i| (i, tree.support(at, i)))
                .collect();
            jobs.push(Job { at, claimants });
        }
        let mut diagnostics = std::mem::take(&mut tree.diagnostics);

        let support_files = tree.support_files();
        let loaded = parallel::map(&support_files, |&at| {
            let entry = &tree.entries[at];
            source::load(&entry.abs, &entry.rel)
        });
        let mut support = HashMap::new();
        for (at, (source, diagnostic)) in support_files.into_iter().zip(loaded) {
            diagnostics.extend(diagnostic);
            if let Some(source) = source {
                support.insert(at, source);
            }
        }

        let seen = Mutex::new(HashMap::new());
        let recognized = parallel::map(&jobs, |job| {
            if self.stopped() {
                Recognized::nothing()
            } else {
                self.recognize(&tree, &support, job, &seen)
            }
        });
        if self.stopped() {
            return interrupted();
        }
        let refining = recognized
            .iter()
            .filter(|r| {
                r.by.iter()
                    .any(|(_, recognition)| recognition.refine.is_some())
            })
            .count();
        let refine = || self.refine(&tree, &support, &jobs, &recognized, refiner, &seen);
        let refinement = if refining > 0 && refiner.is_some() {
            let report = Progress {
                walked: tree.walked,
                test_files: tree.test_files,
                refining: refining as u64,
            };
            std::thread::scope(|scope| {
                let task = scope.spawn(refine);
                while !task.is_finished() {
                    progress(report);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                task.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
        } else {
            refine()
        };
        if self.stopped() {
            return interrupted();
        }
        let mut collection = self.assemble(&tree, &jobs, recognized, refinement, diagnostics);
        if self.options.strict {
            for diagnostic in &mut collection.diagnostics {
                if diagnostic.severity == Severity::Warning {
                    diagnostic.severity = Severity::Error;
                }
            }
        }
        collection
    }
}

#[cfg(test)]
mod tests;
