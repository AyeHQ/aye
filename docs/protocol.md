# Worker protocol (v3)

The runner (`crates/aye-runner`) and each worker talk over the worker's stdin
and stdout, one UTF-8 JSON object per line. The message types are in
`crates/aye-protocol` (`Request`, `Reply`, `WorkerLine`, `Batch`, `Policy`).
The worker is `crates/aye-runner/python/aye_worker.py`. The protocol version
is `PROTOCOL_VERSION` in `aye-protocol` and `PROTOCOL` in `aye_worker.py` and
`aye_engine.py`, and the handshake checks it.

## Starting a worker

Every worker goes through one spawn path, whether it runs batches or refines
files:

- The command is `python -c <source of aye_worker.py>`, with the interpreter
  aye found (see [compatibility.md](compatibility.md#configuration)). The
  source is embedded in the binary, so it can't skew from it.
- The worker starts in the directory aye was started in, with stdin, stdout
  and stderr piped, in its own process group, so that the terminal's Ctrl-C
  reaches only aye.
- Its environment is aye's, plus `PYTHONIOENCODING=utf-8` and
  `PYTEST_XDIST_TESTRUNUID=<the run's id>`, so pytest-xdist's `testrun_uid`
  is shared by a run's workers and new each run. A worker that runs batches
  also gets `AYE_WORKER=<n>`, and when a run has more than one lane,
  `PYTEST_XDIST_WORKER=gw<n>` and `PYTEST_XDIST_WORKER_COUNT=<count>`, so
  suites that key per-worker databases or ports off them keep working.

Before anything else, the worker keeps private copies of fds 0 and 1 for the
protocol, then points fd 1 at stderr and fd 0 at the null device, so a stray
`print()` or `input()` can neither corrupt nor consume the stream. The copies
are close-on-exec and sit high (from half the soft fd limit, at most 512),
out of the way of tests that use low fds (systemd's fd 3). A process forked
from the worker gets them pointed at the null device and its messages
dropped, so it can neither hold the stream open nor add records. The worker
leaves `sys.path` as `python -c` sets it, except that the leading `''` becomes
the absolute working directory, as under `python -m pytest`.

The runner drains stderr on a thread. It keeps the last 64 KiB for crash
reports and, with `-s`, echoes it live: to aye's stdout with text output, to
aye's stderr beside `--output json`, so JSON lines stay whole.
Once a worker has exited, the rest of its stderr is awaited for up to 1
second for a crash report, but after a clean shutdown only until it has been
silent for 100 ms: a subprocess the tests left running holds the stream open.

## Runner to worker (stdin)

Requests are tagged by `type`.

| `type` | Fields | |
|---|---|---|
| `hello` | `protocol`, `root` (the project root, absolute), `modules` [{`name`, `source`}], `executors` [{`adapter`, `entry`, `lists_session?`}], `services` [names] | Always first. |
| `run` | a batch: `id`, `adapter`, `ids`, `policy`, `context`, `ends_session?` | Run tests. With `ends_session`, the worker's last batch for its adapter: an executor that lists the session ends it with this batch (see [below](#batches-and-refinement)); with no `ids`, the batch only ends it. |
| `refine` | `adapter`, `files` (relative to the root), `context`, `digests_only?` | Import files and report their tests; with `digests_only`, only their digests. |
| `shutdown` | | Call each executor's optional `close()`, then end the worker. EOF does the same. A session no batch ended (a stopped run) ends there, with nothing to report into: what fails goes to stderr. |

`policy` says how a batch must run. Adapters honour `maxfail`, `capture` and
the selection; they may ignore the rest.

| Field | |
|---|---|
| `maxfail?` | Stop after this many bad outcomes: what is left of the run's budget. Absent means no limit (`--maxfail 0`). |
| `capture` | Capture output per test; `false` for `-s`. |
| `verbosity` | The count of `-v` minus the count of `-q`. |
| `timeout?` | Seconds per test, from `[tool.aye] timeout`. No adapter enforces it yet. |
| `keyword?`, `tags?` | The command line's `-k` and `-m` expressions. No adapter re-applies them: static selection is final for every case. |
| `framework_args?` | Everything after `--`, unchanged. An adapter may prefer its context's copy (the pytest adapter's front-end parses its context's `native.args`, where these come after `addopts` and `PYTEST_ADDOPTS`, filtered); the unittest adapter ignores them. Every adapter puts them in rerun commands: `aye <rerun_args> <id> -- <these>`, without `--junitxml`. |

`context` is the adapter's own, from its recognizer, on `run` and `refine`
alike. The binary and the runner add keys to every adapter's context, on
`run` only:

| Key | |
|---|---|
| `requested_ids` | Added by the binary when the command line names ids with parameters (`path::test[x]`), relative to the root. The executor reports such an id `not-found` when the framework doesn't make it, instead of running the test's other parameters in its place, as the pytest adapter does for ids aye worked out itself. |
| `rerun_args` | Added by the binary when there are some: aye's own options that shaped the run, as given (`["-c", "alt.ini"]` for `aye -c alt.ini`; never `--python`, the output's options or `--junitxml`), which an adapter puts before the id in a test's rerun command (`aye_engine.rerun_args(context)`). |
| `selected_paths` | Added by the binary when some positional argument isn't a path (a dotted name, a test key): the paths the user's run starts from, absolute and spelled from the root, in the order of the arguments, each once: each path argument (an id's file), and the files of the collected cases each other argument selects, as `pytest <these>` would be given them. The adapter's own context saw only the path arguments (with none, its framework's default), and refinement never gets the key. The pytest front-end starts from these instead of `roots` (a context that differs from the one it listed with starts a new session in the worker), and the doctest executor works out the conftests pytest loads at start-up from them. |
| `ran_ids` | Added by the runner to a batch it re-queued after its worker crashed: the tests that already ran, when an id that stands for several tests (a case without its parameters, pytest's stand-ins) may have run some of them. The adapters don't run them again; the unittest and doctest adapters count them, so an id listed n times skips its first n tests. |

The binary reads these keys from any adapter's context, when present:
`keyword` and `tags` (the framework's own `-k` and `-m`, applied statically to that
adapter's cases when the command line gives none), `deselect` (id prefixes
selection drops from that adapter's cases, counting them as deselected),
`maxfail`, `xdist_workers`, `junitxml`, `junit_prefix` and the `junit_*`
settings (the run's, when the command line gives none; from the first
context that gives each).

### The pytest adapter's context

| Key | |
|---|---|
| `rootdir`, `inifile` | pytest's, as aye found them (also in `native`, which the front-end reads first). |
| `roots` | The user's collection paths, absolute: aye's positional paths, else `testpaths` (when run from the rootdir), else the current directory. The front-end's session starts from them, with the paths the options name: pytest's start-up conftests (those it loads before `pytest_configure`) and its initial paths are theirs, so the other conftests load as collection reaches them, and pytest's ignore rules and `python_files` apply to requested files, as in the user's run. Without the key, pytest's own defaults. A batch whose context has `selected_paths` (the user also selected by dotted name or test key) starts from those instead, as `pytest <those paths>` would. |
| `xdist_workers` | The last `-n` of the three (`addopts`, `PYTEST_ADDOPTS`, the arguments after `--`), as text; the binary's default worker count. |
| `maxfail` | The last `-x` or `--maxfail N` of the three, or `null` (none, or 0); the binary's default `maxfail`. |
| `junitxml`, `junit_prefix` | The last JUnit report path (absolute, as pytest makes it) and prefix of the three, or `null`; the binary writes that report. |
| `junit_suite_name`, `junit_logging`, `junit_log_passing_tests`, `junit_duration_report`, `junit_family` | pytest's JUnit settings as `getini` gives them (the inifile, then `-o` from the three), with pytest's defaults (`pytest`, `no`, `true`, `total`, `xunit2`); `junit_log_passing_tests` is a bool. The binary writes the JUnit report by them, and by aye's own layout when no context gives them. |
| `keyword`, `tags` | The last `-k` and `-m` of the three, or `null`. The binary applies them statically when its command line gives none. |
| `deselect` | The `--deselect` prefixes of the three, as prefixes of aye's ids (none when pytest's rootdir is outside the project root). The binary drops the cases they name, except partial ones, and counts them as deselected. |
| `env_plugins` | The plugins `PYTEST_PLUGINS` names, as `-p NAME` arguments, so the refinement cache's rule for plugins loaded by name covers them. |
| `colliding_modules` | Names of helper modules that two test directories' import bases both hold, which pytest imports once: a refined module that imports one gets `import:NAME` as its first affinity key, as the recognizer gives static ones, so they share a batch. |
| `native` | What the front-end starts from: the resolved settings (`rootdir`, `inifile`, `confcutdir`, `pythonpath`, `python_files`, `python_classes`, `python_functions`, `testpaths`, `norecursedirs`, `usefixtures`, `markers`, `filterwarnings`, the strictness and id flags, `import_mode`, `plugins`, `env_plugins`), `ini` (every raw value of the configuration, for `getini`), `args` (pytest's options as the front-end parses them: `addopts`, `PYTEST_ADDOPTS`, then the arguments after `--`, without what aye takes over: pytest-xdist's, `-x`/`--exitfirst`/`--maxfail`, the JUnit ones and, when the doctest adapter runs, pytest's doctest options; the arguments after `--` also lose `-o addopts=…`), `options` (those parsed with pytest's dest names and defaults, for options conftests read), `unknown_options`, and `modules_digest` (of the executor's sources, so the refinement cache never answers for other sources). `aye-engine-pytest`'s `native.rs` has the details. |

Until October 2026 the context also carried what only a real pytest session
per batch read (`addopts`, `pytest_addopts_env` and `framework_args` as
separate filtered copies, `single_session`, `doctests`, `adds_items`); they
are gone with that executor.

### The unittest adapter's context

| Key | |
|---|---|
| `start_directory`, `top_level_directory` | Discovery's start and top-level directories, absolute (`discover -s` and `-t`, from `[tool.aye.unittest]`). |
| `pattern` | The file pattern (`discover -p`). |
| `starts` | More start directories, absolute: directories on the command line outside the configured start directory, each its own discovery. |
| `named` | Files named on the command line below a package that may define `load_tests`: each is loaded as a module alone, as `python -m unittest path.py` loads it. |

### The doctest adapter's context

| Key | |
|---|---|
| `rootdir`, `inifile` | pytest's, as the adapter found them (`inifile` may be `null`). |
| `optionflags` | The option flag names applied to every example: pytest's `doctest_optionflags` and `[tool.aye.doctest] optionflags`. |
| `encoding` | The encoding of text files. |
| `import_mode`, `consider_namespace_packages`, `pythonpath` | How pytest would import modules (`prepend`, `append` or `importlib`), and the directories `pythonpath` puts first on `sys.path`, absolute. |
| `report`, `ignore_import_errors`, `continue_on_failure` | `--doctest-report`, `--doctest-ignore-import-errors` and `--doctest-continue-on-failure`. |
| `filterwarnings`, `pythonwarnings` | pytest's `filterwarnings`, and its `-W` options, which win. |
| `conftests` | `{initial, confcutdir}`: the conftests pytest imports at start-up and the directory above which it imports none; `null` with `--noconftest`. Imported before the files below them (through aye's `pytest` module), with each directory's conftest from the top down. In a batch whose context has `selected_paths`, `initial` is worked out from those paths instead, as pytest's `_set_initial_conftests` does for the paths it is given (the recognizer saw only the path arguments). One of `initial` that fails, skips or calls `pytest.fail` while loaded is a `usage-error`; another that skips skips the files below it, and one that fails fails them (a `collection-error`). |
| `deselect`, `maxfail`, `junitxml`, `junit_prefix`, `junit_*` | As in the pytest adapter's context, from pytest's options; `junit_*` only those the configuration sets. |

## Worker to runner (stdout)

Each line is a `WorkerLine`: a control reply tagged by `reply`, or an event
record tagged by `event`.

| `reply` | Fields | |
|---|---|---|
| `ready` | `protocol`, `pid`, `python` (`sys.executable`), `version` (the first line of `sys.version`) | Answers `hello`. |
| `refined` | `cases`, `diagnostics`, `digests?` | Answers `refine`. `digests` (for an executor that lists the session, or to `digests_only`, which leaves `cases` empty): by requested file, a digest of its tests' ids in order, the executor's own or the SHA-1 of the ids joined by newlines. |
| `batch_done` | `batch`, `status?` | Ends a `run`. Every test in the batch has a `test_finished`, except those a `maxfail` stop or a framework stop cut off, those in files the framework ignores or fails to collect, ids it can't find (a `not-found` diagnostic), and those a conftest hook deselects. `status` is the framework's own (pytest's exit status; `python -m unittest`'s for the unittest adapter; 0, 1 or 2 for the doctest adapter), when it is an integer. |
| `failed` | `message` | The worker failed outside any test while handling a request; at most the last 8000 characters of the traceback. |

Records are the events of [events.md](events.md). The worker stamps `batch`
and `t_ms` (since the batch started); the runner adds `worker`, then `run`,
`seq` and the run's `t_ms` as it merges workers into the log.

## A session

```
→ {"type":"hello","protocol":3,"root":"/repo","modules":[{"name":"aye_engine","source":"…"},{"name":"aye_model","source":"…"},{"name":"aye_rewrite","source":"…"},{"name":"aye_native_engine","source":"…"},{"name":"pytest","source":"…"},{"name":"aye_pytest_frontend","source":"…"},{"name":"aye_pytest_native","source":"…"}],"executors":[{"adapter":"pytest","entry":"aye_pytest_native:NativeExecutor","lists_session":true}],"services":["context","cleanup"]}
← {"reply":"ready","protocol":3,"pid":95723,"python":"/repo/.venv/bin/python","version":"3.13.1 (main, Dec 19 2024, 14:22:59) [Clang 18.1.8 ]"}
→ {"type":"refine","adapter":"pytest","files":["tests/test_a.py"],"context":{"colliding_modules":[],"deselect":[],"env_plugins":[],"inifile":"/repo/pyproject.toml","junit_duration_report":"total","junit_family":"xunit2","junit_log_passing_tests":true,"junit_logging":"no","junit_prefix":null,"junit_suite_name":"pytest","junitxml":null,"keyword":null,"maxfail":null,"native":{"args":[],"…":"…"},"rootdir":"/repo","roots":["/repo"],"tags":null,"xdist_workers":null}}
← {"reply":"refined","cases":[{"id":"tests/test_a.py::test_three","adapter":"pytest","certainty":"refined","…":"…"}],"diagnostics":[],"digests":{"tests/test_a.py":"…"}}
→ {"type":"run","id":0,"adapter":"pytest","ids":["tests/test_a.py::test_three"],"policy":{"capture":true,"verbosity":0},"context":{"…":"the same context"},"ends_session":true}
← {"batch":0,"t_ms":22.995,"event":"test_started","id":"tests/test_a.py::test_three"}
← {"batch":0,"t_ms":23.315,"event":"test_finished","id":"tests/test_a.py::test_three","outcome":"passed","duration_ms":0.318}
← {"reply":"batch_done","batch":0,"status":0}
→ {"type":"shutdown"}
```

## Handshake

`hello` carries everything the worker runs: aye's engine first, then each
executor's modules, each once, in order. The worker installs every module
into `sys.modules` from memory, under the file name `<aye:name>`, so
tracebacks show its lines. It then builds the services named in `services`,
and one executor per entry (`module:Class`) with the root and the services.

- A worker of another protocol version answers `ready` with its own version
  and installs nothing. The runner kills it and refuses to run.
- A `hello` that fails (a module that doesn't import) is answered with
  `failed`, and the worker exits with status 1.
- Lines before `ready` that aren't protocol messages (a `sitecustomize` that
  prints) are skipped, and the last 20 are kept for the error if the worker
  exits instead.

A worker serves one adapter, so the runner sends one executor: that of
the adapter of the worker's first batch, or of the files a refinement worker
refines. A batch of another adapter gets a new worker.

| Adapter | Module | Entry |
|---|---|---|
| `pytest` | `aye_model`, `aye_rewrite`, `aye_native_engine`, `pytest` (aye's own), `aye_pytest_frontend`, `aye_pytest_native` | `aye_pytest_native:NativeExecutor` (lists the session) |
| `unittest` | `aye_model`, `aye_rewrite`, `aye_native_engine`, `pytest` (aye's own), `aye_engine_unittest` | `aye_engine_unittest:UnittestExecutor` |
| `doctest` | `aye_model`, `aye_rewrite`, `aye_native_engine`, `pytest` (aye's own), `aye_engine_doctest` | `aye_engine_doctest:DoctestExecutor` |

Every worker gets aye's `pytest` module before its executor's own, so what a
test, a conftest or the doctest adapter imports as `pytest` is aye's, even
where pytest is installed; pytest is never imported.

## Batches and refinement

A lane sends `run` and reads records until `batch_done`. The records are
passed into the log as they arrive. The worker stays up for the next batch;
the runner sends `shutdown` when no batch is left, and kills a worker that
hasn't exited 3 seconds later (2 seconds after its stream ended): a thread the
tests left running.

How a batch ends can stop the run:

- A batch whose records hold a `batch-stopped` error, or whose status is 2
  (pytest's interrupted session), stops the run: nothing more is handed out,
  batches already running finish and are reported, and the run counts as
  interrupted (`run_finished.stopped` is `framework`). The pytest adapter
  emits that error whenever the suite stops pytest's session (`pytest.exit`
  whatever its `returncode`, a `KeyboardInterrupt`, `session.shouldstop`, a
  `session.shouldfail` that isn't aye's `maxfail`), even in the batch's
  last test, so the run stops whatever the worker count.
- Status 3 (pytest's internal error) makes the run an internal error. It
  stops the run only with a `batch-stopped` error, and then the run is not
  counted as interrupted, so it exits 3, as pytest does. Without one (an
  internal error in `pytest_sessionfinish`, after every test ran) nothing
  stops.
- No other status means anything alone: `pytest.exit(returncode=4)` gives
  status 4, which is no usage error. The binary finds usage errors from
  `usage-error` diagnostics.
- `maxfail` is counted across workers: each batch is sent with what is left
  of the budget. When it runs out, nothing more is handed out, and busy
  workers end their batches, tearing down: a worker whose batch reached its
  own share stops by itself, and the others get SIGINT (a
  `KeyboardInterrupt` in the test in flight; killed on Windows) and are
  killed if still busy 10 seconds later.

Refinement starts a fresh worker holding only that adapter's executor, sends
one `refine` with all of the adapter's flagged files (for the pytest
adapter, every test file) and its context, reads `refined`, and shuts the
worker down, unless its executor lists the session (below). Its records are
not part of the run's log.

An executor that lists the session (`lists_session`: the pytest adapter's)
keeps what `refine` listed (the imported files, its model) for the
batches it runs later, as each pytest-xdist worker collects the whole
session:

- The worker that refined is kept, with the environment of a run's first
  lane (`AYE_WORKER=0`, and `gw0` with several workers), for the next
  refinement of that adapter and then as the run's first lane, or shut down
  when the runner is done (`--co`). A run that needs another environment for
  its first lane (fewer lanes than workers) shuts it down.
- With several workers and a run to follow (not `--co`), a worker for each
  other lane starts while the first lane's worker refines (with its lane's
  environment) and is sent the same `refine` with `digests_only`, so that
  it lists the files meanwhile; the run's lanes take these workers.
- Before its first `run`, every other worker of that adapter, a crashed
  one's replacement included, sends `refine` with `digests_only` and the
  session's files: every file collection found the adapter's tests in,
  selected or not, in collection order (a worker started during
  refinement, those of them it hasn't listed). The first lane lists what
  of them it hasn't. With several lanes, the others compare each file's
  digest with the first lane's: the tests of a file whose digest differs
  run on the first lane only, and a `listing-differs` warning names the
  file once. A worker that fails to list is a worker that failed to start.
- **The session ends in a batch**, as a pytest-xdist worker's ends with
  its last test, so that what fails there is reported. A batch handed out
  when nothing else of its adapter is left for its lane (and, for the
  first lane, when no other lane may still pin tests to it) has
  `ends_session`: the pytest adapter closes the session's lifetimes in its
  last test's teardown (a session-scoped fixture whose teardown fails
  fails that test, as in pytest), then runs `pytest_sessionfinish`,
  `pytest_unconfigure` and the cleanups. What fails there is a diagnostic
  (an `internal-error`, status 3), and a `session.exitstatus` that
  `pytest_sessionfinish` set to 1 is the batch's status, with a
  `session-failed` warning. A lane whose last batch didn't end its
  worker's session (it found nothing left only afterwards, or leaves the
  adapter) sends one with no ids, which only ends it: what the session's
  teardown fails then is a `lifetime_finished` of its own, with no test to
  blame, and fails the batch (status 1). Either way the teardown runs in a
  batch, never cut short by the shutdown's 3 seconds. A worker whose
  session ended runs no other batch: a crashed batch that comes back to
  its lane gets a new worker, which lists the session again. Not once the
  run is stopped (Ctrl-C): `close()` at shutdown ends the session then.
- `close()` tears down what lives as long as the session and no batch
  ended, once per worker.
- **With one lane**, batches run in plan order whatever their adapter (the
  doctests pytest lists before a file's tests run before them), and the
  worker that listed the session waits, idle, while another adapter's
  worker runs a batch, rather than listing the session again; it is
  announced (`worker_started`) when it first runs one.

A case that declares no affinity shares no lifetime with others: it goes
with its file's lifetime group if the file has one, and otherwise the
scheduler may split its file across batches by duration (with several
workers); a case with affinity keeps its group whole. The batches of an
executor that lists the session are handed out in plan order, so each worker
runs its tests in plan order, as pytest-xdist's do.

## Errors and crashes

- An unreadable request, an unknown `type` or a request before `hello` is
  answered with `failed`, and the worker carries on.
- An executor that raises outside any test is answered with `failed`. The
  runner emits a `batch-failed` diagnostic, the run ends with an internal
  error (exit 3), the batch's unfinished tests are not reported, and the
  worker goes on with the next batch.
- A line from the worker that isn't a `WorkerLine` is a `protocol-error`
  diagnostic, and the worker carries on. A line that looks like a reply but
  doesn't parse ends the batch.
- A worker that exits or reaches EOF in the middle of a batch has crashed.
  Its death is noticed by its pid as well as by the end of its stream, which
  a process its tests started may hold open. The test it was running
  finishes as `error` ("worker crashed while running this test"). The detail
  is its stderr: a fatal error's dump from its start (`Fatal Python error:`,
  the crashing thread first), else the last lines. The batch's tests that
  never started are re-queued once, as a new batch at the front of the
  queue, and are reported as errors if that batch crashes a worker too. A worker that dies outside any test is an internal
  error. The lane starts a new worker with the same index.
- When `maxfail` is reached, records after the bad outcome that reaches it
  are dropped, except the `lifetime_finished` records that close lifetimes
  the log holds open. When the run is stopped (Ctrl-C), busy workers are
  killed and records still arriving are dropped. The kill is SIGKILL, so
  the test in flight gets no `test_finished`, and its `finally` blocks and
  fixture teardown don't run.
- Killing a worker kills its process group, the worker and the processes
  its tests started, with `kill -9 -<pgid>` through `/bin/sh` (on Windows,
  `taskkill /F /T`). A group is signalled only while its worker is unreaped,
  so its id can't have been reused. What a worker that exited by itself left
  running is left alone, as pytest leaves it.
- A worker whose runner is gone (aye killed outright) finds the protocol
  stream broken at its next message, and kills its own process group. One
  that writes nothing (a hanging test) keeps running.

## Inside the worker

`aye_engine.py` is aye's engine: the objects every executor builds on.

- An executor subclasses `aye_engine.Executor`. The worker builds it as
  `Executor(root, services)` and calls `run(batch, sink)`, which returns the
  framework's status or `None`, and `refine(files, context)`, which returns
  `{"cases": [...], "diagnostics": [...]}` in the wire format (and may add
  `"digests"`). For `digests_only`, the worker calls `refine(files, context,
  digests_only=True)` when the executor's `refine` takes that argument, and
  drops the cases. At shutdown it calls the executor's `close()`, if any.
- `Sink` writes records: `test_started`, `test_finished`, `sub_outcome`,
  `lifetime_started`, `lifetime_finished`, `run_property` and
  `diagnostic`. It cleans them first (`aye_engine.sanitize`): lone
  surrogates become U+FFFD, and non-finite numbers are dropped from
  objects and become `null` in lists.
- Every line the worker writes is JSON a strict parser (serde) reads,
  whatever an executor hands it: NaN, infinities and integers beyond a
  double's range become `null`, keys JSON can't hold become text, and lone
  surrogates become U+FFFD. What can't be written at all (a cycle) raises in
  the executor, and nothing is written.
- `EngineExecutor` runs the loop for adapters that map fully onto the
  engine: it opens and closes `Lifetime`s as the order of tests requires and
  runs each `Test`'s setup, body and teardown. The doctest executor
  subclasses it. The unittest executor subclasses `Executor`, since
  unittest's result callbacks decide its outcomes, but uses the engine's
  `Lifetimes` (with `report_early` under `maxfail`), `Result` and
  `Failure`. The pytest adapter's executor (`aye_pytest_native`) subclasses
  `Executor` and hands its front-end's model to aye's engine
  (`aye_native_engine`), one session per worker.
- `aye_engine.captured` and `CaptureIO` capture a test's output without the
  capture service: UTF-8 streams with `.buffer`, `.encoding` and a real
  `fileno()`, still readable when a test closed them, which write what
  UTF-8 can't hold (a lone surrogate) as `?` (`errors="replace"`, as
  pytest's capture). The unittest and doctest executors use them.
- `aye_engine.reproduce(id, framework_args, rerun_args)` builds a test's
  `reproduce`: `aye`, the context's `rerun_args`, the id, then `--` and the
  arguments when there are any. Every executor sets it on a bad outcome.
- A failure's `detail` is the exception and its traceback as Python prints
  them, without the frames before the user's code (the worker's, the
  engine's, the framework's), as unittest leaves out its own.
- Durations come from `perf_counter`, bound when the engine and the
  executors are imported, so a test that replaces `time.perf_counter`
  changes no duration.
- Services are looked up by name (`Services.get` returns `None` when
  absent), so new services stay additive. The names are listed in
  `aye_protocol::services`. Today the runner asks for `context` (the test and
  lifetimes running now) and `cleanup` (teardown registered on the running
  test), and the engine provides both. The others (capture, coverage,
  timeouts, patching, …) are not provided yet.

## Rules

- `aye_worker.py`, aye's engine and every executor import only the
  standard library, so they run without pytest. The pytest front-end and
  its executor import aye's own `pytest` module, never pytest; so does
  what the doctest executor imports as `pytest` (`import_path` for the
  `importlib` import mode, the conftests it imports).
- Nothing in the worker writes to stdout, which belongs to the protocol.
- The worker, the engine and the executors run on Python 3.10 and later.
- `aye_worker.py` stays small: Windows limits the length of the command line
  that carries it.
