//! Vocabulary (October 2026), binding for code, docs, events, the command
//! line and reports:
//! - **Runner**: aye, the Rust binary. It decides which tests run, where,
//!   when, in what order and under what limits, then collects and reports
//!   what happened. It knows nothing about how a framework defines or
//!   executes a test. It replaces every framework's own runner (pytest's main
//!   loop, unittest's `TextTestRunner`, Django's `DiscoverRunner`).
//! - **Engine**: aye's engine, written in Python, running inside workers and
//!   shipped with aye from the start. It is the model of what a test is and
//!   how it executes: tests, lifetimes, the order of setup and teardown, and
//!   the fixed outcome set. Frameworks have engines of their own (pytest's
//!   collection and fixtures, unittest's `TestCase.run`); "engine" alone
//!   always means aye's.
//! - **Adapter**: maps one existing framework onto aye's engine. It recognises
//!   the framework's tests, translates its setup into lifetimes and its results
//!   into outcomes. aye ships the pytest, unittest and doctest adapters; Django
//!   is a set of services used by the unittest and pytest adapters.
//! - **Framework**: what people write tests against, an authoring API
//!   (fixtures, `TestCase`, assertions) plus its own engine and usually its own
//!   runner. pytest and unittest are frameworks. aye does not ship one yet;
//!   when it does, it is written directly against aye's engine, with no
//!   adapter.
//! - **Library**: aye's importable features (patching, fakes, decorators,
//!   snapshots, test data), usable in the tests of any framework. It is
//!   neither a runner nor an engine.
//! - **Worker**: a Python process, started and managed by the runner, in which
//!   aye's engine executes batches.
//!
//! The line between runner and engine is the batch: the runner owns everything
//! up to handing a batch to the engine and everything after its events come
//! back; the engine owns everything in between. Every adapter is a front-end
//! that reads its framework's tests into aye's model and never runs them;
//! aye's engine runs every test. unittest's and doctest's class and module
//! setup become lifetimes that aye opens and closes at times the runner
//! decides; pytest's fixtures, with their five scopes, are the engine's
//! resources, read through aye's own `pytest` module, so pytest itself is
//! never imported.
//!
//! Until the crates are renamed, `aye-engine-pytest`, `aye-engine-unittest`
//! and `aye-engine-doctest` are the adapters, and older text in the crate docs
//! that says "engine" for a per-framework component means "adapter".
//!
//! aye's engine, in Python, embedded in the binary and installed in each
//! worker's `sys.modules` from memory: the objects every executor builds on
//! ([`ENGINE_MODULE`], `python/aye_engine.py`: `Executor`, `EngineExecutor`,
//! `Sink`, `Result`, `Failure`, `Lifetimes`), and the test model, assertion
//! rewriting and the native engine the pytest front-end runs on
//! ([`MODEL_MODULE`], [`REWRITE_MODULE`], [`NATIVE_ENGINE_MODULE`]). Their
//! Python tests are in `python/tests`; `tests/records.rs` checks that what
//! the engine's records send reads as the Rust types.
//!
//! Owns: aye's engine, and the words above. Does not own: the worker
//! protocol (`aye-protocol`), what an adapter implements (`aye-adapter`),
//! or any framework's rules (the `aye-engine-*` crates).
//!
//! Depends on: aye-protocol. Must not depend on Ruff, the runner or any
//! adapter.
//!
//! Native features (October 2026): snapshots, test data and fakes are runner
//! services plus objects in `aye.testing`, reached the same way from every
//! engine (pytest, unittest, doctest, Django tests). They are not features of
//! the future native engine. The services list grows accordingly: capture,
//! coverage, sandbox, timeouts, current-test context, cleanup, resources,
//! patching, clock, randomness, network, filesystem, snapshots and data
//! builders.
//!
//! Open: whether engines reach services by name with a documented fallback
//! when one is absent (new services stay additive), or through a fixed struct
//! (simpler to type-check, but every new service changes the contract); the
//! aye words for a fake, a snapshot and a data builder.

//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The Python half is source, not a compiled library.** `aye_engine.py`
//!   and every adapter's executor modules are embedded in the binary and
//!   installed into the worker's `sys.modules` from memory, so they work
//!   however aye was installed and never skew from the binary.
//! - **Skips are told by every framework's spelling** (`aye_engine.is_skip`):
//!   aye's `Skip`, unittest's `SkipTest` and pytest's `Skipped` (which says
//!   it lives in `builtins`, so it is also told by its name with its base
//!   `OutcomeException` or pytest's attributes, and by `isinstance` once
//!   pytest is loaded), subclasses included.
//! - **Tests may break what the engine uses; the engine doesn't break.** The
//!   engine binds the standard-library functions it calls (`perf_counter`,
//!   `os.path`'s, `traceback`'s, `math.isfinite`, `hashlib.sha1`) when it
//!   is imported and computes paths with them, and building a failure never
//!   raises (what can't be read is left out), so a test that patches them,
//!   even for the tests after it, changes no result, path or duration.
//!   Adapters do the same for what they use: `perf_counter`, `os_path`
//!   (`os.path` as it was then, its functions looking each other up in a
//!   copy, so `abspath` and `realpath` survive a patched `join`), and
//!   `CaptureIO`; and they load a batch with `os.path`'s own functions back
//!   in place (`unpatched_os_path`), as frameworks import their tests
//!   before running any. The worker does it for the protocol (`json.dumps`,
//!   `os.getpid`).
//! - **Capture without a service** (`captured`, `CaptureIO`): `sys.stdout`
//!   and `sys.stderr` become streams as pytest's default capture
//!   (`--capture=fd`) makes them, text in UTF-8 over bytes (`.buffer`,
//!   `.encoding`) with `errors="replace"` (a lone surrogate is written as
//!   `?` rather than failing the test), with a real `fileno()` (a
//!   temporary file made when first asked for, whose writes are captured
//!   too, after the stream's), still readable when a test closed them, so
//!   a test closing one loses nothing and breaks no later test.
//! - **With `maxfail`, a failed lifetime is reported before the tests it
//!   blocks** (`Lifetimes.report_early`): the runner drops what follows the
//!   bad outcome that reaches the limit, and the tests' `cause` must name a
//!   failure in the log. Without it, the lifetime finishes after them, so
//!   renderers can list them under it.
//! - **Signatures without a location**: a `Failure`'s `site` (not sent)
//!   stands for the place in its signature when it has no location (a
//!   doctest's test and example), so failures in different places that
//!   only read alike aren't grouped.
//! - **Reruns:** `EngineExecutor` sets `reproduce` on every bad outcome,
//!   `aye <rerun_args> <id> -- <args after -->` (`aye_engine.reproduce`;
//!   an executor may override `rerun`), as the pytest adapter does. The
//!   binary adds `rerun_args` to every adapter's context of a batch: aye's
//!   own options that shaped the run (`-c FILE`, as given), which a rerun
//!   needs to run the same tests the same way (`aye_engine.rerun_args`).
//! - **A `sub_outcome` may carry a `reason`** (why a subtest was skipped).
//! - **What a test records for reports** travels in the stream: its
//!   `properties` (`Result.properties`, pytest's `record_property`) and the
//!   run's (`Sink.run_property`, pytest's `record_testsuite_property`), and
//!   its body's duration apart (`Result.body_duration`).
//! - **A failure's `detail`** is Python's text of the exception without the
//!   frames before the user's code (`Paths.is_internal`: the worker's, the
//!   engine's, the framework's), as unittest leaves out its own
//!   (`aye_engine.format_exception(exc, paths)`).

use aye_protocol::EmbeddedModule;

/// aye's engine: the Python base classes every executor builds on.
pub const ENGINE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_engine",
    source: include_str!("../python/aye_engine.py"),
};

/// aye's test model (prototype): what a front-end produces when it reads a
/// framework's tests, and what the native engine runs. Knows no framework.
pub const MODEL_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_model",
    source: include_str!("../python/aye_model.py"),
};

/// Assertion rewriting (prototype): a service of the native engine, for
/// the modules a front-end asks it to rewrite.
pub const REWRITE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_rewrite",
    source: include_str!("../python/aye_rewrite.py"),
};

/// The native engine (prototype): runs [`MODEL_MODULE`]'s cases, resources
/// and lifetimes; imports no framework.
pub const NATIVE_ENGINE_MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_native_engine",
    source: include_str!("../python/aye_native_engine.py"),
};

#[cfg(test)]
mod tests;
