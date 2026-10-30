# Architecture

aye takes the thing every Python developer runs many times a day and makes it a
drop-in replacement: no migration, dramatically faster, and absorbing the pile
of plugins people install next to it (xdist, cov, asyncio, mock, timeout,
rerunfailures, randomly, env, sugar) into one tool with one config section.

## Vocabulary

The words are binding in code, docs, events, the command line and reports.
The full definitions are in `crates/aye-engine/src/lib.rs`.

- **Runner**: aye, the Rust binary. Which tests run, where, when, in what
  order and under what limits; collecting and reporting what happened.
- **Engine**: aye's engine, in Python, inside workers. What a test is and how
  it executes: tests, lifetimes, the order of setup and teardown, outcomes.
  "Engine" alone always means aye's, and it runs every test: no framework's
  engine does. It lives in `aye-engine`.
- **Adapter**: maps one existing framework (pytest, unittest, doctest) onto
  aye's engine. Its front-end reads the framework's tests into aye's model
  and never runs them; for pytest, the test modules and conftests import
  aye's own `pytest` module, which stands in for pytest's authoring API.
  Each adapter is an `aye-adapter-*` crate, implementing `aye-adapter`.
- **Framework**: what people write tests against (pytest, unittest). aye
  doesn't ship one yet.
- **Library**: aye's importable features (patching, fakes, decorators,
  snapshots, test data), usable from any framework.
- **Worker**: a Python process managed by the runner, in which the engine
  executes batches.

## Core decisions

Don't deviate from these without a note in this file explaining why.

1. **Compatibility first.** Existing tests, fixtures, `conftest.py`, markers
   and parametrize run unchanged, and `import pytest` gets aye's own
   `pytest` module, which implements pytest 9.1.1's authoring API. pytest's
   configuration is read from `pytest.toml`, `pytest.ini`,
   `pyproject.toml`, `tox.ini` and `setup.cfg` using pytest's rootdir rules.
   CLI flags mirror pytest's, and exit codes 0–5 match. unittest suites run
   as `python -m unittest discover` runs them, and doctests as pytest's
   doctest plugin does. What aye can't run yet (a conftest hook it doesn't
   support, a third-party plugin) is a usage error or a warning naming it,
   never a silently different result. Deliberate differences are listed in
   [compatibility.md](compatibility.md).
2. **One event stream; everything else is a renderer.** The runner emits a
   single canonical, versioned stream ([events.md](events.md)). Text
   output, JSON lines (the raw event log among them), JUnit XML, GitHub
   annotations, run history and the run store (each run's log, kept for
   `aye show`) all consume it, as the live progress display will. No
   renderer may reach into runner state.
3. **A Rust runner, aye's engine in Python, and the batch is the line.** The
   runner owns arguments, config, AST-based collection
   (`ruff_python_parser`/`ruff_python_ast`), selection, scheduling, worker
   lifecycle and rendering: everything up to handing a batch to a worker and
   everything after its events come back. The engine, in Python workers, owns
   everything in between. Listing happens in workers too: a front-end lists
   the session once per worker, and the import that lists a module is the
   one its tests run from.
4. **Every framework is a front-end; aye's engine runs every test.** aye's
   engine has its own model of what a test is (tests, parameters, resources
   with scopes and teardown, lifetimes, conditions, outcomes) and executes
   it itself. A front-end reads one framework's tests into that model and
   never runs them; nothing runs through another framework's runner or
   engine. aye ships its own `pytest` module (standard library only),
   installed in every worker as `pytest` before any test or conftest is
   imported, and the pytest front-end reads what test modules and conftests
   declare through it: pytest is never imported, and need not be
   installed. Exactness comes from implementing pytest 9.1.1's semantics,
   judged by the comparison harness against plain pytest. The unittest and
   doctest adapters are front-ends on the same engine: unittest's class and
   module setup are lifetimes the engine opens and closes.

   Why this changed (October 2026): until then each batch ran in a real
   pytest session. Every file the static reader couldn't predict was
   imported twice (to refine it, then to run it), pytest's own machinery
   cost far more per test than most test bodies, and aye couldn't plan
   lifetimes it didn't own. The native engine ran as a prototype beside it
   (`--native`) until it matched plain pytest on the harness and on real
   suites; it is now the only engine, and `--native` is accepted and does
   nothing.
5. **Static selection is a superset.** Rust-side collection and `-k`/`-m`
   evaluation must never drop a test the framework would run (pytest,
   unittest's discovery, pytest's doctest plugin). Wherever static
   analysis can't know the answer (parameter ids from runtime values,
   `pytest.param(marks=…)`, hook-added markers), evaluation returns
   `Unknown` and the case is kept, and files whose tests only exist at
   runtime are refined by importing them in a worker. The pytest front-end
   lists every pytest file a run runs (refinement), so pytest's cases are
   its own. No adapter applies an expression at runtime: static selection
   is the last word on every case.

## Data flow

```
argv ─▶ aye: args, Session
          │
          ├─▶ aye-config ─────────▶ project root, [tool.aye]
          ├─▶ aye-provision ──────▶ the interpreter
          ├─▶ registry ───────────▶ the adapters (aye-adapter-*), in priority order
          ├─▶ adapters prepare ───▶ a recognizer each (pytest's config,
          │                         [tool.aye.unittest], [tool.aye.doctest])
          ├─▶ aye-collect ────────▶ cases ◀── refine ── worker (imports flagged files)
          ├─▶ aye-select ─────────▶ selected cases
          └─▶ aye-runner ─────────▶ batches ──▶ workers: python -c aye_worker.py
                                                  aye's engine (aye-engine), aye's
                                                  pytest module and what it stands
                                                  on, and one adapter's executor
                                                  (pytest: its front-end, the
                                                  session listed once per worker;
                                                  unittest, doctest)
                                                             │ JSON lines (aye-protocol)
          records, stamped into one log ◀────────────────────┘
                     │
         ┌──────────────┬──┴────────────┬────────────────────┬──────────┬───────────┐
         ▼              ▼               ▼                    ▼          ▼           ▼
       text        JSON lines       JUnit XML     GitHub annotations   history   run store
                                                                                    │
                                                      aye show (text, JSON lines) ◀─┘
```

## Crates

The leaf crates (`aye-index`, `aye-static`, `aye-fs`) depend on no aye
crate. Runner crates never depend on an `aye-adapter-*` crate. Only the binary
links the adapters. `aye-runner`, `aye-render`, `aye-history` and
`aye-select` compile no Ruff: the runner speaks `aye-protocol` and embeds
`aye-engine`, and only `aye-collect` and the adapters, through
`aye-adapter`, read Python's syntax. Each crate's `lib.rs` states its
responsibility, boundaries and decisions.

| Crate | Responsibility |
|---|---|
| `aye-index` | Index newtypes (`newtype_index!`) and the collections they index, `IndexVec` and `IndexSlice`. Depends on no aye crate. |
| `aye-static` | `EnvVars`, every environment variable aye reads by name or sets, in Rust or in the Python it sends to workers, with what each does; `ProcessEnv`, the environment as aye read it, looked up as `os.environ` does: the binary reads it once and hands it down, and `clippy.toml` disallows `std::env`'s readers elsewhere (the tests' own settings excepted). Depends on no aye crate. |
| `aye-fs` | Lexical path algebra (`normpath` and `normalize_absolute`, both of aye's normalizers, named; `absolute`, `relative_to`, `common_ancestor`, the two relativizers, paths shown with `/`, `expand_path` over a lookup it is given) and the state directory: `StateDir` (`.aye`) and its buckets (`history.json`, `runs`, `cache`), `prepare_dir` with the `.gitignore` and `CACHEDIR.TAG` markers, `atomic_write`. Depends on no aye crate. |
| `aye` | The binary: the CLI, the environment it reads once (`EnvironmentOptions`: the variables it reads by name and the whole of it, which it hands down), the run (`session.rs`), `aye config`, `aye show` (`show.rs`), and the adapter registry (`registry.rs`: pytest, unittest and doctest, which of them a project runs, and the prelude every worker installs before an adapter's modules, aye's `pytest` module and what it stands on). |
| `aye-events` | The wire types: events, the envelope and the `Stamper` that writes a run's part of it, outcomes and a test's earlier ones (`OutcomeTrail`), failures (an assertion's `ComparisonOp`, a comparison's `CompareKind`, the `FailureSignature` that groups them), the `RerunCommand` a bad outcome's `reproduce` holds, cases, a batch's `FrameworkStatus`, why a run stopped (`StopReason`), exit codes, `SCHEMA_VERSION`; the identities and paths they hold, each taken apart in one place (`TestId` and its parts, the `IdPrefix` that deselects tests, `TestKey`, `AdapterName`, `LifetimeId` and its `Scope`, `AffinityKey`, `RunId`, `RunNumber`, `WorkerIndex`, `BatchIndex`, `EnvironmentId`, `ProjectPath`, `DiagnosticPath`, `FramePath`, `LineNumber`); the diagnostic codes (`DiagnosticCode`, `Severity`) and the one classification of an error the exit code, the summary and the run store answer from (`ErrorClass`); FNV-1a, the one hash aye computes (`Fnv64`, `Fnv128`), and a file's `ContentDigest`, which the run store and the refinement cache keep. Depends on serde and `aye-index`. |
| `aye-protocol` | The worker protocol: what the runner sends a worker and what it answers (`Request`, `Reply`, `WorkerLine`), a `Batch`, its `Policy` and its `BatchContext` (the adapter's opaque `AdapterContext` and the keys the binary and the runner add, `context_keys`), a `maxfail` limit (`MaxFail`), refinement's messages (`RefineRequest`, `Refined`), the handshake's modules, borrowed from the binary (`ModuleSource`), in the order a worker installs them (`ModuleSet`), and its executors (`ExecutorSpec`), an adapter's runtime face (`Runtime`: its modules, its executor's `EntryPoint` and its `SessionMode`), the services a worker may provide (`ServiceName`), `PROTOCOL_VERSION`, and the `Refiner` trait the runner implements for collection; `FrameworkStatus`, re-exported. Depends on `aye-events` only. |
| `aye-engine` | aye's engine: the Python every executor builds on (`python/aye_engine.py`, which every worker installs first: `ENGINE`), the test model, assertion rewriting and the native engine (`aye_model.py`, `aye_rewrite.py`, `aye_native_engine.py`), embedded as `ModuleSource`s with their Python tests; and the vocabulary. Depends on `aye-protocol` only. |
| `aye-adapter` | What an adapter implements: the `Adapter` trait (which prepares a recognizer, and gives the `Runtime` its workers install and build) and the `Recognizer` trait (a recognizer claims paths as they were `Requested`, named or found, and says whether it `claims_any_name` of a kind), `Claim`, `Recognition` with its `RefineReason`, `SourceFile`, `Project` (with the environment and `[tool.aye]` the binary read), `Setting` with its `ValueSource`, what a framework's configuration says about the run (`FrameworkDefaults`, `JunitDefaults`), `AdapterError`; Ruff's syntax tree, line index and text ranges, re-exported with the `get-size2` pin, so collection and the adapters share one version. Depends on `aye-events`, `aye-protocol`, `aye-config` and `aye-static`. |
| `aye-config` | The project root, `[tool.aye]` as written (`AyeOptions`) and resolved (`AyeSettings`), its values, typed and shared with the command line where it takes the same one (`WorkerCount`, `OutputFormat`, `Timeout`, `KeepRuns`, `AdapterList`), where a resolved value came from (`ValueSource`, and `Sourced` values, equal by their values) and how `aye config` shows it, and generic TOML and INI loaders. Paths come from `aye-fs`, adapters' names from `aye-events`. |
| `aye-collect` | Discovery: the walk, parsing with Ruff's parser, recognizers, merging, refinement and its cache (keyed on `aye-provision`'s `EnvironmentFingerprint`, through which alone it reads the interpreter), diagnostics, strict mode. Knows no framework's rules. |
| `aye-select` | Selection by path, id, dotted name, key, keyword expression, tag expression and last failed, with three-valued logic. |
| `aye-runner` | The spawn path (of the interpreter it is given) and the handshake (aye's engine, the binary's prelude, then each runtime's modules), the transport, scheduling, `maxfail`, crash recovery, the log's stamping (with `aye-events`' `Stamper`), and the worker (`python/aye_worker.py`). |
| `aye-render` | Renderers: text, JSON lines (at each verbosity, up to the raw events), JUnit XML, GitHub annotations; test keys (`Keys`), what the summaries count beside the tests (`Tally`), and what `aye show` asks of them (`ShowMode`, `ShowContext`). |
| `aye-history` | `.aye/history.json`: durations, outcomes (the last, and the recent ones, with when a bad streak began) and last failed, folded from the stream; and the run store, `.aye/runs` (`runs.rs`): each run's id (`StoredRunId`: its number, `r81`, or a hex id from before runs were numbered), log, gzip-compressed, its summary, `last`, retention. Both are written through `aye-fs`. |
| `aye-provision` | The interpreter: what was asked for (`PythonRequest`, from a `RequestSource`), discovery (`find`, over a `DiscoveryEnv`), the `Interpreter` found and its `PythonSource`; and what the refinement cache's key holds of the environment refinement runs in (`EnvironmentFingerprint`: the interpreter, its venv's layout and installed packages, the variables workers inherit, the framework's arguments, and the `InstalledModules` whose content the key doesn't hold). Later, for the Python matrix (Step 5): environments from uv or paths you supply. Depends on `aye-events`, `aye-fs` and `aye-static`. |
| `aye-adapter-pytest` | The pytest adapter: pytest's config discovery, the static recognizer (for claims and `--co --no-refine`), and, embedded, aye's own `pytest` module (`python/native/aye_pytest_shim.py`, which the registry puts in every worker's prelude), the pytest front-end (`aye_pytest_frontend.py`) and its executor (`aye_pytest_native.py`), on aye's engine (`aye-engine`'s `aye_model.py`, `aye_native_engine.py`, `aye_rewrite.py`). |
| `aye-adapter-unittest` | The unittest adapter: `[tool.aye.unittest]`, discovery as `python -m unittest discover` does it, the static recognizer, and a native executor (`python/aye_engine_unittest.py`). |
| `aye-adapter-doctest` | The doctest adapter: when doctests are on, the static recognizer of docstrings and text files (pytest's doctest options, `[tool.aye.doctest]`), and a native executor (`python/aye_engine_doctest.py`). |

The Python package under `python/aye` is only a shim, so that `python -m aye`
finds the binary. The worker, aye's engine, aye's `pytest` module and every
executor are Python source embedded in the binary (`include_str!`), sent to
each worker in the handshake and installed from memory. So they work
however aye was installed (pip, uv tool, cargo), and they can't skew from
the binary's version. Each crate owns its modules in install order, as a
`ModuleSet` (`aye-engine`'s `ENGINE`, each adapter's in its `Runtime`), and
the registry composes the prelude every worker installs between them:
aye's `pytest` module and the modules it stands on.

## Adapters

A run uses the adapters `[tool.aye] adapters` lists, in that order, which is
also their priority; an unknown or repeated name, or an empty list, is a
usage error. Unset, it is doctest then pytest, for every project and
whatever the interpreter installs: aye's engine runs pytest's tests without
pytest, and pytest's rules run `TestCase` classes too. (Until October 2026
the default fell back to doctest then unittest when nothing said pytest: no
pytest configuration, no `conftest.py` and no pytest installed.) A project
that wants `python -m unittest discover`'s rules lists `unittest`. Doctest
comes first because pytest lists a file's doctests before its other tests;
it collects only what pytest's doctest plugin would (`--doctest-modules`,
`test*.txt` files) or what `[tool.aye.doctest]` asks for. `aye config`
prints the adapters and why (`from [tool.aye] adapters`, or `default:
pytest's rules, which run unittest's TestCase classes too`). The table of an
adapter that doesn't run (`[tool.aye.unittest]`) is checked all the same.

Each adapter prepares with `Adapter::prepare_among`, knowing the others:
doctest's default text glob (`test*.txt`) holds when a pytest configuration
exists or pytest runs beside it, and the pytest adapter lifts pytest's
doctest options out of what its front-end parses when the doctest adapter
runs (see [Static collection (doctest)](#static-collection-doctest)).

Two adapters may collect one test: unittest's and pytest's adapters both
give a `TestCase` method the id `path::Class::method`. The higher-priority
adapter's case is kept, without a warning: listing unittest before pytest
is how a project has unittest's rules run its `TestCase` classes and
pytest's the rest. One adapter producing an id twice gets a `duplicate-id` warning.

A worker serves one adapter: two frameworks in one process would share
imported modules (two `conftest.py`, a unittest suite run beside pytest's
conftests and hooks). It holds only the executor of its first batch's
adapter. Every worker has aye's `pytest` module, whatever its adapter, so
what the unittest and doctest adapters import (a conftest, a test module
beside `TestCase` classes) gets it rather than pytest. A lane takes its
adapter's batches first; a batch of another adapter gets a new worker on
that lane, once the old one has shut down, so `workers` stays the cap on
live workers.

## A run

1. **Resolve.** The binary parses the arguments and finds the project root:
   the nearest directory holding `pyproject.toml`, else `.git`, searching up
   from the paths given (with `.` and `..` resolved first) or the current
   directory. A path is taken from the current directory, or from the root
   when only the root has it, so the ids aye prints rerun from anywhere.
   Arguments after `--` that would replace the test run or run no test
   (pytest's `-h`, `--co`, `--fixtures`, `--setup-plan`, …) are rejected.
   It reads `[tool.aye]`, finds the interpreter, chooses the adapters (see
   [Adapters](#adapters)), lets each read its framework's config (pytest's
   files, `addopts`, `PYTEST_ADDOPTS` and the arguments after `--`;
   `[tool.aye.unittest]`; `[tool.aye.doctest]`), parses `-k` and `-m`,
   picks the worker count, and checks that the JUnit report can be
   written. `--no-refine` without `--co` is a usage error when pytest's
   adapter runs: its front-end imports every file the run runs. Anything
   that fails here is a usage error (exit 4), reported
   before any event; so is a run with no path from the home directory or a
   filesystem root (a home directory under git is a project root), which
   would collect every project in it. On a terminal, a `collecting…` line
   shows if this takes over half a second (the pytest adapter's look at the
   directories the run collects can, in a big tree).
2. **Collect.** `aye-collect` walks the explicit paths, or else each
   recognizer's roots (pytest's `testpaths`, unittest's start directory, or
   the current directory), which it also walks when dotted names or keys
   come with the paths. Dotted names and keys are resolved among the cases
   of that whole walk, so refinement reports on all of it, its sessions
   starting from the framework's default: the binary keeps only what the
   run reaches (a `skipped` info a path argument or dotted name reaches; a
   start-up `usage-error` on a conftest in a path argument or whose
   directory holds a path the run starts from, or any when an argument
   named nothing), as `pytest <the paths and the selected files>` would
   report. It offers each file to every recognizer, and parses
   claimed Python files on every core. A file is claimed for tests, as
   support (pytest's conftests) or as both (a conftest matching
   `python_files`), and yields tests only for the recognizers whose walk
   reached it. Recognizers predict the cases in each file from its AST and
   its support files (a text file reaches them as text). A file they can't
   predict is refined: a worker imports it and the framework's front-end
   reports its real tests, and its diagnostics replace the recognizer's
   static errors for the file. The pytest adapter refines every test file
   (its front-end is the single truth for pytest's ids), except for `aye
   --co --no-refine`, which lists the static recognizer's prediction
   without importing anything. The worker that refines pytest's files is
   kept: it becomes the run's first lane. Refinement is cached under
   `.aye/cache/refine/` (see [Static collection](#static-collection-pytest)).
   A `refine-failed` warning is an error: without the front-end, pytest's
   tests are unknown. A hook that fails while the front-end collects is
   pytest's `INTERNALERROR`: the run exits 3 before anything runs. Cases
   are merged by file, then adapter priority; an id two adapters collect
   keeps the higher-priority adapter's case.
3. **Select.** `aye-select` keeps every case whose verdict isn't `False`,
   then applies last failed, and puts what is kept in the order of the
   arguments, as pytest does. A `-k` or `-m` after `--` applies to every
   case, else the command line's, else each adapter's own, from its context
   (pytest's from `addopts` or `PYTEST_ADDOPTS`), to its own cases. When the
   arguments after `--` can't be read (an `@file`, a short option aye
   doesn't know that might hold the letter), every adapter's cases get the
   command line's, else their own: no adapter applies an expression at
   runtime. An adapter's `--deselect` prefixes (its context's `deselect`)
   drop its cases whose ids start with one and count them as deselected.
   An argument that names
   nothing (a missing path; an id, dotted name or key that matches no test)
   is a `not-found` diagnostic, and the run exits 4 before anything runs, as
   pytest does. An existing file or directory without tests is no error.
4. **Run.** `aye-runner` groups the cases into batches, each of one
   adapter, and starts workers, which pull batches until none are left.
   Each adapter's context gets the ids with parameters the command line
   names (`requested_ids`), so the framework reports one it doesn't make as
   `not-found`, aye's options that shaped the run (`rerun_args`), for
   reruns, and, when some positional argument isn't a path, the paths the
   user's run starts from (`selected_paths`: the path arguments and the
   files the dotted names and keys select), so the pytest and doctest
   executors start from them as `pytest <those paths>` would. The plan's
   session is, by adapter, every file collection found tests in, selected
   or not (with dotted names or keys, only those in the paths the run
   starts from): every lane of the pytest adapter lists it before its first
   batch, as each pytest-xdist worker collects the whole session. Each
   record a worker sends is stamped into the run's log and handed to the
   renderers and history.
5. **Finish.** The binary picks the exit code ([events.md](events.md#exit-codes))
   and emits `run_finished`, with `stopped` when the run ended early.

## Static collection (pytest)

The static recognizer decides which files are pytest's (claims, the walk's
rules), reads pytest's configuration, and predicts the cases `aye --co
--no-refine` lists without importing anything. A run, and `--co`, have the
pytest front-end list every pytest test file instead (see [aye's engine and
the pytest front-end](#ayes-engine-and-the-pytest-front-end)), so what
follows is that prediction, which the comparison harness still checks:
every case it calls exact is plain pytest's.

The recognizer follows pytest 9.1.1: config discovery as in
`_pytest/config/findpaths.py`, ids as pytest's `IdMaker`, and collection as
`PyCollector`, `Class` and `UnitTestCase`. Each case gets a certainty:

- **Exact**: files matching `python_files`, `Test*` classes without
  `__init__`, `test*` functions and inherited methods, unittest `TestCase`
  classes, markers from decorators and `pytestmark`, and parameter ids
  computed from static values (literals, `range(...)`, module and class
  constants an escape analysis shows nothing can change, values of
  well-known constructors whose arguments they surely accept,
  `pytest.param(id=…)`, `ids=` lists, duplicate suffixes, escaping).
  Other modules' attributes used as values (`math.pi`) are not read:
  another module of the session may change them.
- **Refined**: files whose ids, or which tests exist, may differ from the
  static reading:
  - parametrize arguments that aren't static, that may raise when built (a
    display or a call aye can't confirm, such as `[*1]` or
    `range(0, 3, 0)`), that depend on a module the file or a support file
    may change (`re.compile = …`, `vars(builtins)[…]`, a `global`
    rebinding), or that pytest refuses (a bad `scope` or `indirect`,
    `pytest.param` marks that are a set or hold no mark); `ids=` callables;
    values whose id depends on the Python version;
  - parametrized fixtures in the test's closure, `pytest_generate_tests`,
    and parametrization of package or session scope;
  - collection hooks (`pytest_pycollect_makeitem`, `pytest_collection`,
    `pytest_ignore_collect`, …), conftests that load plugins aye can't name,
    a test module's own `pytest_plugins`, and code that changes `pytest`'s
    API or collection internals;
  - classes whose bases aren't all known, class members added at runtime
    (class decorators, metaclasses, `__init_subclass__`), decorators aye
    can't see through on a test, `pytestmark` entries it can't read,
    generator tests, imported or assigned names that may be tests
    (capitalized names from project files that may be unittest cases
    included), star imports, tests defined in a branch that may not run,
    and a `del` that may not run;
  - module namespaces reached in ways aye can't follow (`sys.modules[…]`
    with a computed name, frames, `f_globals`, `__globals__`, `gc`, an
    import by a computed name, the module importing itself), and a conftest
    in scope that does so, imports a module of the test module's name, or
    implements `pytest_collectstart` or `pytest_make_collect_report`;
  - module-level calls that may raise (`X = range(0, 3, 0)`), module-level
    `pytest.skip` or `importorskip`, a test file with no test aye can see
    (pytest imports it all the same), and whatever pytest reports as a
    collection error.
- **Partial**: the static guess for a file that needs refining, when
  refinement is off (`--no-refine`) or fails. Where parameters are unknown
  it holds the unparametrized id, which runs every parameter. A file
  flagged only because a module-level call may raise keeps its exact ids.
  Also, with exact ids, cases whose tags may be incomplete: hooks that only
  mark or deselect (`pytest_collection_modifyitems`), decorators of
  libraries known to keep the test (Hypothesis, `flaky`), fixtures imported
  from modules outside the project, and `-p` plugins aye can't read.

pytest's options come from `addopts`, `PYTEST_ADDOPTS` and the arguments
after `--`, in pytest's order, the last one winning. As in pytest, the latter
two are read first for `-c`, `--rootdir` and `-o`, which take part in finding
the config (`-o addopts=` can replace `addopts`). `-o`, `--strict`,
`--ignore`, `--ignore-glob`, `--deselect`, `--confcutdir`, `--noconftest`,
`--collect-in-virtualenv`, `--pyargs`, `--import-mode`, `-p`, `-k` and `-m`
are read from all three. The adapter's context names the `--deselect`
prefixes (`deselect`), which selection applies. pytest-xdist's options,
`-x`/`--maxfail` and the JUnit options are lifted out of all three, since
aye runs the workers, counts `maxfail` and writes the report, and so are
pytest's doctest options when the doctest adapter runs; the rest, in
pytest's order, is what the front-end parses (the context's
`native.args`). A `conftest.py` that matches `python_files`, or is named on
the command line, is a test module too, as in pytest. A conftest pytest
doesn't load at start-up that surely defines `pytest_plugins` fails its
directory with pytest's collection error; one aye isn't sure of is left to
the front-end. A file below a conftest that may skip while loaded
(`pytest.importorskip`) is refined, so a skipped directory has no tests and
one `skipped` info, as pytest counts one skip.

Without the doctest adapter in the run (`[tool.aye] adapters = ["pytest"]`),
the files pytest's own doctest plugin would collect (unless `-p no:doctest`
blocks it) are claimed as test files and refused with a `usage-error` that
names the fix, adding `doctest` to the adapters: aye's engine runs doctests
through the doctest adapter only. `aye config` shows a `doctests` setting
saying who would collect them.

A hook that makes items in `pytest_collection_modifyitems`
(`Function.from_parent`) makes every static case partial, since the hook
sees the whole session's items.

The refinement cache must never answer differently from a fresh import. Its
key covers aye's version, the root and the current directory, the interpreter
and its installed packages, the environment variables (but a few of the
shell's), the arguments after `--`, the adapter's context and settings (with
the content of the files they name, such as the inifile), and the content of
the file, its support files and its packages' `__init__.py` files. The key
holds installed packages by name, not content, so only standalone files are
cached. The file, its conftests and those `__init__.py` files must import
only pure standard-library modules and the framework, touch no files,
processes, clock or randomness, run no code by name (`__import__`,
`import_module`, `importorskip`), and name no project or installed module in
a string (a `pytest_plugins` list). The context and the arguments after `--`
must load no plugin by name (`-p NAME`, unless it names pytest or a pure
standard-library module) and name no project or installed module. Installed
modules are what the interpreter's paths list: its `site-packages` entries,
the directories its `.pth` files and `PYTHONPATH` add, and its
distributions' names and `top_level.txt`. Only clean results are written:
none with a diagnostic, none without a case, and nothing from a request
whose reply had diagnostics or cases outside the requested files. A cached
result is used even with `--no-refine`.

Known gaps:

- Installed plugins, plugins `PYTEST_PLUGINS` names and `sitecustomize` are
  not seen; only conftests and the project files their `pytest_plugins`,
  registrations or `-p` load. Nor is what they, or another test module of
  the session (pytest imports them all into one process), change in a
  module whose values aye reads, nor a module changed through another
  module's alias for it (`helpers.m.x = …` for `import math as m`). Tests
  that a `pytest_collect_file` collector makes of non-Python files (YAML)
  are not collected.
- Module-level statements that raise for reasons aye can't see (`X = 1 / 0`,
  a missing import) leave a file with readable tests exact: `--co
  --no-refine` lists them. A value aye can't read under an explicit id
  (`pytest.param(make(), id="x")`) is taken as built.
- Session- and package-scoped parameters are predicted file by file, where
  pytest interleaves them across files.
- Of the arguments after `--`, `@file` argument files are not read, and
  positional paths select nothing.
- A `pytest_plugins` in a non-top-level conftest that aye can't be sure of
  is left to the front-end, and `--co --no-refine` lists the directory's
  tests.
- Positional paths in `addopts` aren't read: `addopts = "tests"` makes
  pytest collect only `tests`, where aye collects as if they weren't there.
  With `--pyargs` and `testpaths`, a session's start-up conftests are those
  of the packages' directories, where pytest's are the current
  directory's.
- With dotted names or keys, the recognizer (which doesn't see them) and
  refinement (which runs before they are resolved) take pytest's default
  start: a non-top-level `pytest_plugins` in a conftest outside it is
  reported statically, as if no path were given.
- Items a `pytest_collection_modifyitems` hook makes through a function of
  another module, or copies of items (`copy.copy(item)`), don't make the
  static cases partial.
- The refinement cache can't see what an import read: a file that imports a
  project module is always refined, even when that module didn't change.
  Conftests outside the file's directory chain, installed plugins that act
  without an import, the content of plugins `PYTEST_PLUGINS` names, and a
  module named in a string that only an editable install's import hook
  maps (under a name that is neither its distribution's nor in its
  `top_level.txt`) are not in the key. A result with no case isn't cached,
  so a test file pytest finds no test in is imported on every run. Entries
  are never pruned; delete `.aye/cache` to clear them.

## Static collection (unittest)

The recognizer follows `python -m unittest discover` on Python 3.13
(`Lib/unittest/loader.py`); 3.10 to 3.12 load the same tests, except that
before 3.11 an imported `unittest.FunctionTestCase` makes a test of its own.
Discovery starts where `[tool.aye.unittest]` says (see
[compatibility.md](compatibility.md#unittest)) and takes files whose names
are valid module names and match the pattern, entering only packages. Ids
are `path::Class::method`, `Class` being the name the loader finds the
class under in the module (its own, an alias's, a factory's), as pytest
names `TestCase` methods, so the two adapters give a test one id.
Affinity is the file, then `file::Class`: the module's and the class's
lifetimes; below a package that may define `load_tests`, `package:DIR` leads,
so that one batch loads the package once.

- **Exact**: classes, defined or imported from the project's files, whose
  MRO (C3, across files) reaches `TestCase` or `IsolatedAsyncioTestCase`,
  and their callable `test*` attributes, inherited ones included, read
  statement by statement as Python runs the module (a module-level `del`
  removes a class); decorators known to keep a method callable (unittest's
  skips and `expectedFailure`, `mock.patch`, pytest marks,
  `functools.wraps`, local ones that visibly return their argument). A
  class from an installed package holds no tests unless it is named like a
  test class.
- **Refined**: a module that may define `load_tests`, star-imports a module
  outside the standard library, defines `__getattr__` or `__dir__`, may
  raise while imported, changes namespaces or classes at import (`exec`,
  `globals()`, `setattr`, `base.Base.test_x = f`, a class passed to a call,
  in it or in the project modules it imports), or binds a possible test
  class in a branch or from a call; classes with a decorator or metaclass
  aye can't see through, an `__init_subclass__` in a base, or bases it
  can't resolve (Django's `TestCase` among them); `test*` attributes that
  are conditional or come from outside the project; and a claimed module
  with no test aye can see, which discovery imports all the same. Below a
  package whose `__init__.py` may define `load_tests`, every file is
  refined, from `TestLoader.discover` on that package.
- **Partial**: the static guess for a file that needs refining, when
  refinement is off or fails.

## Static collection (doctest)

The recognizer follows pytest 9.1.1's doctest plugin (`_pytest/doctest.py`),
the referee for this adapter. Modules are collected when `--doctest-modules`
is among pytest's options or `[tool.aye.doctest] modules = true`: every
`.py` file the walk reaches (test modules and conftests too), except
`__main__.py` and a `setup.py` that mentions setuptools or distutils. Text
files match `[tool.aye.doctest] glob`, else pytest's `--doctest-glob`s,
else `test*.txt` when a pytest configuration exists or pytest runs too; a
`.txt` or `.rst` file named on the command line is a doctest whatever the
globs. A module gives one case per docstring with examples, named as
`DocTestFinder` names it under the module's name in pytest's default import
mode (`pkg/mod.py::pkg.mod.Class.method`); a text file with examples is one
case (`docs/guide.txt::guide.txt`). A module's affinity is `import:NAME`,
then the file, as for pytest's test modules; a text file's is the file.

- **Exact**: docstrings parsed as `DocTestParser` parses them (as written,
  and as the 3.13 compiler cleans them), found as `DocTestFinder` walks the
  module's and its classes' namespaces (functions, classes, methods, static
  and class methods, properties), through the decorators that keep them
  (`staticmethod`, `classmethod`, `property`, `abstractmethod`,
  `dataclass`, …); text files, read as UTF-8.
- **Refined**: wherever the source can't say what the namespaces hold once
  imported, or what doctest makes of a docstring: definitions with doctests
  that may not run, names bound twice or after a star import, other
  decorators, aliases and wrappers of objects with doctests, namespace
  changes at import (`setattr`, `globals()`, `__test__`, `__doc__` and
  `__module__` assignments, …), metaclasses other than `ABCMeta` and
  `EnumMeta`, a `raise` that always runs, docstrings doctest can't parse,
  the `importlib` import mode, a module that may skip itself while
  imported, a module or text file with doctests below a conftest that may
  skip while pytest loads it (not at start-up; pytest then collects no
  doctest below it), a text file that isn't UTF-8; and a module with no
  doctest aye can see, since pytest imports every module and its import
  error is a collection error. Not refined: a module holding only constants
  and, beside the pytest adapter, a conftest or a test module, whose import
  that adapter reports.
- **Partial**: a refined file's static cases; also every case of a file
  whose doctest items a conftest hook (`pytest_collection_modifyitems`,
  `pytest_itemcollected`) or a plugin aye can't read may mark, so that `-k`
  and `-m` keep them.

The adapter reads pytest's configuration itself, approximately: the first
file holding pytest settings above the paths given, `-c`, `--rootdir` and
`-o`, then `addopts`, `testpaths`, `norecursedirs`, `pythonpath`,
`doctest_optionflags`, `doctest_encoding`, `filterwarnings` and the `junit_*`
settings, and from the options `--doctest-modules`, `--doctest-glob`,
`--doctest-report`, `--doctest-continue-on-failure`,
`--doctest-ignore-import-errors`, `--ignore`, `--ignore-glob`, `--deselect`,
`-W`, `--noconftest`, `--confcutdir`, `--import-mode`, `-x`/`--maxfail` and
the JUnit options. When it runs, the pytest adapter lifts those five doctest
options out of what its front-end parses. Not read: conftests' `collect_ignore`,
what conftests' hooks and plugins do (beyond making cases partial), and
pytest's `-k` and `-m` from `addopts` or `PYTEST_ADDOPTS`.

## Workers and scheduling

- **Interpreter.** The first of `--python`, `$AYE_PYTHON`, `[tool.aye]
  python`, `$VIRTUAL_ENV`, a `.venv` in the current directory then the root,
  the interpreter beside the `aye` binary (beside the file a link to it
  leads to: `uv tool install` and pipx link it into `~/.local/bin` from the
  tool's environment), then `python3` or `python` on `PATH`.
- **Protocol.** JSON lines over the worker's stdin and stdout; see
  [protocol.md](protocol.md). The worker keeps the original fds for the
  protocol (close-on-exec, placed high) and points fd 1 at stderr, so stray
  prints can't corrupt it.
- **Batches.** Cases are grouped by their outermost affinity key, so a
  module's and a class's fixtures are set up once per group. The pytest
  front-end gives a case a key only for a costly lifetime it shares:
  `module:<path>` when module setup runs user code for it (a module- or
  package-scoped fixture in its closure, `setup_module`), then
  `class:<path>::<Class>` when class setup does; a file pytest imports
  under one name as another (its "import file mismatch") keeps
  `import:NAME` first, so the clash is reported as pytest reports it. A
  case without a key is free: with several workers, a big file's free
  cases are split across workers. A group's cases run together, at the
  place of its first case, even when the arguments interleave them. A
  batch holds one adapter's cases. With one worker, each adapter's cases
  form one batch, run in plan order, each on a worker of its own (the one
  that listed pytest's session waits while another adapter's runs, rather
  than listing it again). With
  several, groups go longest-first by past durations (10 ms for a test
  with no history), and groups of one adapter lighter than a quarter of a
  worker's share are merged (for the pytest adapter, only consecutive
  ones, handed out in plan order, as pytest-xdist's workers keep it). A
  batch holding a `serial` case never runs alongside another. With
  `--ff`, last run's failures run first within their batch, across files,
  and their batches are handed out first. `aye-runner`'s crate docs have
  the details.
- **Lanes.** A run starts at most `workers` workers, never more than there
  are batches. Each pulls the next batch when it finishes one, which balances
  like LPT scheduling and adapts when history was wrong. A worker's index is
  its lane's, kept by the worker that replaces a crashed one, as are
  `AYE_WORKER` and, with more than one lane, `PYTEST_XDIST_WORKER` and
  `PYTEST_XDIST_WORKER_COUNT`. Every worker gets the run's id as
  `PYTEST_XDIST_TESTRUNUID`. The worker that refined the pytest adapter's
  files is the first lane; every other lane lists the session (digests
  only) before its first batch, and a file it lists otherwise than the
  first lane runs there only, with a `listing-differs` warning.
- **The pytest executor** is aye's engine with the pytest front-end (see
  [aye's engine and the pytest front-end](#ayes-engine-and-the-pytest-front-end)),
  standard library only. One session per worker, as a pytest-xdist worker
  has: the front-end lists the session's files once (imported once, the
  `pytest_collection_modifyitems` hooks run over what it lists) and keeps
  the model, and one engine runs every batch, its session- and
  package-scoped resources alive from batch to batch until the session
  ends, in a batch, as a pytest-xdist worker's ends with its last test:
  the batch the runner hands out when nothing else of the adapter is left
  for the lane ends it in its last test's teardown (so a failed teardown
  is that test's error, as in pytest), and a lane that finds nothing left
  only afterwards sends one with no tests to end it. `pytest_sessionfinish`
  and `pytest_unconfigure` run there, and a `session.exitstatus` set to 1
  fails the batch. A worker whose session ended runs no other batch. A
  batch keeps its ids among the listed cases:
  an id without parameters selects them all, a parametrized id the
  front-end no longer makes stands for the parameters no other requested
  id claims, unless the user typed it (`requested_ids`), and tests that
  ran before a crash (`ran_ids`) don't run again. Requested ids in a
  module or directory pytest skips finish `skipped`; those it can't find
  are `not-found`; those in a file that failed to collect are its
  `collection-error`'s. The session starts as the user's run does: its
  start-up conftests and initial paths are those of the user's paths (the
  context's `roots`; `selected_paths` when the user also selected by
  dotted name or key). The batch's status is pytest's: 1 when a test
  failed or one of its files failed to collect, 5 when no test ran, 4
  after a usage error, 3 after a hook failed outside a test. A failed
  test's rerun is `aye <aye's options> <id> -- <the arguments after -->`:
  the options that shaped the run (`-c FILE`, the context's `rerun_args`),
  and no `--junitxml`. Assertions are rewritten as pytest rewrites them, for
  the modules pytest rewrites, and a failed comparison carries pytest's
  message and the engine's explanation. `PYTEST_VERSION` is set while the
  executor lists, runs and closes, as pytest sets it.
- **The unittest executor** is native and imports only the standard
  library, so it runs without pytest. Each test runs through `test(result)`
  with aye's own `TestResult`, whose callbacks fold into one outcome, and
  class and module fixtures (`setUpClass`, `setUpModule`, their teardowns
  and cleanups) are lifetimes, opened and closed as `TestSuite.run` would
  for the batch's order. `subTest` blocks are sub-outcomes. Output is
  captured per test, as `-b` does, and a fixture's is kept for its failure.
  A batch loads each module (or a package that may define `load_tests`)
  once, then takes each requested id's tests; a test both a package's
  `load_tests` and a file named on the command line load runs once, under
  the file's id. Its status is `python -m unittest`'s: 0, 1 when a test
  failed, a module didn't load or a class or module fixture failed, 2 when
  stopped, 5 when nothing ran.
- **The doctest executor** is native too, standard library only (with
  a `pytest` module at hand, aye's in every worker, it uses `import_path`
  for the `importlib` import mode and imports the conftests pytest would
  have imported before
  each file, whose side effects reach the doctests: one that skips while
  loaded skips the files below it, one `skipped` info on its directory in
  refinement, and one that fails otherwise fails them, with one diagnostic
  on its path, worded as the pytest adapter words it (`Skipped`, `Failed`
  or `ImportError while loading conftest`); a `usage-error` for one pytest
  loads at start-up, else a `collection-error`. The start-up ones are those
  of the paths the user's run starts from: the context's `initial`, or,
  when the user also selected by dotted name or key, those pytest would
  load for `selected_paths`. The binary keeps one error per conftest: the
  same code on the same `conftest.py` from a second adapter, or from a
  batch after refinement reported it, becomes an info saying so). It imports each file as pytest's default
  import mode does, every file of the batch before any test runs, under
  pytest's warning filters, and runs each docstring with a `DocTestRunner`
  and pytest's output checker (`ALLOW_UNICODE`, `ALLOW_BYTES`, `NUMBER`) in
  a copy of the module's globals. Each example
  is a sub-outcome named `line N: >>> source`; a mismatch carries `got ==
  expected` and their diff. What pytest fails to collect (an import error,
  a docstring doctest can't parse) is a `collection-error`.
- **`maxfail`** counts tests with a bad outcome, across workers: the last
  `-x` or `--maxfail` after `--`, else the command line's (the last one
  given), else the framework's own (pytest's `addopts` or `PYTEST_ADDOPTS`,
  from the pytest or doctest adapter's context); 0 is no limit. Each batch
  gets what is left of the budget. When it runs out, nothing more is handed
  out, and busy workers end their batches as pytest's `-x` ends a session,
  tearing down: a worker whose batch reached its own share stops by itself,
  and the others are interrupted (SIGINT, a `KeyboardInterrupt`; killed on
  Windows) and killed if still busy 10 seconds later. Records after the bad
  outcome that reaches the limit are dropped, so the log holds exactly
  `maxfail` bad outcomes, plus the `lifetime_finished` records that close
  the lifetimes it holds open. With `maxfail`, the unittest adapter reports
  a failed lifetime before the tests it blocks, so the log keeps it.
- **Crashes.** A worker that dies mid-batch fails the test it was running
  (`error`, with the worker's stderr: a fatal error's dump from its start,
  else its last lines). The batch's tests that never started are re-queued
  once. An id that stands for several tests (a case without its parameters,
  pytest's stand-ins for parameters whose ids changed) is re-queued whole,
  and the batch's context names the tests that ran (`ran_ids`), which the
  adapters don't run again; the unittest and doctest adapters count them,
  so an id listed n times skips its first n tests. An id one batch runs
  more than once (a `load_tests` that adds a test again), one of whose
  copies crashed the worker, is not re-queued: its later copies don't run.
  A worker that dies outside any test is an internal error.
- **Stopping.** Before the run, Ctrl-C, SIGTERM or SIGHUP stops
  collection: the walk, parsing and refinement (its worker killed) give up
  within moments, `run_finished` says `interrupted`, exit 2; anything that
  isn't watching is cut short after a second and a half, exit 2 all the
  same. During the run, it stops the run: no more batches,
  running workers killed, exit 2. A second one kills every worker at once
  and exits; one within half a second of the first is that one delivered
  again (GNU `timeout` signals aye, then its process group), not a second
  request. Killing a worker sends SIGKILL to its process group, so the
  processes its tests started go too, but fixture teardown and `finally`
  blocks don't run, and the test in flight gets no `test_finished`. The
  framework can stop the run as well: the pytest adapter emits a
  `batch-stopped` error whenever the suite stops pytest's session
  (`pytest.exit` whatever its `returncode`, a `KeyboardInterrupt`,
  `session.shouldstop`, a `session.shouldfail` that isn't aye's
  `maxfail`), even in the batch's last test, so the outcome doesn't depend
  on the worker count, and the unittest
  and doctest adapters emit one for a `KeyboardInterrupt` (unittest's also
  for a test calling `result.stop()`). That error, or a batch status of 2,
  ends the hand-out, lets running batches finish, and the run exits 2; one
  stopped by pytest's internal error exits 3. A worker whose runner was
  killed outright kills its own process group at its next message.

## Output and history

Renderers are folds over the log and see nothing else.

There are two formats, `--output text` (the default) and `--output json`
(JSON lines), else `[tool.aye] output`, and one scale of detail for both:
the verbosity, the number of `-v` less the number of `-q`, clamped to -1..3
(`-qq` is `-q`, `-vvvvv` is `-vvv`). The names `human` and `agent`, used
until October 2026 and never published, are usage errors (exit 4) that say
what to use instead: `text`, and `json -v`. `aye config` shows the format
at its verbosity (`json -v`) and where the format came from.

One ladder of detail serves both formats, runs and `aye show` alike. A bad
outcome (a failed or errored test, a failed sub-outcome, a failed
lifetime) shows at one of three levels:

- **L1, the line**: its key, id and outcome, where it failed, and why, in
  one line (`cart.summary()['pear']: expected 3, actual 2`, `fixture db:
  ConnectionError: refused`, `passed; fixture tmp failed in teardown: …`).
  A failure whose signature an earlier one had names it (`same as 3fae6`).
- **L2, the detail**: the failing line and the two before it, the failing
  expression marked; the structured diff, labelled expected and actual
  (the dict key, the field, the position, where two strings part), or the
  exception's message; the hint (`did you mean 'cart'?`), the fixture that
  broke, the setup chain, the test's history, the rerun command.
- **L3, everything**: every frame (others than the project's collapsed per
  package, `12 frames in requests`), the captured output's tails and the
  innermost frame's locals.

| verbosity | `--output text` | `--output json` |
|---|---|---|
| `-q` | the summary line, and what never hides (error diagnostics, failed workers, why the run stopped) | the `summary` |
| (none) | a header, each bad outcome at L1 as it finishes, warnings and skips grouped, the summary, `details  aye show <keys>` | the `summary`, with the keys of what failed |
| `-v` | bad outcomes at L2 | a `fail` line per bad outcome at L1, collection errors, failed workers |
| `-vv` | bad outcomes at L3, a line per other result, warnings in full | bad outcomes at L2, warnings and info diagnostics |
| `-vvv` | what the engine did: workers, batches, lifetimes | bad outcomes at L3 |

`aye show` takes the same flags with the same meaning; without any, its
details are at L2 (text `-v`, JSON `-vv`). The run's log itself is `aye
show --log`. The text format is specified in
`crates/aye-render/src/text.rs`, and the JSON lines, their fields and their
limits in `crates/aye-render/src/json.rs`.

- **JUnit XML**, written when the run ends: to `--junitxml`, else to the
  path pytest's own `--junitxml` gives in `addopts`, `PYTEST_ADDOPTS` or
  after `--`, with pytest's `--junitprefix`, and as pytest's `junit_*`
  settings in the adapter's context say (the suite's name, which output a
  test case carries and under which headers, whether a passing test
  carries any, whether `time` counts the body alone, and whether test
  cases have `file` and `line`: xunit1, or xunit2, pytest's default);
  without them, aye's own layout (the suite `aye`, `file` and `line`,
  stdout and the log in `<system-out>`, stderr in `<system-err>`). Test
  cases carry their `properties`, and the suite the run's (each pair
  once). Each test a failed lifetime kept from running gets an `<error>`
  naming it; a failed lifetime that blocked no test (a failed teardown) is
  a test case of its own. A doctest is one test case with a `<failure>` per
  failing example. The path is checked before anything runs.
- **GitHub annotations**, added to text output, at every verbosity, when
  `GITHUB_ACTIONS=true`, with paths relative to the repository
  (`GITHUB_WORKSPACE`, else the directory holding `.git`).

With `-s`, workers' output is echoed to stdout beside text output, and to
stderr beside JSON lines, so those stay whole.

History is one more consumer. `.aye/history.json` keeps per-test durations
(a moving average, the maximum, the count and the last), outcomes and the
last-failed set that `--lf` and `--ff` read. A test that finishes `failed`
or `error` joins the set, and one that finishes any other way leaves it, as
in pytest; every skip leaves it, whatever its reason. A test that did not
run changes nothing, outcome and durations included: one with no
`test_finished`, and one that finished neither `failed` nor `error` without
a `test_started` in its run. A failure the runner reports for a test that
never started (a crashed worker's), or one a failed lifetime kept from
running (`cause`), joins the set but adds no duration. Lifetimes aren't
recorded, so a failed teardown that blocked no test puts no test in the set
(pytest keeps its class's last test there). History knows tests by id,
whatever adapter ran them, so `--lf` and `--ff` work across adapters.
Collect-only runs, and runs where nothing ran, leave the file alone. `.aye/`
is created with a `.gitignore` and a `CACHEDIR.TAG`. Each test also keeps
`recent`, its last 20 outcomes, a letter per run (`p f e s x u`), and
`bad_since`, the number of the run its current streak of bad outcomes
began in; both are optional in the file, whose version stays 1. Before
any reporter or the run store sees a bad outcome, the binary sets its
`history` from them as they were before the run (`History::previous`),
when the test has any, so a renderer can say `passed the previous 12
runs` or `failed the previous 3 runs too (since r78)` from the event
alone.

**Keys.** The renderers name each test and lifetime by a short key, the
start of its id's hash (FNV-1a 64, mixed), 5 hex digits, longer only when an earlier
id starts the same way ([events.md](events.md#keys)). Keys are a fold over
the log alone (`aye_render::Keys`), and every selected case is collected
before any result, so every renderer, at every level, and `aye show`
later, give a test the same key, and `aye show 3fae6` shows it again.

**The run store.** Every run with something to show keeps its whole log,
every record unchanged, in `.aye/runs/r<N>.jsonl.gz` (gzip, miniz_oxide's
fastest level). Runs are numbered: before its `run_started` (whose
`run_number` says it) a run that will be stored claims 1 + the highest
number of the runs there and `last`, by creating its log with
`create_new`, and the next number while another run has it, so two runs
at once never share one; a run with nothing to show deletes its log and
frees its number. Runs stored under hex ids before October 2026 take no
part in numbering, and are pruned like any other. A run
has something to show when a test finished, a lifetime or worker failed,
or an error diagnostic arrived that isn't the command line's (a path or
name that doesn't exist: `not-found` before any batch, `unclaimed`); a
collect-only run and one that ran nothing else (a typo in a path, a `-k`
that matches nothing, a collection interrupted) leave no trace, so `last`
stays the run before them and retention keeps the runs that did run. The
store (`aye-history`'s `runs.rs`) is one more consumer of the stream, the
last to get each record: it serializes the record where it arrives and
hands the serialized log, in chunks of 256 KiB, to a thread of its own that
compresses it and reads the files' digests, so the run never waits on
either. (Moving the records themselves to that thread cost more: freeing
them there slowed the run.) On `packaging`'s suite, 62,000 tests and 57 MB
of records a run, the log is 3.5 MB, the store's work is about 0.13 s of
CPU (serializing on the run's thread, compressing on its own), and the
run's time is the same within the noise of repeated runs (7.2 to 7.6 s,
either way). When the stream ends it writes the run's summary,
`r<N>.json` (when it started and finished by the store's clock, its
exit code, counts and duration; what its summary counts beside the tests'
counts, modules skipped while collecting, failed lifetimes that kept no
test from running and error diagnostics, which the binary folds with
`aye_render::Tally` as the summaries do and hands over, so `aye show
--runs` words a run as its summary did; and the digest, FNV-1a 128, of
each file holding a test it collected or that collection reported an
error on or skipped, read when collection finished), then
`.aye/runs/last`, the id of the run that finished last; each is written to
a temporary file and renamed over, so a reader sees the old one or the new
one. A run is stored once its summary exists: a log still being written,
or one an aye that was killed left, is never shown. Two runs at once each
write their own files, and the one that renames `last` last is `last`;
the store lists it first, whatever their finishing times say, and when the
run `last` names is gone (another aye's retention, a hand), the newest
stored run stands for it.
Retention keeps the `[tool.aye] keep-runs` runs that finished last (10 by
default; a whole number, 0 or more) and deletes the others when a run is
stored, and what runs that never completed left once it is a day old;
`keep-runs = 0` keeps none, deletes those stored, and `aye show` says so.
A store that can't be written is a warning; it never fails a run.

**`aye show`** (a subcommand, detected as `aye config` is: unless a path
named `show` exists) reads the store and never runs anything; it needs no
interpreter and prepares no adapter. It feeds the same renderers (text or
JSON lines) part of a stored log (`aye_render::ShowMode`), with the whole
log's keys and what it knows beside the log (`aye_render::ShowContext`: the
run's number, how long ago it finished, which files shown changed since).
The flags mean what they mean in a run; without `-v` or `-q`, details come
at the second level of detail (text's `-v`, JSON's `-vv`) and the list at
a run's default. `--log` writes the stored log itself, a record per line,
whatever the format (`--run` picks another run's).

- with no test named, the last run's **list**: a line per result, then
  the summary. A test's is text's `-v` line (a bad outcome's with its
  key and why; `-vv` adds the durations) or JSON's `test` line;
  what the run's summary counts beside the tests has one too: a failed
  lifetime that kept no test from running (`error   teardown of
  tests/t.py::T — message`, JSON's compact `lifetime_fail`), a module
  skipped while collecting (`skipped tests/t.py — reason`, a JSON `test`
  line named after the file), and an error diagnostic, as the run shows
  it, like a failed worker. So the summary is the run's own, counts and
  all, when nothing filters the list, and otherwise one counting what is
  listed. `--outcome O[,O]`, `-k` and `-m` (on the log's
  `case_collected` records; what has no name, as a failed worker, matches
  neither) filter it, and so do paths given alone (as ids they name a
  file's tests, lifetimes and diagnostics). `-q`: the summary, with the
  error diagnostics, as a run's `-q`.
- with keys (`aye show 3fae6`, or 4 digits or more of one), test ids or
  dotted names (and paths, which then select their results), or
  `--failed` (every failed or errored test, failed lifetime, error
  diagnostic and failed worker), their **details**, as a run of those
  tests would show them: their `test_started`, `sub_outcome` and
  `test_finished` records, the failed `lifetime_finished` of the
  lifetimes that kept them from running; a failed lifetime's every
  failure (`aye show tests/t.py::T` gives the class's tests and its failed
  `tearDownClass`); a file's diagnostics; the run's `run_started` first
  and a `run_finished` counting them last. A key is its test's every
  result (a duplicate id's too), or its lifetime; the start of several
  keys is a usage error that lists them, with their ids. The filters apply
  here too.

A `run_finished` `aye show` makes (a filtered list, details) carries the
stored run's exit code, so the summary's `exit` (and text's verdict, such
as `interrupted`) is the run's whatever is shown of it, as in the whole
list; text says `no tests shown` when nothing is.
- `--run r81` (or `81`) picks an older stored run, and the start of a
  hex id one stored before runs were numbered (one no run is, or several
  runs' ids start with, is a usage error); `--runs` lists the stored runs,
  the last first: `r81`, start time (UTC), exit code and the counts its
  summary had.

Before anything else, it says which files of the tests it shows changed
since the run (their digest differs) or are gone: a warning line in text
(`tests/x.py changed since this run; rerun: aye tests/x.py`), a `stale`
line in JSON (`{"type":"stale","path":…,"rerun":…}`, `"gone":true` for a
file that is gone; none at `-q`). A digit-only argument that names no file
is a key's start like any other hex. It exits 0 when it could show what
was asked, whatever the outcomes, 4 for what it can't (a name no test of
the run has, listing the closest names without their parameters, by edit
distance to the whole name or to its last name, a file's name counting as
one; the start of several keys; a bad expression or option; a run that
isn't stored), and 5 when no run is stored.

## aye's engine and the pytest front-end

Every test runs on aye's engine (Core decision 4). For pytest suites,
pytest is never imported: aye runs them with an interpreter that has no
pytest, and where one is installed, aye's `pytest` module wins over it.
Inside the worker the layers are:

- **aye's `pytest` module** (`aye-adapter-pytest/python/native/aye_pytest_shim.py`,
  installed as `pytest`, with the `_pytest.*` modules suites import from,
  before any test or conftest is imported, so it shadows a real pytest):
  the authoring API as data (marks, fixtures, `param`, `raises`, `warns`,
  `approx`, `MonkeyPatch`, the outcome exceptions and their helpers, the
  types for annotations, `hookimpl`); see
  [compatibility.md](compatibility.md#ayes-pytest-module);
- **the pytest front-end** (`aye_pytest_frontend.py`): does what pytest's
  collection does (configuration and options, conftests, discovery,
  fixtures and their visibility, closures, parametrize and ids, marks, the
  supported conftest hooks) and produces the model's objects; it never
  runs a test;
- **the model** (`aye-engine/python/aye_model.py`) and **the engine**
  (`aye_native_engine.py`, with `aye_rewrite.py` rewriting asserts, its
  output cached as pytest caches its own, in `__pycache__` as
  `<name>.<cache tag>-aye.pyc`, keyed by the source's digest and the
  rewriter's): runs cases, resources of every scope with their teardown,
  conditions and outcomes, knowing no framework;
- **the glue** (`aye_pytest_native.py`): the executor, `refine` and `run`
  (see [The pytest executor](#workers-and-scheduling)).

The pytest adapter's recognizer claims pytest's files with the static
recognizer's rules, but refines every test file, with no static case: the
front-end is the single truth for ids (`aye --co --no-refine` lists the
static prediction instead). Its context carries a `native` section (the
resolved settings, the raw ini values for `getini`, pytest's options as
the front-end parses them, and those options parsed with pytest's dest
names; see `aye-adapter-pytest`'s `native.rs`). The other adapters' workers
get aye's `pytest` module too (the doctest adapter imports conftests),
`aye config` says `engine   native`, and static `-k` and `-m` are the last
word on every case. What aye can't run fails with a clear message, never a
wrong result: pytest's own doctests (a run without the doctest adapter)
are a `usage-error`, so are conftest hooks the front-end doesn't support
and third-party plugins a conftest, `-p` or `PYTEST_PLUGINS` names; an
installed plugin pytest would load by itself is a warning that names it
(once per run, from refinement), and a failed refinement is an error.
`--native`, the prototype's switch, is accepted and does nothing.

## Roadmap

The work is tracked as GitHub milestones.

- **Foundation**: event schema v1, the engine contract, the worker transport
  and single spawn path, the comparison harness against pytest, and the
  glossary. Done, except the words still open in `aye-engine` (a fake, a
  snapshot, a data builder).
- **Step 1: Runs pytest suites**: static collection, refinement, execution
  (through real pytest until October 2026, on aye's own engine since),
  selection, config with origins (`aye config`), text
  and JSON output, JUnit XML, GitHub annotations, exit
  codes and discovery lints. Done for pytest suites. Still open: lints for
  test functions in files that are never collected, unknown fixture names
  and parametrize names that don't match the function's arguments, and
  reading Django's configuration.
- **Step 2: Fast**: warm workers kept between runs, forking per batch; test
  history with run diffs; parallel by default (the default is one worker
  today, and `-n auto` gives one per core), with the parallel floor
  reported; live progress. History (durations, outcomes, last failed) and
  duration-weighted batches exist.
- **Step 3: Every suite**: unittest and doctest suites, run natively by
  adapters that map fully onto aye's engine, and Django suites, through
  services the unittest and pytest adapters share. Done for unittest and
  doctest, with the choice of adapters (`[tool.aye] adapters`) and the
  examples in `examples/`. Still open: Django (its settings, test databases
  and `TestCase`), per-example doctest reruns and `--accept`, and running
  the copies left of an id a batch runs more than once after one of them
  crashed its worker (the adapters read `ran_ids` by count; the runner
  doesn't re-queue such an id yet).
- **Step 4: Safe by default**: coverage per test, per tag and on the diff;
  per-test timeouts that show where the test hung; network blocked by
  default; aye decorators for existing tests; honouring the most-installed
  plugins' marks.
- **Step 5: CI at scale**: sharding across machines by duration, the Python
  version matrix in one run, retries and flaky detection, random order with
  the seed reported.
- **Step 6: aye's testing library**: patching that reaches every binding, a
  fake clock, seeded randomness, fake environment variables, network record
  and replay, a virtual filesystem, test data built from types, snapshots
  with a review step, async tests without a plugin, Hypothesis, containers,
  and a pytest plugin so plain pytest runs get aye's features.

Maintenance: the `ruff_python_*` crates are pinned to `=0.0.4`, the last
release supporting Rust 1.94, the MSRV. Bump them together with the MSRV.
