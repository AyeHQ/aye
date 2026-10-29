//! Collected cases: what collection found, as the runner plans with it.

use serde::{Deserialize, Serialize};

use crate::id::TestId;
use crate::path::Span;

/// How sure collection is that a case exists exactly as described.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Certainty {
    /// Known from the source alone.
    #[default]
    Exact,
    /// The test exists, but its parameter ids or tags are only known at
    /// runtime; the id is the unparametrized one.
    Partial,
    /// Reported by the framework after importing the file.
    Refined,
}

/// One collected test, as the runner plans with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Case {
    pub id: TestId,
    /// The adapter that collected it (`pytest`).
    pub adapter: String,
    /// Where the test is defined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// User labels, from the framework's markers or decorators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Names a keyword expression matches against (path parts, enclosing
    /// names, the test name, tags), chosen by the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords: Vec<String>,
    /// Capabilities a backend must provide to run this test.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Keys of the lifetimes this test runs inside, outermost first. Tests
    /// sharing a key are kept together when that lifetime is expensive.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub affinity: Vec<String>,
    /// Must not run concurrently with other serial tests.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub serial: bool,
    /// Per-test timeout in seconds, from the framework's metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantine: Option<String>,
    pub certainty: Certainty,
}

impl Case {
    pub fn new(id: impl Into<String>, adapter: impl Into<String>) -> Self {
        Self {
            id: TestId::new(id),
            adapter: adapter.into(),
            span: None,
            tags: Vec::new(),
            keywords: Vec::new(),
            requires: Vec::new(),
            affinity: Vec::new(),
            serial: false,
            timeout: None,
            owner: None,
            quarantine: None,
            certainty: Certainty::Exact,
        }
    }
}
