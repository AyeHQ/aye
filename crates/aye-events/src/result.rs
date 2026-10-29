//! Test results: a test's outcome with its failures, output, resources and
//! history, and the totals they fold into.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::failure::Failure;
use crate::id::TestId;
use crate::lifetime::{LifetimeId, Scope};
use crate::outcome::{Outcome, OutcomeTrail};
use crate::run::RunNumber;
use crate::serde_ext::millis;

/// How one test ended. A test has one outcome: one that passes its body and
/// then fails in teardown is `error`, and every failure is kept, each with
/// its [`Phase`](crate::Phase).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    pub id: TestId,
    pub outcome: Outcome,
    #[serde(rename = "duration_ms", with = "millis", default)]
    pub duration: std::time::Duration,
    /// Every failure, in the order they happened (a failed body, then a
    /// failed teardown).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<Failure>,
    /// The failed lifetime that kept this test from running, instead of a
    /// copy of its failure (see [`Event::LifetimeFinished`](crate::Event::LifetimeFinished)).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<LifetimeId>,
    /// Why it was skipped or expected to fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Captured::is_empty")]
    pub output: Captured,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reproduce: Option<Reproduce>,
    /// Name and value pairs the test recorded about itself, in order
    /// (pytest's `record_property`, its item's `user_properties`). They
    /// travel in the stream for the JUnit report, which writes them on the
    /// test case, and any later consumer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<(String, String)>,
    /// How long the body alone took (the `test` phase, pytest's `call`),
    /// when the adapter tells it apart from setup and teardown, for reports
    /// that count the body alone (pytest's `junit_duration_report = call`);
    /// `duration` stays setup, body and teardown.
    #[serde(
        rename = "body_duration_ms",
        with = "millis::optional",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub body_duration: Option<std::time::Duration>,
    /// The resources (pytest's fixtures) set up for a test with a bad
    /// outcome, in setup order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<Resource>,
    /// How the test ended in earlier runs, from history; set by the runner
    /// on a bad outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Previous>,
}

/// A resource a test used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub name: String,
    /// How long its value lives.
    pub scope: Scope,
}

/// A test's outcomes in the runs before this one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Previous {
    /// One letter per run, oldest first, at most 20: `p` passed, `f`
    /// failed, `e` error, `s` skipped, `x` expected failure, `u`
    /// unexpected pass.
    pub outcomes: OutcomeTrail,
    /// The number of the run in which the test's current streak of bad
    /// outcomes began, when it began before this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<RunNumber>,
}

impl TestResult {
    pub fn new(id: TestId, outcome: Outcome) -> Self {
        Self {
            id,
            outcome,
            duration: std::time::Duration::ZERO,
            failures: Vec::new(),
            cause: None,
            reason: None,
            output: Captured::default(),
            reproduce: None,
            properties: Vec::new(),
            body_duration: None,
            resources: Vec::new(),
            history: None,
        }
    }

    /// The failure renderers lead with: the first one.
    pub fn failure(&self) -> Option<&Failure> {
        self.failures.first()
    }
}

/// Output captured while a test or lifetime ran. Each stream is bounded.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Captured {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stderr: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log: String,
    /// Some stream was cut to fit its bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl Captured {
    pub fn is_empty(&self) -> bool {
        self.stdout.is_empty() && self.stderr.is_empty() && self.log.is_empty()
    }
}

/// How to rerun exactly this test with exactly these inputs. Structured from
/// day one: an argument vector plus an open map of inputs (seed, frozen
/// time, environment).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Reproduce {
    pub command: Vec<String>,
    /// Seed, frozen time, environment, and whatever later features add.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
}

/// Totals for a run; a fold over its `test_finished` events alone: counts
/// are test results.
///
/// A module the framework skipped while collecting (pytest's
/// `allow_module_level` skip, `importorskip`) has no test to finish: it is a
/// `skipped` info [`Diagnostic`](crate::Diagnostic) from collection, and the
/// renderers count it as one skipped item, as pytest's summary and JUnit
/// report do. No field was needed for it. Nor is a failed lifetime that kept
/// no test from running among them (a failed `tearDownClass`,
/// `tearDownModule` or cleanup; a failed setup whose tests the run stopped
/// before under `maxfail`), though the run fails on it: its
/// `lifetime_finished` carries the failure, and the renderers count it from
/// there, as a setup or teardown error (pytest's summary counts it as an
/// error).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default)]
    pub passed: u32,
    #[serde(default)]
    pub failed: u32,
    #[serde(default)]
    pub error: u32,
    #[serde(default)]
    pub skipped: u32,
    #[serde(default)]
    pub expected_failure: u32,
    #[serde(default)]
    pub unexpected_pass: u32,
    #[serde(default)]
    pub deselected: u32,
}

impl Counts {
    pub fn record(&mut self, outcome: Outcome) {
        let slot = match outcome {
            Outcome::Passed => &mut self.passed,
            Outcome::Failed => &mut self.failed,
            Outcome::Error => &mut self.error,
            Outcome::Skipped => &mut self.skipped,
            Outcome::ExpectedFailure => &mut self.expected_failure,
            Outcome::UnexpectedPass => &mut self.unexpected_pass,
        };
        *slot += 1;
    }

    pub const fn total(&self) -> u32 {
        self.passed
            + self.failed
            + self.error
            + self.skipped
            + self.expected_failure
            + self.unexpected_pass
    }

    pub const fn bad(&self) -> u32 {
        self.failed + self.error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fold_outcomes() {
        let mut counts = Counts::default();
        for outcome in [
            Outcome::Passed,
            Outcome::Failed,
            Outcome::Error,
            Outcome::Skipped,
        ] {
            counts.record(outcome);
        }
        assert_eq!(counts.total(), 4);
        assert_eq!(counts.bad(), 2);
        assert!(Outcome::Error.severity() > Outcome::Failed.severity());
    }
}
