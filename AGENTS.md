# Working on aye

aye is a universal python test runner: a Rust control plane plus a Python execution plane, aye's engine,
which runs every test itself: pytest suites through aye's own `pytest` module and its pytest front-end (pytest is never imported),
unittest and doctest suites natively. Read `docs/architecture.md` before changing anything structural. Its "Core decisions" section is binding.

## Vocabulary

Use these words exactly, in code, docs, events, the command line and reports. The full definitions are in `crates/aye-engine/src/lib.rs`.

- **Runner**: aye, the Rust binary. Which tests run, where, when, in what order, under what limits; collecting and reporting results. It replaces every framework's own runner.
- **Engine**: aye's engine, in Python, inside workers. What a test is and how it executes: tests, lifetimes, setup and teardown order, outcomes. "Engine" alone always means aye's, and it runs every test: no framework's engine does.
- **Adapter**: maps one existing framework (pytest, unittest, doctest) onto aye's engine. Its front-end reads the framework's tests into aye's model and never runs them; for pytest, the test modules and conftests import aye's own `pytest` module, which stands in for pytest's authoring API.
- **Framework**: what people write tests against (pytest, unittest). aye does not ship one yet.
- **Library**: aye's importable features (patching, fakes, decorators, snapshots, test data), usable from any framework.
- **Worker**: a Python process managed by the runner, in which the engine executes batches.

The batch is the line: the runner owns everything outside a batch, the engine everything inside. Listing happens in workers too: the pytest front-end lists the session once per worker, and the import that lists a module is the one its tests run from. Until the crates are renamed, the `aye-engine-*` crates are the adapters.

## Layout

Leaf crates (depend on no aye crate):

- `crates/aye-index`: index newtypes (`newtype_index!`) and the collections they index (`IndexVec`, `IndexSlice`)
- `crates/aye-static`: `EnvVars`, every environment variable aye reads by name or sets, and `ProcessEnv`, the environment as aye read it
- `crates/aye-fs`: lexical path algebra (`normpath`, `normalize_absolute`, `absolute`, the relativizers, `expand_path`) and the state directory, `.aye` (`StateDir`, its buckets, the cache markers, `atomic_write`)

Runner crates (engine-agnostic; must never depend on an `aye-engine-*` crate):

- `crates/aye`: the binary (CLI, wiring, `aye config`, `aye show`, the adapter registry and which adapters a project runs)
- `crates/aye-events`: the wire types (events, outcomes and `OutcomeTrail`, failures, collected cases, `FrameworkStatus`, `StopReason`), the identities and paths they hold (`TestId`, `TestKey`, `AdapterName`, `LifetimeId`, `Scope`, `AffinityKey`, `RunId`, `RunNumber`, `WorkerIndex`, `BatchIndex`, `ProjectPath`, `DiagnosticPath`, `FramePath`), the envelope's `Stamper`, the diagnostic codes and their classification (`DiagnosticCode`, `ErrorClass`), and the schema version
- `crates/aye-engine`: the engine contract (traits, the Python-side executor protocol)
- `crates/aye-config`, `crates/aye-collect`, `crates/aye-select`: the project root and config loading with origins, discovery infrastructure (walk, parse, recognize, refine), selection
- `crates/aye-provision`: environments through a `Provisioner` (uv as a subprocess first; bring-your-own paths), the version matrix's supplier; design only so far
- `crates/aye-runner`: workers, warm daemon, scheduling, backends, services, transport, and the embedded worker
- `crates/aye-render`, `crates/aye-history`: consumers of the event stream (aye-history also keeps each run's log in `.aye/runs`, which `aye show` reads back through aye-render)

Adapter crates (one framework each, implementing `aye-engine`; only the binary links them):

- `crates/aye-engine-pytest`: aye's own `pytest` module, the pytest front-end and its executor (`python/native/`), standard library only; pytest's configuration and the static recognizer (`src/`); referee plain pytest
- `crates/aye-engine-unittest`: native, standard library only; referee `python -m unittest discover`
- `crates/aye-engine-doctest`: native, standard library only; referee pytest's doctest plugin

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

The comparison harness (`crates/aye/tests/compare.rs`) runs the binary on each suite in `crates/aye/tests/suites` and each project in `examples/`, and requires the ids, their order, the outcome per id and the exit status that the project's referee gives. `crates/aye/tests/harness/record.py` records them (`make expected` re-records every suite's `expected.json` and every example's `crates/aye/tests/examples/<name>.json`), choosing the referee from the project's `[tool.aye] adapters`: plain unittest (`expected_unittest.py`) when unittest is the only adapter, else plain pytest (`expected.py`), which also runs `TestCase` classes and, through its doctest plugin, doctests. aye runs on aye's engine there, with the referee's interpreter: `.venv` from `uv sync` by default, or set `AYE_TEST_PYTHON` to choose another (it has pytest, and aye's `pytest` module must win over it). The harness is skipped when no Python with pytest is available. Its ignored `native_suites` test runs every project on an interpreter without pytest (`AYE_NATIVE_PYTHON`) at any worker count (`AYE_NATIVE_WORKERS`) and prints a table. `make test` runs the harness with the rest of `cargo test --workspace`, then the Python tests under `crates/*/python/tests`.

All checks live in `.pre-commit-config.yaml`: rustfmt, clippy (pedantic and the workspace's restriction lints, with warnings as errors), ruff, cargo-deny (licenses, advisories and sources, set in `deny.toml`), `uv lock`, actionlint, file hygiene, and a Conventional Commits check on commit messages. The hooks, `make lint` and CI all run that one file, so to add a check, add it there.

Cargo manages the Rust dependencies (`Cargo.lock`), and uv manages the Python ones (`uv.lock`, dev group in `pyproject.toml`). Use `cargo add` and `uv add`; don't use pip.

## Rules

- Renderers see only `Event`s. If a renderer needs more, extend the schema (and `docs/events.md`). Never pass runner state around it.
- Static selection must stay a superset of the framework's. When unsure, return `Truth::Unknown`, which keeps the case; a recognizer that can't predict a file asks for refinement.
- The worker, aye's engine and every executor import only the standard library. The pytest front-end and its executor import aye's own `pytest` module, never pytest: nothing aye ships imports pytest.
- Schema changes that remove or rename a field bump `SCHEMA_VERSION`.
- `ruff_python_*` crates are pinned exactly. Bump them together, with the MSRV.
- Rust names an environment variable only through `aye_static::EnvVars`, and a variable aye starts reading or setting, in Rust or in the Python it sends to workers, joins the catalogue.
- A lint the code breaks on purpose gets `#[expect(lint, reason = "…")]`, not `#[allow]`.
- A new crate gets an entry in `release-please-config.json` and in `[workspace.dependencies]` between the `x-release-please` markers; CI checks the first.

## Releasing

Releases are automated with release-please, so commit messages must follow [Conventional Commits](https://www.conventionalcommits.org): `feat: …`, `fix: …`, `feat!: …` for breaking changes. While aye is below 1.0, `feat` and `fix` bump the patch version and breaking changes bump the minor.

On every push to `main`, release-please updates a release PR. The PR bumps the version (the lines marked `x-release-please` in `Cargo.toml`, plus `Cargo.lock`) and `CHANGELOG.md`. Merging the PR tags `vX.Y.Z`, and `.github/workflows/release.yml` builds the wheels and runs `make publish-crates` and `make publish-pypi`. Every crate and the wheel share one version, so never bump versions by hand.

Credentials come only from the `CARGO_REGISTRY_TOKEN` and `UV_PUBLISH_TOKEN` environment variables, set from repository secrets in CI. To publish by hand: `make dist`, then `CARGO_REGISTRY_TOKEN=… UV_PUBLISH_TOKEN=… make publish`.
