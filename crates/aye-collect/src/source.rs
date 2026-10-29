//! Reading and parsing one file.

use std::path::Path;

use aye_engine::{LineIndex, SourceFile, TextSize, ast};
use aye_events::{Diagnostic, ProjectPath, Severity, Span};
use ruff_python_parser::ParseError;

use crate::codes;

/// A file as read and, when it is Python, parsed.
pub(crate) struct Source {
    pub text: String,
    /// The bytes as read, when `text` is a lossy decoding of them.
    raw: Option<Vec<u8>>,
    pub lines: LineIndex,
    /// `None` when the file doesn't parse.
    pub module: Option<ast::ModModule>,
}

impl Source {
    pub(crate) fn bytes(&self) -> &[u8] {
        self.raw.as_deref().unwrap_or(self.text.as_bytes())
    }

    /// The file is not UTF-8, so `text` only approximates it.
    pub(crate) fn is_lossy(&self) -> bool {
        self.raw.is_some()
    }

    /// The file as recognizers see it, when it parsed.
    pub(crate) fn file<'a>(
        &'a self,
        rel: &'a ProjectPath,
        abs: &'a Path,
    ) -> Option<SourceFile<'a>> {
        Some(SourceFile {
            rel: rel.as_path(),
            abs,
            source: &self.text,
            module: self.module.as_ref()?,
            lines: &self.lines,
        })
    }
}

/// Read and parse a file. Only `.py` files are parsed; any other file a
/// recognizer claims comes with its text and an empty module. `None` when the
/// file can't be read; the diagnostic says why, or why it doesn't parse.
pub(crate) fn load(abs: &Path, rel: &ProjectPath) -> (Option<Source>, Option<Diagnostic>) {
    let bytes = match std::fs::read(abs) {
        Ok(bytes) => bytes,
        Err(err) => {
            let diagnostic = Diagnostic {
                severity: Severity::Error,
                code: codes::UNREADABLE.to_string(),
                path: rel.clone().into(),
                span: None,
                message: format!("cannot read: {err}"),
            };
            return (None, Some(diagnostic));
        }
    };
    let (text, raw) = match String::from_utf8(bytes) {
        Ok(text) => (text, None),
        Err(err) => {
            let bytes = err.into_bytes();
            (String::from_utf8_lossy(&bytes).into_owned(), Some(bytes))
        }
    };
    let lines = LineIndex::from_source_text(&text);
    let (module, diagnostic) = if abs.extension().is_some_and(|ext| ext == "py") {
        match ruff_python_parser::parse_module(&text) {
            Ok(parsed) => (Some(parsed.into_syntax()), None),
            // Lossy text may break what the real encoding wouldn't; the file
            // is refined, so the framework reports any real error.
            Err(_) if raw.is_some() => (Some(empty_module()), None),
            Err(err) => (None, Some(syntax_error(rel, &text, &lines, &err))),
        }
    } else {
        (Some(empty_module()), None)
    };
    let source = Source {
        text,
        raw,
        lines,
        module,
    };
    (Some(source), diagnostic)
}

fn empty_module() -> ast::ModModule {
    ruff_python_parser::parse_module("")
        .expect("an empty module parses")
        .into_syntax()
}

fn syntax_error(rel: &ProjectPath, text: &str, lines: &LineIndex, err: &ParseError) -> Diagnostic {
    let len = TextSize::try_from(text.len()).unwrap_or(TextSize::new(u32::MAX));
    let start = err.location.start().min(len);
    let end = err.location.end().min(len);
    let at = lines.line_column(start, text);
    let line = u32::try_from(at.line.get()).unwrap_or(u32::MAX);
    Diagnostic {
        severity: Severity::Error,
        code: codes::SYNTAX_ERROR.to_string(),
        path: rel.clone().into(),
        span: Some(Span {
            path: rel.clone(),
            start: start.to_u32(),
            end: end.to_u32(),
            line: Some(line),
        }),
        message: format!("syntax error at {line}:{}: {}", at.column, err.error),
    }
}
