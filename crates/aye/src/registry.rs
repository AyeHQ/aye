//! The adapters the runner knows, in priority order, each with what the
//! binary must know of it beyond the contract ([`AdapterTraits`]: which
//! ones pytest collects in one module, which refines every file it
//! collects, the support file pytest loads once), which of them a project
//! runs and their preparation ([`choose`], [`ActiveAdapters`]), and what
//! every worker installs before any adapter's modules ([`PRELUDE`]).
//! Linking an adapter here is the only place one is depended on. aye's
//! engine runs every test: the pytest adapter reads pytest's tests with
//! its front-end and aye's own `pytest` module, which every adapter's
//! workers get, so pytest is never imported.
//!
//! Decided (implementation, October 2026):
//! - **Which adapters run** is `[tool.aye] adapters`, in its order (an
//!   unknown or repeated name, or an empty list, is a usage error). Unset,
//!   it is doctest then pytest, for every project and whatever the
//!   interpreter installs (October 2026; it used to fall back to unittest
//!   where pytest wasn't installed): aye's engine runs pytest's tests
//!   without pytest, pytest's rules run `TestCase` classes too, pytest
//!   lists a file's doctests before its other tests, and the doctest
//!   adapter prepares nothing unless doctests are on. A project that wants
//!   `python -m unittest discover`'s rules lists `unittest`. Each adapter
//!   prepares with `prepare_among`, knowing the others. `aye config` shows
//!   the adapters and why. The table of an adapter that doesn't run
//!   (`[tool.aye.unittest]`) is still checked: its bad values are usage
//!   errors too.
//! - **aye's engine runs every test** (October 2026): pytest suites run
//!   with aye's own `pytest` module and pytest is never imported. The
//!   pytest adapter's front-end refines every test file
//!   ([`AdapterTraits::refines_every_file`]), and every adapter's workers
//!   get aye's `pytest` module, with what it stands on, before their own
//!   modules ([`PRELUDE`]: the doctest adapter imports conftests, which
//!   import it). `aye config` says `engine   native`. `--native`, the
//!   prototype's switch, is accepted and does nothing (`AYE_NATIVE` is no
//!   longer read), and reruns leave it out. A run of pytest's tests can't
//!   do without refinement: `--no-refine` is a usage error, unless with
//!   `--co`, which lists the static recognizer's prediction without
//!   importing anything. A `refine-failed` warning is an error: the
//!   front-end is the only source of pytest's tests, so without it the run
//!   has no result rather than an empty one. A file's collection error
//!   refinement reported counts as the pytest adapter's for the one-module
//!   rule ([`AdapterTraits::one_module`]), since its front-end lists every
//!   pytest file there.

use anyhow::{Context as _, Result, bail};
use aye_adapter::{Adapter, Project};
use aye_adapter_pytest::native::SHIM_MODULE;
use aye_collect::Prepared;
use aye_config::ValueSource;
use aye_engine::{MODEL_MODULE, NATIVE_ENGINE_MODULE, REWRITE_MODULE};
use aye_events::AdapterName;
use aye_protocol::{ModuleSet, Runtime};

/// What the binary must know of an adapter beyond the contract, which each
/// adapter it links declares ([`KNOWN`]): the rules by which pytest, the
/// referee whenever it runs, reports some errors once, and what a run can
/// do without refinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdapterTraits {
    /// Whether pytest collects the adapter's tests of a file through the
    /// one collector that collects pytest's own, its `Module` (which
    /// collects `TestCase` classes too): a file that fails to import is
    /// then one collection error, however many of these adapters load it.
    /// The doctest adapter's tests are pytest's `DoctestModule`, another
    /// collector, whose error pytest reports as well.
    pub(crate) one_module: bool,
    /// Whether its refinement lists every file it collects (aye's pytest
    /// front-end): a run of its tests imports every file it runs, so it
    /// can't do without refinement, and a file's collection error that
    /// refinement reports is its.
    pub(crate) refines_every_file: bool,
    /// The name of the support file its framework loads once per session,
    /// however many adapters load it (pytest's `conftest.py`, which the
    /// doctest adapter imports too): its failure to load is one error.
    pub(crate) support_file: Option<&'static str>,
}

impl AdapterTraits {
    /// What an adapter the binary doesn't link would declare: nothing.
    const NONE: Self = Self {
        one_module: false,
        refines_every_file: false,
        support_file: None,
    };
}

/// How the pytest adapter lists its cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Listing {
    /// As its front-end lists them, refining every test file.
    Refine,
    /// As the static recognizer predicts them, importing nothing: `aye
    /// --co --no-refine` ([`aye_adapter_pytest::Pytest::static_listing`]).
    Static,
}

/// An adapter the binary links: its name, its traits, and the adapter
/// itself, for a listing.
struct Known {
    name: AdapterName,
    traits: AdapterTraits,
    adapter: fn(Listing) -> Box<dyn Adapter>,
}

/// Every adapter the binary links, in priority order: the pytest adapter,
/// then unittest's and doctest's.
static KNOWN: [Known; 3] = [
    Known {
        name: aye_adapter_pytest::NAME,
        traits: AdapterTraits {
            one_module: true,
            refines_every_file: true,
            support_file: Some("conftest.py"),
        },
        adapter: pytest,
    },
    Known {
        name: aye_adapter_unittest::NAME,
        traits: AdapterTraits {
            one_module: true,
            ..AdapterTraits::NONE
        },
        adapter: unittest,
    },
    Known {
        name: aye_adapter_doctest::NAME,
        traits: AdapterTraits::NONE,
        adapter: doctest,
    },
];

fn pytest(listing: Listing) -> Box<dyn Adapter> {
    match listing {
        Listing::Refine => Box::new(aye_adapter_pytest::Pytest::default()),
        Listing::Static => Box::new(aye_adapter_pytest::Pytest::static_listing()),
    }
}

fn unittest(_: Listing) -> Box<dyn Adapter> {
    Box::new(aye_adapter_unittest::Unittest)
}

fn doctest(_: Listing) -> Box<dyn Adapter> {
    Box::new(aye_adapter_doctest::Doctest)
}

/// Every adapter the binary links, in priority order, made for `listing`.
fn known(listing: Listing) -> Vec<Box<dyn Adapter>> {
    KNOWN.iter().map(|known| (known.adapter)(listing)).collect()
}

/// The traits of the adapter named `name`: nothing for a name the binary
/// doesn't link.
pub(crate) fn traits(name: &AdapterName) -> AdapterTraits {
    KNOWN
        .iter()
        .find(|known| known.name == *name)
        .map_or(AdapterTraits::NONE, |known| known.traits)
}

/// The support file a diagnostic's path (relative to the root, `/`
/// separators) names, when it names one some adapter's framework loads
/// once per session ([`AdapterTraits::support_file`]): its file name.
pub(crate) fn support_file(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next()?;
    KNOWN
        .iter()
        .filter_map(|known| known.traits.support_file)
        .find(|file| *file == name)
}

/// What every worker installs after aye's engine and before its adapters'
/// own modules, whatever they are: aye's `pytest` module and what it stands
/// on (the model, the rewriter and the native engine), so that what a
/// worker imports as `pytest` (a conftest the doctest adapter loads, a test
/// module beside `TestCase` classes, the pytest front-end) is aye's, rather
/// than failing without pytest or loading the real one: pytest is never
/// imported.
pub(crate) const PRELUDE: ModuleSet = ModuleSet::new(
    const {
        &[
            MODEL_MODULE,
            REWRITE_MODULE,
            NATIVE_ENGINE_MODULE,
            SHIM_MODULE,
        ]
    },
);

/// The adapters a run uses, in priority order, and why: `[tool.aye]
/// adapters`, whose source is the table's, or [`DEFAULT_ADAPTERS`]. Not
/// prepared yet ([`Chosen::prepare`]).
pub(crate) struct Chosen {
    adapters: Vec<Box<dyn Adapter>>,
    source: ValueSource,
}

impl Chosen {
    /// Their names, in priority order.
    pub(crate) fn names(&self) -> Vec<AdapterName> {
        self.adapters.iter().map(|a| a.name()).collect()
    }

    /// Whether one of them refines every file it collects, which a run of
    /// its tests can't do without ([`AdapterTraits::refines_every_file`]).
    pub(crate) fn refine_every_file(&self) -> bool {
        self.adapters
            .iter()
            .any(|adapter| traits(&adapter.name()).refines_every_file)
    }

    /// Prepare each adapter for the project, knowing the others.
    pub(crate) fn prepare(self, project: &Project<'_>) -> Result<ActiveAdapters> {
        let names = self.names();
        let mut prepared: Vec<Prepared> = Vec::new();
        for adapter in &self.adapters {
            if let Some(recognizer) = adapter
                .prepare_among(project, &names)
                .with_context(|| format!("invalid {} configuration", adapter.name()))?
            {
                prepared.push((adapter.name(), recognizer));
            }
        }
        Ok(ActiveAdapters {
            chosen: self,
            prepared,
        })
    }
}

/// The adapters a run uses ([`Chosen`]), prepared for the project: those
/// with something to collect here have a recognizer, in the same order.
pub(crate) struct ActiveAdapters {
    chosen: Chosen,
    prepared: Vec<Prepared>,
}

impl ActiveAdapters {
    /// The names of the adapters the run uses, in priority order.
    pub(crate) fn names(&self) -> Vec<AdapterName> {
        self.chosen.names()
    }

    /// Why the run uses those: `[tool.aye] adapters`' source, or
    /// [`DEFAULT_ADAPTERS`].
    pub(crate) const fn source(&self) -> &ValueSource {
        &self.chosen.source
    }

    /// What every worker builds an executor from, for each adapter the run
    /// uses, prepared or not.
    pub(crate) fn runtimes(&self) -> Vec<Runtime> {
        self.chosen.adapters.iter().map(|a| a.runtime()).collect()
    }

    /// The adapters prepared for the project, each with its recognizer, in
    /// priority order.
    pub(crate) fn prepared(&self) -> &[Prepared] {
        &self.prepared
    }

    /// The first prepared adapter that refines every file it collects
    /// ([`AdapterTraits::refines_every_file`]): a file's collection error
    /// that refinement reports counts as its.
    pub(crate) fn refined_by(&self) -> Option<&AdapterName> {
        self.prepared
            .iter()
            .map(|(name, _)| name)
            .find(|name| traits(name).refines_every_file)
    }
}

/// Why a run uses doctest then pytest when `[tool.aye] adapters` says
/// nothing.
pub(crate) const DEFAULT_ADAPTERS: ValueSource =
    ValueSource::Rule("default: pytest's rules, which run unittest's TestCase classes too");

/// The adapters a project runs: those `[tool.aye] adapters` names, in its
/// order (an unknown name is an error); else doctest then pytest, whatever
/// the interpreter installs: aye's engine runs pytest's tests without
/// pytest, pytest's rules collect unittest's `TestCase` classes as well,
/// pytest lists a file's doctests before its other tests, and the doctest
/// adapter prepares nothing unless doctests are on. A project that wants
/// `python -m unittest discover`'s rules names `unittest`. `listing` says
/// how the pytest adapter lists its cases. The tables of adapters that
/// don't run (`[tool.aye.unittest]`) are checked all the same: a bad value
/// is an error whichever adapters run. `[tool.aye]` is the project's.
pub(crate) fn choose(project: &Project<'_>, listing: Listing) -> Result<Chosen> {
    let settings = project.tool_aye;
    let (names, source) = match &settings.adapters {
        Some(names) => (names.value.names().to_vec(), names.source.clone()),
        None => (
            vec![aye_adapter_doctest::NAME, aye_adapter_pytest::NAME],
            DEFAULT_ADAPTERS,
        ),
    };
    let mut known = known(listing);
    let all = known
        .iter()
        .map(|a| a.name())
        .collect::<Vec<_>>()
        .join(", ");
    // An `AdapterList` names each adapter once.
    let mut adapters: Vec<Box<dyn Adapter>> = Vec::new();
    for name in &names {
        let Some(at) = known.iter().position(|a| a.name() == *name) else {
            bail!(
                "[tool.aye] adapters: unknown adapter {:?} (known: {all})",
                name.as_str()
            );
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
    Ok(Chosen { adapters, source })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use aye_adapter::ast::Stmt;
    use aye_adapter::ast::visitor::{self, Visitor};
    use aye_config::{AdapterList, AyeSettings, Sourced};
    use aye_protocol::{Request, Runtime, SessionMode};
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
            adapters: chosen.map(|names| {
                Sourced::default_value(
                    AdapterList::new(names.iter().map(ToString::to_string).collect()).unwrap(),
                )
            }),
            ..AyeSettings::default()
        };
        choose(&project(dir.path(), &[], &settings), Listing::Refine).map(|c| c.names())
    }

    /// The table names each adapter as the adapter names itself, in
    /// priority order, whichever the listing.
    #[test]
    fn the_known_adapters_are_pytest_unittest_and_doctest_in_order() {
        let named: Vec<AdapterName> = KNOWN.iter().map(|known| known.name.clone()).collect();
        assert_eq!(named, ["pytest", "unittest", "doctest"]);
        for listing in [Listing::Refine, Listing::Static] {
            let adapters: Vec<AdapterName> = known(listing).iter().map(|a| a.name()).collect();
            assert_eq!(adapters, named, "{listing:?}");
        }
    }

    /// What each adapter declares is what the binary asked of its name: the
    /// pytest and unittest adapters are collected in one `Module`, the
    /// pytest adapter refines every file, and a support file is a path
    /// whose last part is `conftest.py`, whoever reports it.
    #[test]
    fn each_adapter_declares_what_the_binary_knows_of_it() {
        for name in ["pytest", "unittest", "doctest", "nose"] {
            let declared = traits(&AdapterName::from(name));
            assert_eq!(
                declared.one_module,
                matches!(name, "pytest" | "unittest"),
                "{name}"
            );
            assert_eq!(declared.refines_every_file, name == "pytest", "{name}");
        }
        for (path, file) in [
            ("conftest.py", Some("conftest.py")),
            ("tests/conftest.py", Some("conftest.py")),
            ("a/b/conftest.py", Some("conftest.py")),
            ("tests/conftest.pyc", None),
            ("tests/my_conftest.py", None),
            ("conftest.py/test_x.py", None),
            ("", None),
        ] {
            assert_eq!(support_file(path), file, "{path}");
        }
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
            let chosen = choose(&project(root, args, &settings), Listing::Refine).unwrap();
            (chosen.names(), chosen.source)
        };
        let default = (
            vec![aye_adapter_doctest::NAME, aye_adapter_pytest::NAME],
            DEFAULT_ADAPTERS,
        );
        assert_eq!(chosen(&[]), default);
        assert_eq!(chosen(&[PathBuf::from("tests")]), default);
        write(root, "tests/conftest.py", "");
        write(root, "pytest.ini", "[pytest]\n");
        assert_eq!(chosen(&[]), default);
        assert_eq!(
            DEFAULT_ADAPTERS.display_for("adapters").to_string(),
            "default: pytest's rules, which run unittest's TestCase classes too"
        );
    }

    /// Every adapter's executor runs on aye's engine without pytest: its
    /// workers install aye's `pytest` module and what it stands on first,
    /// and its own modules are none of those. The pytest adapter's lists
    /// the session, and a static listing changes only what its recognizer
    /// predicts.
    #[test]
    fn every_adapter_runs_on_ayes_engine_without_pytest() {
        assert_eq!(
            PRELUDE.names().collect::<Vec<_>>(),
            ["aye_model", "aye_rewrite", "aye_native_engine", "pytest"]
        );
        let adapters = known(Listing::Refine);
        let pytest = adapters[0].runtime();
        assert_eq!(pytest.entry.to_string(), "aye_pytest_native:NativeExecutor");
        assert_eq!(pytest.session, SessionMode::ListsSession);
        assert_eq!(known(Listing::Static)[0].runtime(), pytest);
        for adapter in &adapters {
            let runtime = adapter.runtime();
            assert_eq!(runtime.adapter, adapter.name());
            for name in runtime.modules.names() {
                assert!(
                    !aye_engine::ENGINE
                        .names()
                        .chain(PRELUDE.names())
                        .any(|first| first == name),
                    "{}: {name}",
                    runtime.adapter
                );
            }
        }
        for adapter in &adapters[1..] {
            assert_eq!(adapter.runtime().session, SessionMode::PerBatch);
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
    /// adapters, in the order the run lists them), as the runner sends it.
    fn handshake(adapters: &[usize]) -> Request {
        let known = known(Listing::Refine);
        let mut spec = aye_runner::WorkerSpec::new("python", "/repo", "/repo");
        spec.prelude = PRELUDE;
        spec.runtimes = adapters.iter().map(|&at| known[at].runtime()).collect();
        spec.hello()
    }

    /// Each adapter alone, the default run's two, and all three in one
    /// worker.
    const WORKERS: [&[usize]; 5] = [&[0], &[1], &[2], &[2, 0], &[0, 1, 2]];

    /// [`handshake`], with each module's source, once checked against its
    /// file, written as `<name>`.
    fn hello(adapters: &[usize]) -> String {
        let hello = handshake(adapters);
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
        assert_eq!(WORKERS, [&[0][..], &[1], &[2], &[2, 0], &[0, 1, 2]]);
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

    /// The standard library's modules that the Python a worker installs
    /// imports as it is installed. Each must be the standard library's:
    /// what a worker installs imports nothing else but aye's modules
    /// installed before it.
    const STANDARD_LIBRARY: [&str; 54] = [
        "__future__",
        "abc",
        "argparse",
        "ast",
        "atexit",
        "bisect",
        "builtins",
        "collections",
        "configparser",
        "contextlib",
        "dataclasses",
        "datetime",
        "decimal",
        "difflib",
        "doctest",
        "enum",
        "fnmatch",
        "functools",
        "gc",
        "getpass",
        "glob",
        "hashlib",
        "importlib",
        "inspect",
        "io",
        "itertools",
        "json",
        "linecache",
        "logging",
        "marshal",
        "math",
        "nt",
        "numbers",
        "os",
        "pathlib",
        "platform",
        "pprint",
        "re",
        "reprlib",
        "shlex",
        "shutil",
        "stat",
        "sys",
        "tempfile",
        "threading",
        "time",
        "tomllib",
        "traceback",
        "types",
        "typing",
        "unicodedata",
        "unittest",
        "uuid",
        "warnings",
    ];

    /// The modules a module imports as it is installed: in its top-level
    /// statements and its classes' bodies, not in its functions'.
    #[derive(Default)]
    struct Imports(Vec<String>);

    impl<'a> Visitor<'a> for Imports {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(_) => {}
                Stmt::Import(import) => {
                    self.0
                        .extend(import.names.iter().map(|alias| alias.name.to_string()));
                }
                Stmt::ImportFrom(import) => {
                    let module = import.module.as_ref().map_or("", |m| m.as_str());
                    self.0
                        .push(format!("{}{module}", ".".repeat(import.level as usize)));
                }
                _ => visitor::walk_stmt(self, stmt),
            }
        }
    }

    /// Each module a worker installs imports, as it is installed, only the
    /// standard library and the modules installed before it (of aye's
    /// engine, the prelude, or the adapters' sets), for every worker the
    /// adapters can have: a module installed too early fails the
    /// handshake.
    #[test]
    fn every_module_a_worker_installs_imports_only_those_before_it() {
        for adapters in WORKERS {
            let Request::Hello { modules, .. } = handshake(adapters) else {
                panic!("expected hello");
            };
            let mut installed: Vec<&str> = Vec::new();
            for module in &modules {
                let parsed = ruff_python_parser::parse_module(&module.source)
                    .unwrap_or_else(|e| panic!("{}: {e}", module.name))
                    .into_syntax();
                let mut imports = Imports::default();
                imports.visit_body(&parsed.body);
                for import in &imports.0 {
                    let top = import.split('.').next().unwrap_or_default();
                    assert!(
                        installed.contains(&top) || STANDARD_LIBRARY.contains(&top),
                        "{adapters:?}: {} imports {import}, which is neither installed \
                         before it nor the standard library's",
                        module.name
                    );
                }
                installed.push(&module.name);
            }
        }
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
            aye_config::AyeSettings::load(root).unwrap()
        };
        for table in [
            "[tool.aye.unittest]\nfoo = 1\n",
            "[tool.aye.unittest]\nstart-directory = \"missing\"\n",
            "[tool.aye.doctest]\nfoo = 1\n",
            "[tool.aye.doctest]\nmodules = \"yes\"\n",
        ] {
            let err = choose(&project(root, &[], &settings(table)), Listing::Refine)
                .err()
                .unwrap_or_else(|| panic!("{table}: accepted"));
            assert!(
                format!("{err:#}").contains("configuration"),
                "{table}: {err:#}"
            );
        }
        let good = settings("[tool.aye.unittest]\npattern = \"*_test.py\"\n");
        assert_eq!(
            choose(&project(root, &[], &good), Listing::Refine)
                .unwrap()
                .names(),
            ["pytest"]
        );
    }

    /// The Python every worker runs names environment variables itself (it
    /// imports only the standard library): each one it reads or sets by
    /// name is in aye's catalogue.
    #[test]
    fn the_python_sent_to_workers_names_only_catalogued_variables() {
        let mut sources = vec![("aye_worker", aye_runner::WORKER_SOURCE)];
        let runtimes: Vec<Runtime> = known(Listing::Refine).iter().map(|a| a.runtime()).collect();
        let modules = aye_engine::ENGINE
            .iter()
            .chain(PRELUDE)
            .chain(runtimes.iter().flat_map(|runtime| runtime.modules));
        sources.extend(modules.map(|m| (&*m.name, &*m.source)));
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
