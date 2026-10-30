//! The pytest adapter on aye's own engine: pytest suites run without
//! pytest, which is never imported (the default since October 2026; `aye
//! --native`, the prototype's switch, is accepted and does nothing).
//!
//! Parsing is decoupled from execution. In the worker, a front-end
//! (`aye_pytest_frontend`) reads the suite's code, through aye's own
//! `pytest` module (`aye_pytest_shim.py`, installed as `pytest` before any
//! test or conftest is imported), into aye's model (`aye_model`); aye's
//! engine (`aye_native_engine`, with `aye_rewrite` rewriting asserts) runs
//! the model and knows no framework; `aye_pytest_native` is the glue, the
//! executor the worker instantiates. It [lists the
//! session](aye_protocol::SessionMode::ListsSession): the worker that refined
//! runs batches too, and every other worker lists the session's files
//! before its first batch.
//!
//! Rust half: [`Pytest`], the pytest adapter with that executor. Its
//! recognizer ([`NativeRecognizer`]) is the static one's for what is a test
//! file and where to look; but:
//! - **Every claimed test file is refined**, with no static case: the
//!   front-end is the single truth for ids (the static fast path comes back
//!   later). With `aye --co --no-refine` ([`Pytest::static_listing`]), the
//!   static recognizer's prediction is listed instead, without importing
//!   anything: exact where it can be, `Partial` where the front-end would
//!   decide. A run has the front-end list every file it runs.
//! - **The context gains `native`** ([`NativeRecognizer::context`]): the
//!   resolved settings the front-end needs, the raw ini values for
//!   `getini`, and pytest's options parsed as pytest parses its own.
//! - **What it can't run fails loudly**: the files pytest's own doctest
//!   plugin would collect (a run without the doctest adapter) get a
//!   `usage-error` naming the fix, rather than going missing.

use aye_adapter::{Adapter, AdapterError, Project, Recognizer};
use aye_events::AdapterName;
use aye_protocol::{Runtime, SessionMode};

use aye_pytest::names::{self, DOCTEST};

use crate::PytestRecognizer;
use crate::embedded::{ENTRY, MODULES};
use crate::recognizer::NativeRecognizer;

/// The adapter's name.
pub const NAME: AdapterName = names::PYTEST;

/// The pytest adapter: pytest's rules, read by aye's front-end, run by
/// aye's engine, with no pytest installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pytest {
    /// Whether the recognizer gives the static prediction rather than
    /// refining every file ([`Self::static_listing`]).
    static_listing: bool,
}

impl Pytest {
    /// The adapter for `aye --co --no-refine`: the recognizer predicts each
    /// file's cases statically, as far as it can, and leaves the rest to
    /// refinement, which is off, so that collection lists the static guess
    /// without importing anything.
    #[must_use]
    pub const fn static_listing() -> Self {
        Self {
            static_listing: true,
        }
    }
}

impl Adapter for Pytest {
    fn name(&self) -> AdapterName {
        NAME
    }

    /// pytest is the default for Python tests: a recognizer is always
    /// returned, configured or not. The doctest adapter runs doctests.
    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        Ok(Some(Box::new(NativeRecognizer::new(
            PytestRecognizer::new(project)?,
            self.static_listing,
        ))))
    }

    /// [`Pytest::prepare`]; without the doctest adapter, the files pytest's
    /// doctest plugin would collect are claimed, and refused
    /// ([`NativeRecognizer::recognize`]).
    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[AdapterName],
    ) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        let recognizer = PytestRecognizer::new(project)?;
        let recognizer = if adapters.contains(&DOCTEST) {
            recognizer
        } else {
            recognizer.with_pytest_doctests()
        };
        Ok(Some(Box::new(NativeRecognizer::new(
            recognizer,
            self.static_listing,
        ))))
    }

    fn runtime(&self) -> Runtime {
        Runtime {
            adapter: NAME,
            modules: MODULES,
            entry: ENTRY,
            session: SessionMode::ListsSession,
        }
    }
}
