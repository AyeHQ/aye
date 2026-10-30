//! The pytest adapter's recognizer ([`NativeRecognizer`]): the static
//! recognizer's claims, every test file refined (or, for a static listing,
//! predicted), and the context with its `native` section.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aye_adapter::{
    Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested, Setting, SourceFile,
};
use aye_events::{Diagnostic, DiagnosticCode, Severity};
use aye_protocol::AdapterContext;

use crate::PytestRecognizer;
use crate::context::{PytestContext, native_context};

/// Why every test file is refined.
const REFINE_REASON: &str = "aye's pytest front-end reads every test file";

/// The pytest recognizer, with every test file refined (or, for a static
/// listing, predicted) and the `native` context.
#[derive(Debug)]
pub(crate) struct NativeRecognizer {
    inner: PytestRecognizer,
    /// Whether [`Recognizer::recognize`] gives the static recognizer's
    /// prediction ([`crate::Pytest::static_listing`]).
    static_listing: bool,
    /// The context, with its `native` section, built once.
    context: OnceLock<PytestContext>,
}

impl NativeRecognizer {
    pub(crate) fn new(inner: PytestRecognizer, static_listing: bool) -> Self {
        Self {
            inner,
            static_listing,
            context: OnceLock::new(),
        }
    }
}

impl Recognizer for NativeRecognizer {
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        self.inner.roots(project)
    }

    fn descend(&self, dir: &Path) -> bool {
        self.inner.descend(dir)
    }

    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        self.inner.claim(rel, requested)
    }

    /// No static case: the file is refined, the front-end decides; for a
    /// static listing, the static recognizer's prediction. A file pytest's
    /// own doctest plugin would collect (only in a run without the doctest
    /// adapter) is a `usage-error`: aye's engine runs doctests through the
    /// doctest adapter only.
    fn recognize(
        &self,
        file: &SourceFile<'_>,
        requested: Requested,
        support: &[SourceFile<'_>],
    ) -> Recognition {
        if self.inner.doctests.is_some()
            && self
                .inner
                .doctest_file(file.rel.as_str(), requested.is_named())
                .is_some()
        {
            return Recognition {
                diagnostics: vec![Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::UsageError,
                    path: file.rel.to_project_path().into(),
                    span: None,
                    message: "pytest's doctest plugin would collect doctests here, which \
                              aye runs only with its doctest adapter: add \"doctest\" to \
                              [tool.aye] adapters"
                        .into(),
                }],
                ..Recognition::default()
            };
        }
        if self.static_listing {
            return self.inner.recognize(file, requested, support);
        }
        Recognition {
            refine: Some(REFINE_REASON.into()),
            ..Recognition::default()
        }
    }

    fn settings(&self) -> Vec<Setting> {
        self.inner.settings()
    }

    /// The pytest adapter's context (see [`PytestRecognizer`]'s), plus
    /// [`NativeContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.context.get_or_init(|| PytestContext {
            native: Some(native_context(self.inner.config())),
            ..self.inner.pytest_context().clone()
        }))
    }

    fn defaults(&self) -> FrameworkDefaults {
        self.inner.defaults()
    }
}

#[cfg(test)]
mod tests {
    use aye_adapter::{Adapter, RefineReason};
    use aye_pytest::names::DOCTEST;

    use super::*;
    use crate::{NAME, Pytest};

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn recognizer(root: &Path, framework_args: &[String]) -> NativeRecognizer {
        let project = Project {
            framework_args,
            ..Project::new(root, root)
        };
        NativeRecognizer::new(PytestRecognizer::with_env(&project, None).unwrap(), false)
    }

    #[test]
    fn every_test_file_is_refined_without_a_static_case() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = "def test_a():\n    pass\n";
        write(root, "test_a.py", source);
        let recognizer = recognizer(root, &[]);
        assert_eq!(
            recognizer.claim("test_a.py", Requested::Found),
            Claim::Tests
        );
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let lines = aye_adapter::LineIndex::from_source_text(source);
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("test_a.py"),
            abs: &root.join("test_a.py"),
            source,
            module: parsed.syntax(),
            lines: &lines,
        };
        let recognition = recognizer.recognize(&file, Requested::Found, &[]);
        assert_eq!(recognition.cases, [] as [aye_events::Case; 0]);
        assert_eq!(
            recognition.refine.as_ref().map(RefineReason::as_str),
            Some(REFINE_REASON)
        );
    }

    /// Without the doctest adapter, what pytest's doctest plugin would
    /// collect is refused, never dropped.
    #[test]
    fn pytests_own_doctests_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let source = ">>> 1 + 1\n2\n";
        write(root, "test_notes.txt", source);
        let project = Project::new(root, root);
        let recognizer = Pytest::default()
            .prepare_among(&project, &[NAME])
            .unwrap()
            .unwrap();
        assert_eq!(
            recognizer.claim("test_notes.txt", Requested::Found),
            Claim::Tests
        );
        let parsed = ruff_python_parser::parse_module("").unwrap();
        let lines = aye_adapter::LineIndex::from_source_text(source);
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("test_notes.txt"),
            abs: &root.join("test_notes.txt"),
            source,
            module: parsed.syntax(),
            lines: &lines,
        };
        let recognition = recognizer.recognize(&file, Requested::Found, &[]);
        assert!(recognition.cases.is_empty() && recognition.refine.is_none());
        assert_eq!(recognition.diagnostics[0].code, DiagnosticCode::UsageError);
        assert!(
            recognition.diagnostics[0].message.contains("doctest"),
            "{:?}",
            recognition.diagnostics
        );
        // A static listing refuses it too.
        let listing = Pytest::static_listing()
            .prepare_among(&project, &[NAME])
            .unwrap()
            .unwrap();
        assert_eq!(
            listing.recognize(&file, Requested::Found, &[]).diagnostics[0].code,
            DiagnosticCode::UsageError
        );
        // With the doctest adapter, the file is the doctest adapter's.
        let recognizer = Pytest::default()
            .prepare_among(&project, &[DOCTEST, NAME])
            .unwrap()
            .unwrap();
        assert_eq!(
            recognizer.claim("test_notes.txt", Requested::Found),
            Claim::Ignore
        );
    }

    /// `aye --co --no-refine` lists without importing: the static
    /// recognizer's cases, exact where it can predict them, and its guess,
    /// to be refined, where it can't.
    #[test]
    fn a_static_listing_predicts_what_it_can() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let exact =
            "import pytest\n\n@pytest.mark.parametrize('x', [1, 2])\ndef test_a(x):\n    pass\n";
        let dynamic =
            "import pytest\n\n@pytest.mark.parametrize('x', make())\ndef test_b(x):\n    pass\n";
        write(root, "test_a.py", exact);
        write(root, "test_b.py", dynamic);
        let project = Project::new(root, root);
        let listing = Pytest::static_listing()
            .prepare_among(&project, &[DOCTEST, NAME])
            .unwrap()
            .unwrap();
        let recognize = |rel: &str, source: &str| {
            let parsed = ruff_python_parser::parse_module(source).unwrap();
            let lines = aye_adapter::LineIndex::from_source_text(source);
            let file = SourceFile {
                rel: aye_events::ProjectPathRef::as_written(rel),
                abs: &root.join(rel),
                source,
                module: parsed.syntax(),
                lines: &lines,
            };
            listing.recognize(&file, Requested::Found, &[])
        };
        let recognition = recognize("test_a.py", exact);
        assert_eq!(recognition.refine, None);
        let ids: Vec<(String, aye_events::Certainty)> = recognition
            .cases
            .iter()
            .map(|c| (c.id.to_string(), c.certainty))
            .collect();
        assert_eq!(
            ids,
            [
                ("test_a.py::test_a[1]".into(), aye_events::Certainty::Exact),
                ("test_a.py::test_a[2]".into(), aye_events::Certainty::Exact)
            ]
        );
        let recognition = recognize("test_b.py", dynamic);
        assert!(recognition.refine.is_some());
        assert_eq!(recognition.cases[0].id.to_string(), "test_b.py::test_b");
        // A run refines every file.
        let run = Pytest::default()
            .prepare_among(&project, &[DOCTEST, NAME])
            .unwrap()
            .unwrap();
        let parsed = ruff_python_parser::parse_module(exact).unwrap();
        let lines = aye_adapter::LineIndex::from_source_text(exact);
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("test_a.py"),
            abs: &root.join("test_a.py"),
            source: exact,
            module: parsed.syntax(),
            lines: &lines,
        };
        let recognition = run.recognize(&file, Requested::Found, &[]);
        assert_eq!(recognition.cases, []);
        assert_eq!(
            recognition.refine.as_ref().map(RefineReason::as_str),
            Some(REFINE_REASON)
        );
    }
}
