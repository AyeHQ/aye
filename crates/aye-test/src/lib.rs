//! What aye's tests share: the interpreter a test runs ([`test_python`], by
//! what it [`Needs`]), aye as its own tests run it ([`TestContext`]: from a
//! directory, in the hermetic environment, with the interpreter a run is
//! given), what snapshots of its output leave out ([`filters`]), its
//! output read back (exit codes, stdout, JSON lines, and those lines as the
//! records a run's log would hold: [`as_records`]), the fixtures (a
//! project written for a test, a copy of one, the examples, a venv without
//! pytest), and the `fnmatch` answers aye 0.0.1's copies gave, pinned
//! ([`assert_fnmatch_pins`]).
//!
//! Decided (implementation, October 2026):
//! - **One interpreter for every test**: `AYE_TEST_PYTHON` when set, else
//!   the workspace's `.venv` (from `uv sync`), probed once per test binary
//!   for what the test needs: Python 3.10 or later, aye's oldest, and
//!   pytest for the tests that run the referees or show that aye's own
//!   `pytest` module wins over it. Without one, the tests that need it say
//!   so on stderr, once, and pass without running.
//! - **Every run of aye a test makes is hermetic**: the variables aye reads
//!   that a caller's shell may hold are taken out (the interpreter's,
//!   GitHub Actions', pytest's, Python's import path and optimisation,
//!   forced colour), and pytest's plugin autoloading is off, so that what a
//!   test checks doesn't depend on who runs it. The comparison harness
//!   alone runs aye in the referee's environment instead, as
//!   `record.py` ran the referee.
//!
//! Owns: the lookup of the test interpreter, the hermetic environment, the
//! snapshots' filters, reading aye's output back, the fixtures above, and
//! the `fnmatch` pins.
//! Does not own: the adapters' own harnesses, which build parsed files and
//! drive recognizers (they stay in their crates), nor reading the run store
//! (aye-history, which the binary's tests read themselves).
//!
//! Depends on: aye-fs (paths, the state directory), aye-static (the names
//! of the variables aye reads), `serde_json` and tempfile. Not published:
//! each crate names it as a dev-dependency by path, which `cargo publish`
//! strips.

mod context;
mod filters;
mod fixtures;
mod fnmatch;
mod output;
mod python;

pub use context::TestContext;
pub use filters::filters;
pub use fixtures::{
    copy_project, examples_dir, project, runs_dir, venv_without_pytest, workspace_root,
};
pub use fnmatch::{FNMATCH_NAMES, FNMATCH_PINS, FnmatchAnswers, assert_fnmatch_pins};
pub use output::{as_records, code, events, json_lines, stderr, stdout};
pub use python::{Needs, test_python};
