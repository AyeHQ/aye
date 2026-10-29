//! The doctest adapter.
//!
//! Rust half: collection of docstrings and text files (`.txt`, `.rst`, any
//! file a glob names) into one case per docstring or file
//! (`path::object`), examples being sub-outcomes.
//!
//! Python half: the embedded executor, running examples in order in the
//! docstring's `globs` with a `DocTestRunner` subclass that emits structured
//! diffs instead of two text blobs. Per-example re-runs and an `--accept`
//! mode that rewrites expected output in place are planned.
//!
//! Depends on: aye-adapter, aye-events, aye-protocol.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **This crate is the doctest adapter, and it is native**: it maps
//!   doctest onto aye's engine and never goes through pytest. Ids and
//!   outcomes follow pytest 9.1.1's doctest plugin (`_pytest/doctest.py`),
//!   so plain `pytest --doctest-modules` / `--doctest-glob` is the referee.
//!   The executor (`python/aye_engine_doctest.py`) imports only the standard
//!   library and runs on Python 3.10 and later, with or without pytest. The
//!   crate also depends on `aye-config`, for its loaders and `[tool.aye]`,
//!   on `aye-fs` for paths, and on `aye-static` for the variables it reads.
//!   Per-example re-runs and `--accept` are not implemented yet.
//! - **Ids.** A module gives one case per `DocTest` with examples, named as
//!   `DocTestFinder().find(module, module.__name__)` names it, in its order
//!   (sorted by name): `pkg/mod.py::pkg.mod`, `pkg/mod.py::pkg.mod.func`,
//!   `pkg/mod.py::pkg.mod.Class.method`. The module's name is the one
//!   pytest's default import mode (`prepend`) gives it: the dotted path from
//!   the directory above its outermost package (directories holding an
//!   `__init__.py` with an identifier for a name), else its file name; so
//!   `pkg/__init__.py` is `pkg`. A text file with examples is one case,
//!   `path::<file name>`. Markdown is a text file like any other when a
//!   glob names it: fences aren't special, as in pytest. `keywords` are the
//!   directories below pytest's rootdir, the file name and the test's name,
//!   as pytest's `-k` matches them. `affinity` is `["import:NAME", path]`
//!   for a module, so files pytest imports under one name share a batch and
//!   its "import file mismatch" comes out as pytest reports it, and `[path]`
//!   for a text file.
//! - **Activation.** [`Doctest::prepare`] returns `None` when nothing is
//!   enabled (a `.txt` or `.rst` file named on the command line is enough). Modules are collected with `--doctest-modules` in pytest's
//!   options (`addopts`, `PYTEST_ADDOPTS`, the arguments after `--`; not
//!   after `-p no:doctest`), or `[tool.aye.doctest] modules = true`; an
//!   explicit `modules = false` turns them off whatever pytest says. Text
//!   files match `[tool.aye.doctest] glob` when it is set, else pytest's
//!   `--doctest-glob`s, else `test*.txt` when a pytest configuration exists
//!   or the run uses the pytest adapter too ([`Doctest::prepare_among`]:
//!   pytest collects those by default, configured or not), else nothing; a
//!   `.txt` or `.rst` file named on the command line is a doctest whatever
//!   the globs, as in pytest. A text file without examples is no test (no
//!   case, not refined), as pytest makes no item of it. Option flags are
//!   pytest's `doctest_optionflags` (default `ELLIPSIS`; a name pytest
//!   doesn't accept is a configuration error, where pytest fails to
//!   collect) plus `[tool.aye.doctest] optionflags` (any doctest flag,
//!   doctest's combinations `COMPARISON_FLAGS` and `REPORTING_FLAGS`
//!   included, and pytest's `ALLOW_UNICODE`, `ALLOW_BYTES`, `NUMBER`). Text files are read with `[tool.aye.doctest] encoding`, else
//!   `doctest_encoding`, else UTF-8. `--doctest-report` chooses the diff in
//!   a failure's detail, and `--doctest-ignore-import-errors` makes a module
//!   that fails to import a skipped one (not a conftest that fails, as in
//!   pytest). `[tool.aye.doctest]` rejects unknown keys.
//! - **pytest's configuration is approximated**, since the pytest adapter's
//!   isn't shared: the first file holding pytest settings (`pytest.toml`,
//!   `.pytest.toml`, `pytest.ini`, `.pytest.ini`, `pyproject.toml` with
//!   `[tool.pytest]` or `[tool.pytest.ini_options]`, `tox.ini` with
//!   `[pytest]`, `setup.cfg` with `[tool:pytest]`, in that order in each
//!   directory) searching up from the common directory of the paths given,
//!   or the current one, as pytest's `locate_config` does; `-c` and
//!   `--rootdir` from the command line or the arguments after `--`; `-o`
//!   overrides (`-o addopts=` from the environment or the command line).
//!   Read from it: `addopts`, `testpaths` (only when run from the rootdir,
//!   glob-expanded, as pytest), `norecursedirs`, `pythonpath`,
//!   `consider_namespace_packages`, `doctest_optionflags`,
//!   `doctest_encoding`, `filterwarnings` and the `junit_*` settings; from
//!   the options also `--ignore`, `--ignore-glob`,
//!   `--collect-in-virtualenv`, `--import-mode`, `--deselect` (whose
//!   prefixes the context's `deselect` gives as prefixes of aye's ids: the
//!   binary drops the cases they name and counts them, as pytest deselects
//!   its doctest items), `-W`/`--pythonwarnings`, `--noconftest`,
//!   `--confcutdir`, `--doctest-continue-on-failure`, and
//!   `-x`/`--exitfirst`/`--maxfail` and `--junitxml`/`--junit-xml`/
//!   `--junitprefix`, which the context gives as the pytest adapter's does
//!   (`maxfail`, `junitxml`, `junit_prefix`, `junit_*`), for the binary to
//!   make them the run's when its command line gives none. Not read:
//!   conftests' `collect_ignore` and their hooks (but see "Marks" below),
//!   plugins, a `setup.py` as the rootdir; nor pytest's `-k` and `-m` from
//!   `addopts` or `PYTEST_ADDOPTS`, which a conftest's hooks may make match
//!   doctest items (by marking them) that aye can't see.
//! - **Marks**: where a conftest hook may mark doctest items
//!   (`pytest_collection_modifyitems` in any conftest of the project, as
//!   pytest calls it for every item; `pytest_itemcollected` in one in the
//!   file's directory or above, within the confcutdir; none with
//!   `--noconftest`), or a plugin aye can't read may (`-p NAME`,
//!   `PYTEST_PLUGINS`), a file's cases are `Partial`, so that the command
//!   line's `-k` and `-m` keep them (Core decision 5). The executor applies
//!   neither: a doctest pytest deselects through such a mark runs.
//! - **What is collected**: every `.py` file the walk reaches when modules
//!   are on (test modules and `conftest.py` included), except `__main__.py`
//!   and a `setup.py` mentioning setuptools or distutils, as pytest; the
//!   walk skips `__pycache__`, virtualenvs, `norecursedirs` (by default
//!   `*.egg`, `.*`, `_darcs`, `build`, `CVS`, `dist`, `node_modules`,
//!   `venv`, `{arch}`) and `--ignore`d paths.
//! - **Static recognition** parses each docstring as `DocTestParser` does
//!   (`parser.rs`), both as written (Python 3.10 to 3.12) and as the 3.13
//!   compiler cleans it, and walks the module's and its classes' namespaces
//!   as `DocTestFinder` does (`finder.rs`): functions (async too), classes,
//!   nested classes, methods, static and class methods, and properties,
//!   with the getter's docstring (a setter's or deleter's is never used),
//!   overload stubs replaced by their implementation. A file is refined,
//!   never guessed, wherever the source can't say what the namespaces hold
//!   once imported or what doctest makes of a docstring: definitions with
//!   doctests that may not run (in `if`, `try`, `for`, `while`, `with`,
//!   `match`; `if __name__ == "__main__"` never runs), names with doctests
//!   bound more than once or after a star import; decorators other than
//!   `staticmethod`, `classmethod`, `property` (and `@x.setter`,
//!   `@x.deleter`), `abstractmethod`, `final`, `override` and
//!   `no_type_check` on functions with doctests, or than `dataclass`,
//!   `total_ordering`, `final`, `unique`, `runtime_checkable` and
//!   `type_check_only` on classes with them (a `:=` in a function's or a
//!   lambda's default or in an annotation binds there too, the annotation's
//!   maybe, as `from __future__ import annotations` defers it); aliases and wrappers of objects
//!   with doctests (`g = f`, `staticmethod(f)`, `@wraps(f)`, a loop over
//!   them, instances of classes with doctests that may be descriptors);
//!   namespace changes at import (`setattr`, `globals()`, `exec`,
//!   `sys.modules`, three-argument `type()`, `X.__doc__`/`X.__module__`
//!   assignments, attributes set on the module's own functions and classes,
//!   module-level `__test__`, `__doc__` or `__name__`, class-level `__doc__`
//!   or `__module__`, `global` statements naming them), module functions or
//!   classes called at import that do any of that, make docstrings or
//!   return such an alias; metaclasses other than `ABCMeta` and `EnumMeta`,
//!   subclasses of a class defining `__init_subclass__`; a `raise` that
//!   always runs; docstrings doctest can't parse (a collection error in
//!   pytest), option names it doesn't define, carriage returns an escape
//!   may make; pytest's `importlib` mode or `consider_namespace_packages`
//!   (the import decides the name); a package directory whose name isn't
//!   ASCII, a file name holding a dot; `PYTHONOPTIMIZE=2` (no docstrings);
//!   and imports from project modules when a project file assigns
//!   `__module__` and either one spells the module's name in quotes (an
//!   object may claim to belong to it, as `numpy`'s `set_module` does) or
//!   the value is computed (anything but a string or a parameter of the
//!   function assigning it, or of one enclosing it: `__package__`,
//!   `f.__module__.rpartition(".")[0]`); `PY_IGNORE_IMPORTMISMATCH=1`
//!   (pytest then takes whatever module it finds under a file's name). A
//!   `conftest.py` outside a package that pytest finds already imported as
//!   `conftest` from another file when it collects it as a doctest module
//!   (an initial conftest in a `test*` directory, one in a directory that
//!   sorts before it: aye walks as pytest collects, depth first, entries by
//!   name, each directory's conftests imported before it) is pytest's
//!   "import file mismatch", a static collection error. A module that
//!   may skip itself while imported is refined too: a call at import (in
//!   any branch, or in a module function called then) to `skip`,
//!   `importorskip`, `xfail`, `fail` or `exit` (pytest's, or a method of
//!   that name), or a `raise` of `SkipTest`, `Skipped` or
//!   `pytest.skip.Exception`; other conditional `raise`s are not (version
//!   guards in `__init__.py` are everywhere). So is a module or text file
//!   with doctests below a conftest pytest loads for it (in its directory
//!   or above, within the confcutdir; none with `--noconftest`), but not
//!   at start-up (where a skip stops pytest instead), that may skip while
//!   loaded: a call to `skip` or `importorskip`, or a `raise` of
//!   `Skipped` or `pytest.skip.Exception`, in what runs at import or in a
//!   module function called then (`finder::may_skip_while_imported`), the
//!   usual guard of an optional dependency. pytest then skips the
//!   directory and collects no doctest below it; refinement says so once,
//!   on the directory. Text files are exact unless parsing raises, their
//!   encoding isn't UTF-8 or such a conftest is above them. The static
//!   cases of a refined file are `Partial`.
//! - **A module with no doctest aye can see is refined** (Core decisions 1
//!   and 5): pytest imports every module with doctest modules on, so an
//!   import error is its collection error (exit 2), which only the import
//!   can tell. Not refined: a module holding only constants and `pass` (an
//!   empty `__init__.py`), which surely imports; and, beside the pytest
//!   adapter, a `conftest.py` or a module matching pytest's
//!   `python_files`, whose import (and import error) that adapter reports.
//!   Clean zero-case answers aren't cached by collection, so the others are
//!   imported on every run, as pytest imports them.
//! - **Known gaps**, accepted: members a metaclass or `__init_subclass__`
//!   inherited from another module adds, objects code from another module
//!   puts in the module's namespaces at import, decorators from another
//!   module that give a function without doctests some, docstrings built at
//!   runtime from names; conftests' `collect_ignore`; pytest's doctest
//!   fixtures (`doctest_namespace`, `getfixture`, autouse fixtures), which
//!   don't exist natively; what a conftest's `pytest_plugins` load; what
//!   importing another collected module does (pytest imports every module
//!   it collects, in order, before running a test; a batch imports its
//!   own); a conftest's skip the recognizer can't see (one above the
//!   project root, a skip in another module's function), whose doctests
//!   below then finish `skipped` one by one, where pytest counts one skip.
//!
//! Decided for the executor (`python/aye_engine_doctest.py`), October 2026:
//! - **Imports as pytest's default import mode does**: the package root (or
//!   the file's directory) first on `sys.path`, `importlib.import_module`,
//!   then pytest's "import file mismatch" check (`PY_IGNORE_IMPORTMISMATCH`
//!   honoured); `append` mode appends. `importlib` mode and
//!   `consider_namespace_packages` go through pytest's own `import_path`
//!   when pytest is installed, else prepend. `pythonpath` goes first on
//!   `sys.path`. Every file a batch names is loaded and searched before any
//!   test runs, as pytest collects before running, with pytest's
//!   `DocTestFinder` adjustments (properties' and wrapped functions' line
//!   numbers before 3.12.3, `cached_property` before 3.13), and with
//!   `os.path`'s own functions in place (`aye_engine.unpatched_os_path`).
//!   Before a file, the conftests pytest has imported by the time it
//!   collects it are imported, when pytest is installed (they are its
//!   plugins): the initial ones the context lists (`_set_initial_conftests`:
//!   the roots' and their `test*` directories'; when the command line
//!   selects by more than paths, the executor works them out the same way
//!   from the paths the binary's `selected_paths` names, as `pytest <those
//!   paths>` loads them, since the recognizer saw only the path
//!   arguments), then each directory's from
//!   the top down to the file's, but those above the confcutdir; each once,
//!   as `_importconftest` (one outside a package replaces `conftest`), none
//!   with `--noconftest`, and none below one that failed or skipped, as
//!   pytest collects nothing there. Their side effects (`sys.path`,
//!   `os.environ`) reach the doctests, as in pytest. One that raises
//!   pytest's `skip` while loaded (`importorskip`, with or without
//!   `allow_module_level`) skips the files below it: their tests finish
//!   `skipped` with its reason, and refinement reports one `skipped` info,
//!   on the directory, as pytest reports one skip for it (and as the
//!   pytest adapter does). One that fails otherwise (`unittest.SkipTest`
//!   included, which pytest fails the directory for) is one diagnostic on
//!   the conftest's path, with the pytest adapter's message word for word
//!   (`ImportError while loading conftest 'sub/conftest.py'.`, `Failed …`
//!   for pytest's `fail` or `xfail`, `Skipped …` for a skip in an initial
//!   one; the user's frames; the exception), so renderers show the error
//!   both adapters meet once: a `collection-error`, or a `usage-error` for
//!   an initial conftest, which stops pytest before its session (exit 4; a
//!   skip there is an uncaught exception, exit 1). A worker reports a
//!   diagnostic once, whichever batch meets it again. The binary also
//!   keeps one error per conftest across adapters, whatever the words.
//! - **pytest's session**: its warning filters wrap collection and each
//!   test, as `catch_warnings_for_item` sets them (deprecation warnings
//!   shown unless `-W` or `PYTHONWARNINGS` says otherwise, then
//!   `filterwarnings`, then `-W`, which wins; a filter pytest would refuse
//!   is left out), so a warning `error` turns into a failed example or a
//!   collection error; warnings go to the captured `stderr` (pytest keeps
//!   them for its summary). With pytest installed, `PYTEST_VERSION` is set,
//!   and `PYTEST_CURRENT_TEST` (`<node id> (call)`) while a test's examples
//!   run, as pytest sets them; pytest itself isn't imported.
//! - **What pytest fails to collect fails here**: an import error, a
//!   `pytest.skip` without `allow_module_level`, a docstring doctest can't
//!   parse or a text file that can't be read is a `collection-error`
//!   diagnostic with pytest's message, and the file's tests don't run; a
//!   module skipped while importing (`unittest.SkipTest`,
//!   `pytest.skip(allow_module_level=True)`, or any import error with
//!   `--doctest-ignore-import-errors`) finishes its tests as `skipped` (an
//!   info diagnostic in refinement); so do the files below a conftest that
//!   skips or fails (see above). A requested test that doesn't exist is
//!   `not-found`.
//! - **Running.** Each `DocTest` runs with a `DocTestRunner` subclass and
//!   pytest's output checker (`ALLOW_UNICODE`, `ALLOW_BYTES` and `NUMBER`
//!   reimplemented without pytest) in its own copy of the module's globals,
//!   cleared afterwards. Every example runs, even after a failure (doctest's
//!   default; pytest stops at the first unless
//!   `--doctest-continue-on-failure`), and each is a `sub_outcome` named
//!   `line N: >>> source`, passed, failed or skipped. The test's outcome is
//!   pytest's: `skipped` when every example has its own `+SKIP` (none runs;
//!   a `SKIP` among the option flags, as `COMPARISON_FLAGS` holds, makes
//!   doctest pass over every example and the test pass, as in pytest), else
//!   `failed` when an example failed, else `passed`. pytest's `skip`,
//!   `xfail` and `fail` raised in an example end the test as in pytest
//!   (`skipped`, `expected_failure`, `failed`), and so does `FAIL_FAST`; but
//!   an example that failed before them makes the test `failed`, as pytest
//!   stops there, unless `--doctest-continue-on-failure`, where pytest
//!   reaches them too. Anything else an example raises, `unittest.SkipTest`
//!   and `SystemExit` included, fails that example, as in pytest.
//! - **Failures are structured**: a mismatch is a `DocTestFailure` with the
//!   assertion `got == expected` (left the output, right the expectation,
//!   the diff their unified diff, `-expected +got`, with blank lines of the
//!   output shown as `<BLANKLINE>`, as doctest's report shows them, unless
//!   `DONT_ACCEPT_BLANKLINE`), located at the example, with the example as
//!   its one frame; an unexpected exception carries its traceback, the
//!   example's frame mapped to the line it is written on. The detail is
//!   doctest's own report, with `--doctest-report`'s diff. A failure doctest
//!   can give no line for (a `__test__` string, an object it can't place)
//!   has the test and the example's position as its signature's place
//!   (`Failure.site`), so failures elsewhere that only read alike aren't
//!   grouped.
//! - **Policy and services**: `maxfail` and the context and cleanup services
//!   through aye's engine loop; with `capture` (and no capture service from
//!   the runner), `stderr`, and `stdout` printed outside examples, become the
//!   test's output, through `aye_engine.captured` (streams with `.buffer`,
//!   `.encoding` and `fileno()`, read even when an example closed them):
//!   doctest itself takes `stdout` to compare it. A
//!   `KeyboardInterrupt` stops the batch with a `batch-stopped` error naming
//!   the tests that never started, and status 2. The status is otherwise 1
//!   when a test failed or a file failed to load, else 0. A batch the
//!   runner re-queued after a crash leaves out the tests its context's
//!   `ran_ids` says ran, by count (a file's id stands for all its
//!   doctests). `Policy.timeout`, `keyword` and `tags` are not applied
//!   (static cases carry exact keywords). The executor never writes to
//!   stdout.
//! - **Reruns**: a test with a bad outcome carries its `reproduce` (aye's
//!   engine sets it): `aye`, aye's own options that shaped the run (the
//!   context's `rerun_args`: `-c FILE`), the id, then `--` and the
//!   arguments after it, without `--junitxml`/`--junit-xml`, whose report a
//!   rerun would overwrite.
//! - **Examples may patch what the executor uses**: durations come from
//!   `aye_engine.perf_counter`, capture from `aye_engine.CaptureIO` and
//!   tracebacks from `traceback`'s functions, all bound at import; paths go
//!   through `aye_engine.os_path` and the engine's `Paths`, and loading runs
//!   with `os.path`'s own functions in place; and a failure that can't be
//!   built in full is still reported (the example's message and line). An
//!   example that patches `time`, `io`, `os.path` or `traceback`, and leaves
//!   them patched, changes nothing reported, in its batch or a later one.
//! - **A `conftest.py` outside a package**, collected as a doctest module,
//!   is imported afresh when another one holds the name `conftest`: where
//!   pytest would find the other one, the recognizer reports the "import
//!   file mismatch" statically (see "Static recognition"), so a batch never
//!   asks for it.

mod config;
mod conftest;
mod context;
mod finder;
mod fnmatch;
mod parser;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aye_adapter::{
    Adapter, AdapterError, Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested,
    Setting, SourceFile,
};
use aye_events::{
    AdapterName, AffinityKey, Case, Certainty, Diagnostic, DiagnosticCode, Severity, Span,
};
use aye_protocol::{AdapterContext, EmbeddedModule, Executor};
use aye_static::{EnvVars, ProcessEnv};

pub use config::{DoctestConfig, ImportMode};

use crate::context::DoctestContext;

/// The doctest adapter's executor module.
pub const MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_engine_doctest",
    source: include_str!("../python/aye_engine_doctest.py"),
};

/// The adapter's name, in cases, batches and the registry.
pub const NAME: AdapterName = AdapterName::new_static("doctest");

/// The pytest adapter's name: beside it, pytest's default text glob
/// applies without a pytest configuration file.
const PYTEST: AdapterName = AdapterName::new_static("pytest");

/// The doctest adapter.
#[derive(Debug, Clone, Copy, Default)]
pub struct Doctest;

impl Adapter for Doctest {
    fn name(&self) -> AdapterName {
        NAME
    }

    /// A recognizer when modules or text files are enabled, else `None`.
    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        self.prepare_among(project, &[])
    }

    /// [`Doctest::prepare`], where pytest's default text glob (`test*.txt`)
    /// also applies without a pytest configuration file when the run uses
    /// the pytest adapter: pytest collects those files by default.
    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[AdapterName],
    ) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        let recognizer = DoctestRecognizer::among(project, adapters)?;
        if recognizer.config.active() {
            Ok(Some(Box::new(recognizer)))
        } else {
            Ok(None)
        }
    }

    fn executor(&self) -> Executor {
        Executor {
            adapter: NAME,
            modules: &[MODULE],
            entry: "aye_engine_doctest:DoctestExecutor",
            lists_session: false,
        }
    }
}

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

/// The environment variables the recognizer reads.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `PYTEST_ADDOPTS`.
    pub pytest_addopts: Option<String>,
    /// `PYTHONOPTIMIZE`.
    pub python_optimize: Option<String>,
    /// `PY_IGNORE_IMPORTMISMATCH=1`: pytest skips its "import file
    /// mismatch" check.
    pub ignore_import_mismatch: bool,
    /// `PYTEST_PLUGINS`: plugins aye can't read, which may mark items.
    pub pytest_plugins: Option<String>,
}

impl Env {
    /// The variables, as `env` holds them.
    pub fn of(env: &ProcessEnv) -> Self {
        Self {
            pytest_addopts: env.get(EnvVars::PYTEST_ADDOPTS).map(str::to_string),
            python_optimize: env.get(EnvVars::PYTHONOPTIMIZE).map(str::to_string),
            ignore_import_mismatch: env.get(EnvVars::PY_IGNORE_IMPORTMISMATCH) == Some("1"),
            pytest_plugins: env
                .get(EnvVars::PYTEST_PLUGINS)
                .filter(|v| !v.trim().is_empty())
                .map(str::to_string),
        }
    }
}

/// The doctest adapter's static face for one project.
#[derive(Debug)]
pub struct DoctestRecognizer {
    root: PathBuf,
    cwd: PathBuf,
    config: DoctestConfig,
    /// Where collection starts ([`Recognizer::roots`]).
    roots: Vec<PathBuf>,
    /// `PYTHONOPTIMIZE` strips docstrings.
    stripped: bool,
    /// `PY_IGNORE_IMPORTMISMATCH=1`.
    ignore_import_mismatch: bool,
    /// `PYTEST_PLUGINS` is set.
    env_plugins: bool,
    /// pytest's conftest rules for the run.
    conftests: conftest::Conftests,
    /// What the project's files say about `__module__` (found once, when
    /// needed).
    scan: OnceLock<ProjectScan>,
    /// The project's conftests, and what each may do to doctest items
    /// (found once, when needed).
    project_conftests: OnceLock<Vec<ProjectConftest>>,
    /// The context, built once (see [`Self::doctest_context`]).
    context: OnceLock<DoctestContext>,
}

/// A `conftest.py` of the project, as the recognizer reads it.
#[derive(Debug)]
struct ProjectConftest {
    path: PathBuf,
    /// Which of [`conftest::MARKING_HOOKS`] it names.
    hooks: [bool; 2],
    /// It may raise pytest's `skip` while loaded.
    skips: bool,
}

/// The project's Python files, read once: whether any assigns `__module__`,
/// whether any assigns it a value aye can't read (computed rather than a
/// quoted name or a function's parameter), and every dotted name any
/// spells in quotes.
#[derive(Debug, Default)]
struct ProjectScan {
    assigns_module: bool,
    computes_module: bool,
    quoted: std::collections::HashSet<String>,
}

/// The dotted names `source` spells in quotes (`"pkg.mod"`, `'pkg'`).
fn quoted_names(source: &str, out: &mut std::collections::HashSet<String>) {
    let bytes = source.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let quote = bytes[at];
        at += 1;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let start = at;
        while at < bytes.len()
            && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'_' | b'.'))
        {
            at += 1;
        }
        if at < bytes.len() && bytes[at] == quote && at > start {
            out.insert(source[start..at].to_string());
        }
    }
}

/// pytest's `_in_venv`.
fn is_virtualenv(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file() || dir.join("conda-meta").join("history").is_file()
}

/// `str.isidentifier()` for the ASCII names it is asked about here, and
/// `None` for any other name (Python's rules for those are Unicode's).
fn ascii_identifier(name: &str) -> Option<bool> {
    if !name.is_ascii() {
        return None;
    }
    Some(
        name.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
    )
}

/// The name pytest's `prepend` import mode imports a file under, and the
/// directory it puts on `sys.path`: pytest's `resolve_pkg_root_and_module_name`.
/// `Err` holds why the name can't be known statically.
pub fn module_name(file: &Path) -> Result<(String, PathBuf), String> {
    let mut package: Option<&Path> = None;
    for dir in file.ancestors().skip(1) {
        if !dir.join("__init__.py").is_file() {
            break;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match ascii_identifier(&name) {
            Some(true) => package = Some(dir),
            Some(false) => break,
            None => return Err(format!("the package name {name:?} is not ASCII")),
        }
    }
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if stem.contains('.') {
        return Err(format!("the file name {stem:?} holds a dot"));
    }
    let Some(package) = package else {
        let dir = file.parent().unwrap_or(file).to_path_buf();
        return Ok((stem, dir));
    };
    let base = package.parent().unwrap_or(package);
    let rel = file.with_extension("");
    let rel = rel.strip_prefix(base).unwrap_or(&rel);
    let mut names: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if names.last().is_some_and(|n| n == "__init__") {
        names.pop();
    }
    Ok((names.join("."), base.to_path_buf()))
}

/// Why a module with no doctest aye can see is refined.
const NO_STATIC_DOCTESTS: &str =
    "no doctest is visible statically; pytest imports the module all the same";

/// Whether importing a module surely succeeds: it holds nothing but
/// constants (a docstring, `...`) and `pass`, as most packages'
/// `__init__.py` files do (or nothing at all).
fn surely_imports(module: &aye_adapter::ast::ModModule) -> bool {
    use aye_adapter::ast::{Expr, Stmt};
    module.body.iter().all(|stmt| match stmt {
        Stmt::Pass(_) => true,
        Stmt::Expr(expr) => matches!(
            expr.value.as_ref(),
            Expr::StringLiteral(_)
                | Expr::BytesLiteral(_)
                | Expr::NumberLiteral(_)
                | Expr::BooleanLiteral(_)
                | Expr::NoneLiteral(_)
                | Expr::EllipsisLiteral(_)
        ),
        _ => false,
    })
}

/// Whether a source that [`assigns_module`] assigns it a value aye can't
/// read: neither a string nor a parameter of the function doing it (whose
/// callers spell the name in quotes, as `numpy`'s `set_module("numpy")`).
/// `f.__module__ = __package__`, `__module__ = __name__.rpartition(".")[0]`
/// in a class body, and `setattr` or `type()` given anything but a string,
/// are computed; so is a source that doesn't parse.
fn computes_module(source: &str) -> bool {
    use aye_adapter::ast::visitor::{self, Visitor};
    use aye_adapter::ast::{Expr, Stmt};

    struct Assignments<'a> {
        params: Vec<Vec<&'a str>>,
        computed: bool,
    }
    impl<'a> Assignments<'a> {
        fn value(&mut self, value: &'a Expr) {
            let known = match value {
                Expr::StringLiteral(_) => true,
                // A parameter of the function, or of one enclosing it (a
                // decorator factory's argument).
                Expr::Name(name) => self
                    .params
                    .iter()
                    .any(|params| params.contains(&name.id.as_str())),
                _ => false,
            };
            self.computed |= !known;
        }
        fn target(&mut self, target: &'a Expr, value: &'a Expr) {
            let module = match target {
                Expr::Attribute(attr) => attr.attr.as_str() == "__module__",
                Expr::Name(name) => name.id.as_str() == "__module__",
                _ => false,
            };
            if module {
                self.value(value);
            }
        }
    }
    impl<'a> Visitor<'a> for Assignments<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(def) => {
                    let p = &def.parameters;
                    let names = p
                        .posonlyargs
                        .iter()
                        .chain(&p.args)
                        .chain(&p.kwonlyargs)
                        .map(|a| a.parameter.name.as_str())
                        .chain(p.vararg.iter().map(|a| a.name.as_str()))
                        .chain(p.kwarg.iter().map(|a| a.name.as_str()))
                        .collect();
                    self.params.push(names);
                    visitor::walk_stmt(self, stmt);
                    self.params.pop();
                    return;
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.target(target, &assign.value);
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        self.target(&assign.target, value);
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Call(call) => {
                    let args = &call.arguments.args;
                    let names_module = |e: &Expr| matches!(e, Expr::StringLiteral(s) if s.value.to_str() == "__module__");
                    if args.len() == 3 && names_module(&args[1]) {
                        self.value(&args[2]);
                    }
                }
                Expr::Dict(dict) => {
                    for item in &dict.items {
                        if let Some(Expr::StringLiteral(key)) = &item.key
                            && key.value.to_str() == "__module__"
                        {
                            self.value(&item.value);
                        }
                    }
                }
                _ => {}
            }
            visitor::walk_expr(self, expr);
        }
    }

    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return true;
    };
    let mut found = Assignments {
        params: Vec::new(),
        computed: false,
    };
    found.visit_body(&parsed.syntax().body);
    found.computed
}

/// Whether a source assigns `__module__` (`X.__module__ = …`, `__module__
/// = …` in a class body) or names it in a string (`setattr`, `type()`).
fn assigns_module(source: &str) -> bool {
    let mut rest = source;
    while let Some(at) = rest.find("__module__") {
        if matches!(rest[..at].chars().next_back(), Some('"' | '\'')) {
            return true;
        }
        let after = &rest[at + "__module__".len()..];
        let trimmed = after.trim_start_matches([' ', '\t']);
        if trimmed.starts_with('=') && !trimmed.starts_with("==") {
            return true;
        }
        rest = after;
    }
    false
}

impl DoctestRecognizer {
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        Self::among(project, &[])
    }

    /// [`Self::new`] for a run that uses `adapters` (by name): beside
    /// `pytest`, `test*.txt` files are doctests without a pytest
    /// configuration file too.
    pub fn among(project: &Project<'_>, adapters: &[AdapterName]) -> Result<Self, AdapterError> {
        Self::with_environment(project, adapters, &Env::of(project.env))
    }

    /// [`Self::new`] with `PYTEST_ADDOPTS` and `PYTHONOPTIMIZE` given
    /// rather than taken from the project's environment.
    pub fn with_env(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
        python_optimize: Option<&str>,
    ) -> Result<Self, AdapterError> {
        Self::with_env_among(project, &[], pytest_addopts, python_optimize)
    }

    /// [`Self::among`] with `PYTEST_ADDOPTS` and `PYTHONOPTIMIZE` given,
    /// and no other variable set.
    pub fn with_env_among(
        project: &Project<'_>,
        adapters: &[AdapterName],
        pytest_addopts: Option<&str>,
        python_optimize: Option<&str>,
    ) -> Result<Self, AdapterError> {
        let env = Env {
            pytest_addopts: pytest_addopts.map(str::to_string),
            python_optimize: python_optimize.map(str::to_string),
            ..Env::default()
        };
        Self::with_environment(project, adapters, &env)
    }

    /// [`Self::among`] with the environment variables given rather than
    /// taken from the project's environment.
    pub fn with_environment(
        project: &Project<'_>,
        adapters: &[AdapterName],
        env: &Env,
    ) -> Result<Self, AdapterError> {
        let pyproject = project.root.join("pyproject.toml");
        let table = project
            .tool_aye
            .doctest
            .as_ref()
            .map(|t| (t, pyproject.as_path()));
        let beside_pytest = adapters.contains(&PYTEST);
        let config =
            DoctestConfig::discover(project, env.pytest_addopts.as_deref(), table, beside_pytest)?;
        // `PYTHONOPTIMIZE=2` is `-OO`; any other non-empty value is `-O`.
        let stripped = env
            .python_optimize
            .as_deref()
            .is_some_and(|v| v.trim().parse::<u32>().is_ok_and(|n| n >= 2));
        let conftests = conftest::Conftests {
            disabled: config.noconftest,
            confcutdir: config.confcutdir.clone(),
        };
        let mut recognizer = Self {
            root: project.root.to_path_buf(),
            cwd: project.cwd.to_path_buf(),
            config,
            roots: Vec::new(),
            stripped,
            ignore_import_mismatch: env.ignore_import_mismatch,
            env_plugins: env.pytest_plugins.is_some(),
            conftests,
            scan: OnceLock::new(),
            project_conftests: OnceLock::new(),
            context: OnceLock::new(),
        };
        recognizer.roots = recognizer.find_roots(project);
        Ok(recognizer)
    }

    /// The resolved configuration.
    pub fn config(&self) -> &DoctestConfig {
        &self.config
    }

    /// The command-line paths; else `testpaths`, glob-expanded, when run
    /// from the rootdir; else the current directory.
    fn find_roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if !project.args.is_empty() {
            return project
                .args
                .iter()
                .map(|a| aye_fs::absolute(project.cwd, a))
                .collect();
        }
        if project.cwd == self.config.rootdir && !self.config.testpaths.is_empty() {
            let mut roots = Vec::new();
            for pattern in &self.config.testpaths {
                for found in glob(&self.config.rootdir, pattern) {
                    if !roots.contains(&found) {
                        roots.push(found);
                    }
                }
            }
            if !roots.is_empty() {
                return roots;
            }
        }
        vec![project.cwd.to_path_buf()]
    }

    /// `--ignore` and `--ignore-glob`.
    fn ignored(&self, path: &Path) -> bool {
        if self.config.ignore.iter().any(|p| p == path) {
            return true;
        }
        let text = path.to_string_lossy();
        self.config
            .ignore_glob
            .iter()
            .any(|glob| fnmatch::fnmatch(&text, glob))
    }

    /// The project's files, read once: does any assign `__module__`, and
    /// which dotted names do they spell in quotes.
    fn project_scan(&self) -> &ProjectScan {
        self.scan.get_or_init(|| {
            let mut scan = ProjectScan::default();
            let mut stack = vec![self.root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                    } else if path.extension().is_some_and(|e| e == "py")
                        && let Ok(source) = std::fs::read_to_string(&path)
                    {
                        if assigns_module(&source) {
                            scan.assigns_module = true;
                            scan.computes_module |= computes_module(&source);
                        }
                        quoted_names(&source, &mut scan.quoted);
                    }
                }
            }
            scan
        })
    }

    /// Whether an object a project module defines may claim, through its
    /// `__module__`, to belong to the module `name`: some project file
    /// assigns `__module__`, and spells `name` in quotes or computes the
    /// value.
    fn may_be_claimed(&self, name: &str) -> bool {
        let scan = self.project_scan();
        scan.assigns_module && (scan.computes_module || scan.quoted.contains(name))
    }

    /// The project's conftests (in the directories collection enters),
    /// which of [`conftest::MARKING_HOOKS`] each names, and whether each may
    /// skip while loaded; none with `--noconftest`.
    fn project_conftests(&self) -> &[ProjectConftest] {
        self.project_conftests.get_or_init(|| {
            let mut found = Vec::new();
            if self.conftests.disabled {
                return found;
            }
            // A skip in one pytest loads at start-up stops pytest (an
            // uncaught exception): it skips no directory.
            let initial = self.conftests.initial(&self.roots, &self.cwd);
            let mut stack = vec![self.root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                    } else if entry.file_name() == "conftest.py"
                        && let Ok(source) = std::fs::read_to_string(&path)
                    {
                        let hooks =
                            conftest::MARKING_HOOKS.map(|h| conftest::names_hook(&source, h));
                        // One that doesn't parse fails its directory: no skip.
                        let skips = !initial.contains(&path)
                            && ruff_python_parser::parse_module(&source).is_ok_and(|parsed| {
                                finder::may_skip_while_imported(parsed.syntax())
                            });
                        found.push(ProjectConftest { path, hooks, skips });
                    }
                }
            }
            found
        })
    }

    /// Why the tags (marks) and keywords of a file's cases may be more than
    /// the source says: a conftest hook in scope that may mark doctest
    /// items (`pytest_collection_modifyitems` from any conftest,
    /// `pytest_itemcollected` from one in the file's directory or above),
    /// or plugins aye can't read (`-p NAME`, `PYTEST_PLUGINS`).
    fn may_be_marked(&self, file: &Path) -> Option<String> {
        if let Some(plugin) = self.config.plugins.first() {
            return Some(format!("the plugin {plugin} (-p) may mark doctest items"));
        }
        if self.env_plugins {
            return Some("PYTEST_PLUGINS may mark doctest items".into());
        }
        let dir = file.parent().unwrap_or(file);
        for conftest in self.project_conftests() {
            let [modifyitems, itemcollected] = conftest.hooks;
            if modifyitems || (itemcollected && self.loaded_for(&conftest.path, dir)) {
                return Some(format!(
                    "{} may mark doctest items",
                    conftest.path.display()
                ));
            }
        }
        None
    }

    /// Whether pytest loads `conftest` before collecting the files of `dir`
    /// (`_loadconftestmodules`): it is in `dir` or above, within the
    /// confcutdir.
    fn loaded_for(&self, conftest: &Path, dir: &Path) -> bool {
        conftest
            .parent()
            .is_some_and(|d| dir.starts_with(d) && self.conftests.in_confcutdir(d))
    }

    /// Why `file`'s doctests are refined when a conftest pytest loads
    /// before collecting it (not at start-up) may skip while loaded
    /// (`pytest.importorskip`): pytest then skips that directory, and
    /// collects no doctest below it.
    fn skipping_conftest(&self, file: &Path) -> Option<String> {
        let dir = file.parent().unwrap_or(file);
        let conftest = self
            .project_conftests()
            .iter()
            .find(|c| c.skips && self.loaded_for(&c.path, dir))?;
        let shown = conftest.path.strip_prefix(&self.root).map_or_else(
            |_| conftest.path.display().to_string(),
            |rel| rel.to_string_lossy().replace('\\', "/"),
        );
        Some(format!(
            "{shown} may skip while pytest loads it, which skips the doctests below it"
        ))
    }

    /// For a `conftest.py` outside a package, collected as a doctest
    /// module: the other `conftest.py` pytest has imported as `conftest`
    /// (`_importconftest`) by the time it collects it, if any, which makes
    /// it pytest's "import file mismatch". pytest imports the initial
    /// conftests first, then collects depth first, entries sorted by name,
    /// importing each directory's conftests before collecting it; a
    /// conftest already imported isn't imported again.
    fn conftest_mismatch(&self, target: &Path) -> Option<PathBuf> {
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

    /// Whether `from … import` with `level` and `module` reads a project
    /// module: any relative import, or a top-level name found as a package
    /// or module where imports look in the project.
    fn imports_project(&self, level: u32, module: Option<&str>, pkg_root: &Path) -> bool {
        if level > 0 {
            return true;
        }
        let Some(top) = module.and_then(|m| m.split('.').next()) else {
            return false;
        };
        let mut bases = vec![
            pkg_root.to_path_buf(),
            self.root.clone(),
            self.root.join("src"),
            self.config.rootdir.clone(),
        ];
        bases.extend(self.config.pythonpath.iter().cloned());
        bases
            .iter()
            .any(|base| base.join(top).is_dir() || base.join(format!("{top}.py")).is_file())
    }

    /// The names `-k` matches for a file: the directories between pytest's
    /// rootdir and the file, and its name.
    fn path_keywords(&self, file: &SourceFile<'_>) -> Vec<String> {
        match file.abs.strip_prefix(&self.config.rootdir) {
            Ok(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect(),
            Err(_) => file.rel.as_str().split('/').map(str::to_string).collect(),
        }
    }

    fn case(
        &self,
        file: &SourceFile<'_>,
        name: &str,
        span: Span,
        affinity: Vec<AffinityKey>,
    ) -> Case {
        let mut case = Case::new(format!("{}::{name}", file.rel), NAME);
        case.span = Some(span);
        let mut keywords = self.path_keywords(file);
        if !keywords.iter().any(|k| k == name) {
            keywords.push(name.to_string());
        }
        case.keywords = keywords;
        case.affinity = affinity;
        case.certainty = Certainty::Exact;
        case
    }

    fn recognize_text(&self, file: &SourceFile<'_>) -> Recognition {
        let mut recognition = Recognition::default();
        let encoding = self
            .config
            .encoding
            .value
            .to_lowercase()
            .replace(['-', '_'], "");
        if !matches!(encoding.as_str(), "utf8" | "u8" | "utf" | "cp65001") {
            recognition.refine = Some(format!(
                "text files are read as {}",
                self.config.encoding.value
            ));
        }
        let doc = parser::text_file(&parser::universal_newlines(file.source));
        if let Some(why) = doc.unsure {
            recognition.refine = Some(why);
        }
        if doc.examples {
            if let Some(why) = self.skipping_conftest(file.abs) {
                recognition.refine = Some(why);
            }
            let name = file.rel.file_name();
            let span = Span {
                path: file.rel.to_project_path(),
                start: 0,
                end: 0,
                line: Some(1),
            };
            let case = self.case(file, name, span, vec![AffinityKey::file(file.rel)]);
            recognition.cases.push(case);
        }
        recognition
    }

    /// Whether the pytest adapter, beside this one, imports `file` and so
    /// reports its import error: a `conftest.py`, or a module matching
    /// pytest's `python_files`.
    fn pytest_adapter_imports(&self, file: &Path) -> bool {
        self.config.beside_pytest
            && (file.file_name().is_some_and(|n| n == "conftest.py")
                || self
                    .config
                    .python_files
                    .iter()
                    .any(|glob| fnmatch::fnmatch_ex(glob, file)))
    }

    fn recognize_module(&self, file: &SourceFile<'_>) -> Recognition {
        let mut recognition = Recognition::default();
        let mut reasons: Vec<String> = Vec::new();
        if self.stripped {
            reasons.push("PYTHONOPTIMIZE=2 strips docstrings".into());
        }
        if self.config.import_mode == ImportMode::Importlib {
            reasons.push("pytest's importlib import mode names the module".into());
        }
        if self.config.consider_namespace_packages {
            reasons.push("consider_namespace_packages: the import names the module".into());
        }
        let (name, pkg_root) = match module_name(file.abs) {
            Ok(found) => found,
            Err(why) => {
                reasons.push(why);
                let stem = file
                    .abs
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (stem, file.abs.parent().unwrap_or(file.abs).to_path_buf())
            }
        };
        if self.ignore_import_mismatch {
            reasons.push(format!(
                "PY_IGNORE_IMPORTMISMATCH=1: pytest takes whichever module it finds as {name:?}, \
                 maybe another file's"
            ));
        }
        if file.abs.file_name().is_some_and(|n| n == "conftest.py")
            && let Some(other) = self.conftest_mismatch(file.abs)
        {
            // pytest finds the other one under `conftest`: its import file
            // mismatch, a collection error.
            recognition.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::CollectionError,
                path: file.rel.to_project_path().into(),
                span: None,
                message: format!(
                    "import file mismatch:\nimported module 'conftest' has this __file__ \
                     attribute:\n  {}\nwhich is not the same as the test file we want to \
                     collect:\n  {}\nHINT: remove __pycache__ / .pyc files and/or use a unique \
                     basename for your test file modules",
                    other.display(),
                    file.abs.display()
                ),
            });
            return recognition;
        }
        let finding = finder::Finder::new(file).find(&name);
        if let Some(why) = finding.refine {
            reasons.push(why);
        }
        if finding
            .from_imports
            .iter()
            .any(|(level, module)| self.imports_project(*level, module.as_deref(), &pkg_root))
            && self.may_be_claimed(&name)
        {
            reasons.push(format!(
                "it imports from project modules, and a project file assigns `__module__` \
                 (maybe {name:?})"
            ));
        }
        if !finding.tests.is_empty()
            && let Some(why) = self.skipping_conftest(file.abs)
        {
            reasons.push(why);
        }
        if finding.tests.is_empty()
            && reasons.is_empty()
            && !surely_imports(file.module)
            && !self.pytest_adapter_imports(file.abs)
        {
            // pytest imports every module all the same: an import error is
            // its collection error.
            reasons.push(NO_STATIC_DOCTESTS.to_string());
        }
        let affinity = vec![AffinityKey::import(&name), AffinityKey::file(file.rel)];
        for test in finding.tests {
            let span = file.span(test.range);
            let case = self.case(file, &test.name, span, affinity.clone());
            recognition.cases.push(case);
        }
        if !reasons.is_empty() {
            recognition.refine = Some(reasons.join("; "));
        }
        recognition
    }
}

impl Recognizer for DoctestRecognizer {
    fn roots(&self, _project: &Project<'_>) -> Vec<PathBuf> {
        self.roots.clone()
    }

    /// pytest's `pytest_ignore_collect` for a directory: `__pycache__`,
    /// virtualenvs, `norecursedirs`, `--ignore` and `--ignore-glob`.
    fn descend(&self, dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if name == "__pycache__" {
            return false;
        }
        if !self.config.collect_in_virtualenv && is_virtualenv(dir) {
            return false;
        }
        if self
            .config
            .norecursedirs
            .iter()
            .any(|pattern| fnmatch::fnmatch_ex(pattern, dir))
        {
            return false;
        }
        !self.ignored(dir)
    }

    /// pytest's doctest `pytest_collect_file`: a `.py` file (not
    /// `__main__.py`, nor a `setup.py` mentioning setuptools or distutils)
    /// when modules are on; another file when it matches a glob, or is a
    /// `.txt` or `.rst` named on the command line.
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let explicit = requested.is_named();
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let abs = self.root.join(rel);
        let extension = Path::new(name).extension().map(|e| e.to_string_lossy());
        if extension.as_deref() == Some("py") {
            if !self.config.modules.value || name == "__main__.py" {
                return Claim::Ignore;
            }
            if name == "setup.py"
                && std::fs::read(&abs).is_ok_and(|bytes| {
                    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
                    has(b"setuptools") || has(b"distutils")
                })
            {
                return Claim::Ignore;
            }
            if !explicit && self.ignored(&abs) {
                return Claim::Ignore;
            }
            return Claim::Tests;
        }
        if explicit && matches!(extension.as_deref(), Some("txt" | "rst")) {
            return Claim::Tests;
        }
        if !explicit && self.ignored(&abs) {
            return Claim::Ignore;
        }
        if self
            .config
            .globs
            .value
            .iter()
            .any(|glob| fnmatch::fnmatch_ex(glob, &abs))
        {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    /// A file's cases; when it must be refined, they are only a guess
    /// (`Partial`), which stands when refinement is off or fails. Their
    /// tags and keywords are `Partial` too where a hook or a plugin may
    /// mark doctest items, so that `-k` and `-m` keep them (Core decision
    /// 5).
    fn recognize(
        &self,
        file: &SourceFile<'_>,
        _requested: Requested,
        _support: &[SourceFile<'_>],
    ) -> Recognition {
        let mut recognition = if file.abs.extension().is_some_and(|e| e == "py") {
            self.recognize_module(file)
        } else {
            self.recognize_text(file)
        };
        if recognition.refine.is_some()
            || (!recognition.cases.is_empty() && self.may_be_marked(file.abs).is_some())
        {
            for case in &mut recognition.cases {
                case.certainty = Certainty::Partial;
            }
        }
        recognition
    }

    fn settings(&self) -> Vec<Setting> {
        self.config.settings(&self.root)
    }

    /// What the executor needs: pytest's rootdir (for `importlib` mode),
    /// the option flags by name, the text files' encoding, the import mode,
    /// `pythonpath`, the `--doctest-report` choice and
    /// `--doctest-ignore-import-errors`: [`DoctestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.doctest_context())
    }

    /// What the context's keys for the runner say
    /// ([`DoctestContext::defaults`]).
    fn defaults(&self) -> FrameworkDefaults {
        self.doctest_context().defaults()
    }
}

impl DoctestRecognizer {
    /// The context ([`Recognizer::context`]), built the first time it is
    /// asked for.
    fn doctest_context(&self) -> &DoctestContext {
        self.context.get_or_init(|| DoctestContext::new(self))
    }

    /// pytest's `--deselect` prefixes as prefixes of aye's ids, which are
    /// relative to the project root rather than the rootdir; none when the
    /// rootdir is outside the root. The binary drops this adapter's cases
    /// they name and counts them as deselected, as pytest does with its
    /// doctest items.
    fn deselect_ids(&self) -> Vec<String> {
        let Ok(rel) = self.config.rootdir.strip_prefix(&self.root) else {
            return Vec::new();
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        self.config
            .deselect
            .iter()
            .map(|prefix| {
                if rel.is_empty() {
                    prefix.clone()
                } else {
                    format!("{rel}/{prefix}")
                }
            })
            .collect()
    }
}

/// Python's `glob.glob(pattern, recursive=True)` from `base`: the existing
/// paths matching `pattern`, sorted per directory, `**` matching any depth.
fn glob(base: &Path, pattern: &str) -> Vec<PathBuf> {
    let absolute = aye_fs::absolute(base, Path::new(pattern));
    let has_magic = |s: &str| s.contains(['*', '?', '[']);
    if !has_magic(pattern) {
        return if absolute.exists() {
            vec![absolute]
        } else {
            Vec::new()
        };
    }
    // Split the pattern after the longest literal prefix.
    let mut prefix = PathBuf::new();
    let mut parts: Vec<String> = Vec::new();
    for component in absolute.components() {
        let text = component.as_os_str().to_string_lossy().into_owned();
        if parts.is_empty() && !has_magic(&text) {
            prefix.push(component);
        } else {
            parts.push(text);
        }
    }
    let mut found = Vec::new();
    expand(&prefix, &parts, &mut found);
    // `sorted(glob.iglob(...))`: by the whole path.
    found.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    found.dedup();
    found
}

fn expand(dir: &Path, parts: &[String], out: &mut Vec<PathBuf>) {
    let Some((part, rest)) = parts.split_first() else {
        if dir.exists() {
            out.push(dir.to_path_buf());
        }
        return;
    };
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    if part == "**" {
        // Zero directories, then each subdirectory, recursively (hidden
        // ones aside, as glob does).
        expand(dir, rest, out);
        for entry in &entries {
            let hidden = entry
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if entry.is_dir() && !hidden {
                expand(entry, parts, out);
            }
        }
        return;
    }
    for entry in entries {
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') && !part.starts_with('.') {
            continue;
        }
        if fnmatch::fnmatch(&name, part) && (rest.is_empty() || entry.is_dir()) {
            expand(&entry, rest, out);
        }
    }
}
