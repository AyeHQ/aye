//! The adapters the runner knows, in priority order, which of them a
//! project runs, and what the binary must know of them beyond the contract
//! (which ones pytest collects in one module, what a conftest is). Linking
//! an adapter here is the only place one is depended on. aye's engine runs
//! every test: the pytest adapter reads pytest's tests with its front-end
//! and aye's own `pytest` module, and the other adapters' workers get that
//! module too ([`WithAyePytest`]), so pytest is never imported.

use std::fmt;

use anyhow::{Context as _, Result, bail};
use aye_config::AyeSettings;
use aye_engine::{
    Adapter, AdapterError, EmbeddedModule, Executor, MODEL_MODULE, NATIVE_ENGINE_MODULE, Project,
    REWRITE_MODULE, Recognizer,
};
use aye_engine_pytest::native::SHIM_MODULE;

/// Every adapter the binary links, in priority order: the pytest adapter
/// (for a static listing, `aye --co --no-refine`, the one that predicts
/// without importing: [`aye_engine_pytest::Pytest::static_listing`]), then
/// unittest's and doctest's, with aye's `pytest` module in their workers.
fn known(static_listing: bool) -> Vec<Box<dyn Adapter>> {
    let pytest = if static_listing {
        aye_engine_pytest::Pytest::static_listing()
    } else {
        aye_engine_pytest::Pytest::default()
    };
    vec![
        Box::new(pytest),
        Box::new(WithAyePytest {
            inner: aye_engine_unittest::Unittest,
            modules: &UNITTEST_WITH_AYE_PYTEST,
        }),
        Box::new(WithAyePytest {
            inner: aye_engine_doctest::Doctest,
            modules: &DOCTEST_WITH_AYE_PYTEST,
        }),
    ]
}

/// The unittest adapter's modules after aye's `pytest` module and what it
/// stands on, in the native engine's order.
static UNITTEST_WITH_AYE_PYTEST: [EmbeddedModule; 5] = [
    MODEL_MODULE,
    REWRITE_MODULE,
    NATIVE_ENGINE_MODULE,
    SHIM_MODULE,
    aye_engine_unittest::MODULE,
];

/// The doctest adapter's, likewise.
static DOCTEST_WITH_AYE_PYTEST: [EmbeddedModule; 5] = [
    MODEL_MODULE,
    REWRITE_MODULE,
    NATIVE_ENGINE_MODULE,
    SHIM_MODULE,
    aye_engine_doctest::MODULE,
];

/// An adapter other than pytest's whose workers get aye's own `pytest`
/// module first, so that what they import (the conftests the doctest
/// adapter loads, a test module beside `TestCase` classes) imports it
/// rather than failing without pytest, or loading the real one: pytest is
/// never imported. Otherwise the adapter itself.
struct WithAyePytest<A> {
    inner: A,
    /// aye's `pytest` module and its own, in install order (the adapter's
    /// own last).
    modules: &'static [EmbeddedModule],
}

impl<A: Adapter> Adapter for WithAyePytest<A> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        self.inner.prepare(project)
    }

    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[&str],
    ) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        self.inner.prepare_among(project, adapters)
    }

    fn executor(&self) -> Executor {
        Executor {
            modules: self.modules,
            ..self.inner.executor()
        }
    }
}

/// Whether pytest, the referee whenever it runs, collects the adapter's
/// tests of a file through the one collector that collects pytest's own,
/// its `Module` (which collects `TestCase` classes too): a file that fails
/// to import is then one collection error, however many of these adapters
/// load it. The doctest adapter's tests are pytest's `DoctestModule`,
/// another collector, whose error pytest reports as well.
pub(crate) fn collects_in_one_module(adapter: &str) -> bool {
    adapter == aye_engine_pytest::NAME || adapter == aye_engine_unittest::NAME
}

/// Whether a diagnostic's path (relative to the root, `/` separators) is a
/// pytest conftest, which pytest loads once per session however many
/// adapters load it (the doctest adapter imports the conftests pytest
/// would): its failure to load is one error.
pub(crate) fn is_conftest(path: &str) -> bool {
    path.rsplit('/').next() == Some("conftest.py")
}

/// The adapters a run uses, in priority order, and why.
pub(crate) struct Chosen {
    pub adapters: Vec<Box<dyn Adapter>>,
    pub why: Why,
}

impl Chosen {
    #[cfg(test)]
    pub(crate) fn names(&self) -> Vec<&'static str> {
        self.adapters.iter().map(|a| a.name()).collect()
    }
}

/// Why a run uses the adapters it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Why {
    /// `[tool.aye] adapters` lists them.
    Configured,
    /// The default: doctest then pytest.
    Default,
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configured => "from [tool.aye] adapters",
            Self::Default => "default: pytest's rules, which run unittest's TestCase classes too",
        })
    }
}

/// The adapters a project runs: those `[tool.aye] adapters` names, in its
/// order (an unknown name is an error); else doctest then pytest, whatever
/// the interpreter installs: aye's engine runs pytest's tests without
/// pytest, pytest's rules collect unittest's `TestCase` classes as well,
/// pytest lists a file's doctests before its other tests, and the doctest
/// adapter prepares nothing unless doctests are on. A project that wants
/// `python -m unittest discover`'s rules names `unittest`. With
/// `static_listing` (`aye --co --no-refine`), the pytest adapter predicts
/// its cases without importing. The tables of adapters that don't run
/// (`[tool.aye.unittest]`) are checked all the same: a bad value is an
/// error whichever adapters run.
pub(crate) fn choose(
    settings: &AyeSettings,
    project: &Project<'_>,
    static_listing: bool,
) -> Result<Chosen> {
    let (names, why) = match &settings.adapters {
        Some(names) => (names.clone(), Why::Configured),
        None => (
            vec![
                aye_engine_doctest::Doctest.name().to_string(),
                aye_engine_pytest::NAME.to_string(),
            ],
            Why::Default,
        ),
    };
    let mut known = known(static_listing);
    let all = known
        .iter()
        .map(|a| a.name())
        .collect::<Vec<_>>()
        .join(", ");
    let mut adapters: Vec<Box<dyn Adapter>> = Vec::new();
    for name in &names {
        if adapters.iter().any(|a| a.name() == name) {
            bail!("[tool.aye] adapters: {name:?} is listed twice");
        }
        let Some(at) = known.iter().position(|a| a.name() == name) else {
            bail!("[tool.aye] adapters: unknown adapter {name:?} (known: {all})");
        };
        adapters.push(known.remove(at));
    }
    for adapter in &known {
        if settings.adapter_table(adapter.name()).is_some() {
            adapter
                .prepare(project)
                .with_context(|| format!("invalid {} configuration", adapter.name()))?;
        }
    }
    Ok(Chosen { adapters, why })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use aye_static::EnvVars;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn project<'a>(root: &'a Path, args: &'a [PathBuf]) -> Project<'a> {
        Project {
            root,
            cwd: root,
            args,
            config_file: None,
            framework_args: &[],
        }
    }

    fn names(chosen: Option<&[&str]>) -> Result<Vec<&'static str>> {
        let dir = tempfile::tempdir().unwrap();
        let settings = AyeSettings {
            adapters: chosen.map(|names| names.iter().map(ToString::to_string).collect()),
            ..AyeSettings::default()
        };
        choose(&settings, &project(dir.path(), &[]), false).map(|c| c.names())
    }

    #[test]
    fn the_setting_chooses_the_adapters_in_order() {
        assert_eq!(names(None).unwrap(), ["doctest", "pytest"]);
        assert_eq!(
            names(Some(&["unittest", "pytest", "doctest"])).unwrap(),
            ["unittest", "pytest", "doctest"]
        );
        assert_eq!(
            names(Some(&["unittest", "nose"])).unwrap_err().to_string(),
            "[tool.aye] adapters: unknown adapter \"nose\" (known: pytest, unittest, doctest)"
        );
        assert_eq!(
            names(Some(&["unittest", "unittest"]))
                .unwrap_err()
                .to_string(),
            "[tool.aye] adapters: \"unittest\" is listed twice"
        );
    }

    /// The default is doctest then pytest for every project, whatever its
    /// files say and whatever the interpreter installs: a project of
    /// `TestCase` classes with no pytest configuration and no conftest.py
    /// too (pytest's rules run them).
    #[test]
    fn the_default_is_doctest_then_pytest_for_every_project() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "pyproject.toml", "[project]\nname = \"p\"\n");
        write(
            root,
            "tests/test_a.py",
            "import unittest\n\nclass T(unittest.TestCase):\n    def test_a(self):\n        pass\n",
        );
        let settings = AyeSettings::default();
        let chosen = |args: &[PathBuf]| {
            let chosen = choose(&settings, &project(root, args), false).unwrap();
            (chosen.names(), chosen.why)
        };
        let default = (vec!["doctest", "pytest"], Why::Default);
        assert_eq!(chosen(&[]), default);
        assert_eq!(chosen(&[PathBuf::from("tests")]), default);
        write(root, "tests/conftest.py", "");
        write(root, "pytest.ini", "[pytest]\n");
        assert_eq!(chosen(&[]), default);
        assert_eq!(
            Why::Default.to_string(),
            "default: pytest's rules, which run unittest's TestCase classes too"
        );
    }

    /// The pytest adapter's executor is aye's engine with the pytest
    /// front-end; the other adapters' workers get aye's `pytest` module
    /// before their own modules, and keep their entries. A static listing
    /// changes only what the pytest adapter's recognizer predicts.
    #[test]
    fn every_adapter_runs_on_ayes_engine_without_pytest() {
        let adapters = known(false);
        let pytest = adapters[0].executor();
        assert_eq!(pytest.adapter, "pytest");
        assert_eq!(pytest.entry, "aye_pytest_native:NativeExecutor");
        assert!(pytest.lists_session);
        assert_eq!(known(true)[0].executor().entry, pytest.entry);
        for (adapter, own) in adapters.iter().skip(1).zip([
            (aye_engine_unittest::Unittest.executor()),
            (aye_engine_doctest::Doctest.executor()),
        ]) {
            let executor = adapter.executor();
            let names: Vec<&str> = executor.modules.iter().map(|m| m.name).collect();
            assert_eq!(
                names[..4],
                ["aye_model", "aye_rewrite", "aye_native_engine", "pytest"]
            );
            assert_eq!(executor.modules[4..], *own.modules, "{}", own.adapter);
            assert_eq!((executor.adapter, executor.entry), (own.adapter, own.entry));
        }
    }

    #[test]
    fn the_reason_is_shown_in_words() {
        assert_eq!(Why::Configured.to_string(), "from [tool.aye] adapters");
    }

    /// Finding C34: an adapter's table is checked even when the adapter
    /// doesn't run.
    #[test]
    fn the_tables_of_adapters_that_dont_run_are_checked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let settings = |table: &str| {
            write(
                root,
                "pyproject.toml",
                &format!("[project]\nname = \"p\"\n[tool.aye]\nadapters = [\"pytest\"]\n{table}"),
            );
            aye_config::AyeConfig::load(root).unwrap().settings
        };
        for table in [
            "[tool.aye.unittest]\nfoo = 1\n",
            "[tool.aye.unittest]\nstart-directory = \"missing\"\n",
            "[tool.aye.doctest]\nfoo = 1\n",
            "[tool.aye.doctest]\nmodules = \"yes\"\n",
        ] {
            let err = choose(&settings(table), &project(root, &[]), false)
                .err()
                .unwrap_or_else(|| panic!("{table}: accepted"));
            assert!(
                format!("{err:#}").contains("configuration"),
                "{table}: {err:#}"
            );
        }
        let good = settings("[tool.aye.unittest]\npattern = \"*_test.py\"\n");
        assert_eq!(
            choose(&good, &project(root, &[]), false).unwrap().names(),
            ["pytest"]
        );
    }

    /// The Python every worker runs names environment variables itself (it
    /// imports only the standard library): each one it reads or sets by
    /// name is in aye's catalogue.
    #[test]
    fn the_python_sent_to_workers_names_only_catalogued_variables() {
        let mut sources = vec![("aye_worker", aye_runner::WORKER_SOURCE)];
        for adapter in known(false) {
            sources.extend(
                adapter
                    .executor()
                    .modules
                    .iter()
                    .map(|m| (m.name, m.source)),
            );
        }
        let mut named = BTreeSet::new();
        for (module, source) in sources {
            let lines = source
                .lines()
                .filter(|line| line.contains("environ") || line.contains("_env="));
            for name in lines.flat_map(|line| line.split('"').skip(1).step_by(2)) {
                if name.len() > 1
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                {
                    named.insert((name, module));
                }
            }
        }
        let missing: Vec<_> = named
            .iter()
            .filter(|(name, _)| !EnvVars::ALL.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "not in aye_static::EnvVars: {missing:?}"
        );
        assert!(
            named
                .iter()
                .any(|(name, _)| *name == EnvVars::PYTEST_CURRENT_TEST)
        );
    }
}
