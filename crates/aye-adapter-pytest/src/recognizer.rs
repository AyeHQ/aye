//! The pytest adapter's recognizer ([`NativeRecognizer`]): the static
//! recognizer's claims, every test file refined (or, for a static listing,
//! predicted), and the context with its `native` section.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::{
    Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested, Setting, SourceFile,
};
use aye_config::ValueSource;
use aye_events::{Diagnostic, DiagnosticCode, Severity};
use aye_protocol::AdapterContext;
use aye_pytest::config::ConfigRules;
use aye_pytest::session::is_virtualenv;
use aye_python_stdlib::fnmatch;
use aye_python_stdlib::os_path::normalize_absolute;

use crate::adapter::Listing;
use crate::claims::doctest_plugin::{PYTEST_DOCTESTS, PytestDoctests};
use crate::claims::ignore::Ignored;
use crate::conftest::{Summary, Supports};
use crate::context::{PytestContext, native_context};
use crate::predict;
use crate::session::plugins::SessionPlugin;
use crate::session::scan::Scan;
use crate::{PytestConfig, SETS};

/// The pytest adapter's static face for one project.
#[derive(Debug)]
pub struct PytestRecognizer {
    pub(crate) root: PathBuf,
    pub(crate) config: PytestConfig,
    pub(crate) settings: predict::Settings,
    pub(crate) supports: Supports,
    /// Project modules `-p NAME` loads (from addopts or `PYTEST_ADDOPTS`),
    /// then those `PYTEST_PLUGINS` names, in order: plugins of the whole
    /// session, each with what loads it, for messages.
    pub(crate) preparse: Vec<SessionPlugin>,
    /// Conftests above the project root that pytest loads, outermost first.
    pub(crate) outer: Vec<Arc<Summary>>,
    /// What walking the project once finds (see [`PytestRecognizer::scan`]).
    pub(crate) scan: OnceLock<Scan>,
    /// `-p NAME` and `PYTEST_PLUGINS` plugins aye can't read: neither
    /// pytest's own nor project modules. They may mark, deselect or
    /// parametrize any test.
    pub(crate) unseen_plugins: Vec<String>,
    /// The paths pytest starts collecting from in the user's run
    /// ([`Recognizer::roots`]: the command-line paths, else `testpaths`,
    /// else the current directory), absolute.
    pub(crate) collection_args: Vec<PathBuf>,
    /// The directories whose conftests pytest loads before
    /// `pytest_configure` (its "initial conftests"): those of the collection
    /// arguments and their `test*` subdirectories. Each holds its ancestors
    /// too.
    pub(crate) initial_dirs: Vec<PathBuf>,
    /// Directories already reported for a non-top-level `pytest_plugins`.
    pub(crate) reported: Mutex<HashSet<PathBuf>>,
    /// `PYTHONPATH`'s directories, absolute: on `sys.path` in the workers.
    pub(crate) env_pythonpath: Vec<PathBuf>,
    /// Who collects the run's doctests: the doctest adapter, unless
    /// [`Self::with_pytest_doctests`] says no doctest adapter runs.
    pub(crate) doctests: PytestDoctests,
    /// The context, built once (see [`Self::pytest_context`]).
    pub(crate) context: OnceLock<PytestContext>,
}

impl Recognizer for PytestRecognizer {
    /// The command-line paths; else `testpaths`, glob-expanded (as they
    /// are with `--pyargs`), when run from the rootdir; else the current
    /// directory.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if !project.args.is_empty() {
            return project.args.iter().map(|a| project.cwd.join(a)).collect();
        }
        let testpaths = &self.config.testpaths.value;
        if project.cwd == self.config.rootdir && !testpaths.is_empty() {
            let rootdir = &self.config.rootdir;
            // With `--pyargs`, testpaths are taken as they are: dotted
            // module names pytest looks up on `sys.path` first, else paths.
            if self.config.pyargs {
                return testpaths
                    .iter()
                    .map(|entry| {
                        self.pyarg(entry)
                            .unwrap_or_else(|| normalize_absolute(&rootdir.join(entry)))
                    })
                    .collect();
            }
            let roots = ConfigRules::Exact.testpaths(rootdir, testpaths, SETS);
            if !roots.is_empty() {
                return roots;
            }
        }
        vec![project.cwd.to_path_buf()]
    }

    /// `pytest_ignore_collect` for a directory met while walking.
    /// Directories named on the command line are never ignored by pytest
    /// and need not be asked.
    fn descend(&self, dir: &Path) -> bool {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if name == "__pycache__" {
            return false;
        }
        if !self.config.collect_in_virtualenv && is_virtualenv(dir) {
            return false;
        }
        if self
            .config
            .norecursedirs
            .value
            .iter()
            .any(|pattern| fnmatch::fnmatch_ex(pattern, dir, SETS))
        {
            return false;
        }
        self.ignored(dir) != Ignored::Yes
    }

    /// pytest's `pytest_collect_file`: a `.py` file named on the command
    /// line, or matching `python_files` and not ignored, is a test module.
    /// A `conftest.py` is support whatever else it is (ignoring it doesn't
    /// stop it loading as a conftest), and a test module too when it
    /// qualifies as one; with `--noconftest` it is only an ordinary file.
    /// When pytest's doctest plugin collects doctests in this run, the
    /// files it collects are test files too ([`Self::doctest_file`]).
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let support = name == "conftest.py" && !self.config.noconftest;
        let explicit = requested.is_named();
        let tests =
            || self.python_module(rel, explicit) || self.doctest_file(rel, explicit).is_some();
        match (support, tests()) {
            (true, true) => Claim::TestsAndSupport,
            (true, false) => Claim::Support,
            (false, true) => Claim::Tests,
            (false, false) => Claim::Ignore,
        }
    }

    /// The static prediction: what pytest collects from `file`, predicted
    /// without importing anything.
    fn recognize(
        &self,
        file: &SourceFile<'_>,
        requested: Requested,
        support: &[SourceFile<'_>],
    ) -> Recognition {
        self.predict(file, requested, support)
    }

    fn settings(&self) -> Vec<Setting> {
        let mut settings = self.config.settings();
        if let Some(doctests) = self.doctests.by_pytest() {
            let mut what = Vec::new();
            if doctests.modules {
                what.push("modules (--doctest-modules)".to_string());
            }
            if doctests.globs.is_empty() {
                what.push("text files matching test*.txt".to_string());
            } else {
                what.push(format!("text files matching {}", doctests.globs.join(" ")));
            }
            settings.push(Setting {
                name: "doctests".into(),
                value: format!("collected by pytest's doctest plugin: {}", what.join(", ")),
                source: ValueSource::Rule(PYTEST_DOCTESTS),
            });
        }
        settings
    }

    /// What the front-end needs to start as pytest would start itself (the
    /// rest is the native context's: see the adapter's recognizer), and
    /// what the runner takes from pytest's options: [`PytestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.pytest_context())
    }

    /// What the context's keys for the runner say ([`PytestContext::defaults`]).
    fn defaults(&self) -> FrameworkDefaults {
        self.pytest_context().defaults()
    }
}

/// Why every test file is refined.
const REFINE_REASON: &str = "aye's pytest front-end reads every test file";

/// The pytest recognizer, with every test file refined (or, for a static
/// listing, predicted) and the `native` context.
#[derive(Debug)]
pub(crate) struct NativeRecognizer {
    inner: PytestRecognizer,
    /// What [`Recognizer::recognize`] gives for a test file.
    listing: Listing,
    /// The context, with its `native` section, built once.
    context: OnceLock<PytestContext>,
}

impl NativeRecognizer {
    pub(crate) fn new(inner: PytestRecognizer, listing: Listing) -> Self {
        Self {
            inner,
            listing,
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
        if self.inner.doctests.by_pytest().is_some()
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
        match self.listing {
            Listing::Static => self.inner.recognize(file, requested, support),
            Listing::Refine => Recognition {
                refine: Some(REFINE_REASON.into()),
                ..Recognition::default()
            },
        }
    }

    fn settings(&self) -> Vec<Setting> {
        self.inner.settings()
    }

    /// The pytest adapter's context (see [`PytestRecognizer`]'s), plus
    /// [`NativeContext`](crate::context::NativeContext).
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
        NativeRecognizer::new(
            PytestRecognizer::with_env(&project, None).unwrap(),
            Listing::Refine,
        )
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
