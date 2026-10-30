//! The diagnostics that came already ([`SeenDiagnostics`]), so that one
//! every batch repeats (a framework rejecting its arguments) is shown,
//! annotated and counted once.

use std::collections::HashSet;

use aye_events::{Diagnostic, DiagnosticCode, Severity};

/// The diagnostics seen so far, by severity, code, path and message: two
/// that agree on all four are one.
#[derive(Debug, Default)]
pub struct SeenDiagnostics {
    seen: HashSet<(Severity, DiagnosticCode, String, String)>,
}

impl SeenDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `diagnostic` is the first with its severity, code, path and
    /// message; it is seen from now on.
    pub fn first(&mut self, diagnostic: &Diagnostic) -> bool {
        self.seen.insert((
            diagnostic.severity,
            diagnostic.code.clone(),
            diagnostic.path.to_string(),
            diagnostic.message.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use aye_events::DiagnosticPath;

    use super::*;

    fn diagnostic(severity: Severity, code: &str, path: &str, message: &str) -> Diagnostic {
        Diagnostic {
            severity,
            code: code.into(),
            path: DiagnosticPath::as_written(path),
            span: None,
            message: message.into(),
        }
    }

    #[test]
    fn a_diagnostic_is_first_once_by_severity_code_path_and_message() {
        let mut seen = SeenDiagnostics::new();
        let usage = diagnostic(Severity::Error, "usage-error", "", "bad -k");
        assert!(seen.first(&usage));
        assert!(!seen.first(&usage.clone()));
        for other in [
            diagnostic(Severity::Warning, "usage-error", "", "bad -k"),
            diagnostic(Severity::Error, "syntax-error", "", "bad -k"),
            diagnostic(Severity::Error, "usage-error", "t.py", "bad -k"),
            diagnostic(Severity::Error, "usage-error", "", "bad -m"),
        ] {
            assert!(seen.first(&other), "{other:?}");
            assert!(!seen.first(&other), "{other:?}");
        }
    }
}
