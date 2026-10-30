# Event schema (v1)

A run is its event log. The types are in `crates/aye-events`, and nowhere
else. Every run keeps it ([Stored runs](#stored-runs)), and `aye show
--log` prints it verbatim, one JSON record per line. Every other
output (text, JSON lines at the lower verbosities, JUnit XML, GitHub
annotations, `aye show`) and the run history are folds over it. Workers write the same records over the worker
protocol ([protocol.md](protocol.md)), and the runner merges them into the
run's log.

## Conventions

- A record is an envelope and an event, flattened into one JSON object. The
  event kind is under `"event"`, in `snake_case`.
- Durations are fractional milliseconds (microsecond precision) under `*_ms`
  keys.
- Optional fields are omitted rather than set to `null` or left empty. The
  one exception is a failure's `phase`, which is `null` when no phase is
  known (a worker that crashed). All seven `counts` are always present.
- Paths are relative to the project root, with `/` separators. A path
  outside the root starts with `..` when collection or the binary wrote it,
  and is absolute when a worker did.
- Text is bounded by aye's engine: a failure's `message` is one line of at
  most 1000 characters, `detail` at most 8000, assertion operands 2000 each,
  `diff` 200 lines of 500, `frames` the innermost 30, `reason` 1000, and each
  captured stream its last 20000 characters (with `truncated` set when cut).
  A failure's `exception`, each frame's `source` and `function`, and a
  sub-outcome's `name` are cut to 1000; a worker's diagnostic `message` to
  8000.
- Every record a worker writes is valid JSON: lone surrogates become
  U+FFFD, and NaN, infinities and integers beyond a double's range are left
  out or become `null`. A `duration_ms` that is missing or `null` reads as
  0. Negative durations read as 0, and durations too large to hold are
  capped.
- Consumers must ignore event kinds and fields they don't know.
- `schema_version` is on `run_started`, and is `SCHEMA_VERSION` in the crate.
  New event kinds and new optional fields don't change it. Removing or
  renaming a field, or changing its meaning, bumps it. Adding an outcome is a
  breaking change.

Two records from a real run:

```json
{"run":"e14d5f08d1554fbc","seq":0,"t_ms":0.002,"event":"run_started","schema_version":1,"aye_version":"0.0.1","root":"/tmp/demo","workers":1,"run_number":81}
{"run":"e14d5f08d1554fbc","worker":0,"batch":0,"seq":16,"t_ms":111.546,"event":"test_finished","id":"tests/test_a.py::test_fail","outcome":"failed","duration_ms":8.371,"failures":[{"phase":"test","exception":"AssertionError","message":"assert [1, 2] == [1, 3]","assertion":{"op":"==","left":"[1, 2]","right":"[1, 3]","diff":["At index 1 diff: 2 != 3","Use -v to get more diff"]},"location":{"path":"tests/test_a.py","line":7},"frames":[{"path":"tests/test_a.py","line":7,"function":"test_fail","source":"assert [1, 2] == [1, 3]"}],"signature":"275dc3af7ee3f535","detail":"…"}],"reproduce":{"command":["aye","tests/test_a.py::test_fail"]}}
```

## Envelope

Every field is optional on the wire, so that workers can send partial
envelopes for the runner to complete. Events never repeat an envelope field.

| Field | Set by | Meaning |
|---|---|---|
| `run` | runner | The run's id, on every record. |
| `environment` | runner | The environment (interpreter and dependencies). Not set yet; the Python matrix (Step 5) will set it. |
| `worker` | runner | The worker's index, from 0, on worker and batch records. A worker that replaces a crashed one keeps the index. |
| `batch` | worker or runner | The batch's index, from 0, on batch records. |
| `seq` | runner | The record's position in the run's log, from 0. |
| `t_ms` | runner | Milliseconds since the run started. Workers send their own (since the batch started), and the runner replaces it. |

## Order

```
run_started
collection_started
collection_progress*        only once collection has taken half a second,
                            then at most four a second (so none in a quick
                            run); Ctrl-C here goes straight to
                            collection_finished (nothing selected) and
                            run_finished (stopped: interrupted, exit 2)
diagnostic*                 collection's: syntax errors, missing paths, lints,
                            refinement; then the binary's not-found, for
                            arguments that name nothing
case_collected*             one per selected test
collection_finished
                            usage error, collection error (without
                            --continue-on-collection-errors), nothing
                            selected or --collect-only: straight to
                            run_finished
diagnostic*                 the runner's, before any worker (no-executor)
per worker:
  worker_started            or a worker_finished with `error` if it can't start
  per batch, on that worker:
    batch_started
      lifetime_started / test_started / sub_outcome / test_finished /
      lifetime_finished / run_property / diagnostic
    batch_finished
  worker_finished
run_finished
```

Workers run concurrently, so their records interleave in the log. A worker
runs the batches of one adapter only: a lane whose next batch is another
adapter's finishes its worker and starts a new one, with the same `worker`
index. With one worker, the one that listed pytest's session waits while
another adapter's worker runs a batch, and gets its `worker_started` when
it first runs one, so each worker's records still come between its
`worker_started` and its `worker_finished`. A worker that crashes gets a
`worker_finished` with `error`, and its replacement a new `worker_started`
with the same index.

A worker of the pytest adapter ends its session in a batch (see
[protocol.md](protocol.md#batches-and-refinement)): its last batch, or one
with no tests (`cases` 0) that only ends it.

Within a batch, a lifetime's `lifetime_started` comes before the first test
inside it, and its `lifetime_finished` after the last, unless the lifetime
failed to set up under `maxfail`: then its `lifetime_finished` comes first,
before the tests it kept from running, so that the log keeps it when the
runner drops what follows the bad outcome that reaches the limit. The pair
may not close: a worker that crashes, or that a stop kills, sends no
`lifetime_finished` for the lifetimes it started, so consumers must not
depend on one. A lifetime starts again in each batch that needs it.

## Events

| Event | Fields |
|---|---|
| `run_started` | `schema_version`, `aye_version`, `root` (the project root, absolute), `collect_only` (only when true), `workers` (the most workers the run may start: the count asked for, or 1 when an adapter needs one session), `run_number?` (the run's number in the run store, `81` for `r81`, when it will be stored: not collect-only, `keep-runs` above 0; the envelope's `run` is the run's id) |
| `collection_started` | |
| `collection_progress` | `walked` (files and directories walked so far), `test_files` (files an adapter claimed for tests), `refining?` (files being imported to know their tests; left out while 0). For a live display; text output on a terminal shows it as one line it rewrites and clears |
| `diagnostic` | `severity` (`error`, `warning`, `info`), `code`, `path`, `span?`, `message`; see [Diagnostics](#diagnostics) |
| `case_collected` | a [case](#case_collected); one per selected test, so the log alone knows the inventory |
| `collection_finished` | `selected`, `deselected`, `refined_files` (files whose tests came from importing them, fresh or cached), `duration_ms` |
| `worker_started` | `pid`, `python` (the worker's `sys.executable`), `version?` (the first line of its `sys.version`, `3.13.1 (main, …) […]`; absent in logs written before it was added) |
| `batch_started` | `adapter` (`pytest`, `unittest` or `doctest`), `cases` (how many ids the batch holds) |
| `lifetime_started` | `lifetime`, `kind` (`module`, `class`, …): unittest's `tests/test_a.py` and `tests/test_a.py::A`, named from the class's own module |
| `lifetime_finished` | `lifetime`, `failure?`, `duration_ms` |
| `test_started` | `id` |
| `sub_outcome` | `id`, `name`, `outcome`, `failure?`, `reason?` (why it was skipped or expected to fail: unittest's `skipTest` in a `subTest` block), `duration_ms`: a result inside a test (a pytest subtest, `msg (k=v)`; a `subTest` block, `[msg] (i=1)`; a doctest example, `line 3: >>> f()`). The test still finishes with its own outcome. |
| `test_finished` | a [test result](#test_finished) |
| `run_property` | `name`, `value`: a property of the whole run that a test recorded (pytest's `record_testsuite_property`), when it recorded it. Each worker holds a framework session, so a session-scoped fixture records one once per worker: consumers keep each pair once. The JUnit report writes them on its `<testsuite>`. |
| `batch_finished` | `status?`: the framework's own status for the batch (pytest's exit status; the unittest and doctest adapters' are 0, 1 when a test failed, a file didn't load or, for unittest, a class or module fixture failed, 2 when stopped, and unittest's 5 when nothing ran); absent when the batch crashed or failed |
| `worker_finished` | `exit_status?`, `error?` (set when the worker could not start or died) |
| `run_finished` | `exit_code`, `counts` {`passed`, `failed`, `error`, `skipped`, `expected_failure`, `unexpected_pass`, `deselected`}, `duration_ms`, `stopped?` |

Lifetime events come from the unittest adapter, which maps fully onto aye's
engine: a module's (`setUpModule`, `tearDownModule` and module cleanups) and
a class's (`setUpClass`, `tearDownClass` and class cleanups). A failed setup
is on `lifetime_finished`, with phase `setup`, and each test it kept from
running is `error` with `cause` naming the lifetime; a failed teardown or
cleanup is there too, with phase `teardown`, and fails the batch (status 1)
without failing a test. The doctest adapter has no lifetimes (a docstring is
one test). The pytest adapter attaches a failed fixture's failure to each
test it affects, as pytest does, a session- or package-scoped one's
teardown to the test after which it tears down; it emits only the
`lifetime_finished` of a teardown with no test to blame (a session's
lifetimes, kept open from one batch to the next, that the next batch
doesn't share, or that a batch with no tests ends), which fails the batch
as a unittest teardown does.

`counts` fold `test_finished` records alone. A module or directory skipped
while collected (a `skipped` info) and a failed lifetime that kept no test
from running (a failed teardown or cleanup; a failed setup whose tests the
run stopped before under `maxfail`) count nowhere there. The text and JSON
lines summaries and the JUnit report count each skipped module or directory
as one skipped item, as pytest's summary does, and each such lifetime once,
by the phase that failed: the text summary's `1 setup error` or `1 teardown
error` (`1 lifetime error` when the failure names no phase), the JSON
summary's `setup_error`, `teardown_error` and `lifetime_error`, and a JUnit
test case of its own. A failed lifetime a test names as its `cause` counts
through that test.

`selected` counts the `case_collected` records. `deselected` counts what
`-k`, `-m`, an adapter's `--deselect` and `--lf` drop from the cases the
positional arguments select, as pytest's summary counts it; the expressions
may be the command line's or pytest's own (from `addopts`, `PYTEST_ADDOPTS`
or the arguments after `--`). Tests a conftest hook deselects
(`pytest_collection_modifyitems`) never start and are not counted.

`stopped` says why the run ended before every selected test ran, the first
reason that came up: `maxfail`, `interrupted` (Ctrl-C, SIGTERM or SIGHUP) or
`framework` (the framework stopped a batch: `pytest.exit`, a
`KeyboardInterrupt`, `session.shouldstop` such as `--stepwise`'s, a
`maxfail` set by a conftest or plugin, an internal error, unittest's
`result.stop()`). It is absent when nothing stopped the run.

### `case_collected`

| Field | Notes |
|---|---|
| `id` | The test id, see [Identity](#identity). |
| `adapter` | The adapter that collected it: `pytest`, `unittest` or `doctest`. |
| `span?` | Where the test is defined: `{path, start, end, line?}` (byte offsets, 1-based line). Refined cases carry the line only (`start` and `end` are 0). |
| `tags?` | User labels: pytest's mark names, in pytest's `iter_markers` order. unittest and doctest cases have none. |
| `keywords?` | The names a keyword expression (`-k`) matches: for pytest and doctest cases, those pytest's `-k` matches; for unittest's, the path's parts and the class and method names. |
| `requires?` | Capabilities a backend must provide. Not set by any adapter yet. |
| `affinity?` | Keys of the lifetimes the test runs inside, outermost first. The runner keeps the tests that share the first key in one batch, and may split the tests with none across batches. The pytest adapter's front-end gives only the costly lifetimes a test shares: `module:<path>` when module setup runs user code for it (a module- or package-scoped fixture, `setup_module`), then `class:<path>::<Class>` when class setup does; a file pytest can't import beside another (the name it imports the module under) keeps `import:NAME` first. Its static prediction (`--co --no-refine`) gives `import:NAME`, then the file, then each enclosing class. The unittest adapter gives the file, then `file::Class`, led by `package:DIR` below a package that may define `load_tests`; the doctest adapter `import:NAME` then the file for a module, the file for a text file. |
| `serial?` | Must not run alongside other serial tests (only when true). |
| `timeout?` | Seconds, from the framework's metadata (`@pytest.mark.timeout`). |
| `owner?`, `quarantine?` | Reserved. |
| `certainty` | `exact`, `partial` or `refined`, see [Certainty](#certainty). |

### `test_finished`

| Field | Notes |
|---|---|
| `id` | The test id. A case collected without its parameters finishes as one record per parameter id. |
| `outcome` | See [Outcomes](#outcomes). One per test: the worst of its phases. |
| `duration_ms` | Setup, body and teardown. |
| `failures?` | Every failure, in the order they happened (a failed body, then a failed teardown). |
| `cause?` | The failed lifetime that kept this test from running. |
| `reason?` | Why it was skipped or expected to fail. |
| `output?` | Captured `{stdout?, stderr?, log?, truncated?}`, all phases. |
| `reproduce?` | How to rerun exactly this test: `{command, inputs?}`, an argument vector plus an open map of inputs (seed, frozen time, environment). Every adapter sets it for bad outcomes: `"aye"`, aye's own options that shaped the run (`-c FILE`, as given: the context's `rerun_args`), the id, then `--` and the arguments given after `--` when there are any, without `--junitxml`. From a subdirectory holding a path of the same name as the test's file, the binary writes the id relative to the current directory, so the command reruns the test from where aye ran. |
| `properties?` | `[[name, value], …]`: what the test recorded about itself, in order, as text (pytest's `record_property`, its item's `user_properties`; only the pytest adapter sets them). The JUnit report writes them on the test case. Names are cut to 1000 characters, values to 8000. |
| `body_duration_ms?` | The body alone (the `test` phase, pytest's `call`), when the adapter times it apart (the pytest adapter does): what pytest's `junit_duration_report = call` reports. |
| `resources?` | `[{name, scope}]`: the resources (pytest's fixtures) set up for a test with a bad outcome, in setup order; `scope` is `session`, `package`, `module`, `class` or `function`. |
| `history?` | `{outcomes, since?}`, set by the runner on a bad outcome from the test's history as it was before this run (`.aye/history.json`'s `recent` and `bad_since`), before any reporter or the run store sees it; absent when no earlier run recorded the test: `outcomes`, one letter per earlier run, oldest first, at most 20 (`p` passed, `f` failed, `e` error, `s` skipped, `x` expected failure, `u` unexpected pass); `since`, the number of the run in which the test's current streak of bad outcomes began, when its last outcome was bad and that run had a number. |

### Failures

| Field | Notes |
|---|---|
| `phase` | `setup`, `test` or `teardown`; `null` when unknown. |
| `exception?` | The exception's type name: `AssertionError`. |
| `message` | One line: `assert 4 == 5`, `ValueError: boom`. |
| `assertion?` | `{op, left, right, diff?, expected?, left_source?, right_source?, compare?}` for a failed comparison: the operands' reprs and the framework's explanation lines (pytest's, from `pytest_assertrepr_compare`); `expected`, `left` or `right`, the operand written as a literal or a named constant (absent when neither or both are); each operand's source text; `compare`, how they differ by structure (below). |
| `location?` | `{path, line}` where the exception was raised: the innermost user frame. |
| `frames?` | `[{path, line, function?, source?, cols?, before?, indent?, locals?}]`, user frames only, innermost last. Runner, engine and framework frames are removed by the adapter. `source` is the line without its indentation; `cols`, `[start, end]`, the failing expression's span in it (0-based characters, the end excluded): a failed `assert`'s actual operand when the expected side is known, else its test, on every Python; elsewhere the running instruction's span, from Python 3.11; absent when it would cover all of `source`; on the innermost frame of the project only (under the root, outside installed packages), `before`, the two lines before it, keeping their indentation relative to the least indented of them and the frame's line, and `indent`, how far the frame's line is indented beyond that (absent when 0), and `locals`, `[[name, value], …]`, its local variables as Python shows them (at most 20 sorted by name, 200 characters each; modules, functions, classes and dunders left out). |
| `signature?` | Groups identical failures: a hash of the exception type, the message with numbers blanked, and the location. Renderers group by it; failures without one are never grouped. |
| `detail?` | The framework's own full text (pytest's long representation), for renderers that want it. |
| `resource?` | The resource (pytest's fixture) whose setup or teardown raised it. |
| `hint?` | One line suggesting a fix: the names closest to an unknown resource (`did you mean 'cart'?`, `did you mean 'cart' or 'card'?`, at most three); absent when none is close. |

`compare` is `{kind, summary, same?, differs?, omitted?}`: `kind`, what was
compared (`dict`, `list`, `tuple`, `set`, `str`, `bytes`, `dataclass`,
`namedtuple`, `datetime`, `number`, or a type's name); `summary`, one line
(`dict differs in 1 of 2 keys`); `same`, the entries that are equal;
`differs`, at most 50 `{at?, left?, right?}`, each entry that differs,
`at` written as an access on the value (`['pear']`, `[3]`, `.total`) or,
for text, where it first differs (`line 2, column 5`), and `left` or
`right` absent when only the other operand has the entry; `omitted`, the
differing entries left out. When exactly one entry of two containers
differs and its values are containers of one kind, `compare` describes
them instead, at most five levels down: each `at` then starts with the way
there (`['db']['port']`), and `summary` too (`['db']: dict differs in 1 of
2 keys`). A set's items have no `at`; nor have dates and times, shown in ISO
format; numbers have no `differs`, only `summary` (`differ by 0.01`).

### Diagnostics

| Code | Severity | From |
|---|---|---|
| `syntax-error` | error | collection: a test or support file doesn't parse |
| `not-found` | error | collection: an explicit path doesn't exist; the binary: an id, dotted name or key that matches no test (after collection's diagnostics, before `case_collected`), but a dotted name for a module or package a `skipped` info is about, which pytest's `--pyargs` finds and counts as skipped; each adapter, in a batch: a requested file or id the framework can't find (for pytest, including an id typed with parameters that pytest doesn't make) |
| `unclaimed` | error | collection: no adapter claims an explicit file |
| `unreadable` | error | collection: a claimed file, or a directory an adapter's walk enters, can't be read |
| `duplicate-id` | warning | collection: one adapter makes two cases with one id (aye lists it once, and running it runs every test with it); an id two adapters collect goes to the higher-priority one, with no diagnostic. The pytest and unittest adapters, in a run: the framework ran more than one test with one id, and each result is reported under it |
| `no-tests` | info | collection: a test file has no tests, unless every adapter claiming it claims such files whatever their names (doctest modules), or an adapter already reported about the file (`skipped`, `ignored`, `class-has-init`) or, in refinement, about a directory it is in (a `skipped` directory), which says why. The binary leaves out one on a file below a conftest that collection reported failing to load (an error on a `conftest.py`), whose error says why |
| `not-refined` | info | collection: a file needs importing, but refinement is off (`--no-refine`) |
| `refine-failed` | warning | collection: refinement failed; the static guess stays |
| `class-has-init` | warning | pytest adapter: a `Test*` class with `__init__`, which pytest won't collect |
| `duplicate-definition` | warning | pytest adapter: a test name bound twice in one namespace |
| `collection-error` | error | pytest adapter: pytest failed to collect a file (a generator test included, through refinement) or to load a conftest it meets while collecting (an import error or `pytest.fail`, on the conftest's path; a `pytest_plugins`, on its directory, with pytest's message); statically, a non-top-level conftest (one pytest doesn't load at start-up) that surely defines `pytest_plugins`, on its directory, with pytest's message. unittest adapter: a module that fails to import, or whose `load_tests` raises or fails to load a test (one diagnostic per file, with unittest's messages). doctest adapter: a module that fails to import, a docstring doctest can't parse, a text file that can't be read, a conftest above the file that fails to load (on the conftest's path, in the pytest adapter's words, once per worker, so renderers show one error for both adapters); statically, a `conftest.py` outside a package that pytest would find already imported as `conftest` (its "import file mismatch"). The pytest and unittest adapters' batches report a file that fails to import once each, where pytest counts one error: the binary turns the second into an info saying so. Likewise a `collection-error` or `usage-error` on a `conftest.py`, which pytest loads once per session: when a second adapter reports it (or a batch, after refinement did), the repeat becomes an info, "also reported by the doctest adapter, one usage error as pytest loads a conftest once", with its own words when they differ; one adapter's own repeats (every pytest batch reports a start-up conftest's) stay errors, which renderers show once |
| `skipped` | info | each adapter, in refinement: the framework skipped a whole module while importing it (pytest's `allow_module_level` skip, `importorskip`, `unittest.SkipTest`); in a batch, that module's tests finish `skipped`. The pytest and doctest adapters, in refinement: a conftest skipped its directory while loaded, on the directory's path; in a batch, the directory's requested tests finish `skipped`. When some positional argument isn't a path (a dotted name, a key), collection covers the whole project, and the binary leaves out a `skipped` info the run doesn't reach: no path argument holds it or lies in it, and no dotted name names it, something in it or something holding it |
| `ignored` | info or warning | pytest adapter: pytest ignores a requested file (`collect_ignore`, `--ignore`, `norecursedirs`, `pytest_ignore_collect`); info in refinement, a warning naming the tests not run in a batch. unittest adapter, in refinement: an info for a file below a package whose `load_tests` decides its tests, that ends up with none, saying why |
| `usage-error` | error | pytest adapter: pytest rejected its arguments (`aye -- --bad-flag`, a bad option in `addopts`; an option nothing defines is an `unknown-option` warning instead when pytest would load a plugin aye doesn't, which may define it), a conftest pytest loads at start-up (one on the way to a collection root or a `test*` subdirectory of one) failed to load, skipped or called `pytest.fail` (every batch reports it), a conftest's or plugin's `pytest_configure` raised `pytest.UsageError`, or `--setup-plan` or `--setup-only` came from `addopts` or `PYTEST_ADDOPTS`. doctest adapter: such a conftest failed to load, skipped or called `pytest.fail`, on its path, in the pytest adapter's words (`Skipped`, `Failed` or `ImportError while loading conftest …`). When some positional argument isn't a path, refinement's sessions start from pytest's default, and the binary leaves out such an error on a conftest the run doesn't reach: none of the path arguments holds it, its directory holds none of the paths the run starts from (`selected_paths`), and every argument matched something |
| `unknown-option` | warning | pytest adapter: an option nothing aye loads defines (no builtin, conftest or `-p` plugin) while pytest would load a plugin by itself that aye doesn't (a `pytest11` entry point), which may define it: on the root, naming those plugins and the options, which aye ignores. Without such a plugin, the option is a `usage-error` |
| `plugin-not-loaded` | warning | pytest adapter, in refinement: an installed plugin pytest would load by itself (a `pytest11` entry point) and aye doesn't, on the root, naming it, once per run (batches don't repeat it; `-p no:NAME` silences it); none for the plugins whose job aye does itself or that only change pytest's display ([compatibility](compatibility.md)) |
| `internal-error` | error | pytest adapter: pytest's own internal error, led by the exception raised |
| `batch-stopped` | error or warning | The batch stopped early, naming the tests that never started. pytest adapter: when pytest stopped it (`pytest.exit` whatever its `returncode`, a `KeyboardInterrupt`, `session.shouldstop`, a `session.shouldfail` that isn't aye's `maxfail`), an error naming pytest's exit status, even when every test started; otherwise (an internal error, an option that replaces the run) only when tests never started: a warning when pytest's status was 0, else an error. unittest and doctest adapters: an error, for a `KeyboardInterrupt` (and unittest's `result.stop()`). Not for aye's `maxfail` or a usage error. An error stops the run |
| `protocol-error` | error | runner: a worker line that isn't a protocol message |
| `batch-failed` | error | runner: an adapter raised outside any test |
| `no-executor` | error | runner: no executor for a case's adapter |
| `listing-differs` | warning | runner: a lane lists a file's tests otherwise than the first lane (their order or parameters differ between processes), so they run on the first lane only; once per file |
| `session-failed` | warning | pytest adapter: `pytest_sessionfinish` set the session's exit status to 1 (tests failed) although none of its tests failed: a conftest or plugin failed the session (`session.exitstatus`, pytest-cov's `--cov-fail-under`). It comes from the batch that ends a worker's session, once per worker. The run then exits 1, as pytest does |

When a file's tests come from refinement, its refined diagnostics replace
the recognizer's static errors for that file (its warnings and infos stay),
so pytest's one collection error is reported once.

`--strict` turns every warning collection reports (refinement's included)
into an error. Infos stay infos, and diagnostics from batches keep their
severity.

What an error diagnostic means for the exit code is under
[Exit codes](#exit-codes).

## Outcomes

The set is fixed. A test's outcome is the most severe of its phases.

| Outcome | Severity | Bad | pytest |
|---|---|---|---|
| `passed` | 0 | | passed |
| `skipped` | 1 | | skipped |
| `expected_failure` | 2 | | xfailed |
| `unexpected_pass` | 3 | | xpassed (non-strict) |
| `failed` | 4 | yes | failed, including a strict xpass |
| `error` | 5 | yes | an error in setup or teardown; a test in flight when the session stopped or its worker crashed |

A test that passes its body and then fails in teardown is `error`, with every
failure kept, each with its phase. A failed setup that adapters can see as a
lifetime is reported once, on that lifetime's `lifetime_finished`, and each
test it kept from running is `error` with `cause` set.

## Certainty

How sure collection is that a case exists exactly as described.

| Value | Meaning |
|---|---|
| `exact` | Known from the source alone. |
| `partial` | The test exists, but its parameter ids or its tags are only known at runtime. When its parameters are unknown, the id is the unparametrized one, and handing it to the engine runs every parameter. |
| `refined` | Reported by the framework after importing the file. |

## Identity

- A **test id** is the file path relative to the project root, then
  `::`-separated names, then the parameter id in brackets:
  `tests/test_cart.py::TestCart::test_add[eur]`. It has no line number.
  Parameters are opaque inside the id. aye's project root is the nearest
  directory holding `pyproject.toml`, else `.git`; ids are relative to it
  even when pytest's rootdir is elsewhere.
- A **test key** is the 64-bit FNV-1a hash of the id, mixed with SplitMix64's finalizer so that every digit depends on every byte of the id, as 16 hex digits. Any
  prefix of 4 to 16 digits selects on the command line.
- A **span** is a file and a byte range in it, kept apart from ids: failure
  locations, snapshots and the source rewriter point at spans.

## Keys

The renderers name each test, and each lifetime, by a short key beside its
id: the start of its [test key](#identity), 5 hex digits long, longer only
when needed (`3fae6`). A fold over the log in order
(`aye_render::Keys`): every id, the first time a record names it
(`case_collected`, `test_started`, `sub_outcome`, `test_finished`; a
lifetime's `lifetime_started` or `lifetime_finished`), takes the shortest
start of at least 5 digits that no id before it starts with, and keeps it.
Every selected case is collected before any result, so a run's keys are
fixed before the first is printed, and a stored log gives back the keys
the run printed. A test keeps its key from run to run, unless an id that
starts the same way comes before it.

`aye show K` takes a key, or the start of one: a key names its test's
every result (a `duplicate-id` has several) or its lifetime; otherwise, for
4 to 16 hex digits, the one test or lifetime with a result whose whole key
starts with them. The start of several is a usage error (4) listing each
with its key and id. A string of digits is a key or its start like any
other hex string; one no key starts with is read as any other name (an
id, a dotted name, a path).

## Stored runs

Every run that has something to show keeps its log in `.aye/runs` under
the project root, for `aye show`: a test's result, a failed lifetime or
worker, or an error diagnostic other than the command line's (a path, id
or name that doesn't exist: `not-found` from outside any batch,
`unclaimed`). A run that ran nothing else (a typo in a path, a `-k` that
matches nothing, a collection interrupted) and a collect-only run are not
kept, so `last` stays the run before them.

Runs are numbered, `r1`, `r2`, …: a run takes 1 + the highest number among
the runs in `.aye/runs` (stored, being written or left over) and `last`,
and claims it by creating its log, `r<N>.jsonl.gz`, which no other run may
have created first (else it tries the next). It claims it before its
`run_started`, whose `run_number` says it, when the run will be stored
(not collect-only, `keep-runs` above 0). A run that turns out to have
nothing to show deletes its log, and its number is free again. Runs stored
before October 2026, under their 16 hex digit ids, count for no number;
they are listed and pruned like any other.

- `r<N>.jsonl.gz`: the log, every record as the run's reporters got it, one
  per line, gzip-compressed, written as the records arrive. `aye show --log`
  writes it as stored (`--run` picks another run's).
- `r<N>.json`: written once the log is complete, which makes the run a
  stored one. `version` (1), `run` (`r81`), `root`, `started_ms` and `finished_ms`
  (milliseconds since the Unix epoch, by the store's clock, when it saw
  `run_started` and when the log was complete), `exit_code`, `counts` and
  `ms` (from `run_finished`; `exit_code` absent when the log has none),
  `stopped`, `extra` (what the summaries count beside `counts`, the
  non-zero ones: `skipped_modules`, `setup_error`, `teardown_error`,
  `lifetime_error`, `collection_error`, `usage_error`, `internal_error`;
  absent when none), and `files`: each file holding a collected test (or
  that collection reported an error on, or skipped), relative to the root
  as ids spell it, with the digest of its content when
  `collection_finished` arrived (FNV-1a, 128 bits, 32 hex digits), so that
  `aye show` can say which changed since.
- `last`: the id of the run that finished last (`r81`). When the run it
  names is no longer stored, the newest stored run stands for it.

The summary's and `last`'s files are written to a temporary file and
renamed over. `[tool.aye] keep-runs` (10) says how many runs are kept, the
ones that finished last (and `last`).

`aye show` picks the last run, or the one `--run` names: `r81` or `81`, or
the start of an older run's hex id. It gives the details of bad outcomes
at the second level of detail unless `-v` or `-q` says otherwise (text's
`-v`, JSON's `-vv`), and tells the renderers which run they come from, how
long ago it finished, and which of their files changed since
(`aye_render::ShowContext`). It writes two more kinds of JSON line beside those of
`crates/aye-render/src/json.rs`: `stale` (`path`, and `rerun`, the command
that runs the file's tests again, or `gone`: true), first, for each file of
a result shown that changed since the run; and, for `aye show --runs`, `run`
(`id`, `r81`, `started` in UTC as RFC 3339, `exit`, the non-zero counts as the
summary names them, then `collection_error`, `usage_error` and
`internal_error`, `ms`, `stopped`, and `last` when it is the last run).
Its list adds a `test` line per module skipped while collecting (`id` the
file's path, `outcome` `skipped`, `reason`) and a compact `lifetime_fail`
line per failed lifetime that kept no test from running. Its `summary`'s
`exit` is the stored run's exit code, whatever part of the run it shows;
the process exits 0 when it could show what was asked.

## Exit codes

`run_finished.exit_code` is the process's exit code, with one exception: a
JUnit report that can't be written when the run ends (it was writable when
the run started) makes the process exit 1 where `run_finished` said 0 or 5.
The values are pytest's.

| Code | Meaning |
|---|---|
| 0 | Every selected test passed, was skipped, or was an expected failure or an unexpected pass. |
| 1 | Some test failed or errored; or, under `--continue-on-collection-errors`, an error diagnostic arrived, whether collection or a batch reported it (`--collect-only` included); or the framework failed a batch none of whose tests failed (a `session-failed` warning: a conftest's `session.exitstatus`, pytest-cov's `--cov-fail-under`; a unittest `tearDownClass`, `tearDownModule` or cleanup that failed). |
| 2 | Interrupted: Ctrl-C, SIGTERM or SIGHUP; the framework stopping a batch (a `batch-stopped` error, for `pytest.exit` whatever its `returncode`, a `KeyboardInterrupt`, `session.shouldstop`, a `maxfail` set by a conftest or plugin, or unittest's `result.stop()`; or a batch status of 2), whatever the worker count; or, without `--continue-on-collection-errors`, collection errors. Errors found while collecting (statically or by refinement) stop the run before anything runs. Error diagnostics that arrive during the run (a file the framework failed to collect, a requested id it can't find) let the other batches finish, then the run exits 2. With `--collect-only`, any collection error (1 with `--continue-on-collection-errors`). |
| 3 | Internal error: a worker that could not start or died outside any test, an adapter that raised outside any test, a protocol error, a case no executor handles, pytest's own internal error (`internal-error`, or a batch status of 3), including a hook that fails while aye's pytest front-end collects, which ends the run before anything runs. |
| 4 | Usage error: bad flags (including arguments after `--` that replace the test run or run no test, such as `--fixtures` or `--setup-plan`), configuration (`[tool.aye]`, an unknown adapter, `[tool.aye.unittest]`, `[tool.aye.doctest]`, a doctest option flag pytest doesn't accept) or expressions, no interpreter found, a `--junitxml` path that can't be written, an explicit path that doesn't exist, an id, dotted name or key that matches nothing, an explicit file no adapter collects (`unclaimed`), or a `usage-error` from refinement or any batch, `--collect-only` included: arguments pytest rejects, a conftest pytest loads at start-up that fails to load or skips, `pytest.UsageError` from `pytest_configure`, `--setup-plan` or `--setup-only` in `addopts` or `PYTEST_ADDOPTS`. |
| 5 | Nothing was collected or selected, including when the only arguments are files or directories without tests. |

When several apply during a run, an interruption decides first, then an
internal error, then a usage error. A batch the framework stopped with an
internal error exits 3, as pytest does.
