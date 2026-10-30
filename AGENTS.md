# Working on aye

aye is a universal python test runner: a Rust control plane plus a Python execution plane, aye's engine,
which runs every test itself: pytest suites through aye's own `pytest` module and its pytest front-end (pytest is never imported),
unittest and doctest suites natively. Read `docs/architecture.md` before changing anything structural. Its "Core decisions" section is binding.

## Vocabulary

Use these words exactly, in code, docs, events, the command line and reports. The full definitions are in `crates/aye-engine/src/lib.rs`.

- **Runner**: aye, the Rust binary. Which tests run, where, when, in what order, under what limits; collecting and reporting results. It replaces every framework's own runner.
- **Engine**: aye's engine, in Python, inside workers. What a test is and how it executes: tests, lifetimes, setup and teardown order, outcomes. "Engine" alone always means aye's, and it runs every test: no framework's engine does. It lives in `aye-engine`.
- **Adapter**: maps one existing framework (pytest, unittest, doctest) onto aye's engine. Its front-end reads the framework's tests into aye's model and never runs them; for pytest, the test modules and conftests import aye's own `pytest` module, which stands in for pytest's authoring API. Each adapter is an `aye-adapter-*` crate, implementing `aye-adapter`.
- **Framework**: what people write tests against (pytest, unittest). aye does not ship one yet.
- **Library**: aye's importable features (patching, fakes, decorators, snapshots, test data), usable from any framework.
- **Worker**: a Python process managed by the runner, in which the engine executes batches.

The batch is the line: the runner owns everything outside a batch, the engine everything inside. Listing happens in workers too: the pytest front-end lists the session once per worker, and the import that lists a module is the one its tests run from.

## Layout

Leaf crates (depend on no aye crate):

- `crates/aye-index`: index newtypes (`newtype_index!`) and the collections they index (`IndexVec`, `IndexSlice`)
- `crates/aye-static`: `EnvVars`, every environment variable aye reads by name or sets, and `ProcessEnv`, the environment as aye read it, once, in the binary
- `crates/aye-fs`: lexical path algebra (`normpath`, `normalize_absolute`, `absolute`, the relativizers, `expand_path`) and the state directory, `.aye` (`StateDir`, its buckets, the cache markers, `atomic_write`)

Runner crates (engine-agnostic; must never depend on an `aye-adapter-*` crate):

- `crates/aye`: the binary, a library whose `main` `main.rs` calls (the command line as clap reads it, `cli/`; the environment it reads once, `EnvironmentOptions`; each command's settings, resolved once in `settings/`; its positional arguments, each read as a path, an id or a word, `Positional`s, in `positional.rs`; the commands in `commands/`: the run, in `commands/run/` (collect, reach, select, plan, the stop signals, and `RunVerdict`, which decides its exit code), `aye config` and `aye show`, in `commands/show/`; a run's event stream, `stream/`, the passes that rewrite its records and the sink they go to; `AyeError`, what ends it before a run's stream starts; the `Printer`, through which it writes everything beside the stream; the adapter registry, each adapter's `AdapterTraits`, which adapters a project runs, and the prelude every worker installs: aye's `pytest` module and what it stands on)
- `crates/aye-events`: the wire types (events, outcomes and `OutcomeTrail`, failures with `ComparisonOp`, `CompareKind` and `FailureSignature`, `RerunCommand`, collected cases, `FrameworkStatus`, `StopReason`), the identities and paths they hold (`TestId`, `TestKey`, `AdapterName`, `LifetimeId`, `Scope`, `AffinityKey`, `RunId`, `RunNumber`, `WorkerIndex`, `BatchIndex`, `ProjectPath`, `DiagnosticPath`, `FramePath`), the envelope's `Stamper`, the diagnostic codes and their classification (`DiagnosticCode`, `ErrorClass`), FNV-1a (`Fnv64`, `Fnv128`) and `ContentDigest`, and the schema version
- `crates/aye-protocol`: the worker protocol (`Request`, `Reply`, `WorkerLine`, `Batch`, `Policy`, `BatchContext` over the adapter's `AdapterContext`, `MaxFail`, refinement's messages, the handshake's `ModuleSource`s in `ModuleSet`s, an adapter's runtime face `Runtime` with its `EntryPoint` and `SessionMode`, `ServiceName`, `PROTOCOL_VERSION`, the `Refiner` trait); depends on `aye-events` only, so the runner compiles no Ruff
- `crates/aye-adapter`: what an adapter implements (`Adapter` with its `runtime()`, `Recognizer`, `Claim`, `Requested`, `Recognition`, `RefineReason`, `SourceFile`, `Project`, `Setting`, `FrameworkDefaults`, `AdapterError`) and the Ruff re-exports collection and the adapters share
- `crates/aye-config`, `crates/aye-collect`, `crates/aye-select`: the project root and config loading (`[tool.aye]` read as `AyeOptions` and resolved into typed `AyeSettings`, each value `Sourced` with its `ValueSource`), discovery (what a run collects from, `Targets`; the walk into a table of files, `walk/`, each a `FileId`, with what each recognizer, an `AdapterId`, claims of it; parse and recognize; refine, `refine/`, with the refinement cache, its key typed part by part and `Cacheability`; the merge, with collection's own `CollectDiagnostic`s, and each adapter's session files), selection
- `crates/aye-provision`: the interpreter (`PythonRequest` from a `RequestSource`, discovery, the `Interpreter` and its `PythonSource`) and the refinement cache's `EnvironmentFingerprint` (its venv layout and `InstalledModules`); later, environments through a `Provisioner` (uv as a subprocess first; bring-your-own paths), the version matrix's supplier
- `crates/aye-runner`: workers, warm daemon, scheduling, backends, services, transport, and the embedded worker
- `crates/aye-stream`: the folds over the event stream that aren't presentation, which the renderers, the run store and `aye show` share (`Consumer`, `Tee` and its error policies, `Keys`, `Tally`, held sub-outcomes, blocked tests, the first failure of each signature, the diagnostics seen, definition lines, the run's facts, a stored run's `RunIndex`, `FileChange`)
- `crates/aye-render`, `crates/aye-history`: consumers of the event stream (aye-render's text, JSON lines, JUnit and GitHub renderers, presentation only, over aye-stream's folds, each a module tree by concept, with what they say of a failure alike in `failure/` and their words in `words/`; the detail each shows at a `Level`, a `Ladder` per format; what a renderer is fed, a `RenderMode`; aye-history keeps `.aye/history.json` in `history/` and each run's log in `.aye/runs` in `store/`, where its writer says where the run is with a `WriterState` and folds the summary's counts itself, and `aye show --run` names a run with a `RunSelector`; `aye show` reads a run back through aye-stream's `RunIndex` and shows it through aye-render)

The engine crate (what workers run; the runner embeds it):

- `crates/aye-engine`: aye's engine, in Python (`python/`: `aye_engine.py`, the model, the assertion rewriter, the native engine), standard library only, embedded in the binary (`ENGINE`, which every worker installs first); and the vocabulary's full definitions

Adapter crates (one framework each, implementing `aye-adapter`; only the binary links them):

- `crates/aye-adapter-pytest`: aye's own `pytest` module, the pytest front-end and its executor (`python/native/`), standard library only; pytest's configuration and the static recognizer (`src/`); referee plain pytest
- `crates/aye-adapter-unittest`: native, standard library only; referee `python -m unittest discover`
- `crates/aye-adapter-doctest`: native, standard library only; referee pytest's doctest plugin

Test support (not published; each crate names it by path, as a dev-dependency):

- `crates/aye-test`: what the crates' tests share: the interpreter they run (`test_python`), aye run in a hermetic environment (`TestContext`), what snapshots of its output leave out, its output read back, and fixtures

Each crate's `lib.rs` states its responsibility, boundaries and dependencies; read it before adding code there.

`examples/` holds a small project per framework (pytest, unittest, doctest), each passing under aye and under its own runner.

Crate names use hyphens (`aye-events`), never underscores.

## Commands

```sh
make hooks   # once: install the pre-commit and commit-msg hooks
make test    # uv sync + cargo test --workspace + the Python tests (pytest)
make bench   # benchmarks (divan); back performance claims with numbers
make bench-suite PROJECT=dir  # time a project's suite under pytest and aye (hyperfine)
make lint    # every check in .pre-commit-config.yaml, on all files
make dist    # sdist + wheel (maturin backend) into dist/
make         # list all targets
```

Unit tests sit beside the code they test, in a module's `tests.rs` or its inline `mod tests`. The binary's end-to-end tests are `crates/aye`'s test targets: `tests/cli/` (`main.rs` holds the helpers the themes share, and each theme is a module: arguments, selection, output, junit, reruns, maxfail, signals, collection_errors, usage_errors, adapters, sessions, engine, config), `tests/show/` (`aye show` and the run store: store, list, details, command_line), the comparison harness (`tests/compare.rs`), and three guards: `tests/snapshots.rs` (insta snapshots, recorded with aye 0.0.1, in `tests/snapshots/`, of the help, `aye config`, each output format at each verbosity and the usage errors), `tests/contracts.rs` (the Python names user code sees, recorded with aye 0.0.1) and `tests/constants.rs` (the constants Python and Rust must agree on). What tests share lives in `crates/aye-test`: the interpreter they run (`AYE_TEST_PYTHON`, else the workspace's `.venv`, with pytest where a test needs it; without one, a test passes without running and says so on stderr), aye run in a hermetic environment, the snapshots' filters, aye's output read back, and fixtures. `crates/aye/tests/common/` holds what the binary's tests read of the run store. The adapters' harnesses, which drive their recognizers, stay in their own crates. A snapshot changes only with the behaviour it pins: never re-record one to make a test pass.

The comparison harness (`crates/aye/tests/compare.rs`) runs the binary on each suite in `crates/aye/tests/suites` and each project in `examples/`, and requires the ids, their order, the outcome per id and the exit status that the project's referee gives. `crates/aye/tests/harness/record.py` records them (`make expected` re-records every suite's `expected.json` and every example's `crates/aye/tests/examples/<name>.json`), choosing the referee from the project's `[tool.aye] adapters`: plain unittest (`expected_unittest.py`) when unittest is the only adapter, else plain pytest (`expected.py`), which also runs `TestCase` classes and, through its doctest plugin, doctests. aye runs on aye's engine there, with the referee's interpreter: `.venv` from `uv sync` by default, or set `AYE_TEST_PYTHON` to choose another (it has pytest, and aye's `pytest` module must win over it). The harness is skipped when no Python with pytest is available. Its ignored `native_suites` test runs every project on an interpreter without pytest (`AYE_NATIVE_PYTHON`) at any worker count (`AYE_NATIVE_WORKERS`) and prints a table. `make test` runs the harness with the rest of `cargo test --workspace`, then the Python tests under `crates/*/python/tests`.

All checks live in `.pre-commit-config.yaml`: rustfmt, clippy (pedantic and the workspace's restriction lints, with warnings as errors), ruff, cargo-deny (licenses, advisories and sources, set in `deny.toml`), `uv lock`, actionlint, file hygiene, and a Conventional Commits check on commit messages. The hooks, `make lint` and CI all run that one file, so to add a check, add it there.

Cargo manages the Rust dependencies (`Cargo.lock`), and uv manages the Python ones (`uv.lock`, dev group in `pyproject.toml`). Use `cargo add` and `uv add`; don't use pip.

## Rules

- Renderers see only `Event`s. If a renderer needs more, extend the schema (and `docs/events.md`). Never pass runner state around it.
- Static selection must stay a superset of the framework's. When unsure, return `Truth::Unknown`, which keeps the case; a recognizer that can't predict a file asks for refinement.
- The worker, aye's engine and every executor import only the standard library. The pytest front-end and its executor import aye's own `pytest` module, never pytest: nothing aye ships imports pytest.
- Schema changes that remove or rename a field bump `SCHEMA_VERSION`.
- `ruff_python_*` crates are pinned exactly. Bump them together, with the MSRV.
- Rust names an environment variable only through `aye_static::EnvVars`, and a variable aye starts reading or setting, in Rust or in the Python it sends to workers, joins the catalogue. The binary reads the environment once (`EnvironmentOptions`, over `ProcessEnv::from_process`) and hands it down (an adapter reads `Project.env`, the refinement cache's key `EnvironmentFingerprint.variables`); `clippy.toml` disallows `std::env::var` and its kin everywhere else, but for the tests' own settings, each with an `#[expect]`.
- A lint the code breaks on purpose gets `#[expect(lint, reason = "…")]`, not `#[allow]`.
- A new crate gets an entry in `release-please-config.json` and, if it is published, in `[workspace.dependencies]` between the `x-release-please` markers; CI checks the first. A crate that isn't published (`aye-test`) is named by path, as a dev-dependency, which `cargo publish` strips.

## Releasing

Releases are automated with release-please, so commit messages must follow [Conventional Commits](https://www.conventionalcommits.org): `feat: …`, `fix: …`, `feat!: …` for breaking changes. While aye is below 1.0, `feat` and `fix` bump the patch version and breaking changes bump the minor.

On every push to `main`, release-please updates a release PR. The PR bumps the version (the lines marked `x-release-please` in `Cargo.toml`, plus `Cargo.lock`) and `CHANGELOG.md`. Merging the PR tags `vX.Y.Z`, and `.github/workflows/release.yml` builds the wheels and runs `make publish-crates` and `make publish-pypi`. Every crate and the wheel share one version, so never bump versions by hand.

Credentials come only from the `CARGO_REGISTRY_TOKEN` and `UV_PUBLISH_TOKEN` environment variables, set from repository secrets in CI. To publish by hand: `make dist`, then `CARGO_REGISTRY_TOKEN=… UV_PUBLISH_TOKEN=… make publish`.
