//! Diagnostics: what collection, a worker or the runner wants the user to
//! know, and how severe it is.

use serde::{Deserialize, Serialize};

use crate::path::{DiagnosticPath, Span};
use crate::serde_ext::string_enum;

/// How much a diagnostic matters, from the least to the worst, so that
/// `max()` of several is the worst: an error fails the run, a warning and
/// an info only tell. Written by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    /// Its name on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

string_enum! {
    /// What a diagnostic reports: one stable, short, kebab-case code for
    /// each thing collection, an adapter, the runner or the binary tells
    /// (`docs/events.md`, "Diagnostics"). A code this aye doesn't know (a
    /// newer worker's) reads as [`DiagnosticCode::Other`].
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum DiagnosticCode {
        /// Collection: a claimed file doesn't parse.
        SyntaxError = "syntax-error",
        /// Collection: a claimed file, or a directory a walk enters, can't
        /// be read.
        Unreadable = "unreadable",
        /// What the command line names doesn't exist (a path), or matches no
        /// test (an id, dotted name or key); in a batch, a requested file or
        /// id the framework can't find.
        NotFound = "not-found",
        /// Collection: no adapter claims an explicit file (pytest's "found
        /// no collectors").
        Unclaimed = "unclaimed",
        /// One adapter makes two cases with one id, or a framework ran more
        /// than one test with one id.
        DuplicateId = "duplicate-id",
        /// Collection: a test file has no tests.
        NoTests = "no-tests",
        /// Collection: a file needs importing, but refinement is off.
        NotRefined = "not-refined",
        /// Collection: refinement failed; the static guess stays.
        RefineFailed = "refine-failed",
        /// The pytest adapter: a `Test*` class with `__init__`, which pytest
        /// won't collect.
        ClassHasInit = "class-has-init",
        /// The pytest adapter: a test name bound twice in one namespace.
        DuplicateDefinition = "duplicate-definition",
        /// The framework failed to collect a file or to load a conftest.
        CollectionError = "collection-error",
        /// The framework rejected its arguments, or a conftest it loads at
        /// start-up.
        UsageError = "usage-error",
        /// The pytest adapter: an option nothing defines, when pytest would
        /// load a plugin aye doesn't, which may define it.
        UnknownOption = "unknown-option",
        /// The pytest adapter: a plugin pytest would load that aye doesn't.
        PluginNotLoaded = "plugin-not-loaded",
        /// The framework's own internal error (pytest's `INTERNALERROR`).
        InternalError = "internal-error",
        /// The framework skipped a module or a directory while collecting
        /// it.
        Skipped = "skipped",
        /// The pytest adapter: pytest ignores a requested file; the unittest
        /// adapter: a file whose package's `load_tests` gives it no test.
        Ignored = "ignored",
        /// The framework stopped a batch early; as an error, it stops the
        /// run.
        BatchStopped = "batch-stopped",
        /// The pytest adapter: the session failed although none of its tests
        /// did (`session.exitstatus`, pytest-cov's `--cov-fail-under`).
        SessionFailed = "session-failed",
        /// The runner: a worker line that isn't a protocol message.
        ProtocolError = "protocol-error",
        /// The runner: an adapter raised outside any test.
        BatchFailed = "batch-failed",
        /// The runner: no executor for a case's adapter.
        NoExecutor = "no-executor",
        /// The runner: a lane lists a file's tests otherwise than the first
        /// lane, so they run on the first lane only.
        ListingDiffers = "listing-differs",
        /// Reserved: documented by 0.0.1 for a re-queued pytest batch whose
        /// parameters can't be matched with the ones that ran, and emitted
        /// by nothing.
        StandIn = "stand-in",
        /// Reserved: documented by 0.0.1 for a pytest hook that failed to
        /// translate, emitted by nothing, and counted as internal by the
        /// summary.
        AdapterError = "adapter-error",
    }
}

/// Something collection wants the user to know: a syntax error, a test that
/// will never run, a name defined twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub path: DiagnosticPath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severities_order_from_the_least_to_the_worst_and_keep_their_names() {
        let all = [Severity::Info, Severity::Warning, Severity::Error];
        assert!(all.is_sorted());
        assert_eq!(all.into_iter().max(), Some(Severity::Error));
        for severity in all {
            assert_eq!(
                serde_json::to_string(&severity).unwrap(),
                format!("\"{}\"", severity.as_str())
            );
        }
        assert_eq!(all.map(Severity::as_str), ["info", "warning", "error"]);
    }

    /// Each code reads back as itself from its text, and is written as it;
    /// a code no variant spells keeps its text.
    #[test]
    fn codes_are_their_text() {
        let texts: Vec<&str> = DiagnosticCode::ALL
            .iter()
            .map(DiagnosticCode::as_str)
            .collect();
        assert_eq!(texts.len(), 25);
        for (at, text) in texts.iter().enumerate() {
            assert!(!texts[..at].contains(text), "{text} twice");
        }
        for code in DiagnosticCode::ALL {
            let text = code.as_str();
            assert!(
                text.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{text}"
            );
            assert_eq!(DiagnosticCode::from(text), *code);
            assert_eq!(code.to_string(), text);
            let json = serde_json::to_string(code).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(
                serde_json::from_str::<DiagnosticCode>(&json).unwrap(),
                *code
            );
        }
        let other: DiagnosticCode = "Not A Code".parse().unwrap();
        assert_eq!(other, DiagnosticCode::Other("Not A Code".into()));
        assert_eq!(serde_json::to_string(&other).unwrap(), "\"Not A Code\"");
        assert!(!DiagnosticCode::ALL.contains(&other));
    }
}
