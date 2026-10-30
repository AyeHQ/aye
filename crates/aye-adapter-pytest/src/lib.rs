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
//! The modules, by what they answer (`predict` is what only `aye --co
//! --no-refine` needs; every other run claims, and refines every test
//! file):
//! - `adapter`: [`Pytest`], the adapter, on aye's engine.
//! - [`embedded`]: the Python it embeds, and the digest of its sources.
//! - `recognizer`: [`PytestRecognizer`], the static recognizer, and the one
//!   the adapter prepares over it, which refines every test file.
//! - `context`: the context, and its `native` section, what the front-end
//!   starts from.
//! - `options`: pytest's built-in options, which the native context parses
//!   its arguments by.
//! - `session`: pytest's session for one project: its plugins, the walk
//!   that finds its session-wide hooks and colliding module names, the
//!   name it imports a test file under, and `--pyargs`.
//! - `claims`: which files pytest collects: `python_files`, the ignore
//!   rules and pytest's doctest plugin.
//! - `conftest`: what support modules do to collection: their summaries,
//!   fixtures, hooks and ini changes, and the questions asked across them.
//! - `predict`: a test module's cases, predicted: pytest's collectors, the
//!   fixture closure, `reorder_items` and the making of cases.
//! - `marks`: marks, decorators, `parametrize` and pytest's ids.
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
//! The decisions about one module are on it; those about the whole crate
//! follow.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
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
//!
//! [`Pytest::prepare`]: aye_adapter::Adapter::prepare
//! [`Recognizer::context`]: aye_adapter::Recognizer::context

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
