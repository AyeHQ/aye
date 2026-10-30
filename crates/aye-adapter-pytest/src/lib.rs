//! The pytest adapter, a faithful implementation of pytest's rules: pytest
//! is a front-end of aye's engine, and pytest itself is never imported.
//!
//! Rust half: pytest's configuration (rootdir, inifile, ini values and the
//! options `addopts`, `PYTEST_ADDOPTS` and the arguments after `--` add,
//! with what the runner takes over lifted out), the files it claims, the
//! context the front-end starts from ([`native`]), and the static
//! recognizer: file patterns, `Test*` classes and `test*` functions from
//! the project's pytest config, inherited methods, markers, parametrize
//! and pytest's id algorithm (including `pytest.param`, `ids=`, stacked
//! decorators, duplicate suffixes, escaping), conftest parsing for fixtures
//! and collection-affecting hooks, pytest's rootdir and ini semantics
//! (asked from `aye-config`'s loaders). The static recognizer predicts the
//! cases `aye --co --no-refine` lists without importing anything; every
//! other run has the front-end list each test file (see [`native`]).
//!
//! Python half (embedded, standard library only): aye's own `pytest`
//! module (`python/native/aye_pytest_shim.py`, the authoring API, which the
//! binary installs in every worker), the pytest front-end
//! (`aye_pytest_frontend.py`), which reads a suite into aye's model and
//! never runs a test, and the executor (`aye_pytest_native.py`), which
//! hands that model to aye's engine.
//!
//! Owns: everything pytest-specific. Nothing here is questioned; the
//! differential harness against plain pytest is the referee.
//!
//! Depends on: aye-adapter, aye-config (the INI and TOML loaders, where a
//! setting came from, the worker count), aye-engine (the modules its
//! front-end runs on), aye-events, aye-fs (path algebra), aye-protocol,
//! aye-static (the variables it reads by name). Must not be depended on by
//! any runner crate; only the binary links it.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The project scan covers what pytest's session can load**: the
//!   directories the run collects (the paths given, else `testpaths` or the
//!   current directory) and the conftests above them up to the root, not the
//!   whole project. pytest loads no other conftest, so hooks, `sys.path`
//!   changes and colliding module names elsewhere can't matter, and a run
//!   of one directory of a large repository doesn't walk all of it.
//! - **The reference is pytest 9.1.1.** Config discovery follows
//!   `_pytest/config/findpaths.py` (including `pytest.toml`, native
//!   `[tool.pytest]` with its strict types, and a bare `pyproject.toml`
//!   becoming the inifile when nothing else is found); ids follow `IdMaker`
//!   in `_pytest/python.py` and `ascii_escaped` in `_pytest/compat.py`;
//!   collection follows `PyCollector.collect`, `Class.collect` and
//!   `UnitTestCase.collect` (unittest methods sorted by name, parametrize not
//!   applied). Venvs are detected as pytest does (`pyvenv.cfg` or
//!   `conda-meta/history`). The crate also depends on `aye-config`, for its
//!   loaders, on `aye-fs` for paths, and on `aye-static` for the variables
//!   it reads. [`Pytest::prepare`] always returns a recognizer: pytest is
//!   the default for Python tests.
//! - **Canonical ids are relative to the aye project root**; pytest's own
//!   node ids are relative to its rootdir. The two differ only in the file
//!   part, which the front-end translates using the rootdir in
//!   [`Recognizer::context`].
//! - **Static values.** Ids are computed from literals (strings with any
//!   escapes, bytes, ints in any base, floats and complex numbers printed as
//!   Python prints them, booleans, `None`), unary minus, `range(...)` and
//!   `float("inf")`-style calls, `re.compile(...)` of patterns known to
//!   compile, lambdas (`<lambda>`) and generators (`<genexpr>`), builtins
//!   and classes or functions of the module (their `__name__`, unless
//!   something may rename them), and module or class constants bound
//!   unconditionally before use that an escape analysis of the whole module
//!   shows nothing can change (`CASES = [...]`, concatenations and `*`
//!   splats of such lists; a `del` in a class body falls through to the
//!   module's name; see `scope.rs` for what counts as a change). Dict
//!   argvalues keep one entry per key, as Python compares keys. Containers,
//!   and instances of well-known classes without a `__name__`
//!   (`Decimal(...)`, `Path(...)`, `np.array(...)`), make pytest fall back
//!   to `argname + index`, which is known statically too, but only for
//!   values aye knows are built: displays whose parts are known values,
//!   bound names or attributes of imported modules (`[*1]`, `{[1]: 2}` and
//!   `{**1}` raise), and calls whose arguments are of the types and values
//!   the callable accepts (`range(0, 3, 0)`, `Decimal("x")`, `Path(1)`,
//!   `frozenset([[1]])`, `dict([1])` raise; pandas' constructors are never
//!   read). A call to one of these with literal arguments aye can't confirm
//!   anywhere in the module's import-time code (`X = range(0, 3, 0)`)
//!   refines the file. Module attributes (`math.pi`) are not read: another
//!   module of the session may change them. Values that depend on a module
//!   the module or a support file in scope may change (`re.compile = …`,
//!   `setattr(builtins, "range", …)`, `vars(re)[…]`, `sys.modules["re"].x =
//!   …`, a module handed to code aye can't follow, a `global` rebinding of
//!   the name) are left to pytest. Left to refinement too: values whose id
//!   depends on the Python version (complex sums whose zero's sign changed
//!   in 3.14, digits `str.isdigit` changed on, builtins only some versions
//!   have), strings holding U+FFFD or lone surrogates, ints over 640
//!   digits, and `float()` or `re.compile()` calls that may raise. Known
//!   gaps: changes made by another test module of the session (pytest
//!   imports them all into one process), by installed plugins or
//!   `sitecustomize`, through a module held under another name by a module
//!   (`helpers.m.x = …` for `import math as m`), and module-level statements
//!   that raise for other reasons (a missing import, a failing call) are not
//!   seen statically: the run reports pytest's collection error. A value
//!   aye doesn't read under an explicit id (`pytest.param(make(), id="x")`)
//!   is taken as built.
//! - **Refinement versus `Partial`.** A file is refined when its ids, or
//!   which tests exist, may differ from the static reading (Core decision 5:
//!   when unsure, pytest decides): parametrize arguments that are not
//!   static, `ids=` callables, parametrized fixtures in the test's closure
//!   (through arguments, fixtures, `usefixtures` and autouse, with pytest's
//!   override rules), `pytest_generate_tests` in the module, the class or a
//!   conftest in scope, `pytest_make_parametrize_id` with parametrize, the
//!   collection hooks `pytest_pycollect_makeitem`,
//!   `pytest_pycollect_makemodule`, `pytest_collection`,
//!   `pytest_ignore_collect` and `pytest_collect_directory`,
//!   `pytest_collect_file` when it mentions Python files, a conftest in
//!   scope that loads plugins aye can't name (`pytest_plugins` it can't read,
//!   `pluginmanager.register(make())`), a test module's own
//!   `pytest_plugins`, classes whose bases aren't all known (whatever their
//!   name, since a unittest subclass inherits its tests; classes from
//!   packages that ship no test cases, such as the standard library's,
//!   don't count), class members added at runtime (class decorators aye
//!   can't see through, a metaclass other than `ABCMeta`, a base's
//!   `__init_subclass__`, `exec` or `locals()` in a class body, test names
//!   the module sets on the class), decorators aye can't see through on a
//!   test (they may parametrize, rename, replace or multiply it: ddt,
//!   `parameterized.expand`, registries), including the module's own unless
//!   they visibly return their argument or a `functools.wraps` wrapper of
//!   it, `pytestmark` entries it can't read and `test.pytestmark = …` (each
//!   may be a parametrize; `.append`, `.extend` and `+=` on a list are
//!   read), generator tests (pytest fails the module, which refinement
//!   reports once; a decorated one may be a plain function), imported or
//!   assigned names that may be tests (see "What may change which tests
//!   exist" below; a capitalized name from a project file
//!   that may be a unittest case, which pytest collects whatever its name,
//!   counts: a class whose bases may make it one, or a name aye can't tell
//!   is no class, followed through the project's re-exports), module
//!   namespaces reached in ways aye can't follow (`sys.modules[…]` for a
//!   computed name, frames, `f_globals`, `__globals__`, `gc`,
//!   `importlib.import_module` or `__import__` of a computed name, the
//!   module importing itself), and likewise a conftest in scope that does
//!   so, imports a module of the test module's name or implements
//!   `pytest_collectstart` or `pytest_make_collect_report` (they get the
//!   module before pytest collects it), code that changes `pytest`'s API or
//!   collection internals, a test file with no test aye can see (pytest
//!   imports it all the same: an import error is a collection error, and
//!   tests may exist only at runtime), star imports, test names defined in
//!   a branch that may not run (`if`, loops, `match`, `except`, a `try`
//!   body after a statement that may raise) or differently in alternative
//!   branches, a
//!   `del` that may not run, conditional `collect_ignore` entries,
//!   parametrization of package, session or unknown scope (pytest reorders
//!   the whole session by it), parametrize arguments pytest refuses (a
//!   `scope` other than its five names, an `indirect` that is neither a
//!   boolean nor a sequence of the argument names, `pytest.param` marks
//!   that are a set, which isn't hashable, or hold something that is surely
//!   no mark), `pytest.skip` or `pytest.importorskip` run while the
//!   module or a conftest in scope is imported, and whatever pytest reports
//!   as a collection error. The static guess keeps the unparametrized id
//!   with `Partial` certainty. Hooks that only mark or deselect
//!   (`pytest_collection_modifyitems`, unless its module makes items: see
//!   "Tests a `pytest_collection_modifyitems` adds" below;
//!   `pytest_itemcollected`), decorators of well-known libraries that keep
//!   the test (Hypothesis, `flaky`), unread `pytestmark` entries of
//!   unittest classes (parametrize doesn't apply there), fixtures imported
//!   from modules that aren't project files and `-p NAME` or
//!   `PYTEST_PLUGINS` plugins aye can't read (neither pytest's own nor
//!   project modules: they may mark, deselect or parametrize anything) keep
//!   their ids but make the case `Partial`.
//! - **Which support files count.** Hooks pytest calls per path count from
//!   the conftests in the file's directory and above, within pytest's
//!   confcutdir, the project files their `pytest_plugins` name or their
//!   `pluginmanager.register(module)` / `import_plugin("name")` load, and the
//!   project modules `-p NAME` loads (from addopts, `PYTEST_ADDOPTS` or the
//!   arguments after `--`) or `PYTEST_PLUGINS` names; with `--noconftest`,
//!   no conftest counts.
//!   Session-wide hooks (`pytest_collection`, `pytest_make_parametrize_id`,
//!   `pytest_collection_modifyitems`) count from any conftest of the
//!   project, found once by walking it, and from the plugins test modules
//!   load through `pytest_plugins`, since pytest applies them to every test
//!   once loaded. What a support module imports (`from m import name`, `from
//!   m import *`, honouring `__all__`) is followed into project files, for
//!   fixtures and hooks; a test module's imports are followed for the names
//!   its tests may request. Project files are found as Python finds them
//!   (see "Imports" below) from the importing directory's package base,
//!   the rootdir, the project root, their `src`, `pythonpath`, `PYTHONPATH`
//!   and what conftests put on `sys.path`. Installed plugins and modules
//!   are not seen. `-p
//!   no:unittest` makes `TestCase` classes plain classes (pytest's
//!   `class-has-init`), and `-p no:python` collects nothing: no `.py` file
//!   is claimed for tests, not even one on the command line, which is then
//!   `unclaimed`, as pytest's "found no collectors" (status 4), unless
//!   pytest's doctest plugin collects it.
//! - **A `conftest.py` is a test module too** ([`Claim::TestsAndSupport`])
//!   when it matches `python_files` (and isn't ignored) or is named on the
//!   command line, as pytest's `pytest_collect_file` doesn't spare it; it
//!   stays support when ignored. With `--noconftest` it is an ordinary file.
//! - **`pytest_plugins` in a non-top-level conftest fails its directory**,
//!   as `_check_non_top_pytest_plugins` makes pytest fail it: a conftest
//!   outside pytest's initial conftests (those of the collection arguments'
//!   directories, their `test*` subdirectories and above; see
//!   [`Recognizer::roots`]) that surely defines `pytest_plugins` (a plain
//!   top-level assignment, never deleted) gives the tests below it no cases
//!   and one `collection-error` diagnostic on its directory, with pytest's
//!   message, unless `--pyargs` is given. A definition aye isn't sure of is
//!   left alone, and so is a directory holding no test file: the
//!   front-end starts from the user's paths, so it fails such a directory
//!   there as pytest would in the user's run.
//! - **pytest's options.** addopts, `PYTEST_ADDOPTS` and the arguments
//!   after `--` ([`Project::framework_args`]) are read in pytest's order,
//!   the last one winning; the latter two first, as pytest reads them to
//!   find the config (their `-c`, `--rootdir` and `-o`, which may replace
//!   `addopts`). `-o`, `--strict`, `--ignore`, `--ignore-glob`,
//!   `--deselect`, `--confcutdir`, `--noconftest`,
//!   `--collect-in-virtualenv`, `--pyargs`, `--import-mode`, `-p`, `-k` and
//!   `-m` are read statically from all three. `--deselect` drops no case
//!   here: the context names its prefixes as prefixes of aye's ids
//!   (`deselect`), which the binary applies in selection, counting what
//!   they drop as deselected.
//!   pytest-xdist's options, `-x`, `--exitfirst`, `--maxfail`, the JUnit
//!   options (`--junitxml`/`--junit-xml`, `--junitprefix`/`--junit-prefix`)
//!   and pytest's doctest options (`--doctest-modules`, `--doctest-glob`,
//!   `--doctest-report`, `--doctest-continue-on-failure`,
//!   `--doctest-ignore-import-errors`) are taken out of all three: the
//!   runner runs the workers and counts `maxfail` across them, aye writes
//!   the JUnit report from the event stream, and the doctest adapter runs
//!   doctests; without the doctest adapter in the run, the doctest options
//!   stay (see "pytest's own doctests" below). The arguments after `--`
//!   also lose `-o addopts=…`, whose value is `addopts`. What is left, in
//!   pytest's order, is [`PytestConfig::args`], the options the front-end
//!   parses (the context's `native.args`); [`Recognizer::context`] carries
//!   what was lifted out (not the doctest options, which the doctest
//!   adapter reads itself).
//! - **The first affinity key is the module's import name** (`import:NAME`),
//!   as pytest computes it for the import mode in effect: the dotted name
//!   from the outermost package for `prepend` and `append` (the basename
//!   for a file outside packages), the rootdir-relative name for
//!   `importlib` outside packages, and the stem with
//!   `consider_namespace_packages`. The scheduler groups cases by their
//!   first key, so files pytest can't import in one session (its "import
//!   file mismatch") share a batch and the clash is reported as pytest
//!   reports it, whatever the worker count. The file and its classes follow.
//! - **Order** is pytest's: collection order, then `reorder_items` for
//!   class- and module-scoped parameters (direct ones numbered by call, as
//!   pytest renumbers them), which never reach across files.
//! - **Tags** are mark names in pytest's `iter_markers` order (the function's
//!   decorators, its parameter set's marks, then each class's, innermost
//!   first, then the module's), deduplicated. **Keywords** are the names
//!   pytest's `-k` matches: directories below the rootdir, the file name,
//!   class names, the test name with and without parameters, the names in
//!   the function's `__dict__` (`pytestmark` after mark decorators,
//!   `__wrapped__` and `patchings` from other decorators, attributes the
//!   module sets), and the tags. `timeout` comes from the closest
//!   `@pytest.mark.timeout`, as pytest-timeout reads it; no other mark maps
//!   onto case metadata.
//! - **What may change which tests exist** (Core decision 5: whatever aye
//!   can't prove is refined, and its guess is `Partial`, never `Exact`).
//!   Refined, besides the causes above:
//!   - a class whose tests may be added or removed: one handed to code aye
//!     can't follow (passed to a call, iterated over, held by a name whose
//!     attributes are set, `setattr`, `del K.attr`, a method called on it;
//!     `scope.rs`'s escape analysis, which doesn't count `super(K, …)`,
//!     reads, or the module's own `K.attr = …` statements), one holding a
//!     value when the module defines a `__set_name__`, one whose class or
//!     base the module sets a test name, `pytestmark`, `runTest` or a dunder
//!     on (`__bases__`, `__init__`; not `__doc__`, `__name__`,
//!     `__qualname__`, `__module__`); its cases are `Partial`. A non-test
//!     class with test methods handed on may get `__test__`;
//!   - a test function handed to code aye can't follow (`mark(test)`,
//!     `disable(test)`, `vars(test)[…] = …`, `TestK.test_a` handed on):
//!     its marks or `__test__` may change; its case is `Partial`;
//!   - a value bound by a call whatever its name, when the callee may make
//!     a class: `type(…)` with three arguments, `types.new_class`, and
//!     functions (or lambdas, or methods of the module's classes) of the
//!     module or of project files whose bodies hold a class statement,
//!     `type(…)`, `new_class` or `__test__`. Under a test function's name,
//!     every call but a builtin that returns data or a class of the module
//!     whose instances aren't callable (no `__new__`, `__call__`,
//!     metaclass or unknown base) may make a function (`functools.partial`,
//!     `update_wrapper`, a capitalized factory);
//!   - names imported from project files (or bound to them) whose
//!     `__test__` may be true (a class body setting it to anything but
//!     `False`, a project base that does, or the module's `name.__test__ =
//!     …`), and classes inheriting from them;
//!   - a non-test class whose bases from project files are no unittest case
//!     but whose other bases make it one or set `__test__` (pytest collects
//!     it); its guess, from the bases aye reads, is `Partial`;
//!   - names a function that runs at import binds or deletes through
//!     `global` (unless a non-test name is bound to a literal); their cases
//!     are `Partial`;
//!   - test names defined, and `del`s, in a `with` body after a statement
//!     that may raise (any context manager may swallow it, as
//!     `contextlib.suppress` does), as in a `try` body;
//!   - `from unittest import *` (it binds `FunctionTestCase`, which pytest
//!     collects);
//!   - module-level code that surely or may raise: a division of number
//!     literals by zero, a `raise` statement (`unittest.SkipTest` skips the
//!     module), an `assert` of a false literal.
//!
//!   `__test__` is read along the MRO from class bodies and from the
//!   module's own `K.__test__ = <literal>` statements, which win over the
//!   body's; a conditional or computed one is left to pytest. Private names
//!   in class bodies are mangled as Python stores them (`__x` in `K` is
//!   `_K__x`), which custom `python_functions`/`python_classes` globs may
//!   match. A reason that holds for the whole module (it may raise or skip
//!   itself at import, star imports, reflection, collection hooks, plugins
//!   aye can't read, a `collect_ignore` that may exclude it) makes every
//!   case of its guess `Partial`. Reasons name what implements a hook: "a
//!   conftest.py (in scope)", "a -p plugin", "a `PYTEST_PLUGINS` plugin",
//!   "a plugin a conftest.py loads". Known gaps: descriptors defined in
//!   other modules, factories from installed packages, classes taken out
//!   of containers (`A = CASES[0]`), and changes made by other modules.
//! - **Diagnostics**: `class-has-init` (pytest's collection warning),
//!   `duplicate-definition` (a test name bound twice in one namespace,
//!   outside exclusive branches and with no `del` between; the last
//!   definition is the one collected, at the position of the first, as
//!   Python's dicts keep it), and `collection-error` for a non-top-level
//!   `pytest_plugins`. A generator test is pytest's collection error too,
//!   but it is left to refinement, so it is reported once, as pytest
//!   reports it.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **A support module that changes collection's ini values refines the
//!   files below it.** pytest calls a conftest's `pytest_configure` when it
//!   registers it, before collecting the files below it, so what its code
//!   (any hook, or the module itself) does to `config` holds there:
//!   `addinivalue_line` of `python_functions`, `python_classes`,
//!   `usefixtures`, `collect_imported_tests`, `empty_parameter_set_mark`,
//!   `strict`, `strict_parametrization_ids`, the id-escaping option or
//!   `consider_namespace_packages` (or of a name aye can't read), a
//!   mutation of `config.getini(…)` of one of them, a write to the ini
//!   cache (`_inicache`, `inicfg`), or to `config.option`'s `ignore`,
//!   `ignore_glob`, `deselect`, `pyargs`, `importmode`, `noconftest`,
//!   `collect_in_virtualenv`, `confcutdir` (`setattr` included). Those
//!   files are refined and their static cases are `Partial`. A literal
//!   `python_files` pattern is read instead: files below the conftest that
//!   match it are test modules too (`claim`). Known gaps: a conftest that
//!   pytest loads earlier but that is not among the file's (a sibling
//!   directory's, collected first), a computed `python_files` pattern
//!   (which can't claim files) or one a plugin the conftest loads adds,
//!   `norecursedirs` additions, and a hook imported from another module.
//! - **Fixtures are read from every binding that may be the final one.**
//!   For each name of a module, class or support namespace: the final
//!   binding, the bindings in exclusive branches before it (`if`/`else`,
//!   `try`/`except`, `match`), and the previous one when the final one may
//!   not run (`if` without `else`, a loop, a `try` body after a statement
//!   that may raise). Merged, the fixture is parametrized if any of them
//!   is, requests what any requests, and is `conditional`: it doesn't hide
//!   the definitions further out (a conftest's parametrized one stays in
//!   the closure). Besides `@pytest.fixture` functions, the call forms
//!   `fixture(…)(function)` and `fixture(function, …)` are read (name,
//!   parameters, scope, autouse and what the function requests) and an
//!   alias of a fixture (`alias = some_fixture`) is that fixture under its
//!   name, as pytest's `parsefactories` registers it. A name bound in a way
//!   aye can't read that may make a fixture (a decorator or a call it can't
//!   see through, `reg = register(reg)`, `held = _Holder.held`, unpacking)
//!   is an opaque fixture, and a computed `name=` (or a support module that
//!   binds names at runtime: `globals()`, `vars()`, `exec`, `setattr` on
//!   `sys.modules[…]`) makes one under any name. A test whose closure
//!   reaches an opaque fixture is refined, as one reaching a parametrized
//!   one is. A call to a class of the module that makes its own instances
//!   (unconditional and undecorated, with no metaclass, no `__new__`,
//!   nothing that changes it afterwards, and only such classes or `object`
//!   as bases), to a builtin, or to a function of a package that holds no
//!   tests makes no fixture: pytest registers only objects whose type is
//!   exactly its `FixtureFunctionDefinition`. A capitalized callee is not
//!   taken for a class (`backend = Param("a", "b")` may return a
//!   parametrized fixture), as for test names above; this was decided at
//!   integration, where the two rounds' rules differed, for Core decision 5.
//! - **`sys.path` is read where it can be.** A support module's code that
//!   runs while pytest collects (at import, and in `pytest_*` hooks) may
//!   put directories on `sys.path` (`insert`, `append`, `extend`, `+=`,
//!   slice assignment, `site.addsitedir`). Those built from `__file__` and
//!   literals through `os.path` (`dirname`, `join`, `abspath`, `realpath`,
//!   `normpath`) and `pathlib` (`Path`, `.parent`, `.parents[n]`, `/`,
//!   `joinpath`, `resolve`, `absolute`), directly or through a module
//!   constant, are read, from every conftest of the project (found once by
//!   walking it), and their modules are project files, as `PYTHONPATH`'s
//!   are. Others (a relative path, a computed one, a new list) leave
//!   `sys.path` unknown: then a capitalized name imported from a module
//!   aye can't find may be a unittest case (pytest collects one whatever
//!   its name), and a fixture name no readable definition has (pytest's own
//!   aside) may come from a plugin a conftest in scope names but aye can't
//!   find: both refine. What a test module puts on `sys.path` itself counts
//!   for its own imports. Without such doubt, a module aye can't find is an
//!   installed package's, whose classes are unittest cases only when named
//!   like one (`…Test`, `…Tests`, `…TestCase`, `…TestCases`).
//! - **Imports.** A module is looked up as Python's `FileFinder` does: each
//!   part where the previous one says, a package (`name/__init__.py`)
//!   before a module (`name.py`), either before a namespace portion. The
//!   importing directory's package base comes first in the `prepend`
//!   import mode, after the other bases with `append` (where pytest
//!   appends it) and `importlib` (which adds none).
//! - **`--pyargs` testpaths are looked up on `sys.path`** as pytest's
//!   `search_pypath` does (`pythonpath` first, then the rootdir,
//!   `PYTHONPATH` and `src`): a module's file or a regular package's
//!   directory, else the entry as a path.
//! - **Helpers that collide across test directories.** With `prepend` or
//!   `append`, pytest puts each test module's package base on `sys.path`
//!   and Python keeps the first module it imports under a name: a test
//!   module that imports (absolutely) a module its own base holds and
//!   another test directory's base holds too (`helpers.py` in two
//!   rootdir-inserted directories) gets whichever pytest imported first.
//!   Such a module is refined (its static cases `Partial`), and its first
//!   affinity key is `import:NAME`, NAME being that module's name, so that
//!   the modules sharing it run in one session, as pytest runs them. The
//!   context's `colliding_modules` lists the names, for the front-end to
//!   give the files it lists the same key.
//! - **`PYTEST_PLUGINS`** names plugins of the whole session, imported
//!   after the `-p` ones and blocked by a `-p no:NAME`: project modules
//!   there count as `-p` ones do, others make every case `Partial`, and
//!   the context names them as `-p NAME` arguments (`env_plugins`), so the
//!   refinement cache's rule for plugins loaded by name covers them.
//! - **pytest's own doctests, without the doctest adapter** (finding F21).
//!   [`Pytest::prepare_among`] tells whether the run uses the doctest
//!   adapter; [`Pytest::prepare`] assumes it does. When it doesn't
//!   (`[tool.aye] adapters = ["pytest"]`), pytest's doctest plugin would
//!   collect doctests ([`PytestRecognizer::with_pytest_doctests`]), unless
//!   `-p no:doctest` blocks it: the front-end gets the doctest options as
//!   given, and the files the plugin collects (`_pytest/doctest.py`'s
//!   `pytest_collect_file`) are claimed as test files, which the pytest
//!   front-end refuses with a `usage-error` naming the fix (add the
//!   doctest adapter): aye's engine doesn't run pytest's own doctests
//!   ([`native`]), and a test is never silently dropped. With
//!   `--doctest-modules`, every `.py` file but `__main__.py` and a
//!   `setup.py` mentioning setuptools or distutils; text files matching
//!   `--doctest-glob` (pytest appends them across addopts,
//!   `PYTEST_ADDOPTS` and the arguments after `--`), else `test*.txt`,
//!   which pytest collects even without any doctest option, and a `.txt`
//!   or `.rst` file named on the command line; `--ignore` and the other
//!   ignore rules hold, but not for named files. Their doctests are
//!   refined, never guessed (pytest's parser, `doctest_encoding` and the
//!   module's import decide): a module only the doctest plugin collects
//!   (`m.py`, not matching `python_files`) gives no static case, a test
//!   module whose text holds `>>>`, `__test__` or `__doc__` is refined
//!   with its static cases `Partial` (others keep theirs: a docstring with
//!   examples holds a prompt), a text file is refined unless, read as
//!   UTF-8, it holds no `>>>` (then it has no test). `aye config` shows a
//!   `doctests` setting saying who would collect them.
//!   Known gap: doctests of a test module whose text holds none of those
//!   (a docstring built from escapes).
//! - **Tests a `pytest_collection_modifyitems` adds** (finding C16;
//!   `Function.from_parent(…)` in the hook) are listed, as `pytest --co`
//!   lists them: when a module that implements the hook (a conftest of
//!   the project, a plugin it or a test module loads, a `-p` or
//!   `PYTEST_PLUGINS` plugin) makes items (`from_parent` in the hook's
//!   body, or in a function or class of its module the hook names; in the
//!   module at all when the hook isn't a plain function), every test file
//!   is refined, its static cases `Partial`, since the hook gets the whole
//!   session's items. A hook that only marks, reorders or deselects keeps the
//!   ids, and so does one whose module makes collectors elsewhere.
//!   Known gaps: a hook that makes items through a function of another
//!   module, or adds copies (`copy.copy(item)`), and an added test's id
//!   named on the command line, which pytest can't find (its `not found`,
//!   exit 4), aye runs.
//!
//! Decided (C5, October 2026): the adapter's executor is aye's engine
//! with the pytest front-end ([`native`]). The executor that drove a real
//! pytest session per batch, and what only fed it (its filtered options as
//! separate vectors and a `PYTEST_ADDOPTS` string, its `-p no:` lists, the
//! context keys only it read: `addopts`, `pytest_addopts_env`,
//! `framework_args`, `doctests`, `adds_items`, `single_session`), are
//! gone. What it did that pytest users rely on is the front-end's and the
//! engine's now; their modules say how.

mod config;
mod context;
mod glob;
mod ids;
mod marks;
mod module;
pub mod native;
mod scope;
mod support;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::{
    AdapterError, Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested, Setting,
    SourceFile,
};
use aye_config::ValueSource;
use aye_events::{AffinityKey, Certainty, Diagnostic, DiagnosticCode, DiagnosticPath, Severity};
use aye_fs::normalize_absolute;
use aye_protocol::AdapterContext;
use aye_python_stdlib::fnmatch::{self, SetTranslation};
use aye_static::{EnvVars, ProcessEnv};

pub use config::{PytestConfig, split_xdist};
pub use native::{NAME, Pytest};

use crate::context::PytestContext;
use crate::ids::IdOptions;
use crate::marks::EmptyMark;
use crate::support::{Summary, Supports};

/// How pytest's patterns read `[...]` sets here (`python_files`,
/// `python_classes`, `python_functions`, `norecursedirs`, the ignore globs,
/// `--doctest-glob` and `testpaths`): as this adapter always has, which
/// differs from CPython for a set whose first item is a reversed range
/// followed by `!`.
pub(crate) const SETS: SetTranslation = SetTranslation::Legacy;

/// The pytest adapter's static face for one project.
#[derive(Debug)]
pub struct PytestRecognizer {
    root: PathBuf,
    config: PytestConfig,
    settings: module::Settings,
    supports: Supports,
    /// Project modules `-p NAME` loads (from addopts or `PYTEST_ADDOPTS`),
    /// then those `PYTEST_PLUGINS` names, in order: plugins of the whole
    /// session.
    preparse: Vec<Arc<Summary>>,
    /// What loads each of [`Self::preparse`], for messages: [`PLUGIN_P`]
    /// or [`PLUGIN_ENV`].
    preparse_labels: Vec<&'static str>,
    /// Conftests above the project root that pytest loads, outermost first.
    outer: Vec<Arc<Summary>>,
    /// What walking the project once finds (see [`PytestRecognizer::scan`]).
    scan: OnceLock<Scan>,
    /// `-p NAME` and `PYTEST_PLUGINS` plugins aye can't read: neither
    /// pytest's own nor project modules. They may mark, deselect or
    /// parametrize any test.
    unseen_plugins: Vec<String>,
    /// The paths pytest starts collecting from in the user's run
    /// ([`Recognizer::roots`]: the command-line paths, else `testpaths`,
    /// else the current directory), absolute.
    collection_args: Vec<PathBuf>,
    /// The directories whose conftests pytest loads before
    /// `pytest_configure` (its "initial conftests"): those of the collection
    /// arguments and their `test*` subdirectories. Each holds its ancestors
    /// too.
    initial_dirs: Vec<PathBuf>,
    /// Directories already reported for a non-top-level `pytest_plugins`.
    reported: Mutex<HashSet<PathBuf>>,
    /// `PYTHONPATH`'s directories, absolute: on `sys.path` in the workers.
    env_pythonpath: Vec<PathBuf>,
    /// What pytest's own doctest plugin collects, when it collects doctests
    /// in this run (no doctest adapter runs beside this one; see
    /// [`Self::with_pytest_doctests`]).
    doctests: Option<config::DoctestOptions>,
    /// The context, built once (see [`Self::pytest_context`]).
    context: OnceLock<PytestContext>,
}

/// What pytest's doctest plugin makes of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctestFile {
    /// A `DoctestModule` (`--doctest-modules`).
    Module,
    /// A `DoctestTextfile` (`--doctest-glob`, `test*.txt` by default).
    Text,
}

/// What walking the project once finds ([`PytestRecognizer::scan`]).
#[derive(Debug, Default)]
struct Scan {
    /// [`support::GLOBAL_HOOKS`] implemented by any conftest of the project,
    /// each with what implements it first (for messages).
    hooks: BTreeMap<String, String>,
    /// The summaries of test modules that load plugins (kept, since
    /// [`Supports`] caches by address).
    modules: Vec<Arc<Summary>>,
    /// What the project's conftests put on `sys.path`.
    sys_path: support::SysPath,
    /// Module names more than one test directory's `sys.path` entry holds
    /// (see [`PytestRecognizer::colliding`]).
    colliding: BTreeSet<String>,
    /// What implements a `pytest_collection_modifyitems` that may add
    /// tests (its module makes items: [`support::Summary::makes_items`]),
    /// the first found, for messages.
    adds_items: Option<String>,
}

/// What the pytest adapter reads from the environment, which the workers
/// inherit.
#[derive(Debug, Clone, Copy, Default)]
pub struct PytestEnv<'e> {
    /// `PYTEST_ADDOPTS`.
    pub pytest_addopts: Option<&'e str>,
    /// `PYTEST_PLUGINS`.
    pub pytest_plugins: Option<&'e str>,
    /// `PYTHONPATH`.
    pub pythonpath: Option<&'e str>,
}

impl<'e> PytestEnv<'e> {
    /// The variables, as `env` holds them.
    pub fn of(env: &'e ProcessEnv) -> Self {
        Self {
            pytest_addopts: env.get(EnvVars::PYTEST_ADDOPTS),
            pytest_plugins: env.get(EnvVars::PYTEST_PLUGINS),
            pythonpath: env.get(EnvVars::PYTHONPATH),
        }
    }
}

/// pytest's `builtin_plugins`, and the plugins merged into pytest that it
/// refuses to register (`DEPRECATED_EXTERNAL_PLUGINS`): `-p NAME` for these
/// loads nothing aye doesn't model.
const BUILTIN_PLUGINS: [&str; 37] = [
    "mark",
    "main",
    "runner",
    "fixtures",
    "helpconfig",
    "python",
    "terminal",
    "debugging",
    "unittest",
    "capture",
    "skipping",
    "legacypath",
    "tmpdir",
    "monkeypatch",
    "recwarn",
    "pastebin",
    "assertion",
    "junitxml",
    "doctest",
    "cacheprovider",
    "setuponly",
    "setupplan",
    "stepwise",
    "unraisableexception",
    "threadexception",
    "warnings",
    "logging",
    "reports",
    "faulthandler",
    "subtests",
    "pytester",
    "pytester_assertions",
    "terminalprogress",
    "pytest_catchlog",
    "pytest_capturelog",
    "pytest_faulthandler",
    "pytest_subtests",
];

/// Whether ignoring a path is certain, possible or ruled out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ignored {
    No,
    Maybe,
    Yes,
}

/// pytest's `_in_venv`.
fn is_virtualenv(dir: &Path) -> bool {
    dir.join("pyvenv.cfg").is_file() || dir.join("conda-meta").join("history").is_file()
}

/// Where code imports project modules from: the rootdir (on `sys.path`
/// under `python -m pytest`), the project root, their `src`, and
/// `pythonpath`.
fn plugin_bases(config: &PytestConfig, root: &Path) -> Vec<PathBuf> {
    let mut bases = vec![config.rootdir.clone(), root.to_path_buf()];
    bases.push(config.rootdir.join("src"));
    bases.push(root.join("src"));
    bases.extend(config.pythonpath.iter().cloned());
    let mut seen = Vec::new();
    bases.retain(|b| {
        let new = !seen.contains(b);
        seen.push(b.clone());
        new
    });
    bases
}

/// pytest's `resolve_package_path`: the last directory upward from `file`
/// that is still a package with an identifier for a name.
fn package_path(file: &Path) -> Option<PathBuf> {
    let mut found = None;
    for dir in file.ancestors().skip(1) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        let identifier = name
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_alphanumeric() || c == '_');
        if !dir.join("__init__.py").is_file() || !identifier {
            break;
        }
        found = Some(dir.to_path_buf());
    }
    found
}

/// The directories pytest loads conftests for before `pytest_configure`
/// (`_set_initial_conftests`): each existing collection argument's (its
/// own, for a directory, and its `test*` subdirectories; its parent, for a
/// file), or else the current directory's. A conftest in one of them or
/// above is "initial".
fn initial_dirs(cwd: &Path, args: &[PathBuf]) -> Vec<PathBuf> {
    let test_dirs = |dir: &Path| -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                // `Path.glob("test*")`: case-insensitive on Windows only.
                if cfg!(windows) {
                    name.to_lowercase().starts_with("test")
                } else {
                    name.starts_with("test")
                }
            })
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()
    };
    let mut anchors = Vec::new();
    for arg in args {
        let arg = normalize_absolute(arg);
        if arg.is_dir() {
            anchors.extend(test_dirs(&arg));
            anchors.push(arg);
        } else if arg.exists()
            && let Some(parent) = arg.parent()
        {
            anchors.push(parent.to_path_buf());
        }
    }
    if anchors.is_empty() {
        anchors.push(cwd.to_path_buf());
        anchors.extend(test_dirs(cwd));
    }
    anchors
}

/// pytest's `compute_module_name`: `file`'s dotted name from `root`.
fn module_name(root: &Path, file: &Path) -> Option<String> {
    let rel = file.with_extension("");
    let rel = rel.strip_prefix(root).ok()?;
    let mut names: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if names.last().is_some_and(|n| n == "__init__") {
        names.pop();
    }
    (!names.is_empty()).then(|| names.join("."))
}

impl PytestRecognizer {
    /// The recognizer for `project`, with the variables its environment
    /// holds.
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        Self::with_environment(project, PytestEnv::of(project.env))
    }

    /// [`Self::new`] with `PYTEST_ADDOPTS` given rather than taken from the
    /// project's environment, and neither `PYTEST_PLUGINS` nor `PYTHONPATH`.
    pub fn with_env(
        project: &Project<'_>,
        pytest_addopts: Option<&str>,
    ) -> Result<Self, AdapterError> {
        Self::with_environment(
            project,
            PytestEnv {
                pytest_addopts,
                ..PytestEnv::default()
            },
        )
    }

    /// [`Self::new`] with the variables given rather than taken from the
    /// project's environment.
    pub fn with_environment(
        project: &Project<'_>,
        env: PytestEnv<'_>,
    ) -> Result<Self, AdapterError> {
        let config = PytestConfig::discover_with(project, env.pytest_addopts)?
            .with_env_plugins(env.pytest_plugins);
        let env_pythonpath: Vec<PathBuf> = env
            .pythonpath
            .map(|value| {
                std::env::split_paths(value)
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| normalize_absolute(&project.cwd.join(p)))
                    .collect()
            })
            .unwrap_or_default();
        let blocked = |name: &str| {
            config
                .plugins
                .iter()
                .any(|p| p.strip_prefix("no:").is_some_and(|n| n.trim() == name))
        };
        let settings = module::Settings {
            python_classes: config.python_classes.value.clone(),
            python_functions: config.python_functions.value.clone(),
            ids: IdOptions {
                no_escaping: config.disable_test_id_escaping.value,
                strict: config.strict_parametrization_ids.value,
            },
            empty: EmptyMark::parse(&config.empty_parameter_set_mark.value),
            collect_imported_tests: config.collect_imported_tests.value,
            usefixtures: config.usefixtures.value.clone(),
            unittest_blocked: blocked("unittest"),
            python_blocked: blocked("python"),
        };
        let mut supports = Supports::default();
        supports.bases_first = config.import_mode != config::ImportMode::Prepend;
        let mut bases = plugin_bases(&config, project.root);
        bases.extend(env_pythonpath.iter().cloned());
        // `-p` plugins, then `PYTEST_PLUGINS`' (which no `-p no:` blocks).
        let session_plugins: Vec<&String> =
            config.plugins.iter().chain(&config.env_plugins).collect();
        let (preparse, preparse_labels): (Vec<Arc<Summary>>, Vec<&'static str>) = session_plugins
            .iter()
            .enumerate()
            .filter(|(_, name)| !name.starts_with("no:"))
            .filter_map(|(at, name)| {
                let label = if at < config.plugins.len() {
                    PLUGIN_P
                } else {
                    PLUGIN_ENV
                };
                Some((supports.module(name, &config.rootdir, &bases)?, label))
            })
            .unzip();
        let unseen_plugins = session_plugins
            .iter()
            .enumerate()
            .filter(|(at, name)| {
                // A later `-p no:NAME` blocks it again.
                !name.starts_with("no:")
                    && !BUILTIN_PLUGINS.contains(&name.as_str())
                    && !session_plugins[at + 1..]
                        .iter()
                        .any(|later| later.strip_prefix("no:") == Some(name.as_str()))
                    && supports.module(name, &config.rootdir, &bases).is_none()
            })
            .map(|(_, name)| (*name).clone())
            .collect();
        let mut outer: Vec<Arc<Summary>> = if config.noconftest {
            Vec::new()
        } else {
            project
                .root
                .ancestors()
                .skip(1)
                .filter(|dir| in_confcutdir(&config.confcutdir, dir))
                .filter_map(|dir| supports.conftest_in(dir))
                .collect()
        };
        outer.reverse();
        let mut recognizer = Self {
            root: project.root.to_path_buf(),
            config,
            settings,
            supports,
            preparse,
            preparse_labels,
            outer,
            scan: OnceLock::new(),
            unseen_plugins,
            collection_args: Vec::new(),
            initial_dirs: Vec::new(),
            reported: Mutex::new(HashSet::new()),
            env_pythonpath,
            doctests: None,
            context: OnceLock::new(),
        };
        recognizer.collection_args = recognizer.roots(project);
        recognizer.initial_dirs = initial_dirs(project.cwd, &recognizer.collection_args);
        Ok(recognizer)
    }

    /// [`support::GLOBAL_HOOKS`] implemented by any conftest pytest may
    /// load in this project (and the plugins they load, or test modules
    /// load through `pytest_plugins`, which hook every module collected
    /// after them), each with what implements it first, for messages.
    fn global_hooks(&self) -> &BTreeMap<String, String> {
        &self.scan().hooks
    }

    /// What pytest's session can load, walked once the way collection walks
    /// it: the session-wide hooks of its conftests (see
    /// [`Self::global_hooks`]), what they put on `sys.path` (see
    /// [`Self::sys_path`]) and the module names its test directories share
    /// (see [`Self::colliding`]). That is the directories the run collects
    /// (the paths given, else `testpaths` or the current directory, as
    /// pytest's arguments), each with the conftests above it up to the root:
    /// pytest loads no other conftest, so a run of one directory doesn't
    /// pay for the whole project. The walk claims files, so claiming must
    /// not need it: it looks no module up.
    fn scan(&self) -> &Scan {
        self.scan.get_or_init(|| {
            let mut scan = Scan::default();
            let mut conftests: Vec<Arc<Summary>> = Vec::new();
            let mut test_bases: BTreeSet<PathBuf> = BTreeSet::new();
            let (mut stack, files) = self.scan_starts();
            for dir in self.scan_above(&stack, &files) {
                if let Some(summary) = self.conftest_in(&dir) {
                    if in_confcutdir(&self.config.confcutdir, &dir) {
                        add_sys_path(&mut scan.sys_path, &summary);
                    }
                    conftests.push(summary);
                }
            }
            for file in &files {
                if let (Some(dir), Ok(rel)) = (file.parent(), file.strip_prefix(&self.root)) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.scan_file(&mut scan, &mut test_bases, &mut None, dir, file, &rel);
                }
            }
            while let Some(dir) = stack.pop() {
                if let Some(summary) = self.conftest_in(&dir) {
                    if in_confcutdir(&self.config.confcutdir, &dir) {
                        add_sys_path(&mut scan.sys_path, &summary);
                    }
                    conftests.push(summary);
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut base: Option<PathBuf> = None;
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                        continue;
                    }
                    let Ok(rel) = path.strip_prefix(&self.root) else {
                        continue;
                    };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.scan_file(&mut scan, &mut test_bases, &mut base, &dir, &path, &rel);
                }
            }
            for summary in self.preparse.iter().chain(&self.outer) {
                add_sys_path(&mut scan.sys_path, summary);
            }
            // Modules looked up so far (`-p` plugins) are looked up again,
            // where the conftests' directories count too.
            self.supports.forget_resolutions();
            let mut bases = self.basic_bases();
            for dir in &scan.sys_path.added {
                if !bases.contains(dir) {
                    bases.push(dir.clone());
                }
            }
            // Each with what implements its hooks: the summary itself
            // (`own`), or a plugin it loads (`loaded`).
            let labelled = self
                .preparse
                .iter()
                .zip(&self.preparse_labels)
                .map(|(s, label)| (s, *label, *label))
                .chain(
                    self.outer
                        .iter()
                        .chain(&conftests)
                        .map(|s| (s, CONFTEST, PLUGIN_OF_CONFTEST)),
                )
                .chain(
                    scan.modules
                        .iter()
                        .map(|s| (s, PLUGIN_OF_MODULE, PLUGIN_OF_MODULE)),
                );
            for (summary, own, loaded) in labelled {
                for (i, expanded) in self.supports.expand(summary, &bases).iter().enumerate() {
                    let exports = self.supports.exports(expanded, &bases);
                    let source = if i == 0 { own } else { loaded };
                    for hook in support::GLOBAL_HOOKS {
                        if exports.hooks.contains(hook) {
                            scan.hooks
                                .entry(hook.to_string())
                                .or_insert_with(|| source.to_string());
                        }
                    }
                    if exports.makes_items
                        && exports.hooks.contains(MODIFYITEMS)
                        && scan.adds_items.is_none()
                    {
                        scan.adds_items = Some(source.to_string());
                    }
                }
            }
            scan.colliding = self.colliding_names(&test_bases);
            scan
        })
    }

    /// Where [`Self::scan`] walks: the directories the run collects, none
    /// inside another, and the files it names outside them. The project
    /// root when it names nothing that exists.
    fn scan_starts(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for arg in &self.collection_args {
            let arg = normalize_absolute(arg);
            if arg.is_dir() {
                dirs.push(arg);
            } else if arg.is_file() {
                files.push(arg);
            }
        }
        if dirs.is_empty() && files.is_empty() {
            dirs.push(self.root.clone());
        }
        dirs.sort();
        let mut kept: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if !kept.iter().any(|outer| dir.starts_with(outer)) {
                kept.push(dir);
            }
        }
        files.retain(|file| !kept.iter().any(|dir| file.starts_with(dir)));
        files.dedup();
        (kept, files)
    }

    /// The directories above where [`Self::scan`] walks, up to the root,
    /// outermost first: their conftests load before anything collected
    /// below them. A file's own directory counts, since it isn't walked.
    fn scan_above(&self, dirs: &[PathBuf], files: &[PathBuf]) -> Vec<PathBuf> {
        let mut above: Vec<PathBuf> = Vec::new();
        let starts = dirs
            .iter()
            .filter_map(|dir| dir.parent())
            .chain(files.iter().filter_map(|file| file.parent()));
        for start in starts {
            for dir in start.ancestors() {
                if !dir.starts_with(&self.root) {
                    break;
                }
                if !above.iter().any(|seen| seen == dir) {
                    above.push(dir.to_path_buf());
                }
            }
        }
        above.sort_by_key(|dir| dir.components().count());
        above
    }

    /// One file met by [`Self::scan`] in `dir`: a test or doctest module
    /// marks its package base (`base`, once per directory), and a test
    /// module's `pytest_plugins` joins the plugins the session loads.
    fn scan_file(
        &self,
        scan: &mut Scan,
        test_bases: &mut BTreeSet<PathBuf>,
        base: &mut Option<PathBuf>,
        dir: &Path,
        path: &Path,
        rel: &str,
    ) {
        // pytest imports test modules, and doctest modules with
        // `--doctest-modules` (text files it only reads).
        let python = self.python_module(rel, false);
        let doctest = self.doctest_file(rel, false) == Some(DoctestFile::Module);
        if !python && !doctest {
            return;
        }
        if base.is_none() {
            let found = self.supports.package_base(dir);
            test_bases.insert(found.clone());
            *base = Some(found);
        }
        if !python {
            // A doctest module's `pytest_plugins` loads nothing.
            return;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            return;
        };
        if !source.contains("pytest_plugins") {
            return;
        }
        let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
            return;
        };
        let module = Arc::new(Summary::build(dir, &source, parsed.syntax()));
        if !module.plugins.is_empty() {
            scan.modules.push(module);
        }
    }

    /// The resolved pytest configuration.
    pub fn config(&self) -> &PytestConfig {
        &self.config
    }

    /// This recognizer for a run without the doctest adapter: pytest's own
    /// doctest plugin would collect doctests, as without aye. Its options
    /// reach the front-end ([`PytestConfig::keep_doctest_options`]), and
    /// the files it collects are claimed (and refused: see
    /// [`native::NativeRecognizer`]): with `--doctest-modules`
    /// every `.py` file but `__main__.py` and a `setup.py` mentioning
    /// setuptools or distutils, and the text files matching
    /// `--doctest-glob` (`test*.txt` without one) or named on the command
    /// line as `.txt` or `.rst`, unless ignored. Unless `-p no:doctest`
    /// blocks the plugin, which then collects nothing.
    #[must_use]
    pub fn with_pytest_doctests(mut self) -> Self {
        self.config = self.config.keep_doctest_options();
        let blocked = self
            .config
            .plugins
            .iter()
            .any(|p| p.strip_prefix("no:").is_some_and(|n| n.trim() == "doctest"));
        if !blocked {
            self.doctests = Some(self.config.doctest.clone());
        }
        self
    }

    /// pytest's `pytest_collect_file` for Python modules: a `.py` file
    /// named on the command line, or matching `python_files` (or a pattern
    /// a conftest adds) and not ignored, is a test module. With `-p
    /// no:python` none is: pytest's python plugin collects them.
    fn python_module(&self, rel: &str, explicit: bool) -> bool {
        if self.settings.python_blocked {
            return false;
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        // pathlib's `suffix`: a dotfile like `.py` has none.
        if Path::new(name).extension().is_none_or(|e| e != "py") {
            return false;
        }
        if explicit {
            return true;
        }
        let abs = self.root.join(rel);
        self.ignored(&abs) != Ignored::Yes
            && (self
                .config
                .python_files
                .value
                .iter()
                .any(|pattern| fnmatch::fnmatch_ex(pattern, &abs, SETS))
                || self
                    .added_python_files(abs.parent().unwrap_or(&abs))
                    .iter()
                    .any(|pattern| fnmatch::fnmatch_ex(pattern, &abs, SETS)))
    }

    /// What pytest's doctest plugin collects from `rel`, when it collects
    /// doctests in this run (`_pytest/doctest.py`'s `pytest_collect_file`).
    fn doctest_file(&self, rel: &str, explicit: bool) -> Option<DoctestFile> {
        let doctests = self.doctests.as_ref()?;
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let abs = self.root.join(rel);
        let extension = Path::new(name).extension().and_then(|e| e.to_str());
        // pytest never asks its ignore rules about a path on its command
        // line.
        let ignored = || !explicit && self.ignored(&abs) == Ignored::Yes;
        if extension == Some("py") {
            let setup_py = || {
                name == "setup.py"
                    && std::fs::read(&abs).is_ok_and(|bytes| {
                        [&b"setuptools"[..], b"distutils"]
                            .iter()
                            .any(|word| bytes.windows(word.len()).any(|w| w == *word))
                    })
            };
            if !doctests.modules || name == "__main__.py" || setup_py() || ignored() {
                return None;
            }
            return Some(DoctestFile::Module);
        }
        if explicit && matches!(extension, Some("txt" | "rst")) {
            return Some(DoctestFile::Text);
        }
        let default = ["test*.txt".to_string()];
        let globs = if doctests.globs.is_empty() {
            &default[..]
        } else {
            &doctests.globs[..]
        };
        if globs
            .iter()
            .any(|glob| fnmatch::fnmatch_ex(glob, &abs, SETS))
            && !ignored()
        {
            return Some(DoctestFile::Text);
        }
        None
    }

    /// The conftest pytest loads from `dir`, if any (none with
    /// `--noconftest`).
    fn conftest_in(&self, dir: &Path) -> Option<Arc<Summary>> {
        if self.config.noconftest {
            return None;
        }
        self.supports.conftest_in(dir)
    }

    /// The directory pytest fails to collect because a conftest in it, not
    /// loaded before `pytest_configure`, defines `pytest_plugins`
    /// (`_check_non_top_pytest_plugins`), and pytest's message. Only
    /// certain definitions count: a guess would drop tests pytest runs.
    fn non_top_level_plugins(
        &self,
        conftests: &[(&Path, Arc<Summary>)],
    ) -> Option<(PathBuf, String)> {
        if self.config.pyargs {
            return None;
        }
        let (dir, _) = conftests.iter().find(|(dir, summary)| {
            summary.defines_plugins && !self.initial_dirs.iter().any(|d| d.starts_with(dir))
        })?;
        let message = format!(
            "Defining 'pytest_plugins' in a non-top-level conftest is no longer supported:\n\
             It affects the entire test suite instead of just below the conftest as expected.\n  \
             {}\n\
             Please move it to a top level conftest file at the rootdir:\n  \
             {}\n\
             For more information, visit:\n  \
             https://docs.pytest.org/en/stable/deprecations.html#pytest-plugins-in-non-top-level-conftest-files",
            normalize_absolute(&dir.join("conftest.py")).display(),
            normalize_absolute(&self.config.confcutdir).display(),
        );
        Some((dir.to_path_buf(), message))
    }

    /// [`plugin_bases`] and `PYTHONPATH`.
    fn basic_bases(&self) -> Vec<PathBuf> {
        let mut bases = plugin_bases(&self.config, &self.root);
        for dir in &self.env_pythonpath {
            if !bases.contains(dir) {
                bases.push(dir.clone());
            }
        }
        bases
    }

    /// Where project modules are imported from: [`plugin_bases`],
    /// `PYTHONPATH`, then what the project's conftests put on `sys.path`.
    fn plugin_bases(&self) -> Vec<PathBuf> {
        let mut bases = self.basic_bases();
        for dir in &self.sys_path().added {
            if !bases.contains(dir) {
                bases.push(dir.clone());
            }
        }
        bases
    }

    /// What the conftests pytest may load in this project (and those above
    /// it, and the `-p` plugins) put on `sys.path`: pytest keeps them there
    /// for the rest of the session, so their modules are project files
    /// too, wherever they are imported from.
    fn sys_path(&self) -> &support::SysPath {
        &self.scan().sys_path
    }

    /// Module names that more than one directory pytest puts on
    /// `sys.path` for test modules holds. With the `prepend` and `append`
    /// import modes, pytest puts each test module's package base (its
    /// directory, outside packages) on `sys.path`, and Python keeps the
    /// first module it imports under a name for the rest of the session: a
    /// test module importing such a name gets whichever of them pytest
    /// imported first, which depends on the other files of the session.
    /// Empty with `importlib`, which puts no test directory on `sys.path`,
    /// and with `consider_namespace_packages`.
    fn colliding(&self) -> &BTreeSet<String> {
        &self.scan().colliding
    }

    /// [`Self::colliding`], from the package bases of the test modules.
    fn colliding_names(&self, bases: &BTreeSet<PathBuf>) -> BTreeSet<String> {
        if self.config.import_mode == config::ImportMode::Importlib
            || self.config.consider_namespace_packages
            || bases.len() < 2
        {
            return BTreeSet::new();
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        for base in bases {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let name = if path.extension().is_some_and(|e| e == "py") {
                    path.file_stem().map(|n| n.to_string_lossy().into_owned())
                } else if path.join("__init__.py").is_file() {
                    path.file_name().map(|n| n.to_string_lossy().into_owned())
                } else {
                    None
                };
                if let Some(name) = name
                    && name != "conftest"
                    && name != "__init__"
                {
                    *seen.entry(name).or_default() += 1;
                }
            }
        }
        seen.into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, _)| name)
            .collect()
    }

    /// The first module name the test module at `file` imports (anywhere
    /// in it, absolutely) that its own `sys.path` entry holds and another
    /// test directory's holds too (see [`Self::colliding`]).
    fn colliding_import(&self, file: &SourceFile<'_>) -> Option<String> {
        let colliding = self.colliding();
        if colliding.is_empty() {
            return None;
        }
        let base = self.supports.package_base(file.abs.parent()?);
        // In order: the first name is the key.
        support::absolute_imports(file.module)
            .into_iter()
            .filter(|name| colliding.contains(name))
            .find(|name| {
                base.join(format!("{name}.py")).is_file()
                    || base.join(name).join("__init__.py").is_file()
            })
    }

    /// Where pytest's `search_pypath` finds the dotted name `name` with
    /// `--pyargs`: a module's file, or a regular package's directory,
    /// looked up on `sys.path` as pytest has it then: `pythonpath` (put
    /// first), the rootdir (the working directory under `python -m
    /// pytest`), `PYTHONPATH`, then the project's `src` directories (an
    /// editable install's). A namespace package is no match (pytest then
    /// takes the name as a path).
    fn pyarg(&self, name: &str) -> Option<PathBuf> {
        let valid = name.split('.').all(|part| {
            part.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
                && part.chars().all(|c| c.is_alphanumeric() || c == '_')
        });
        if !valid {
            return None;
        }
        let mut search: Vec<PathBuf> = self.config.pythonpath.clone();
        search.push(self.config.rootdir.clone());
        search.extend(self.env_pythonpath.iter().cloned());
        search.push(self.config.rootdir.join("src"));
        search.push(self.root.join("src"));
        let found = support::find_module(name, &search)?;
        let dir = found.parent().map(Path::to_path_buf);
        Some(if found.file_name().is_some_and(|f| f == "__init__.py") {
            dir.unwrap_or(found)
        } else {
            found
        })
    }

    /// The support modules pytest has registered, and so called
    /// `pytest_configure` of, by the time it collects the files in `dir`:
    /// the session's plugins (`-p`, `PYTEST_PLUGINS`), the conftests above
    /// the project root and those of `dir` and its ancestors within the
    /// confcutdir. Not the plugins they load: claiming runs while the
    /// project is walked (see [`Self::scan`]), before modules can be looked
    /// up.
    fn configuring(&self, dir: &Path) -> Vec<Arc<Summary>> {
        let mut local: Vec<Arc<Summary>> = dir
            .ancestors()
            .take_while(|d| d.starts_with(&self.root))
            .filter(|d| in_confcutdir(&self.config.confcutdir, d))
            .filter_map(|d| self.conftest_in(d))
            .collect();
        local.reverse();
        self.preparse
            .iter()
            .chain(&self.outer)
            .chain(&local)
            .cloned()
            .collect()
    }

    /// `python_files` patterns the support modules pytest has registered
    /// before collecting the files in `dir` add to the ini value
    /// (`config.addinivalue_line("python_files", …)` in a
    /// `pytest_configure`). A conftest outside `dir`'s ancestors that
    /// pytest happens to load earlier is not seen.
    fn added_python_files(&self, dir: &Path) -> Vec<String> {
        // Most projects add none: look without collecting first.
        let adds = |s: &Summary| !s.ini.python_files.is_empty();
        let any = self.preparse.iter().chain(&self.outer).any(|s| adds(s))
            || dir
                .ancestors()
                .take_while(|d| d.starts_with(&self.root))
                .filter_map(|d| self.conftest_in(d))
                .any(|s| adds(&s));
        if !any {
            return Vec::new();
        }
        self.configuring(dir)
            .iter()
            .flat_map(|s| s.ini.python_files.iter().cloned())
            .collect()
    }

    /// The name pytest imports a test file under, for the import mode in
    /// effect, as the first affinity key of its cases: `import:NAME`.
    /// Files that share it can't be imported in one session (pytest's
    /// "import file mismatch"), so the scheduler, which groups cases by
    /// their first key, keeps them in one batch and pytest reports the
    /// clash as it would on its own, whatever the worker count. With
    /// `consider_namespace_packages` the name depends on `sys.path` at
    /// runtime; the file's stem stands in, which groups every file that
    /// could clash (and some that can't).
    fn import_key(&self, file: &Path) -> AffinityKey {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.config.consider_namespace_packages {
            return AffinityKey::import(&stem);
        }
        let packaged = package_path(file)
            .and_then(|package| module_name(package.parent().unwrap_or(&package), file));
        let name = match (packaged, self.config.import_mode) {
            (Some(name), _) => name,
            (None, config::ImportMode::Importlib) => {
                // `module_name_from_path`: unique per file.
                let rel = file.with_extension("");
                let parts: Vec<String> = match rel.strip_prefix(&self.config.rootdir) {
                    Ok(rel) => rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                    Err(_) => rel
                        .components()
                        .skip(1)
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                };
                let mut parts = parts;
                if parts.len() >= 2 && parts.last().is_some_and(|p| p == "__init__") {
                    parts.pop();
                }
                parts
                    .iter()
                    .map(|p| p.replace('.', "_"))
                    .collect::<Vec<_>>()
                    .join(".")
            }
            (None, _) => stem,
        };
        AffinityKey::import(&name)
    }

    /// Whether pytest ignores `path` (`collect_ignore`, `--ignore` and
    /// their glob forms).
    fn ignored(&self, path: &Path) -> Ignored {
        let path = normalize_absolute(path);
        if self
            .config
            .ignore
            .iter()
            .any(|p| normalize_absolute(p) == path)
        {
            return Ignored::Yes;
        }
        let text = path.to_string_lossy();
        if self
            .config
            .ignore_glob
            .iter()
            .any(|glob| fnmatch::fnmatch(&text, glob, SETS))
        {
            return Ignored::Yes;
        }
        let Some(parent) = path.parent() else {
            return Ignored::No;
        };
        let nearest = |pick: fn(&Summary) -> Option<&support::IgnoreList>| {
            parent
                .ancestors()
                .filter(|dir| in_confcutdir(&self.config.confcutdir, dir))
                .filter_map(|dir| self.conftest_in(dir))
                .find(|summary| pick(summary).is_some())
        };
        let mut verdict = Ignored::No;
        if let Some(summary) = nearest(|s| s.collect_ignore.as_ref()) {
            let list = summary.collect_ignore.as_ref().expect("picked");
            for (entry, conditional) in &list.entries {
                if normalize_absolute(&summary.dir.join(entry)) == path {
                    if !conditional {
                        return Ignored::Yes;
                    }
                    verdict = Ignored::Maybe;
                }
            }
            if list.unknown {
                verdict = Ignored::Maybe;
            }
        }
        if let Some(summary) = nearest(|s| s.collect_ignore_glob.as_ref()) {
            let list = summary.collect_ignore_glob.as_ref().expect("picked");
            for (entry, conditional) in &list.entries {
                let glob = normalize_absolute(&summary.dir.join(entry));
                if fnmatch::fnmatch(&text, &glob.to_string_lossy(), SETS) {
                    if !conditional {
                        return Ignored::Yes;
                    }
                    verdict = Ignored::Maybe;
                }
            }
            if list.unknown {
                verdict = Ignored::Maybe;
            }
        }
        verdict
    }

    /// The context ([`Recognizer::context`]), built the first time it is
    /// asked for, which walks the project once for
    /// [`PytestContext::colliding_modules`].
    pub(crate) fn pytest_context(&self) -> &PytestContext {
        self.context.get_or_init(|| PytestContext::new(self))
    }

    /// `--deselect`'s prefixes as prefixes of aye's ids: relative to the
    /// project root, when the rootdir is inside it.
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

    /// The names `-k` matches for a file's position: the directories
    /// between the rootdir and the file, and the file name.
    fn path_keywords(&self, file: &SourceFile<'_>) -> Vec<String> {
        match file.abs.strip_prefix(&self.config.rootdir) {
            Ok(rel) => rel
                .components()
                .filter_map(|c| match c {
                    Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect(),
            Err(_) => file.rel.as_str().split('/').map(str::to_string).collect(),
        }
    }
}

/// The session-wide hook that may add tests (finding C16).
const MODIFYITEMS: &str = "pytest_collection_modifyitems";

/// Why pytest's doctest plugin collects doctests in a run, for messages.
const PYTEST_DOCTESTS: &str = "the doctest adapter isn't among the run's adapters";

/// A text file pytest's doctest plugin collects: one test when it holds
/// examples, which only pytest's parser (with `doctest_encoding`) can tell
/// for sure; none when, read as UTF-8, it holds no `>>>` prompt.
fn doctest_text(file: &SourceFile<'_>) -> Recognition {
    if !file.source.contains(">>>") && !file.source.contains('\u{FFFD}') {
        return Recognition::default();
    }
    Recognition {
        refine: Some(
            format!("pytest's doctest plugin collects this text file ({PYTEST_DOCTESTS})").into(),
        ),
        ..Recognition::default()
    }
}

/// Whether a test module may hold doctests: a `>>>` prompt in its text, a
/// `__test__` table (which may hold strings from elsewhere) or a
/// `__doc__` set at runtime. pytest's doctest plugin collects them beside
/// its tests.
fn may_hold_doctests(source: &str) -> bool {
    [">>>", "__test__", "__doc__"]
        .iter()
        .any(|needle| source.contains(needle))
}

/// What implements a hook, in refinement reasons.
const PLUGIN_P: &str = "a -p plugin";
const PLUGIN_ENV: &str = "a PYTEST_PLUGINS plugin";
const CONFTEST: &str = "a conftest.py";
const PLUGIN_OF_CONFTEST: &str = "a plugin a conftest.py loads";
const PLUGIN_OF_MODULE: &str = "a plugin a test module loads";
const SCOPED_CONFTEST: &str = "a conftest.py in scope";
const PLUGIN_OF_SCOPED_CONFTEST: &str = "a plugin a conftest.py in scope loads";

/// A test module's imports, followed into the project's files.
struct ProjectImports<'r> {
    supports: &'r Supports,
    /// The test module's directory.
    dir: &'r Path,
    bases: &'r [PathBuf],
    /// Code that runs before the module's imports (its own, or a
    /// conftest's) puts directories aye can't tell on `sys.path`: a module
    /// it can't find may be a project file there.
    path_unknown: bool,
    /// Directories the module itself puts on `sys.path` that aren't among
    /// `bases`.
    own_dirs: &'r [PathBuf],
}

/// Endings of class names unittest cases usually have.
const UNITTEST_ENDINGS: [&str; 4] = ["Test", "Tests", "TestCase", "TestCases"];

impl module::Imports for ProjectImports<'_> {
    fn may_be_unittest(&self, path: &str) -> bool {
        self.supports.may_be_unittest(path, self.dir, self.bases)
    }

    /// [`Supports::project_unittest`], and for a module aye can't find
    /// (an installed package, or a directory put on `sys.path` in a way it
    /// can't tell): any class when `sys.path` may hold project files it
    /// can't see, else one named like a unittest case.
    fn project_unittest(&self, path: &str) -> bool {
        if self.supports.project_unittest(path, self.dir, self.bases) {
            return true;
        }
        if module::no_tests(path) || self.supports.resolvable(path, self.dir, self.bases) {
            return false;
        }
        let name = path.rsplit('.').next().unwrap_or(path);
        self.path_unknown
            || support::found_in(path, self.own_dirs)
            || UNITTEST_ENDINGS.iter().any(|end| name.ends_with(end))
    }

    fn project_test(&self, path: &str) -> bool {
        self.supports.project_test(path, self.dir, self.bases)
    }

    fn project_factory(&self, path: &str) -> bool {
        self.supports.project_factory(path, self.dir, self.bases)
    }
}

/// Add what `summary`'s code puts on `sys.path` to `out`.
fn add_sys_path(out: &mut support::SysPath, summary: &Summary) {
    for dir in &summary.sys_path.added {
        if !out.added.contains(dir) {
            out.added.push(dir.clone());
        }
    }
    out.unknown |= summary.sys_path.unknown;
}

/// pytest's `_is_in_confcutdir`: a directory strictly above the confcutdir
/// does not load its conftest.
fn in_confcutdir(confcutdir: &Path, dir: &Path) -> bool {
    !(confcutdir.starts_with(dir) && confcutdir != dir)
}

impl Recognizer for PytestRecognizer {
    /// The command-line paths; else `testpaths`, glob-expanded (as they
    /// are with `--pyargs`), when run from the rootdir; else the current
    /// directory.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if !project.args.is_empty() {
            return project.args.iter().map(|a| project.cwd.join(a)).collect();
        }
        let testpaths = &self.config.testpaths.value;
        if project.cwd == self.config.rootdir && !testpaths.is_empty() {
            let rootdir = &self.config.rootdir;
            // With `--pyargs`, testpaths are taken as they are: dotted
            // module names pytest looks up on `sys.path` first, else paths.
            if self.config.pyargs {
                return testpaths
                    .iter()
                    .map(|entry| {
                        self.pyarg(entry)
                            .unwrap_or_else(|| normalize_absolute(&rootdir.join(entry)))
                    })
                    .collect();
            }
            let mut roots: Vec<PathBuf> = Vec::new();
            for pattern in testpaths {
                for found in glob::glob(rootdir, pattern) {
                    let path = normalize_absolute(&rootdir.join(found));
                    if !roots.contains(&path) {
                        roots.push(path);
                    }
                }
            }
            if !roots.is_empty() {
                return roots;
            }
        }
        vec![project.cwd.to_path_buf()]
    }

    /// `pytest_ignore_collect` for a directory met while walking.
    /// Directories named on the command line are never ignored by pytest
    /// and need not be asked.
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
            .value
            .iter()
            .any(|pattern| fnmatch::fnmatch_ex(pattern, dir, SETS))
        {
            return false;
        }
        self.ignored(dir) != Ignored::Yes
    }

    /// pytest's `pytest_collect_file`: a `.py` file named on the command
    /// line, or matching `python_files` and not ignored, is a test module.
    /// A `conftest.py` is support whatever else it is (ignoring it doesn't
    /// stop it loading as a conftest), and a test module too when it
    /// qualifies as one; with `--noconftest` it is only an ordinary file.
    /// When pytest's doctest plugin collects doctests in this run, the
    /// files it collects are test files too ([`Self::doctest_file`]).
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let support = name == "conftest.py" && !self.config.noconftest;
        let explicit = requested.is_named();
        let tests =
            || self.python_module(rel, explicit) || self.doctest_file(rel, explicit).is_some();
        match (support, tests()) {
            (true, true) => Claim::TestsAndSupport,
            (true, false) => Claim::Support,
            (false, true) => Claim::Tests,
            (false, false) => Claim::Ignore,
        }
    }

    fn recognize(
        &self,
        file: &SourceFile<'_>,
        requested: Requested,
        support: &[SourceFile<'_>],
    ) -> Recognition {
        let explicit = requested.is_named();
        let doctest = self.doctest_file(file.rel.as_str(), explicit);
        if self.settings.python_blocked && doctest.is_none() {
            // `-p no:python`: nothing collects Python files.
            return Recognition::default();
        }
        let bases = self.plugin_bases();
        let mut local: Vec<(&Path, Arc<Summary>)> = Vec::new();
        if !self.config.noconftest {
            for conftest in support {
                let dir = conftest.abs.parent().unwrap_or(conftest.abs);
                if in_confcutdir(&self.config.confcutdir, dir) {
                    local.push((dir, self.supports.of(conftest)));
                }
            }
        }
        if let Some((dir, message)) = self.non_top_level_plugins(&local) {
            // pytest fails the directory, so none of the tests below it
            // exist; the error is reported once.
            let mut recognition = Recognition::default();
            if self.reported.lock().expect("lock").insert(dir.clone()) {
                let rel = dir.strip_prefix(&self.root).map_or_else(
                    |_| dir.to_string_lossy().into_owned(),
                    |rel| rel.to_string_lossy().replace('\\', "/"),
                );
                recognition.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::CollectionError,
                    path: if rel.is_empty() {
                        DiagnosticPath::Root
                    } else {
                        DiagnosticPath::as_written(rel)
                    },
                    span: None,
                    message,
                });
            }
            return recognition;
        }
        match doctest {
            Some(DoctestFile::Text) => return doctest_text(file),
            Some(DoctestFile::Module)
                if self.settings.python_blocked
                    || !self.python_module(file.rel.as_str(), explicit) =>
            {
                // Only pytest's doctest plugin collects it, importing it: an
                // import error is a collection error.
                return Recognition {
                    refine: Some(
                        format!(
                            "pytest's doctest plugin imports it for its doctests ({PYTEST_DOCTESTS})"
                        )
                        .into(),
                    ),
                    ..Recognition::default()
                };
            }
            _ => {}
        }
        let mut conftests: Vec<Arc<Summary>> = self.outer.clone();
        conftests.extend(local.into_iter().map(|(_, summary)| summary));
        // `-p` plugins first, then each conftest's plugins, then the
        // conftests: the order pytest registers their fixtures in.
        let mut plugins: Vec<(Arc<Summary>, &str)> = Vec::new();
        for (summary, label) in self.preparse.iter().zip(&self.preparse_labels) {
            let expanded = self.supports.expand(summary, &bases);
            plugins.extend(expanded.into_iter().map(|s| (s, *label)));
        }
        for conftest in &conftests {
            let expanded = self.supports.expand(conftest, &bases).into_iter().skip(1);
            plugins.extend(expanded.map(|s| (s, PLUGIN_OF_SCOPED_CONFTEST)));
        }
        let global = self.global_hooks();
        let mut scope = module::Scope {
            global_hooks: global.keys().cloned().collect(),
            global_hook_sources: global.clone(),
            ..module::Scope::default()
        };
        // A collection ini value a support module in scope changes at
        // runtime (`config.addinivalue_line("python_functions", …)` in a
        // `pytest_configure`): pytest's reading of the file may differ.
        let ini_change = plugins
            .iter()
            .map(|(s, _)| s)
            .chain(&conftests)
            .find_map(|summary| summary.ini.collection.clone());
        let labelled = conftests.iter().map(|s| (s, SCOPED_CONFTEST));
        for (summary, source) in plugins.iter().map(|(s, l)| (s, *l)).chain(labelled) {
            let exports = self.supports.exports(summary, &bases);
            for hook in &exports.hooks {
                scope
                    .hook_sources
                    .entry(hook.clone())
                    .or_insert_with(|| source.to_string());
            }
            scope.hooks.extend(exports.hooks.iter().cloned());
            scope.collect_file_touches_py |= exports.collect_file_touches_py;
            scope.skips_at_import |= exports.skips_at_import;
            scope.fixtures.extend(exports.fixtures.iter().cloned());
            scope
                .maybe_fixtures
                .extend(exports.maybe_fixtures.iter().cloned());
            scope.any_fixture |= exports.any_fixture;
            scope.hooks_unknown |= summary.hooks_unknown();
            scope.patched.extend(summary.patched.iter().cloned());
            scope.reflective |= summary.reflective;
            scope.imports_module |= module::names_module(&summary.modules, file.rel.as_str());
        }
        // Fixtures the test module imports from project files register
        // with its own.
        let dir = file.abs.parent().unwrap_or(file.abs);
        let mut namespace = scope::Namespace::build(&file.module.body);
        // What support files do to modules holds here too.
        namespace.assume_patched(scope.patched.iter().cloned(), scope.reflective);
        if !module::reached(&namespace, &scope, file.rel.as_str()).is_empty() {
            // Its names may change in ways the module's code doesn't show:
            // none is a constant.
            namespace.assume_reached();
        }
        let own = Summary::of_test_module(dir, file.source, file.module, &namespace);
        // Only names the module may request as fixtures are followed, and
        // those the fixtures found request in turn.
        let mut requested = support::requested_names(file.module);
        let exports = loop {
            let exports = self
                .supports
                .exports_of(&own, &bases, |name| requested.contains(name));
            let before = requested.len();
            for fixture in &exports.fixtures {
                requested.extend(fixture.args.iter().cloned());
            }
            if requested.len() == before {
                break exports;
            }
        };
        scope.module_imports = module::ModuleImports {
            fixtures: exports.fixtures[own.fixtures.len()..].to_vec(),
            maybe: exports.maybe_fixtures,
            any: exports.any_fixture,
        };
        if !own.plugins.is_empty() || own.plugins_unknown {
            scope.module_plugins = true;
        }
        // `sys.path` may hold directories aye can't tell: those a conftest
        // puts there in ways it can't read, or that the module puts there
        // itself (its own imports would be looked up there).
        let own_dirs: Vec<PathBuf> = own
            .sys_path
            .added
            .iter()
            .filter(|d| !bases.contains(d))
            .cloned()
            .collect();
        let path_unknown = self.sys_path().unknown || own.sys_path.unknown;
        if self.sys_path().unknown {
            // A plugin a conftest in scope loads that aye can't find may be
            // a project file there, with fixtures of its own.
            scope.hidden_fixtures =
                plugins
                    .iter()
                    .map(|(s, _)| s)
                    .chain(&conftests)
                    .any(|summary| {
                        summary
                            .plugins
                            .iter()
                            .chain(&summary.registered)
                            .any(|name| {
                                !BUILTIN_PLUGINS.contains(&name.as_str())
                                    && !module::no_tests(name)
                                    && self.supports.module(name, &summary.dir, &bases).is_none()
                            })
                    });
        }

        let imports = ProjectImports {
            supports: &self.supports,
            dir,
            bases: &bases,
            path_unknown,
            own_dirs: &own_dirs,
        };
        let mut recognition = module::recognize(
            &self.settings,
            &scope,
            &imports,
            file,
            &namespace,
            &self.path_keywords(file),
        );
        if recognition.cases.is_empty() && recognition.refine.is_none() {
            // Nothing to run statically, but pytest imports the module all
            // the same: an import error is a collection error, and tests
            // may only exist at runtime.
            recognition.refine =
                Some("no test is visible statically; pytest imports the module to tell".into());
        }
        let maybe_ignored = file
            .abs
            .ancestors()
            .take_while(|dir| dir.starts_with(&self.root) || *dir == file.abs)
            .any(|path| self.ignored(path) == Ignored::Maybe);
        if maybe_ignored {
            let reason = "collect_ignore may exclude this file at runtime".to_string();
            recognition.refine = Some(match recognition.refine {
                Some(existing) => format!("{reason}; {existing}").into(),
                None => reason.into(),
            });
            // None of its tests may exist.
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        if let Some(changed) = ini_change {
            let reason = format!(
                "a conftest.py in scope changes {changed} at runtime, which collection reads"
            );
            recognition.refine = Some(match recognition.refine {
                Some(existing) => format!("{existing}; {reason}").into(),
                None => reason.into(),
            });
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        let mut key = self.import_key(file.abs);
        if let Some(name) = self.colliding_import(file) {
            let reason = format!(
                "it imports {name}, a module of more than one directory pytest puts on sys.path: \
                 which one it gets depends on what pytest imported before"
            );
            recognition.refine = Some(match recognition.refine {
                Some(existing) => format!("{existing}; {reason}").into(),
                None => reason.into(),
            });
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
            // Test modules that import it run in one session, as pytest
            // runs them.
            key = AffinityKey::import(&name);
        }
        for case in &mut recognition.cases {
            case.affinity.insert(0, key.clone());
        }
        if !self.unseen_plugins.is_empty() {
            // A plugin aye can't read may mark, deselect or parametrize any
            // test: the ids stay, pytest has the final word.
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        if let Some(who) = &self.scan().adds_items {
            // pytest lists what such a hook adds (`Function.from_parent`),
            // which only the session that runs it can tell.
            let reason =
                format!("{who} implements {MODIFYITEMS} and makes items: it may add tests");
            recognition.refine = Some(match recognition.refine {
                Some(existing) => format!("{existing}; {reason}").into(),
                None => reason.into(),
            });
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        if doctest == Some(DoctestFile::Module) && may_hold_doctests(file.source) {
            let reason =
                format!("pytest's doctest plugin collects its doctests ({PYTEST_DOCTESTS})");
            recognition.refine = Some(match recognition.refine {
                Some(existing) => format!("{existing}; {reason}").into(),
                None => reason.into(),
            });
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        recognition
    }

    fn settings(&self) -> Vec<Setting> {
        let mut settings = self.config.settings();
        if let Some(doctests) = &self.doctests {
            let mut what = Vec::new();
            if doctests.modules {
                what.push("modules (--doctest-modules)".to_string());
            }
            if doctests.globs.is_empty() {
                what.push("text files matching test*.txt".to_string());
            } else {
                what.push(format!("text files matching {}", doctests.globs.join(" ")));
            }
            settings.push(Setting {
                name: "doctests".into(),
                value: format!("collected by pytest's doctest plugin: {}", what.join(", ")),
                source: ValueSource::Rule(PYTEST_DOCTESTS),
            });
        }
        settings
    }

    /// What the front-end needs to start as pytest would start itself (the
    /// rest is the native context's: see [`native::NativeRecognizer`]), and
    /// what the runner takes from pytest's options: [`PytestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.pytest_context())
    }

    /// What the context's keys for the runner say ([`PytestContext::defaults`]).
    fn defaults(&self) -> FrameworkDefaults {
        self.pytest_context().defaults()
    }
}
