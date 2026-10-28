# Reorganising aye before 0.1.0

This is the plan for reorganising the 0.0.1 code before 0.1.0: which crates
exist and what each owns, what each core type means and how the types
relate, which module holds what, and the order in which the code moves. It
is for review: no code moves until it is approved.

It is written against the 0.0.1 tree imported unchanged into this
repository (`fcd8ee9`). Code is cited as `crate/path.rs:line` (`crates/`
left out); Python files by name, which are unique. Section 10 lists the
bugs and gaps found while mapping the code, and section 11 the questions
only RJ can settle, each with a recommended answer.

**Behaviour does not change.** The command line, the output in every
format, the exit codes, the event log, the stored runs and the comparison
harness's results stay as they are, except where §8.2 proposes a change and
says what it bumps. Two checks hold at every step of §9: the workspace's
tests (`cargo test --workspace`, and the Python tests: 932 passed, 2
skipped), and the behavioural oracle (`.context/refactor/oracle/compare_binaries.py`),
which runs the 0.0.1 binary and the new one on the 3 examples and the 9
harness suites with 15 argument sets each, and diffs exit codes, stdout,
stderr, the stored log and `aye show`. Run against a copy of itself it
reports 180/180 identical. The fixes in §10 land after the move, each as
its own `fix:` commit, except two the new structure removes by
construction (findings 1 and 5), which land as their own `fix:` commits
inside the step that makes them. Both change behaviour, so §11 (Q12) puts
them to RJ.

The patterns come from ruff, ty and uv where they fit aye; §2.2 says where
they don't. References like `ruff/crates/…` and `uv/crates/…` are paths in
the clones under `.context/ref`. Their code is MIT (uv's MIT OR
Apache-2.0): aye re-implements the patterns and copies no code.

## 1. Why

1. **Structured strings, parsed and classified in many places.**
   - Test ids are taken apart in about 25 places, Rust and Python, with
     three rules for stripping the parameters. `TestId::without_params`
     (`aye-events/src/lib.rs:122-127`) scans the whole id for `[`, so it
     gets `tests/t[1].py::test_x[a]` wrong; `split_params`
     (`aye-select/src/selector.rs:312-317`) and `unparametrized`
     (`aye_pytest_native.py:758-762`) scan the names part, and differ on a
     missing `]`.
   - The 25 diagnostic codes are bare strings, declared in three places
     (`aye-collect/src/lib.rs:200-218`, `aye/src/session.rs:37-51`,
     `aye-runner/src/run.rs:39,51`) and classified three times, three ways:
     for the exit code (`aye/src/session.rs:1186-1197`), for the summary
     (`aye-render/src/tally.rs:240-266`) and for the run store
     (`aye-history/src/runs.rs:513-526`).
   - Affinity keys and lifetime ids are prefix-encoded (`module:`,
     `class:`, `import:`, `package:`, a bare path, the literal `session`),
     and the adapters spell one lifetime differently.
   - A run has five identities: the envelope's hex `run`, `run_number`,
     `r81` in the store, `bad_since` and `since`. Adapter names are
     `String` or `&str` in about 20 places.
2. **Untyped JSON carries Rust-to-Rust data.** `Recognizer::context()`
   returns a `serde_json::Value` (`aye-engine/src/lib.rs:419`). The binary
   reads 12 keys out of it by name (`xdist_workers`, `maxfail`,
   `junitxml`, `junit_prefix`, five `junit_*`, `keyword`, `tags`,
   `deselect`; `aye/src/session.rs:215-220, 1416-1426, 1573-1714`),
   rebuilding the pytest adapter's whole context on each lookup
   (`context_strings`, `session.rs:1720-1732`). The binary and the runner
   splice four keys of their own into the adapter's object
   (`session.rs:1536-1572`, `aye-runner/src/run.rs:1940-1950`). About half
   of what the pytest adapter puts in its context is read by no one: the
   355-line option table of `aye-engine-pytest/src/native.rs:568-923`
   feeds a `native.options` the front-end shadows
   (`aye_pytest_frontend.py:3198-3202`).
3. **God modules.**

   | File | Lines | What it holds |
   |---|---|---|
   | `aye-engine-pytest/src/scope.rs` | 6229 | namespaces, escape analysis, name resolution, constant evaluation, a regex checker, version tables: nothing about pytest's scopes |
   | `aye-engine-pytest/src/{module,support,config,lib}.rs` | 3217, 2709, 2298, 2102 | collectors and fixture closures; conftest summaries, hook tables and an import resolver; two configuration readers; 450 lines of decisions and the session model |
   | `aye-engine-unittest/src/module.rs` | 2878 | tables, the statement walker, a project model with its own parse cache, C3 |
   | `aye-runner/src/run.rs` | 2548 | eight subsystems; the lane loop is a state machine in six local variables |
   | `aye/src/session.rs` | 2146 | settings, the whole run in one 393-line function (282-674), record passes, exit codes, signals |
   | `aye-render/src/{text,shared}.rs` | 2160, 1620 | the text format; 13 unrelated concepts |
   | `aye_pytest_frontend.py` | 5284 | one class of 2260 lines and 49 attributes |
   | `aye_native_engine.py` | 4179 | the engine, and pytest's runtime API (half the file) |
   | `aye_pytest_shim.py`, `aye_engine.py` | 3191, 2100 | a dozen libraries; the engine's base, its wire and its failures |
4. **Duplicated code that has drifted.** Three `fnmatch`s, two of which
   disagree with CPython and one of which is exponential; pytest's
   configuration read twice in Rust and again in Python; four path
   normalizers with two semantics; five FNV-1a copies; five safe-repr
   functions; three warning-filter grammars; four implementations of
   pytest's initial conftests; every fold written again in each renderer
   (§6).
5. **Settings resolved in pieces.** `Session` (`aye/src/session.rs:83`)
   is both the resolved run and its driver. It keeps the raw `Args` and
   `AyeConfig` and recomputes `output()` three times, and `junit()`,
   `policy()` and `maxfail()` on demand (`session.rs:676-760`). Where a
   value came from has five representations: `aye_config::Origin`,
   `Setting.origin: String`, `aye_runner::PythonSource`, `registry::Why`
   and `&'static str` (`session.rs:1830-1838`). `aye show` copies the root
   discovery (`aye/src/show.rs:112-121` from `session.rs:128-137`).
6. **Crate names and edges contradict the vocabulary.** `aye-engine` is
   the adapter contract, not the engine, and the `aye-engine-*` crates are
   adapters. The runner compiles Ruff's AST because the protocol shares a
   crate with the recognizer traits (`aye-engine/src/lib.rs:179-181`).
   `aye-provision` holds no code, and two crates depend on it.
   `aye-collect` depends on `aye-history` for one function
   (`aye-collect/src/refine.rs:394`). Wire values live in their consumers:
   `StopReason` in the runner (`aye-runner/src/run.rs:161-193`), outcome
   letters in the history (`aye-history/src/lib.rs:251-269`), lifetime-id
   parsing in the renderer (`aye-render/src/shared.rs:466-479`), pytest's
   statuses and progress letters in the runner (`run.rs:40-46`,
   `aye-runner/src/worker.rs:801-808`). The adapters' text rules in the
   renderer (pytest's private fixture names, `shared.rs:229-240`, and the
   others of the `render` map §7) are a different case: stored 0.0.1 logs
   carry nothing else to read them from, so they stay for 0.1.0 (Q13).
7. **Implicit state and identity.** The run's phase is five flags in four
   structs; three batch structs are copied field by field in five places
   (`aye-runner/src/schedule.rs:414-612`); `RunWriter` is a state machine
   in four `Option`s (`aye-history/src/runs.rs:193-207`). Relations go
   through parallel vectors zipped by position (`aye-collect/src/lib.rs:730`),
   raw indices (`aye/src/show.rs:636-678`), pointer addresses
   (`aye-engine-pytest/src/support.rs:1793, 1885`; `Arc::ptr_eq` in
   `aye-engine-unittest/src/module.rs:2564`) and formatted strings
   (`"local:<offset>"`, `aye-engine-pytest/src/module.rs:331-343`).
8. **The Python half is flat and order-dependent.** Nine top-level modules
   are installed in an order that matters but is written down only as Rust
   arrays (`aye/src/registry.rs:44-59`, `aye-engine-pytest/src/native.rs:69-76`);
   the binary wraps two adapters to give their workers aye's `pytest`
   module (`WithAyePytest`, `registry.rs:61-96`). The worker can't install
   a package and is 33 bytes under its 16,000-byte cap
   (`aye-runner/src/lib.rs:262`). Each executor builds its records by hand:
   three case builders, seven diagnostic builders.

## 2. Principles

### 2.1 Adopted

| # | Pattern | Reference | Replaces in aye |
|---|---|---|---|
| 1 | Validated newtypes: private field, `as_str`, `Display`, `FromStr` where parsing means something, serde as the same string or number | `uv/crates/uv-normalize/src/package_name.rs`; `ruff/crates/ty_module_resolver/src/module_name.rs` | `TestId(pub String)`, `TestKey(pub String)`, adapter names, paths, run numbers (§4) |
| 2 | Codes as enums with `as_str` and one documented variant each | `ruff/crates/ruff_db/src/diagnostic/mod.rs:998, 1149` (`DiagnosticId`) | diagnostic codes, `stopped`, scopes, lifetime kinds, `Compare.kind`, `Assertion.op` |
| 3 | An unknown arm, `Other(Box<str>)`, on every enum read from a worker or a stored log | aye's own: ruff never reads its diagnostics back | `RunStore::log` drops a line that fails to parse (`aye-history/src/runs.rs:646-656`) |
| 4 | Index newtypes and `IndexVec`, written as aye's own `macro_rules!` | `ruff/crates/ruff_index/src/{idx,vec,slice}.rs`, `ruff_macros/src/newtype_index.rs` | `WorkerId = u32`, `BatchId = u32`, collection's `usize` file and recognizer indices, `aye show`'s record positions |
| 5 | Options (as written) → Settings (resolved), each value with its source | `ruff/crates/ruff_ranged_value/src/lib.rs`; `ty_project/src/metadata/{options,settings}.rs`; `uv/crates/uv-cli/src/settings.rs:733-895` | `AyeSettings` read field by field; the five origin types |
| 6 | Clap types apart from logic; `resolve` destructures the args exhaustively | `uv/crates/uv-cli/src/lib.rs` vs `settings.rs`; `ruff/crates/ruff/src/args.rs:800-858` | `Args::verbosity/framework_verbosity/maxfail` (`aye/src/args.rs:147-168`), the pytest-argv readers in `args.rs:330-579` |
| 7 | One catalogue of environment variables | `uv/crates/uv-static/src/env_vars.rs`; `ruff/crates/ty_static/src/env_vars.rs` | literal names in six crates (`PYTEST_ADDOPTS` alone at 14 sites) |
| 8 | One exit-status decision, a pure function of typed facts | `ruff/crates/ty/src/lib.rs:257-285, 727-759`; `ruff/crates/ruff/src/lib.rs:38-54` | ten exit-code sites, the JUnit override buried in `session.rs:1288-1290` |
| 9 | A printer for output outside the event stream | `ruff/crates/ty/src/printer.rs`; `uv/crates/uv-command-support/src/printer.rs` | `aye/src/busy.rs`; `eprintln!("aye: error: …")` scattered over `main.rs`, `session.rs` and `show.rs`; `aye show --runs` printed by `show.rs:308-395` |
| 10 | Thin `main.rs`; `lib.rs::main`; `commands/`, one module per subcommand | `ruff/crates/ruff/src/main.rs:30-78`, `commands/mod.rs`; `uv/crates/uv/src/bin/uv.rs` | `aye/src/main.rs` (237 lines of notes, hand dispatch), `session.rs`, `show.rs`, `config_cmd.rs` |
| 11 | A shared test context, hermetic environment and output filters | `uv/crates/uv-test/src/lib.rs`; `ruff/crates/ty/tests/cli/main.rs` + `tests/common/mod.rs` | helpers copied in `aye/tests/{cli,show,compare}.rs`; six copies of "find a test Python" |
| 12 | A typed state directory with named buckets | `uv/crates/uv-state/src/lib.rs`; `uv-cache/src/lib.rs:1044` | the `.aye` layout spread over three crates |
| 13 | A cache key built from types | `ruff/crates/ruff_cache/src/cache_key.rs:15-80`; `uv/crates/uv-cache-key` | the refinement key hashed from display strings (`aye-collect/src/refine.rs:312-347`) |
| 14 | A portable project-relative path type | `uv/crates/uv-fs/src/path.rs:541-667`; `ruff/crates/ruff_db/src/system/path.rs` | `String` paths; `replace('\\', "/")` at 11 sites outside tests |
| 15 | What was requested, apart from what was found | `uv/crates/uv-python-types/src/request.rs:16-44, 173-198` | `PythonSource`, which mixes both (`aye-runner/src/python.rs:9-25`); `Positional` |
| 16 | A trait in a low crate breaks a cycle | `uv/crates/uv-types/src/traits.rs:92` | already aye's `Refiner`: kept |
| 17 | Typed, borrowed DTOs for outputs other than the event log | `ruff/crates/ruff_db/src/diagnostic/render/json.rs:51-250` | the `run` line hand-assembled in `show.rs:351-372` |
| 18 | A cancellation token pair | `ruff/crates/ruff_db/src/cancellation.rs` | bare `Arc<AtomicBool>` flags (`session.rs:324-332`) |
| 19 | Embedded files owned by one crate each, as an ordered set with tested invariants | `ruff/crates/ty_vendored/src/lib.rs` | three hand-maintained arrays of module order |
| 20 | `foo.rs` + `foo/`, no `mod.rs`; private submodules re-exported by the parent; tests beside the code | `ruff/crates/ty_project/src/metadata.rs` + `metadata/` | 2,000-line `src/tests.rs` files |
| 21 | `thiserror` in libraries, `anyhow` at the edge; matches that list every variant | `ty_project/src/metadata.rs:620-647`; `uv/crates/uv-install-operations/src/error.rs:59-85` | 16 `Result<_, String>` sites; usage versus internal decided by call site |
| 22 | Accepted no-op flags in a compat group | `uv/crates/uv-cli/src/compat.rs:6-120` | `--native` as a bare field (`aye/src/args.rs:114-117`). Unlike uv's (`#[clap(long, hide = true)]`, `compat.rs:17-38`), it stays visible and in its place: today's help lists it |
| 23 | Lints: `unreachable_pub`, `print_stdout`, `print_stderr`, `iter_over_hash_type`, `rest_pat_in_fully_bound_structs`, `exit`; `#[expect]` over `#[allow]` | `ruff/Cargo.toml`, `uv/Cargo.toml` `[workspace.lints]` | `pedantic` only |

### 2.2 Rejected

| Pattern | Reference | Why not |
|---|---|---|
| salsa, a `Db`, incremental queries | `ruff/crates/ruff_db/src/lib.rs` | a run is a one-shot pipeline; take interned ids and cheap handles, not the framework |
| Per-directory settings, `extend`, user and system config files, `--config KEY=VALUE` | `ruff_workspace/src/resolver.rs`; `uv-settings/src/lib.rs:33-218` | aye reads one `[tool.aye]`; each would change behaviour or the CLI |
| Proc-macros for `Combine`, `OptionsMetadata`, `CacheKey`, `newtype_index` | `ty_combine`, `ruff_macros` | tens of types, not hundreds: `macro_rules!` and hand-written impls |
| Printer quiet modes, `warn_user!` globals, tracing behind `-v`, multi-line `cause:`/`hint:` errors | `uv-command-support`, `uv-warnings`, `uv-errors` | `-q` and `-v` set the renderers' detail, as pytest's do; warnings about a run are events (Core decision 2); errors print `aye: error: …` on one line |
| Exit codes 0/1/2, 101, 130 | `ty/src/lib.rs:257-273` | aye's are pytest's 0-5 (Core decision 1) |
| Clap's required subcommand | `ruff/src/args.rs:115-135` | `aye` alone runs tests, and `show` or `config` is a subcommand only when no such path exists: a typed `Command` comes after the hand detection |
| `Diagnostic` as an opaque `Arc` builder without serde | `ruff_db/src/diagnostic/mod.rs:29` | aye's diagnostics are wire records that workers send and the store keeps |
| `TextRange`'s serde and panicking constructor on the wire | `ruff_text_size/src/serde_impls.rs` | a reversed span from a worker would become a `protocol-error`; `Span` keeps flat `start` and `end` |
| `repr(transparent)` borrowed newtypes over `Path` or `str` | `ruff_db/src/system/path.rs:12` | they need `unsafe`, which the workspace forbids; borrowed forms are small `Copy` views |
| One crate per command, a separate CLI crate | `uv-*-commands`, `uv-cli` | three commands: modules with `pub(crate)` suffice |
| `CWD` and `OnceLock` globals; a pre-spawn interpreter query; versioned bucket names | `uv-fs/src/path.rs:10`; `uv-python-interpreter`; `uv-cache/src/lib.rs:1296` | aye passes the cwd; the handshake is the query; renaming `.aye` entries would orphan users' history |
| Depending on ruff's HEAD crates | | MSRV 1.97 against aye's 1.94; `ruff_python_*` stay at `=0.0.4` |

### 2.3 House rules that follow

- A type with an invariant keeps its field private; a plain wire record
  keeps `pub` fields.
- Every type on the event or protocol wire serializes exactly as its
  0.0.1 counterpart; a golden test reads a 0.0.1 fixture and writes it
  back byte for byte.
- `BTreeMap` or an insertion-ordered map wherever iteration reaches
  output; hash maps only for lookups (`iter_over_hash_type` enforces it).
- The environment is read once, by the binary, as a whole
  (`EnvironmentOptions::vars`, a `ProcessEnv`: a case-preserving map whose
  lookup ignores case on Windows, as `os.environ` does), and handed down;
  `std::env::var` is a disallowed method elsewhere (`clippy.toml`), with
  the refinement fingerprint, which reads the whole environment on purpose,
  as the documented exception. `aye-static` catalogues every variable aye
  reads by name, and variable expansion is the documented exception to
  the catalogue: `--junitxml` (`aye/src/session.rs:1785-1826`) and pytest's
  `os.path.expandvars` port (`aye-engine-pytest/src/config.rs:771-791`, for
  `--rootdir` at `config.rs:373-374`) read any variable the user names, so
  they take the whole map, never a snapshot of catalogued names.
- Decision logs move from crate docs onto the items they decide
  (`aye/src/main.rs:1-237`, `aye-engine/src/lib.rs:81-171`,
  `aye-runner/src/lib.rs:39-227`); each `lib.rs` keeps its responsibility,
  boundaries and dependencies, as the 0.0.1 AGENTS.md's Rules ask. Those
  Rules, its Vocabulary, Layout and Commands did not come over with the
  import: this repository's AGENTS.md has only "Releasing". Step 1 ports
  them.

## 3. Target crate layout

### 3.1 Crates

| Crate | Responsibility | Moves in, from | Allowed aye dependencies |
|---|---|---|---|
| `aye-index` | Index newtypes (`newtype_index!`), `IndexVec`, `IndexSlice` | new | none |
| `aye-static` | `EnvVars`: every variable aye reads by name or sets, documented, with `ALL` for tests; `ProcessEnv`, the environment as the binary read it (case-preserving; lookup ignores case on Windows and refuses names with `=` or NUL, as `env_var` does) | literals in six crates; `env_var` (`aye-engine-pytest/src/config.rs:771-776`) | none |
| `aye-fs` | Lexical path algebra (`normpath` and `absolute`, both of today's semantics, named), `relative_to`, `common_ancestor`, path display, `expand_path` over a lookup it is given; the state directory (`StateDir`, `StateBucket`, `prepare_dir`, the `.gitignore` and `CACHEDIR.TAG` markers, `atomic_write`) | `aye-config/src/lib.rs:128-258`; `aye/src/session.rs:1785-1828, 1918-1981` (`expand_path`, `normalize`, the relativizers; `output()` at 1830, positional resolution at 1855-1914 and `repository_prefix` at 1983 stay in the binary); `aye-collect/src/tree.rs:294`; `aye-history/src/lib.rs:107-129, 509-580`, `runs.rs:747-761` | none |
| `aye-events` | The event log: identities, outcomes, failures, cases, diagnostics and their classification, events, `FrameworkStatus`, `Stamper`, digests, `SCHEMA_VERSION` | `aye-events/src/lib.rs`; `StopReason`, `Stamper`, `new_run_id` (`aye-runner/src/run.rs:161-341`); outcome letters (`aye-history/src/lib.rs:251-269`); `file_of` (`aye-render/src/shared.rs:466-479`); `is_project_path` (`shared.rs:887-897`); FNV-1a from three other crates | aye-index |
| `aye-protocol` | The worker protocol: requests, replies, `Batch`, `BatchContext`, `Policy`, `MaxFail`, refinement messages, `ModuleSource`, `ModuleSet`, `EntryPoint`, `SessionMode`, `Runtime`, `ServiceName`, `FrameworkStatus` (re-exported), the `Refiner` trait, `PROTOCOL_VERSION` | `aye-engine/src/lib.rs:183-195, 224-244, 424-616`; `Executor` as `Runtime` (`lib.rs:304-316`); `aye-runner/src/run.rs:40-48`; `aye/src/session.rs:53-78` | aye-events |
| `aye-config` | The project root, `[tool.aye]` (`AyeOptions` → `AyeSettings`), `ValueSource`, `Sourced<T>`, its value types | `aye-config/src/lib.rs:64-124, 333-580`; `aye/src/args.rs:584-667`; root discovery from `session.rs:128-137` and `show.rs:112-121` | aye-events, aye-fs, aye-static |
| `aye-adapter` | What an adapter implements: `Adapter` (with `runtime()`), `Recognizer`, `Claim`, `Requested`, `Recognition`, `RefineReason`, `ParsedFile`, `Project`, `Setting`, `FrameworkDefaults`, `AdapterError`; the Ruff re-exports and the `get-size2` pin that goes with them | `aye-engine/src/lib.rs:246-422`, without `Executor` | aye-events, aye-protocol, aye-config, aye-static |
| `aye-engine` | aye's engine: the embedded package `aye_engine` (`ENGINE: ModuleSet`), its Python tests, the tests that keep Python's constants equal to Rust's; the vocabulary | `aye-engine/python/*`; `aye-engine/src/lib.rs:1-41, 197-222` | aye-protocol |
| `aye-collect` | Discovery: walk, parse, recognize, refine and its cache, merge, collection diagnostics | as today; targets (`aye/src/session.rs:394-413`); session files (`session.rs:1488-1507`) | aye-adapter, aye-config, aye-events, aye-fs, aye-index, aye-protocol, aye-provision, aye-static |
| `aye-select` | Selection: three-valued logic, last failed, argument order, each adapter's own `-k`/`-m` applied to its cases | as today; the `--ff` order (`session.rs:566-568`). The policy that picks which `-k`/`-m` counts (`Pick`, `session.rs:1639-1781`) stays in the binary: it reads `aye-pytest`'s `ArgValue`, which no runner-side crate may name | aye-events, aye-index |
| `aye-runner` | Workers, transport, scheduling, `maxfail`, crash recovery, refinement on workers, the embedded worker | as today, without `python.rs` | aye-protocol, aye-engine, aye-events, aye-index, aye-static |
| `aye-provision` | The interpreter: the request, discovery, `Interpreter`, venv layout and installed modules for the refinement fingerprint; later the Step 5 environments | `aye-runner/src/python.rs`; `aye-collect/src/environment.rs:130-388` | aye-events (`ContentDigest`), aye-fs, aye-static |
| `aye-stream` | Folds over the log that aren't presentation: `Consumer`, `Tee` and its error policies, `Keys`, `Tally`, the sub-outcome buffer, blocked tests, first-by-signature, definition lines, run facts, `RunIndex`, `FileChange` | `aye-render/src/{lib.rs:183-267, keys.rs, tally.rs}`, `shared.rs:789-881`; `aye/src/session.rs:1328-1384`; `aye/src/show.rs:636-823`; `Change` (`aye-history/src/runs.rs:129`) | aye-events, aye-index |
| `aye-render` | Presentation only: text, JSON lines, JUnit XML, GitHub annotations, `aye show`'s list, `Level`, `DetailLevel`, `RenderMode`, and the words (durations, times, plurals) the binary's printer reuses | as today; `aye/src/show.rs:945-1000` | aye-stream, aye-events |
| `aye-history` | `.aye/history.json` (`history`) and the run store (`store`), which produces aye-stream's `FileChange` | as today | aye-stream, aye-events, aye-fs |
| `aye-python-stdlib` | CPython's library behaviour in Rust: `fnmatch`, `glob`, `os.path` (`expandvars`, `expanduser`, the NT forms, over a lookup they are given: `ProcessEnv::get`), reprs, `str.isdigit` and builtins per version, identifier and module-name rules, the stdlib module list, unittest's loader rules | the three `fnmatch.rs`; `aye-engine-pytest/src/{config.rs:771-930, ids.rs:201-460}`; `scope.rs` regex and tables; `aye-engine-unittest/src/module.rs:27-395, 2835-2860` | aye-fs |
| `aye-python-semantic` | What a module binds, read statically on Ruff's AST: module resolution in FileFinder order, package roots, qualified names through imports, namespaces and escape analysis, static values, C3, decorator and import-time facts | `aye-engine-pytest/src/scope.rs`; class model and body helpers of `module.rs`; `support.rs:190-390, 2390-2525`; the shared parts of unittest's `module.rs` and doctest's `finder.rs` | aye-python-stdlib, aye-index |
| `aye-pytest` | pytest 9.1.1 as a library for the adapters that need it: configuration discovery, ini values, argv reading and lifting (the binary's too), conftest rules, the doctest plugin's file rule, the adapter names pytest's and doctest's adapters test for (`names`); embedded Python: aye's `pytest` module (`PYTEST: ModuleSet`) and `aye_pytest` (import modes, conftests, warning filters, collection-error texts) | `aye-engine-pytest/src/config.rs`, `native.rs:400-523`; `aye-engine-doctest/src/{config.rs, conftest.rs}`; `aye/src/args.rs:330-579`; `aye-config/src/ini.rs`; `aye_pytest_shim.py`; the pytest rules of `aye_engine_doctest.py:259-591` and of the front-end | aye-adapter, aye-config, aye-events, aye-fs, aye-protocol, aye-static, aye-python-stdlib, aye-python-semantic |
| `aye-adapter-pytest` | The pytest adapter: recognizer, static predictor, typed context; Python `aye_adapter_pytest` (front-end, executor, pytest's runtime API) | `aye-engine-pytest` | aye-adapter, aye-engine, aye-events, aye-protocol, aye-config, aye-pytest, aye-python-stdlib, aye-python-semantic |
| `aye-adapter-unittest` | The unittest adapter | `aye-engine-unittest` | aye-adapter, aye-engine, aye-events, aye-protocol, aye-config, aye-python-stdlib, aye-python-semantic |
| `aye-adapter-doctest` | The doctest adapter | `aye-engine-doctest` | aye-adapter, aye-engine, aye-events, aye-protocol, aye-config, aye-pytest, aye-python-stdlib, aye-python-semantic |
| `aye` | The binary: CLI, settings, commands, registry, printer | `aye/src` | any |
| `aye-test` (`publish = false`) | `TestContext`, the test Python, the hermetic environment, output filters, shared fixtures | `aye/tests/{cli,show,compare}.rs` helpers; the test-Python lookups of the adapters' `src/tests.rs`. The adapters' harnesses stay in their crates: they build `ParsedFile`s and drive `Recognizer`s (`aye-engine-unittest/src/tests.rs:10`), and with `aye-adapter` here `aye-runner`'s tests would compile Ruff | aye-events, aye-fs, aye-static |

That is 23 crates, 22 of them published, from 13. Section 11 (Q1) lists
the merges that would bring it to 20 if RJ prefers fewer.

### 3.2 Dependencies

```
 6  aye
 5  aye-adapter-pytest    aye-adapter-unittest    aye-adapter-doctest
 4  aye-pytest            aye-collect             aye-runner
 3  aye-adapter           aye-engine              aye-render          aye-history
 2  aye-protocol  aye-config  aye-provision  aye-select  aye-stream  aye-python-semantic  (aye-test)
 1  aye-events            aye-python-stdlib
 0  aye-index             aye-static              aye-fs
    ───────────────────────────────────────────────────────────────────────
    adapter side: aye-python-stdlib, aye-python-semantic, aye-pytest, aye-adapter-*
    engine side:  aye-engine, which workers run; it shares aye-protocol and aye-events
    runner side:  every other crate but the binary
```

Arrows only point to lower lines, and each crate depends only on what its
row in §3.1 lists. The rules:

1. The leaves depend on no aye crate.
2. `aye-events` depends on serde and `aye-index`; `aye-protocol` on
   `aye-events`. Neither knows Ruff, a framework or the filesystem.
3. No runner-side or engine-side crate depends on an adapter-side crate
   (today's rule, kept and widened to the framework libraries). A
   runner-side crate may carry an adapter's name as data (an
   `AdapterName`, a `[tool.aye.<name>]` table looked up by name), but never
   branches on one.
4. Adapters don't depend on one another: what two share lives in
   `aye-pytest` or `aye-python-*`.
5. `aye-runner`, `aye-render`, `aye-history`, `aye-stream` and
   `aye-select` compile no Ruff.
6. Only the binary depends on clap.
7. The `aye-python-*` crates know no framework but the standard library's
   own unittest; pytest's knobs (`pytest.mark.*` being inert, what skips
   at import) are parameters.

### 3.3 The names

- "Engine" alone means aye's engine, so `aye-engine` holds it: the Python
  package and its embedding. The contract becomes two crates, `aye-adapter`
  (what adapters implement) and `aye-protocol` (what crosses to a worker).
  The `aye-engine-*` crates become `aye-adapter-pytest`,
  `aye-adapter-unittest` and `aye-adapter-doctest`, which ends the 0.0.1
  AGENTS.md's "until the crates are renamed" (its Vocabulary, which step 1
  ports, then drops the phrase).
- The Python package a crate embeds is named after the crate
  (`aye-adapter-doctest` embeds `aye_adapter_doctest`), except names that
  are contracts (`pytest`, §7.2).
- On crates.io, `aye-engine` 0.0.1 is the contract and 0.1.0 will be the
  engine, and the `aye-engine-*` 0.0.1 crates stay as published. Nothing
  outside the workspace depends on them (every crate pins `=0.0.1`, and
  users install the binary), so the breaking minor, 0.1.0, is where it
  belongs.

### 3.4 Publishing

`make publish-crates` runs `cargo publish --workspace --locked`, which
skips `publish = false` crates. New published crates join
`[workspace.dependencies]` between the `x-release-please` markers with `=`
versions, as today. `aye-test` does not: each crate names it as a bare
`path` dev-dependency (`aye-test = { path = "../aye-test" }`), which
`cargo publish` strips. With a version it would stay in the published
manifests, and crates.io rejects a dependency it doesn't host.

release-please bumps `Cargo.lock` through one JSONPath entry per crate in
`release-please-config.json` (`extra-files`, `$.package[?(@.name ==
'aye-engine-pytest')].version` and so on), which names exactly today's 13.
Every release build is `--locked` (`.github/workflows/release.yml:61`,
`Makefile:91`, `.github/workflows/ci.yml:35, 51`), so a crate without an
entry keeps its old version in `Cargo.lock` and the release fails. Every
step that adds or renames a crate (1, 3, 6, 7, 10) updates the entries in
the same commit: step 3 removes the three `aye-engine-*` entries and adds
the three `aye-adapter-*`, `aye-protocol` and `aye-adapter`, and each new
crate gets one, `aye-test` included, since the workspace sets its version
too. A CI check, added in step 1, fails when a workspace member has no
entry.

## 4. Core data model

Each concept: what it is today, the one new type, its home, its wire form,
and how it relates to the others. Wire forms are byte-identical unless
said. The relations in brief:

```
TestId ─path()─▶ ProjectPath ◀─built from─ AffinityKey ─unittest─▶ LifetimeId ─file()─▶ ProjectPath
  │ key()                                                              │ kind
  ▼                                                                    ▼
TestKey ◀─key()─ LifetimeId                        Resource.scope ──▶ Scope
Case { id: TestId, adapter: AdapterName, span: Span { path: ProjectPath }, affinity: [AffinityKey] }
Frame { path: FramePath::{Project(ProjectPath), External} }      Failure { signature: Option<FailureSignature> }
Envelope { run: RunId, worker: WorkerIndex, batch: BatchIndex } ─▶ DiagnosticOrigin ─┐
Diagnostic { code: DiagnosticCode, path: DiagnosticPath } ─────────────────────────────┴─▶ ErrorClass
RunStarted.run_number = Previous.since = TestStats.bad_since = StoredRunId::Numbered: RunNumber
```

### 4.1 Test id

Today: `TestId(pub String)` (`aye-events/src/lib.rs:101-132`), its field
reached as `.0` in ten files; taken apart by `path()`, `without_params()`
and about 25 hand-written splits; `Parts` in `aye-select/src/selector.rs:234`;
a plain `str` in Python.

```rust
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]   // Ord stays string order
pub struct TestId(Box<str>);                              // serde: transparent string
pub struct TestIdParts<'a> { pub path: ProjectPathRef<'a>, pub names: &'a str, pub params: Option<&'a str> }
impl TestId {
    pub fn new(id: impl Into<Box<str>>) -> Self;  // never rejects: an odd id from a worker still flows
    pub fn parts(&self) -> TestIdParts<'_>;       // the path ends at the first `::`; params: from the first `[` in the names, when they end with `]`
    pub fn path(&self) -> ProjectPathRef<'_>;
    pub fn without_params(&self) -> &str;
    pub fn ancestors(&self) -> impl Iterator<Item = &str>;  // what a started test covers (`aye-runner/src/run.rs:2188-2195`)
    pub fn is_file(&self) -> bool;                // no `::`: a module skipped while collecting
    pub fn key(&self) -> TestKey;
}
pub struct IdPrefix(Box<str>);   // a `--deselect` prefix: matches by `str::starts_with`, as today
```

Home: `aye-events`; Python `aye_engine.ids` (`TestId = NewType(...)`,
`parse`, `file_of`, `unparametrized`, `selectors`), which every executor
uses, and so do the default listing digests that move out of the worker. Wire: unchanged. `Ord` stays lexicographic:
it is the key order of `history.json`'s tests (`BTreeMap<TestId,
TestStats>`, `aye-history/src/lib.rs:202`) and of every map keyed by id
whose iteration reaches output. The params rule is today's
(`aye-select/src/selector.rs:310-317`): names never hold `[`, so the first
one opens the parameter id, which may itself hold `[`; the table tests
include `test_x[a[1]]`. Relations:
exposes its `ProjectPath`; gives its `TestKey`; `--lf`'s entries become
`FailedEntry::{Test(TestId), Path(ProjectPath)}` instead of bare paths in
a `TestId` set (`aye-select/src/lib.rs:615-699`). The one rule changes one
answer, an id whose path holds `[` (finding 1 in §10; Q12). Ids are cloned
into cases, batches, records, history and keys (62,000 tests on
`packaging`), so `Arc<str>` may pay; the private field makes it a one-line
change, taken only if `make bench` shows it does.

### 4.2 Test key

Today: `TestKey(pub String)` of 16 hex digits (`aye-events/src/lib.rs:144-160`),
sliced as a string in `aye-render/src/keys.rs:115, 187, 192`,
`text.rs:535`, `json.rs:778` and `aye-select/src/selector.rs:118, 541`.

```rust
pub struct TestKey(u64);          // of(&str): FNV-1a 64 + the SplitMix64 finalizer, unchanged; Display {:016x}
pub struct KeyPrefix(Box<str>);   // 4 to 16 lower-case hex digits; FromStr; `aye show 3fae6`
pub struct ShortKey { key: TestKey, len: u8 }   // 5 to 16 digits shown (aye-stream's `Keys`)
```

Home: `aye-events` (`TestKey`, `KeyPrefix`), `aye-stream` (`ShortKey`).
Wire: 16 hex wherever written. Relations: tests and lifetimes share one
key space: `Keys` maps `Subject::{Test(TestId), Lifetime(LifetimeId)}` to
a `ShortKey`, where today one `HashMap<String, String>` mixes them.

### 4.3 Project-relative path

Today: `String` in `Span.path`, `Location.path`, `Frame.path`,
`Diagnostic.path`, `SourceFile.rel`, `RefineRequest.files`, the
`Refined.digests` keys, `Plan.session_files`, `RunMeta.files` and
`ShowContext.stale`; four relativizers, two of which disagree outside the
root (`aye-collect/src/tree.rs:294`, `aye/src/session.rs:1952`); whether a
frame's path is the project's, decided in the renderer
(`aye-render/src/shared.rs:887-897`).

```rust
pub struct ProjectPath(Box<str>);   // `/`, relative to aye's root, no `.` parts, may start `../`; serde transparent
#[derive(Clone, Copy)] pub struct ProjectPathRef<'a>(&'a str);
impl ProjectPath {
    pub fn components(&self) -> impl Iterator<Item = &str>;
    pub fn parent(&self) -> Option<ProjectPathRef<'_>>;
    pub fn file_name(&self) -> &str;
    pub fn starts_with_dir(&self, dir: ProjectPathRef<'_>) -> bool;  // whole components (today's `within`)
    pub fn walk_cmp(&self, other: &Self) -> Ordering;                // the walk's order, `__init__.py` first
    pub fn is_conftest(&self) -> bool;                               // `aye/src/registry.rs:112`
}
pub enum DiagnosticPath { Nowhere, Root, At(ProjectPath) }           // "" | "." | the path
pub enum FramePath { Project(ProjectPath), External(Box<str>) }      // Frame.path, Location.path; serde as the string
impl FramePath { pub fn is_project(&self) -> bool; }                 // was `is_project_path`
```

Paths reach the type from the filesystem only through
`aye_config::ProjectRoot::relativize`, which keeps both of today's
behaviours outside the root as named functions until finding 3 in §10 picks one.
A traceback's path is a `FramePath`, since frames from the standard
library, site-packages and generated code are absolute, `C:\…` or `<…>`.
Reading sorts a frame by today's `is_project_path` rule (a leading `/`,
`\` or `<`, `..`, a drive letter, a `site-packages` or `dist-packages`
part make it external), and every form round-trips as written.
`ProjectPath`'s invariant then holds for spans, diagnostics, cases and
`RunMeta.files`, with one named lenient case: unittest's inherited methods
can give an absolute `Span.path` (finding 6 in §10), which reads and
writes back unchanged until that finding is fixed. Home: `aye-events`;
construction in `aye-config`. Wire: unchanged, `""` and `"."` included.
Relations: `TestId::path()`, `Span.path`, `AffinityKey`'s constructors,
`RunMeta.files: BTreeMap<ProjectPath, ContentDigest>`,
`StateBucket::RefineCache`; the renderers' "innermost project frame" asks
`FramePath::is_project`.

### 4.4 Spans, locations and lines

Today: `Span { path, start, end, line: Option<u32> }`, where `0..0` means
"range unknown" (refined cases) and `span.path` repeats `Diagnostic.path`
(`aye-collect/src/source.rs:101-110`); `Location { path, line }`;
`Frame { line, cols: Option<[u32; 2]>, before, indent, locals, … }`;
`SourceFile::line()` turns Ruff's `OneIndexed` into `u32` with
`unwrap_or(u32::MAX)` (`aye-engine/src/lib.rs:355-358`).

```rust
pub struct LineNumber(NonZeroU32);                 // 1-based
pub struct Span { pub path: ProjectPath, pub start: u32, pub end: u32, pub line: Option<u32> }
impl Span { pub fn range(&self) -> Option<(u32, u32)>; pub fn line(&self) -> Option<LineNumber>; }  // None for 0..0
pub struct Location { pub path: ProjectPath, pub line: u32 }   // line() -> Option<LineNumber>
pub struct Columns { pub start: u32, pub end: u32 }   // 0-based characters, end excluded; serde [start, end]
pub struct Local { pub name: String, pub value: String }  // serde [name, value]
```

Wire fields stay `u32`: a worker's line 0 must not become a
`protocol-error`; the accessors give the typed value. `aye_engine::SourceFile`
becomes `aye_adapter::ParsedFile`, which says what it holds: a file already
parsed, with its AST and line index (`aye-engine/src/lib.rs:343-352`). Its
`line()` returns a `LineNumber`. Home:
`aye-events`, `aye-adapter`. Relations: `Diagnostic::span_or_file()` reads
`path` and `span` as one in memory; both fields stay on the wire.

### 4.5 Adapter name

Today: `&'static str` (`Adapter::name`, `Executor.adapter`, `Prepared.0`);
`String` (`Case.adapter`, `BatchStarted.adapter`, `RefineRequest.adapter`,
`Batch.adapter`, the keys of `Plan.contexts`, `Plan.session_files`,
`Selection`'s three maps, the binary's folds); literals
(`aye-engine-doctest/src/lib.rs:339, 365, 1068`;
`aye-config/src/lib.rs:435-436`; `adapters.contains(&"doctest")` in
`aye-engine-pytest/src/native.rs:132`).

```rust
pub struct AdapterName(Cow<'static, str>);   // non-blank; serde transparent; Ord
impl AdapterName { pub const fn new_static(name: &'static str) -> Self; pub fn as_str(&self) -> &str; }
// each adapter crate: pub const NAME: AdapterName;  aye-pytest: pub mod names { PYTEST, DOCTEST }
// within a run: newtype_index!(AdapterId), Ord = priority, into IndexVec<AdapterId, ActiveAdapter>
```

Not a closed enum, under §3.2's rule 3: runner-side crates may carry an
adapter's name as data but never branch on one. So the names live with the
adapters: each exports its `NAME`; the two that test for each other
(pytest's for `doctest`, `aye-engine-pytest/src/native.rs:132`; doctest's
for `pytest`, `aye-engine-doctest/src/lib.rs:339, 365, 1068`) take both
from `aye-pytest::names`; the binary's registry lists all three. aye-config
keeps its `unittest` and `doctest` fields and `adapter_table`'s two
literals (`aye-config/src/lib.rs:360-363, 435-436`) for 0.1.0, as data
looked up by name: the error for an unknown `[tool.aye]` key lists every
key, `unittest` and `doctest` among them, and that text is a contract
(§8.1). A map keyed by `AdapterName` and checked against the registry
(`unittest-doctest-rs` map §8) waits until that text may change. Home:
`aye-events`; the names with the adapters and in `aye-pytest`. Wire:
unchanged. Relations: `AdapterId` replaces collection's recognizer `usize` and its
`Mask` bit (`aye-collect/src/tree.rs:13`); a duplicate id keeps the case
of the lowest `AdapterId`; `[tool.aye] adapters` is an `AdapterList`.

### 4.6 Lifetime id, lifetime kind and resource scope

Today: `type LifetimeId = String` (`aye-events/src/lib.rs:185`), in the
forms `tests/t.py`, `module:NAME` and `tests/t.py::A` (unittest) and the
literal `session` the engine uses for session and package leftovers
(`aye_native_engine.py:1676-1677`); the doc's example
`class:tests/t.py::TestX` matches no producer; parsed back by
`aye-render/src/shared.rs:466-479`. `LifetimeStarted.kind: String`,
`Resource.scope: String`; in Python `aye_model.Scope(IntEnum)`, the shim's
`ScopeName` literal; in the pytest adapter `marks::Scoped`, with an
`Unknown` arm.

```rust
pub struct LifetimeId(Box<str>);    // serde transparent
impl LifetimeId { pub const SESSION: &'static str = "session";
    pub fn file(&self) -> Option<ProjectPathRef<'_>>;   // was `file_of`; None for `session`
    pub fn key(&self) -> TestKey; }
#[derive(PartialOrd, Ord)]          // widest first
pub enum Scope { Session, Package, Module, Class, Function, Other(Box<str>) }   // serde lowercase
pub struct Resource { pub name: String, pub scope: Scope }
// Event::LifetimeStarted { lifetime: LifetimeId, kind: Scope }
```

One `Scope` for resources and lifetimes: a lifetime's kind is the scope it
opens (`Function` never is one). Python's `Scope` gains `label` (the wire
string) and `Lifetime` takes a `Scope`; the pytest adapter's `FixtureScope`
is `Option<Scope>`. Home: `aye-events`. Wire: unchanged. Relations:
`TestResult.cause` names a `lifetime_finished.lifetime` of the same batch;
unittest's lifetime ids equal its affinity keys
(`LifetimeId::from(&AffinityKey)`, the convention made a function).

### 4.7 Affinity key

Today: `Case.affinity: Vec<String>`, outermost first; the spellings in
`docs/events.md:189`; producers in five places, Rust and Python; the one
consumer, the scheduler, treats the first key as opaque
(`aye-runner/src/schedule.rs:98-140`) and needs `Key::{Affinity, Free}`
because a key can be a file name.

```rust
pub struct AffinityKey(Box<str>);   // serde transparent; Eq and Hash are string equality
impl AffinityKey {
    pub fn import(module: &str) -> Self;                           // import:NAME
    pub fn package(dir: &ProjectPath) -> Self;                     // package:DIR
    pub fn module(file: &ProjectPath) -> Self;                     // module:PATH
    pub fn class(file: &ProjectPath, names: &[&str]) -> Self;      // class:PATH::A::B
    pub fn file(file: &ProjectPath) -> Self;                       // PATH
    pub fn file_class(file: &ProjectPath, names: &[&str]) -> Self; // PATH::A::B
    pub fn kind(&self) -> AffinityKind<'_>;                        // Import | Package | Module | Class | File | FileClass | Other
}
```

Python's `aye_engine.affinity` has the same constructors. Home:
`aye-events`. Wire: unchanged byte for byte; one `kind:…` grammar would be
schema 2 (§11, Q6). Relations: built from a `ProjectPath` and names; the
scheduler groups on `GroupKey::{Affinity(&AffinityKey), File(ProjectPathRef)}`.

### 4.8 Worker, batch and run identities; the run number

Today: aliases `WorkerId = u32` and `BatchId = u32`
(`aye-events/src/lib.rs:186-189`), with `lane == 0` tested in nine places;
the run's id a random 16-hex `String` (`aye-runner/src/run.rs:332-341`),
also `PYTEST_XDIST_TESTRUNUID`; `run_number: u64`; `RunMeta.run: String`
(`r81`, or a legacy hex id); `format!("r{n}")` at six sites;
`TestStats.last_run` holds the hex id while `bad_since` holds a number.

```rust
newtype_index!(pub struct WorkerIndex);  // the lane: FIRST, is_first(), xdist_name() -> "gw3"; serde as the 0-based u32
newtype_index!(pub struct BatchIndex);   // dense, in hand-out order; serde as the u32
pub struct RunId(Box<str>);              // random(): 16 hex; opaque on read ("r1" in test logs)
pub struct RunNumber(NonZeroU64);        // Display "r81"; FromStr "r81" or "81"; serde as the number
pub struct EnvironmentId(Box<str>);      // reserved; set by nothing yet
// aye-history
pub enum StoredRunId { Numbered(RunNumber), Legacy(RunId) }      // serde as the string
pub enum RunSelector { Last, Number { number: RunNumber, or_prefix: Box<str> }, LegacyPrefix(Box<str>) }   // `aye show --run`
```

`--run` tries one value both ways, as today (`aye/src/show.rs:268-283`):
`81` or `r81` names a numbered run, and when none has that number the same
text, lower-cased, is matched as the start of a legacy hex id. So `Number`
keeps the prefix it falls back to, and a test pins that `--run 81` selects
a legacy run `81ab…` when no `r81` is stored. Home: `aye-events`; the
store's two types in `aye-history`. Wire: unchanged. Relations: `RunStarted.run_number`, `Previous.since` and
`TestStats.bad_since` are `RunNumber`s; `TestStats.last_run` is a
`RunId`; `WorkerIndex` formats `AYE_WORKER` and `PYTEST_XDIST_WORKER`;
`Stamper`, moved to `aye-events`, holds the `RunId` and the run-wide `seq`
the binary and the runner share.

### 4.9 Diagnostic code and severity

Today: `code: String`; 25 codes. The table of `docs/events.md:244-267`
lists 22, two of which nothing emits, and lacks three that are emitted.
Three classifications (§1).

```rust
pub enum DiagnosticCode {
    SyntaxError, Unreadable, NotFound, Unclaimed, DuplicateId, NoTests, NotRefined, RefineFailed,
    ClassHasInit, DuplicateDefinition, CollectionError, UsageError, UnknownOption, PluginNotLoaded,
    InternalError, Skipped, Ignored, BatchStopped, SessionFailed, ProtocolError, BatchFailed,
    NoExecutor, ListingDiffers,
    StandIn, AdapterError,          // documented, never emitted: reserved (§11, Q8)
    Other(Box<str>),
}                                   // serde: the kebab-case string; ALL lists the known ones
pub enum DiagnosticOrigin { BeforeBatches, Batch(BatchIndex) }   // from the envelope's `batch`
pub enum ErrorClass { CommandLine, Usage, Internal, RunnerInternal, Collection, Stop }
impl DiagnosticCode { pub fn class(&self, origin: DiagnosticOrigin) -> ErrorClass; }
#[derive(PartialOrd, Ord)] pub enum Severity { Info, Warning, Error }   // reordered so max() is the worst; serde by name
```

The three consumers' rules (the exit code's, `aye/src/session.rs:1186-1197`;
the summary's, `aye-render/src/tally.rs:240-266`; the store's "something
to show", `aye-history/src/runs.rs:513-526`) become three functions over
`ErrorClass` in one module, `aye-events/src/diagnostic/classify.rs`. It
takes six classes to give 0.0.1's three answers for every error code and
origin:

| Class | Codes | Exit code | Summary | Store shows |
|---|---|---|---|---|
| `CommandLine` | `unclaimed`; `not-found` before batches | usage | usage | no |
| `Usage` | `usage-error` | usage | usage | yes |
| `Internal` | `internal-error` | internal | internal | yes |
| `RunnerInternal` | `adapter-error`, `protocol-error`, `batch-failed`, `no-executor` | an error (3 only through the runner's own flag) | internal | yes |
| `Stop` | `batch-stopped` | an error | not counted | yes |
| `Collection` | `not-found` from a batch; every other code | an error | collection | yes |

A table test pins each rule's answer for every code and origin to 0.0.1's
before the switch, so their disagreements stay exactly as they are and
become visible (finding 13 in §10).
Python gets `aye_engine.records.DiagnosticCode(str, Enum)` and
`Severity(str, Enum)`; a Rust test checks Python's codes against `ALL`.
Home: `aye-events`. Wire: unchanged strings. Relations: `Diagnostic {
severity, code, path: DiagnosticPath, span, message }`; `--strict` raises
collection's warnings.

### 4.10 Setting and value source

Today: `aye_config::Origin { Default, Flag(String), Env(String), File {
path, location: String } }`, whose `Flag` carries chains of provenance as
text (`-o python_classes (PYTEST_ADDOPTS)`,
`aye-engine-pytest/src/config.rs:1119-1153`); `Setting.origin: String`,
already rendered by each adapter; `PythonSource`; `registry::Why`;
`&'static str` origins; `Sourced<T>` whose equality includes the origin.

```rust
pub enum ValueSource {
    Default,
    Cli(&'static str),                                   // "--output", "--python", "--"
    Env(&'static str),                                   // "AYE_PYTHON" → "$AYE_PYTHON"
    File { path: Arc<Path>, table: Table },              // a row: "pyproject.toml [tool.aye]"; a header: "[tool.aye] output"
    FrameworkOption { option: Box<str>, via: &'static str }, // "-o python_classes (PYTEST_ADDOPTS)", "--strict (after --)"
    Rule(&'static str),                                  // "default: pytest's rules, which run unittest's TestCase classes too"
}
pub struct Sourced<T> { value: T, source: ValueSource }   // Eq and Hash on the value only
pub struct Setting { pub name: Cow<'static, str>, pub value: String, pub source: ValueSource }
impl ValueSource {
    pub fn display_row<'a>(&'a self, root: &'a Path) -> impl Display + 'a;   // a `[tool.aye]` or `[pytest]` row: which file
    pub fn display_for<'a>(&'a self, name: &'a str) -> impl Display + 'a;    // a header line: which key
}
// binary: impl From<&PythonSource> for ValueSource   (for printing only; §4.18's type stays the interpreter's source)
```

`aye config` renders one source two ways today: the header lines name the
key (`[tool.aye] output`, `aye/src/session.rs:1834-1836`; `[tool.aye]
python`, `aye-runner/src/python.rs:32`; `from [tool.aye] adapters`,
`aye/src/registry.rs:141`), the `[tool.aye]` rows the file (`pyproject.toml
[tool.aye]`, `aye-config/src/lib.rs:782-783`). A `File` source has no key
of its own, so the header form takes the setting's name. The interpreter's
source is not duplicated: `PythonSource` (§4.18) maps into a `ValueSource`
only to print, `Requested(Flag)` to `Cli("--python")`, `Requested(Env)` to
`Env("AYE_PYTHON")`, `Requested(Config)` to the `File` header form,
`VirtualEnv` to `Env("VIRTUAL_ENV")`, and `ProjectVenv`, `BesideAye` and
`SearchPath` to `Rule(".venv")`, `Rule("beside aye")` and `Rule("PATH")`.
`via` is the adapter's word for where it read an option; pytest's
`ArgSource { Addopts, PytestAddopts, AfterDoubleDash }` gives it from
`aye-pytest`, so aye-config names no framework's sources.

Home: `aye-config` (`ValueSource`, `Sourced`); `Setting` in `aye-adapter`,
without its unused serde derives. Wire: none, but `aye config`'s text is a
contract: a test renders every variant in both forms and compares with
0.0.1's strings, the header strings above and the pinned ones in
`aye-engine-pytest/src/config.rs:1811, 1915, 1934` and
`aye-config/src/lib.rs:782-783` included. Relations: `aye config` prints
`Sourced` values; the refinement key hashes a setting's name and value and
ignores its source.

### 4.11 Outcome letters and history

Today: the letters `p f e s x u` are written in
`aye-history/src/lib.rs:251-269` and read again in
`aye-render/src/shared.rs:309-355` and `json.rs:893-901`;
`Previous { outcomes: String, since: Option<u64> }`; the history's
`Before { recent, bad_since, bad }` is a third shape.

```rust
impl Outcome { pub fn letter(self) -> char; pub fn from_letter(c: char) -> Option<Self>;
               pub fn words(self) -> &'static str; pub fn worst(self, other: Self) -> Self; }
pub struct OutcomeTrail(Box<str>);   // at most 20 letters, oldest first; serde as the string
impl OutcomeTrail { pub const MAX: usize = 20; pub fn push(&mut self, outcome: Outcome);
                    pub fn iter(&self) -> impl Iterator<Item = Outcome> + '_; pub fn counts(&self) -> (u32, u32); }
pub struct Previous { pub outcomes: OutcomeTrail, pub since: Option<RunNumber> }
```

`Outcome` keeps its fixed set, without an unknown arm, by policy
(`docs/events.md:282`). Home: `aye-events`; `TestStats { recent:
OutcomeTrail, bad_since: Option<RunNumber>, last_run: Option<RunId>, … }`
in `aye-history`. Wire: unchanged (`history.json` version 1,
`test_finished.history`). Relations: the binary sets `Previous` before any
consumer sees the record.

### 4.12 Stop reason

Today: `aye_runner::StopReason` turned into a `String` for
`run_finished.stopped` and `RunMeta.stopped`, matched as text with a
fallback (`aye-render/src/text.rs:1860-1877`), and used by the binary for
a collection interrupted before the runner ran (`aye/src/session.rs:438`).

```rust
pub enum StopReason { Maxfail, Interrupted, Framework, Other(Box<str>) }   // serde snake_case
```

Home: `aye-events`. Wire: unchanged. Relations: the run's interruption is
not derivable from it (a framework stop with status 3 is an internal
error, not an interruption), so `RunVerdict` keeps both facts.

### 4.13 Exit status

Today: `aye_events::ExitCode` (`aye-events/src/lib.rs:648-692`) is sound,
but it is decided at ten sites, and the frameworks' statuses are raw
`i32`s: pytest's in `aye-runner/src/run.rs:40-46` and
`aye_pytest_native.py:97-102`, unittest's own (`OK`, `FAILED`,
`INTERRUPTED`, `NO_TESTS`, "as `python -m unittest` exits") in
`aye_engine_unittest.py:179-182`, and `Event::BatchFinished.status:
Option<i32>` in the log (`aye-events/src/lib.rs:840-843`).

```rust
impl ExitCode { pub fn precedence(self, other: Self) -> Self; }  // interrupted, then internal, then usage (`docs/events.md:420-423`)
pub struct FrameworkStatus(i32);   // aye-events, re-exported by aye-protocol: OK, TESTS_FAILED, INTERRUPTED, INTERNAL_ERROR, USAGE_ERROR, NO_TESTS
// Event::BatchFinished { status: Option<FrameworkStatus> }; Reply::BatchDone likewise
// binary, commands/run/exit.rs: the one decision
pub struct RunVerdict { pub interrupted: bool, pub internal: bool, pub usage: bool, pub errors: bool,
    pub framework_failed: bool, pub selected: u32, pub counts: Counts, pub collect_only: bool,
    pub continue_on_collection_errors: bool }
impl RunVerdict { pub fn exit_code(&self, stage: RunStage) -> ExitCode; }   // RunStage::{Collection, Run}
// binary, error.rs: failures before any event, and show's
pub enum AyeError { Usage(anyhow::Error), NoRun(String), Output(io::Error) }   // 4; 5; 4, or 0 on a broken pipe
```

The process exits with the run's `ExitCode`, except that a JUnit report
that fails at the end turns 0 or 5 into 1 (`docs/events.md:406-408`),
written once in `lib.rs`. At the collection stage, 0.0.1 decides 0
against 5, and without `--co` 5 against 1, on the number of cases
selected (`aye/src/session.rs:587-609`), while `Counts` holds only
`deselected` (`session.rs:577-580`) and has no selected field: hence
`selected`. A table test written against 0.0.1 before the switch pins both
stages: internal, usage, `--co` with and without collection errors and
with and without `--continue-on-collection-errors`, nothing selected, and
the run stage's order. Home: as above. Wire: unchanged; 0-5 are pytest's,
and `FrameworkStatus` serializes as the number. Relations: `RunVerdict`
reads `ErrorClass` counts and the runner's `RunSummary`; `FrameworkStatus`
feeds the runner's batch verdict (`aye-runner/src/run.rs:2223-2250`).

### 4.14 Verbosity and the detail ladder

Today: `u8` counts in both clap structs; `i8` clamped
(`aye/src/args.rs:147`) and unclamped (`args.rs:155`, sent as
`Policy.verbosity`, read by no one); `aye show`'s defaults as magic
numbers (`args.rs:285-294`); thresholds named differently in
`aye-render/src/text.rs:166-172` and `json.rs:180-186`; the L1/L2/L3 enum
private to `text.rs:227`.

```rust
// binary, cli/run.rs and cli/show.rs: `verbose`, `quiet`, `output` and `color` stay fields of each, in today's place
// binary, cli/output.rs: what the two share
pub struct Verbosity { verbose: u8, quiet: u8 }   // level(); is_unset() for show's default; Display " -vv"
// aye-render
pub enum Level { Quiet = -1, Default, Verbose, VeryVerbose, Engine }   // the net count, clamped once
pub enum DetailLevel { L1, L2, L3 }
impl Text { pub const SHOW_DEFAULT: Level = Level::Verbose; pub fn detail(level: Level) -> Option<DetailLevel>; }
impl JsonLines { pub const SHOW_DEFAULT: Level = Level::VeryVerbose; pub fn detail(level: Level) -> Option<DetailLevel>; }
```

The four arguments are not one flattened clap struct, because the two
commands' help differs: the run's `-v` is "More output, in either format;
repeat for more (up to -vvv)." and its `--output` "Output format; -v and
-q set how much it shows." (`aye/src/args.rs:46-52, 86-96`); show's `-v`
ends "Without -v or -q, details come at -v in text, -vv in JSON." and its
`--output` is "Output format, as for a run." (`args.rs:244-260`). In the
run's help, `-v` and `-q` sit after `--maxfail`, `--output` after
`--strict` and `--color` after `-c`, and a flattened group would list the
four together. So `aye --help` and `aye show --help` stay as they are, and
step 0 records both. The ladder of `docs/architecture.md` ("Output and
history") becomes two constant tables beside the renderers, tested
against the document. Home: the binary and `aye-render`. Wire:
`Policy.verbosity` goes in protocol 4 (§8.2). Relations: `OutputSettings { format: Sourced<OutputFormat>,
verbosity: Verbosity, color: ColorChoice }`; `aye config`'s `json -v` is
`Verbosity`'s `Display`.

### 4.15 The batch context and each adapter's context

Today: one `serde_json::Value` per adapter (`Plan.contexts`,
`aye-runner/src/run.rs:91`) holds the adapter's keys, the hints only Rust
reads and the keys the binary and the runner splice in; each executor
coerces `context if isinstance(context, dict) else {}`.

```rust
// aye-protocol
pub struct AdapterContext(serde_json::Value);   // opaque to Rust, built once by the adapter
pub struct BatchContext {
    pub adapter: AdapterContext,
    pub requested_ids: Vec<TestId>, pub rerun_args: Vec<String>,
    pub selected_paths: Vec<String>, pub ran_ids: Vec<TestId>,
}   // Serialize: the adapter's object plus each non-empty key; the binary's keys only into an object; ran_ids turns null into {}
pub mod context_keys { pub const SESSION_NEUTRAL: [&str; 3] = ["requested_ids", "ran_ids", "rerun_args"]; }
// aye-adapter
pub struct FrameworkDefaults { pub workers: Option<WorkerCount>, pub maxfail: Option<MaxFail>,
    pub keyword: Option<String>, pub tags: Option<String>, pub deselect: Vec<IdPrefix>, pub junit: JunitDefaults }
// each adapter: #[derive(Serialize)] PytestContext, UnittestContext, DoctestContext → AdapterContext
```

`serde_json` is built without `preserve_order`, so an object's keys
serialize sorted whatever order they were inserted in: the typed contexts,
going through `Value`, give today's bytes. `Recognizer::defaults()`
replaces the binary's 12 lookups; the hint keys stay in the JSON for 0.1.0,
since the refinement key hashes the context (§8.2). `SESSION_NEUTRAL` is
the pytest executor's `BATCH_KEYS` (`aye_pytest_native.py:115`), checked by
a Rust test; `selected_paths` is deliberately not in it. `JunitDefaults`
carries pytest's spellings, and the adapters, not the binary, read
`junit_log_passing_tests` as a bool: the binary's second `_strtobool`
(`aye/src/session.rs:1621-1632`) goes. Python: `aye_engine.context.BatchContext.from_wire()`
with `RanIds`. Home: `aye-protocol`, `aye-adapter`, the adapters. Wire:
unchanged JSON. Relations: `Plan.contexts: BTreeMap<AdapterName,
AdapterContext>`; the runner adds `ran_ids` through `BatchContext`.

The contract's one hidden string protocol goes too. Collection learns
whether a recognizer claims a file of some kind whatever its name by
asking it to claim a made-up `__aye_any_name__.<ext>`
(`aye-collect/src/lib.rs:955-984`). `Recognizer::claims_any_name(dir,
extension)` makes that a method of the contract, with today's probe as its
default implementation, so the answers stay the same (`collect` map
§10.1).

### 4.16 Service names

Today: 14 `&str` constants (`aye-engine/src/lib.rs:226-244`), copied as
class attributes in `aye_engine.py:1633-1646` with no check; the worker
drops unknown names silently (`aye_worker.py:268-274`); only `context`
and `cleanup` are provided.

```rust
pub enum ServiceName { Capture, Context, Cleanup, Coverage, Sandbox, Timeout, Resources, Patching,
                       Clock, Randomness, Network, Filesystem, Snapshots, Data }   // serde lowercase
impl ServiceName { pub const DEFAULT: [Self; 2] = [Self::Context, Self::Cleanup]; }
```

Home: `aye-protocol`; Python `aye_engine.services.ServiceName(str, Enum)`,
compared by a Rust test. Wire: unchanged (`hello.services`).

### 4.17 Embedded Python modules

Today: `EmbeddedModule { name, source }` (`aye-engine/src/lib.rs:190-195`),
copied into an owned `ModuleSource` for every spawn, about 600 KB for the
pytest stack (`aye-runner/src/worker.rs:121-144`); the install order
written in three arrays; `Executor.entry` a `"module:Class"` string parsed
by `load_entry`; `WithAyePytest` overriding two adapters' module lists.

```rust
pub struct ModuleSource { pub name: Cow<'static, str>, pub source: Cow<'static, str>, pub package: bool }  // borrowed when serialized
pub struct ModuleSet(&'static [ModuleSource]);   // install order; tested: a module imports only earlier ones and the stdlib
pub struct EntryPoint { pub module: &'static str, pub class: &'static str }   // serde "module:Class"
pub enum SessionMode { PerBatch, ListsSession }                                // serde as `lists_session`
pub struct Runtime { pub adapter: AdapterName, pub modules: ModuleSet, pub entry: EntryPoint, pub session: SessionMode }
// aye-adapter: Adapter::runtime(&self) -> Runtime   (was `executor()`)
// aye-runner: every hello starts with aye_engine::ENGINE (the worker reads sys.modules["aye_engine"])
// binary registry: then aye's pytest module for every adapter, then the adapter's own set
const PRELUDE: ModuleSet = aye_pytest::PYTEST;   // from step 12(e); before it, the shim and what it stands on (§9)
```

Home: `aye-protocol` (the types, `Runtime` and `SessionMode` included),
`aye-engine` (`ENGINE`), `aye-pytest` (`PYTEST`, aye's `pytest` module),
each adapter (its `Runtime` value). The runner names `Runtime` in its
options, worker specs and tests (`aye-runner/src/run.rs:67-70, 668-689`;
`worker.rs:25-27, 90`; `tests/workers.rs:12`), so it lives where the
runner can see it without the edge to `aye-adapter`, which would bring
Ruff and aye-config with it. Wire: `hello.modules[{name, source}]`
unchanged until protocol 4 adds `package`. Relations: `WorkerSpec::hello`
sends the engine, the prelude, then the adapter's set, each name once, in
the order a test pins. The pytest adapter's `modules_digest` keeps hashing
today's six modules (model, rewrite, native engine, shim, front-end,
executor) in today's order (`aye-engine-pytest/src/native.rs:69-76,
346-361`), wherever each moves: the digest is part of the context, so of
the refinement key, and covering the whole prelude is §8.2's deferred
change (Q10).

### 4.18 The interpreter and its source

Today: `find_python(flag: Option<&Path>, config: Option<&str>, root, cwd)
-> Option<(PathBuf, PythonSource)>` (`aye-runner/src/python.rs:50-61`), a
`PythonSource` that mixes where the request came from with where
discovery found the interpreter, `Session.python` beside
`Session.python_source`, and venv layout read again in
`aye-collect/src/environment.rs:130-220` and in two adapters (`_in_venv`).

```rust
pub enum PythonRequest { File(PathBuf), ExecutableName(String) }   // parse(value, base): one component, no root → a name
pub enum RequestSource { Flag, Env, Config }                       // "--python", "$AYE_PYTHON", "[tool.aye] python"
pub enum PythonSource { Requested(RequestSource), VirtualEnv, ProjectVenv(VenvDir), BesideAye, SearchPath }
pub struct Interpreter { pub executable: PathBuf, pub source: PythonSource }
pub struct PythonNotFound;          // "no Python interpreter found: pass --python, or set [tool.aye] python"
pub struct DiscoveryEnv { pub virtual_env: Option<OsString>, pub path: Option<OsString>, pub current_exe: Option<PathBuf> }
pub fn find(request: Option<(PythonRequest, RequestSource)>, env: &DiscoveryEnv,
            root: &Path, cwd: &Path) -> Result<Interpreter, PythonNotFound>;
pub struct EnvironmentFingerprint { pub digest: ContentDigest, pub installed: InstalledModules }
```

The binary builds `DiscoveryEnv` from its `EnvironmentOptions` (empty
values count as unset, as `aye-runner/src/python.rs:70-72` does), which
replaces the `System` test seam. `InterpreterInfo { pid, python, version }`
types `Reply::Ready` in `aye-protocol`. Home: `aye-provision`, which
depends on `aye-events` for `ContentDigest`. Wire: none; `aye config`'s
`python   <path>  (<source>)` and the discovery order (`python` before
`python3` beside aye, `python3` before `python` on `PATH`) unchanged.
Relations: the runner takes `Interpreter.executable`; collection receives
the fingerprint as data instead of inspecting the interpreter, so
`aye-collect` depends on `aye-provision` for its type (Q3); `aye config`
prints `PythonSource` through §4.10's mapping.

### 4.19 pytest's ini values

Today: the ini file is read twice in Rust (`aye-engine-pytest/src/config.rs:218`,
into `RawConfig { values: BTreeMap<String, (Raw, Origin)> }`; `native.rs:400`,
into JSON), with `-o` applied twice (`config.rs:1119-1150`,
`native.rs:463`); a third reader, approximate, in
`aye-engine-doctest/src/config.rs:59-145`; a legacy one in the front-end
that only tests use (`aye_pytest_frontend.py:684-825`); `PytestConfig` is a
35-field bag, partly sourced.

```rust
pub enum ConfigFileName { PytestToml, DotPytestToml, PytestIni, DotPytestIni, PyprojectToml, ToxIni, SetupCfg }  // ALL in pytest's order
pub enum IniSection { Pytest, ToolPytest, ToolPytestIniOptions, ToolColonPytest }   // Display "[pytest]" …
pub enum RawIniValue { Text(String), List(Vec<String>), Toml(toml::Value) }          // to_json(): today's `native.ini` value
pub struct RawIni { values: BTreeMap<IniKey, Sourced<RawIniValue>> }                  // read once
pub struct PytestArgv { addopts: Vec<String>, env: Vec<String>, after_dashes: Vec<String> }  // pytest's order; last_value()
pub enum ArgSource { Addopts, PytestAddopts, AfterDoubleDash }   // which of the three; as_str() is ValueSource's `via`
pub enum ConfigRules { Exact, Approximate }   // the pytest adapter's rules; the doctest adapter's 0.0.1 rules (§6)
pub struct PytestConfig { pub root: PytestRoot, pub ini: PytestIni, pub options: CollectionOptions,
    pub selection: PytestSelection, pub takeover: RunnerTakeover, pub doctest: DoctestOptions, pub frontend_args: FrontendArgs }
impl PytestConfig { pub fn resolve(project: &Project, env: &PytestEnv, rules: ConfigRules) -> Result<Self, ConfigError>; }
```

`ConfigRules` says which rules find pytest's rootdir, ini file and
options; it is not called `Discovery`, a word collection and
`aye-provision`'s `DiscoveryEnv` already use.

In Python, the front-end's `getini` precedence (`aye_pytest_frontend.py:1418-1431`)
becomes an explicit `IniSource { OVERRIDE, RESOLVED, RAW, DEFAULT }`.
Home: `aye-pytest`. Wire: the `native` section, byte-identical. Relations:
`aye config`'s `[pytest]` rows derive from `PytestIni`, `FrameworkDefaults`
from `RunnerTakeover`.

### 4.20 Glob matching

Today: three `fnmatch.rs` (pytest's 421 lines, unittest's 264, doctest's
140), pytest's `glob`, doctest's private `glob`
(`aye-engine-doctest/src/lib.rs:1390-1456`) and the front-end's
`fnmatch_ex`.

```rust
pub enum SetTranslation { CPython, Legacy }   // unittest's, exact against CPython; pytest's and doctest's
pub struct FnPattern { source: Box<str>, tokens: Box<[Token]> }   // compiled once; as_str(), Display
impl FnPattern { pub fn new(pattern: &str, sets: SetTranslation) -> Self; pub fn matches(&self, name: &str) -> bool; }
pub fn fnmatch_ex(pattern: &FnPattern, path: &Path) -> bool;      // pytest's
pub fn matches_prefix_or_glob(pattern: &str, name: &str) -> bool; // python_classes, python_functions
pub fn glob(base: &Path, pattern: &str) -> Vec<String>;           // sorted(iglob(recursive=True)), spelled as Python spells
```

One iterative matcher for all, so doctest's exponential one goes with the
same answers; the `Legacy` translation stays where it is used today until
finding 2 in §10. Home: `aye-python-stdlib`. Relations: `norecursedirs`,
`python_files`, `--ignore-glob` and `testpaths` in `aye-pytest`'s
`CollectRules`; unittest's `pattern`; doctest's `--doctest-glob`.

### 4.21 The failure vocabulary

Today: `Assertion.op: String`, `Compare.kind: String` and
`Failure.signature: String` (`aye-events/src/lib.rs:287, 322, 271`), the
signature's empty value meaning "never grouped" and left out on the wire;
the renderers match on the strings (`compare.kind` against `"str"` and
`"bytes"`, `aye-render/src/text.rs:1484-1492`; `assertion.op == "=="`,
`aye-render/src/shared.rs:83, 207`).

```rust
pub enum ComparisonOp { Eq, NotEq, Lt, LtE, Gt, GtE, Is, IsNot, In, NotIn, Other(Box<str>) }   // serde as "==", "is not", …
pub enum CompareKind { Dict, List, Tuple, Set, Dataclass, Namedtuple, Str, Bytes,
                       Datetime, Date, Time, Number, Other(Box<str>) }   // serde lower-case; is_text() for str and bytes
pub struct FailureSignature(Box<str>);   // 16 hex
// Failure { …, signature: Option<FailureSignature> }   #[serde(default, skip_serializing_if = "Option::is_none")]
```

The operators are the rewriter's `_COMPARISONS` (`aye_rewrite.py:141-152`),
the kinds `aye_engine.compare`'s `_COMPARERS` (`aye_engine.py:1343-1354`).
An empty signature reads as `None`, so a record that held `""` writes back
without it, as today. Home: `aye-events`. Wire: byte-identical. Relations:
`Assertion { op: ComparisonOp, compare: Option<Compare { kind: CompareKind, … }> }`;
first-by-signature in `aye-stream` groups on `FailureSignature`.

### 4.22 The rerun command

Today: Python builds `["aye", *rerun_args, id, "--", *framework_args]` in
three places (`reproduce()` and `rerun_args()`, `aye_engine.py:1431-1452`,
and the native engine's `reproduce` callback,
`aye_native_engine.py:1520`); the binary takes the command apart again by
position to rerun from the current directory (`Here::rerun_from_here`,
`aye/src/session.rs:763-800`).

```rust
pub struct RerunCommand { pub options: Vec<String>, pub target: String, pub framework_args: Vec<String> }
impl RerunCommand {
    pub fn parse(command: &[String], id: &TestId) -> Option<Self>;   // `aye` first; the target is the id before the first `--`
    pub fn to_command(&self) -> Vec<String>;
}
```

Python gets `aye_engine.result.RerunCommand(rerun_args, framework_args)`
with `for_test(id) -> Reproduce`, which the three builders become
(`python-engine` map §10.2). A command `parse` doesn't read is left as it
is, as today. Home: `aye-events`. Wire: `Reproduce.command` stays a list
of strings. Relations: `RerunFromHere` replaces the target of a
`RerunCommand` instead of an index.

## 5. Module decompositions

Each tree is the target; what isn't listed moves to another crate, as
§3.1 says.

**`aye` (the binary)**, from `main.rs`, `args.rs`, `session.rs`,
`show.rs`, `config_cmd.rs`, `busy.rs`, `registry.rs`:

```
main.rs          fn main() -> ExitCode { aye::main(std::env::args_os()) }
lib.rs           main: Command::parse → EnvironmentOptions → settings → commands::*; AyeError → ExitCode
error.rs         AyeError
printer.rs       `aye: error|warning: …`, the `collecting…` line (busy.rs), `\naye: interrupted`
environment.rs   EnvironmentOptions, read once: the catalogued variables and `vars`, the whole environment (ProcessEnv)
cli.rs, cli/     Command (show/config detected as argv[1] when no such path exists), run.rs RunArgs,
                 show.rs ShowArgs (each with its own -v, -q, --output and --color, in today's place and words),
                 output.rs Verbosity, compat.rs RunCompatArgs (--native, visible, where it is today)
settings.rs, settings/   RunSettings::resolve, in today's order (session.rs:124-246); phase 2 adds FrameworkDefaults;
                 output.rs, workers.rs, maxfail.rs, selection.rs, junit.rs, show.rs
positional.rs    Positional { Path | Id | Word }, Positionals (session.rs:106-115, 1855-1914)
registry.rs      the adapter table with AdapterTraits, the worker prelude, choose() → ActiveAdapters
commands.rs, commands/
  config.rs
  run.rs, run/   collect.rs, reach.rs (Reach, dotted reach via Selector::reaches), select.rs (Pick: which
                 -k/-m counts, over aye-pytest's ArgValue), plan.rs, exit.rs (RunVerdict),
                 interrupt.rs (Presses, the grace periods)
  show.rs, show/ choose.rs (keys, closest names), feed.rs (over aye-stream's RunIndex from step 7),
                 runs.rs (--runs, printed from RunMeta through a typed RunLine)
stream.rs, stream/  sink.rs (Sink, no field splitting around the Stamper), passes.rs (RerunFromHere,
                 OneModuleErrors, OneConftestError, PreviousOutcome over one BatchAdapters fold)
tests/           common/ (aye-test), cli/main.rs + modules by theme, show/main.rs, compare.rs (its own binary)
```

**`aye-runner`**, from `run.rs` (2548), `worker.rs` (1139), `schedule.rs`
(1290):

```
lib.rs options.rs (RunnerSettings, Plan) summary.rs (RunSummary, RunReport) stop.rs (StopHandle, Registry) env.rs (LaneEnv) sync.rs
worker.rs, worker/     spec, handshake (LaunchedWorker → Worker), transport, process, exit (ExitDescription),
                       stderr, crash_report, timeouts (every grace and poll, documented once)
schedule.rs, schedule/ weights (Weights, the one duration rule), plan (GroupKey, Group), batch (BatchSpec, BatchOrigin, Pin, Priority), queue
run.rs, run/           log (Log, RunPhase), message, lane (Lane, LaneWorker, SessionState), batch (BatchRead), crash
                       (BatchProgress), maxfail (MaxfailBudget, Stopping)
listing.rs             WarmWorker, PrelistedWorker, Listings
refine.rs              impl Refiner
tests/it/              main.rs, common.rs and one module per topic; fixtures/fake_executor.py by include_str!
```

`BatchSpec` with `BatchOrigin::{Planned, Retry { ran }, Closing}` and
`Pin::{AnyLane, FirstLane}` replaces `Planned`, `Pending` and `Assigned`;
`RunPhase::{Running, FrameworkStopped, WindingDown { kill_at }, Killed}`
replaces four flags; `Lane` and `LaneWorker { worker, adapter, session,
announced }` replace the loop's six locals.

**`aye-adapter-pytest`**, from `lib.rs`, `native.rs`, `module.rs`,
`support.rs`, `marks.rs`, `ids.rs`; the line between what every run needs
and what only `aye --co --no-refine` needs becomes a directory:

```
lib.rs adapter.rs (Pytest, Listing::{Refine, Static}) embedded.rs (the Python set, ENTRY, digest)
recognizer.rs (the trait impl only) context.rs (PytestContext, NativeContext)
session.rs, session/  plugins (SessionPlugin, PluginSource, HookSource), scan (CollidingModules apart from
                      SessionHooks), import_key, pyargs
claims.rs, claims/    ignore, doctest_plugin (PytestDoctests::{ByDoctestAdapter, ByPytestPlugin, Blocked})
conftest.rs, conftest/ summary, hooks (Hook, HookSet), fixtures (Fixture, FixtureCertainty), ini_changes,
                      index (SupportIndex, SummaryId: no pointer-address keys)
predict.rs, predict/  static listing only: scope (SupportScope), module, class, unittest, function, closure,
                      reorder, case, refinement (RefineReason, demotion)
marks.rs, marks/      mark, pytestmark, parametrize, ids (IdMaker)
```

`scope.rs` goes to `aye-python-semantic` (namespace, escape, resolve,
eval, flow) and `aye-python-stdlib` (regex acceptance, version tables,
reprs); `config.rs` and `native.rs`'s second reader to `aye-pytest`;
`fnmatch.rs` to `aye-python-stdlib`. `module.rs` becomes `predict/`, its
class model and body helpers going to `aye-python-semantic`; `support.rs`
becomes `conftest/`, its `SysPath` and import resolver going to
`aye-python-semantic`; `ids.rs`'s Python facts (float and complex reprs,
the `isdigit` tables) go to `aye-python-stdlib`; `lib.rs`'s 450 lines of
decisions go to `docs/architecture.md` and onto the items they decide.
`NativeRecognizer` becomes the
`Listing::Refine` mode of the one recognizer, and `explicit` (a side
channel from `claim` to `recognize`, `lib.rs:1716-1718`) becomes an
argument of `recognize`.

**`aye-adapter-unittest`**, from `lib.rs` (759) and `module.rs` (2878):

```
lib.rs options.rs settings.rs discovery.rs (Starts { configured, named }) recognizer.rs context.rs
module.rs, module/  binding, builder, class, scan, tables
project.rs          ModuleIndex: an arena with one OnceLock per file, ClassId { module, class }
loader.rs           Loaded, Method, the MRO through aye_python_semantic::c3
```

**`aye-adapter-doctest`**, from `lib.rs` (1456), `config.rs` (923),
`finder.rs` (1777):

```
lib.rs options.rs settings.rs (aye_pytest::PytestConfig with ConfigRules::Approximate, plus the doctest layer)
recognizer.rs module.rs text.rs project_scan.rs conftests.rs mismatch.rs (pytest's collection order) context.rs
finder.rs, finder/  namespace, alias, checks, tables
parser.rs           DocTestParser, OptionFlag
```

**`aye-events`**: `lib.rs` (docs, `SCHEMA_VERSION`, re-exports), `id.rs`,
`path.rs`, `outcome.rs`, `failure.rs`, `case.rs`, `result.rs`,
`diagnostic.rs` + `diagnostic/classify.rs`, `event.rs`, `stamp.rs`,
`digest.rs` (FNV-1a 64 and 128, `ContentDigest`), `serde_ext.rs` (`millis`,
a `string_enum!` macro), `tests/wire.rs` with `fixtures/every_event.jsonl`.

**`aye-render`**, from `text.rs` (2160), `json.rs` (1415), `shared.rs`
(1620):

```
lib.rs show.rs (RenderMode::{Run, Show { mode, keys, context }}, ShowContext over aye-stream's FileChange)
failure.rs, failure/ why, oriented (expected and actual), message and explanation (the adapters' text rules,
                     each documented with its adapter, kept for stored 0.0.1 logs: Q13), resource, frames
words.rs, words/     truncate, plural, duration, history, time (ago, utc, rfc3339: from show.rs)
rerun.rs
text.rs, text/       settings, stylesheet, out, limits, progress, header, diagnostics, engine, bad (Subject),
                     block, code, explain, summary, show (list lines, stale warnings)
json.rs, json/       schema (every line type, field order pinned), limits, detail, summary
junit.rs, junit/     settings (JunitSettings, from_pytest spellings), xml, names
github.rs, github/   command
```

`aye show --runs` renders the stored runs' summaries, not events, and
renderers see only events (the 0.0.1 AGENTS.md's Rules). So it is output
outside the event stream, which the binary's printer prints (pattern 9):
`commands/show/runs.rs` builds each line from `RunMeta`, through a typed
`RunLine` for the JSON form (pattern 17), with aye-render's `words`.
aye-render takes only `show.rs`'s time helpers (`aye/src/show.rs:945-1000`).

**`aye-collect`**: `lib.rs`, `targets.rs` (`Targets::{Default, Paths,
PathsAndDefault}`), `walk.rs` + `walk/{tree, order}` (`FileId`,
`RecognizerSet`), `source.rs`, `recognize.rs`, `refine.rs` +
`refine/{cache, key, standalone}`, `assemble.rs`, `diagnostic.rs`
(`CollectDiagnostic`, today's texts), `parallel.rs`.

**`aye-select`**: `truth.rs`, `expression.rs` + `expression/{scan,
parse}`, `keyword.rs` and `marker.rs` (each with its own "off" rule),
`selector.rs`, `selection.rs` (`AdapterSelection`, `SelectedCase { case,
run_as }`, `SelectorIndex` for unmatched arguments), `last_failed.rs`,
`order.rs`.

**`aye-config`**: `project.rs` (`ProjectRoot`), `options.rs`
(`AyeOptions`), `settings.rs`, `source.rs`, `values.rs`, `error.rs`. The
INI reader goes to `aye-pytest`, its only users.

**`aye-history`**: `history.rs` + `history/{stats, file}`; `store.rs` +
`store/{id, meta, layout, writer (WriterState::{Idle, Claimed, Writing,
NotStored, Failed}), reader, retention}`; the writer folds its own `Tally`,
so the binary's `Keep` goes.

The Python modules' decompositions are in §7.1.

## 6. Duplication to remove

| Duplicate | Copies | How they differ | Single home |
|---|---|---|---|
| `fnmatch` | `aye-engine-{pytest,unittest,doctest}/src/fnmatch.rs` | Fuzzed against CPython 3.13 on 2.6 million cases: unittest's is exact; pytest's and doctest's agree with each other and differ for sets whose first item is a reversed range followed by `!` (`[&-%!a]`); doctest's matcher is exponential (340 ms for 40 characters and 7 stars) | `aye-python-stdlib::FnPattern`, both translations named, one matcher (§4.20) |
| `glob` | pytest's (`fnmatch.rs:178`), doctest's (`aye-engine-doctest/src/lib.rs:1390-1456`) | doctest's returns absolute paths, spells `**` differently, treats hidden files differently | `aye-python-stdlib::glob`; doctest's kept, named, while `ConfigRules::Approximate` lives |
| pytest's configuration | `aye-engine-pytest/src/config.rs`; `native.rs:400-523` (second ini reader, second `-o`); `aye-engine-doctest/src/config.rs`; the front-end's legacy `_read_ini_file` | doctest's: `-c` precedence, rootdir fallback (aye's root, no `setup.py` rule), no `setup.cfg [pytest]` error, lenient TOML coercion, no `--rootdir` check, `--import-mode` errors where pytest's is lenient, `-o` passes (`unittest-doctest-rs` map §4.2) | `aye-pytest::PytestConfig` with `ConfigRules::{Exact, Approximate}`; the second Rust reader and the legacy Python one go |
| `_strtobool`, `python_str` | `aye-engine-pytest/src/config.rs:443, 206`; `native.rs:487, 365`; `aye/src/session.rs:1621-1632` | the binary's reads a string the doctest adapter sends raw | `aye-pytest` |
| Short-option tables | `aye/src/args.rs:381-385, 478-482`; `aye-engine-pytest/src/config.rs:593-596`; `aye-engine-doctest/src/config.rs:284-285` | two distinct sets (`vqlsxhV`/`kmporWc` for `-k`/`-m`; `xvqslhVdf`/`kmcpoWrn` for `-x`), each copied | `aye-pytest::argv`, both sets named |
| `-x`/`--maxfail` readers | `args.rs:492` (`passed_maxfail`); `config.rs:631-660` (`split_maxfail`); doctest `config.rs:313` | `None` means "unset or unreadable" in the binary's | `aye-pytest::argv` with `ArgValue::{Unset, Set, Unreadable}`; the binary keeps treating `Unreadable` as unset |
| pytest's option table | `native.rs:568-923` (Rust, 87 dests); `BUILTIN_OPTIONS` (`aye_pytest_frontend.py:860-956`) | the Rust copy reaches no reader (finding 15 in §10) | the Python table; the Rust one goes when finding 15 in §10 is fixed |
| Path normalizers | `aye-config/src/lib.rs:146` (`normalize`: keeps a leading `..`, empty → `.`); `aye/src/session.rs:1918`, `aye-engine-pytest/src/lib.rs:615`, `aye-engine-unittest/src/lib.rs:336` (pop on every `..`) | two semantics, equal for the absolute inputs given today | `aye-fs::{normpath, absolute}`, each call site keeping its function |
| Relativizers | `aye-collect/src/tree.rs:294`; `session.rs:1936-1981`; `aye-config/src/lib.rs:178`; `aye-engine-unittest/src/lib.rs:521` | outside the root: `../../x` versus `/x`; unittest falls back to an absolute path | `aye_config::ProjectRoot::relativize`, both behaviours named (finding 3 in §10) |
| Root discovery | `session.rs:128-137`; `show.rs:112-121` | none | `aye_config::ProjectRoot::discover` |
| FNV-1a | `aye-events/src/lib.rs:148-158`; `aye-collect/src/refine.rs:434-472`; `aye-history/src/runs.rs:171-180`; `aye-engine-pytest/src/{native.rs:345, support.rs:1712}` | 64 and 128 bits, prefixed parts or raw bytes | `aye-events::digest` |
| Diagnostic classification | `session.rs:1186-1197`; `tally.rs:240-266`; `runs.rs:513-526` | each consumer's answer differs for `protocol-error`, `batch-failed`, `no-executor`, and for `not-found` by origin | one module, three named rules, pinned (§4.9) |
| Test-id parsing | about 25 sites (`wire` map §3.1) | three rules (§1) | `TestId::parts`; Python `aye_engine.ids` |
| Dotted-name reading | `aye-select/src/selector.rs:52-58, 262-275`; `aye/src/session.rs:1149-1176` | `Selector::parse` reads a word as dotted only when every part is an identifier, and splits `[params]`; `dotted_reach` takes any non-empty parts, drops `__init__` parts and splits no params. So for a `my-mod.py` skipped at module level, `aye my-mod` is reached today (no not-found, `session.rs:480-482`), and would be a not-found usage error under `Selector`'s rule | `Selector::reaches`, with `dotted_reach`'s rule kept behind a named mode until the harness decides |
| Duration estimates | `aye-history/src/lib.rs:449-472` (`History::estimate`); `aye-runner/src/schedule.rs:304-331` (`Weights`) | `estimate` has no caller outside tests (finding 23); `Weights` is the only live rule | the runner's `Weights`, over `History::durations()` handed down as data, as today; `estimate` goes with finding 23 |
| `r{n}` | `aye-render/src/json.rs:798, 900, 1278`; `shared.rs:331`; `aye-history/src/runs.rs:166`; `aye/src/show.rs:270` | none | `RunNumber`'s `Display` |
| Renderer folds | sub-outcome buffer ×4, blocked tests ×4 + `Tally` + `show.rs`, first-by-signature ×3, definition lines ×2, error dedupe ×4, run facts ×4 (`render` map §5) | JUnit doesn't dedupe errors; titles of lifetimes differ; text and JSON re-show a lifetime that fails again | `aye-stream`, each renderer's difference kept as an explicit choice |
| Value-type pairs | `args::Output` and `[tool.aye] output`; `args::Workers` and `aye_config::Workers`; `Maxfail`, pytest's `MaxFail`, `Option<u32>`; two `ImportMode`s; `JunitSettings` strings and the renderer's enums | messages per surface (CLI and TOML word `human`/`agent` differently); `logical` accepted by `-n` only | one type each (`OutputFormat`, `WorkerCount`, `MaxFail`, `ImportMode`, `JunitSettings`), keeping each surface's messages and accepted words |
| pytest rules in two adapters | `CONFIG_NAMES`, `DEFAULT_NORECURSEDIRS`, `MARKING_HOOKS`, `deselect_ids`, `is_virtualenv`, `in_confcutdir`, `path_keywords`, `roots`, `descend`, the `setup.py` rule (`unittest-doctest-rs` map §4.4-4.5) | initial conftests: four implementations with different case sensitivity and order; package roots: three identifier rules | `aye-pytest` and `aye-python-semantic`, each difference kept behind a named mode until the harness says it can go |
| unittest knowledge | `aye-engine-unittest/src/module.rs:31, 365, 397`; `aye-engine-pytest/src/module.rs:178, 203`, `lib.rs:1592` | 25 versus 17 known bases; different "looks like a test class" heuristics | `aye-python-stdlib::unittest`, both heuristics named |
| Engine module stack | `registry.rs:44-59` (twice), `native.rs:69-76` | none | `PRELUDE` (§4.17) |
| Safe repr | `aye_engine.py:878-951`, `aye_native_engine.py:342-405`, `aye_rewrite.py:1078-1131`, the shim's `_saferepr`, unittest's `_safe_repr` | defaults 200, 240, unlimited, 2000; two texts for a broken `__repr__` | `aye_engine.saferepr.SafeRepr` with each site's defaults and wording as parameters |
| Failure building | `Failure.from_exception`; `Engine._failure`; unittest's and doctest's builders | frame hiding: `is_internal` alone versus `__tracebackhide__`, `__unittest` and own files | `aye_engine.failure` with one `FrameFilter` configured per engine |
| Warning filters | front-end `parse_warning_filter`; native engine's; doctest's `_warning_filter` | `all` accepted, kept or refused; ambiguous prefixes refused or first-wins | `aye_pytest.warnings` with the three modes named (finding 9 in §10) |
| Records | three case builders, seven diagnostic builders | pytest omits empty `tags`/`keywords`; only unittest and doctest bound messages on refinement | `aye_engine.records`, each adapter's presence rules kept in `to_wire()` |
| `ran_ids` | a set in `aye_pytest_native.py:628`; a count in `aye_engine_unittest.py:222`, `aye_engine_doctest.py:164` | finding 11 in §10 | `aye_engine.context.RanIds`, both behaviours kept until decided |
| Small Python helpers | id → file (nine sites), `without_junitxml` ×2, `_nothing` ×3, statuses ×3, context coercion ×5, `MAX_NAMED` ×4 | `batch-stopped` texts differ on purpose and stay | `aye_engine.{ids, context, status, messages}` |
| Worker install loop | `aye_worker.py:194-207`; `test_frontend.py:48-140`; the unittest and doctest test conftests | the tests install fewer modules than production | one helper reading the manifest Rust uses |
| Test Python lookup | `aye/tests/{cli,show,compare}.rs`; `aye-engine-pytest/src/{scope.rs:6152-6158, ids.rs:776, tests.rs:185}`; `aye-runner/tests/workers.rs:281-287`; the other adapters' `tests.rs` | probes differ (`import pytest`, `import sys`) | `aye-test::test_python(Needs::{Stdlib, Pytest})` |
| Atomic writes | `aye-history/src/lib.rs:314-338`; `runs.rs:747-761` | file name prefixes | `aye_fs::atomic_write`, prefixes kept |

## 7. Python side

### 7.1 Organisation

The target, once the worker installs packages (protocol 4, §8.2):

```
aye-engine           aye_engine/            __init__ (the facade: every name adapters import today), _stdlib (bound stdlib
                                            callables, `guarded` public), wire, records, ids, context, affinity, outcome,
                                            paths, saferepr, failure, compare, result, capture, services, lifetime,
                                            executor, model (was aye_model), rewrite/ (hook, cache, plan, message),
                                            run/ (engine, scope, resources, conditions, state, errors, logcap, warnings)
aye-runner           aye_worker.py          one file, standard library only, under 16,000 bytes
aye-pytest           pytest                 aye's pytest module: one runtime module, built from ordered source parts
                     aye_pytest/            importing, conftests, warnings, messages, outcomes: pytest's session rules
aye-adapter-pytest   aye_adapter_pytest/    executor, session, listing, selection, frontend, config/, plugins/, conftests,
                                            importer, nodes, marks, collect, fixtures/, parametrize/, model, refined,
                                            runtime/ (request, item, builtins, tmpdir, capture, caplog, explain, unittest)
aye-adapter-unittest aye_adapter_unittest/  discovery, ids, results, lifetimes, executor
aye-adapter-doctest  aye_adapter_doctest/   settings, finding, checker, examples, executor
```

- **The engine** (`aye_engine.py`, 2100 lines) splits along its sections
  (`python-engine` map §1.1) behind a facade that re-exports today's names,
  so no adapter import breaks during the move.
- **The native engine** (`aye_native_engine.py`, 4179 lines) keeps its
  core (about 1,900 lines: `Engine`, resources, setup state, conditions,
  phases) in `aye_engine.run`, handed an `EngineSettings` the front-end
  resolves, instead of calling pytest's `config.getoption`/`getini`
  (`aye_native_engine.py:289-336`). pytest's runtime API (`Request`,
  `Item`/`NodeView`, the builtin fixtures, `TempPathFactory`, `LocalPath`,
  `caplog`, `capsys`, `explain`, the warning syntax, `TestCaseFunction`
  semantics) moves to `aye_adapter_pytest.runtime`, and the engine's `Item`
  merges into the front-end's `Function`: one item class.
- **The front-end** (`aye_pytest_frontend.py`, 5284 lines) splits into the
  `aye_adapter_pytest` modules above (`python-adapters` map §9.2), and
  `Frontend` becomes a composition (`PathSpace`, `FrontendSettings`,
  `Config`, `Plugins`, `Conftests`, `Collector`, `FixtureRegistry`,
  `ModelBuilder`, `CollectionOutcomes`) whose parts take the few others
  they need, instead of each reaching through `self._frontend`. Its
  lifecycle becomes one enum (`NEW → CONFIGURING → CONFIGURED | FAILED`)
  instead of `_started`, `_configured` and `_configure_error`.
- **aye's `pytest` module** (`aye_pytest_shim.py`, 3191 lines) stays one
  runtime module named `pytest`, built by concatenating ordered parts
  (`concat!(include_str!(…), …)`): header, warning types, session,
  outcomes, version, marks, fixtures, types, hooks, code and raises,
  recwarn, approx, monkeypatch, private (the `_pytest` table as one map
  instead of three parallel dicts), distribution, footer. It stays one
  `<aye:pytest>` module with the same names, but `pytest._aye` and the
  merged table shift its line numbers, which reach `detail` until §8.2's
  frame filter lands (§7.2). Its private contract with aye (`_aye_rewrite_modules`,
  `MarkGenerator._config`/`_markers`/`_markers_of`, the attribute names,
  the lookup of `sys.modules["aye_native_engine"]`) becomes one object,
  `pytest._aye`, with `bind_config`, `unbind_config` and
  `set_fixture_lookup_error`, so the module names no aye module.
- **The doctest executor** uses `aye_pytest` instead of its own copies of
  pytest's import modes, conftest order, warning filters and texts
  (`aye_engine_doctest.py:259-591`), each 0.0.1 difference kept behind a
  named mode (§6).
- **Shared helpers** live in `aye_engine`: `ids`, `context` (`BatchContext`,
  `RanIds`), `records` (`CaseRecord`, `Diagnostic` with `to_wire()`),
  `affinity`, `status`, `messages`.
- **Three run loops stay three**: pytest's cases on the model and
  `aye_engine.run`, doctest's on `EngineExecutor`, unittest's on
  `Lifetimes`. Converging them is a Core decision 4 question, not a
  reorganisation.

Constraints: standard library only; Python 3.10 (`(str, Enum)`, no
`StrEnum`). Such members format differently across the Pythons aye
supports: `f"{Code.NOT_FOUND}"` gives `not-found` on 3.10 and
`Code.NOT_FOUND` on 3.11 and later, and `str()` and `%s` give
`Code.NOT_FOUND` on every version (checked on CPython 3.10 to 3.14). So
every such enum defines `__str__ = str.__str__` and `__format__ =
str.__format__`, and a test run on 3.10 and on 3.14, CI's two, formats
each one. Hot paths (`Sink.emit`, `_sanitize_dict`, the front-end's
`_case` and `case_dict`, `Selection.select`) are no slower: `__slots__`
dataclasses whose `to_wire()` builds the dict directly, checked with
`make bench` before and after.

### 7.2 Names that are contracts

- **`pytest`**: its identity (`__version__ == "9.1.1"`, `version_tuple`,
  `__aye__`, `__file__ == "<aye:pytest>/__init__.py"`, `__spec__`,
  `importlib.metadata.version("pytest")`), the 73 names of `__all__` and
  the others suites use (`NOTSET`, `MARK_GEN`, `FixtureSpec`, …), the
  `AttributeError` text for a missing name, the 29 `_pytest.*` aliases and
  the `ModuleNotFoundError` text for the rest, the refusal when a real
  `_pytest` is already imported, and `__module__`: `pytest` on the warning
  classes, `ExitCode` and `UsageError`, `builtins` on `Skipped`, `Failed`
  and `Session.Interrupted`.
- **Qualified names that reach output**: `detail` prints
  `aye_native_engine.FixtureLookupError`, and `aye_native_engine.Failed` or
  `XFailed` when the policy names no class; `aye_engine.Skip` is recognized
  by name (`SKIP_EXCEPTIONS`). Wherever these classes move, their
  `__module__` stays pinned, and a test checks it (§11, Q5).
- **Default reprs of what user code holds**: in `frames[].locals` and in
  `-vv`'s assertion explanations, and so in the stored log, aye's objects
  print as `<module.Class object at 0x…>`. The 0.0.1 binary
  stores, for builtin fixtures, `<aye_native_engine.LogCaptureFixture …>`,
  `CaptureFixture`, `TempPathFactory`, `_Subtests` and `_TempdirFactory`,
  and `<aye_pytest_frontend.Config …>` (`pytestconfig`, `request.config`)
  and `<aye_pytest_frontend._Cache …>`. `__module__` and `__qualname__`
  stay pinned for every class whose instances user code can hold: fixture
  objects, `Config`, `Cache`, `Request` and the node classes. A contract
  test lists them, so the moves of step 12(d) and (f) can't change them.
- **aye's frames in `detail`**: `_detail` skips only the leading hidden
  frames, then formats the rest (`aye_native_engine.py:2713-2717`), so
  every later `<aye:…>` frame prints with its module, function, line
  number and source line, and JUnit writes `detail` as it is
  (`aye-render/src/junit.rs:684-686`). The 0.0.1 binary stores
  `File "<aye:pytest>", line 406, in __call__` for `pytest.fail("boom")`,
  `line 1875, in __exit__` for a `pytest.raises` that doesn't raise, and
  `File "<aye:aye_native_engine>", line 1219, in getfixturevalue`, then
  `line 2231, in _value` and `line 2263, in _active` for a missing fixture
  asked through `request.getfixturevalue`. No reorganisation can keep
  those line numbers, so §8.2 proposes leaving every internal frame out
  of `detail`, as pytest hides its own (Q5). The oracle sees little of
  either today: `normalise_text` masks only `0x…` addresses, the stored
  logs of the current suites hold no `<aye:` at all, and the only aye repr
  in them is that of `_Subtests`, in the `outcomes` suite. Step 0 adds
  cases that show both.
- **What aye leaves on user objects**: `_aye_fixture`, `_aye_hookimpl`,
  `_aye_hookspec`, `Mark._param_ids_from` and `_param_ids_generated`; on
  nodes, `aye_id` and the `_aye_*` caches.
- **Synthesized fixture names** (`_xunit_setup_module_fixture_<module>` …
  `_unittest_setup_method_fixture_<qualname>`, `_aye_package_setup_<module>`),
  which suites see in `fixturenames`. The engine and the front-end stop
  recognizing them by prefix: `ResourceDef.origin: ResourceOrigin` says
  what they are.
- **Assertion rewriting**: `_aye_assert` and `_aye_span` on
  `AssertionError`, the `@aye_t{n}` temporaries, the `@aye_rewrite` and
  `@aye_builtins` helpers in rewritten code, the cache files
  `<name>.<cache tag>-aye.pyc`. Only `aye_engine.rewrite` builds and reads
  `_aye_assert`, through `comparison_of(exc)`.
- **Process-wide effects**: doctest's `ALLOW_UNICODE`, `ALLOW_BYTES` and
  `NUMBER` flags registered at import; `PYTEST_VERSION` and
  `PYTEST_CURRENT_TEST` set; `PYTEST_XDIST_*` read; the temporary
  directories' layout (`pytest-of-<user>/pytest-<N>`, `aye-<run>`,
  `.aye-run`).
- **The executor ABI** the worker calls: `Executor(root, services)`,
  `refine(files, context, *, digests_only=False)`, `run(batch, sink)`,
  `close()`.

### 7.3 What can be renamed

An installed module name other than `pytest` is not a contract by
itself: the worker resolves entries from the handshake, and `frames`
leaves out every `<aye:…>` frame. Module names reach output in the two
ways §7.2 pins: `detail`'s internal frames, until §8.2's filter lands,
and the default reprs, whose `__module__` and `__qualname__` stay as they
are. With both in place, `aye_model`, `aye_rewrite`,
`aye_native_engine` (as a module), `aye_pytest_frontend`,
`aye_pytest_native`, `aye_engine_unittest`, `aye_engine_doctest`, the
entries (`aye_adapter_pytest.executor:PytestExecutor`, was
`aye_pytest_native:NativeExecutor`) and the executor classes may change
together with the Rust side, in protocol 4. Renaming the rewrite helper's
module invalidates the assertion-rewrite cache once. The class names
`Channel` and `Worker` in `aye_worker.py` stay while the runner's tests
find them through `gc` (`aye-runner/tests/workers.rs:62-64, 172-184`).

### 7.4 How the worker gets there

Rust sends a package's submodules in dependency order and its `__init__`
(the facade) last. So the parent must exist first: a submodule's `from
aye_engine.wire import …` makes the import system look up `aye_engine` in
`sys.modules`, and binding `aye_engine.wire` needs a parent to bind into
(`python-engine` map §10.4). The mechanism:

1. Before the first submodule of a package, the worker puts a bare package
   module in `sys.modules`, with `__path__ = []` and a `__spec__`
   (`ModuleSpec(name, None, is_package=True)`).
2. `install(name, source, package)` installs each dotted submodule with
   `__package__` set to its parent, and binds it into the parent
   (`setattr(parent, child, module)`).
3. The package's `__init__` source is executed last, into that same module
   object, so `sys.modules["aye_engine"]` is one object throughout.
4. A flat module, `pytest` among them, is installed exactly as today:
   no `__path__`, no `__spec__`, so the shim's own `__file__ ==
   "<aye:pytest>/__init__.py"` and its `ModuleSpec(origin=__file__)`
   (`aye_pytest_shim.py:105-117`) still apply. Step 0's contract test of
   §7.2 covers it.

The worker is 15,967 bytes against its 16,000-byte cap
(`aye-runner/src/lib.rs:262`), 33 to spare. The bytes are budgeted:

| | Bytes |
|---|---|
| Spent: the bare parent, dotted install and binding, executing `__init__` into the parent | about 400, measured in step 12(b) |
| Freed: the module docstring, into `docs/protocol.md` | 2,350 |
| Freed: the default listing digests, into `Executor.digests` | about 600 (`python-engine` map §10.4) |
| Freed: the `co_varnames` probe (`aye_worker.py:310-313`), every executor taking `digests_only` | about 250 |

The cap test (`aye-runner/src/lib.rs:258-263`) checks the result. If RJ
declines package support (§11, Q4), the same split works as flat modules
prefixed by the package name (`aye_engine_wire`, …).

The Python tests install a crate's module set as the worker does, from the
manifest Rust uses, replacing four install loops; the unittest and doctest
tests install the production set, aye's `pytest` included, which today
they omit (finding 22 in §10).

### 7.5 The Python model

The engine's and the adapters' Python carry the same kind of structured
strings and tuples as the Rust side (`python-engine` map §0, problem 3).
Each concept gets one type: a `__slots__` dataclass, a `NamedTuple`, or a
`(str, Enum)` under §7.1's formatting rule. None of them is on the wire,
so none changes a byte there; the classes user code can hold keep their
`__module__` and `__qualname__` (§7.2).

| Concept | Today | Type | Module | Constraint | Step |
|---|---|---|---|---|---|
| A setup scope's key | tuples: `("session",)` (`aye_native_engine.py:159`), `("node", node.kind, node.id)` (`:772`), `("case", index)` (`:1306`) | `ScopeKey`: frozen slots classes `SessionKey`, `NodeKey(kind, id)`, `CaseKey(index)`; `label` gives today's `key[-1]` text | `aye_engine.run.scope` | hashing and equality as the tuples' | 12(d) |
| A source location | `ResourceDef.location: str`, `"path:line"` (`aye_model.py:70`), beside `Case.location: tuple[str, int]` (`:130`) | `SourceLocation(NamedTuple path, line)`, whose `str()` is `"path:line"` | `aye_engine.model` | the text reaches messages unchanged | 12(c) |
| Node and case kinds | free strings: `Node.kind` (`aye_model.py:79`: package, module, class), `Case.kind` (`:133`: function, unittest) | `NodeKind(str, Enum)`, `CaseKind(str, Enum)` | `aye_engine.model` | equal to today's strings | 12(c) |
| The framework's outcome rules | `aye_model.Policy` (`aye_model.py:147`), unrelated to `aye_engine.Policy`, the wire's batch policy (`aye_engine.py:1883`) | `OutcomeRules`, its exception classes grouped as `FrameworkExceptions(skip, xfail, fail, exit)`; `Policy` kept as an alias while the front-end moves | `aye_engine.model` | the classes it names are §7.2's | 12(c) |
| A lifetime's state | `Lifetime.is_open` beside `failure` and `skip_reason` (`aye_engine.py:1730-1786`) | `LifetimeState(Enum)`: `CLOSED`, `OPEN`, `FAILED`, `SKIPPED` | `aye_engine.lifetime` | `failure` and `skip_reason` stay: executors read them | 12(c) |
| A resource's cached value | `_Slot.cached: (value, key, exception)` (`aye_native_engine.py:1232-1240`) | `Cached(value, param_key, error)` | `aye_engine.run.scope` | | 12(d) |
| A stop | `(why, status)` tuples | `Stop(reason, status)` | `aye_engine.run.engine` | `status` is the framework's (§4.13) | 12(d) |
| A run's options | keyword arguments and `config.getoption`/`getini` reads (`aye_native_engine.py:289-336`) | `EngineSettings`, resolved once by the front-end; `RunOptions(maxfail, capture, rerun, keep_session)` | `aye_engine.run.engine` | | 12(d) |
| A case's progress | `CaseState`, one bag with `setup_ok`/`call_ok` booleans | `CaseTags`, `CaseValues`, `CaseProgress` (the `Phase` reached) | `aye_engine.run.state` | `make bench`: a per-case hot path | 12(d) |
| A rerun command | three builders (§4.22) | `RerunCommand(rerun_args, framework_args)`, `for_test(id) -> Reproduce` | `aye_engine.result` | `Reproduce.command` as today | 12(a) |
| A parameter binding | `CallSpec2`'s parallel dicts (`params`, `indices`, `_arg2scope`, `_ids`, …; `aye_pytest_frontend.py:1839-1853`) | `ParamBinding(NamedTuple value, index, scope, id)`, one per argument | `aye_adapter_pytest.parametrize` | `CallSpec2`'s attributes stay as views: conftests read them | 12(f) |
| Direct or indirect parametrization | `dict[str, "direct" \| "indirect"]` | `direct: frozenset[str]` | `aye_adapter_pytest.parametrize` | | 12(f) |
| Hook order | `tryfirst`/`trylast` booleans | `HookOrder(Enum)`: `FIRST`, `NORMAL`, `LAST` | `aye_adapter_pytest.plugins` | pluggy's order, as today | 12(f) |
| A conftest that failed | `_ConftestFailed.kind: str` (`error`, `skip`, `unsupported`; `aye_pytest_frontend.py:363-371`); the doctest executor's `(code, rel, text)` tuples | `ConftestFailureKind(Enum)`; `ConftestProblem(kind, path, text, initial)` with `diagnostic()` | `aye_pytest.conftests` | the diagnostics' codes and texts unchanged | 12(e) |
| A pytest outcome | the doctest executor's `_pytest_outcome` strings | `PytestOutcome(Enum)`: `SKIP`, `XFAIL`, `FAIL`; `of(exc)` through the shim's classes, imported, not looked up in `sys.modules` | `aye_pytest.outcomes` | | 12(e) |
| A warning filter | three parsers (finding 9) | `WarningFilter(NamedTuple action, message, category, module, lineno)`, one parser with the three modes named (§6) | `aye_pytest.warnings` | each mode's 0.0.1 answers | 12(e) |
| A unittest unit's place | `units_of` 4-tuples, `Unit.kind` strings | `UnitPlace(NamedTuple kind, path, start, top)`, `UnitKind(Enum)` | `aye_adapter_unittest.discovery` | | 12(g) |
| A planned unittest test | `_plan`/`_resolve` triples | `PlanEntry(test_id, test, skip)`; `Resolution`: `Found`, `SkippedModule`, `Problem` | `aye_adapter_unittest.executor` | | 12(g) |
| A doctest file loaded | `Loaded`, four optionals and a `"missing"` sentinel | `LoadedFile`: `Doctests`, `Skipped`, `Failed`, `Missing` | `aye_adapter_doctest.finding` | | 12(g) |

From the maps' lists (`python-engine` map §10.2, `python-adapters` map
§9.4), `ResourceOrigin` is §7.2's, and the shared helpers (`ids`,
`context`, `records`, `affinity`) are §7.1's. Only `IdentityMap`, a
utility rather than a model type, is deferred: it waits for a second
user.

## 8. Wire contracts

### 8.1 What must not change

| Contract | What it covers | Pinned by |
|---|---|---|
| Command line | every flag, alias and value of `aye` and `aye show` (`aye/src/args.rs:14-265`), `args_override_self`, `-k`/`-m` hyphen values, `--output` in any case with the `human`/`agent` errors, `-n auto`/`logical`/N, `--native` accepted and listed, `show`/`config` only as `argv[1]` and only when no such path exists, the `-- ARG` usage errors (`REPLACING`, `args.rs:333-377`); `aye --help` and `aye show --help`, each argument's place and words (§4.14) | oracle (help and usage errors, step 0); `tests/cli` |
| Exit codes and stderr | 0-5 as `docs/events.md` "Exit codes"; clap errors 4, help 0; `aye show` 0/4/5; every `aye: error: …` and `aye: warning: …` line (`binary` map §15.7) | oracle |
| Output | text (`aye-render/src/text.rs:1-140`), JSON lines (types, field order, limits; `json.rs:1-186`), JUnit XML, GitHub annotations, `aye show` (list, details, `--runs` with `error` where the summary says `errors`, `--log`), the `collecting…` line | oracle; renderer tests |
| Event log | `Record` lines: the envelope's fields then `event` then the event's in declaration order; `phase: null` and the seven `counts` always written; `collect_only` only when true; `run_number`; zeros, empty lists and `None`s left out as today; durations rounded to µs; unknown kinds tolerated on read; `SCHEMA_VERSION = 1` | `aye-events/src/tests.rs`; the golden fixture of §9 step 0 |
| Stored data | `.aye/runs/r<N>.jsonl.gz`, `r<N>.json` (`RunMeta` field order, `extra` keys alphabetical: `SummaryExtra` declares its fields in that order), `last`, legacy hex runs, retention; `.aye/history.json` version 1; the `.gitignore` and `CACHEDIR.TAG` markers and the 0.0.1 repair; stores written by 0.0.1 readable by 0.1.0 | `aye-history` tests; oracle (`show`) |
| Stable algorithms | `TestKey` (FNV-1a 64 + SplitMix64), the failure signature (SHA-1, 16 hex), file digests (FNV-1a 128, 32 hex), listing digests (SHA-1 of ids joined by `\n`) | tests; oracle (keys) |
| Configuration | `[tool.aye]` keys and every error text (`aye-config/src/lib.rs:369-498`); `[tool.aye.unittest]`, `[tool.aye.doctest]`; iniconfig semantics; pytest's discovery as each adapter does it today; the root rule | config tests |
| `aye config` | its lines, `engine   native`, the two placeholder lines, every origin's text | test of §4.10; oracle (step 0) |
| Diagnostics | codes, severities and messages, refine reasons included (they reach `not-refined` infos) | oracle; collection and adapter tests |
| Environment | set: `PYTHONIOENCODING`, `PYTEST_XDIST_TESTRUNUID`, `AYE_WORKER`, `PYTEST_XDIST_WORKER(_COUNT)` with more than one lane, `PYTEST_VERSION`, `PYTEST_CURRENT_TEST`; read by name: `AYE_PYTHON`, `VIRTUAL_ENV`, `PATH`, `HOME`/`USERPROFILE`, `PYTEST_ADDOPTS`, `PYTEST_PLUGINS`, `PYTHONPATH`, `PYTHONOPTIMIZE`, `PY_IGNORE_IMPORTMISMATCH`, `PYTEST_DISABLE_PLUGIN_AUTOLOAD`, `GITHUB_ACTIONS`, `GITHUB_WORKSPACE`, `NO_COLOR`, `CLICOLOR_FORCE`; expanded: any variable a user names in `--junitxml` (`$NAME`, `${NAME}`, then `~`) and in the paths pytest expands (`--rootdir`; on Windows `%NAME%` too), looked up as `os.environ` does, ignoring case on Windows | `aye-static`'s catalogue test; expansion tests over `ProcessEnv` |
| Worker protocol (internal, versioned) | `hello`, `run`, `refine`, `shutdown`; `ready`, `refined`, `batch_done` (status normalized, `null` accepted), `failed` (last 8000 characters); strict JSON from the worker; the spawn command and fd layout; the 16,000-byte cap | `aye-protocol` golden fixtures; worker tests |
| Contexts (internal) | the keys and spellings each executor reads (`python-adapters` map §5); `selected_paths` not session-neutral; `ran_ids` making `null` into `{}`; the binary's keys only into objects | context tests |
| Refinement cache (internal) | `.aye/cache/refine/<adapter>/<32 hex>.json`; a cached answer equals a fresh import; any change to the key's composition or the entry bumps `FORMAT` (`aye-collect/src/refine.rs:18`) | collection tests |
| Python-visible names | §7.2 | contract tests (step 0) |
| Comparison harness | `expected.json`, `tests/examples/<name>.json`, `record.py`, `make expected`, the `AYE_*` test variables | `tests/compare.rs` |

### 8.2 Deliberate changes

| Change | Why | Consequence |
|---|---|---|
| `ModuleSource` gains `package`; `Policy` loses `keyword`, `tags` and `verbosity`; `refine` always passes `digests_only` | packages in the worker (§7.4); the three fields are read by no executor, and static selection is the last word (Core decision 5); the worker stops introspecting | `PROTOCOL_VERSION` 3 → 4 in Rust, `aye_worker.py` and `aye_engine`; `docs/protocol.md` rewritten. The worker is embedded, so no older one can meet a newer binary. `Policy.timeout` stays, reserved for Step 4 |
| The context loses what no Python reads (`native.options`, `unknown_options`, the hint keys), and `modules_digest` covers the whole prelude | dead data in the refinement key | refinement `FORMAT` `refine-2` → `refine-3`: caches miss once. Optional for 0.1.0 (§11, Q10) |
| The rewrite helper's module moves under `aye_engine` | the package layout | the assertion-rewrite cache misses once |
| `detail` leaves out every `<aye:…>` frame, not only the leading ones (§7.2) | otherwise every edit to aye's Python changes stored and printed text (module names, functions, line numbers), and pytest hides its own frames | an output change in `detail`: the stored log, `-vv`, JUnit XML. Recorded in `docs/compatibility.md`; lands before step 12 as its own `fix:` commit, and the oracle's reference binary is rebuilt from that commit (§11, Q5) |
| Nothing else | | no `SCHEMA_VERSION` bump; `docs/events.md` gets corrections only (finding 14 in §10) |

## 9. Migration order

Every step keeps the workspace building, every test passing and the
oracle at 180/180 (more, once step 0 widens it). Each lands as one or a
few `refactor:` commits; the crate renames in step 3 are the breaking
change (`refactor!:`), which makes the next release 0.1.0. Each step
updates the docs it touches (`docs/architecture.md`'s "Crates", the
"Layout" step 1 ports into AGENTS.md, each `lib.rs`), and each step that
adds or renames a crate updates `release-please-config.json` (§3.4).

| # | Step | Changes | Crates | Parallel | Risk |
|---|---|---|---|---|---|
| 0 | Guards | Golden fixtures written from 0.0.1: every event kind, every protocol message, `RunMeta`, `history.json`, byte-exact round-trips. A strict round-trip of Python's records: the engine's `Sink`, `Result` and `Failure` on a fixture set, each record read as a Rust `Record`, written back and compared as JSON values, which catches a field serde drops (`python-engine` map §10.1). Python-Rust constant tests (outcomes and severities, phases, services, `PROTOCOL`, the codes Python emits). Contract tests for §7.2 (the `pytest` module's identity and `__module__` values, the qualified names in `detail`, the `__module__` and `__qualname__` of every class user code can hold). The oracle widened: `aye config`; `aye --help` and `aye show --help`; usage errors, whatever Q11 decides (a bad `-k`, `-- --pdb`, `--output human`, `-n x`, an unknown adapter, `[tool.aye]` type errors); `--junitxml` with the XML compared; aye's own code in output, each case also with `--junitxml`: `pytest.fail`, a `pytest.raises` that doesn't raise, a missing fixture through `request.getfixturevalue`, builtin fixtures' locals at `-vv`; `GITHUB_ACTIONS=true`; `--lf` and `--ff` on a second run; a second run with `.aye` kept (the refinement cache); `--co --no-refine`; `-s`; `aye show <key>` and `aye show <path>`; recalibrated old against old. A `make bench` baseline | tests, oracle | yes, each guard alone | none |
| 1 | Leaves and hygiene | `aye-index`, `aye-static`, `aye-fs` (path helpers with both normalizers named, `StateDir`, markers, `atomic_write`). Move `prepare_dir` and the markers into `aye-fs`, then drop the edge `aye-collect` → `aye-history` (live today: `aye-collect/src/refine.rs:394`). Drop the dead edges `aye-runner` and `aye` → `aye-provision`, and the binary's duplicate dev `serde_json`; keep `aye-provision` → `aye-events`, which `EnvironmentFingerprint` needs. Port the 0.0.1 AGENTS.md's Vocabulary, Layout, Commands and Rules into AGENTS.md. release-please entries for the three new crates, and the CI check that every workspace member has one (§3.4). The new lints, with `#[expect]` where the code means it | new 3, `aye-config`, `aye-collect`, `aye-history`, `aye-runner`, `aye`; AGENTS.md, release configuration | the three leaves | low |
| 2 | The event vocabulary | `aye-events` split into modules first, then one type at a time with its call sites: `TestId` and `ProjectPath` (the `[` fix as its own `fix:` commit, Q12), `FramePath`, `TestKey`, `AdapterName`, `DiagnosticCode` with the three rules pinned, ordered `Severity`, `LifetimeId`, `Scope`, `AffinityKey`, `WorkerIndex`, `BatchIndex`, `RunId`, `RunNumber`, `StopReason` and `Stamper` (from the runner), `OutcomeTrail` (from the history), `FrameworkStatus`, the failure types (§4.21), `RerunCommand`, `digest` | `aye-events`, then every crate | per type, after the module split; merges in sequence | medium: tolerant reads, `TestId` order |
| 3 | Split the contract; rename | `aye-engine` becomes `aye-protocol`, `aye-adapter` and `aye-engine` (the Python engine); the runner stops compiling Ruff. The `get-size2` pin moves with the Ruff re-exports to `aye-adapter`, and the workspace `Cargo.toml`'s comment says so (`aye-engine/Cargo.toml:14`). `aye-engine-{pytest,unittest,doctest}` become `aye-adapter-*`, and `release-please-config.json` follows (§3.4). Docs and AGENTS.md follow | all | no: one mechanical change | low behaviour, high churn |
| 4 | A typed contract | `BatchContext`, `AdapterContext` built once, `FrameworkDefaults` (the binary stops reading JSON), `Requested` in `claim`, `explicit` passed to `recognize`, `Recognizer::claims_any_name`, `Project.env` (the whole environment, §2.3) and `Project.tool_aye` (adapters stop reading the process environment and reloading `pyproject.toml`), `Setting` with `ValueSource`, `RefineReason`, `Runtime` and `SessionMode` in `aye-protocol` from `Adapter::runtime()`, `EntryPoint`, `ModuleSet`, `ServiceName`. This step introduces `ValueSource` and `WorkerCount`, which step 5 uses. The registry composes the prelude, from `aye-adapter-pytest`'s shim and the modules it stands on until step 12(e) moves them into `aye-pytest`, and `WithAyePytest` goes; `modules_digest` keeps hashing today's six modules in today's order (§4.17) | `aye-adapter`, `aye-protocol`, adapters, `aye-collect`, `aye-runner`, `aye` | the context, module and setting work apart | medium: "the first context that gives it" order |
| 5 | Settings and the binary's edge | `AyeOptions` → `AyeSettings` with typed values over step 4's `ValueSource` and `WorkerCount`; `EnvironmentOptions` with the whole environment; interpreter discovery into `aye-provision`; `cli/`, `settings/` (`RunSettings::resolve` in today's order), `Verbosity` and `Level`, `RunVerdict` with `selected`, `AyeError`, `printer.rs`, `lib.rs::main` | `aye-config`, `aye-provision`, `aye-runner`, `aye-render`, `aye` | `aye-provision` beside `aye-config` | medium-high: which usage error wins depends on the order; exact messages |
| 6 | The binary's commands | `commands/run/*` (`select.rs` holds the `-k`/`-m` policy), `stream/*`, `commands/show/*` (`feed.rs` over today's fold until step 7, `runs.rs` for `--runs`), `commands/config.rs`, a data-driven registry with `AdapterTraits`; `aye-test`, and `tests/` regrouped on it | `aye`, `aye-test` | run, show, tests | medium: record pass order, the `Reach` patch order (`session.rs:487-490`) |
| 7 | Consumers | `aye-stream` (folds out of the renderers and the binary, `FileChange`), the renderers' module trees, JSON schema structs, `time` out of `show.rs` into aye-render's `words`, `commands/show/feed.rs` moved onto `RunIndex`, `DetailLevel`, `RenderMode`, the GitHub environment read moved to the binary; `aye-history`'s `history` and `store`, `WriterState`, `RunSelector` | `aye-stream`, `aye-render`, `aye-history`, `aye` | render and history after `aye-stream` | medium: each renderer's own rule kept (§6) |
| 8 | Collection and selection | `FileId`, `AdapterId`, `RecognizerSet`, `Targets`, `CollectDiagnostic`, `refine/{cache, key}` with a typed key giving the same bytes, `Cacheability`; `AdapterSelection`, `SelectorIndex`, `SelectedCase`, `FailedEntry`, the `--ff` order out of the binary, `Selector::reaches` with `dotted_reach`'s rule as a named mode (§6) | `aye-collect`, `aye-select`, `aye` | the two crates | medium: walk and merge order, cache key bytes |
| 9 | The runner | modules first (pure moves), then `BatchSpec`, `RunPhase`, `Lane`, `MaxfailBudget`, the worker typestate | `aye-runner` | beside 6 to 8 | high: concurrency, `maxfail`, crash recovery |
| 10 | Framework libraries | `aye-python-stdlib` (`fnmatch` with both translations, `glob`, `os_path`, reprs, tables), then `aye-python-semantic` (resolution, packages, qualified names, C3, the pytest adapter's namespace and escape analysis; unittest and doctest keep their own namespace models for now, their questions differ) and `aye-pytest` (configuration with `ConfigRules::{Exact, Approximate}`, argv, ini, conftest and doctest-plugin rules, `names`) | new 3, adapters, `aye` | semantic beside `aye-pytest` | high: pytest's semantics; every copy's behaviour kept behind a named function or mode |
| 11 | Adapter internals | the trees of §5 for the three adapters; typed contexts; unittest's `ModuleIndex` arena (which also ends the race of finding 5 in §10, as its own `fix:` commit, Q12) | the adapters | per adapter | medium |
| 12 | Python | (a) shared helpers in `aye_engine` (`ids`, `context`, `records`, `affinity`, `status`, `messages`), executors switched; (b) protocol 4 and package support; (c) `aye_engine` package behind its facade, one `saferepr`, one `CleanupStack`; (d) the native engine's core into `aye_engine.run`, pytest's runtime API into `aye_adapter_pytest.runtime`; (e) aye's `pytest` module split by concatenation into `aye-pytest`, `pytest._aye`, `aye_pytest`, the doctest executor switched; (f) the front-end package and typed `FrontendSettings`, its tests on the production context; (g) the unittest and doctest packages; the Python tests on the production module set. Each sub-step brings §7.5's types for the modules it moves | `aye-engine`, `aye-runner`, `aye-pytest`, adapters | (c) to (g) after (b), largely apart | high: hot paths, module identity |
| 13 | Release | `docs/architecture.md`, `docs/events.md` (finding 14 in §10), `docs/protocol.md` (v4) reviewed as a whole; benchmarks against step 0's baseline; 0.1.0 | docs | | low |

Steps 0 to 5 go in order. The others depend on one another as follows:

| Step | Needs | Why |
|---|---|---|
| 6 | 5 | `RunSettings`, `RunVerdict` |
| 7 | 5, 6 | `Level` for `DetailLevel`; it moves 6's `feed.rs` onto `RunIndex` |
| 8 | 4 | the typed contract |
| 9 | 4 | the protocol types; beside 6 to 8 |
| 10 | 4 | `aye-adapter` and `aye-protocol`; beside 5 to 9 |
| 11 | 10 | `aye_python_semantic::c3`, `aye_pytest::PytestConfig` |
| 12 | 4, 10 | (e) moves the shim into `aye-pytest`; (c) to (g) after (b) |

Steps 4 to 8 all edit `aye/src/session.rs` (4 at :1573-1714, 5 at
:124-246, 6 at :282-674, 7 at :1328-1384, 8 at :394-413 and :566-568), so
they merge one at a time, in that order. The others proceed beside them,
crate by crate, and every merge re-runs the oracle.

## 10. Findings

These are behaviour gaps and bugs found while mapping. The reorganisation
keeps every one of them but findings 1 and 5 (§9, steps 2 and 11); each
other is fixed separately, afterwards, with a test and, where pytest is the
referee, the harness.

| # | Finding | Where |
|---|---|---|
| 1 | `TestId::without_params` leaves `tests/t[1].py::test_x[a]` whole: it finds the first `[` in the path | `aye-events/src/lib.rs:122-127` |
| 2 | pytest's and doctest's `fnmatch` differ from CPython for `[z-a!…]`-shaped sets; doctest's is exponential (340 ms for a 40-character name and 7 stars) | `aye-engine-{pytest,doctest}/src/fnmatch.rs` |
| 3 | For a path outside the root sharing only `/` with it, collection climbs with `..` and the binary writes an absolute path, so a selector can miss collection's id | `aye-collect/src/tree.rs:294-316`, `aye/src/session.rs:1952-1981` |
| 4 | unittest's module lookup tries `a/b.py` before `a/b/__init__.py`; CPython's FileFinder takes the package first | `aye-engine-unittest/src/module.rs:2233-2251` |
| 5 | Two threads can each parse one file and get two `Arc<Summary>`s, so `Node` equality by pointer misjudges an MRO | `aye-engine-unittest/src/module.rs:2215-2231, 2564` |
| 6 | `Span.path` can be absolute for an inherited method defined outside the root; the Python side avoids it, so static and refined spans differ | `aye-engine-unittest/src/lib.rs:521-531`; `aye_engine_unittest.py:1170` |
| 7 | In a worker, doctest's `importlib` import mode and `consider_namespace_packages` fall back to `prepend`: aye's `pytest` serves no `_pytest.pathlib`, and the `ImportError` is swallowed | `aye_engine_doctest.py:492-503`; `aye_pytest_shim.py:2987-2993` |
| 8 | A conftest's `config.getoption("maxfail")` is 0 under `aye -x`, `aye -- -x` and `aye -- --maxfail=3` alike (checked with 0.0.1), for two reasons. (a) aye's own `-x`/`--maxfail` never reaches the front-end's config: it goes only to `Policy.maxfail`, while Rust inserts pytest's own `config.maxfail`, unset under `aye -x`; fixed by giving the front-end the run's maxfail. (b) For pytest's `-x` from `addopts`, `PYTEST_ADDOPTS` or after `--`, argparse's defaults shadow the `maxfail`, `xmlpath` and `junitprefix` Rust inserts; fixed with finding 15 | `aye/src/session.rs:676-686`; `aye-engine-pytest/src/native.rs:286-299`; `aye_pytest_frontend.py:3198-3202` |
| 9 | Three warning-filter grammars (`all` accepted, kept or refused; ambiguous prefixes) | front-end `:5111-5141`; `aye_native_engine.py:3048-3101`; `aye_engine_doctest.py:264-298` |
| 10 | `PYTEST_CURRENT_TEST` holds the aye id, not pytest's node id, when the rootdir isn't the root; the doctest executor sets `PYTEST_VERSION` and never restores it | `aye_native_engine.py` `_set_env`; `aye_engine_doctest.py:1269-1298` |
| 11 | The pytest executor reads `ran_ids` as a set, the others by count, as `docs/architecture.md` (Step 3) says the adapters do | `aye_pytest_native.py:628, 650` |
| 12 | A second session in one worker inherits the first's plugin modules for assertion rewriting (`_aye_rewrite_modules` is shared by identity) | `aye_pytest_frontend.py:2741, 2957` |
| 13 | The exit code, the summary and the store classify codes differently: `adapter-error`, `protocol-error`, `batch-failed` and `no-executor` are internal for the summary by code, and for the exit code only through the runner's `internal_error` flag (§4.9's `RunnerInternal`); `not-found` counts by origin in two of the three | `aye/src/session.rs:1186-1197`; `aye-render/src/tally.rs:252-264`; `aye-history/src/runs.rs:513-526` |
| 14 | `docs/events.md`'s code table lacks `listing-differs`, `plugin-not-loaded` and `unknown-option` (mentioned in prose only), lists `stand-in` and `adapter-error` (never emitted), documents a `batch-stopped` warning (never emitted), omits the `session` lifetime kind and the `date` and `time` comparison kinds; `docs/protocol.md` says the legacy context keys are gone while the front-end reads them | `docs/events.md:189, 228, 244-269`; `docs/protocol.md:103-106` |
| 15 | Dead data in the protocol and the context: `Policy.keyword/tags/verbosity` and `timeout` read by no executor; `native.options` (a 355-line Rust table), `unknown_options` and seven other `native` keys read by no Python | `aye-engine/src/lib.rs:468-509`; `aye-engine-pytest/src/native.rs:282-342, 568-923` |
| 16 | Reserved wire fields no one sets: `Case.requires`, `owner`, `quarantine`, `serial` (set only in tests), `timeout` (set, never read), `Reproduce.inputs`, `Envelope.environment`; the worker's `t_ms` is replaced unread | `aye-events/src/lib.rs:432-484, 407-413, 698-714` |
| 17 | The run store re-serializes records, so an event kind it doesn't know is stored as `{"event":"unknown"}` without its fields; a line that doesn't parse is dropped silently on read | `aye-history/src/runs.rs:306, 646-656` |
| 18 | JUnit doesn't deduplicate repeated error diagnostics, contrary to the crate docs; renderers title a failed lifetime differently (`test of X`, `lifetime of X`, `setup of X`); JSON's `lifetime_fail.phase` can be `test` | `aye-render/src/junit.rs:505, 550-576`, `lib.rs:89-91`, `json.rs:1111` |
| 19 | `RunStore::runs` breaks ties by id string, so `r10` sorts before `r9`; `--output json`'s help describes an old ladder | `aye-history/src/runs.rs:564`; `aye/src/args.rs:607-610` |
| 20 | An unknown doctest option flag in the context raises `KeyError` while the executor builds its settings | `aye_engine_doctest.py:214-219` |
| 21 | Refinement sizes the warm worker's lane environment by `workers`, so a run with fewer lanes than workers can't reuse it | `aye-runner/src/run.rs:567, 763` |
| 22 | The Python tests don't run production's configuration: `test_frontend.py` mostly sends the legacy context; the unittest and doctest conftests install neither aye's `pytest` nor the native engine, and doctest's tests run with a real pytest importable | `test_frontend.py`; the two `python/tests/conftest.py` |
| 23 | Dead code: `Environment::installs`, `Collection::has_errors`, `RunStore::find`, `History::estimate`, `durations_by_group`, `is_last_failed` (tests only), `registry::choose`'s duplicate check, `Frontend.collect` and `_visible`, the engine's fallback unittest class fixture | `aye-collect/src/environment.rs:230`; `aye/src/registry.rs:178-180`; `aye_native_engine.py:2145-2209` |
| 24 | Stale docs: `aye-select`'s and `Policy`'s "the engine re-applies both expressions", `aye-collect`'s header, `aye_engine.py`'s docstring, `aye_native_engine.py`'s and `aye_model.py`'s references to files that don't exist, `aye-runner`'s and `aye-history`'s promised features | `aye-select/src/lib.rs:14-16`; `aye-engine/src/lib.rs:481-486`; `aye-runner/src/lib.rs:1-37` |
| 25 | `[tool.aye]` is parsed several times per run (the binary, unittest's and doctest's `prepare`, idle adapters' checks); validating an idle adapter's table runs its whole preparation | `aye-engine-unittest/src/config.rs:43`; `aye-engine-doctest/src/lib.rs:774`; `aye/src/registry.rs:189-193` |
| 26 | A crash report leaves out every stderr line made only of `.sxXFE`, pytest's progress letters, so a crashing test whose own stderr ends in `...` or `E` loses those lines. The filter is live: removing it changes crash reports, so it is a deliberate change, with a test | `aye-runner/src/worker.rs:793-808` |

## 11. Open questions for RJ

1. **How many crates?** The plan has 23, 22 of them published (§3.1).
   Fewer is possible at the cost of a boundary each: `aye-static` into
   `aye-fs`, `aye-python-stdlib` into `aye-python-semantic`, `aye-stream`
   into `aye-render` (the history then depends on the renderer), which
   leaves 20. *Recommended:* the 23. Each new crate enforces a rule the
   code broke at least once, and one release publishes them all at one
   version.
2. **The names.** `aye-adapter` for the contract next to `aye-adapter-*`
   for the adapters (uv's `uv-python` and `uv-python-*`); `aye-engine`
   reused for the engine; `aye-protocol`; `aye-pytest` for pytest as a
   library. *Recommended:* as proposed; the alternative for the contract is
   `aye-contract`.
3. **Where interpreter discovery lives.** `aye-provision`, so the reserved
   crate gets its first code and Step 5 grows it, or a new
   `aye-interpreter`. The trade-off: in `aye-provision`, collection
   depends on a provisioning crate for one type, `EnvironmentFingerprint`.
   The runner map's `aye-python` (`runner` map §9.1) would let collection
   use interpreter facts without that edge. *Recommended:* `aye-provision`;
   if the edge grows beyond the fingerprint, the fingerprint moves into
   `aye-events` beside `ContentDigest`.
4. **Package support in the worker** (protocol 4, a few hundred bytes paid
   for by moving the docstring out). *Recommended:* yes, in step 12(b). Without
   it, the Python split uses flat prefixed modules (§7.4).
5. **aye's own names in output** (§7.2): the qualified names in `detail`
   (`aye_native_engine.FixtureLookupError`, `Failed`, `XFailed`), the
   default reprs of the objects user code holds
   (`<aye_native_engine.LogCaptureFixture object at 0x…>`,
   `<aye_pytest_frontend.Config …>`), and aye's internal frames after the
   first user frame in `detail`, with their module, function and line
   number. *Recommended:* keep the names and reprs pinned for 0.1.0, and
   rename them later as a documented output change, since
   `aye_native_engine` is prototype vocabulary. Leave every internal frame
   out of `detail` before step 12, as its own `fix:` commit (§8.2): no
   reorganisation of the Python can keep their line numbers, and pytest
   hides its own frames.
6. **Affinity keys and lifetime ids**: kept byte-identical behind
   `AffinityKey` and `LifetimeId`, or normalized to one `kind:…` grammar
   (schema 2, and the `session` lifetime named for what it closes).
   *Recommended:* keep them for 0.1.0.
7. **Reserved wire fields** (finding 16 in §10): keep them (optional, they cost
   nothing on the wire) or remove them (schema 2). *Recommended:* keep them,
   grouped and documented as reserved.
8. **`stand-in` and `adapter-error`**, documented and never emitted. Any
   log reads either way, since `DiagnosticCode::Other` takes any code, so
   the choice is how typed these names are. Reserved variants keep them
   typed and keep 0.0.1's classification exact (the summary counts
   `adapter-error` as internal, `aye-render/src/tally.rs:261`); a smaller
   enum keeps `ALL` to what aye emits, and would read either name as
   `Other`, a collection error. *Recommended:* remove them from
   `docs/events.md` and keep them as reserved variants until the next
   schema change.
9. **Which fixes follow the move, in which order.** *Recommended:*
   findings 2, 4 and 17 of §10 first, as `fix:` commits (each moves towards
   CPython or correctness, with tests); then 7, 8(a) and 11 with the
   harness; the doctest adapter's switch to exact pytest discovery last,
   recorded in `docs/compatibility.md`, since it changes which rootdir and
   options some projects get.
10. **Removing dead context data** (§8.2, refinement `FORMAT` 3).
    *Recommended:* after 0.1.0, with findings 8(b) and 15 of §10, so
    0.1.0's caches stay warm across the upgrade.
11. **Snapshot tests** with insta and insta-cmd (Apache-2.0, dev-only) for
    `aye config`, the text and JSON outputs, both help texts and the usage
    errors, beside the oracle, which compares the help texts and usage
    errors from step 0 either way. *Recommended:* yes, recorded against
    0.0.1 in step 0: the oracle diffs two binaries, snapshots keep the
    guard once 0.0.1 is gone.
12. **Two fixes inside the move** (findings 1 and 5 of §10). Both change
    behaviour, and the new structure removes them by construction: one
    `TestId::parts` can't keep `without_params`' answer for
    `tests/t[1].py::test_x[a]`, and unittest's `ModuleIndex` arena parses
    each file once, so no two `Arc<Summary>`s of one file exist.
    *Recommended:* accept both, each as its own `fix:` commit inside its
    step (2 and 11), with a test, so release-please lists them in
    `CHANGELOG.md`.
13. **Where the adapters' text rules in the renderers live**: pytest's
    private fixture names (`_xunit_setup_module_fixture_<m>` as
    `setup_module`, `aye-render/src/shared.rs:229-240`), the `E   `
    marker, unittest's and doctest's message shapes and the bare subtest
    failure (`render` map §7, items 1-6). They stay in
    `aye-render/src/failure/` for 0.1.0, since stored 0.0.1 logs carry
    nothing else to read them from. Moving them into the log as additive
    fields (`Resource.display_name`, `Failure.repeats_subs`, …; `render`
    map §12, Q2), with the renderers' rules kept as fallbacks for old logs,
    follows the 0.0.1 AGENTS.md's "if a renderer needs more, extend the
    schema". *Recommended:* after 0.1.0, as additive fields, with no
    `SCHEMA_VERSION` bump.
