//! The pytest adapter, a faithful implementation of pytest's rules: pytest
//! is a front-end of aye's engine, and pytest itself is never imported.
//!
//! Rust half: pytest's configuration, read through aye-pytest (rootdir,
//! inifile, ini values and the options `addopts`, `PYTEST_ADDOPTS` and the
//! arguments after `--` add, with what the runner takes over lifted out),
//! the files it claims, the
//! context the front-end starts from ([`Recognizer::context`]), and the static
//! recognizer: file patterns, `Test*` classes and `test*` functions from
//! the project's pytest config, inherited methods, markers, parametrize
//! and pytest's id algorithm (including `pytest.param`, `ids=`, stacked
//! decorators, duplicate suffixes, escaping), conftest parsing for fixtures
//! and collection-affecting hooks, pytest's rootdir and ini semantics
//! (asked from aye-pytest). The static recognizer predicts the
//! cases `aye --co --no-refine` lists without importing anything; every
//! other run has the front-end list each test file (see [`Pytest`]).
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
//! Depends on: aye-adapter, aye-config (where a setting came from, the
//! worker count), aye-engine (the modules its front-end runs on),
//! aye-events, aye-protocol, aye-pytest (pytest's configuration, the
//! variables it reads, the raw ini values the front-end's `getini`
//! coerces), aye-python-semantic (what a module binds, read with
//! pytest's `Framework`, `PYTEST`: namespaces and the escape analysis,
//! static values, classes and their MRO, decorators, import-time facts,
//! `sys.path` and module resolution), aye-python-stdlib (`fnmatch`, `glob`,
//! `os.path`, ids' reprs and `str.isdigit`, identifiers, unittest's bases).
//! Must not be depended on by any runner crate; only the binary links it.
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
//!   `conda-meta/history`). The crate reads pytest's configuration through
//!   `aye-pytest`. [`Pytest::prepare`] always returns a recognizer: pytest
//!   is the default for Python tests.
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
//!   module's name; see aye-python-semantic's `namespace`). Dict
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
//!     the escape analysis, which doesn't count `super(K, …)`,
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
//!   ([`Pytest`]), and a test is never silently dropped. With
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
//! with the pytest front-end ([`Pytest`]). The executor that drove a real
//! pytest session per batch, and what only fed it (its filtered options as
//! separate vectors and a `PYTEST_ADDOPTS` string, its `-p no:` lists, the
//! context keys only it read: `addopts`, `pytest_addopts_env`,
//! `framework_args`, `doctests`, `adds_items`, `single_session`), are
//! gone. What it did that pytest users rely on is the front-end's and the
//! engine's now; their modules say how.

mod adapter;
mod claims;
mod conftest;
mod context;
pub mod embedded;
mod marks;
mod options;
mod predict;
mod recognizer;
mod session;

#[cfg(test)]
mod tests;

use aye_python_semantic::framework::Framework;
use aye_python_stdlib::fnmatch::SetTranslation;

pub use adapter::{NAME, Pytest};
pub use aye_pytest::argv::split_xdist;
pub use aye_pytest::config::PytestConfig;
pub use aye_pytest::env::PytestEnv;
pub use recognizer::PytestRecognizer;

/// How pytest's patterns read `[...]` sets here (`python_files`,
/// `python_classes`, `python_functions`, `norecursedirs`, the ignore globs,
/// `--doctest-glob` and `testpaths`): as this adapter always has, which
/// differs from CPython for a set whose first item is a reversed range
/// followed by `!`.
pub(crate) const SETS: SetTranslation = SetTranslation::Legacy;

/// What pytest does with a module's code, for aye-python-semantic's
/// readings of it: marks and `pytest.param` keep their arguments, its own
/// decorators leave a function alone, it calls `pytest_*` hooks while it
/// collects, and `pytest.skip` or `pytest.importorskip` at import skips
/// the module.
pub(crate) const PYTEST: Framework = Framework {
    packages: &["pytest", "_pytest"],
    marks: Some("pytest.mark."),
    inert_calls: &["pytest.param", "pytest.fixture"],
    harmless_decorators: &["pytest.", "_pytest."],
    hooks: Some("pytest_"),
    skips: &["pytest.skip", "pytest.importorskip"],
};
