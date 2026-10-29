//! The unittest adapter. Native: it does not go through pytest.
//!
//! Rust half: recognition of `TestCase` subclasses (including through in-module
//! bases), `test*` methods, `load_tests`, skip and expected-failure decorators,
//! and `python -m unittest` discovery rules and dotted ids as input aliases.
//!
//! Python half: the embedded executor, which drives `TestCase.run(result)` with
//! aye's own `TestResult`, owns the class- and module-level fixture handling
//! that `TestSuite.run` does today, reports `subTest` blocks as sub-outcomes,
//! and gives unittest suites the parallelism, selection and history unittest
//! never had. Twisted trial, nose2 and testslide are meant to run through
//! this adapter; none does yet.
//!
//! Django is a set of services (settings, test databases, live-server ports)
//! used by this adapter and by the pytest adapter, not a fourth adapter; it
//! lands later behind the services interface in `aye-protocol`.
//!
//! Depends on: aye-adapter, aye-events, aye-protocol.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The reference is `python -m unittest discover` on Python 3.13**
//!   (`Lib/unittest/loader.py`, `suite.py`, `case.py`); 3.10 to 3.12 load
//!   the same tests, except that before 3.11 an imported
//!   `unittest.FunctionTestCase` makes a test of its own, which aye doesn't
//!   predict. Besides the contract, the crate depends on `aye-config` (for
//!   `[tool.aye]`), `aye-fs` (for paths), `ruff_python_parser` (to read the
//!   project modules a test module imports from) and `serde_json`.
//! - **Configuration** is `[tool.aye.unittest]` in the project root's
//!   `pyproject.toml`: `start-directory` (default `.`), `pattern` (default
//!   `test*.py`) and `top-level-directory` (default: the start directory),
//!   as `discover -s -p -t` take them, but relative to the project root
//!   rather than the current directory. Unknown keys, values that aren't
//!   strings, an empty pattern, directories that don't exist, a start
//!   directory outside the top-level directory and one below it that is no
//!   package (unittest's "Start directory is not importable") are
//!   configuration errors.
//!   [`Unittest::prepare`] always returns a recognizer: which adapters a run
//!   uses is the binary's choice (`[tool.aye] adapters`).
//! - **Discovery** walks as unittest's does: from the start directory, files
//!   whose names are valid module names (`VALID_MODULE_NAME`) and match the
//!   pattern (`fnmatch`, case-sensitive but on Windows), and directories
//!   only when they are packages (`__init__.py`). A package's `__init__.py`
//!   holds tests too (discovery loads the package's namespace), except the
//!   top-level directory's; it is claimed only when it may bind a class (it
//!   mentions `class` or `import`, or may define `load_tests`), so that
//!   empty ones aren't reported as files without tests. A package whose
//!   name has a dot can't be imported by its dotted name: its `__init__.py`
//!   is refined, to report unittest's import error, and nothing below it
//!   has tests. Paths on the command line narrow the walk: a directory
//!   inside the start directory is walked as discovery from the start
//!   directory would reach it (but entered even when it isn't a package),
//!   one that holds the start directory walks the start directory, and one
//!   outside it is a start directory of its own (its own top-level
//!   directory when outside the configured one), sent to the executor in
//!   the context's `starts`. A file named on the command line is a test
//!   module whatever the pattern, and a module alone, as `python -m
//!   unittest path.py` takes it: even below a package that may define
//!   `load_tests`, which doesn't decide its tests then (the context's
//!   `named` lists such files for the executor). A test both such a file
//!   and its package's discovery load (`aye tests/pkg tests/pkg/test_x.py`,
//!   the package's `load_tests` loading `test_x`) runs once, under the
//!   file's id, as pytest dedups a directory and a file in it (and as aye
//!   does without `load_tests`); `python -m unittest`, given both names,
//!   runs it twice. Without paths, the walk starts at the start directory,
//!   wherever aye was started.
//! - **Ids** are `path::Class::method`, the path relative to aye's root and
//!   `Class` the name the loader found the class under in the module's
//!   namespace (`dir()`): its own, an alias's (`Alias = Base`, `from m
//!   import Base as TestBase`), or that of a class a factory made
//!   (`TestMemory = make_suite("memory")`), as pytest names `TestCase`
//!   methods, so the two adapters give a test one id. A class only a
//!   `load_tests` loads goes by its `__qualname__`, split on dots.
//!   Selection also reads ids as dotted names from the root, which are
//!   unittest's own when the top-level directory is the root and classes go
//!   by their own names. The path is the module whose loading produced the
//!   test, so a `TestCase` imported into two test modules is two tests, as
//!   unittest runs it twice; two tests with one id (a `load_tests` adding a
//!   class the loader already took) both run under it (collection warns
//!   `duplicate-id`).
//!   Tests that aren't `TestCase` methods, which only a `load_tests` makes,
//!   are `path::name`: a `FunctionTestCase` its function's `__qualname__`, a
//!   `DocTestSuite` or `DocFileSuite` case its `DocTest` name (`pkg.mod.f`,
//!   `README.txt`), anything else its `id()`. **Keywords** are the path's
//!   parts and those names; there are no tags. **Affinity** is the file,
//!   then `file::Class`: a module's and a class's lifetimes, which the
//!   executor names the same way (from the class's own module, which an
//!   imported class doesn't share with its file); the tests of a package
//!   that may define `load_tests` are led by `package:DIR`, so that one
//!   batch loads the package once.
//! - **Order** is the loader's: classes by the name they are bound to in
//!   the module (`dir()`), methods by name, both in code point order, and
//!   `runTest` for a class with no `test*` methods. Files follow aye's walk,
//!   which sorts as unittest's `sorted(os.listdir())` does, a package's
//!   `__init__.py` first in its directory, as discovery loads a package
//!   before the files in it.
//! - **Static recognition** reads the module's statements in order, as
//!   Python runs them: classes, defined or imported, whose MRO (C3, across
//!   the project's files) reaches `unittest.TestCase` or
//!   `IsolatedAsyncioTestCase`, and their callable `test*` attributes
//!   (methods, lambdas, aliases, nested classes, `staticmethod` and
//!   `classmethod`; not properties, constants or `None`), inherited ones
//!   included; `del` at module level removes a class. Decorators known to
//!   keep a method callable (unittest's skip family and `expectedFailure`,
//!   `mock.patch` and its forms, pytest marks, `functools.wraps`, local
//!   decorators that visibly return their argument or a `wraps` wrapper of
//!   it) are seen through; class decorators only unittest's, `mock.patch`'s,
//!   pytest marks and local ones that return the class untouched. `if
//!   __name__ == "__main__":` and `if TYPE_CHECKING:` blocks don't run.
//!   Imports are followed into the project's own files (from the top-level
//!   directory, the root and `src`, or relative to the module); a name found
//!   only beside the importing module (reachable through a `sys.path`
//!   change) is unknown. Installed packages aren't read: a class imported
//!   from one is taken to hold no tests unless its name looks like a test
//!   class (`Test*`, `*Test`, `*Tests`, `*TestCase`) and it isn't one of the
//!   well-known bases without tests (Django's, DRF's, Twisted's, …), and one
//!   inherited from is unknown (Django's `TestCase` included: refined, and
//!   it needs Django's settings to import). A class with no bases but
//!   `object` or the standard library's can't be made a `TestCase` by
//!   `dataclass`, `total_ordering`, `enum.unique`, `enum.verify` or
//!   `runtime_checkable` either, so a test module importing the project's
//!   dataclasses stays exact.
//! - **Refinement** (Core decision 5) for anything else: a module that may
//!   define `load_tests`, star-imports a module outside the standard
//!   library, defines `__getattr__` or `__dir__`, may raise while imported
//!   (`raise SkipTest`), calls `exec`, `globals()`, `setattr` and the like at
//!   import (or a local function that does), changes a class's attributes or
//!   passes a class to a call at import, binds a possible test class in a
//!   branch that may not run or from a call that may return a class; one
//!   whose import may change the classes it loads (or their bases) through
//!   another module: what runs before the loader reads them (the packages
//!   holding it, which discovery imports first; it; the project modules it
//!   imports, and theirs) assigns or deletes their attributes
//!   (`base.Base.test_x = f`), passes one to a call (`install(CheckBase)`),
//!   calls a method on one (a classmethod adding tests), calls a project
//!   function that changes objects it doesn't make, changes unittest's
//!   loader, or runs `exec` (what another test module's import changes is
//!   unittest's order to decide, and isn't followed); a
//!   class with a decorator or a metaclass aye can't see through,
//!   `__init_subclass__` in a base, bases it can't resolve or an
//!   inconsistent MRO; `test*` attributes defined conditionally, by unknown
//!   decorators or values, or imported from outside the project. The static
//!   guess is kept, `Partial`. A claimed module with no test aye can see is
//!   refined too, as the pytest adapter refines such files: discovery
//!   imports it all the same, so its import error (or `load_tests`'s) is
//!   unittest's error to report, and a test may exist only at runtime.
//!   That includes a package's `__init__.py` claimed for its imports; one
//!   holding only constants isn't claimed. Clean zero-case answers aren't
//!   cached by collection, so such modules are imported on every run.
//! - **A package whose `__init__.py` may define `load_tests`** decides the
//!   tests of every file below it, as discovery hands it the package: by the
//!   rule [`may_define_load_tests`] (a word-level reading of the bytes, which
//!   the executor applies too), each such file is refined, and the executor
//!   answers from `TestLoader.discover` on the outermost such package
//!   between the start directory and the file, giving each file the tests
//!   discovery found in it (the `__init__.py` takes what belongs to no
//!   discovered file, such as tests `load_tests` makes itself, or loads
//!   from a module discovery doesn't import itself). A file below such a
//!   package that ends up without a test gets an `ignored` info saying why
//!   (the `load_tests` loads none from it, or its tests run under the
//!   `__init__.py`, or under files named on the command line), rather than
//!   collection's `no-tests`.
//!
//! Decided for the executor (`python/aye_engine_unittest.py`), October 2026:
//! - **Native, standard library only**: it works without pytest installed.
//!   Each test runs through `test(result)`, as `TestSuite` calls it, with
//!   aye's `TestResult`, whose callbacks fold into one outcome (the worst):
//!   `addUnexpectedSuccess` is `failed` with "unexpected success" (unittest's
//!   `wasSuccessful()` is false for it); a failed or errored subtest fails or
//!   errors its test, with its failure, and every subtest is a
//!   `sub_outcome` named by its description (`[msg] (i=1)`); a test whose
//!   only callbacks are subtests' passed. It subclasses `aye_engine.Executor`
//!   rather than `EngineExecutor`, whose loop decides outcomes from what the
//!   body raises, but uses the engine's `Lifetimes`, `Result` and `Failure`.
//! - **Lifetimes** are unittest's fixtures, opened and closed as
//!   `TestSuite.run` would for the batch's order: the module's
//!   (`setUpModule`, `tearDownModule`, `doModuleCleanups`) and the class's
//!   (`setUpClass`, `tearDownClass`, `doClassCleanups`), a new one whenever
//!   the class or module differs from the previous test's. A skipped class
//!   runs neither its setup nor its teardown. A failed setup is reported
//!   once, on `lifetime_finished`, and each test it blocks finishes `error`
//!   with `cause`; a `SkipTest` from `setUpClass` or `setUpModule` skips them
//!   with its reason. A failed `tearDownClass`, `tearDownModule` or class or
//!   module cleanup (an `enterClassContext` whose exit raised included) is
//!   reported on `lifetime_finished` and makes the batch's status 1, as
//!   unittest's `wasSuccessful()` is false then; a `SkipTest` raised there
//!   is unittest's skip, no failure. With `maxfail`, a failed lifetime is
//!   reported before the tests it blocks (`Lifetimes.report_early`), so
//!   that the runner, which drops what follows the bad outcome reaching the
//!   limit, keeps it. Classes of unittest and doctest themselves
//!   (`FunctionTestCase`, `DocTestCase`) get no lifetimes.
//! - **Lookup**: a batch loads each unit (module, or package that may
//!   define `load_tests`) once, takes every test of each requested id,
//!   falls back to the module alone and then to `Class(method)`, and
//!   reports the rest as `not-found`. A batch the runner re-queued after a
//!   crash leaves out what its context's `ran_ids` says already ran, by
//!   count: an id listed n times skips its first n tests, so that an id
//!   unittest runs more than once (a `load_tests` adding a test again) runs
//!   only the copies left (the runner doesn't yet re-queue such an id once
//!   one of its copies started: finding F41's other half). Every test
//!   unittest failed to load for a file (each `loadTestsFromName` a
//!   `load_tests` calls that fails, say) is in the file's
//!   `collection-error`, with unittest's message. A module that fails to
//!   import, or whose `load_tests` raises, is one `collection-error` and
//!   its tests get no result, as in the pytest adapter (so the run stops
//!   with exit 2 unless `--continue-on-collection-errors`, where unittest
//!   counts one error and goes on); one that raises `SkipTest` while
//!   imported skips them. In refinement those are `collection-error` and `skipped`
//!   diagnostics. Each is reported once: by refinement when the module is
//!   refined (its tests then don't exist, so no batch loads it again), else
//!   by the one batch its tests share (affinity). A package whose
//!   `__init__.py` fails to import is one error in unittest, which never
//!   reaches the modules below it; aye collects those statically, so with
//!   `--continue-on-collection-errors` each also reports the package's
//!   failure as its own import error (as pytest reports them).
//! - **Capture, maxfail, stops**: with `policy.capture`, `sys.stdout` and
//!   `sys.stderr` are captured per test (unittest's `-b`), through the
//!   `capture` service when the runner provides one, else into
//!   `aye_engine.CaptureIO` streams, which have `.buffer`, `.encoding` and
//!   a `fileno()` (a temporary file, whose writes are captured after the
//!   stream's), write what UTF-8 can't hold (a lone surrogate) as `?`, as
//!   pytest's default capture does (`errors="replace"`; `-b`'s `StringIO`
//!   takes it too), and stay readable when a test closes them. A class or
//!   module fixture's output is captured too: kept for its failure, after
//!   its text (`Stdout:`, `Stderr:`), as `-b` adds it to the fixture's
//!   error, and dropped when it passes; without capture it goes to the
//!   worker's stderr. `maxfail` stops the batch after that many bad
//!   outcomes. A `KeyboardInterrupt` (while importing, in a fixture's
//!   setup, teardown or cleanup, or in a test), or a test calling
//!   `result.stop()`, stops it with a `batch-stopped` error (the test in
//!   flight finishes `error`; the lifetimes open still close) and status
//!   2; otherwise the status is `python -m unittest`'s: 0, 1 when a test
//!   failed, a module didn't load or a class or module fixture failed, 5
//!   when nothing ran. The `context` and `cleanup` services are honoured as
//!   `EngineExecutor` honours them.
//! - **Failures** carry the exception, unittest's text as the detail, the
//!   phase (from unittest's frames: `setUp`, the test, `tearDown`, a
//!   cleanup), the user's frames (unittest's, asyncio's and doctest's
//!   hidden) and, for a failed `assert*` comparison (`assertEqual`,
//!   `assertIn`, `assertLess`, …), its operands read from the method's
//!   arguments (`assertCountEqual`'s, which it rebinds to `Counter`s, from
//!   its copies of them as lists), with unittest's explanation (the message
//!   after its first line) as the diff. Building one never raises (a result
//!   callback that raised would lose the test): what a test broke is left
//!   out, and an expected failure whose exception can't say what it is
//!   gets its type as its reason. A test
//!   with a bad outcome carries its `reproduce`: `aye`, aye's own options
//!   that shaped the run (the context's `rerun_args`, `-c FILE`), the id,
//!   then `--` and the arguments after it (`aye_engine.reproduce`). A
//!   skipped subtest's `sub_outcome` carries unittest's reason (`reason`).
//!   A failed lifetime's traceback starts at the user's code, as unittest
//!   prints it (no frame of the executor's `open` or `close`).
//! - **Tests may patch what the executor uses**: durations come from
//!   `aye_engine.perf_counter`, capture from `aye_engine.CaptureIO` and
//!   paths from `aye_engine.os_path` (`os.path` as it was when the engine
//!   was imported, `abspath` and `realpath` included) and the engine's
//!   `Paths`; and a batch loads its modules with `os.path`'s own functions
//!   back in place (`aye_engine.unpatched_os_path`), as unittest imports
//!   every test before running any: a test patching `time`, `io` or
//!   `os.path` (and leaving them patched) changes nothing the executor
//!   reports, in its batch or a later one on the worker.
//! - **Not honoured yet**: the policy's keyword and tag expressions (static
//!   selection is exact for this adapter, which has no tags), its timeout
//!   and verbosity, the arguments after `--`, custom `TestSuite` subclasses'
//!   `run` (suites are flattened), and a pattern matching `__init__.py`
//!   (unittest then imports a package twice; aye runs its tests once).
//!   Django's `TestCase` needs Django's settings, which are a later
//!   service. unittest counts a skipped subtest, a module skipped while
//!   imported and a `SkipTest` from a fixture's teardown among its skips;
//!   aye reports the first as a `sub_outcome` of a passing test, the
//!   second as an info diagnostic and the third not at all. What a test
//!   module prints while imported, which unittest prints as discovery runs,
//!   is dropped under capture. A failed teardown fails the batch but adds
//!   no test to the counts (unittest counts it among its errors).

mod config;
mod context;
mod fnmatch;
mod module;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::{
    Adapter, AdapterError, Claim, Project, Recognition, Recognizer, Requested, Setting, SourceFile,
};
use aye_events::{AdapterName, AffinityKey, Case, Certainty, ProjectPath, ProjectPathRef, Span};
use aye_fs::normalize_absolute;
use aye_protocol::{AdapterContext, EmbeddedModule, Executor};

pub use config::UnittestConfig;
pub use fnmatch::{fnmatch, valid_module_name};
pub use module::may_define_load_tests;

use crate::context::UnittestContext;
use crate::module::Modules;

/// The unittest adapter's executor module.
pub const MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_engine_unittest",
    source: include_str!("../python/aye_engine_unittest.py"),
};

/// The adapter's name, in cases, batches and the registry.
pub const NAME: AdapterName = AdapterName::new_static("unittest");

/// Why a test module with no test aye can see is refined.
const NO_STATIC_TESTS: &str = "no test is visible statically; discovery imports the module to tell";

/// The unittest adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unittest;

impl Adapter for Unittest {
    fn name(&self) -> AdapterName {
        NAME
    }

    /// Always a recognizer: whether a run uses this adapter is the
    /// binary's choice.
    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        Ok(Some(Box::new(UnittestRecognizer::new(project)?)))
    }

    fn executor(&self) -> Executor {
        Executor {
            adapter: NAME,
            modules: &[MODULE],
            entry: "aye_engine_unittest:UnittestExecutor",
            lists_session: false,
        }
    }
}

fn is_package(dir: &Path) -> bool {
    dir.join("__init__.py").is_file()
}

/// The unittest adapter's static face for one project.
#[derive(Debug)]
pub struct UnittestRecognizer {
    root: PathBuf,
    config: UnittestConfig,
    /// `(start, top)`: the configured start directory first, then the
    /// directories named on the command line outside it.
    starts: Vec<(PathBuf, PathBuf)>,
    modules: Modules,
    /// By package directory: whether its `__init__.py` may define
    /// `load_tests`.
    takes_over: Mutex<HashMap<PathBuf, bool>>,
    /// The Python files named on the command line: each is a module alone,
    /// as `python -m unittest path.py` loads it, even below a package that
    /// may define `load_tests`.
    named: Vec<PathBuf>,
    /// The context, built once.
    context: OnceLock<UnittestContext>,
}

impl UnittestRecognizer {
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        let root = normalize_absolute(project.root);
        let config = UnittestConfig::load(&root, project.tool_aye)?;
        let start = normalize_absolute(&config.start);
        let top = normalize_absolute(&config.top);
        let mut starts = vec![(start.clone(), top.clone())];
        for arg in project.args {
            let path = normalize_absolute(&project.cwd.join(arg));
            // A directory holding the start directory walks it; one inside
            // it narrows discovery from it.
            if !path.is_dir() || start.starts_with(&path) || path.starts_with(&start) {
                continue;
            }
            if starts.iter().any(|(s, _)| path.starts_with(s)) {
                continue;
            }
            starts.retain(|(s, _)| *s == start || !s.starts_with(&path));
            let own_top = if path.starts_with(&top) {
                top.clone()
            } else {
                path.clone()
            };
            starts.push((path, own_top));
        }
        let modules = Modules::new(&top, &root);
        let named = project
            .args
            .iter()
            .map(|arg| normalize_absolute(&project.cwd.join(arg)))
            .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "py"))
            .collect();
        Ok(Self {
            root,
            config,
            starts,
            modules,
            takes_over: Mutex::new(HashMap::new()),
            named,
            context: OnceLock::new(),
        })
    }

    /// The files named on the command line that a package that may define
    /// `load_tests` would otherwise answer for: the executor loads each as
    /// a module alone.
    fn named_below_load_tests(&self) -> Vec<&PathBuf> {
        self.named
            .iter()
            .filter(|path| self.taken_over_by(path).is_some())
            .collect()
    }

    /// The resolved configuration.
    pub fn config(&self) -> &UnittestConfig {
        &self.config
    }

    /// The start (and top-level directory) discovery reaches `path` from:
    /// the configured one, else the outermost named directory outside it.
    fn start_for(&self, path: &Path) -> Option<(&Path, &Path)> {
        let (start, top) = &self.starts[0];
        if path.starts_with(start) && path != start {
            return Some((start, top));
        }
        self.starts[1..]
            .iter()
            .filter(|(s, _)| path.starts_with(s) && path != s)
            .min_by_key(|(s, _)| s.components().count())
            .map(|(s, t)| (s.as_path(), t.as_path()))
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
        } else if !(valid_module_name(&name) && fnmatch(&name, &self.config.pattern.value)) {
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
    fn taken_over_by(&self, path: &Path) -> Option<PathBuf> {
        let (start, top) = self.start_for(path)?;
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
    fn unimportable(&self, path: &Path) -> Option<PathBuf> {
        let (start, top) = self.start_for(path)?;
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

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).map_or_else(
            |_| path.to_string_lossy().replace('\\', "/"),
            |rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            },
        )
    }

    fn cases(
        &self,
        file: &SourceFile<'_>,
        loaded: &[module::Loaded],
        package: Option<ProjectPathRef<'_>>,
        certainty: Certainty,
    ) -> Vec<Case> {
        let mut cases = Vec::new();
        let parts: Vec<String> = file.rel.components().map(str::to_string).collect();
        for class in loaded {
            // The name the loader found the class under, as pytest names it.
            let class_names = [class.name.as_str()];
            let class_id = format!("{}::{}", file.rel, class.name);
            for method in &class.methods {
                let mut case = Case::new(format!("{class_id}::{}", method.name), NAME);
                case.span = Some(Span {
                    path: ProjectPath::as_written(self.relative(&method.file)),
                    start: method.range.start().to_u32(),
                    end: method.range.end().to_u32(),
                    line: Some(method.line),
                });
                let mut keywords = parts.clone();
                keywords.extend(class_names.iter().map(|n| (*n).to_string()));
                keywords.push(method.name.clone());
                let mut seen = std::collections::HashSet::new();
                keywords.retain(|k| seen.insert(k.clone()));
                case.keywords = keywords;
                case.affinity = package
                    .map(AffinityKey::package)
                    .into_iter()
                    .chain([
                        AffinityKey::file(file.rel),
                        AffinityKey::file_class(file.rel, &class_names),
                    ])
                    .collect();
                case.certainty = certainty;
                cases.push(case);
            }
        }
        cases
    }
}

impl Recognizer for UnittestRecognizer {
    /// The command-line paths; else the start directory.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if project.args.is_empty() {
            return vec![self.starts[0].0.clone()];
        }
        project
            .args
            .iter()
            .map(|arg| project.cwd.join(arg))
            .collect()
    }

    /// Towards a start directory, and below one only into packages, as
    /// discovery recurses.
    fn descend(&self, dir: &Path) -> bool {
        let dir = normalize_absolute(dir);
        let mut inside = false;
        for (start, _) in &self.starts {
            if start.starts_with(&dir) {
                return true;
            }
            inside |= dir.starts_with(start);
        }
        inside && is_package(&dir)
    }

    /// A Python file named on the command line; else a module discovery
    /// would import: a valid module name matching the pattern below a start
    /// directory (or a named directory), or a package's `__init__.py`.
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if Path::new(name).extension().is_none_or(|e| e != "py") {
            return Claim::Ignore;
        }
        if requested.is_named() {
            return Claim::Tests;
        }
        let abs = normalize_absolute(&self.root.join(rel));
        let Some((start, top)) = self.start_for(&abs) else {
            return Claim::Ignore;
        };
        if name == "__init__.py" {
            let dir = abs.parent().unwrap_or(&abs);
            if dir == top || !dir.starts_with(start) {
                return Claim::Ignore;
            }
            // Most are empty: claimed only when they may bind a class, so
            // that collection doesn't call each one a file without tests,
            // or when discovery fails to import them.
            return if may_hold_tests(&abs) || self.unimportable(&abs).is_some() {
                Claim::Tests
            } else {
                Claim::Ignore
            };
        }
        if valid_module_name(name) && fnmatch(name, &self.config.pattern.value) {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    fn recognize(
        &self,
        file: &SourceFile<'_>,
        _requested: Requested,
        _support: &[SourceFile<'_>],
    ) -> Recognition {
        let abs = normalize_absolute(file.abs);
        if let Some(package) = self.unimportable(&abs) {
            // Its `__init__.py` is refined, to report the import error.
            let refine = (abs == package.join("__init__.py")).then(|| {
                format!(
                    "{} has a dot in its name, so discovery can't import it",
                    self.relative(&package)
                )
            });
            return Recognition {
                refine,
                ..Recognition::default()
            };
        }
        let summary = Arc::new(module::summarize(&abs, file.module, file.lines));
        // A file named on the command line is loaded alone, as `python -m
        // unittest path.py` loads it: no package's `load_tests` decides it.
        let package = if self.named.contains(&abs) {
            None
        } else {
            self.taken_over_by(&abs)
        };
        let package_rel = package.as_deref().map(|p| self.relative(p));
        // Without an import, the guess keeps the classes that are surely
        // test classes, for when refinement is off or fails.
        let (loaded, mut refine) = match self.modules.loaded(&summary) {
            // What runs before the loader reads the classes may change them.
            Ok(loaded) => {
                let effect = self.modules.effects(&summary, &loaded);
                (loaded, effect)
            }
            Err(reason) => (guess(&self.modules, &summary), Some(reason)),
        };
        if let Some(package) = &package_rel {
            let reason = format!(
                "{package}/__init__.py may define load_tests, which decides the tests below it"
            );
            refine = Some(match refine {
                Some(existing) => format!("{reason}; {existing}"),
                None => reason,
            });
        }
        let certainty = if refine.is_some() {
            Certainty::Partial
        } else {
            Certainty::Exact
        };
        let cases = self.cases(
            file,
            &loaded,
            package_rel.as_deref().map(ProjectPathRef::as_written),
            certainty,
        );
        if cases.is_empty() && refine.is_none() {
            // Nothing to run statically, but discovery imports the module
            // all the same: its import error is unittest's error to report.
            refine = Some(NO_STATIC_TESTS.to_string());
        }
        Recognition {
            cases,
            refine,
            diagnostics: Vec::new(),
        }
    }

    fn settings(&self) -> Vec<Setting> {
        self.config.settings()
    }

    /// What the executor needs to discover as unittest would:
    /// [`UnittestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.context.get_or_init(|| UnittestContext::new(self)))
    }
}

/// Whether a package's `__init__.py` may bind a test class: it defines a
/// class, imports, or may define `load_tests`. Unreadable files are claimed,
/// for collection to report.
fn may_hold_tests(init: &Path) -> bool {
    let Ok(bytes) = std::fs::read(init) else {
        return true;
    };
    let has = |word: &[u8]| bytes.windows(word.len()).any(|w| w == word);
    has(b"class") || has(b"import") || may_define_load_tests(&bytes)
}

/// The best static guess for a module only an import can tell: the
/// classes that are surely test classes, without what made it uncertain.
fn guess(modules: &Modules, summary: &Arc<module::Summary>) -> Vec<module::Loaded> {
    let mut out = Vec::new();
    for (name, binding) in &summary.names {
        if let module::Binding::Class(index) = binding
            && let Ok(Some(mut loaded)) = modules.class_tests_for_guess(summary, *index)
        {
            loaded.name.clone_from(name);
            out.push(loaded);
        }
    }
    out
}
