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
//! Depends on: aye-adapter, aye-config (`[tool.aye.doctest]`, where a
//! setting came from), aye-events, aye-protocol, aye-pytest (pytest's
//! configuration, read approximately; its conftest rules and module names;
//! the variables it reads), aye-python-semantic (the `__main__` guard),
//! aye-python-stdlib (`fnmatch`, `glob`, `os.path`).
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **This crate is the doctest adapter, and it is native**: it maps
//!   doctest onto aye's engine and never goes through pytest. Ids and
//!   outcomes follow pytest 9.1.1's doctest plugin (`_pytest/doctest.py`),
//!   so plain `pytest --doctest-modules` / `--doctest-glob` is the referee.
//!   The executor (`python/aye_engine_doctest.py`) imports only the standard
//!   library and runs on Python 3.10 and later, with or without pytest. The
//!   crate reads pytest's configuration, its conftest rules and the
//!   variables it reads through `aye-pytest`, and `[tool.aye]` through
//!   `aye-config`. Per-example re-runs and `--accept` are not implemented
//!   yet.
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
//! - **pytest's configuration is approximated**, by aye-pytest's
//!   `ConfigRules::Approximate`, aye 0.0.1's rules, until the adapter switches
//!   to pytest's own: the first file holding pytest settings (`pytest.toml`,
//!   `.pytest.toml`, `pytest.ini`, `.pytest.ini`, `pyproject.toml` with
//!   `[tool.pytest]` or `[tool.pytest.ini_options]`, `tox.ini` with
//!   `[pytest]`, `setup.cfg` with `[tool:pytest]`, in that order in each
//!   directory) searching up from the common directory of the paths given, or
//!   the current one, as pytest's `locate_config` does; `-c` and `--rootdir`
//!   from the command line or the arguments after `--`; `-o` overrides
//!   (`-o addopts=` from the environment or the command line). Read from it:
//!   `addopts`, `testpaths` (only when run from the rootdir, glob-expanded, as
//!   pytest), `norecursedirs`, `pythonpath`, `consider_namespace_packages`,
//!   `doctest_optionflags`, `doctest_encoding`, `filterwarnings` and the
//!   `junit_*` settings; from the options also `--ignore`, `--ignore-glob`,
//!   `--collect-in-virtualenv`, `--import-mode`, `--deselect` (whose prefixes
//!   the context's `deselect` gives as prefixes of aye's ids: the binary drops
//!   the cases they name and counts them, as pytest deselects its doctest
//!   items), `-W`/`--pythonwarnings`, `--noconftest`, `--confcutdir`,
//!   `--doctest-continue-on-failure`, and `-x`/`--exitfirst`/`--maxfail` and
//!   `--junitxml`/`--junit-xml`/`--junitprefix`, which the context gives as
//!   the pytest adapter's does (`maxfail`, `junitxml`, `junit_prefix`,
//!   `junit_*`), for the binary to make them the run's when its command line
//!   gives none. Not read: conftests' `collect_ignore` and their hooks (but
//!   see "Marks" below), plugins, a `setup.py` as the rootdir; nor pytest's
//!   `-k` and `-m` from `addopts` or `PYTEST_ADDOPTS`, which a conftest's
//!   hooks may make match doctest items (by marking them) that aye can't see.
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
mod conftests;
mod context;
mod finder;
mod mismatch;
mod module;
mod parser;
mod project_scan;
mod recognizer;
mod text;

#[cfg(test)]
mod tests;

use aye_adapter::{Adapter, AdapterError, Project, Recognizer};
use aye_events::AdapterName;
use aye_protocol::{EntryPoint, ModuleSet, ModuleSource, Runtime, SessionMode};
use aye_pytest::names;
use aye_python_stdlib::fnmatch::SetTranslation;

pub use aye_pytest::env::DoctestEnv;
pub use config::{DoctestConfig, ImportMode};
pub use recognizer::DoctestRecognizer;

/// The doctest adapter's executor module.
pub const MODULE: ModuleSource = ModuleSource::embedded(
    "aye_engine_doctest",
    include_str!("../python/aye_engine_doctest.py"),
);

/// What a worker installs for the doctest adapter's executor, after aye's
/// engine and the prelude the binary gives every worker (aye's `pytest`
/// module and what it stands on).
pub const MODULES: ModuleSet = ModuleSet::new(const { &[MODULE] });

/// The adapter's name, in cases, batches and the registry.
pub const NAME: AdapterName = names::DOCTEST;

/// How pytest's patterns read `[...]` sets here (`--doctest-glob`,
/// `norecursedirs`, `--ignore-glob` and `testpaths`): as the pytest
/// adapter reads them, which differs from CPython for a set whose first
/// item is a reversed range followed by `!`.
const SETS: SetTranslation = SetTranslation::Legacy;

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

    fn runtime(&self) -> Runtime {
        Runtime {
            adapter: NAME,
            modules: MODULES,
            entry: EntryPoint::new("aye_engine_doctest", "DoctestExecutor"),
            session: SessionMode::PerBatch,
        }
    }
}
