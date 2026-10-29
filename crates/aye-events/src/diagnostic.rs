//! Diagnostics: what collection, a worker or the runner wants the user to
//! know, and how severe it is.

use serde::{Deserialize, Serialize};

use crate::path::{DiagnosticPath, Span};

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

/// Something collection wants the user to know: a syntax error, a test that
/// will never run, a name defined twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable, short, kebab-case: `syntax-error`, `duplicate-definition`.
    pub code: String,
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
}
