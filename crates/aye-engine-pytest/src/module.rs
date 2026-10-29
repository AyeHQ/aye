//! Recognition of one test module, the way pytest's `Module`, `Class` and
//! `UnitTestCase` collectors walk it (`_pytest/python.py`,
//! `_pytest/unittest.py`), without importing it.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use aye_engine::ast::{Expr, ExprLambda, Stmt, StmtClassDef, StmtFunctionDef};
use aye_engine::{Ranged, Recognition, SourceFile, TextRange, TextSize};
use aye_events::{Case, Certainty, Diagnostic, Severity};

use crate::fnmatch::matches_prefix_or_glob;
use crate::ids::{IdOptions, parameterset_ids};
use crate::marks::{self, Decorator, EmptyMark, Mark, MarkList, Scoped, SetTimeout, decorator};
use crate::scope::{
    Binding, Bound, Flag, Lookup, Namespace, OpKind, Resolved, exclusive, import_time_raise,
    import_time_skip, is_generator,
};
use crate::support::{
    BUILTIN_FIXTURES, FixtureDef, MARKING_HOOKS, REFINING_HOOKS, default_argnames, fixture_of,
};

/// The configuration recognition depends on.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub python_classes: Vec<String>,
    pub python_functions: Vec<String>,
    pub ids: IdOptions,
    pub empty: EmptyMark,
    pub collect_imported_tests: bool,
    /// The `usefixtures` ini option.
    pub usefixtures: Vec<String>,
    /// `-p no:unittest`: `TestCase` classes are plain classes to pytest.
    pub unittest_blocked: bool,
    /// `-p no:python`: nothing collects Python files.
    pub python_blocked: bool,
}

/// What the support files in scope do.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scope {
    /// Hooks of the conftests in scope (and the plugins they load).
    pub hooks: BTreeSet<String>,
    /// [`GLOBAL_HOOKS`](crate::support::GLOBAL_HOOKS) implemented by any
    /// conftest of the project: pytest applies them to every test of the
    /// session once that conftest is loaded.
    pub global_hooks: BTreeSet<String>,
    pub collect_file_touches_py: bool,
    /// A conftest in scope calls `pytest.skip` or `pytest.importorskip`
    /// while it is imported.
    pub skips_at_import: bool,
    pub fixtures: Vec<FixtureDef>,
    /// Names conftests in scope import from modules that are not project
    /// files: each may be a fixture.
    pub maybe_fixtures: BTreeSet<String>,
    /// A conftest in scope star-imports a module that is not a project
    /// file: any name may be a fixture.
    pub any_fixture: bool,
    /// A conftest in scope loads plugins aye cannot name.
    pub hooks_unknown: bool,
    /// What the test module imports from other modules.
    pub module_imports: ModuleImports,
    /// The test module names plugins in `pytest_plugins`.
    pub module_plugins: bool,
    /// Modules whose attributes support files in scope may change.
    pub patched: BTreeSet<String>,
    /// A support file in scope reaches module namespaces in ways aye can't
    /// follow: the test module's names may change.
    pub reflective: bool,
    /// A support file in scope imports or looks up a module of the test
    /// module's name (by literal name).
    pub imports_module: bool,
    /// What implements each of `hooks` ("a conftest.py in scope", "a -p
    /// plugin"), for messages.
    pub hook_sources: BTreeMap<String, String>,
    /// The same for `global_hooks`.
    pub global_hook_sources: BTreeMap<String, String>,
    /// A conftest in scope loads plugins aye can't find while some
    /// conftest puts directories aye can't tell on `sys.path`: a name no
    /// readable definition has (but pytest's own) may be one of their
    /// fixtures.
    pub hidden_fixtures: bool,
}

impl Scope {
    /// What implements `hook`, of those in scope.
    fn hook_source(&self, hook: &str) -> &str {
        self.hook_sources
            .get(hook)
            .map_or("a conftest.py in scope", String::as_str)
    }

    /// What implements `hook`, of the session-wide ones.
    fn global_hook_source(&self, hook: &str) -> &str {
        self.global_hook_sources
            .get(hook)
            .map_or("a conftest.py", String::as_str)
    }
}

/// What the module's imports refer to, beyond its own source.
pub(crate) trait Imports {
    /// Whether the class `path` names (a dotted path through the module's
    /// imports) may be a unittest case, which a subclass inherits tests
    /// from whatever its name.
    fn may_be_unittest(&self, path: &str) -> bool;

    /// Whether `path` names something in a project file (not a module)
    /// that may be a unittest case, which pytest collects whatever its
    /// name.
    fn project_unittest(&self, path: &str) -> bool;

    /// Whether `path` names a class or function of a project file whose
    /// `__test__` may be true (set in a class body, inherited from one, or
    /// set by its module), which pytest collects whatever its name.
    fn project_test(&self, path: &str) -> bool;

    /// Whether `path` names a function of a project file that may make
    /// classes (a class statement, `type(…)`, `__test__`), whose result
    /// may then be a test class whatever the name it is bound to.
    fn project_factory(&self, path: &str) -> bool;
}

/// Fixtures a test module imports.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleImports {
    /// Read from project files.
    pub fixtures: Vec<FixtureDef>,
    /// Imported from modules that are not project files: each may be a
    /// fixture.
    pub maybe: BTreeSet<String>,
    /// Star-imported from a module that is not a project file.
    pub any: bool,
}

/// Attributes every module, class and instance has; pytest never collects
/// them.
const IGNORED_ATTRIBUTES: &[&str] = &[
    "__builtins__",
    "__cached__",
    "__class__",
    "__delattr__",
    "__dict__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__file__",
    "__firstlineno__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__le__",
    "__loader__",
    "__lt__",
    "__module__",
    "__name__",
    "__ne__",
    "__new__",
    "__package__",
    "__qualname__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__sizeof__",
    "__spec__",
    "__static_attributes__",
    "__str__",
    "__subclasshook__",
    "__weakref__",
];

/// unittest base classes that hold no tests of their own.
const UNITTEST_BASES: &[&str] = &[
    "unittest.TestCase",
    "unittest.case.TestCase",
    "unittest.IsolatedAsyncioTestCase",
    "unittest.async_case.IsolatedAsyncioTestCase",
    "django.test.TestCase",
    "django.test.SimpleTestCase",
    "django.test.TransactionTestCase",
    "django.test.LiveServerTestCase",
    "django.test.testcases.TestCase",
    "django.test.testcases.SimpleTestCase",
    "django.test.testcases.TransactionTestCase",
    "django.test.testcases.LiveServerTestCase",
    "django.contrib.staticfiles.testing.StaticLiveServerTestCase",
    "rest_framework.test.APITestCase",
    "rest_framework.test.APISimpleTestCase",
    "rest_framework.test.APITransactionTestCase",
    "rest_framework.test.APILiveServerTestCase",
    "twisted.trial.unittest.TestCase",
    "twisted.trial.unittest.SynchronousTestCase",
    "asynctest.TestCase",
    "aiounittest.AsyncTestCase",
];

/// unittest base classes that bring a `runTest` of their own.
const UNITTEST_RUNTEST: &[&str] = &[
    "unittest.FunctionTestCase",
    "unittest.case.FunctionTestCase",
];

/// Packages whose names never hold tests (nor fixtures or hooks) when
/// imported into a test module or a conftest, and whose classes are never
/// unittest cases with tests of their own: the standard library's usual
/// modules and widespread libraries that ship no test cases.
const NO_TESTS: &[&str] = &[
    "_pytest",
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "attr",
    "attrs",
    "base64",
    "bisect",
    "builtins",
    "calendar",
    "codecs",
    "collections",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "django",
    "email",
    "enum",
    "errno",
    "factory",
    "fnmatch",
    "fractions",
    "functools",
    "glob",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "httpx",
    "hypothesis",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "logging",
    "marshmallow",
    "math",
    "mock",
    "multiprocessing",
    "numbers",
    "numpy",
    "operator",
    "os",
    "pandas",
    "pathlib",
    "pickle",
    "platform",
    "pprint",
    "pydantic",
    "pytest",
    "queue",
    "random",
    "re",
    "requests",
    "rest_framework",
    "secrets",
    "shlex",
    "shutil",
    "signal",
    "socket",
    "sqlalchemy",
    "sqlite3",
    "ssl",
    "statistics",
    "string",
    "struct",
    "subprocess",
    "sys",
    "tempfile",
    "textwrap",
    "threading",
    "time",
    "tomllib",
    "traceback",
    "types",
    "typing",
    "typing_extensions",
    "unittest",
    "urllib",
    "uuid",
    "warnings",
    "weakref",
    "xml",
    "zipfile",
    "zoneinfo",
];

/// Why a class's MRO is not known, and whether only classes from
/// [`NO_TESTS`] packages are missing from it (they hold no tests to
/// inherit, and are no unittest cases).
#[derive(Debug, Clone)]
struct Unresolved {
    reason: String,
    harmless: bool,
}

/// A class in a method resolution order.
#[derive(Debug, Clone)]
enum Klass<'a> {
    Local(&'a StmtClassDef),
    /// A known class from elsewhere, by dotted path.
    External(String),
}

impl Klass<'_> {
    fn key(&self) -> String {
        match self {
            Self::Local(def) => format!("local:{}", def.start().to_u32()),
            Self::External(path) => path.clone(),
        }
    }

    fn object() -> Self {
        Self::External("builtins.object".to_string())
    }
}

/// What an external base class brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum External {
    Object,
    Abc,
    Plain,
    /// `typing.Protocol`: its metaclass derives from `ABCMeta`, and
    /// `Protocol.__init_subclass__` gives protocol classes an `__init__`
    /// that their subclasses inherit.
    Protocol,
    UnitTest,
    /// Defines `__init__` and `__new__` (exceptions).
    InitNew,
    /// Defines `__new__` (enums, named tuples).
    New,
}

fn external_kind(path: &str) -> Option<External> {
    const PLAIN: &[&str] = &["typing.Generic", "typing_extensions.Generic"];
    const PROTOCOL: &[&str] = &["typing.Protocol", "typing_extensions.Protocol"];
    const INIT_NEW: &[&str] = &[
        "enum.Enum",
        "enum.IntEnum",
        "enum.StrEnum",
        "enum.Flag",
        "enum.IntFlag",
        "builtins.list",
        "builtins.dict",
        "builtins.set",
        "builtins.frozenset",
        "builtins.bytearray",
    ];
    const NEW: &[&str] = &[
        "typing.NamedTuple",
        "builtins.int",
        "builtins.str",
        "builtins.tuple",
        "builtins.float",
        "builtins.bytes",
    ];
    const EXCEPTIONS: &[&str] = &[
        "BaseException",
        "BaseExceptionGroup",
        "ExceptionGroup",
        "KeyboardInterrupt",
        "SystemExit",
        "GeneratorExit",
        "StopIteration",
        "StopAsyncIteration",
    ];
    let builtin = path.strip_prefix("builtins.");
    Some(match path {
        "builtins.object" => External::Object,
        "abc.ABC" => External::Abc,
        _ if PLAIN.contains(&path) => External::Plain,
        _ if PROTOCOL.contains(&path) => External::Protocol,
        _ if UNITTEST_BASES.contains(&path) || UNITTEST_RUNTEST.contains(&path) => {
            External::UnitTest
        }
        _ if INIT_NEW.contains(&path) => External::InitNew,
        _ if NEW.contains(&path) => External::New,
        _ if builtin.is_some_and(|name| {
            name.ends_with("Error")
                || name.ends_with("Exception")
                || name.ends_with("Warning")
                || EXCEPTIONS.contains(&name)
        }) =>
        {
            External::InitNew
        }
        _ => return None,
    })
}

/// A class being collected, as a node in pytest's tree.
#[derive(Clone)]
struct Frame<'s, 'a> {
    /// The node name (the name it is bound to).
    name: String,
    mro: Vec<Klass<'a>>,
    /// The class's marks, from its MRO, base first.
    marks: Vec<Mark<'s, 'a>>,
    /// Some of its marks could not be read.
    marks_unknown: bool,
}

/// What an attribute of a unittest class is to `getTestCaseNames`.
enum Attr<'a> {
    Test(Callable<'a>),
    NotTest,
    Unknown,
}

/// Something callable bound to a test name.
#[derive(Clone, Copy)]
enum Callable<'a> {
    Def(&'a StmtFunctionDef),
    Lambda(&'a ExprLambda, TextRange),
}

impl Callable<'_> {
    fn span(&self) -> TextRange {
        match self {
            Self::Def(def) => def.name.range(),
            Self::Lambda(_, name) => *name,
        }
    }
}

/// Whether changing the module `path` names may change what aye reads of
/// pytest: its API (`pytest.param`, `pytest.mark`), or the internals that
/// collect and make ids. Other internals (`_pytest.doctest`, reporting)
/// don't.
fn changes_collection(path: &str) -> bool {
    let mut parts = path.split('.');
    match (parts.next(), parts.next()) {
        (Some("pytest" | "_pytest"), None) => true,
        (Some("pytest"), Some(name)) => matches!(
            name,
            "mark" | "param" | "HIDDEN_PARAM" | "fixture" | "skip" | "importorskip"
        ),
        (Some("_pytest"), Some(module)) => matches!(
            module,
            "python"
                | "python_api"
                | "mark"
                | "fixtures"
                | "compat"
                | "main"
                | "nodes"
                | "unittest"
        ),
        _ => false,
    }
}

/// Why code other than the module's own statements may change the test
/// module's names (empty when nothing does): the module imports itself by
/// name, or a support file in scope reaches module namespaces, imports a
/// module of its name, or implements a hook that gets the module before
/// pytest collects it. `rel` is the module's path.
pub(crate) fn reached(module: &Namespace<'_>, scope: &Scope, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    if names_module(module.imported_modules(), rel) {
        out.push("the module imports itself, which may change its names".to_string());
    }
    if scope.reflective {
        out.push("a conftest.py in scope reaches module namespaces at runtime".to_string());
    }
    if scope.imports_module {
        out.push("a conftest.py in scope imports a module of this name".to_string());
    }
    for hook in ["pytest_collectstart", "pytest_make_collect_report"] {
        if scope.hooks.contains(hook) {
            out.push(format!(
                "{} implements {hook}, which sees the module before pytest collects it",
                scope.hook_source(hook)
            ));
        }
    }
    out
}

/// Whether one of `modules` (dotted names, as imported) may be the module
/// at `rel`: its last part is the file's stem.
pub(crate) fn names_module(modules: &[String], rel: &str) -> bool {
    let stem = std::path::Path::new(rel)
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    modules.iter().any(|m| m.rsplit('.').next() == Some(&*stem))
}

/// Recognize one test module. `module` is its namespace
/// ([`Namespace::build`] over its body).
pub(crate) fn recognize<'a>(
    settings: &Settings,
    scope: &Scope,
    imports: &dyn Imports,
    file: &SourceFile<'a>,
    module: &Namespace<'a>,
    path_keywords: &[String],
) -> Recognition {
    let mut classes = HashMap::new();
    class_namespaces(&file.module.body, None, &mut classes);
    let empty = Namespace::default();
    let mut recognizer = ModuleRecognizer {
        settings,
        scope,
        imports,
        file,
        module,
        classes: &classes,
        empty: &empty,
        path_keywords,
        cases: Vec::new(),
        keys: Vec::new(),
        facts: std::cell::RefCell::default(),
        diagnostics: Vec::new(),
        refine: Vec::new(),
        module_marks: Vec::new(),
        module_marks_unknown: false,
        module_fixtures: Vec::new(),
        class_fixtures: HashMap::new(),
        module_wide: false,
        partial_names: HashSet::new(),
        sets_names: classes
            .values()
            .any(|info| info.ns.get("__set_name__").is_some()),
    };
    recognizer.run();
    if recognizer.module_wide || !recognizer.partial_names.is_empty() {
        let prefix = format!("{}::", file.rel);
        for case in &mut recognizer.cases {
            let name = case.id.0.strip_prefix(&prefix).unwrap_or_default();
            let name = name.split("::").next().unwrap_or_default();
            let name = name.split('[').next().unwrap_or_default();
            if (recognizer.module_wide || recognizer.partial_names.contains(name))
                && case.certainty == Certainty::Exact
            {
                case.certainty = Certainty::Partial;
            }
        }
    }
    if recognizer.keys.iter().any(|k| !k.is_empty()) {
        let order = reorder_items(&recognizer.keys);
        let mut cases: Vec<Option<Case>> = recognizer.cases.drain(..).map(Some).collect();
        recognizer.cases = order.into_iter().filter_map(|i| cases[i].take()).collect();
    }
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for diagnostic in recognizer.diagnostics {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by_key(|d| d.span.as_ref().map_or(0, |s| s.start));
    let mut refine: Vec<String> = Vec::new();
    for reason in recognizer.refine {
        if !refine.contains(&reason) {
            refine.push(reason);
        }
    }
    Recognition {
        cases: recognizer.cases,
        refine: (!refine.is_empty()).then(|| refine.join("; ")),
        diagnostics,
    }
}

/// A class body's namespace, and the class body its statement sits in.
struct ClassInfo<'a> {
    def: &'a StmtClassDef,
    ns: Namespace<'a>,
    site: Option<&'a StmtClassDef>,
}

fn class_namespaces<'a>(
    body: &'a [Stmt],
    site: Option<&'a StmtClassDef>,
    out: &mut HashMap<TextSize, ClassInfo<'a>>,
) {
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class) => {
                out.insert(
                    class.start(),
                    ClassInfo {
                        def: class,
                        ns: Namespace::build(&class.body),
                        site,
                    },
                );
                class_namespaces(&class.body, Some(class), out);
            }
            Stmt::If(s) => {
                class_namespaces(&s.body, site, out);
                for clause in &s.elif_else_clauses {
                    class_namespaces(&clause.body, site, out);
                }
            }
            Stmt::Try(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
                class_namespaces(&s.finalbody, site, out);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        class_namespaces(&handler.body, site, out);
                    }
                }
            }
            Stmt::With(s) => class_namespaces(&s.body, site, out),
            Stmt::For(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::While(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::Match(s) => {
                for case in &s.cases {
                    class_namespaces(&case.body, site, out);
                }
            }
            _ => {}
        }
    }
}

struct ModuleRecognizer<'s, 'a> {
    settings: &'s Settings,
    scope: &'s Scope,
    imports: &'s dyn Imports,
    file: &'s SourceFile<'a>,
    module: &'s Namespace<'a>,
    classes: &'s HashMap<TextSize, ClassInfo<'a>>,
    /// Stands in for a class body that was not walked (never expected).
    empty: &'s Namespace<'a>,
    path_keywords: &'s [String],
    cases: Vec<Case>,
    /// Each case's class- and module-scoped parameters, for `reorder_items`.
    keys: Vec<Vec<ParamKey>>,
    /// [`ModuleRecognizer::body_facts`], by class (`None` for the module).
    facts: std::cell::RefCell<HashMap<Option<TextSize>, std::rc::Rc<BodyFacts<'a>>>>,
    diagnostics: Vec<Diagnostic>,
    refine: Vec<String>,
    module_marks: Vec<Mark<'s, 'a>>,
    module_marks_unknown: bool,
    /// Fixtures defined in the module, in order.
    module_fixtures: Vec<FixtureDef>,
    /// Fixtures defined in each class body, by class.
    class_fixtures: HashMap<TextSize, Vec<FixtureDef>>,
    /// A reason that holds for the whole module was found: none of its
    /// cases is exact.
    module_wide: bool,
    /// Module names whose tests may not be what the source shows (bound
    /// or deleted through `global`): their cases are partial.
    partial_names: HashSet<String>,
    /// Some class of the module defines `__set_name__`, which Python calls
    /// with the class that holds an instance as an attribute.
    sets_names: bool,
}

/// The parts of a test's identity and metadata, before it becomes a case.
struct Item<'s, 'a> {
    classes: Vec<String>,
    name: String,
    originalname: String,
    span: TextRange,
    /// In pytest's `iter_markers` order: the function's, the parameter
    /// set's, each class's (innermost first), the module's.
    marks: Vec<Mark<'s, 'a>>,
    /// How many of `marks` are the function's own.
    fn_marks: usize,
    /// Marks from `pytest.param(marks=…)` and the empty parameter set.
    param_marks: Vec<String>,
    /// The timeout the first `timeout` mark among `param_marks` sets.
    param_timeout: SetTimeout,
    /// Names in the function's `__dict__`: what decorators leave there,
    /// and attributes the module sets.
    attributes: Vec<String>,
    certainty: Certainty,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    fn lookup(&self, class: Option<&'a StmtClassDef>, at: TextSize) -> Lookup<'s, 'a> {
        Lookup {
            module: self.module,
            class: class.map(|c| self.class_ns(c)),
            source: self.file.source,
            at,
        }
    }

    fn class_ns(&self, class: &StmtClassDef) -> &'s Namespace<'a> {
        self.classes
            .get(&class.start())
            .map_or(self.empty, |info| &info.ns)
    }

    fn end(&self) -> TextSize {
        TextSize::try_from(self.file.source.len()).unwrap_or(TextSize::new(u32::MAX))
    }

    /// Whether some hook may add marks to this file's tests.
    fn marking_hooks(&self) -> bool {
        MARKING_HOOKS
            .iter()
            .any(|hook| self.scope.hooks.contains(*hook))
            || self
                .scope
                .global_hooks
                .contains("pytest_collection_modifyitems")
    }

    /// Whether a name bound to `path` may be a unittest case of the
    /// project, which pytest collects whatever its name: a capitalized
    /// name (as classes are) from a project file, unless unittest is off.
    fn may_be_project_unittest(&self, name: &str, path: &str) -> bool {
        !self.settings.unittest_blocked
            && name.chars().next().is_some_and(char::is_uppercase)
            && self.imports.project_unittest(path)
    }

    fn is_test_function_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_functions, name)
    }

    fn is_test_class_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_classes, name)
    }

    /// What a module (`None`) or class body may skip at runtime, worked out
    /// once per body.
    fn body_facts(&self, site: Option<&'a StmtClassDef>) -> std::rc::Rc<BodyFacts<'a>> {
        let key = site.map(Ranged::start);
        if let Some(found) = self.facts.borrow().get(&key) {
            return std::rc::Rc::clone(found);
        }
        let body = site.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let facts = std::rc::Rc::new(BodyFacts {
            deletes: conditional_deletes(body),
            guarded: guarded_by_try(body),
        });
        self.facts
            .borrow_mut()
            .insert(key, std::rc::Rc::clone(&facts));
        facts
    }

    fn doubt(&mut self, reason: impl Into<String>) {
        self.refine.push(reason.into());
    }

    fn diagnostic(&mut self, severity: Severity, code: &str, range: TextRange, message: String) {
        self.diagnostics.push(Diagnostic {
            severity,
            code: code.to_string(),
            path: self.file.rel.to_string(),
            span: Some(self.file.span(range)),
            message,
        });
    }

    fn nodeid(&self, classes: &[String]) -> String {
        let mut id = self.file.rel.to_string();
        for class in classes {
            id.push_str("::");
            id.push_str(class);
        }
        id
    }

    /// Fixtures defined directly in a namespace, as [`fixture_of`] reads
    /// them (alternative branches, call forms and aliases included).
    fn fixtures_in(&self, ns: &Namespace<'a>, class: Option<&'a StmtClassDef>) -> Vec<FixtureDef> {
        let lookup = self.lookup(class, self.end());
        let body = class.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let guarded = guarded_by_try(body);
        ns.iter()
            .filter_map(|(name, _)| fixture_of(lookup, ns, name, class.is_some(), &guarded))
            .collect()
    }

    fn run(&mut self) {
        let module = self.module;
        let end = self.end();
        let lookup = self.lookup(None, end);
        self.module_fixtures = self.fixtures_in(module, None);
        self.module_fixtures
            .extend(self.scope.module_imports.fixtures.iter().cloned());
        let classes = self.classes;
        let mut class_fixtures = HashMap::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills a map: the order shows nowhere"
        )]
        for (start, info) in classes {
            let class = self.class_def(*start);
            class_fixtures.insert(*start, self.fixtures_in(&info.ns, class));
        }
        self.class_fixtures = class_fixtures;

        if let Some(bound) = module.get("__test__") {
            let value = match bound.binding {
                Binding::Value(expr) => lookup.at(bound.range.end()).boolean(expr),
                _ => None,
            };
            match value {
                Some(true) => {}
                Some(false) => return,
                None => {
                    self.doubt("the module's __test__ is computed at runtime");
                    return;
                }
            }
        }
        let reached = reached(module, self.scope, self.file.rel);
        if !reached.is_empty() {
            for reason in reached {
                self.doubt(reason);
            }
        } else if module.reflective {
            self.doubt(
                "the module reaches module namespaces at runtime (sys.modules, frames or gc)",
            );
        } else if module.dynamic {
            self.doubt("the module binds names at runtime (globals(), setattr or exec)");
        }
        if let Some(patched) = module
            .patched_modules()
            .chain(&self.scope.patched)
            .find(|m| changes_collection(m))
        {
            self.doubt(format!(
                "{patched} is changed at runtime, which may change how pytest collects"
            ));
        }
        if let Some(call) = import_time_skip(&self.file.module.body, lookup) {
            self.doubt(format!("the module may skip itself at import ({call})"));
        }
        if let Some(what) = import_time_raise(&self.file.module.body, lookup) {
            self.doubt(format!("{what} may raise while the module is imported"));
        }
        if self.settings.collect_imported_tests {
            for source in &module.star_imports {
                if !no_tests(source) {
                    self.doubt(format!("`from {source} import *` may bring in tests"));
                } else if !self.settings.unittest_blocked
                    && matches!(source.as_str(), "unittest" | "unittest.case")
                {
                    // `FunctionTestCase` brings a `runTest`, so pytest
                    // collects it wherever it is bound.
                    self.doubt(format!(
                        "`from {source} import *` binds FunctionTestCase, which pytest collects"
                    ));
                }
            }
        }
        match self.pytestmark(module, None) {
            Ok(list) => {
                self.module_marks = list.marks;
                self.module_marks_unknown = list.unknown;
            }
            Err(reason) => self.doubt(format!("module {reason}")),
        }
        for hook in REFINING_HOOKS {
            if self.scope.hooks.contains(hook) {
                self.doubt(format!(
                    "{} implements {hook}",
                    self.scope.hook_source(hook)
                ));
            }
        }
        if self.scope.global_hooks.contains("pytest_collection") {
            self.doubt(format!(
                "{} implements pytest_collection",
                self.scope.global_hook_source("pytest_collection")
            ));
        }
        if self.scope.skips_at_import {
            self.doubt("a conftest.py in scope may skip at import");
        }
        if self.scope.collect_file_touches_py {
            self.doubt("a conftest.py in scope implements pytest_collect_file for Python files");
        }
        if self.scope.hooks_unknown {
            self.doubt("a conftest.py in scope loads plugins aye can't read");
        }
        if self.scope.module_plugins {
            self.doubt("the module loads plugins through pytest_plugins");
        }
        // Each reason so far may change any of the module's tests.
        self.module_wide = !self.refine.is_empty();
        // A function that runs at import and binds or deletes a module
        // name through `global`: the name may be a test, or no longer one.
        for (name, value) in module.global_writes() {
            let unknown = self.is_test_function_name(name)
                || self.is_test_class_name(name)
                || value.is_none_or(|value| !is_literal(value));
            if unknown {
                self.doubt(format!(
                    "{name} is bound or deleted through `global` by a function that runs at import"
                ));
                self.partial_names.insert((*name).to_string());
            }
        }

        self.duplicates(module);
        let mut entries: Vec<(&'a str, Bound<'a>)> =
            module.iter().map(|(n, b)| (n, b.clone())).collect();
        entries.extend(maybe_deleted(module, &self.file.module.body));
        for (name, bound) in entries {
            if IGNORED_ATTRIBUTES.contains(&name) {
                continue;
            }
            self.member(name, &bound, None, &[], true);
        }
    }

    /// `pytest_pycollect_makeitem` for one name of a module or class
    /// namespace. `site` is the class whose body bound the name.
    fn member(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) {
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        let bindings = ns.bindings(name);
        let mut alternatives: Vec<&Bound<'a>> = bindings
            .iter()
            .filter(|earlier| {
                earlier.range.start() < bound.range.start() && exclusive(earlier, bound)
            })
            .collect();
        if bound.conditional
            && let Some(previous) = bindings
                .iter()
                .rev()
                .find(|b| b.range.start() < bound.range.start() && !exclusive(b, bound))
        {
            alternatives.push(previous);
        }
        let before = self.cases.len();
        let facts = self.body_facts(site);
        // A `del` that may not run leaves the name bound or not.
        let deleted = facts
            .deletes
            .iter()
            .any(|(n, at)| *n == name && *at > bound.range.start());
        let guarded = facts.guarded.contains(&bound.range.start());
        if deleted {
            self.member_binding(name, bound, site, frames, in_module);
            if self.cases.len() > before {
                self.doubt(format!("{name} may be deleted at runtime"));
                for case in &mut self.cases[before..] {
                    case.certainty = Certainty::Partial;
                }
            }
            return;
        }
        if alternatives.is_empty() {
            self.member_binding(name, bound, site, frames, in_module);
            // Bound in a branch that may not run (`if`, `for`, `match`,
            // `except`, or a `try` body after a statement that may raise):
            // whether the test exists is known at runtime.
            if (bound.conditional || guarded) && self.cases.len() > before {
                self.doubt(format!(
                    "{name} is defined conditionally; whether it exists is known at runtime"
                ));
                for case in &mut self.cases[before..] {
                    case.certainty = Certainty::Partial;
                }
            }
            return;
        }
        // Which binding runs is known at runtime: fine when they all make
        // the same tests.
        let outcomes: Vec<Option<Vec<String>>> = alternatives
            .iter()
            .map(|alternative| self.trial(name, alternative, site, frames, in_module))
            .collect();
        let chosen = self.trial(name, bound, site, frames, in_module);
        if chosen.is_none() || outcomes.iter().any(|o| *o != chosen) {
            self.doubt(format!(
                "{name} is defined in alternative branches; which one runs is known at runtime"
            ));
        }
        self.member_binding(name, bound, site, frames, in_module);
    }

    /// The ids a binding would make, without keeping them; `None` when it
    /// raises doubts of its own.
    fn trial(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) -> Option<Vec<String>> {
        let (cases, refine, diagnostics) =
            (self.cases.len(), self.refine.len(), self.diagnostics.len());
        self.member_binding(name, bound, site, frames, in_module);
        let ids: Vec<String> = self.cases[cases..].iter().map(|c| c.id.0.clone()).collect();
        let clean = self.refine.len() == refine;
        self.cases.truncate(cases);
        self.keys.truncate(cases);
        self.refine.truncate(refine);
        self.diagnostics.truncate(diagnostics);
        clean.then_some(ids)
    }

    fn member_binding(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) {
        let lookup = self.lookup(site, bound.range.start());
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        let test_attr = ns.test_attr(name);
        match &bound.binding {
            Binding::Function(def) => {
                self.function(name, Callable::Def(def), site, frames, test_attr);
            }
            Binding::Class(class) => self.class(name, class, site, frames),
            Binding::Value(expr) => match expr {
                Expr::Lambda(lambda) => {
                    self.function(
                        name,
                        Callable::Lambda(lambda, bound.name_range),
                        site,
                        frames,
                        test_attr,
                    );
                }
                Expr::Name(_) | Expr::Attribute(_) => {
                    let wanted = self.is_test_function_name(name)
                        || self.is_test_class_name(name)
                        || test_attr == Flag::Set(true);
                    match lookup.resolve(expr) {
                        Resolved::Local(target) if !target.conditional => match target.binding {
                            Binding::Function(def) => {
                                self.function(name, Callable::Def(def), site, frames, test_attr);
                            }
                            Binding::Class(class) => self.class(name, class, site, frames),
                            Binding::Value(Expr::Lambda(lambda)) => self.function(
                                name,
                                Callable::Lambda(lambda, target.name_range),
                                site,
                                frames,
                                test_attr,
                            ),
                            _ if wanted && !is_literal(expr) => {
                                self.doubt(format!("{name} is bound at runtime"));
                            }
                            _ => {}
                        },
                        Resolved::Path(path) if wanted => {
                            if (!in_module || self.settings.collect_imported_tests)
                                && !no_tests(&path)
                            {
                                self.doubt(format!(
                                    "{name} is bound to {path}, which may be a test"
                                ));
                            }
                        }
                        Resolved::Path(path) => {
                            if !in_module || self.settings.collect_imported_tests {
                                if self.may_be_project_unittest(name, &path) {
                                    self.doubt(format!(
                                        "{name} is bound to {path}, which may be a unittest case"
                                    ));
                                } else if self.imports.project_test(&path) {
                                    self.doubt(format!(
                                        "{name} is bound to {path}, whose __test__ may be true"
                                    ));
                                }
                            }
                        }
                        _ if wanted => self.doubt(format!("{name} is bound at runtime")),
                        _ => {}
                    }
                }
                other => {
                    let class_name = self.is_test_class_name(name);
                    let wanted = class_name
                        || self.is_test_function_name(name)
                        || matches!(test_attr, Flag::Set(true) | Flag::Computed);
                    // pytest collects a bound object only if it is a
                    // function (or a class, for a class name).
                    let instance = !class_name && self.makes_instance(lookup, other);
                    if wanted && !is_literal(other) && !instance {
                        self.doubt(format!("{name} is bound at runtime"));
                    } else if !wanted && let Some(callee) = self.may_make_class(lookup, other) {
                        // A unittest case, or a class whose `__test__` is
                        // true: pytest collects it whatever its name.
                        self.doubt(format!(
                            "{name} is bound to what {callee}(…) returns, which may be a test class"
                        ));
                    }
                }
            },
            Binding::Import(path) => {
                if in_module && !self.settings.collect_imported_tests {
                    return;
                }
                let wanted = self.is_test_function_name(name)
                    || self.is_test_class_name(name)
                    || name.ends_with("TestCase");
                // `FunctionTestCase` brings a `runTest`, so pytest collects
                // it wherever it is bound.
                if (wanted && !no_tests(path)) || UNITTEST_RUNTEST.contains(&path.as_str()) {
                    self.doubt(format!("{name} is imported from {path} and may be a test"));
                } else if self.may_be_project_unittest(name, path) {
                    // pytest collects a unittest case whatever its name.
                    self.doubt(format!(
                        "{name} is imported from {path}, which may be a unittest case"
                    ));
                } else if !wanted && self.imports.project_test(path) {
                    // And anything whose `__test__` is true.
                    self.doubt(format!(
                        "{name} is imported from {path}, whose __test__ may be true"
                    ));
                }
            }
            Binding::Other => {
                if self.is_test_function_name(name) || self.is_test_class_name(name) {
                    self.doubt(format!("{name} is bound at runtime"));
                }
            }
        }
    }

    // ------------------------------------------------------------ classes

    /// The bases of a class, resolved where the class statement runs.
    /// `lenient` leaves out bases from elsewhere that are no unittest case
    /// and set no `__test__` (as a guess of what pytest collects).
    fn bases(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        lenient: bool,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        let lookup = self.lookup(site, class.start());
        let mut out = Vec::new();
        let mut error: Option<Unresolved> = None;
        let mut fail = |reason: String, harmless: bool| match &mut error {
            Some(found) => found.harmless &= harmless,
            None => error = Some(Unresolved { reason, harmless }),
        };
        let Some(arguments) = &class.arguments else {
            return Ok(out);
        };
        'bases: for base in &*arguments.args {
            let target = match base {
                Expr::Subscript(sub) => &*sub.value,
                other => other,
            };
            if base.is_starred_expr() {
                fail(
                    format!("{} has bases computed at runtime", class.name),
                    false,
                );
                continue;
            }
            if let Expr::Name(name) = target {
                // The binding the name has when the class statement runs.
                let ns = site.map_or(self.module, |c| self.class_ns(c));
                let scopes = [Some(ns), (site.is_some()).then_some(self.module)];
                let mut found = None;
                for ns in scopes.into_iter().flatten() {
                    let before: Vec<_> = ns
                        .bindings(name.id.as_str())
                        .iter()
                        .filter(|b| b.range.end() <= class.start())
                        .collect();
                    if let Some(last) = before.last() {
                        found = Some(((*last).clone(), before.iter().any(|b| b.conditional)));
                        break;
                    }
                }
                if let Some((bound, conditional)) = found {
                    match bound.binding {
                        Binding::Class(def) if !conditional => {
                            out.push(Klass::Local(def));
                            continue;
                        }
                        Binding::Class(_) => {
                            fail(
                                format!(
                                    "{} inherits from {}, defined conditionally",
                                    class.name, name.id
                                ),
                                false,
                            );
                            continue 'bases;
                        }
                        Binding::Import(_) => {}
                        _ => {
                            fail(
                                format!(
                                    "{} inherits from {}, bound at runtime",
                                    class.name, name.id
                                ),
                                false,
                            );
                            continue 'bases;
                        }
                    }
                }
            }
            match lookup.path(target) {
                Some(path) if external_kind(&path).is_some() => out.push(Klass::External(path)),
                Some(path) => {
                    let harmless =
                        !self.imports.may_be_unittest(&path) && !self.imports.project_test(&path);
                    if lenient && harmless {
                        continue;
                    }
                    fail(
                        format!(
                            "{} inherits from {path}, defined outside the module",
                            class.name
                        ),
                        harmless,
                    );
                }
                None => fail(
                    format!("{} has bases computed at runtime", class.name),
                    false,
                ),
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(out),
        }
    }

    /// The class's C3 linearization, through classes of this module and
    /// known external ones.
    fn mro(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        depth: u8,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        self.mro_of(class, site, depth, false)
    }

    /// [`ModuleRecognizer::mro`], `lenient` as in
    /// [`ModuleRecognizer::bases`].
    fn mro_of(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        depth: u8,
        lenient: bool,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        if depth > 32 {
            return Err(Unresolved {
                reason: format!("{} has too deep a hierarchy", class.name),
                harmless: false,
            });
        }
        let bases = self.bases(class, site, lenient)?;
        let mut sequences: Vec<Vec<Klass<'a>>> = Vec::new();
        for base in &bases {
            sequences.push(match base {
                Klass::Local(def) => {
                    let site = self.site_of(def);
                    self.mro_of(def, site, depth + 1, lenient)?
                }
                Klass::External(path) if path == "builtins.object" => vec![base.clone()],
                Klass::External(_) => vec![base.clone(), Klass::object()],
            });
        }
        sequences.push(bases.clone());
        let mut out = vec![Klass::Local(class)];
        loop {
            sequences.retain(|s| !s.is_empty());
            if sequences.is_empty() {
                break;
            }
            let head = sequences.iter().map(|s| s[0].clone()).find(|candidate| {
                let key = candidate.key();
                sequences
                    .iter()
                    .all(|s| s[1..].iter().all(|k| k.key() != key))
            });
            let Some(head) = head else {
                return Err(Unresolved {
                    reason: format!("{} has an inconsistent MRO", class.name),
                    harmless: false,
                });
            };
            let key = head.key();
            out.push(head);
            for sequence in &mut sequences {
                if sequence[0].key() == key {
                    sequence.remove(0);
                }
            }
        }
        if !out.iter().any(|k| k.key() == "builtins.object") {
            out.push(Klass::object());
        }
        Ok(out)
    }

    /// The class statement starting at `start`.
    fn class_def(&self, start: TextSize) -> Option<&'a StmtClassDef> {
        self.classes.get(&start).map(|info| info.def)
    }

    /// The class body a class statement sits in, if any.
    fn site_of(&self, class: &'a StmtClassDef) -> Option<&'a StmtClassDef> {
        self.classes.get(&class.start()).and_then(|info| info.site)
    }

    /// The first binding of `name` along the MRO, with the class whose body
    /// holds it.
    fn lookup_mro(&self, mro: &[Klass<'a>], name: &str) -> Option<(Bound<'a>, &'a StmtClassDef)> {
        for klass in mro {
            if let Klass::Local(def) = klass
                && let Some(bound) = self.class_ns(def).get(name)
            {
                return Some((bound.clone(), def));
            }
        }
        None
    }

    fn decorators(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
    ) -> Vec<Decorator<'s, 'a>> {
        let lookup = self.lookup(site, class.start());
        class
            .decorator_list
            .iter()
            .rev()
            .map(|d| decorator(lookup, &d.expression))
            .collect()
    }

    /// A namespace's `pytestmark` once its body has run: the value bound
    /// last, then what `.append`, `.extend` and `+=` add to it. `Err` when
    /// pytest would fail on it.
    fn pytestmark(
        &self,
        ns: &'s Namespace<'a>,
        class: Option<&'a StmtClassDef>,
    ) -> Result<MarkList<'s, 'a>, String> {
        let ops: Vec<_> = ns.ops.iter().filter(|op| op.name == "pytestmark").collect();
        let Some(bound) = ns.get("pytestmark") else {
            return Ok(MarkList {
                marks: Vec::new(),
                unknown: ns.dynamic || !ops.is_empty(),
            });
        };
        let mut list = match bound.binding {
            Binding::Value(expr) => {
                let lookup = self.lookup(class, bound.range.end());
                let list = marks::pytestmark(lookup, expr)?;
                let (value, _) = marks::deref(lookup, expr);
                // Only a list grows in place.
                let grows = ops.iter().any(|op| op.at > bound.range.start());
                if grows && !matches!(value, Expr::List(_)) {
                    return Ok(MarkList {
                        marks: list.marks,
                        unknown: true,
                    });
                }
                list
            }
            _ => MarkList {
                marks: Vec::new(),
                unknown: true,
            },
        };
        if ns.dynamic || ns.globals.contains("pytestmark") {
            list.unknown = true;
        }
        for op in ops.into_iter().filter(|op| op.at > bound.range.start()) {
            let lookup = self.lookup(class, op.at);
            if op.conditional {
                list.unknown = true;
                continue;
            }
            match op.kind {
                OpKind::Assign(_) => {}
                OpKind::Append(expr) => match marks::mark(lookup, expr) {
                    Ok(Some(mark)) => list.marks.push(mark),
                    _ => list.unknown = true,
                },
                OpKind::Extend(expr) | OpKind::AugAdd(expr) => {
                    let more = marks::mark_collection(lookup, expr);
                    list.marks.extend(more.marks);
                    list.unknown |= more.unknown;
                }
                OpKind::Mutate => list.unknown = true,
            }
        }
        Ok(list)
    }

    /// A class's marks, considering its MRO (`get_unpacked_marks`): for
    /// each class from the most basic, its `pytestmark`, then its mark
    /// decorators, innermost first; whether some could not be read; and
    /// whether a decorator aye can't see through applies to it, which may
    /// add, remove or rename its tests. `Err` when pytest would fail on
    /// them.
    fn class_marks(&self, mro: &[Klass<'a>]) -> Result<(Vec<Mark<'s, 'a>>, bool, bool), String> {
        let mut marks = Vec::new();
        let mut unknown = false;
        let mut opaque = false;
        for klass in mro.iter().rev() {
            let Klass::Local(def) = klass else {
                continue;
            };
            let ns = self.class_ns(def);
            let list = self
                .pytestmark(ns, Some(def))
                .map_err(|reason| format!("{} {reason}", def.name))?;
            marks.extend(list.marks);
            unknown |= list.unknown;
            let site = self.site_of(def);
            for d in self.decorators(def, site) {
                match d {
                    Decorator::Mark(mark) => marks.push(mark),
                    Decorator::Unknown => {
                        unknown = true;
                        opaque = true;
                    }
                    Decorator::Keeps => unknown = true,
                    _ => {}
                }
            }
        }
        Ok((marks, unknown, opaque))
    }

    /// Why the classes of `mro` may hold other tests than their bodies
    /// say: a decorator aye can't see through, code that binds names in a
    /// class body (`exec`, `locals()`), a metaclass, a base's
    /// `__init_subclass__`, or attributes the module sets on them.
    fn dynamic_members(&self, mro: &[Klass<'a>], opaque: bool) -> Option<String> {
        if opaque {
            return Some("a class decorator aye can't see through applies".into());
        }
        for (i, klass) in mro.iter().enumerate() {
            let Klass::Local(def) = klass else {
                continue;
            };
            let name = def.name.id.as_str();
            let ns = self.class_ns(def);
            if ns.dynamic {
                return Some(format!("{name}'s body binds names at runtime"));
            }
            let site = self.site_of(def);
            let metaclass = def.arguments.as_ref().and_then(|args| {
                args.keywords
                    .iter()
                    .find(|k| k.arg.as_ref().is_some_and(|a| a.id.as_str() == "metaclass"))
            });
            if let Some(keyword) = metaclass
                && self
                    .lookup(site, def.start())
                    .path(&keyword.value)
                    .as_deref()
                    != Some("abc.ABCMeta")
            {
                return Some(format!("{name} has a metaclass"));
            }
            if i > 0 && ns.get("__init_subclass__").is_some() {
                return Some(format!("{name} defines __init_subclass__"));
            }
            if self.module.class_handed(def) {
                return Some(format!(
                    "{name} is handed to code aye can't follow, which may add or remove its tests"
                ));
            }
            // `__set_name__` gets the class whose body holds the value.
            if self.sets_names
                && ns.iter().any(
                    |(_, bound)| matches!(bound.binding, Binding::Value(expr) if !is_literal(expr)),
                )
            {
                return Some(format!(
                    "{name} holds values whose __set_name__ may change it"
                ));
            }
            let set = self.module.attributes.get(name).into_iter().flatten();
            for attribute in set {
                // `__bases__`, `__init__`, `__getattr__` and the like change
                // what pytest finds; `__test__` is read.
                let dunder = attribute.len() > 4
                    && attribute.starts_with("__")
                    && attribute.ends_with("__")
                    && !matches!(
                        *attribute,
                        "__test__" | "__doc__" | "__name__" | "__qualname__" | "__module__"
                    );
                if *attribute == "pytestmark"
                    || *attribute == "runTest"
                    || dunder
                    || self.is_test_function_name(attribute)
                    || self.is_test_class_name(attribute)
                {
                    return Some(format!("the module sets {name}.{attribute}"));
                }
            }
        }
        None
    }

    /// Whether a call surely makes something pytest doesn't collect under
    /// a test function's name: no function, nor a callable that wraps one
    /// (`functools.partial`, `update_wrapper`). Builtins that return data,
    /// and classes of the module that are not callable and make their own
    /// instances (no `__new__`, `__call__` or metaclass along a known MRO).
    fn makes_instance(&self, lookup: Lookup<'s, 'a>, expr: &'a Expr) -> bool {
        const DATA: &[&str] = &[
            "builtins.dict",
            "builtins.list",
            "builtins.set",
            "builtins.frozenset",
            "builtins.tuple",
            "builtins.str",
            "builtins.bytes",
            "builtins.int",
            "builtins.float",
            "builtins.bool",
            "builtins.range",
            "builtins.len",
            "builtins.sorted",
            "builtins.open",
        ];
        let Expr::Call(call) = expr else {
            return false;
        };
        if lookup
            .path(&call.func)
            .is_some_and(|path| DATA.contains(&path.as_str()))
        {
            return true;
        }
        let Resolved::Local(Bound {
            binding: Binding::Class(class),
            conditional: false,
            ..
        }) = lookup.resolve(&call.func)
        else {
            return false;
        };
        let Ok(mro) = self.mro(class, self.site_of(class), 0) else {
            return false;
        };
        mro.iter().all(|klass| match klass {
            Klass::Local(def) => {
                let ns = self.class_ns(def);
                def.arguments
                    .as_ref()
                    .is_none_or(|args| args.keywords.is_empty())
                    && ns.get("__new__").is_none()
                    && ns.get("__call__").is_none()
                    && !ns.dynamic
                    && !self.module.class_handed(def)
            }
            Klass::External(path) => !matches!(
                external_kind(path),
                Some(External::UnitTest | External::Protocol) | None
            ),
        })
    }

    /// The callee, when a call may make a class pytest collects whatever
    /// the name it is bound to: a unittest case or a class whose
    /// `__test__` is true. `type(…)` with three arguments,
    /// `types.new_class`, and functions of the module or of project files
    /// (methods of the module's classes included) that make classes.
    fn may_make_class(&self, lookup: Lookup<'s, 'a>, expr: &'a Expr) -> Option<String> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let path = lookup.path(&call.func);
        let arguments = call.arguments.args.len() + call.arguments.keywords.len();
        match path.as_deref() {
            Some("builtins.type") if arguments >= 3 => return Some("type".into()),
            Some("types.new_class") => return Some("types.new_class".into()),
            _ => {}
        }
        if let Some(path) = path {
            return self.imports.project_factory(&path).then_some(path);
        }
        let function = match lookup.resolve(&call.func) {
            Resolved::Local(Bound {
                binding: Binding::Function(def),
                ..
            }) => Some(def),
            Resolved::Local(Bound {
                binding: Binding::Value(Expr::Lambda(lambda)),
                name_range,
                ..
            }) => {
                return ClassMaker::expr(&lambda.body)
                    .then(|| self.file.source[name_range].to_string());
            }
            // `Base.make(…)`: a method of one of the module's classes.
            _ => match &*call.func {
                Expr::Attribute(attr) => match lookup.resolve(&attr.value) {
                    Resolved::Local(Bound {
                        binding: Binding::Class(class),
                        ..
                    }) => self
                        .mro(class, self.site_of(class), 0)
                        .ok()
                        .and_then(|mro| self.lookup_mro(&mro, attr.attr.id.as_str()))
                        .and_then(|(bound, _)| match bound.binding {
                            Binding::Function(def) => Some(def),
                            _ => None,
                        }),
                    _ => None,
                },
                _ => None,
            },
        };
        function
            .filter(|def| makes_classes(&def.body))
            .map(|def| def.name.to_string())
    }

    fn is_unittest(mro: &[Klass<'a>]) -> bool {
        mro.iter().any(|k| {
            matches!(k, Klass::External(path)
                if UNITTEST_BASES.contains(&path.as_str())
                    || UNITTEST_RUNTEST.contains(&path.as_str()))
        })
    }

    /// `__test__` along the MRO: set in a class body, or on a class of
    /// the module after its body (`K.__test__ = …`), which wins over the
    /// body's.
    fn dunder_test(&self, mro: &[Klass<'a>]) -> Flag {
        for klass in mro {
            let Klass::Local(def) = klass else {
                continue;
            };
            if self.site_of(def).is_none() {
                match self.module.test_attr(def.name.id.as_str()) {
                    Flag::Unset => {}
                    flag => return flag,
                }
            }
            if let Some(bound) = self.class_ns(def).get("__test__") {
                let value = match bound.binding {
                    Binding::Value(expr) => self.lookup(Some(def), bound.range.end()).boolean(expr),
                    _ => None,
                };
                return value.map_or(Flag::Computed, Flag::Set);
            }
        }
        Flag::Unset
    }

    fn class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
    ) {
        let mro = match self.mro(class, site, 0) {
            Ok(mro) => mro,
            Err(Unresolved { reason, harmless }) => {
                // Unknown bases: maybe unittest (whatever the class's name,
                // and with tests it only inherits), maybe more tests. Guess
                // from the class's own body.
                if self.is_test_class_name(name) {
                    self.doubt(reason);
                    let guess = vec![Klass::Local(class), Klass::object()];
                    let before = self.cases.len();
                    self.pytest_class(name, class, frames, &guess);
                    for case in &mut self.cases[before..] {
                        case.certainty = Certainty::Partial;
                    }
                } else if class_mentions_tests(class) || !harmless {
                    self.doubt(reason);
                } else if let Ok(guess) = self.mro_of(class, site, 0, true) {
                    // Only classes that hold no unittest case nor
                    // `__test__` are unknown: the bases aye knows decide
                    // whether pytest collects it, not which tests it has.
                    let unittest = Self::is_unittest(&guess) && !self.settings.unittest_blocked;
                    let dunder_test = self.dunder_test(&guess);
                    if unittest || dunder_test != Flag::Unset {
                        self.doubt(reason);
                        let before = self.cases.len();
                        if unittest {
                            self.unittest_class(name, class, frames, &guess);
                        } else if dunder_test == Flag::Set(true) {
                            self.pytest_class(name, class, frames, &guess);
                        }
                        for case in &mut self.cases[before..] {
                            case.certainty = Certainty::Partial;
                        }
                    }
                }
                return;
            }
        };
        if Self::is_unittest(&mro) {
            if self.settings.unittest_blocked {
                // With `-p no:unittest`, pytest's `Class` collector takes
                // the class, and `TestCase.__init__` stops it.
                if self.is_test_class_name(name) || self.dunder_test(&mro) == Flag::Set(true) {
                    let parent =
                        self.nodeid(&frames.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
                    self.diagnostic(
                        Severity::Warning,
                        "class-has-init",
                        class.name.range(),
                        format!(
                            "cannot collect test class '{}' because it has a __init__ constructor (from: {parent})",
                            class.name
                        ),
                    );
                }
                return;
            }
            self.unittest_class(name, class, frames, &mro);
            return;
        }
        let dunder_test = self.dunder_test(&mro);
        if !self.is_test_class_name(name) && dunder_test != Flag::Set(true) {
            if dunder_test == Flag::Computed {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
            } else if class_mentions_tests(class)
                && mro
                    .iter()
                    .any(|k| matches!(k, Klass::Local(def) if self.module.class_handed(def)))
            {
                // Code aye can't follow may set its `__test__`.
                self.doubt(format!(
                    "{name} is handed to code aye can't follow, which may make it a test class"
                ));
            }
            return;
        }
        self.pytest_class(name, class, frames, &mro);
    }

    fn is_abstract(&self, mro: &[Klass<'a>]) -> bool {
        let abcmeta = mro.iter().any(|k| match k {
            Klass::External(path) => {
                matches!(
                    external_kind(path),
                    Some(External::Abc | External::Protocol)
                )
            }
            Klass::Local(def) => def.arguments.as_ref().is_some_and(|args| {
                args.keywords.iter().any(|k| {
                    k.arg.as_ref().is_some_and(|a| a.id.as_str() == "metaclass")
                        && self
                            .lookup(self.site_of(def), def.start())
                            .path(&k.value)
                            .as_deref()
                            == Some("abc.ABCMeta")
                })
            }),
        });
        if !abcmeta {
            return false;
        }
        let mut names = BTreeSet::new();
        for klass in mro {
            if let Klass::Local(def) = klass {
                for (name, _) in self.class_ns(def).iter() {
                    names.insert(name);
                }
            }
        }
        names.into_iter().any(|name| {
            let Some((bound, def)) = self.lookup_mro(mro, name) else {
                return false;
            };
            let Binding::Function(func) = bound.binding else {
                return false;
            };
            let lookup = self.lookup(Some(def), func.start());
            func.decorator_list.iter().any(|d| {
                matches!(
                    decorator(lookup, &d.expression),
                    Decorator::Abstract | Decorator::AbstractProperty
                )
            })
        })
    }

    /// pytest's `Class` collector.
    fn pytest_class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        frames: &[Frame<'s, 'a>],
        mro: &[Klass<'a>],
    ) {
        if self.is_abstract(mro) {
            return;
        }
        match self.dunder_test(mro) {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let parent = self.nodeid(&frames.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
        for dunder in ["__init__", "__new__"] {
            let mut has = self.lookup_mro(mro, dunder).is_some();
            for klass in mro {
                match klass {
                    Klass::External(path) => {
                        let kind = external_kind(path);
                        has |= kind == Some(External::InitNew)
                            || (dunder == "__init__" && kind == Some(External::Protocol))
                            || (dunder == "__new__" && kind == Some(External::New));
                    }
                    Klass::Local(def) if dunder == "__init__" => {
                        let site = self.site_of(def);
                        has |= self
                            .decorators(def, site)
                            .iter()
                            .any(|d| matches!(d, Decorator::Init));
                    }
                    Klass::Local(_) => {}
                }
            }
            if has {
                self.diagnostic(
                    Severity::Warning,
                    "class-has-init",
                    class.name.range(),
                    format!(
                        "cannot collect test class '{}' because it has a {dunder} constructor (from: {parent})",
                        class.name
                    ),
                );
                return;
            }
        }
        let (marks, unknown, opaque) = match self.class_marks(mro) {
            Ok(found) => found,
            Err(reason) => {
                self.doubt(reason);
                (Vec::new(), true, false)
            }
        };
        let members_unknown = match self.dynamic_members(mro, opaque) {
            Some(reason) => {
                self.doubt(reason);
                true
            }
            None => false,
        };
        let mut frames = frames.to_vec();
        frames.push(Frame {
            name: name.to_string(),
            mro: mro.to_vec(),
            marks,
            marks_unknown: unknown,
        });
        let before = self.cases.len();

        // `PyCollector.collect`: each class's dict along the MRO, a name
        // only once (the most derived wins), base classes' items first.
        // The dict holds private names mangled (`__x` is `_K__x`).
        let mut seen: HashSet<String> = HashSet::new();
        let mut groups: Vec<Vec<(String, Bound<'a>, &'a StmtClassDef)>> = Vec::new();
        for klass in std::iter::once(&Klass::Local(class)).chain(mro.iter()) {
            let Klass::Local(def) = klass else {
                continue;
            };
            let ns = self.class_ns(def);
            let mut group = Vec::new();
            let entries = ns
                .iter()
                .map(|(n, b)| (n, b.clone()))
                .chain(maybe_deleted(ns, &def.body));
            for (member, bound) in entries {
                let key = mangle(member, def.name.id.as_str());
                if IGNORED_ATTRIBUTES.contains(&member) || !seen.insert(key.clone()) {
                    continue;
                }
                group.push((key, bound, *def));
            }
            groups.push(group);
        }
        self.duplicates(self.class_ns(class));
        for group in groups.into_iter().rev() {
            for (member, bound, def) in group {
                self.member(&member, &bound, Some(def), &frames, false);
            }
        }
        if unknown || members_unknown {
            for case in &mut self.cases[before..] {
                case.certainty = Certainty::Partial;
            }
        }
    }

    /// pytest's `UnitTestCase` collector: `TestLoader.getTestCaseNames`.
    fn unittest_class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        frames: &[Frame<'s, 'a>],
        mro: &[Klass<'a>],
    ) {
        match self.dunder_test(mro) {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let (marks, unknown, opaque) = match self.class_marks(mro) {
            Ok(found) => found,
            Err(reason) => {
                self.doubt(reason);
                (Vec::new(), true, false)
            }
        };
        let members_unknown = match self.dynamic_members(mro, opaque) {
            Some(reason) => {
                self.doubt(reason);
                true
            }
            None => false,
        };
        let before = self.cases.len();
        self.duplicates(self.class_ns(class));
        let mut frames = frames.to_vec();
        frames.push(Frame {
            name: name.to_string(),
            mro: mro.to_vec(),
            marks,
            // parametrize does not apply to unittest methods: unknown marks
            // only make tags uncertain.
            marks_unknown: false,
        });
        let mut names: BTreeSet<&'a str> = BTreeSet::new();
        for klass in mro {
            if let Klass::Local(def) = klass {
                let ns = self.class_ns(def);
                for (member, _) in ns.iter() {
                    if member.starts_with("test") {
                        names.insert(member);
                    }
                }
                for (member, _) in maybe_deleted(ns, &def.body) {
                    if member.starts_with("test") && names.insert(member) {
                        self.doubt(format!("{name}.{member} may be deleted at runtime"));
                    }
                }
            }
        }
        let mut found = Vec::new();
        for member in names {
            let Some((bound, def)) = self.lookup_mro(mro, member) else {
                continue;
            };
            match self.unittest_callable(member, &bound, def) {
                Attr::Test(callable) => found.push((member, callable, def)),
                Attr::NotTest => {}
                Attr::Unknown => self.doubt(format!("{name}.{member} is bound at runtime")),
            }
        }
        if found.is_empty()
            && let Some((bound, def)) = self.lookup_mro(mro, "runTest")
            && let Attr::Test(callable) = self.unittest_callable("runTest", &bound, def)
        {
            found.push(("runTest", callable, def));
        }
        // `unittest.FunctionTestCase` brings a `runTest` of its own.
        if found.is_empty()
            && self.lookup_mro(mro, "runTest").is_none()
            && mro.iter().any(
                |k| matches!(k, Klass::External(path) if UNITTEST_RUNTEST.contains(&path.as_str())),
            )
        {
            let certainty = if unknown || self.module_marks_unknown || self.marking_hooks() {
                Certainty::Partial
            } else {
                Certainty::Exact
            };
            let mut all = Vec::new();
            for frame in frames.iter().rev() {
                all.extend(frame.marks.iter().cloned());
            }
            all.extend(self.module_marks.iter().cloned());
            let item = Item {
                classes: frames.iter().map(|f| f.name.clone()).collect(),
                name: "runTest".to_string(),
                originalname: "runTest".to_string(),
                span: class.name.range(),
                marks: all,
                fn_marks: 0,
                param_marks: Vec::new(),
                param_timeout: SetTimeout::Unmarked,
                attributes: Vec::new(),
                certainty,
            };
            self.push(&item);
        }
        for (member, callable, def) in found {
            let lookup = self.lookup(Some(def), callable.span().start());
            let mut fn_marks = Vec::new();
            let mut certainty = if unknown {
                Certainty::Partial
            } else {
                Certainty::Exact
            };
            if let Callable::Def(func) = callable {
                let mut opaque = false;
                for d in func.decorator_list.iter().rev() {
                    match decorator(lookup, &d.expression) {
                        Decorator::Mark(mark) => fn_marks.push(mark),
                        Decorator::Unknown => opaque = true,
                        Decorator::Keeps => certainty = Certainty::Partial,
                        _ => {}
                    }
                }
                if opaque {
                    // `parameterized.expand` and the like replace the method
                    // with generated ones.
                    certainty = Certainty::Partial;
                    self.doubt(format!(
                        "{name}.{member} has a decorator aye can't see through"
                    ));
                }
            }
            if self
                .class_ns(def)
                .attributes
                .get(member)
                .is_some_and(|names| names.contains(&"pytestmark"))
            {
                certainty = Certainty::Partial;
            }
            let own = fn_marks.len();
            let mut all = fn_marks;
            for frame in frames.iter().rev() {
                all.extend(frame.marks.iter().cloned());
            }
            all.extend(self.module_marks.iter().cloned());
            if self.module_marks_unknown || self.marking_hooks() {
                certainty = Certainty::Partial;
            }
            let item = Item {
                classes: frames.iter().map(|f| f.name.clone()).collect(),
                name: member.to_string(),
                originalname: member.to_string(),
                span: callable.span(),
                marks: all,
                fn_marks: own,
                param_marks: Vec::new(),
                param_timeout: SetTimeout::Unmarked,
                attributes: self.attributes(member, callable, Some(def), lookup),
                certainty,
            };
            self.push(&item);
        }
        if members_unknown {
            for case in &mut self.cases[before..] {
                case.certainty = Certainty::Partial;
            }
        }
    }

    /// The names in a function's `__dict__` that `-k` matches.
    fn attributes(
        &self,
        name: &str,
        callable: Callable<'a>,
        site: Option<&'a StmtClassDef>,
        lookup: Lookup<'s, 'a>,
    ) -> Vec<String> {
        let mut out: Vec<String> = match callable {
            Callable::Def(def) => decorator_attributes(def, lookup)
                .into_iter()
                .map(str::to_string)
                .collect(),
            Callable::Lambda(..) => Vec::new(),
        };
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        for attribute in ns.attributes.get(name).into_iter().flatten() {
            if !out.iter().any(|a| a == attribute) {
                out.push((*attribute).to_string());
            }
        }
        out
    }

    /// Whether a unittest attribute is a callable test.
    fn unittest_callable(&self, name: &str, bound: &Bound<'a>, def: &'a StmtClassDef) -> Attr<'a> {
        let ns = self.class_ns(def);
        match ns.test_attr(name) {
            Flag::Set(false) => return Attr::NotTest,
            Flag::Computed => return Attr::Unknown,
            _ => {}
        }
        let lookup = self.lookup(Some(def), bound.range.start());
        match &bound.binding {
            Binding::Function(func) => {
                let property = func.decorator_list.iter().any(|d| {
                    matches!(
                        decorator(lookup, &d.expression),
                        Decorator::Property | Decorator::AbstractProperty
                    )
                });
                if property {
                    Attr::NotTest
                } else {
                    Attr::Test(Callable::Def(func))
                }
            }
            Binding::Value(Expr::Lambda(lambda)) => {
                Attr::Test(Callable::Lambda(lambda, bound.name_range))
            }
            Binding::Value(expr) if is_literal(expr) => Attr::NotTest,
            Binding::Value(expr) => match lookup.resolve(expr) {
                Resolved::Local(Bound {
                    binding: Binding::Function(func),
                    conditional: false,
                    ..
                }) => Attr::Test(Callable::Def(func)),
                _ => Attr::Unknown,
            },
            // A nested class is callable too, and imports could be anything.
            _ => Attr::Unknown,
        }
    }

    // ------------------------------------------------------------ functions

    /// `istestfunction` and `_genfunctions` for one callable.
    fn function(
        &mut self,
        name: &str,
        callable: Callable<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        test_attr: Flag,
    ) {
        if !self.is_test_function_name(name) && test_attr != Flag::Set(true) {
            if test_attr == Flag::Computed {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
            }
            return;
        }
        match test_attr {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let is_method = !frames.is_empty();
        let mut fn_marks = Vec::new();
        let mut certainty = Certainty::Exact;
        // Handed to code aye can't follow (`mark(test)`, `disable(test)`,
        // `vars(test)[…] = …`): its marks or `__test__` may change.
        let own_name = match callable {
            Callable::Def(def) => def.name.id.as_str(),
            Callable::Lambda(..) => name,
        };
        let handed = if site.is_some() {
            self.module.attribute_handed(name) || self.module.attribute_handed(own_name)
        } else {
            self.module.handed(name) || self.module.handed(own_name)
        };
        if handed {
            self.doubt(format!(
                "{name} is handed to code aye can't follow, which may change it"
            ));
            certainty = Certainty::Partial;
        }
        let mut staticmethod = false;
        // Whether something other than marks and `staticmethod` or
        // `classmethod` wraps the function: pytest then sees the wrapper.
        let mut wrapped = false;
        if let Callable::Def(def) = callable {
            let lookup = self.lookup(site, def.start());
            let mut opaque = false;
            for d in def.decorator_list.iter().rev() {
                match decorator(lookup, &d.expression) {
                    Decorator::Mark(mark) => fn_marks.push(mark),
                    Decorator::Fixture(_) | Decorator::Property | Decorator::AbstractProperty => {
                        return;
                    }
                    Decorator::StaticMethod => staticmethod = true,
                    Decorator::ClassMethod => {}
                    Decorator::Unknown => {
                        opaque = true;
                        wrapped = true;
                    }
                    Decorator::Keeps => {
                        certainty = Certainty::Partial;
                        wrapped = true;
                    }
                    _ => wrapped = true,
                }
            }
            if opaque {
                // It may parametrize, mark, rename, replace or multiply
                // the test.
                certainty = Certainty::Partial;
                self.doubt(format!("{name} has a decorator aye can't see through"));
            }
            if !def.is_async && is_generator(&def.body) {
                if wrapped {
                    // pytest checks the decorated object, which a wrapper
                    // (`functools.wraps`, `inlineCallbacks`, `gen_test`)
                    // makes a plain function.
                    self.doubt(format!(
                        "{name} is a generator wrapped by a decorator; pytest decides whether it is a test"
                    ));
                    certainty = Certainty::Partial;
                } else {
                    // pytest fails the module's collection ("'yield'
                    // keyword is allowed in fixtures, but not in tests");
                    // refinement reports its error, once.
                    self.doubt(format!(
                        "{name} is a generator, which pytest reports as a collection error"
                    ));
                    return;
                }
            }
        }
        // `test.pytestmark = …` replaces the marks the decorators left.
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        if ns
            .attributes
            .get(name)
            .is_some_and(|names| names.contains(&"pytestmark"))
        {
            certainty = Certainty::Partial;
            self.doubt(format!("{name}.pytestmark is assigned"));
        }
        // Marks that could not be read may parametrize.
        if self.module_marks_unknown {
            self.doubt("the module's pytestmark can't be read");
        }
        if let Some(frame) = frames.iter().rev().find(|f| f.marks_unknown) {
            self.doubt(format!("{}'s marks can't be read", frame.name));
        }
        let own = fn_marks.len();
        let attributes = self.attributes(
            name,
            callable,
            site,
            self.lookup(site, callable.span().start()),
        );
        let mut chain = fn_marks.clone();
        for frame in frames.iter().rev() {
            chain.extend(frame.marks.iter().cloned());
        }
        chain.extend(self.module_marks.iter().cloned());
        if self.module_marks_unknown {
            certainty = Certainty::Partial;
        }
        if self.marking_hooks() {
            certainty = Certainty::Partial;
        }

        let classes: Vec<String> = frames.iter().map(|f| f.name.clone()).collect();
        let base = Item {
            classes: classes.clone(),
            name: name.to_string(),
            originalname: name.to_string(),
            span: callable.span(),
            marks: chain.clone(),
            fn_marks: own,
            param_marks: Vec::new(),
            param_timeout: SetTimeout::Unmarked,
            attributes: attributes.clone(),
            certainty,
        };

        // Reasons ids can only be known at runtime.
        let mut runtime: Option<String> = None;
        if self.scope.hooks.contains("pytest_generate_tests") {
            runtime = Some(format!(
                "{} implements pytest_generate_tests",
                self.scope.hook_source("pytest_generate_tests")
            ));
        }
        if let Some(Bound {
            binding: Binding::Function(_) | Binding::Value(_) | Binding::Import(_),
            ..
        }) = self.module.get("pytest_generate_tests")
        {
            runtime = Some("the module defines pytest_generate_tests".into());
        }
        if let Some(frame) = frames.last()
            && self
                .lookup_mro(&frame.mro, "pytest_generate_tests")
                .is_some()
        {
            runtime = Some(format!("{} defines pytest_generate_tests", frame.name));
        }

        let parametrize_marks: Vec<&Mark<'s, 'a>> =
            chain.iter().filter(|m| m.name == "parametrize").collect();
        if !parametrize_marks.is_empty()
            && self
                .scope
                .global_hooks
                .contains("pytest_make_parametrize_id")
        {
            runtime = Some(format!(
                "{} implements pytest_make_parametrize_id",
                self.scope.global_hook_source("pytest_make_parametrize_id")
            ));
        }

        let mut parsed = Vec::new();
        if runtime.is_none() {
            for mark in &parametrize_marks {
                match marks::parametrize(mark, &self.settings.empty) {
                    Ok(p) => parsed.push(p),
                    Err(reason) => {
                        runtime = Some(format!("{name}: {reason}"));
                        break;
                    }
                }
            }
        }

        // The fixture closure: parametrized fixtures make ids pytest's
        // business; arguments parametrize does not cover are errors.
        if runtime.is_none() {
            let args = match callable {
                Callable::Def(def) => crate::support::funcargnames(def, is_method && !staticmethod),
                Callable::Lambda(lambda, _) => lambda_args(lambda, is_method),
            };
            let defaults = match callable {
                Callable::Def(def) => default_argnames(def),
                Callable::Lambda(..) => Vec::new(),
            };
            let covered: HashSet<&str> = parsed
                .iter()
                .flat_map(|p| p.argnames.iter().map(String::as_str))
                .collect();
            let direct: HashSet<&str> = parsed
                .iter()
                .flat_map(|p| p.direct().map(String::as_str))
                .collect();
            let (closure, verdict) = self.closure(&args, &chain, &direct, &covered, frames);
            match verdict {
                Closure::Fine => {}
                // A fixture imported from elsewhere may be parametrized.
                Closure::Imported => certainty = Certainty::Partial,
                Closure::Unreadable => {
                    runtime = Some(format!("{name}: usefixtures arguments are not literal"));
                }
                Closure::Parametrized(fixture) => {
                    runtime = Some(format!("{name} uses the parametrized fixture {fixture}"));
                }
                Closure::Opaque(fixture) => {
                    runtime = Some(format!(
                        "{name} uses {fixture}, a fixture defined in a way aye can't read"
                    ));
                }
            }
            if runtime.is_none() {
                let mut seen_args: HashSet<&str> = HashSet::new();
                'check: for p in &parsed {
                    for argname in &p.argnames {
                        if !seen_args.insert(argname.as_str()) {
                            runtime =
                                Some(format!("{name}: duplicate parametrization of {argname:?}"));
                            break 'check;
                        }
                        if !closure.contains(argname.as_str()) && !args.contains(argname) {
                            runtime = Some(if defaults.contains(argname) {
                                format!(
                                    "{name} already takes an argument {argname:?} with a default value"
                                )
                            } else {
                                format!("{name} uses no argument {argname:?}")
                            });
                            break 'check;
                        }
                    }
                }
            }
        }

        // Each parametrization's scope (`_find_parametrized_scope`):
        // explicit, else for an indirect-only one the narrowest of its
        // fixtures'. Class and module scopes make pytest reorder the file's
        // tests (`reorder_items`), which `reorder` models; package and
        // session scopes reorder across files.
        let mut scopes: Vec<Scoped> = Vec::new();
        if runtime.is_none() {
            let defs = self.fixture_defs(frames);
            for p in &parsed {
                let indirect_all =
                    !p.indirect.is_empty() && p.argnames.iter().all(|a| p.indirect.contains(a));
                let scope = match p.scope {
                    Some(scope) => scope,
                    None if indirect_all => p
                        .argnames
                        .iter()
                        .map(|a| {
                            defs.iter()
                                .rev()
                                .find(|d| d.name == *a)
                                .map_or(Scoped::Unknown, |d| d.scope)
                        })
                        .min()
                        .unwrap_or(Scoped::Function),
                    None => Scoped::Function,
                };
                if matches!(scope, Scoped::Package | Scoped::Session | Scoped::Unknown) {
                    runtime = Some(format!(
                        "{name}: parameters of a session-wide or unknown scope reorder tests at runtime"
                    ));
                    break;
                }
                scopes.push(scope);
            }
        }

        if let Some(reason) = runtime {
            self.doubt(reason);
            let mut item = base;
            item.certainty = Certainty::Partial;
            self.push(&item);
            return;
        }

        // `Metafunc.parametrize`, once per mark, each multiplying the calls.
        let mut calls: Vec<Call> = vec![(Vec::new(), Vec::new(), SetTimeout::Unmarked, Vec::new())];
        let mut any = false;
        let mut unknown_marks = false;
        let class_key = (!classes.is_empty()).then(|| self.nodeid(&classes));
        for (p, scope) in parsed.iter().zip(&scopes) {
            any = true;
            unknown_marks |= p.unknown_marks;
            let ids =
                match parameterset_ids(&p.argnames, &p.sets, p.ids.as_deref(), self.settings.ids) {
                    Ok(ids) => ids,
                    Err(error) => {
                        self.doubt(format!("{name}: {error}"));
                        let mut item = base;
                        item.certainty = Certainty::Partial;
                        self.push(&item);
                        return;
                    }
                };
            let mut next = Vec::with_capacity(calls.len() * p.sets.len());
            for (idlist, marks, timeout, keys) in &calls {
                for (index, ((id, set), set_timeout)) in
                    ids.iter().zip(&p.sets).zip(&p.timeouts).enumerate()
                {
                    let mut idlist = idlist.clone();
                    if let Some(id) = id {
                        idlist.push(id.clone());
                    }
                    let mut marks = marks.clone();
                    marks.extend(set.marks.iter().cloned());
                    let timeout = if *timeout == SetTimeout::Unmarked {
                        *set_timeout
                    } else {
                        *timeout
                    };
                    let mut keys = keys.clone();
                    if matches!(scope, Scoped::Class | Scoped::Module) {
                        for argname in &p.argnames {
                            let key = ParamKey {
                                scope: *scope,
                                argname: argname.clone(),
                                index,
                                class: (*scope == Scoped::Class)
                                    .then(|| class_key.clone())
                                    .flatten(),
                            };
                            keys.push((key, !p.indirect.contains(argname)));
                        }
                    }
                    next.push((idlist, marks, timeout, keys));
                }
            }
            calls = next;
        }
        // `_recompute_direct_params_indices`: a direct parameter's index is
        // its call's position, so only tests at the same position share it.
        for (position, call) in calls.iter_mut().enumerate() {
            for (key, direct) in &mut call.3 {
                if *direct {
                    key.index = position;
                }
            }
        }
        if !any {
            let mut item = base;
            item.certainty = certainty;
            self.push(&item);
            return;
        }
        for (idlist, param_marks, param_timeout, keys) in calls {
            let full = if idlist.is_empty() {
                name.to_string()
            } else {
                format!("{name}[{}]", idlist.join("-"))
            };
            let item = Item {
                classes: classes.clone(),
                name: full,
                originalname: name.to_string(),
                span: callable.span(),
                marks: chain.clone(),
                fn_marks: own,
                param_marks,
                param_timeout,
                attributes: attributes.clone(),
                certainty: if unknown_marks {
                    Certainty::Partial
                } else {
                    certainty
                },
            };
            self.push(&item);
            if let Some(last) = self.keys.last_mut() {
                *last = keys.into_iter().map(|(key, _)| key).collect();
            }
        }
    }

    /// The fixtures visible to a test, in registration order: plugins,
    /// conftests, the module, then each class (bases first). The last
    /// definition of a name is the closest.
    fn fixture_defs(&self, frames: &[Frame<'s, 'a>]) -> Vec<&FixtureDef> {
        let mut defs: Vec<&FixtureDef> = self.scope.fixtures.iter().collect();
        defs.extend(&self.module_fixtures);
        for frame in frames {
            for klass in frame.mro.iter().rev() {
                if let Klass::Local(def) = klass {
                    defs.extend(self.class_fixtures.get(&def.start()).into_iter().flatten());
                }
            }
        }
        defs
    }

    /// The names in a test's fixture closure that this file and its
    /// conftests define, and what that says about its ids.
    fn closure(
        &self,
        args: &[String],
        chain: &[Mark<'s, 'a>],
        direct: &HashSet<&str>,
        covered: &HashSet<&str>,
        frames: &[Frame<'s, 'a>],
    ) -> (HashSet<String>, Closure) {
        let defs = self.fixture_defs(frames);
        let mut verdict = Closure::Fine;
        let mut roots: Vec<String> = self.settings.usefixtures.clone();
        roots.extend(defs.iter().filter(|d| d.autouse).map(|d| d.name.clone()));
        for mark in chain.iter().filter(|m| m.name == "usefixtures") {
            let (names, unknown) = marks::usefixtures(mark);
            roots.extend(names);
            if unknown {
                verdict = Closure::Unreadable;
            }
        }
        roots.extend(args.iter().cloned());

        // A fixture registered under a name aye can't read may be any of
        // them.
        let any_name = defs.iter().any(|d| d.any_name);
        let mut closure: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = roots;
        while let Some(name) = queue.pop() {
            if !closure.insert(name.clone()) || name == "request" {
                continue;
            }
            if direct.contains(name.as_str()) {
                continue;
            }
            if any_name {
                return (closure, Closure::Opaque(name));
            }
            let matching: Vec<&FixtureDef> =
                defs.iter().copied().filter(|d| d.name == name).collect();
            if matching.is_empty() {
                // Defined by a plugin a conftest in scope loads from a
                // directory it puts on `sys.path` aye can't tell: it may be
                // a parametrized fixture.
                if self.scope.hidden_fixtures && !BUILTIN_FIXTURES.contains(&name.as_str()) {
                    return (closure, Closure::Opaque(name));
                }
                // Not defined where aye can read it: imported by the
                // module or a conftest in scope from somewhere else, it may
                // be a parametrized fixture.
                let imports = &self.scope.module_imports;
                let imported = imports.maybe.contains(&name)
                    || imports.any
                    || self.scope.maybe_fixtures.contains(&name)
                    || self.scope.any_fixture;
                if imported && matches!(verdict, Closure::Fine) {
                    verdict = Closure::Imported;
                }
                continue;
            }
            // Closest first; a definition that does not request the name
            // it overrides hides the ones further out.
            for def in matching.into_iter().rev() {
                if def.opaque && !covered.contains(name.as_str()) {
                    return (closure, Closure::Opaque(name));
                }
                if def.params && !covered.contains(name.as_str()) {
                    return (closure, Closure::Parametrized(name));
                }
                queue.extend(def.args.iter().filter(|a| **a != name).cloned());
                // One that may not be defined at runtime doesn't hide the
                // ones further out.
                if !def.args.contains(&name) && !def.conditional {
                    break;
                }
            }
        }
        (closure, verdict)
    }

    fn push(&mut self, item: &Item<'s, 'a>) {
        self.keys.push(Vec::new());
        let mut case = Case::new(
            format!("{}::{}", self.nodeid(&item.classes), item.name),
            "pytest",
        );
        case.span = Some(self.file.span(item.span));
        // `iter_markers` order: the function's own marks (its decorators,
        // then the parameter set's), the classes', the module's.
        let names = item.marks[..item.fn_marks]
            .iter()
            .map(|m| m.name.clone())
            .chain(item.param_marks.iter().cloned())
            .chain(item.marks[item.fn_marks..].iter().map(|m| m.name.clone()));
        let mut tags: Vec<String> = Vec::new();
        for name in names {
            if !tags.contains(&name) {
                tags.push(name);
            }
        }
        // pytest-timeout reads the closest `timeout` mark.
        let own = item.marks[..item.fn_marks]
            .iter()
            .find(|m| m.name == "timeout");
        let rest = || {
            item.marks[item.fn_marks..]
                .iter()
                .find(|m| m.name == "timeout")
        };
        case.timeout = match (own, item.param_timeout) {
            (Some(mark), _) => marks::timeout(mark),
            (None, SetTimeout::Seconds(seconds)) => Some(seconds),
            (None, SetTimeout::Unreadable) => None,
            (None, SetTimeout::Unmarked) => rest().and_then(marks::timeout),
        };
        let mut keywords: Vec<String> = Vec::new();
        let mut add = |k: &str| {
            if !keywords.iter().any(|x| x == k) {
                keywords.push(k.to_string());
            }
        };
        for k in self.path_keywords {
            add(k);
        }
        for class in &item.classes {
            add(class);
        }
        add(&item.originalname);
        add(&item.name);
        // Mark decorators store `pytestmark` in the function's `__dict__`.
        if item.fn_marks > 0 {
            add("pytestmark");
        }
        for attribute in &item.attributes {
            add(attribute);
        }
        for tag in &tags {
            add(tag);
        }
        case.keywords = keywords;
        case.tags = tags;
        case.affinity.push(self.file.rel.to_string());
        for i in 1..=item.classes.len() {
            case.affinity.push(self.nodeid(&item.classes[..i]));
        }
        case.certainty = item.certainty;
        self.cases.push(case);
    }

    /// `duplicate-definition` warnings for test names bound twice.
    fn duplicates(&mut self, ns: &'s Namespace<'a>) {
        let mut reported: Vec<(TextRange, String)> = Vec::new();
        for (name, _) in ns.iter() {
            let defs: Vec<&Bound<'a>> = ns
                .bindings(name)
                .iter()
                .filter(|b| matches!(b.binding, Binding::Function(_) | Binding::Class(_)))
                .collect();
            if defs.len() < 2 {
                continue;
            }
            let is_test = match defs.last().map(|b| &b.binding) {
                Some(Binding::Function(_)) => self.is_test_function_name(name),
                Some(Binding::Class(_)) => self.is_test_class_name(name),
                _ => false,
            };
            if !is_test {
                continue;
            }
            let last = defs[defs.len() - 1];
            let line = self.file.line(last.name_range.start());
            let deletions = ns.deletions.get(name).map_or(&[][..], Vec::as_slice);
            for earlier in &defs[..defs.len() - 1] {
                let deleted = deletions
                    .iter()
                    .any(|at| *at > earlier.range.start() && *at < last.range.start());
                if deleted || exclusive(earlier, last) {
                    continue;
                }
                reported.push((
                    earlier.name_range,
                    format!("{name} is defined again on line {line}; pytest only runs the last definition"),
                ));
            }
        }
        for (range, message) in reported {
            self.diagnostic(Severity::Warning, "duplicate-definition", range, message);
        }
    }
}

/// What decorators other than marks leave in a function's `__dict__`
/// (which `-k` matches): `__wrapped__` from `functools.wraps`, which
/// nearly all of them use, and `patchings` from `mock.patch`.
fn decorator_attributes<'a>(
    func: &'a StmtFunctionDef,
    lookup: Lookup<'_, 'a>,
) -> Vec<&'static str> {
    let mut out = Vec::new();
    for d in &func.decorator_list {
        match decorator(lookup, &d.expression) {
            Decorator::Mark(_) | Decorator::StaticMethod | Decorator::ClassMethod => {}
            _ => {
                if !out.contains(&"__wrapped__") {
                    out.push("__wrapped__");
                }
                let callee = match &d.expression {
                    Expr::Call(call) => &*call.func,
                    other => other,
                };
                let path = lookup.path(callee).unwrap_or_default();
                let is_patch = ["mock.patch", "unittest.mock.patch"].iter().any(|p| {
                    path == *p || path == format!("{p}.object") || path == format!("{p}.multiple")
                });
                if is_patch && !out.contains(&"patchings") {
                    out.push("patchings");
                }
            }
        }
    }
    out
}

/// A call `Metafunc.parametrize` builds: the ids, the parameter sets'
/// marks, their timeout, and its parameters of class or module scope, each
/// with whether it is direct (not through a fixture).
type Call = (Vec<String>, Vec<String>, SetTimeout, Vec<(ParamKey, bool)>);

/// pytest's `ParamArgKey` for a class- or module-scoped parameter: tests
/// sharing one share the parameter's setup, and pytest moves them
/// together. The file is implied (keys are per file).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ParamKey {
    scope: Scoped,
    argname: String,
    index: usize,
    /// For class scope, the class (its node id); `None` for functions
    /// outside classes.
    class: Option<String>,
}

/// pytest's `reorder_items` over one file's tests, given each test's
/// class- and module-scoped parameter keys: the order they run and are
/// listed in. Keys never span files at these scopes, so the file's order
/// is the session's, restricted to it.
fn reorder_items(keys: &[Vec<ParamKey>]) -> Vec<usize> {
    /// The keys of each item at a scope, and the items of each key.
    struct Index {
        by_item: Vec<Vec<ParamKey>>,
        by_key: HashMap<ParamKey, Vec<usize>>,
    }
    fn at_scope(items: &[usize], level: usize, indexes: &mut [Index]) -> Vec<usize> {
        if level >= indexes.len() || items.len() < 3 {
            return items.to_vec();
        }
        let members: HashSet<usize> = items.iter().copied().collect();
        let mut ignore: HashSet<ParamKey> = HashSet::new();
        let mut queue: std::collections::VecDeque<usize> = items.iter().copied().collect();
        let mut done: Vec<usize> = Vec::new();
        let mut done_set: HashSet<usize> = HashSet::new();
        while !queue.is_empty() {
            let mut no_key: Vec<usize> = Vec::new();
            let mut slicing: Option<ParamKey> = None;
            while let Some(item) = queue.pop_front() {
                if done_set.contains(&item) || no_key.contains(&item) {
                    continue;
                }
                let live: Vec<&ParamKey> = indexes[level].by_item[item]
                    .iter()
                    .filter(|k| !ignore.contains(*k))
                    .collect();
                // `dict.popitem()` takes the last one.
                let Some(key) = live.last().map(|k| (*k).clone()) else {
                    no_key.push(item);
                    continue;
                };
                let matching: Vec<usize> = indexes[level].by_key[&key]
                    .iter()
                    .copied()
                    .filter(|i| members.contains(i))
                    .collect();
                for &i in matching.iter().rev() {
                    queue.push_front(i);
                    // Keep every scope's lists in the new order.
                    for index in indexes.iter_mut() {
                        for k in index.by_item[i].clone() {
                            let list = index.by_key.get_mut(&k).expect("indexed");
                            list.retain(|x| *x != i);
                            list.insert(0, i);
                        }
                    }
                }
                slicing = Some(key);
                break;
            }
            if !no_key.is_empty() {
                for item in at_scope(&no_key, level + 1, indexes) {
                    if done_set.insert(item) {
                        done.push(item);
                    }
                }
            }
            if let Some(key) = slicing {
                ignore.insert(key);
            }
        }
        done
    }

    const SCOPES: [Scoped; 2] = [Scoped::Module, Scoped::Class];
    let mut indexes: Vec<Index> = SCOPES
        .iter()
        .map(|scope| {
            let mut by_item = Vec::with_capacity(keys.len());
            let mut by_key: HashMap<ParamKey, Vec<usize>> = HashMap::new();
            for (item, item_keys) in keys.iter().enumerate() {
                let mut own: Vec<ParamKey> = Vec::new();
                for key in item_keys.iter().filter(|k| k.scope == *scope) {
                    if !own.contains(key) {
                        own.push(key.clone());
                        by_key.entry(key.clone()).or_default().push(item);
                    }
                }
                by_item.push(own);
            }
            Index { by_item, by_key }
        })
        .collect();

    let all: Vec<usize> = (0..keys.len()).collect();
    at_scope(&all, 0, &mut indexes)
}

/// What a fixture closure says about a test's ids.
enum Closure {
    Fine,
    /// It uses a name imported from another module, maybe a fixture.
    Imported,
    /// `usefixtures` names it with something other than literals.
    Unreadable,
    Parametrized(String),
    /// A fixture in the closure is defined in a way aye can't read.
    Opaque(String),
}

fn lambda_args(lambda: &ExprLambda, is_method: bool) -> Vec<String> {
    let Some(params) = &lambda.parameters else {
        return Vec::new();
    };
    let mut names: Vec<String> = params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_none())
        .map(|p| p.parameter.name.id.to_string())
        .collect();
    if is_method && !names.is_empty() {
        names.remove(0);
    }
    names
}

/// Whether an expression is a literal that is not callable.
fn is_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::FString(_)
            | Expr::List(_)
            | Expr::Tuple(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_)
            | Expr::Generator(_)
            | Expr::Compare(_)
    )
}

/// Whether a body (a function's, nested ones included) may make a class
/// pytest collects whatever its name: a class statement, `type(…)` with
/// three arguments, `types.new_class`, or a `__test__` it sets.
pub(crate) fn makes_classes(body: &[Stmt]) -> bool {
    use aye_engine::ast::visitor::Visitor;

    let mut finder = ClassMaker(false);
    for stmt in body {
        finder.visit_stmt(stmt);
    }
    finder.0
}

/// The walk behind [`makes_classes`].
struct ClassMaker(bool);

impl ClassMaker {
    /// [`makes_classes`] for an expression (a lambda's body).
    fn expr(expr: &Expr) -> bool {
        use aye_engine::ast::visitor::Visitor;

        let mut finder = Self(false);
        finder.visit_expr(expr);
        finder.0
    }
}

impl<'a> aye_engine::ast::visitor::Visitor<'a> for ClassMaker {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        if matches!(stmt, Stmt::ClassDef(_)) {
            self.0 = true;
        }
        if !self.0 {
            aye_engine::ast::visitor::walk_stmt(self, stmt);
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call)
                if matches!(&*call.func, Expr::Name(n) if n.id.as_str() == "type")
                    && call.arguments.args.len() + call.arguments.keywords.len() >= 3 =>
            {
                self.0 = true;
            }
            Expr::Attribute(attr) if matches!(attr.attr.id.as_str(), "new_class" | "__test__") => {
                self.0 = true;
            }
            Expr::Name(name) if name.id.as_str() == "__test__" => self.0 = true,
            _ => {}
        }
        if !self.0 {
            aye_engine::ast::visitor::walk_expr(self, expr);
        }
    }
}

/// Whether `path` names one of the unittest base classes pytest's unittest
/// plugin knows.
pub(crate) fn is_unittest_base(path: &str) -> bool {
    UNITTEST_BASES.contains(&path) || UNITTEST_RUNTEST.contains(&path)
}

/// Whether names imported from `path` are known not to be tests (nor
/// fixtures or hooks).
pub(crate) fn no_tests(path: &str) -> bool {
    let top = path.split('.').next().unwrap_or_default();
    NO_TESTS.contains(&top) || UNITTEST_BASES.contains(&path)
}

/// What a module or class body may skip at runtime.
struct BodyFacts<'a> {
    /// [`conditional_deletes`].
    deletes: Vec<(&'a str, TextSize)>,
    /// [`guarded_by_try`].
    guarded: Vec<TextSize>,
}

/// Whether a statement of a module or class body surely raises nothing
/// while it runs.
fn cannot_raise(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::FunctionDef(def) => def.decorator_list.is_empty(),
        Stmt::ClassDef(class) => class.decorator_list.is_empty() && class.arguments.is_none(),
        Stmt::Pass(_) => true,
        Stmt::Expr(expr) => expr.value.is_string_literal_expr(),
        _ => false,
    }
}

/// The statements of a `try` body (with `except` clauses) or a `with`
/// body, each with whether an earlier one may have raised and been
/// swallowed, so that it never runs: any context manager may suppress
/// what its body raises (`contextlib.suppress`, `pytest.raises`).
fn swallowed(stmt: &Stmt) -> Option<Vec<(&Stmt, bool)>> {
    let (body, handled) = match stmt {
        Stmt::Try(s) => (&s.body, !s.handlers.is_empty()),
        Stmt::With(s) => (&s.body, true),
        _ => return None,
    };
    let mut risky = false;
    let mut out = Vec::with_capacity(body.len());
    for inner in body {
        out.push((inner, risky));
        risky |= handled && !cannot_raise(inner);
    }
    Some(out)
}

/// `del name` statements in `body` (a module or class body, not its
/// functions) that may not run: inside an `if`, a loop, a `match`, an
/// `except` or `else` clause, or a `try` or `with` body after a statement
/// that may raise. With where each one is.
fn conditional_deletes(body: &[Stmt]) -> Vec<(&str, TextSize)> {
    fn walk<'a>(body: &'a [Stmt], conditional: bool, out: &mut Vec<(&'a str, TextSize)>) {
        for stmt in body {
            match stmt {
                Stmt::Delete(delete) if conditional => {
                    for target in &delete.targets {
                        if let Expr::Name(name) = target {
                            out.push((name.id.as_str(), stmt.start()));
                        }
                    }
                }
                Stmt::If(s) => {
                    walk(&s.body, true, out);
                    for clause in &s.elif_else_clauses {
                        walk(&clause.body, true, out);
                    }
                }
                Stmt::Try(s) => {
                    for (inner, risky) in swallowed(stmt).unwrap_or_default() {
                        walk(std::slice::from_ref(inner), conditional || risky, out);
                    }
                    for handler in &s.handlers {
                        if let Some(handler) = handler.as_except_handler() {
                            walk(&handler.body, true, out);
                        }
                    }
                    walk(&s.orelse, true, out);
                    walk(&s.finalbody, conditional, out);
                }
                Stmt::With(_) => {
                    for (inner, risky) in swallowed(stmt).unwrap_or_default() {
                        walk(std::slice::from_ref(inner), conditional || risky, out);
                    }
                }
                Stmt::For(s) => {
                    walk(&s.body, true, out);
                    walk(&s.orelse, true, out);
                }
                Stmt::While(s) => {
                    walk(&s.body, true, out);
                    walk(&s.orelse, true, out);
                }
                Stmt::Match(s) => {
                    for case in &s.cases {
                        walk(&case.body, true, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(body, false, &mut out);
    out
}

/// Where statements start, in `body` (a module or class body), that sit in
/// a `try` body with `except` clauses, or a `with` body, after a statement
/// that may raise (an import, a call): if it raises and the exception is
/// swallowed, they never run.
pub(crate) fn guarded_by_try(body: &[Stmt]) -> Vec<TextSize> {
    fn walk(body: &[Stmt], guarded: bool, out: &mut Vec<TextSize>) {
        for stmt in body {
            if guarded {
                out.push(stmt.start());
            }
            match stmt {
                Stmt::Try(s) => {
                    for (inner, risky) in swallowed(stmt).unwrap_or_default() {
                        walk(std::slice::from_ref(inner), guarded || risky, out);
                    }
                    walk(&s.orelse, guarded, out);
                    walk(&s.finalbody, guarded, out);
                }
                Stmt::With(_) => {
                    for (inner, risky) in swallowed(stmt).unwrap_or_default() {
                        walk(std::slice::from_ref(inner), guarded || risky, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(body, false, &mut out);
    out
}

/// Names a conditional `del` took out of `ns` that may still be bound at
/// runtime, with the binding the `del` would have removed.
fn maybe_deleted<'a>(ns: &Namespace<'a>, body: &'a [Stmt]) -> Vec<(&'a str, Bound<'a>)> {
    let mut out: Vec<(&'a str, Bound<'a>)> = Vec::new();
    for (name, at) in conditional_deletes(body) {
        if ns.get(name).is_some() || out.iter().any(|(n, _)| *n == name) {
            continue;
        }
        if let Some(bound) = ns
            .bindings(name)
            .iter()
            .rev()
            .find(|b| b.range.start() < at)
        {
            out.push((name, bound.clone()));
        }
    }
    out
}

/// A name bound in the body of the class `class` as Python stores it:
/// `__x` (not a dunder) becomes `_K__x`, `K` being the class's name
/// without its leading underscores (unless that leaves nothing).
fn mangle(name: &str, class: &str) -> String {
    let stripped = class.trim_start_matches('_');
    if name.starts_with("__") && !name.ends_with("__") && !stripped.is_empty() {
        format!("_{stripped}{name}")
    } else {
        name.to_string()
    }
}

/// Whether a class body defines anything that looks like a test, so that
/// not knowing its bases matters (it may be a unittest class).
fn class_mentions_tests(class: &StmtClassDef) -> bool {
    class.body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(def) => {
            def.name.id.as_str().starts_with("test") || def.name.id.as_str() == "runTest"
        }
        _ => false,
    })
}
