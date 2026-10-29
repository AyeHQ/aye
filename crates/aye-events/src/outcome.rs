//! Outcomes: of a test, the phase it happened in, and how a run ended and
//! why it stopped early.

use serde::{Deserialize, Serialize};

use crate::serde_ext::string_enum;

/// The fixed outcome set. Adding a variant is a breaking change.
/// Annotations such as *flaky* are separate from it.
///
/// Open: whether a snapshot with no value yet maps onto an existing outcome
/// or onto an annotation (a new outcome after release would break every
/// consumer that matches on outcomes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    /// A check in the test failed.
    Failed,
    /// The test could not run to a verdict: an unexpected exception, or a
    /// failure in setup or teardown.
    Error,
    Skipped,
    /// The test was expected to fail, and did.
    ExpectedFailure,
    /// The test was expected to fail, and passed.
    UnexpectedPass,
}

impl Outcome {
    /// Whether this outcome makes the run fail.
    pub const fn is_bad(self) -> bool {
        matches!(self, Self::Failed | Self::Error)
    }

    /// Severity for combining outcomes: the worse outcome wins.
    pub const fn severity(self) -> u8 {
        match self {
            Self::Passed => 0,
            Self::Skipped => 1,
            Self::ExpectedFailure => 2,
            Self::UnexpectedPass => 3,
            Self::Failed => 4,
            Self::Error => 5,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Skipped => "skipped",
            Self::ExpectedFailure => "expected_failure",
            Self::UnexpectedPass => "unexpected_pass",
        }
    }
}

/// Where in a test's life something happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Setup,
    Test,
    Teardown,
}

/// How a run ended. Values match pytest's wherever pytest defines one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
#[repr(u8)]
pub enum ExitCode {
    /// Every selected test passed (or was skipped or expected to fail).
    Ok = 0,
    /// Some test failed or errored.
    TestsFailed = 1,
    /// Interrupted, including by collection errors.
    Interrupted = 2,
    /// A bug in aye or a worker that died outside any test.
    InternalError = 3,
    /// Bad command line, configuration or expression.
    UsageError = 4,
    /// Nothing was collected or selected.
    NoTests = 5,
}

impl From<ExitCode> for u8 {
    fn from(code: ExitCode) -> Self {
        code as u8
    }
}

impl From<ExitCode> for std::process::ExitCode {
    fn from(code: ExitCode) -> Self {
        Self::from(code as u8)
    }
}

impl TryFrom<u8> for ExitCode {
    type Error = String;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Ok,
            1 => Self::TestsFailed,
            2 => Self::Interrupted,
            3 => Self::InternalError,
            4 => Self::UsageError,
            5 => Self::NoTests,
            other => return Err(format!("unknown exit code {other}")),
        })
    }
}

string_enum! {
    /// Why a run stopped before every selected test ran: `run_finished`'s
    /// `stopped`, and the run store's. The runner gives the first reason
    /// that came up; the binary gives `interrupted` for a collection
    /// stopped before the run. A reason a stored log holds that this aye
    /// doesn't know reads as [`StopReason::Other`].
    ///
    /// The run's interruption doesn't follow from it: after a framework
    /// stop with an internal error (pytest's status 3) the run is an
    /// internal error, not an interrupted one.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum StopReason {
        /// `maxfail` bad outcomes were reached.
        Maxfail = "maxfail",
        /// The run was stopped from outside (Ctrl-C).
        Interrupted = "interrupted",
        /// The framework stopped a batch, ending its session there: the
        /// adapter said so with a `batch-stopped` error diagnostic
        /// (`pytest.exit` whatever its `returncode`, a `KeyboardInterrupt`,
        /// `shouldstop`, a `shouldfail` that isn't aye's `maxfail`, an
        /// internal error that left tests), or the batch ended with
        /// pytest's status 2. The run counts as interrupted, except after
        /// an internal error (status 3), which makes it an internal error
        /// instead.
        Framework = "framework",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each reason is written as `run_finished.stopped` spelled it, and
    /// one this aye doesn't know reads back as it was.
    #[test]
    fn stop_reasons_are_spelled_as_run_finished_spells_them() {
        let names: Vec<&str> = StopReason::ALL.iter().map(StopReason::as_str).collect();
        assert_eq!(names, ["maxfail", "interrupted", "framework"]);
        assert_eq!(StopReason::Framework.to_string(), "framework");
        for text in ["maxfail", "interrupted", "framework", "timeout"] {
            let read: StopReason = serde_json::from_str(&format!("\"{text}\"")).unwrap();
            assert_eq!(read.as_str(), text);
            assert_eq!(serde_json::to_string(&read).unwrap(), format!("\"{text}\""));
        }
        assert_eq!(
            StopReason::from("timeout"),
            StopReason::Other("timeout".into())
        );
    }
}
