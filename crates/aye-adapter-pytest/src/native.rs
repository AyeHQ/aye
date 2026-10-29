//! The pytest adapter on aye's own engine: pytest suites run without
//! pytest, which is never imported (the default since October 2026; `aye
//! --native`, the prototype's switch, is accepted and does nothing).
//!
//! Parsing is decoupled from execution. In the worker, a front-end
//! (`aye_pytest_frontend`) reads the suite's code, through aye's own
//! `pytest` module (`aye_pytest_shim.py`, installed as `pytest` before any
//! test or conftest is imported), into aye's model (`aye_model`); aye's
//! engine (`aye_native_engine`, with `aye_rewrite` rewriting asserts) runs
//! the model and knows no framework; `aye_pytest_native` is the glue, the
//! executor the worker instantiates. It [lists the
//! session](aye_protocol::Executor::lists_session): the worker that refined
//! runs batches too, and every other worker lists the session's files
//! before its first batch.
//!
//! Rust half: [`Pytest`], the pytest adapter with that executor. Its
//! recognizer ([`NativeRecognizer`]) is the static one's for what is a test
//! file and where to look; but:
//! - **Every claimed test file is refined**, with no static case: the
//!   front-end is the single truth for ids (the static fast path comes back
//!   later). With `aye --co --no-refine` ([`Pytest::static_listing`]), the
//!   static recognizer's prediction is listed instead, without importing
//!   anything: exact where it can be, `Partial` where the front-end would
//!   decide. A run has the front-end list every file it runs.
//! - **The context gains `native`** ([`NativeRecognizer::context`]): the
//!   resolved settings the front-end needs, the raw ini values for
//!   `getini`, and pytest's options parsed as pytest parses its own.
//! - **What it can't run fails loudly**: the files pytest's own doctest
//!   plugin would collect (a run without the doctest adapter) get a
//!   `usage-error` naming the fix, rather than going missing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aye_adapter::{
    Adapter, AdapterError, Claim, FrameworkDefaults, Project, Recognition, Recognizer, Requested,
    Setting, SourceFile,
};
use aye_engine::{MODEL_MODULE, NATIVE_ENGINE_MODULE, REWRITE_MODULE};
use aye_events::{AdapterName, Diagnostic, DiagnosticCode, Fnv64, Severity};
use aye_protocol::{AdapterContext, Executor, ModuleSet, ModuleSource};
use serde::Serialize;
use serde_json::{Value, json};

use crate::context::PytestContext;
use crate::{PytestConfig, PytestRecognizer, config::ImportMode};

/// aye's own `pytest` module: the authoring API (marks, fixtures,
/// `raises`, `approx`, outcomes) the front-end reads, installed as
/// `pytest`.
pub const SHIM_MODULE: ModuleSource = ModuleSource::embedded(
    "pytest",
    include_str!("../python/native/aye_pytest_shim.py"),
);

/// The pytest front-end: reads a suite into aye's model, never runs a test.
pub const FRONTEND_MODULE: ModuleSource = ModuleSource::embedded(
    "aye_pytest_frontend",
    include_str!("../python/native/aye_pytest_frontend.py"),
);

/// The glue: the executor (`refine`, `run`) over the front-end and the
/// engine.
pub const NATIVE_MODULE: ModuleSource = ModuleSource::embedded(
    "aye_pytest_native",
    include_str!("../python/native/aye_pytest_native.py"),
);

/// The native executor's modules, in install order (after `aye_engine`):
/// the model, the rewriter and the engine first, then aye's `pytest`, so
/// it is in `sys.modules` before the front-end and any test import it.
pub const MODULES: ModuleSet = ModuleSet::new(
    const {
        &[
            MODEL_MODULE,
            REWRITE_MODULE,
            NATIVE_ENGINE_MODULE,
            SHIM_MODULE,
            FRONTEND_MODULE,
            NATIVE_MODULE,
        ]
    },
);

/// `module:Class` of the native executor.
pub const ENTRY: &str = "aye_pytest_native:NativeExecutor";

/// Why every test file is refined.
const REFINE_REASON: &str = "aye's pytest front-end reads every test file";

/// The adapter's name.
pub const NAME: AdapterName = AdapterName::new_static("pytest");

/// The doctest adapter's name: with it in the run, the files pytest's
/// doctest plugin would collect are its.
const DOCTEST: AdapterName = AdapterName::new_static("doctest");

/// The pytest adapter: pytest's rules, read by aye's front-end, run by
/// aye's engine, with no pytest installed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pytest {
    /// Whether the recognizer gives the static prediction rather than
    /// refining every file ([`Self::static_listing`]).
    static_listing: bool,
}

impl Pytest {
    /// The adapter for `aye --co --no-refine`: the recognizer predicts each
    /// file's cases statically, as far as it can, and leaves the rest to
    /// refinement, which is off, so that collection lists the static guess
    /// without importing anything.
    #[must_use]
    pub const fn static_listing() -> Self {
        Self {
            static_listing: true,
        }
    }
}

impl Adapter for Pytest {
    fn name(&self) -> AdapterName {
        NAME
    }

    /// pytest is the default for Python tests: a recognizer is always
    /// returned, configured or not. The doctest adapter runs doctests.
    fn prepare(&self, project: &Project<'_>) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        Ok(Some(Box::new(NativeRecognizer::new(
            PytestRecognizer::new(project)?,
            self.static_listing,
        ))))
    }

    /// [`Pytest::prepare`]; without the doctest adapter, the files pytest's
    /// doctest plugin would collect are claimed, and refused
    /// ([`NativeRecognizer::recognize`]).
    fn prepare_among(
        &self,
        project: &Project<'_>,
        adapters: &[AdapterName],
    ) -> Result<Option<Box<dyn Recognizer>>, AdapterError> {
        let recognizer = PytestRecognizer::new(project)?;
        let recognizer = if adapters.contains(&DOCTEST) {
            recognizer
        } else {
            recognizer.with_pytest_doctests()
        };
        Ok(Some(Box::new(NativeRecognizer::new(
            recognizer,
            self.static_listing,
        ))))
    }

    fn executor(&self) -> Executor {
        Executor {
            adapter: NAME,
            modules: MODULES,
            entry: ENTRY,
            lists_session: true,
        }
    }
}

/// The pytest recognizer, with every test file refined (or, for a static
/// listing, predicted) and the `native` context.
#[derive(Debug)]
pub struct NativeRecognizer {
    inner: PytestRecognizer,
    /// Whether [`Recognizer::recognize`] gives the static recognizer's
    /// prediction ([`Pytest::static_listing`]).
    static_listing: bool,
    /// The context, with its `native` section, built once.
    context: OnceLock<PytestContext>,
}

impl NativeRecognizer {
    pub fn new(inner: PytestRecognizer, static_listing: bool) -> Self {
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

// ---------------------------------------------------------------- the context

/// The `native` section of the pytest adapter's context: what the
/// front-end starts from.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct NativeContext {
    /// pytest's rootdir, absolute.
    rootdir: String,
    /// pytest's inifile, absolute; `null` without one.
    inifile: Option<String>,
    /// pytest's confcutdir, absolute.
    confcutdir: String,
    /// `pythonpath`, absolute, in order.
    pythonpath: Vec<String>,
    /// From `python_files` to `markers`, and `filterwarnings`: lists of
    /// strings, pytest's defaults when unset.
    python_files: Vec<String>,
    python_classes: Vec<String>,
    python_functions: Vec<String>,
    testpaths: Vec<String>,
    norecursedirs: Vec<String>,
    usefixtures: Vec<String>,
    markers: Vec<String>,
    filterwarnings: Vec<String>,
    /// `strict_xfail`, its alias `xfail_strict`, else `strict`.
    xfail_strict: bool,
    /// `strict_markers`, else `strict`; both also from `--strict-markers`
    /// and `--strict`.
    strict_markers: bool,
    /// From `strict_parametrization_ids` to `noconftest`: resolved.
    strict_parametrization_ids: bool,
    disable_test_id_escaping: bool,
    collect_imported_tests: bool,
    consider_namespace_packages: bool,
    noconftest: bool,
    /// `prepend`, `append` or `importlib`.
    import_mode: &'static str,
    /// `""` for pytest's default.
    empty_parameter_set_mark: String,
    /// The `-p` values in order (`no:NAME` included).
    plugins: Vec<String>,
    /// The modules `PYTEST_PLUGINS` names.
    env_plugins: Vec<String>,
    /// Every raw value of the configuration, by name, after `-o` and the
    /// `--strict…` flags, for `getini` to coerce by the option's type: a
    /// string from an INI file, `[tool.pytest.ini_options]` scalars or
    /// `-o`; a list of strings for an array of `[tool.pytest.ini_options]`;
    /// the TOML value itself (string, bool, number, list) from
    /// `[tool.pytest]` or `pytest.toml`.
    ini: BTreeMap<String, Value>,
    /// pytest's arguments, as the front-end parses them
    /// ([`PytestConfig::args`]: `addopts`, `PYTEST_ADDOPTS`, then those
    /// after `--`; without pytest-xdist's, `-x`/`--maxfail`, the JUnit and,
    /// when the doctest adapter runs, the doctest options, which the runner
    /// and the doctest adapter handle), for options conftests add.
    args: Vec<String>,
    /// [`Self::args`] parsed as pytest 9.1.1 parses its own options,
    /// `{dest: value}` for every built-in option (pytest's dest names and
    /// defaults: `verbose`, `keyword`, `markexpr`, `plugins`, `runxfail`,
    /// `basetemp`, `capture`, `pythonwarnings`, `override_ini`, …),
    /// `maxfail`, `xmlpath` and `junitprefix` from the options the runner
    /// lifted, and `file_or_dir` for positional arguments.
    options: BTreeMap<String, Value>,
    /// The options [`Self::options`] doesn't know (a plugin's, a
    /// conftest's), as given.
    unknown_options: Vec<String>,
    /// A digest of the executor's sources, so that the refinement cache
    /// never answers for other sources.
    modules_digest: String,
}

/// The `native` section for `config`.
fn native_context(config: &PytestConfig) -> NativeContext {
    let args = config.args.clone();
    let mut ini = read_ini(config.inifile.as_deref());
    apply_overrides(&mut ini, &args);
    let mut parsed = parse_options(&args);
    parsed
        .values
        .insert("maxfail".into(), json!(config.maxfail.map_or(0, u64::from)));
    parsed.values.insert(
        "xmlpath".into(),
        config
            .junitxml
            .as_ref()
            .map_or(Value::Null, |p| json!(p.to_string_lossy())),
    );
    parsed
        .values
        .insert("junitprefix".into(), json!(config.junit_prefix));
    let flag = |names: &[&str]| -> Option<bool> {
        names
            .iter()
            .find_map(|name| ini.get(*name))
            .and_then(ini_bool)
    };
    let strict = flag(&["strict"]).unwrap_or(false);
    let path = |p: &Path| p.to_string_lossy().into_owned();
    NativeContext {
        rootdir: path(&config.rootdir),
        inifile: config.inifile.as_deref().map(path),
        confcutdir: path(&config.confcutdir),
        pythonpath: config.pythonpath.iter().map(|p| path(p)).collect(),
        python_files: config.python_files.value.clone(),
        python_classes: config.python_classes.value.clone(),
        python_functions: config.python_functions.value.clone(),
        testpaths: config.testpaths.value.clone(),
        norecursedirs: config.norecursedirs.value.clone(),
        usefixtures: config.usefixtures.value.clone(),
        markers: config.markers.value.clone(),
        filterwarnings: ini.get("filterwarnings").map(ini_lines).unwrap_or_default(),
        xfail_strict: flag(&["strict_xfail", "xfail_strict"]).unwrap_or(strict),
        strict_markers: flag(&["strict_markers"]).unwrap_or(strict),
        strict_parametrization_ids: config.strict_parametrization_ids.value,
        disable_test_id_escaping: config.disable_test_id_escaping.value,
        collect_imported_tests: config.collect_imported_tests.value,
        consider_namespace_packages: config.consider_namespace_packages,
        noconftest: config.noconftest,
        import_mode: match config.import_mode {
            ImportMode::Prepend => "prepend",
            ImportMode::Append => "append",
            ImportMode::Importlib => "importlib",
        },
        empty_parameter_set_mark: config.empty_parameter_set_mark.value.clone(),
        plugins: config.plugins.clone(),
        env_plugins: config.env_plugins.clone(),
        ini,
        args,
        options: parsed.values,
        unknown_options: parsed.unknown,
        modules_digest: modules_digest(),
    }
}

/// A 64-bit FNV-1a digest of the executor's module names and sources, in
/// hex.
fn modules_digest() -> String {
    let mut hash = Fnv64::new();
    for module in MODULES {
        for text in [&module.name, &module.source] {
            hash.write(text.as_bytes());
            hash.write(&[0]);
        }
    }
    format!("{:016x}", hash.finish())
}

/// Python's `str()` of a TOML scalar, as `[tool.pytest.ini_options]`
/// stringifies values.
fn python_str(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        toml::Value::Float(f) => crate::ids::float_repr(*f),
        other => other.to_string(),
    }
}

/// A native TOML value as JSON (dates as their text).
fn toml_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_json).collect()),
        toml::Value::Table(table) => Value::Object(
            table
                .iter()
                .map(|(k, v)| (k.clone(), toml_json(v)))
                .collect(),
        ),
    }
}

/// The raw values of pytest's configuration in `inifile` (see
/// [`NativeRecognizer::context`]'s `ini`). The file was read once already,
/// with its errors reported; here an unreadable one gives nothing.
fn read_ini(inifile: Option<&Path>) -> BTreeMap<String, Value> {
    let Some(path) = inifile else {
        return BTreeMap::new();
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let texts = |section: Option<aye_config::ini::Section>| -> BTreeMap<String, Value> {
        section
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, json!(v)))
            .collect()
    };
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "ini" => texts(aye_config::ini_section(path, "pytest").ok().flatten()),
        "cfg" => texts(aye_config::ini_section(path, "tool:pytest").ok().flatten()),
        "toml" => {
            let Ok(doc) = aye_config::read_toml(path) else {
                return BTreeMap::new();
            };
            let native = |table: &toml::Table| -> BTreeMap<String, Value> {
                table
                    .iter()
                    .map(|(k, v)| (k.clone(), toml_json(v)))
                    .collect()
            };
            if name == "pytest.toml" || name == ".pytest.toml" {
                return doc
                    .get("pytest")
                    .and_then(toml::Value::as_table)
                    .map(native)
                    .unwrap_or_default();
            }
            let Some(tool) = doc
                .get("tool")
                .and_then(|t| t.get("pytest"))
                .and_then(toml::Value::as_table)
            else {
                return BTreeMap::new();
            };
            let mut table = tool.clone();
            match table.remove("ini_options") {
                Some(toml::Value::Table(ini)) if table.is_empty() => ini
                    .iter()
                    .map(|(k, v)| {
                        let value = match v {
                            toml::Value::Array(items) => {
                                json!(items.iter().map(python_str).collect::<Vec<_>>())
                            }
                            other => json!(python_str(other)),
                        };
                        (k.clone(), value)
                    })
                    .collect(),
                _ => native(&table),
            }
        }
        _ => BTreeMap::new(),
    }
}

/// `-o name=value` and the `--strict…` flags (pytest's
/// `OverrideIniAction`), in the order pytest reads `args`: the last one
/// wins.
fn apply_overrides(ini: &mut BTreeMap<String, Value>, args: &[String]) {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            break;
        }
        let value = match arg.as_str() {
            "--strict-config" => Some("strict_config=true".to_string()),
            "--strict-markers" => Some("strict_markers=true".to_string()),
            "--strict" => Some("strict=true".to_string()),
            "-o" | "--override-ini" => iter.next().cloned(),
            _ => arg
                .strip_prefix("--override-ini=")
                .or_else(|| arg.strip_prefix("-o"))
                .map(str::to_string),
        };
        if let Some((name, value)) = value.as_deref().and_then(|v| v.split_once('=')) {
            ini.insert(name.trim().to_string(), json!(value));
        }
    }
}

/// pytest's `_strtobool`, or a native bool.
fn ini_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" | "t" | "true" | "on" | "1" => Some(true),
            "n" | "no" | "f" | "false" | "off" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// A `linelist` value: one entry per non-empty line of a string, or a
/// list's strings.
fn ini_lines(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => text
            .split('\n')
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- options

/// What an option does with what it is given.
#[derive(Debug, Clone, Copy)]
enum Action {
    /// `store_true`.
    True,
    /// `store_false`.
    False,
    /// `store_const`: the value, as JSON text.
    Const(&'static str),
    /// `count`, adding this.
    Count(i64),
    /// `store` of a string.
    Text,
    /// `store` of an int.
    Int,
    /// `store` of a float.
    Float,
    /// `append` of a string.
    Append,
    /// `nargs="?"`: the value when attached (`--debug=FILE`), else the
    /// constant.
    Optional(&'static str),
}

/// One of pytest's built-in options.
struct Opt {
    names: &'static [&'static str],
    dest: &'static str,
    action: Action,
    /// Its default, as JSON text.
    default: &'static str,
}

const fn opt(
    names: &'static [&'static str],
    dest: &'static str,
    action: Action,
    default: &'static str,
) -> Opt {
    Opt {
        names,
        dest,
        action,
        default,
    }
}

/// pytest 9.1.1's built-in options (`_pytest/*.py`'s `addoption`), with
/// their dests and defaults. `--strict…` override ini values too
/// ([`apply_overrides`]).
const OPTIONS: &[Opt] = &[
    opt(&["-k"], "keyword", Action::Text, r#""""#),
    opt(&["-m"], "markexpr", Action::Text, r#""""#),
    opt(&["--markers"], "markers", Action::True, "false"),
    opt(&["-x", "--exitfirst"], "maxfail", Action::Const("1"), "0"),
    opt(&["--maxfail"], "maxfail", Action::Int, "0"),
    opt(&["--strict-config"], "strict_config", Action::True, "null"),
    opt(
        &["--strict-markers"],
        "strict_markers",
        Action::True,
        "null",
    ),
    opt(&["--strict"], "strict", Action::True, "null"),
    opt(
        &["--fixtures", "--funcargs"],
        "showfixtures",
        Action::True,
        "false",
    ),
    opt(
        &["--fixtures-per-test"],
        "show_fixtures_per_test",
        Action::True,
        "false",
    ),
    opt(&["--pdb"], "usepdb", Action::True, "false"),
    opt(&["--pdbcls"], "usepdb_cls", Action::Text, "null"),
    opt(&["--trace"], "trace", Action::True, "false"),
    opt(&["--capture"], "capture", Action::Text, r#""fd""#),
    opt(&["-s"], "capture", Action::Const(r#""no""#), r#""fd""#),
    opt(&["--runxfail"], "runxfail", Action::True, "false"),
    opt(&["--lf", "--last-failed"], "lf", Action::True, "false"),
    opt(
        &["--ff", "--failed-first"],
        "failedfirst",
        Action::True,
        "false",
    ),
    opt(&["--nf", "--new-first"], "newfirst", Action::True, "false"),
    opt(
        &["--cache-show"],
        "cacheshow",
        Action::Optional(r#""*""#),
        "null",
    ),
    opt(&["--cache-clear"], "cacheclear", Action::True, "false"),
    opt(
        &["--lfnf", "--last-failed-no-failures"],
        "last_failed_no_failures",
        Action::Text,
        r#""all""#,
    ),
    opt(&["--sw", "--stepwise"], "stepwise", Action::True, "false"),
    opt(
        &["--sw-skip", "--stepwise-skip"],
        "stepwise_skip",
        Action::True,
        "false",
    ),
    opt(
        &["--sw-reset", "--stepwise-reset"],
        "stepwise_reset",
        Action::True,
        "false",
    ),
    opt(&["--durations"], "durations", Action::Int, "null"),
    opt(&["--durations-min"], "durations_min", Action::Float, "null"),
    opt(&["-v", "--verbose"], "verbose", Action::Count(1), "0"),
    opt(&["-q", "--quiet"], "verbose", Action::Count(-1), "0"),
    opt(&["--verbosity"], "verbose", Action::Int, "0"),
    opt(&["--no-header"], "no_header", Action::True, "false"),
    opt(&["--no-summary"], "no_summary", Action::True, "false"),
    opt(
        &["--no-fold-skipped"],
        "fold_skipped",
        Action::False,
        "true",
    ),
    opt(
        &["--force-short-summary"],
        "force_short_summary",
        Action::True,
        "false",
    ),
    opt(
        &["-r", "--report-chars"],
        "reportchars",
        Action::Text,
        r#""fE""#,
    ),
    opt(
        &["--disable-warnings", "--disable-pytest-warnings"],
        "disable_warnings",
        Action::True,
        "false",
    ),
    opt(&["-l", "--showlocals"], "showlocals", Action::True, "false"),
    opt(&["--no-showlocals"], "showlocals", Action::False, "false"),
    opt(&["--tb"], "tbstyle", Action::Text, r#""auto""#),
    opt(&["--xfail-tb"], "xfail_tb", Action::True, "false"),
    opt(&["--show-capture"], "showcapture", Action::Text, r#""all""#),
    opt(
        &["--fulltrace", "--full-trace"],
        "fulltrace",
        Action::True,
        "false",
    ),
    opt(&["--color"], "color", Action::Text, r#""auto""#),
    opt(
        &["--code-highlight"],
        "code_highlight",
        Action::Text,
        r#""yes""#,
    ),
    opt(&["--pastebin"], "pastebin", Action::Text, "null"),
    opt(
        &["--junitxml", "--junit-xml"],
        "xmlpath",
        Action::Text,
        "null",
    ),
    opt(
        &["--junitprefix", "--junit-prefix"],
        "junitprefix",
        Action::Text,
        "null",
    ),
    opt(
        &["-W", "--pythonwarnings"],
        "pythonwarnings",
        Action::Append,
        "null",
    ),
    opt(&["--max-warnings"], "max_warnings", Action::Int, "null"),
    opt(
        &["--collectonly", "--collect-only", "--co"],
        "collectonly",
        Action::True,
        "false",
    ),
    opt(&["--pyargs"], "pyargs", Action::True, "false"),
    opt(&["--ignore"], "ignore", Action::Append, "null"),
    opt(&["--ignore-glob"], "ignore_glob", Action::Append, "null"),
    opt(&["--deselect"], "deselect", Action::Append, "null"),
    opt(&["--confcutdir"], "confcutdir", Action::Text, "null"),
    opt(&["--noconftest"], "noconftest", Action::True, "false"),
    opt(
        &["--keepduplicates", "--keep-duplicates"],
        "keepduplicates",
        Action::True,
        "false",
    ),
    opt(
        &["--collect-in-virtualenv"],
        "collect_in_virtualenv",
        Action::True,
        "false",
    ),
    opt(
        &["--continue-on-collection-errors"],
        "continue_on_collection_errors",
        Action::True,
        "false",
    ),
    opt(
        &["--import-mode"],
        "importmode",
        Action::Text,
        r#""prepend""#,
    ),
    opt(
        &["--doctest-modules"],
        "doctestmodules",
        Action::True,
        "false",
    ),
    opt(
        &["--doctest-report"],
        "doctestreport",
        Action::Text,
        r#""udiff""#,
    ),
    opt(&["--doctest-glob"], "doctestglob", Action::Append, "[]"),
    opt(
        &["--doctest-ignore-import-errors"],
        "doctest_ignore_import_errors",
        Action::True,
        "false",
    ),
    opt(
        &["--doctest-continue-on-failure"],
        "doctest_continue_on_failure",
        Action::True,
        "false",
    ),
    opt(
        &["-c", "--config-file"],
        "inifilename",
        Action::Text,
        "null",
    ),
    opt(&["--rootdir"], "rootdir", Action::Text, "null"),
    opt(&["--basetemp"], "basetemp", Action::Text, "null"),
    opt(&["-V", "--version"], "version", Action::Count(1), "0"),
    opt(&["-h", "--help"], "help", Action::True, "false"),
    opt(&["-p"], "plugins", Action::Append, "[]"),
    opt(
        &["--disable-plugin-autoload"],
        "disable_plugin_autoload",
        Action::True,
        "false",
    ),
    opt(
        &["--traceconfig", "--trace-config"],
        "traceconfig",
        Action::True,
        "false",
    ),
    opt(
        &["--debug"],
        "debug",
        Action::Optional(r#""pytestdebug.log""#),
        "null",
    ),
    opt(
        &["-o", "--override-ini"],
        "override_ini",
        Action::Append,
        "null",
    ),
    opt(&["--assert"], "assertmode", Action::Text, r#""rewrite""#),
    opt(
        &["--setuponly", "--setup-only"],
        "setuponly",
        Action::True,
        "false",
    ),
    opt(
        &["--setupshow", "--setup-show"],
        "setupshow",
        Action::True,
        "false",
    ),
    opt(
        &["--setupplan", "--setup-plan"],
        "setupplan",
        Action::True,
        "false",
    ),
    opt(&["--log-level"], "log_level", Action::Text, "null"),
    opt(&["--log-format"], "log_format", Action::Text, "null"),
    opt(
        &["--log-date-format"],
        "log_date_format",
        Action::Text,
        "null",
    ),
    opt(&["--log-cli-level"], "log_cli_level", Action::Text, "null"),
    opt(
        &["--log-cli-format"],
        "log_cli_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-cli-date-format"],
        "log_cli_date_format",
        Action::Text,
        "null",
    ),
    opt(&["--log-file"], "log_file", Action::Text, "null"),
    opt(&["--log-file-mode"], "log_file_mode", Action::Text, "null"),
    opt(
        &["--log-file-level"],
        "log_file_level",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-file-format"],
        "log_file_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-file-date-format"],
        "log_file_date_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-auto-indent"],
        "log_auto_indent",
        Action::Text,
        "null",
    ),
    opt(&["--log-disable"], "logger_disable", Action::Append, "[]"),
];

/// pytest's arguments, parsed.
#[derive(Debug, Default)]
struct Parsed {
    /// Every built-in option's dest, with its value (its default when not
    /// given), and `file_or_dir`.
    values: BTreeMap<String, Value>,
    /// The options no built-in one matches, as given.
    unknown: Vec<String>,
}

fn find_option(name: &str) -> Option<&'static Opt> {
    OPTIONS.iter().find(|o| o.names.contains(&name))
}

/// Whether `action` takes a value.
const fn takes_value(action: Action) -> bool {
    matches!(
        action,
        Action::Text | Action::Int | Action::Float | Action::Append
    )
}

/// argparse's test for a negative number (`-1`, `-1.5`, `-.5`), which is
/// a value, not an option.
fn is_negative_number(arg: &str) -> bool {
    arg.strip_prefix('-').is_some_and(|rest| {
        !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
            && rest.chars().filter(|c| *c == '.').count() <= 1
            && rest != "."
    })
}

/// Apply one option with its value (`None` when it takes none).
fn apply(values: &mut BTreeMap<String, Value>, option: &Opt, value: Option<&str>) {
    let dest = option.dest.to_string();
    let parsed = match option.action {
        Action::True => json!(true),
        Action::False => json!(false),
        Action::Const(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        Action::Count(step) => {
            let now = values.get(&dest).and_then(Value::as_i64).unwrap_or(0);
            json!(now + step)
        }
        Action::Text => json!(value.unwrap_or_default()),
        Action::Int => value
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map_or(Value::Null, |n| json!(n)),
        Action::Float => value
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map_or(Value::Null, |n| json!(n)),
        Action::Append => {
            let mut list = values
                .get(&dest)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(json!(value.unwrap_or_default()));
            Value::Array(list)
        }
        Action::Optional(constant) => value.map_or_else(
            || serde_json::from_str(constant).unwrap_or(Value::Null),
            |v| json!(v),
        ),
    };
    values.insert(dest, parsed);
}

/// `args` parsed as pytest's argparse parses its built-in options: long
/// options whole (`--name value`, `--name=value`; pytest allows no
/// abbreviation), short ones alone or in clusters (`-vv`, `-ra`, `-kexpr`,
/// `-vk expr`), a lone `--` ending the options. What it doesn't know is
/// left in `unknown`, and doesn't consume the next argument.
fn parse_options(args: &[String]) -> Parsed {
    let mut parsed = Parsed::default();
    for option in OPTIONS {
        parsed
            .values
            .entry(option.dest.to_string())
            .or_insert_with(|| serde_json::from_str(option.default).unwrap_or(Value::Null));
    }
    let mut positional: Vec<Value> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            positional.extend(iter.by_ref().map(|a| json!(a)));
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            let Some(option) = find_option(&name) else {
                parsed.unknown.push(arg.clone());
                continue;
            };
            let value = if takes_value(option.action) {
                attached.or_else(|| iter.next().cloned())
            } else {
                attached.filter(|_| matches!(option.action, Action::Optional(_)))
            };
            apply(&mut parsed.values, option, value.as_deref());
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') && !is_negative_number(arg) {
            let cluster = &arg[1..];
            for (at, flag) in cluster.char_indices() {
                let Some(option) = find_option(&format!("-{flag}")) else {
                    parsed.unknown.push(arg.clone());
                    break;
                };
                if takes_value(option.action) {
                    let rest = &cluster[at + flag.len_utf8()..];
                    let value = if rest.is_empty() {
                        iter.next().cloned()
                    } else {
                        Some(rest.to_string())
                    };
                    apply(&mut parsed.values, option, value.as_deref());
                    break;
                }
                apply(&mut parsed.values, option, None);
            }
            continue;
        }
        positional.push(json!(arg));
    }
    parsed
        .values
        .insert("file_or_dir".into(), Value::Array(positional));
    parsed
}

#[cfg(test)]
mod tests {
    use aye_adapter::RefineReason;

    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn recognizer(root: &Path, framework_args: &[String]) -> NativeRecognizer {
        let project = Project {
            framework_args,
            ..Project::new(root, root)
        };
        NativeRecognizer::new(PytestRecognizer::with_env(&project, None).unwrap(), false)
    }

    #[test]
    fn the_executor_installs_the_native_modules_in_order() {
        let executor = Pytest::default().executor();
        assert_eq!(executor.adapter, "pytest");
        assert_eq!(executor.entry, "aye_pytest_native:NativeExecutor");
        let names: Vec<&str> = executor.modules.names().collect();
        assert_eq!(
            names,
            [
                "aye_model",
                "aye_rewrite",
                "aye_native_engine",
                "pytest",
                "aye_pytest_frontend",
                "aye_pytest_native"
            ]
        );
        assert!(executor.lists_session);
        assert_eq!(Pytest::default().name(), NAME);
        assert_eq!(Pytest::static_listing().executor().entry, executor.entry);
    }

    #[test]
    fn options_parse_as_pytests_argparse_parses_them() {
        let parsed = parse_options(&strings(&[
            "-vv",
            "-q",
            "-ra",
            "-kslow and not fast",
            "-p",
            "no:cacheprovider",
            "-pxdist",
            "--runxfail",
            "-W",
            "error",
            "-Wignore::DeprecationWarning",
            "--basetemp=/tmp/b",
            "-s",
            "--tb",
            "short",
            "--durations=5",
            "--runslow",
            "--debug",
            "-x",
            "tests/a.py",
            "--",
            "-notanoption",
        ]));
        let v = &parsed.values;
        assert_eq!(v["verbose"], json!(1));
        assert_eq!(v["reportchars"], json!("a"));
        assert_eq!(v["keyword"], json!("slow and not fast"));
        assert_eq!(v["markexpr"], json!(""));
        assert_eq!(v["plugins"], json!(["no:cacheprovider", "xdist"]));
        assert_eq!(v["runxfail"], json!(true));
        assert_eq!(
            v["pythonwarnings"],
            json!(["error", "ignore::DeprecationWarning"])
        );
        assert_eq!(v["basetemp"], json!("/tmp/b"));
        assert_eq!(v["capture"], json!("no"));
        assert_eq!(v["tbstyle"], json!("short"));
        assert_eq!(v["durations"], json!(5));
        assert_eq!(v["debug"], json!("pytestdebug.log"));
        assert_eq!(v["maxfail"], json!(1));
        assert_eq!(v["file_or_dir"], json!(["tests/a.py", "-notanoption"]));
        assert_eq!(parsed.unknown, ["--runslow"]);
        // Defaults for what isn't given.
        assert_eq!(v["importmode"], json!("prepend"));
        assert_eq!(v["assertmode"], json!("rewrite"));
        assert_eq!(v["ignore"], Value::Null);
        assert_eq!(v["doctestglob"], json!([]));
        // An unknown short option ends its cluster; a negative number is a
        // value.
        let parsed = parse_options(&strings(&["-vZq", "-1"]));
        assert_eq!(parsed.values["verbose"], json!(1));
        assert_eq!(parsed.unknown, ["-vZq"]);
        assert_eq!(parsed.values["file_or_dir"], json!(["-1"]));
    }

    #[test]
    fn the_native_context_resolves_the_settings_and_the_raw_ini() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = -ra --strict-markers -p no:cacheprovider -n 4 -x\n\
             markers =\n    slow: slow tests\nfilterwarnings =\n    error\n    ignore::UserWarning\n\
             python_functions = test check\ntimeout = 5\n",
        );
        let cli = strings(&["-o", "xfail_strict=true", "--runslow"]);
        let context = recognizer(root, &cli).context().into_value();
        // Nothing of what only fed a pytest session per batch.
        for key in [
            "addopts",
            "pytest_addopts_env",
            "framework_args",
            "single_session",
            "doctests",
            "adds_items",
        ] {
            assert_eq!(context.get(key), None, "{key}");
        }
        let native = &context["native"];
        assert_eq!(native["rootdir"], json!(root.to_string_lossy()));
        assert_eq!(
            native["inifile"],
            json!(root.join("pytest.ini").to_string_lossy())
        );
        assert_eq!(native["python_functions"], json!(["test", "check"]));
        assert_eq!(native["python_files"], json!(["test_*.py", "*_test.py"]));
        assert_eq!(native["markers"], json!(["slow: slow tests"]));
        assert_eq!(
            native["filterwarnings"],
            json!(["error", "ignore::UserWarning"])
        );
        assert_eq!(native["xfail_strict"], json!(true));
        assert_eq!(native["strict_markers"], json!(true));
        assert_eq!(native["import_mode"], json!("prepend"));
        assert_eq!(native["ini"]["timeout"], json!("5"));
        assert_eq!(native["ini"]["xfail_strict"], json!("true"));
        assert_eq!(native["ini"]["strict_markers"], json!("true"));
        // `-n` and `-x` are the runner's: not in the arguments the
        // front-end parses, but `maxfail` says what was asked.
        assert_eq!(
            native["args"],
            json!([
                "-ra",
                "--strict-markers",
                "-p",
                "no:cacheprovider",
                "-o",
                "xfail_strict=true",
                "--runslow"
            ])
        );
        assert_eq!(native["options"]["maxfail"], json!(1));
        assert_eq!(native["options"]["plugins"], json!(["no:cacheprovider"]));
        assert_eq!(native["options"]["strict_markers"], json!(true));
        assert_eq!(native["unknown_options"], json!(["--runslow"]));
        assert_eq!(native["modules_digest"], json!(modules_digest()));
    }

    #[test]
    fn toml_values_keep_their_shape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "pyproject.toml",
            "[tool.pytest.ini_options]\nxfail_strict = true\nmarkers = [\"slow\", \"fast\"]\n",
        );
        let native = &recognizer(root, &[]).context().into_value()["native"];
        assert_eq!(native["ini"]["xfail_strict"], json!("True"));
        assert_eq!(native["ini"]["markers"], json!(["slow", "fast"]));
        assert_eq!(native["xfail_strict"], json!(true));
        write(
            root,
            "pyproject.toml",
            "[tool.pytest]\nstrict = true\nminversion = \"9.0\"\n",
        );
        let native = &recognizer(root, &[]).context().into_value()["native"];
        assert_eq!(native["ini"]["strict"], json!(true));
        assert_eq!(native["xfail_strict"], json!(true));
        assert_eq!(native["strict_markers"], json!(true));
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

    /// The context's bytes as aye 0.0.1 built them (`tests/contexts.jsonl`,
    /// one line each), for a project whose options and ini set every key
    /// the binary reads, and for one that sets nothing. The refinement
    /// cache's key hashes them, so a change here misses every cached entry.
    /// The project root is written `<root>`, and the digest of the
    /// executor's sources `<modules_digest>`, which changes with them.
    /// Pinned where paths are written with `/`.
    #[cfg(unix)]
    #[test]
    fn the_context_is_pinned_byte_for_byte() {
        let everything = tempfile::tempdir().unwrap();
        let root = everything.path();
        write(
            root,
            "pytest.ini",
            "[pytest]\naddopts = -ra -n 3 --maxfail=2 --junitxml=reports/out.xml \
             --junitprefix=ci -k \"not slow\" -m db --deselect tests/a/test_x.py::test_skip \
             -p no:cacheprovider\ntestpaths = tests\nmarkers =\n    slow: slow tests\n\
             junit_family = xunit1\njunit_log_passing_tests = false\n\
             junit_suite_name = suite\ntimeout = 5\n",
        );
        write(root, "tests/conftest.py", "");
        write(
            root,
            "tests/a/test_x.py",
            "import helpers\n\ndef test_x():\n    pass\n",
        );
        write(root, "tests/a/helpers.py", "");
        write(root, "tests/b/test_y.py", "def test_y():\n    pass\n");
        write(root, "tests/b/helpers.py", "");
        let cli = strings(&["-o", "xfail_strict=true", "--runslow"]);
        let project = Project {
            framework_args: &cli,
            ..Project::new(root, root)
        };
        let env = crate::PytestEnv {
            pytest_addopts: Some("-v"),
            pytest_plugins: Some("myplugin"),
            pythonpath: None,
        };
        let set = NativeRecognizer::new(
            PytestRecognizer::with_environment(&project, env).unwrap(),
            false,
        );
        let nothing = tempfile::tempdir().unwrap();
        let unset = recognizer(nothing.path(), &[]);
        let written = |recognizer: &NativeRecognizer, root: &Path| {
            let root = serde_json::to_string(&root.to_string_lossy()).unwrap();
            let digest = serde_json::to_string(&modules_digest()).unwrap();
            serde_json::to_string(&recognizer.context().into_value())
                .unwrap()
                .replace(root.trim_matches('"'), "<root>")
                .replace(&digest, "\"<modules_digest>\"")
        };
        let pinned: Vec<&str> = include_str!("tests/contexts.jsonl").lines().collect();
        assert_eq!(
            [written(&set, root), written(&unset, nothing.path())],
            pinned[..]
        );
    }
}
