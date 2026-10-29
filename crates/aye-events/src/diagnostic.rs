//! Diagnostics: what collection, a worker or the runner wants the user to
//! know, and how severe it is.

use serde::{Deserialize, Serialize};

use crate::path::{DiagnosticPath, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Info,
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
