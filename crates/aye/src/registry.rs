//! The adapters the runner knows, in priority order, which of them a
//! project runs, and what the binary must know of them beyond the contract
//! (which ones pytest collects in one module, what a conftest is). Linking
//! an adapter here is the only place one is depended on. aye's engine runs
//! every test: the pytest adapter reads pytest's tests with its front-end
//! and aye's own `pytest` module, and the other adapters' workers get that
//! module too ([`WithAyePytest`]), so pytest is never imported.

use std::fmt;

use anyhow::{Context as _, Result, bail};
use aye_adapter::{Adapter, AdapterError, Project, Recognizer};
use aye_adapter_pytest::native::SHIM_MODULE;
use aye_engine::{MODEL_MODULE, NATIVE_ENGINE_MODULE, REWRITE_MODULE};
use aye_events::AdapterName;
use aye_protocol::{Executor, ModuleSet};

/// Every adapter the binary links, in priority order: the pytest adapter
/// (for a static listing, `aye --co --no-refine`, the one that predicts
/// without importing: [`aye_adapter_pytest::Pytest::static_listing`]), then
/// unittest's and doctest's, with aye's `pytest` module in their workers.
fn known(static_listing: bool) -> Vec<Box<dyn Adapter>> {
    let pytest = if static_listing {
        aye_adapter_pytest::Pytest::static_listing()
    } else {
        aye_adapter_pytest::Pytest::default()
    };
    vec![
        Box::new(pytest),
        Box::new(WithAyePytest {
            inner: aye_adapter_unittest::Unittest,
            modules: UNITTEST_WITH_AYE_PYTEST,
        }),
        Box::new(WithAyePytest {
            inner: aye_adapter_doctest::Doctest,
            modules: DOCTEST_WITH_AYE_PYTEST,
        }),
    ]
}

/// The unittest adapter's modules after aye's `pytest` module and what it
/// stands on, in the native engine's order.
const UNITTEST_WITH_AYE_PYTEST: ModuleSet = ModuleSet::new(
    const {
        &[
            MODEL_MODULE,
            REWRITE_MODULE,
            NATIVE_ENGINE_MODULE,
            SHIM_MODULE,
            aye_adapter_unittest::MODULE,
        ]
    },
);

/// The doctest adapter's, likewise.
const DOCTEST_WITH_AYE_PYTEST: ModuleSet = ModuleSet::new(
    const {
        &[
            MODEL_MODULE,
            REWRITE_MODULE,
            NATIVE_ENGINE_MODULE,
            SHIM_MODULE,
            aye_adapter_doctest::MODULE,
        ]
    },
);

/// An adapter other than pytest's whose workers get aye's own `pytest`
/// module first, so that what they import (the conftests the doctest
/// adapter loads, a test module beside `TestCase` classes) imports it
/// rather than failing without pytest, or loading the real one: pytest is
/// never imported. Otherwise the adapter itself.
struct WithAyePytest<A> {
    inner: A,
    /// aye's `pytest` module and its own, in install order (the adapter's
    /// own last).
    modules: ModuleSet,
}

impl<A: Adapter> Adapter for WithAyePytest<A> {
    fn name(&self) -> AdapterName {
        self.inner.name()
    }

    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        self.inner.prepare(project)
    }

    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[AdapterName],
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
pub(crate) fn collects_in_one_module(adapter: &AdapterName) -> bool {
    *adapter == aye_adapter_pytest::NAME || *adapter == aye_adapter_unittest::NAME
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
    pub(crate) fn names(&self) -> Vec<AdapterName> {
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
/// error whichever adapters run. `[tool.aye]` is the project's.
pub(crate) fn choose(project: &Project<'_>, static_listing: bool) -> Result<Chosen> {
    let settings = project.tool_aye;
    let (names, why) = match &settings.adapters {
        Some(names) => (names.clone(), Why::Configured),
        None => (
            vec![
                aye_adapter_doctest::NAME.to_string(),
                aye_adapter_pytest::NAME.to_string(),
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
        if adapters.iter().any(|a| a.name() == name.as_str()) {
            bail!("[tool.aye] adapters: {name:?} is listed twice");
        }
        let Some(at) = known.iter().position(|a| a.name() == name.as_str()) else {
            bail!("[tool.aye] adapters: unknown adapter {name:?} (known: {all})");
        };
        adapters.push(known.remove(at));
    }
    for adapter in &known {
        if settings.adapter_table(adapter.name().as_str()).is_some() {
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

    use aye_config::AyeSettings;
    use aye_protocol::Request;
    use aye_static::EnvVars;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn project<'a>(root: &'a Path, args: &'a [PathBuf], tool_aye: &'a AyeSettings) -> Project<'a> {
        Project {
            args,
            tool_aye,
            ..Project::new(root, root)
        }
    }

    fn names(chosen: Option<&[&str]>) -> Result<Vec<AdapterName>> {
        let dir = tempfile::tempdir().unwrap();
        let settings = AyeSettings {
            adapters: chosen.map(|names| names.iter().map(ToString::to_string).collect()),
            ..AyeSettings::default()
        };
        choose(&project(dir.path(), &[], &settings), false).map(|c| c.names())
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
            let chosen = choose(&project(root, args, &settings), false).unwrap();
            (chosen.names(), chosen.why)
        };
        let default = (
            vec![aye_adapter_doctest::NAME, aye_adapter_pytest::NAME],
            Why::Default,
        );
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
            (aye_adapter_unittest::Unittest.executor()),
            (aye_adapter_doctest::Doctest.executor()),
        ]) {
            let executor = adapter.executor();
            let names: Vec<&str> = executor.modules.names().collect();
            assert_eq!(
                names[..4],
                ["aye_model", "aye_rewrite", "aye_native_engine", "pytest"]
            );
            assert_eq!(
                executor.modules.modules()[4..],
                *own.modules.modules(),
                "{}",
                own.adapter
            );
            assert_eq!((executor.adapter, executor.entry), (own.adapter, own.entry));
        }
    }

    /// The file each module a worker installs comes from, relative to the
    /// workspace's `crates`.
    const SOURCES: [(&str, &str); 9] = [
        ("aye_engine", "aye-engine/python/aye_engine.py"),
        ("aye_model", "aye-engine/python/aye_model.py"),
        ("aye_rewrite", "aye-engine/python/aye_rewrite.py"),
        (
            "aye_native_engine",
            "aye-engine/python/aye_native_engine.py",
        ),
        (
            "pytest",
            "aye-adapter-pytest/python/native/aye_pytest_shim.py",
        ),
        (
            "aye_pytest_frontend",
            "aye-adapter-pytest/python/native/aye_pytest_frontend.py",
        ),
        (
            "aye_pytest_native",
            "aye-adapter-pytest/python/native/aye_pytest_native.py",
        ),
        (
            "aye_engine_unittest",
            "aye-adapter-unittest/python/aye_engine_unittest.py",
        ),
        (
            "aye_engine_doctest",
            "aye-adapter-doctest/python/aye_engine_doctest.py",
        ),
    ];

    /// The handshake of a worker for `adapters` (indices into the known
    /// adapters, in the order the run lists them), as the runner sends it,
    /// with each module's source, once checked against its file, written
    /// as `<name>`.
    fn hello(adapters: &[usize]) -> String {
        let known = known(false);
        let mut spec = aye_runner::WorkerSpec::new("python", "/repo", "/repo");
        spec.executors = adapters.iter().map(|&at| known[at].executor()).collect();
        let hello = spec.hello();
        let mut line = serde_json::to_string(&hello).unwrap();
        let Request::Hello { modules, .. } = hello else {
            panic!("expected hello");
        };
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for module in modules {
            let name = &module.name;
            let (_, file) = SOURCES
                .iter()
                .find(|(of, _)| of == name)
                .unwrap_or_else(|| panic!("no file for {name}"));
            let source = fs::read_to_string(crates.join(file)).unwrap();
            assert_eq!(module.source, source, "{name}");
            let written = serde_json::to_string(&module.source).unwrap();
            line = line.replacen(&written, &format!("\"<{name}>\""), 1);
        }
        line
    }

    /// What every worker is told to install and to build, as aye 0.0.1
    /// told it: aye's engine, aye's `pytest` module and what it stands on,
    /// then each adapter's own modules, each once, its executor's entry,
    /// whether it lists the session, and the services. Alone, and as a
    /// run's workers that hold several adapters' executors.
    #[test]
    fn every_workers_handshake_is_pinned() {
        const PRELUDE: &str = r#"{"name":"aye_engine","source":"<aye_engine>"},{"name":"aye_model","source":"<aye_model>"},{"name":"aye_rewrite","source":"<aye_rewrite>"},{"name":"aye_native_engine","source":"<aye_native_engine>"},{"name":"pytest","source":"<pytest>"}"#;
        const PYTEST: &str = r#"{"name":"aye_pytest_frontend","source":"<aye_pytest_frontend>"},{"name":"aye_pytest_native","source":"<aye_pytest_native>"}"#;
        const UNITTEST: &str = r#"{"name":"aye_engine_unittest","source":"<aye_engine_unittest>"}"#;
        const DOCTEST: &str = r#"{"name":"aye_engine_doctest","source":"<aye_engine_doctest>"}"#;
        const PYTEST_ENTRY: &str = r#"{"adapter":"pytest","entry":"aye_pytest_native:NativeExecutor","lists_session":true}"#;
        const UNITTEST_ENTRY: &str =
            r#"{"adapter":"unittest","entry":"aye_engine_unittest:UnittestExecutor"}"#;
        const DOCTEST_ENTRY: &str =
            r#"{"adapter":"doctest","entry":"aye_engine_doctest:DoctestExecutor"}"#;
        let pinned = |modules: &[&str], executors: &[&str]| {
            format!(
                r#"{{"type":"hello","protocol":3,"root":"/repo","modules":[{PRELUDE},{}],"executors":[{}],"services":["context","cleanup"]}}"#,
                modules.join(","),
                executors.join(","),
            )
        };
        assert_eq!(hello(&[0]), pinned(&[PYTEST], &[PYTEST_ENTRY]));
        assert_eq!(hello(&[1]), pinned(&[UNITTEST], &[UNITTEST_ENTRY]));
        assert_eq!(hello(&[2]), pinned(&[DOCTEST], &[DOCTEST_ENTRY]));
        assert_eq!(
            hello(&[2, 0]),
            pinned(&[DOCTEST, PYTEST], &[DOCTEST_ENTRY, PYTEST_ENTRY])
        );
        assert_eq!(
            hello(&[0, 1, 2]),
            pinned(
                &[PYTEST, UNITTEST, DOCTEST],
                &[PYTEST_ENTRY, UNITTEST_ENTRY, DOCTEST_ENTRY]
            )
        );
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
            let err = choose(&project(root, &[], &settings(table)), false)
                .err()
                .unwrap_or_else(|| panic!("{table}: accepted"));
            assert!(
                format!("{err:#}").contains("configuration"),
                "{table}: {err:#}"
            );
        }
        let good = settings("[tool.aye.unittest]\npattern = \"*_test.py\"\n");
        assert_eq!(
            choose(&project(root, &[], &good), false).unwrap().names(),
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
                    .map(|m| (&*m.name, &*m.source)),
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
