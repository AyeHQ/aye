# Compatibility

aye runs the tests of three frameworks, each through its adapter
([architecture.md](architecture.md#adapters) says which a project uses):

- **pytest** suites run on aye's own engine: `import pytest` gets aye's own
  `pytest` module, and aye's pytest front-end reads tests, fixtures,
  `conftest.py`, markers and parametrize as pytest 9.1.1 does, so they
  behave as under pytest. pytest itself is never imported, and need not be
  installed. Third-party plugins don't load yet (see [aye's `pytest`
  module](#ayes-pytest-module)). The reference is pytest 9.1.1.
- **unittest** suites run natively, without pytest, as `python -m unittest
  discover` runs them on Python 3.13. See [unittest](#unittest).
- **doctests** run natively, as pytest's doctest plugin (`pytest
  --doctest-modules`, `--doctest-glob`) runs them. See [doctest](#doctest).

Django suites aren't supported yet. The example suites in
`crates/aye/tests/suites` and the projects in `examples/` are checked by the
comparison harness (`crates/aye/tests/compare.rs`) against each one's
referee, plain pytest or plain unittest, as
`crates/aye/tests/harness/record.py` records it: the same ids in the same
order, the same outcome per id, and the same exit status.

## Command line

| pytest | aye | Notes |
|---|---|---|
| `PATH`, `PATH::Class::test[id]` | yes | Relative to the current directory; a path that only exists from the project root is taken from there, so the ids aye prints rerun from any directory (pytest would say `not found`). A path that exists from the current directory wins, so in a subdirectory holding a path of the same name as a test's file, the rerun aye prints names the test's file from there. aye also takes dotted names (`pkg.mod.Class.test`) and test keys (4 to 16 hex digits), and runs as `pytest <the files they select>` (with the path arguments) would: pytest's start-up conftests are those files' (see [Known differences](#known-differences) for collection errors elsewhere). A dotted name for a module or package that skips while collected counts one skip, as `pytest --pyargs` does, and one for a test in it is that skip and a `not-found` (exit 4). Tests are listed in the order of the arguments, as in pytest. |
| `-k EXPR` | yes | pytest 9's expression language, applied to the cases collection lists (for pytest's, its front-end's, with pytest's keywords); static selection is final for every adapter. A value may start with `-` where pytest allows it (`-k -1`). Without `-k`, the one in `addopts` or `PYTEST_ADDOPTS` applies to pytest's cases; one after `--` wins over both, for every case. |
| `-m MARKEXPR` | yes | As `-k`. A `marker(key=value)` expression keeps every case aye can't decide. unittest and doctest cases have no marks. |
| `-x`, `--exitfirst`, `--maxfail N` | yes | The last one given wins, and `--maxfail 0` means no limit, as in pytest. One after `--` wins over the command line's, since pytest reads those arguments last (`aye --maxfail 3 -- -x` stops at the first failure). Those in `addopts` or `PYTEST_ADDOPTS` apply when neither gives one. Counted across workers, in tests; running batches then end as pytest's `-x` ends a session, tearing down. See [Known differences](#known-differences). |
| `-v`, `-q` | yes | Repeatable. They shape aye's output, text or JSON lines alike: the number of `-v` less the number of `-q`, from `-q` (the summary alone) to `-vvv` (everything; in JSON, the raw event stream). See [Output](#output). |
| `-s` | yes | Output is shown live: on stdout with text output, on stderr with `--output json`, so JSON lines stay whole. `--capture=…` is not supported. |
| `--lf`, `--last-failed` | yes | From aye's history, for every adapter's tests. When nothing failed, everything runs (pytest's default `--lfnf=all`). A test that didn't run (cut off by a stop) keeps its state. |
| `--ff`, `--failed-first` | yes | From aye's history. Last run's failures run first, across files. |
| `--co`, `--collect-only` | yes | A flat list of ids, after refinement (aye's pytest front-end lists every pytest file). With `--no-refine`, the static prediction, without importing anything; a partial case is shown as `id[…]`. |
| `-c FILE`, `--config-file FILE` | yes | Also after `--` or in `PYTEST_ADDOPTS`. A failed test's printed rerun repeats aye's own (`aye -c FILE <id>`), as given. |
| `--continue-on-collection-errors` | yes | The run goes on past collection errors, whether collection or a batch finds them; they still fail it (exit 1), as in pytest. |
| `--junitxml PATH`, `--junit-xml PATH` | yes | `$VAR` and `~` are expanded, as in pytest. Also read from `addopts`, `PYTEST_ADDOPTS` or after `--` (the command line's wins): aye writes one report for the run. A path that can't be written fails before anything runs (exit 4). It follows pytest's `junit_suite_name`, `junit_logging`, `junit_log_passing_tests`, `junit_duration_report` and `junit_family`, and carries what tests record with `record_property` and `record_testsuite_property` (each run property once, though every worker's session records it). A failure's text is the traceback as Python prints it, not pytest's report. Lines are 1-based (pytest's are 0-based), a decorated test's is its `def` line (pytest's is its first decorator's), and a file that fails to collect has its error's (pytest gives none); `expected_failure` counts as skipped and `unexpected_pass` as passed; each failed subtest is a `<failure>` of its test, and so is each failing doctest example; a skipped test's output is written once (pytest writes it twice); `record_xml_attribute` adds no attribute. |
| `--junitprefix`, `--junit-prefix` | in `addopts` | From `addopts`, `PYTEST_ADDOPTS` or after `--`; it goes before every `classname`. |
| `-n N\|auto` | yes | Built in, without pytest-xdist. The long form is `--workers`; `logical` means `auto`. |
| `--color WHEN` | yes | `auto`, `always` or `never` (pytest's are `yes`, `no` and `auto`). |
| `--strict` | differs | aye's turns collection warnings into errors. pytest's (the `strict` ini option) goes in `addopts` or after `--`, where aye reads it too. |
| `-p NAME`, `-o NAME=VALUE`, `-W` | after `--` | `aye -- -p myplugin -o xfail_strict=true`. Read by aye's pytest front-end after `addopts` and `PYTEST_ADDOPTS`, so they win. `-p` loads a project module as a plugin; a third-party one is a usage error. See [The arguments after `--`](#the-arguments-after---). |
| `--ignore`, `--ignore-glob`, `--deselect`, `--confcutdir`, `--noconftest`, `--pyargs`, `--import-mode` | after `--` or in `addopts` | Read from `addopts`, `PYTEST_ADDOPTS` and the arguments after `--`, statically and by the front-end, which applies pytest's ignore rules. `--deselect` drops the cases it names before the run and counts them as deselected, as pytest does. |
| `--rootdir DIR` | after `--` | Or in `PYTEST_ADDOPTS`; read as pytest reads it. Otherwise aye uses pytest's rootdir discovery. |
| `-h`, `-V`, `--co`, `--fixtures`, `--fixtures-per-test`, `--markers`, `--setup-plan`, `--setup-only` after `--` | no | A usage error (exit 4) before anything runs, naming what to use instead (`aye --co`, `python -m pytest --fixtures`). Also inside a cluster (`-vh`). `--setup-show` runs the tests. `--setup-plan` and `--setup-only` from `addopts` or `PYTEST_ADDOPTS` are a usage error too: the batch runs nothing. |
| `--doctest-modules`, `--doctest-glob` | yes | Run natively by the doctest adapter, with `--doctest-report`, `--doctest-continue-on-failure` and `--doctest-ignore-import-errors`; see [doctest](#doctest). Without the doctest adapter in the run, the files pytest's doctest plugin would collect are a usage error that says to add it. |
| `--timeout` | planned | Per-test timeouts (Step 4). |
| `--cov` | planned | Built-in coverage (Step 4). pytest-cov doesn't load: its options are unknown to aye's engine (a warning); see [aye's `pytest` module](#ayes-pytest-module). |
| `--durations` | no | History keeps per-test durations. |
| *(none)* | `aye show` | No pytest equivalent: pytest's results are gone once its report is printed (its cache keeps only the ids that failed). aye keeps the log of every run that ran something in `.aye/runs` and `aye show` reads it back, never running anything: the last run's list, a line per test (and per failed setup or teardown, collection error or module skipped while collecting), ending with the run's own summary (`--outcome`, `-k`, `-m` and paths filter it), or the details of the tests named by key (or its first digits), id, dotted name or path, or of every failure (`--failed`), as the run showed them, at any `-v`/`-q` and in either format; `--run r12` for an older run, `--runs` to list them, `--log` for a run's raw events. See [Output](#output). |
| `--pdb`, `--trace` | no | Needs an interactive worker. |

aye's own flags: `--output text|json` (any case; see [Output](#output)),
`--python PATH`, `--no-refine` (keep the static guess instead of importing
files; cached results are still used; with pytest's adapter in the run,
only with `--co`, since aye's pytest front-end imports every file a run
runs: a run with it is a usage error), `--native` (the prototype's switch,
accepted and doing nothing), `aye config` and `aye show`. Unknown flags
are usage errors (exit 4), as in pytest.

### The arguments after `--`

Everything after `--` is pytest's: aye's pytest front-end parses it after
`addopts` and `PYTEST_ADDOPTS` (`aye -k a -- -k b` selects with `-k b`), as
pytest's options, the built-in ones and those conftests add with
`pytest_addoption`. Static collection reads them as pytest would, the last
one winning: `-c`, `--rootdir`, `-o`, `--strict`, the collection options in
the table above, `-p` (a project module is read as a plugin; with `-p
no:python`, no `.py` file holds tests but pytest's doctests, so one named
explicitly is otherwise `unclaimed`, exit 4, as pytest finds no collector
for it), `-k` and `-m`. aye takes over pytest-xdist's options (`-n` sets
the worker count when neither `-n` nor `[tool.aye] workers` does),
`-x`/`--maxfail` and the JUnit options. An option nothing defines (no
builtin, conftest or `-p` plugin) is a usage error (exit 4), as in pytest,
unless pytest would load a plugin by itself that aye doesn't (a `pytest11`
entry point, whose options aye can't know): then it is an `unknown-option`
warning naming those plugins, and aye ignores the option. The doctest
adapter reads the pytest options it knows there too; the
unittest adapter ignores them. `@file` argument files aren't read, and
positional paths there select nothing: give paths before `--`. When `-k` or
`-m` may hide in what aye can't read (an `@file`, a plugin's short option),
every adapter's cases get the command line's, else their own. A failed
test's rerun command repeats the arguments after aye's own options that
shaped the run: `aye -c FILE <id> -- <the arguments>`, without
`--junitxml`.

### Output

aye's output is its own, not pytest's terminal report. There are two
formats: `--output text` (the default, for people) and `--output json`
(JSON lines, for programs and coding agents), also set by `[tool.aye]
output`. A run is short: in text, a line per failure saying why
(`cart.summary()['pear']: expected 3, actual 2`), in JSON the summary with
the keys of what failed; `-v`, `-vv` and `-vvv` add detail, the same in
both formats (the failing code and an expected/actual diff, then every
frame, the captured output and the locals), and `-q` leaves the summary
alone. More `-v` than three or `-q` than one changes nothing. `--output
json -v` is the one for coding agents: a line per failure, then `aye show
<key>` for the ones to fix. Each
level is described in
[architecture.md](architecture.md#output-and-history). The formats' names
until October 2026, `human` and `agent`, are usage errors that say what to
use instead (`text`; `json -v`).

Each test has a short key beside its id, the first 5 hex digits of a hash
of the id, longer only when two tests need it (`3fae6`; see
[events.md](events.md#keys)), and each run a number (`r12`). pytest has
neither. `aye show 3fae6` shows that test's result in the last run again,
in either format and at any level, without running anything: run `aye
--output json` for the cheap summary and its failures' keys, then `aye
show 3fae6 --output json` for what it takes to fix one. Any 4 digits or
more of a key name the test, unless several keys start with them (a usage
error that lists them). `aye show --failed` gives every failure in
detail, `--run r12` another stored run, `--log` a run's raw events (what
`--output json -vvv` printed until October 2026). `aye
show` with no test named lists the run, a line per test, a failed
`tearDownClass` or collection error included, and ends with the run's
summary; it says first when a file of a test it shows changed since the
run (`{"type":"stale",…}` in JSON). Its summary's `exit` is the run's. It
exits 0 when it could show what was asked, whatever the outcomes, 4 for a
name no test of the run has (with the closest ids), and 5 when no run is
stored. A run that ran nothing (a path that doesn't exist, a `-k` that
matches nothing) isn't kept, so `aye show` still shows the run before it.

## Configuration

### pytest's

aye finds pytest's rootdir and inifile as pytest 9 does: `pytest.toml`,
`.pytest.toml`, `pytest.ini`, `.pytest.ini`, `pyproject.toml` (native
`[tool.pytest]` or `[tool.pytest.ini_options]`; a bare `pyproject.toml`
becomes the inifile when nothing else is found), `tox.ini` (`[pytest]`) and
`setup.cfg` (`[tool:pytest]`), then a directory with `setup.py`. `-c`
overrides the search and `--rootdir` the rootdir, from the command line,
`PYTEST_ADDOPTS` or the arguments after `--`, as in pytest.

| Option | Read by aye | |
|---|---|---|
| `testpaths`, `python_files`, `python_classes`, `python_functions`, `norecursedirs` | collection | |
| `addopts` | collection and workers | `-o`, `--strict`, `--ignore`, `--ignore-glob`, `--deselect`, `--confcutdir`, `--noconftest`, `--collect-in-virtualenv`, `--pyargs`, `--import-mode`, `-p`, `-k` and `-m` are read statically; positional paths are not: with `addopts = "tests"`, pytest collects only `tests`, and aye collects as if `addopts` named no path. Lifted out: pytest-xdist's options (`-n` becomes aye's default worker count), `-x` and `--maxfail` (aye counts them across workers), the JUnit options (aye writes the report) and, when the doctest adapter runs, the doctest options (it runs the doctests). The front-end parses the rest, before `PYTEST_ADDOPTS` and the command line, which still override it. |
| `PYTEST_ADDOPTS` | collection and workers | Read as `addopts` is, after it and before the arguments after `--`, the last one winning; also for `-c`, `--rootdir` and `-o`, which take part in finding the config. |
| `usefixtures`, `empty_parameter_set_mark`, `collect_imported_tests` | collection | |
| `strict`, `strict_parametrization_ids`, `disable_test_id_escaping_and_forfeit_all_rights_to_community_support` | collection (ids) | |
| `doctest_optionflags`, `doctest_encoding` | the doctest adapter | A flag name pytest doesn't accept is a usage error. |
| `markers` | the front-end | It applies `--strict-markers` and the like, as pytest does. |
| `junit_suite_name`, `junit_logging`, `junit_log_passing_tests`, `junit_duration_report`, `junit_family` | the JUnit report | aye writes the report, by them; also from `-o`. |
| everything else | the front-end | Every raw value reaches it, for `getini` and the options it implements (`xfail_strict`, `filterwarnings`, `pythonpath`, `log_*`, …). |

### aye's

aye's project root is the nearest directory holding `pyproject.toml`, else
`.git`, searching up from the paths given (or the current directory). Test
ids are relative to it. Its own settings are `[tool.aye]` in that
`pyproject.toml`; unknown keys and bad values are usage errors.

```toml
[tool.aye]
workers = "auto"            # or a number
python = ".venv/bin/python"
output = "text"             # text | json, in any case
timeout = 30.0              # seconds per test; read, not enforced yet
adapters = ["unittest", "pytest"]   # which run, in priority order
keep-runs = 10              # runs whose logs `aye show` can read; 0 keeps none

[tool.aye.unittest]         # see unittest below
[tool.aye.doctest]          # see doctest below
```

- **Adapters**: `[tool.aye] adapters`, in its order, which is their
  priority when two collect one test. Unset, doctest then pytest, for every
  project and whatever the interpreter installs (pytest's rules run
  `TestCase` classes too); list `unittest` for `python -m unittest
  discover`'s rules. See [architecture.md](architecture.md#adapters).
- **Workers**: `-n`, else `[tool.aye] workers`, else the last `-n` in
  pytest's `addopts`, `PYTEST_ADDOPTS` or the arguments after `--`, else 1.
  0 means 1, as `-n 0` does under pytest-xdist.
- **Interpreter**: `--python`, else `$AYE_PYTHON`, `[tool.aye] python`,
  `$VIRTUAL_ENV`, a `.venv` in the current directory then the root, the
  interpreter beside the `aye` binary (or beside the file a link to it
  leads to), then `python3` or `python` on `PATH`. aye needs only the
  standard library there, for every adapter: pytest need not be installed,
  and when it is, aye's own `pytest` module wins over it.
- **`aye config`** prints the root, the interpreter and where it came from,
  the worker count, the adapters and why, the engine (`engine   native`:
  aye's own runs every test), `[tool.aye]`, and each adapter's settings
  with their origins (pytest's rootdir, inifile and options;
  `[tool.aye.unittest]`; the doctest adapter's), or that an adapter has
  nothing to collect.
- **State** lives in `.aye/` under the root: `history.json`, the
  refinement cache, and `runs/`, the logs of the last runs (`keep-runs`, 10
  by default; a collect-only run isn't kept), which `aye show` reads. aye's
  runs don't feed pytest's own `--lf`: `--lf` and `--ff` read aye's
  history.

## aye's `pytest` module

Every worker installs aye's own `pytest` module (standard library only)
under the name `pytest`, with the `_pytest.*` modules suites import from,
before any test or conftest is imported, so `import pytest` gets it even
where pytest is installed. It implements pytest 9.1.1's authoring API:

- marks (`pytest.mark.*`, `Mark`, `MarkDecorator`, `param`), with
  `markers`, `--strict-markers` and pytest's unknown-mark warning;
- fixtures (`pytest.fixture` of every scope, with `params`, `ids`,
  `autouse`, `name`, yield fixtures and their teardown), `usefixtures`,
  `request` and the built-in fixtures (`tmp_path`, `tmp_path_factory`,
  `tmpdir`, `monkeypatch`, `capsys`, `capfd`, `caplog`, `recwarn`,
  `pytestconfig`, `cache`, `subtests`, `record_property`,
  `record_testsuite_property`, `doctest_namespace`);
- parametrize (marks, `pytest_generate_tests`, `metafunc`), with pytest's
  ids;
- outcomes: `skip`, `fail`, `xfail`, `exit`, `importorskip`, their
  exceptions and the `skipif`/`xfail` conditions;
- checks: `raises`, `warns`, `deprecated_call`, `approx`, and assertion
  rewriting with pytest's messages for failed comparisons;
- `MonkeyPatch`, `Stash`, the types suites annotate with or test against
  (`FixtureRequest`, `Config`, `Item`, `Function`, `CaptureFixture`…),
  `hookimpl`, `register_assert_rewrite`, `ExitCode`, `UsageError`, the
  warning classes and `__version__`.

Anything else is an error naming it (an `AttributeError`, or a
`ModuleNotFoundError` for a `_pytest` module aye doesn't provide); `pytest.main`
is a usage error, since aye runs the suite. What aye's engine doesn't run
yet is a clear error, never a different result:

- **Conftest hooks.** aye runs `pytest_addoption`, `pytest_configure`,
  `pytest_unconfigure`, `pytest_sessionstart`, `pytest_sessionfinish`,
  `pytest_collection`, `pytest_collection_modifyitems`,
  `pytest_collection_finish`, `pytest_deselected`, `pytest_itemcollected`,
  `pytest_ignore_collect`, `pytest_generate_tests`,
  `pytest_make_parametrize_id`, `pytest_markeval_namespace`,
  `pytest_report_header`, `pytest_runtest_setup`, `pytest_runtest_call` and
  `pytest_runtest_teardown`, and ignores those that only shape pytest's
  terminal output. A conftest implementing any other (or one of these as a
  hook wrapper) is a `usage-error` naming it, and the tests below it don't
  run.
- **Third-party plugins** don't load yet. One that `-p`, `pytest_plugins`
  or `PYTEST_PLUGINS` names is a `usage-error` (a project module named
  there loads, as pytest loads it). One installed that pytest would load
  by itself (a `pytest11` entry point) is a `plugin-not-loaded` warning
  naming it, once per run (`-p no:NAME` silences it), except those whose
  job aye does itself or that only change pytest's display (pytest-xdist,
  pytest-cov, pytest-timeout, pytest-sugar, pytest-instafail,
  pytest-pretty, pytest-metadata, pytest-html), which are left out
  quietly. An option nothing aye loads defines is an `unknown-option`
  warning while such a plugin is installed (it may be the plugin's), else
  a usage error.
- **pytest's own doctests**, without the doctest adapter in the run, are a
  `usage-error` naming the fix: add `doctest` to `[tool.aye] adapters`.

## Known differences

These are pytest's; unittest's and doctest's follow in their sections.

- **No path from the home directory.** `aye` with no path (only options,
  dotted names or keys) run from your home directory or a filesystem root
  is a usage error (exit 4) that says to run it inside a project or name
  what to test; pytest would collect everything under it. Naming a path
  (`aye .`) runs as usual.

- **One outcome per test.** A test that fails and then errors in teardown is
  one `error`, carrying both failures. pytest counts it as "1 failed, 1
  error".
- **Outcome names** are aye's: `expected_failure` (xfailed) and
  `unexpected_pass` (xpassed). A strict xpass is `failed`, as in pytest.
- **Ids are relative to aye's project root**, even when pytest's rootdir is
  elsewhere: with `sub/pytest.ini`, pytest says `tests/test_x.py::test_y`
  and aye says `sub/tests/test_x.py::test_y`.
- **Collection errors.** aye's pytest front-end imports every pytest file
  while aye collects, so a file that fails to collect stops the run before
  anything runs (exit 2), as in pytest; a hook that fails while collecting
  is pytest's internal error (exit 3). A requested id the front-end can't
  find is a `not-found` error.
- **A broken conftest.** One pytest loads at start-up (in or above a
  collection root, or in a `test*` directory of one) that fails to load,
  or a `pytest.UsageError` from a conftest's `pytest_configure`, is a usage
  error (exit 4), as in pytest. So is one that skips
  (`pytest.importorskip`) or calls `pytest.fail` while loaded, where pytest
  stops with a traceback (exit 1). `aye --co --no-refine` imports nothing,
  so it lists the tests and exits 0 where pytest exits 4. Any other
  conftest only matters to its directory, as in pytest: one that skips
  while loaded skips the directory, counted as one skip, and one that fails
  to import, calls `pytest.fail` or defines `pytest_plugins` is the
  directory's collection error. With the default adapters, a conftest both
  the pytest and doctest adapters fail to load is one error. With dotted
  names or keys, a start-up conftest's failure or a directory's skip that
  the run doesn't reach (one `pytest <the selected files>` wouldn't meet)
  is left out.
- **Collection errors elsewhere stop a dotted-name run.** To resolve dotted
  names and keys, aye collects the whole project (`testpaths`, else the
  current directory), so a file that fails to collect anywhere in it (a
  syntax error, a non-top-level conftest's `pytest_plugins`) stops the run
  (exit 2), where `pytest <the selected files>` never meets it. Give paths
  to avoid it.
- **The framework stopping the run.** `pytest.exit(returncode=N)`, a
  `KeyboardInterrupt`, `session.shouldstop` (`--stepwise`) or a `maxfail`
  a conftest sets stops the run whatever the worker count: nothing more is
  handed out, batches already running finish and report their tests, and
  aye exits 2, as pytest does, except that pytest exits N for `pytest.exit`
  and 1 for a `maxfail`.
- **`pytest_sessionfinish` runs once per worker**, as each pytest-xdist
  worker runs it, in the batch that ends the worker's session: a
  `session.exitstatus` it sets to 1 fails the run (a `session-failed`
  warning per worker), as pytest exits 1; one it sets to 0 doesn't make a
  run with failed tests pass, where pytest's exit status follows it. When
  the run is stopped (Ctrl-C), a worker the stop doesn't kill runs it as it
  shuts down, and what fails there isn't reported.
- **Stopping kills.** Ctrl-C, SIGTERM or SIGHUP kills the workers with the
  processes their tests started (SIGKILL to each worker's process group), so
  fixture teardown and `finally` blocks don't run, where pytest runs them,
  and the test in flight is not reported. A second signal within half a
  second of the first counts as the same one (GNU `timeout` signals aye,
  then its process group); a later one makes aye exit at once.
- **`--maxfail`** counts tests with a bad outcome, across workers; pytest
  counts failed reports (a test that fails and then errors in teardown
  counts twice, each failed subtest counts, and with
  `--continue-on-collection-errors` so does each file that fails to
  collect). Collection errors don't count toward aye's.
  When the limit is reached, busy workers end their batches as pytest's
  `-x` ends a session, so fixtures tear down; one still busy 10 seconds
  later is killed.
- **A file's tests run together**, at the place of its first test:
  `aye a.py::TestK b.py a.py::test_a1` runs both of `a.py`'s tests, then
  `b.py`'s, where pytest runs `TestK`, `b.py`, then `test_a1`. `--co` lists
  pytest's order.
- **Session-scoped fixtures** are set up once per worker, as under
  pytest-xdist: once per run with one worker, as in pytest. Every worker
  imports the whole session before its first test, as pytest-xdist's do.
  They tear down in the teardown of the worker's last test when the runner
  knows it is the last (always with one worker), so a failed teardown is
  that test's error, as in pytest; otherwise (a worker that found nothing
  left only after its batch) in a batch with no tests, whose `teardown of
  session` error fails the run all the same.
- **Doctests beside pytest's tests run adapter by adapter.** With
  `--doctest-modules`, pytest runs each file's doctests just before its
  tests; aye's doctest adapter runs them in batches of their own. With one
  worker, a run goes in plan order (the doctest batch first, when the first
  file has doctests), each adapter's tests together. `--co` lists pytest's
  order.
- **`-m` with a mark's arguments** (`-m "device(serial='1')"`) keeps every
  test that has the mark, whatever its arguments: cases don't carry them,
  and nothing re-applies the expression in a batch, where pytest compares
  them.
- **Deselection isn't always counted.** Tests a conftest hook deselects
  never start and are not in `deselected`: aye says "3 passed" where
  pytest says "3 passed, 1 deselected". `--co` lists them.
- **One id, two tests.** When pytest makes two tests with one id (colliding
  parameter ids), `--co` lists the id once with a `duplicate-id` warning;
  the run reports both results under it, as pytest counts them.
- **Import file mismatch.** Two test files pytest imports under one name run
  in one batch, so the run fails as pytest's does, whatever the worker count.
- **A worker that crashes** fails only the test it was running; the batch's
  remaining tests run in a new worker.
- **Subtests.** Counts are per test: where pytest says "3 failed, 2
  subtests passed", aye says "2 failed". A subtest's result carries no
  output of its own: the test's captured output is shown once, with the
  test's own failure or, for a test that failed only through its subtests,
  in a short block after theirs (text) or on the first subtest's line
  (JSON lines, `-v`).
- **Not collected yet:** tests a `pytest_collect_file` collector makes of
  non-Python files (YAML): a conftest implementing the hook is a usage
  error, and an explicit argument naming such a file is `unclaimed`.
- **The static prediction** (`aye --co --no-refine`) doesn't see installed
  plugins, `PYTEST_PLUGINS` and `sitecustomize`, what they or another test
  module of the session change in a module aye reads values from, and
  module-level code that raises for reasons aye can't see (`X = 1 / 0`, a
  missing import) in a file with tests aye can read; its cases are exact
  only where [architecture.md](architecture.md#static-collection-pytest)
  says.
- **Workers** get the run's id as `PYTEST_XDIST_TESTRUNUID` (16 hex digits;
  pytest-xdist's are 32). With more than one worker, `PYTEST_XDIST_WORKER`
  and `PYTEST_XDIST_WORKER_COUNT` are set too, and a `--basetemp` gets a
  `popen-gw<n>` directory per worker, as under xdist. A run's temporary
  directories are one, as a single session's would be: pytest's retention
  (`tmp_path_retention_count`) rotates once per run.
- **pytest's terminal output** doesn't exist: aye renders the run from its
  events, and hooks that only shape pytest's terminal output are ignored.

## unittest

The unittest adapter runs `TestCase` suites natively: it imports only the
standard library, so it works without pytest. The reference is `python -m
unittest discover` on Python 3.13; 3.10 to 3.12 load the same tests, except
that before 3.11 an imported `unittest.FunctionTestCase` makes a test of its
own. It runs when `[tool.aye] adapters` names it. By default the pytest
adapter runs `TestCase` classes, by pytest's rules; list `unittest` (alone,
or before `pytest`) to run them with unittest's discovery.

### Settings

```toml
[tool.aye.unittest]
start-directory = "tests"      # discover -s; default "."
pattern = "test*.py"           # discover -p; default "test*.py"
top-level-directory = "."      # discover -t; default: the start directory
```

The directories are relative to the project root, not the current
directory. Unknown keys, values that aren't strings, an empty pattern,
directories that don't exist, a start directory outside the top-level
directory, and one below it that isn't a package (unittest's "Start
directory is not importable") are usage errors (exit 4), even when the
unittest adapter doesn't run.

### Discovery, ids and order

- **Discovery** walks as unittest's does: from the start directory, files
  whose names are valid module names and match the pattern (case-sensitive
  except on Windows), entering only packages. A package's `__init__.py`
  holds tests too, except the top-level directory's. Paths on the command
  line narrow the walk; a file named there is a test module whatever the
  pattern, as `python -m unittest path/to/test_x.py` takes it.
- **Ids** are `path::Class::method`, with the path relative to aye's root
  and `Class` the name the class is bound to in its module (an alias's, a
  factory's), as pytest names `TestCase` methods: the unittest and pytest
  adapters give a test one id. A test only a `load_tests` makes is
  `path::name` (a `FunctionTestCase`'s function, a `DocTestSuite` case's
  doctest name). Dotted names select too (`aye
  tests.test_cart.CheckoutTests`), read from the root: unittest's own when
  the top-level directory is the root.
- **Order** is the loader's: classes by the name they are bound to, methods
  by name, and `runTest` for a class with no `test*` methods. Files follow
  aye's walk, sorted by name, each package's `__init__.py` first.
- **Selection.** `-k` matches the path's parts and those names. unittest
  cases have no marks, so `-m slow` keeps none of them.

### Running

| unittest | aye |
|---|---|
| ok | `passed` |
| FAIL: a failed assertion (in the test, `setUp`, `tearDown` or a cleanup) | `failed` |
| ERROR: any other exception there | `error` |
| skipped | `skipped` |
| expected failure | `expected_failure` |
| unexpected success | `failed` ("unexpected success"), as `wasSuccessful()` is false then |

- **Fixtures are lifetimes.** `setUpModule` and `tearDownModule`,
  `setUpClass` and `tearDownClass`, and the module's and class's cleanups
  run as `TestSuite.run` runs them for the batch's order, and are reported
  as lifetimes (`lifetime_started`, `lifetime_finished`). A failed
  `setUpClass` or `setUpModule` is reported once, on its lifetime, and each
  test it kept from running is `error`, with `cause` naming it; a
  `SkipTest` raised there skips them. A skipped class runs neither its
  setup nor its teardown. A failed teardown or cleanup is reported on its
  lifetime and fails the run (exit 1).
- **Subtests.** Each `subTest` block is a sub-outcome named by its
  description (`[msg] (i=1)`). A failed or errored subtest fails or errors
  its test.
- **`load_tests`** is honoured: a module that may define one is imported to
  collect it, and its tests are what `load_tests` returns. A package whose
  `__init__.py` may define one decides the tests of every file below it,
  as discovery hands it the package: aye asks `TestLoader.discover` on it.
- **Failures** carry unittest's message and text, and for a failed
  comparison (`assertEqual`, `assertIn`, `assertLess`, …) its operands as
  `left` and `right`, with unittest's explanation as the diff.
- **Output** is captured per test, as `-b` does; a class or module
  fixture's is kept for its failure (`Stdout:`, `Stderr:`). `-s` turns
  capture off.
- **Stops.** A `KeyboardInterrupt`, or a test calling `result.stop()`,
  stops the run (exit 2).

### Differences from `python -m unittest`

- **Counts are per test.** unittest counts a failed `setUpClass` as one
  error; aye makes each test it kept from running an `error`. A failed
  `tearDownClass`, `tearDownModule` or cleanup fails the run (exit 1) and
  is counted apart from the tests: aye says `1 passed, 1 teardown error`
  where unittest says `FAILED (errors=1)`. unittest counts a skipped
  subtest and a `SkipTest` from a teardown among its skips; aye reports
  the first as a sub-outcome of a passing test and the second not at all.
  A module that raises `SkipTest` while imported is a `skipped` info
  diagnostic, which the summary counts as one skip.
- **A module that fails to import** is one `collection-error`, and its
  tests don't run. The run stops before anything runs when collection finds
  it, or exits 2 once the other batches finish when a batch does, unless
  `--continue-on-collection-errors` (exit 1); unittest counts one error and
  goes on. Every test a `load_tests` fails to load is in the file's one
  `collection-error`. A package whose `__init__.py` fails to import hides
  the modules below it from unittest; aye collects them statically, so with
  `--continue-on-collection-errors` each also reports the package's error.
- **A package and a file in it, both named** (`aye tests/pkg
  tests/pkg/test_x.py`, the package's `load_tests` loading `test_x`): the
  file's tests run once, under the file's id; `python -m unittest` given
  both names runs them twice.
- **Dotted names select among the tests collection finds**, so aye collects
  the whole project for them: `aye tests.test_cart.CheckoutTests` stops on a
  module elsewhere that fails to import, where `python -m unittest
  tests.test_cart.CheckoutTests` imports only what it names.
- **Modules are imported batch by batch**: unittest imports every test
  module before running a test, and a worker runs several batches in one
  process, so a test that reads, at import, a value another file's test
  changes may pass or fail from run to run.
- **What a test module prints while imported** is dropped under capture;
  unittest prints it as discovery runs.
- **Captured output is UTF-8**: what UTF-8 can't hold (a lone surrogate)
  is written as `?`, as pytest's capture writes it; `-b` keeps it.
- **Exit codes are pytest's**, as for every adapter: 2 for an import error
  or an interrupted run, 4 for a bad configuration, 5 when no test ran.
- **Not honoured yet:** the arguments after `--` (reruns still repeat
  them), custom `TestSuite` subclasses' `run` (suites
  are flattened), and a pattern matching `__init__.py` (unittest then
  imports a package twice and runs its tests twice; aye once). Django's
  `TestCase` needs Django's settings, which aye doesn't set up yet.

### Beside pytest

With `adapters = ["unittest", "pytest"]`, both adapters collect the
`TestCase` classes, under the same ids, and the first listed runs them:
unittest's adapter runs them, by unittest's rules, and pytest's the rest.
Where that differs from pytest running everything: any exception but a failed
assertion, in a test, `setUp` or `tearDown`, is unittest's `error` and
pytest's `failed`; pytest's marks and autouse fixtures on `TestCase`
methods apply only under pytest; and a file's tests are listed adapter by
adapter (its `TestCase` methods first), where pytest keeps the file's
order. A file that fails to import counts one collection error, as in
pytest: the second adapter's report of it comes as an info (aye's pytest
front-end reports it while aye collects; a unittest batch's report of it
is the info).

## doctest

The doctest adapter runs doctests natively, with the standard library's
`doctest`, so it works without pytest. Its reference is pytest 9.1.1's
doctest plugin: `pytest --doctest-modules` and `--doctest-glob` give the
same ids, order and outcomes. It is in the default adapters, first, as
pytest lists a file's doctests before its other tests.

### Activation

- **Modules** are collected with `--doctest-modules` in pytest's options
  (`addopts`, `PYTEST_ADDOPTS` or the arguments after `--`; not after `-p
  no:doctest`), or `[tool.aye.doctest] modules = true`. `modules = false`
  turns them off whatever pytest says. Every `.py` file the walk reaches is
  one, test modules and conftests included, except `__main__.py` and a
  `setup.py` that mentions setuptools or distutils.
- **Text files** match `[tool.aye.doctest] glob`, else pytest's
  `--doctest-glob`s, else `test*.txt` when a pytest configuration exists or
  pytest runs too, else nothing. A `.txt` or `.rst` file named on the
  command line is a doctest whatever the globs, as in pytest. A text file
  without examples is no test. Markdown is a text file like any other: its
  fences aren't special.
- **Without the doctest adapter** in the run (`[tool.aye] adapters =
  ["pytest"]`), the files pytest's own doctest plugin would collect are a
  `usage-error` that says to add the doctest adapter: aye runs doctests
  with it only. `aye config` says who would collect them (`doctests =
  collected by pytest's doctest plugin`).

```toml
[tool.aye.doctest]
modules = true                          # as --doctest-modules
glob = ["test*.txt", "*.rst"]           # as --doctest-glob
optionflags = ["NORMALIZE_WHITESPACE"]  # added to doctest_optionflags
encoding = "utf-8"                      # as doctest_encoding
```

Unknown keys and bad values are usage errors, even when the doctest adapter
doesn't run.

### Ids, options and outcomes

- **Ids** are pytest's: a module gives one test per docstring with examples,
  named as `DocTestFinder` names it under the module's name in pytest's
  default import mode (`pkg/__init__.py::pkg`, `pkg/mod.py::pkg.mod.f`,
  `pkg/mod.py::pkg.mod.Class.method`), in its order; a text file is one
  test (`docs/guide.txt::guide.txt`). `-k` matches the directories below
  pytest's rootdir, the file name and the test's name; doctests have no
  marks.
- **Options**: pytest's `doctest_optionflags` (default `ELLIPSIS`) plus
  `[tool.aye.doctest] optionflags`, which takes any doctest flag, doctest's
  `COMPARISON_FLAGS` and `REPORTING_FLAGS`, and pytest's `ALLOW_UNICODE`,
  `ALLOW_BYTES` and `NUMBER`. Text files are read with `[tool.aye.doctest]
  encoding`, else `doctest_encoding`, else UTF-8. `--doctest-report` picks
  the diff in a failure's text, `--doctest-continue-on-failure` lets pytest
  reach the examples after a failure, and `--doctest-ignore-import-errors`
  makes a module that fails to import a skipped one. From pytest's
  configuration, aye also reads `testpaths`, `norecursedirs`, `pythonpath`,
  `filterwarnings`, `-W`, `--ignore`, `--ignore-glob`, `--deselect`,
  `--import-mode`, `--noconftest` and `--confcutdir`.
- **Each example is a sub-outcome**, named `line N: >>> source`. The test's
  outcome is pytest's: `skipped` when every example has its own `+SKIP`,
  `failed` when an example failed, else `passed`; pytest's `skip`, `xfail`
  and `fail` raised in an example end the test as in pytest. A mismatch is
  reported as `got == expected`, with their unified diff.
- **Conftests**: the conftests pytest would have imported before a file are
  imported first (through aye's `pytest` module), so what they do to `sys.path`
  or the environment reaches the doctests. One that skips while loaded
  (`pytest.importorskip`) skips the files below it, counted as one skip,
  as in pytest; one that fails otherwise fails them, with one error on the
  conftest's path, which the pytest adapter reports in the same words, so
  it shows once. One pytest loads at start-up that fails or skips is a
  usage error (exit 4). pytest's warning filters apply (`filterwarnings`,
  `-W`), so a warning turned into an error fails its example; and
  `PYTEST_VERSION` and `PYTEST_CURRENT_TEST` are set as pytest sets them.
- **What pytest fails to collect fails here**: an import error, a docstring
  doctest can't parse, a text file that can't be read is a
  `collection-error`, and the file's tests don't run.

### Differences from pytest's doctest plugin

- **pytest's fixtures don't exist natively**: `doctest_namespace`,
  `getfixture` and autouse fixtures. Doctests that need them can't run
  yet.
- **Every example runs**, even after one failed, as doctest itself runs
  them; pytest stops at the first failure unless
  `--doctest-continue-on-failure`. The outcome is pytest's either way.
- **Marks and `-k`/`-m` from pytest's options.** The adapter applies no
  expression at runtime: a doctest that a conftest hook or plugin marks,
  and pytest deselects by that mark, runs. pytest's `-k` and `-m` from
  `addopts` or `PYTEST_ADDOPTS` don't apply to doctests either; the command
  line's do.
- **Not read**: conftests' `collect_ignore`, plugins, what a conftest's
  `pytest_plugins` loads, and a `setup.py` directory as the rootdir.
- **Conftests that skip.** One pytest loads at start-up that skips is a
  usage error (exit 4), where pytest stops with a traceback (exit 1). A
  skip aye can't see coming (a conftest above the project root, a skip in
  another module's function) skips the doctests below it one by one, where
  pytest counts one skip.
- **Modules are imported batch by batch**: pytest imports every module it
  collects before running a test.
- **`--import-mode=importlib`**: a module is imported as in pytest's
  default mode, so one outside any package is named from its file
  (`docs/helper.py::helper.g`, and `helper` is its `__name__`), where
  pytest names it from its path below the rootdir (`docs.helper.g`).
- **Warnings** go to the test's captured stderr; pytest keeps them for its
  summary.
- **Collection errors**: with `--doctest-modules`, pytest reports a test
  module that fails to import twice (as a test module and as a doctest
  module); aye reports it once when the module's text holds no `>>>`.
- **Not implemented yet**: rerunning one example, and `--accept`, which is
  to rewrite expected output in place.
