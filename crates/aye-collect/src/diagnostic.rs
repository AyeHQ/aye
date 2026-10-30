//! Collection's own diagnostics ([`CollectDiagnostic`]), each with its
//! code, its severity and its text. The recognizers' and the workers' pass
//! through as they come.

use std::path::PathBuf;

use aye_adapter::RefineReason;
use aye_events::{
    AdapterName, Diagnostic, DiagnosticCode, DiagnosticPath, ProjectPath, Severity, Span, TestId,
};

/// A diagnostic collection gives itself, as it becomes a [`Diagnostic`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CollectDiagnostic {
    /// An explicit path that doesn't exist (`given` as it was given), as
    /// pytest's `file or directory not found:`.
    NotFound {
        path: DiagnosticPath,
        given: PathBuf,
    },
    /// An explicit file no recognizer claims, as pytest's "found no
    /// collectors".
    Unclaimed { path: DiagnosticPath },
    /// A file a recognizer claimed, or a directory the walk would enter,
    /// that can't be read, and why.
    Unreadable { path: DiagnosticPath, error: String },
    /// A Python file that doesn't parse: where (the byte range, clamped to
    /// the text, its 1-based line and column) and the parser's error.
    SyntaxError {
        path: ProjectPath,
        start: u32,
        end: u32,
        line: u32,
        column: usize,
        error: String,
    },
    /// A test file in which none of the `adapters` claiming it, in priority
    /// order, finds a test.
    NoTests {
        path: ProjectPath,
        adapters: Vec<AdapterName>,
    },
    /// An id `adapter` gives a second time, where its case is.
    DuplicateId {
        adapter: AdapterName,
        id: TestId,
        span: Option<Span>,
    },
    /// A file `adapter` would refine, and why, when refinement is off.
    NotRefined {
        adapter: AdapterName,
        path: ProjectPath,
        reason: RefineReason,
    },
    /// A file `adapter` couldn't refine, and why.
    RefineFailed {
        adapter: AdapterName,
        path: ProjectPath,
        error: String,
    },
}

impl CollectDiagnostic {
    pub(crate) fn code(&self) -> DiagnosticCode {
        match self {
            Self::NotFound { .. } => DiagnosticCode::NotFound,
            Self::Unclaimed { .. } => DiagnosticCode::Unclaimed,
            Self::Unreadable { .. } => DiagnosticCode::Unreadable,
            Self::SyntaxError { .. } => DiagnosticCode::SyntaxError,
            Self::NoTests { .. } => DiagnosticCode::NoTests,
            Self::DuplicateId { .. } => DiagnosticCode::DuplicateId,
            Self::NotRefined { .. } => DiagnosticCode::NotRefined,
            Self::RefineFailed { .. } => DiagnosticCode::RefineFailed,
        }
    }

    /// Before strict mode, which makes every warning an error.
    pub(crate) const fn severity(&self) -> Severity {
        match self {
            Self::NotFound { .. }
            | Self::Unclaimed { .. }
            | Self::Unreadable { .. }
            | Self::SyntaxError { .. } => Severity::Error,
            // The framework selects by id, so it runs them all (pytest
            // does); and without refinement the static guess may do.
            Self::DuplicateId { .. } | Self::RefineFailed { .. } => Severity::Warning,
            Self::NoTests { .. } | Self::NotRefined { .. } => Severity::Info,
        }
    }
}

impl From<CollectDiagnostic> for Diagnostic {
    fn from(diagnostic: CollectDiagnostic) -> Self {
        let severity = diagnostic.severity();
        let code = diagnostic.code();
        let (path, span, message) = match diagnostic {
            CollectDiagnostic::NotFound { path, given } => (
                path,
                None,
                format!("file or directory not found: {}", given.display()),
            ),
            CollectDiagnostic::Unclaimed { path } => {
                (path, None, "no adapter collects this file".to_string())
            }
            CollectDiagnostic::Unreadable { path, error } => {
                (path, None, format!("cannot read: {error}"))
            }
            CollectDiagnostic::SyntaxError {
                path,
                start,
                end,
                line,
                column,
                error,
            } => (
                path.clone().into(),
                Some(Span {
                    path,
                    start,
                    end,
                    line: Some(line),
                }),
                format!("syntax error at {line}:{column}: {error}"),
            ),
            CollectDiagnostic::NoTests { path, adapters } => {
                let names: Vec<&str> = adapters.iter().map(AdapterName::as_str).collect();
                (
                    path.into(),
                    None,
                    format!("claimed for tests by {}, but has none", names.join(", ")),
                )
            }
            CollectDiagnostic::DuplicateId { adapter, id, span } => (
                DiagnosticPath::as_written(id.path().as_str()),
                span,
                format!(
                    "{adapter} collects `{id}` more than once; aye lists it once, \
                     but running it runs every test with that id"
                ),
            ),
            CollectDiagnostic::NotRefined {
                adapter,
                path,
                reason,
            } => (
                path.into(),
                None,
                format!(
                    "{adapter} must import this file to know its tests ({reason}), \
                     but refinement is off; keeping the static guess"
                ),
            ),
            CollectDiagnostic::RefineFailed {
                adapter,
                path,
                error,
            } => (
                path.into(),
                None,
                format!(
                    "{adapter} could not import this file to know its tests ({error}); \
                     keeping the static guess"
                ),
            ),
        };
        Self {
            severity,
            code,
            path,
            span,
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use aye_events::{AdapterName, Diagnostic, DiagnosticPath, ProjectPath, Span, TestId};

    use super::CollectDiagnostic;

    /// Each of collection's diagnostics, as the stream gives it: code,
    /// severity, path, span and text.
    #[test]
    fn each_diagnostic_says_what_collection_always_said() {
        let path = || ProjectPath::as_written("tests/test_a.py");
        let span = Span {
            path: path(),
            start: 4,
            end: 9,
            line: Some(2),
        };
        let cases = [
            (
                CollectDiagnostic::NotFound {
                    path: DiagnosticPath::as_written("nope.py"),
                    given: PathBuf::from("../nope.py"),
                },
                r#"{"severity":"error","code":"not-found","path":"nope.py","message":"file or directory not found: ../nope.py"}"#,
            ),
            (
                CollectDiagnostic::Unclaimed {
                    path: DiagnosticPath::as_written("README.md"),
                },
                r#"{"severity":"error","code":"unclaimed","path":"README.md","message":"no adapter collects this file"}"#,
            ),
            (
                CollectDiagnostic::Unreadable {
                    path: DiagnosticPath::as_written("locked"),
                    error: "Permission denied (os error 13)".to_string(),
                },
                r#"{"severity":"error","code":"unreadable","path":"locked","message":"cannot read: Permission denied (os error 13)"}"#,
            ),
            (
                CollectDiagnostic::SyntaxError {
                    path: path(),
                    start: 4,
                    end: 9,
                    line: 2,
                    column: 5,
                    error: "Expected an expression".to_string(),
                },
                r#"{"severity":"error","code":"syntax-error","path":"tests/test_a.py","span":{"path":"tests/test_a.py","start":4,"end":9,"line":2},"message":"syntax error at 2:5: Expected an expression"}"#,
            ),
            (
                CollectDiagnostic::NoTests {
                    path: path(),
                    adapters: vec![AdapterName::from("unittest"), AdapterName::from("pytest")],
                },
                r#"{"severity":"info","code":"no-tests","path":"tests/test_a.py","message":"claimed for tests by unittest, pytest, but has none"}"#,
            ),
            (
                CollectDiagnostic::DuplicateId {
                    adapter: AdapterName::from("pytest"),
                    id: TestId::new("tests/test_a.py::test_x"),
                    span: Some(span),
                },
                r#"{"severity":"warning","code":"duplicate-id","path":"tests/test_a.py","span":{"path":"tests/test_a.py","start":4,"end":9,"line":2},"message":"pytest collects `tests/test_a.py::test_x` more than once; aye lists it once, but running it runs every test with that id"}"#,
            ),
            (
                CollectDiagnostic::NotRefined {
                    adapter: AdapterName::from("pytest"),
                    path: path(),
                    reason: "a hook makes its items".into(),
                },
                r#"{"severity":"info","code":"not-refined","path":"tests/test_a.py","message":"pytest must import this file to know its tests (a hook makes its items), but refinement is off; keeping the static guess"}"#,
            ),
            (
                CollectDiagnostic::RefineFailed {
                    adapter: AdapterName::from("pytest"),
                    path: path(),
                    error: "the worker died".to_string(),
                },
                r#"{"severity":"warning","code":"refine-failed","path":"tests/test_a.py","message":"pytest could not import this file to know its tests (the worker died); keeping the static guess"}"#,
            ),
        ];
        for (diagnostic, expected) in cases {
            let written = serde_json::to_string(&Diagnostic::from(diagnostic)).unwrap();
            assert_eq!(written, expected);
        }
    }
}
