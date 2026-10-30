//! What `conftest.py` files (and the plugin modules they name in
//! `pytest_plugins`, register or import from) do to collection: hooks,
//! fixtures, `collect_ignore`, the modules they change and whether they
//! reach module namespaces; and what project modules define, for telling
//! whether a name a test module imports may be a unittest case.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{Expr, ExprCall, ModModule, Stmt, StmtFunctionDef};
use aye_adapter::{Ranged, SourceFile, TextSize};
use aye_events::Fnv64;
use aye_python_semantic::class::is_unittest_base;
use aye_python_semantic::eval::Elem;
use aye_python_semantic::flow::{exclusive, guarded_by_try, import_time_skip};
use aye_python_semantic::module_resolver::{package_base, resolve_from};
use aye_python_semantic::namespace::{Binding, Bound, Flag, Namespace, OpKind};
use aye_python_semantic::resolve::{Lookup, Resolved};
use aye_python_semantic::sys_path::{SysPath, SysPathVisitor};

use crate::marks::{Decorator, Scoped, decorator};

/// A fixture definition, as far as parametrization is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureDef {
    /// The name pytest registers it under.
    pub name: String,
    /// The attribute that holds it in its module: the function's name, or
    /// the name it was imported as.
    pub attribute: String,
    /// `name=` was given, so importing it under another name keeps it.
    pub explicit_name: bool,
    /// Defined with `params=`.
    pub params: bool,
    pub autouse: bool,
    pub scope: Scoped,
    /// The fixtures it requests.
    pub args: Vec<String>,
    /// Whether the name is a fixture at runtime depends on a branch that
    /// may not run, or on which of several definitions runs: it doesn't
    /// hide the definitions further out.
    pub conditional: bool,
    /// Bound in a way aye can't read (a decorator or a call it can't see
    /// through, an alias of something it can't follow): it may be a
    /// fixture, with any parameters, requesting anything.
    pub opaque: bool,
    /// `name=` is computed (or the module binds names at runtime): it may
    /// be registered under any name.
    pub any_name: bool,
}

impl FixtureDef {
    /// A plain fixture `name`, as pytest registers a function.
    fn named(name: &str, args: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            attribute: name.to_string(),
            explicit_name: false,
            params: false,
            autouse: false,
            scope: Scoped::Function,
            args,
            conditional: false,
            opaque: false,
            any_name: false,
        }
    }

    /// Something bound to `name` that may be a fixture aye can't read.
    fn opaque(name: &str) -> Self {
        Self {
            scope: Scoped::Unknown,
            opaque: true,
            ..Self::named(name, Vec::new())
        }
    }
}

/// pytest's own fixtures (and those of the plugins it ships), which are
/// never parametrized: a test may request them without any project file
/// defining them.
pub(crate) const BUILTIN_FIXTURES: &[&str] = &[
    "cache",
    "capfd",
    "capfdbinary",
    "caplog",
    "capsys",
    "capsysbinary",
    "capteesys",
    "doctest_namespace",
    "linecomp",
    "LineMatcher",
    "monkeypatch",
    "pytestconfig",
    "pytester",
    "record_property",
    "record_testsuite_property",
    "record_xml_attribute",
    "recwarn",
    "request",
    "subtests",
    "testdir",
    "testrun_uid",
    "tmp_path",
    "tmp_path_factory",
    "tmpdir",
    "tmpdir_factory",
    "worker_id",
];

/// `collect_ignore` or `collect_ignore_glob`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IgnoreList {
    /// Entries, and whether each may not be there at runtime.
    pub entries: Vec<(String, bool)>,
    /// Built in a way that cannot be read.
    pub unknown: bool,
}

/// One support module, summarised.
#[derive(Debug, Clone, Default)]
pub(crate) struct Summary {
    pub dir: PathBuf,
    /// Hook implementations defined at module level.
    pub hooks: BTreeSet<String>,
    /// `pytest_collect_file` mentions Python files.
    pub collect_file_touches_py: bool,
    /// It calls `pytest.skip` or `pytest.importorskip` while imported.
    pub skips_at_import: bool,
    pub fixtures: Vec<FixtureDef>,
    pub collect_ignore: Option<IgnoreList>,
    pub collect_ignore_glob: Option<IgnoreList>,
    /// `pytest_plugins` entries.
    pub plugins: Vec<String>,
    /// `pytest_plugins` could not be read.
    pub plugins_unknown: bool,
    /// The module surely has a `pytest_plugins` attribute once imported:
    /// a plain assignment at its top level, never deleted, in a module that
    /// neither skips itself nor binds names in ways aye can't follow.
    pub defines_plugins: bool,
    /// `from module import attribute as local` at module level, for names
    /// that stay bound to the import: `(local, module, attribute)`, the
    /// module with its leading dots.
    pub from_imports: Vec<(String, String, String)>,
    /// `from module import *` sources.
    pub star_imports: Vec<String>,
    /// `__all__`, when it is a list of literal strings never changed.
    pub all: Option<Vec<String>>,
    /// Modules registered as plugins at runtime:
    /// `pluginmanager.register(module)` on an imported module, or
    /// `pluginmanager.import_plugin("module")`.
    pub registered: Vec<String>,
    /// It registers plugins aye cannot read (`register(make())`).
    pub registers_unknown: bool,
    /// Module-level classes and their bases, for telling whether a class a
    /// test module subclasses may be a unittest case.
    pub classes: HashMap<String, Vec<BaseRef>>,
    /// Module-level names surely never bound to a class: only to
    /// functions and values that are no classes (literals, strings made by
    /// methods, typing forms).
    pub plain: HashSet<String>,
    /// Modules whose attributes its code that runs (at import, and in its
    /// hooks) may change: `math.pi = …`, `setattr(re, …)`.
    pub patched: BTreeSet<String>,
    /// Its code that runs reaches module namespaces in ways aye can't
    /// follow (`sys.modules[name]`, frames, `gc`): any module's names and
    /// attributes may change, a test module's included.
    pub reflective: bool,
    /// Modules its code that runs imports or looks up by a literal name.
    pub modules: Vec<String>,
    /// Module-level classes and functions whose `__test__` may be true:
    /// set in a class body to anything but `False`, or by the module
    /// (`name.__test__ = …`). pytest collects them whatever their name.
    pub tests: HashSet<String>,
    /// Module-level functions that may make classes (see
    /// [`crate::module::makes_classes`]).
    pub factories: HashSet<String>,
    /// What its code (`pytest_configure`, usually) changes in pytest's
    /// configuration that collection depends on.
    pub ini: IniChanges,
    /// What its code that runs (at import, and in its hooks) puts on
    /// `sys.path`.
    pub sys_path: SysPath,
    /// Its `pytest_collection_modifyitems` may add tests: it makes pytest
    /// nodes (`from_parent`; see `makes_items`).
    pub makes_items: bool,
}

/// What a support module's code changes in pytest's configuration that
/// collection reads while it walks: `config.addinivalue_line(…)`, writes
/// to `config.option` or to the ini values themselves. pytest calls a
/// conftest's `pytest_configure` as soon as it registers it, so the change
/// holds for the files collected after it: those below the conftest, at
/// least.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IniChanges {
    /// `python_files` patterns it adds, as literals: the files they match
    /// are test modules too.
    pub python_files: Vec<String>,
    /// Why the tests below it may differ from the static reading: a change
    /// to another value collection reads (`python_functions`,
    /// `python_classes`, `usefixtures`, …), to `python_files` with a value
    /// aye can't read, to an option that decides which files are collected
    /// (`config.option.ignore`, …), or one aye can't name.
    pub collection: Option<String>,
}

/// Ini values collection reads, whose change a conftest makes at runtime
/// may change which tests exist or their ids. `python_files` additions
/// are read when literal; `markers` and the rest only describe or report.
const COLLECTION_INI: [&str; 10] = [
    "python_files",
    "python_classes",
    "python_functions",
    "usefixtures",
    "collect_imported_tests",
    "empty_parameter_set_mark",
    "strict",
    "strict_parametrization_ids",
    "disable_test_id_escaping_and_forfeit_all_rights_to_community_support",
    "consider_namespace_packages",
];

/// `config.option` attributes that decide which files pytest collects and
/// how it imports them.
const COLLECTION_OPTIONS: [&str; 9] = [
    "ignore",
    "ignore_glob",
    "deselect",
    "pyargs",
    "importmode",
    "noconftest",
    "collect_in_virtualenv",
    "confcutdir",
    "python_files",
];

/// The visitor behind [`IniChanges`].
struct IniVisitor<'s, 'a> {
    lookup: Lookup<'s, 'a>,
    out: IniChanges,
}

impl IniVisitor<'_, '_> {
    fn changes(&mut self, what: String) {
        if self.out.collection.is_none() {
            self.out.collection = Some(what);
        }
    }

    /// A store into `target`: `config.option.NAME = …`, `….option.NAME +=
    /// …`, `config._inicache[…] = …`.
    fn store(&mut self, target: &Expr) {
        match target {
            Expr::Attribute(attr) => {
                if is_option(&attr.value) && COLLECTION_OPTIONS.contains(&attr.attr.id.as_str()) {
                    self.changes(format!("config.option.{}", attr.attr.id));
                } else if INI_STORES.contains(&attr.attr.id.as_str()) {
                    self.changes(format!("config.{}", attr.attr.id));
                } else if attr.attr.id.as_str() == "option" && is_config(&attr.value) {
                    self.changes("config.option".to_string());
                }
            }
            Expr::Subscript(sub) => self.store(&sub.value),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| self.store(e)),
            Expr::List(list) => list.elts.iter().for_each(|e| self.store(e)),
            _ => {}
        }
    }
}

/// Where pytest keeps ini values: writing there changes them.
const INI_STORES: [&str; 4] = ["_inicache", "inicfg", "_inicfg", "_override_ini"];

/// `….option` of a pytest config (`config.option`,
/// `request.config.option`, `pytestconfig.option`): of a name or attribute
/// that says `config`.
fn is_option(expr: &Expr) -> bool {
    matches!(expr, Expr::Attribute(attr) if attr.attr.id.as_str() == "option" && is_config(&attr.value))
}

/// A name or attribute that says `config`.
fn is_config(expr: &Expr) -> bool {
    let name = match expr {
        Expr::Name(name) => name.id.as_str(),
        Expr::Attribute(attr) => attr.attr.id.as_str(),
        _ => return false,
    };
    name.to_ascii_lowercase().contains("config")
}

impl<'a> Visitor<'a> for IniVisitor<'_, 'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Assign(assign) => assign.targets.iter().for_each(|t| self.store(t)),
            Stmt::AugAssign(assign) => self.store(&assign.target),
            Stmt::AnnAssign(assign) if assign.value.is_some() => self.store(&assign.target),
            Stmt::Delete(delete) => delete.targets.iter().for_each(|t| self.store(t)),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(method) = &*call.func
        {
            let first = call.arguments.args.first();
            match method.attr.id.as_str() {
                "addinivalue_line" => match first.and_then(|arg| self.lookup.string(arg)) {
                    Some(name) if name == "python_files" => {
                        match call
                            .arguments
                            .args
                            .get(1)
                            .and_then(|v| self.lookup.string(v))
                        {
                            Some(pattern) => self.out.python_files.push(pattern),
                            None => self.changes("python_files".to_string()),
                        }
                    }
                    Some(name) if COLLECTION_INI.contains(&name.as_str()) => self.changes(name),
                    Some(_) => {}
                    None => self.changes("an ini value it computes".to_string()),
                },
                name if MUTATORS.contains(&name) => match &*method.value {
                    // `config.getini("python_functions").append(…)`: the
                    // list is the cached value itself.
                    Expr::Call(inner) if matches!(&*inner.func, Expr::Attribute(a) if a.attr.id.as_str() == "getini") => {
                        match inner
                            .arguments
                            .args
                            .first()
                            .and_then(|a| self.lookup.string(a))
                        {
                            Some(ini) if !COLLECTION_INI.contains(&ini.as_str()) => {}
                            Some(ini) => self.changes(ini),
                            None => self.changes("an ini value it computes".to_string()),
                        }
                    }
                    // `config.option.ignore.append(…)`.
                    Expr::Attribute(attr)
                        if is_option(&attr.value)
                            && COLLECTION_OPTIONS.contains(&attr.attr.id.as_str()) =>
                    {
                        self.changes(format!("config.option.{}", attr.attr.id));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if let Expr::Call(call) = expr
            && let Expr::Name(func) = &*call.func
            && matches!(func.id.as_str(), "setattr" | "delattr")
            && let Some(target) = call.arguments.args.first()
        {
            let name = call
                .arguments
                .args
                .get(1)
                .and_then(|n| self.lookup.string(n));
            if is_option(target) {
                match name {
                    Some(name) if !COLLECTION_OPTIONS.contains(&name.as_str()) => {}
                    Some(name) => self.changes(format!("config.option.{name}")),
                    None => self.changes("config.option".to_string()),
                }
            } else if name.is_none_or(|n| INI_STORES.contains(&n.as_str())) {
                // `setattr(config, name, …)` may reach the ini values.
                if is_config(target) {
                    self.changes("config attributes it computes".to_string());
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

/// Methods that change a list in place.
const MUTATORS: [&str; 6] = [
    "append",
    "extend",
    "insert",
    "remove",
    "clear",
    "__setitem__",
];

/// [`IniChanges`] of a module: what any of its code may change, whichever
/// hook runs it.
fn ini_changes<'a>(module: &'a ModModule, lookup: Lookup<'_, 'a>) -> IniChanges {
    let mut visitor = IniVisitor {
        lookup,
        out: IniChanges::default(),
    };
    for stmt in &module.body {
        visitor.visit_stmt(stmt);
    }
    visitor.out
}

/// A base class, as a module refers to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BaseRef {
    /// A class of the same module.
    Local(String),
    /// A dotted path through imports or builtins.
    Path(String),
    Unknown,
}

impl Summary {
    /// It loads plugins aye cannot name: anything may be hooked.
    pub(crate) fn hooks_unknown(&self) -> bool {
        self.plugins_unknown || self.registers_unknown
    }
}

/// Whether `stmt` assigns `name` directly: `name = …`, `name: T = …` or
/// `name += …`.
fn assigns(stmt: &Stmt, name: &str) -> bool {
    let is_name = |expr: &Expr| matches!(expr, Expr::Name(n) if n.id.as_str() == name);
    match stmt {
        Stmt::Assign(assign) => assign.targets.iter().any(is_name),
        Stmt::AnnAssign(assign) => assign.value.is_some() && is_name(&assign.target),
        Stmt::AugAssign(assign) => is_name(&assign.target),
        _ => false,
    }
}

/// `getfuncargnames`: positional-or-keyword and keyword-only parameters
/// without defaults, minus `self`/`cls` for methods.
pub(crate) fn funcargnames(def: &StmtFunctionDef, drop_first: bool) -> Vec<String> {
    let params = &def.parameters;
    let mut names: Vec<String> = params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_none())
        .map(|p| p.parameter.name.id.to_string())
        .collect();
    // pytest drops the first name it kept, whatever it is.
    if drop_first && !names.is_empty() {
        names.remove(0);
    }
    names
}

/// Parameter names that have defaults.
pub(crate) fn default_argnames(def: &StmtFunctionDef) -> Vec<String> {
    let params = &def.parameters;
    params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_some())
        .map(|p| p.parameter.name.id.to_string())
        .collect()
}

/// A fixture defined by `def`, if its decorators make it one.
pub(crate) fn fixture_def<'a>(
    lookup: Lookup<'_, 'a>,
    def: &'a StmtFunctionDef,
    is_method: bool,
) -> Option<FixtureDef> {
    let lookup = lookup.at(def.start());
    let mut fixture = None;
    let mut staticmethod = false;
    for d in &def.decorator_list {
        match decorator(lookup, &d.expression) {
            Decorator::Fixture(call) => fixture = Some(call),
            Decorator::StaticMethod => staticmethod = true,
            _ => {}
        }
    }
    let call = fixture?;
    let mut out = FixtureDef::named(
        def.name.id.as_str(),
        funcargnames(def, is_method && !staticmethod),
    );
    if let Some(call) = call {
        fixture_options(lookup, call, &mut out);
        // `pytest.fixture(scope, params, autouse, ids, name)` positionally
        // is gone in pytest 9; a bare positional is the function itself.
    }
    Some(out)
}

/// The keywords of a `pytest.fixture(…)` call, read into `out`.
fn fixture_options<'a>(lookup: Lookup<'_, 'a>, call: &'a ExprCall, out: &mut FixtureDef) {
    for keyword in &call.arguments.keywords {
        let Some(arg) = &keyword.arg else {
            out.params = true;
            out.autouse = true;
            out.any_name = true;
            continue;
        };
        match arg.id.as_str() {
            "name" => {
                if let Some(name) = lookup.string(&keyword.value) {
                    out.name = name;
                    out.explicit_name = true;
                } else if !matches!(keyword.value, Expr::NoneLiteral(_)) {
                    out.any_name = true;
                }
            }
            "params" => out.params = !matches!(keyword.value, Expr::NoneLiteral(_)),
            "scope" => out.scope = Scoped::of(lookup, &keyword.value),
            "autouse" => out.autouse = lookup.boolean(&keyword.value) != Some(false),
            _ => {}
        }
    }
}

/// Whether `func` names pytest's fixture decorator.
fn is_fixture_decorator(lookup: Lookup<'_, '_>, func: &Expr) -> bool {
    lookup.path(func).is_some_and(|path| {
        path == "pytest.fixture"
            || path == "pytest.yield_fixture"
            || path == "fixture"
            || path.ends_with(".fixture")
    })
}

/// The fixture `name` is once the namespace `ns` has run, if it may be one:
/// its final binding's, or, when which binding runs is known only at
/// runtime (alternative branches, `try`/`except ImportError`, a last
/// binding that may not run), any of theirs, merged: parametrized if any
/// is, requesting what any requests, and `conditional`. Besides `@fixture`
/// functions, it reads `fixture(…)(function)` and `fixture(function, …)`
/// calls and aliases of fixtures (`alias = fixture_function`); names bound
/// in ways aye can't read that may make a fixture (a decorator or a call
/// it can't see through, unpacking) are `opaque`. `guarded` holds where the
/// body's statements that may not run start ([`crate::module`]'s
/// `guarded_by_try`).
pub(crate) fn fixture_of<'a>(
    lookup: Lookup<'_, 'a>,
    ns: &Namespace<'a>,
    name: &str,
    is_method: bool,
    guarded: &[TextSize],
) -> Option<FixtureDef> {
    let last = ns.get(name)?;
    let bindings = ns.bindings(name);
    let start = last.range.start();
    let mut candidates: Vec<&Bound<'a>> = vec![last];
    candidates.extend(
        bindings
            .iter()
            .filter(|b| b.range.start() < start && exclusive(b, last)),
    );
    let may_not_run = last.conditional || guarded.contains(&start);
    if may_not_run
        && let Some(previous) = bindings
            .iter()
            .rev()
            .find(|b| b.range.start() < start && !exclusive(b, last))
    {
        candidates.push(previous);
    }
    let alternatives = candidates.len() > 1;
    let mut merged: Option<FixtureDef> = None;
    for bound in candidates {
        let Some(def) = binding_fixture(lookup, name, bound, is_method, 0) else {
            continue;
        };
        merged = Some(match merged {
            None => def,
            Some(mut found) => {
                found.params |= def.params;
                found.autouse |= def.autouse;
                found.opaque |= def.opaque;
                found.any_name |= def.any_name;
                if found.scope != def.scope {
                    found.scope = Scoped::Unknown;
                }
                for arg in def.args {
                    if !found.args.contains(&arg) {
                        found.args.push(arg);
                    }
                }
                found
            }
        });
    }
    let mut def = merged?;
    def.conditional |= may_not_run || alternatives;
    Some(def)
}

/// The fixture one binding of `name` makes, if it may make one (see
/// [`fixture_of`]).
fn binding_fixture<'a>(
    lookup: Lookup<'_, 'a>,
    name: &str,
    bound: &Bound<'a>,
    is_method: bool,
    depth: u8,
) -> Option<FixtureDef> {
    let lookup = lookup.at(bound.range.start());
    match &bound.binding {
        Binding::Function(def) => {
            if let Some(mut fixture) = fixture_def(lookup, def, is_method) {
                if fixture.attribute != name {
                    // Bound under another name by an alias.
                    fixture.attribute = name.to_string();
                    if !fixture.explicit_name {
                        fixture.name = name.to_string();
                    }
                }
                return Some(fixture);
            }
            // A decorator aye can't see through may make it a fixture.
            def.decorator_list
                .iter()
                .any(|d| matches!(decorator(lookup, &d.expression), Decorator::Unknown))
                .then(|| FixtureDef::opaque(name))
        }
        Binding::Value(expr) => value_fixture(lookup, name, expr, is_method, depth),
        Binding::Other => Some(FixtureDef::opaque(name)),
        Binding::Class(_) | Binding::Import(_) => None,
    }
}

/// The fixture `name = expr` makes, if it may make one: `fixture(…)(f)`,
/// `fixture(f, …)`, an alias of a fixture, or a call or attribute aye can't
/// read (opaque). Calls that surely make no fixture (a class of the module
/// that makes its own instances, a builtin, a function of a package that
/// holds none) don't.
fn value_fixture<'a>(
    lookup: Lookup<'_, 'a>,
    name: &str,
    expr: &'a Expr,
    is_method: bool,
    depth: u8,
) -> Option<FixtureDef> {
    if depth > 4 {
        return Some(FixtureDef::opaque(name));
    }
    match expr {
        Expr::Call(call) => {
            // `fixture(…)(function)`: the options are the inner call's.
            let (options, function) = match &*call.func {
                Expr::Call(inner) if is_fixture_decorator(lookup, &inner.func) => {
                    (Some(inner), call.arguments.args.first())
                }
                func if is_fixture_decorator(lookup, func) => {
                    (Some(call), call.arguments.args.first())
                }
                _ => (None, None),
            };
            if let Some(options) = options {
                let Some(function) = function else {
                    // `fixture(…)` alone makes a decorator, not a fixture.
                    return None;
                };
                let mut out = FixtureDef::named(name, Vec::new());
                match lookup.resolve(function) {
                    Resolved::Local(target) => match target.binding {
                        Binding::Function(def) => out.args = funcargnames(def, is_method),
                        Binding::Value(Expr::Lambda(lambda)) => {
                            out.args = lambda
                                .parameters
                                .iter()
                                .flat_map(|p| p.args.iter().chain(&p.kwonlyargs))
                                .filter(|p| p.default.is_none())
                                .map(|p| p.parameter.name.id.to_string())
                                .collect();
                        }
                        _ => out.opaque = true,
                    },
                    _ => out.opaque = true,
                }
                fixture_options(lookup, options, &mut out);
                return Some(out);
            }
            // A class of the module makes its own instance, which pytest
            // never registers. A capitalized callee may still be a factory
            // that returns a fixture (`backend = Param("a", "b")`).
            if makes_own_instance(lookup, &call.func, 0) {
                return None;
            }
            if let Some(path) = lookup.path(&call.func)
                && (path.starts_with("builtins.") || crate::module::no_tests(&path))
            {
                return None;
            }
            Some(FixtureDef::opaque(name))
        }
        Expr::Name(_) | Expr::Attribute(_) => match lookup.resolve(expr) {
            Resolved::Local(target) => {
                if let Binding::Function(def) = target.binding
                    && fixture_def(lookup, def, is_method).is_none()
                    && !def
                        .decorator_list
                        .iter()
                        .any(|d| matches!(decorator(lookup, &d.expression), Decorator::Unknown))
                {
                    return None;
                }
                binding_fixture(lookup, name, &target, is_method, depth + 1)
            }
            Resolved::Path(path) if crate::module::no_tests(&path) => None,
            Resolved::Path(path) if path.starts_with("builtins.") => None,
            _ => Some(FixtureDef::opaque(name)),
        },
        _ => None,
    }
}

/// Whether calling `func` surely makes an instance of a class of this
/// module, which pytest never registers as a fixture (it registers only
/// objects whose type is exactly its `FixtureFunctionDefinition`): an
/// unconditional, undecorated class with no class keywords (a metaclass),
/// no `__new__`, nothing in its body that may bind one, nothing that may
/// change it afterwards, and only such classes or `object` as bases.
fn makes_own_instance<'a>(lookup: Lookup<'_, 'a>, func: &'a Expr, depth: u8) -> bool {
    if depth > 8 || lookup.module.dynamic {
        return false;
    }
    let Resolved::Local(Bound {
        binding: Binding::Class(class),
        conditional: false,
        ..
    }) = lookup.resolve(func)
    else {
        return false;
    };
    let plain_body = class.body.iter().all(|stmt| match stmt {
        Stmt::FunctionDef(def) => def.name.id.as_str() != "__new__",
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .all(|target| !matches!(target, Expr::Name(n) if n.id.as_str() == "__new__")),
        Stmt::AnnAssign(assign) => {
            !matches!(&*assign.target, Expr::Name(n) if n.id.as_str() == "__new__")
        }
        Stmt::Pass(_) => true,
        Stmt::Expr(expr) => matches!(&*expr.value, Expr::StringLiteral(_)),
        _ => false,
    });
    let changed = lookup.module.class_handed(class)
        || lookup
            .module
            .attributes
            .get(class.name.id.as_str())
            .is_some_and(|attributes| attributes.iter().any(|a| a.starts_with("__")));
    if !plain_body || changed || !class.decorator_list.is_empty() {
        return false;
    }
    let Some(arguments) = &class.arguments else {
        return true;
    };
    let lookup = lookup.at(class.start());
    arguments.keywords.is_empty()
        && arguments.args.iter().all(|base| {
            lookup.path(base).as_deref() == Some("builtins.object")
                || makes_own_instance(lookup, base, depth + 1)
        })
}

/// Hooks pytest calls for the files below their conftest only, that decide
/// which tests exist or what they are called: a file in their scope can
/// only be known by importing it.
pub(crate) const REFINING_HOOKS: [&str; 4] = [
    "pytest_pycollect_makeitem",
    "pytest_pycollect_makemodule",
    "pytest_ignore_collect",
    "pytest_collect_directory",
];

/// Whether a module's `pytest_collection_modifyitems` (`hook`) may make
/// items, and so add tests: `from_parent` in its body, or in a function or
/// class of the module it names, followed further; anywhere in the module
/// when the hook isn't a plain function. Other ways (`copy.copy(item)`, a
/// function of another module) are not seen.
fn makes_items(source: &str, ns: &Namespace<'_>, hook: &Bound<'_>) -> bool {
    const MAKER: &str = "from_parent";
    if !source.contains(MAKER) {
        return false;
    }
    if !matches!(hook.binding, Binding::Function(_)) {
        return true;
    }
    let names_in = |text: &str, name: &str| {
        text.match_indices(name).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !part(before) && !part(after)
        })
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut pending = vec![hook.range];
    while let Some(range) = pending.pop() {
        let text = &source[range];
        if text.contains(MAKER) {
            return true;
        }
        for (name, bound) in ns.iter() {
            if matches!(bound.binding, Binding::Function(_) | Binding::Class(_))
                && !seen.contains(name)
                && names_in(text, name)
            {
                seen.insert(name);
                pending.push(bound.range);
            }
        }
    }
    false
}

/// Hooks pytest calls for every test of the session, wherever their
/// conftest is: `pytest_collection` replaces collection,
/// `pytest_make_parametrize_id` makes ids, `pytest_collection_modifyitems`
/// may add marks, deselect or (when its module makes items) add tests.
pub(crate) const GLOBAL_HOOKS: [&str; 3] = [
    "pytest_collection",
    "pytest_make_parametrize_id",
    "pytest_collection_modifyitems",
];

/// Hooks that may add marks to collected tests: ids stay, tags become
/// uncertain. `pytest_itemcollected` is called below its conftest only.
pub(crate) const MARKING_HOOKS: [&str; 2] =
    ["pytest_collection_modifyitems", "pytest_itemcollected"];

impl Summary {
    pub(crate) fn build(dir: &Path, source: &str, module: &ModModule) -> Self {
        Self::of_namespace(dir, source, module, &Namespace::build(&module.body))
    }

    /// [`Summary::build`] with the module's namespace already built.
    pub(crate) fn of_namespace<'a>(
        dir: &Path,
        source: &str,
        module: &'a ModModule,
        ns: &Namespace<'a>,
    ) -> Self {
        Self::summarize(dir, source, module, ns, true)
    }

    /// [`Summary::of_namespace`] for a test module, whose own fixtures the
    /// recognizer reads itself: only what it imports and loads.
    pub(crate) fn of_test_module<'a>(
        dir: &Path,
        source: &str,
        module: &'a ModModule,
        ns: &Namespace<'a>,
    ) -> Self {
        Self::summarize(dir, source, module, ns, false)
    }

    fn summarize<'a>(
        dir: &Path,
        source: &str,
        module: &'a ModModule,
        ns: &Namespace<'a>,
        fixtures: bool,
    ) -> Self {
        let lookup = Lookup {
            module: ns,
            class: None,
            source,
            at: TextSize::try_from(source.len()).unwrap_or(TextSize::new(u32::MAX)),
        };
        let mut out = Self {
            dir: dir.to_path_buf(),
            ..Self::default()
        };
        let guarded = if fixtures {
            guarded_by_try(&module.body)
        } else {
            Vec::new()
        };
        for (name, bound) in ns.iter() {
            if name.starts_with("pytest_") && name != "pytest_plugins" {
                out.hooks.insert(name.to_string());
                if name == "pytest_collect_file" {
                    let text = &source[bound.range];
                    out.collect_file_touches_py = text.contains(".py")
                        || text.contains("\"py\"")
                        || text.contains("'py'")
                        || !matches!(bound.binding, Binding::Function(_));
                }
                if name == "pytest_collection_modifyitems" {
                    out.makes_items = makes_items(source, ns, bound);
                }
            }
            if fixtures && let Some(fixture) = fixture_of(lookup, ns, name, false, &guarded) {
                out.fixtures.push(fixture);
            }
        }
        if fixtures && ns.dynamic && binds_names_at_runtime(module, lookup) {
            // `globals()[name] = fixture(…)(…)`: any name may be a fixture.
            out.fixtures.push(FixtureDef {
                any_name: true,
                ..FixtureDef::opaque("*")
            });
        }
        out.skips_at_import = import_time_skip(&module.body, lookup).is_some();
        out.patched = ns.patched_modules().cloned().collect();
        out.reflective = ns.reflective;
        out.modules = ns.imported_modules().to_vec();
        out.collect_ignore = ignore_list(ns, lookup, "collect_ignore");
        out.collect_ignore_glob = ignore_list(ns, lookup, "collect_ignore_glob");
        if let Some(bound) = ns.get("pytest_plugins") {
            match bound.binding {
                Binding::Value(expr) => {
                    let lookup = lookup.at(bound.range.end());
                    if let Some(single) = lookup.string(expr) {
                        out.plugins.push(single);
                    } else if let Ok(elems) = lookup.sequence(expr) {
                        for elem in elems {
                            match elem {
                                Elem::Expr(expr, lk) => match lk.string(expr) {
                                    Some(name) => out.plugins.push(name),
                                    None => out.plugins_unknown = true,
                                },
                                Elem::Value(_) => out.plugins_unknown = true,
                            }
                        }
                    } else {
                        out.plugins_unknown = true;
                    }
                }
                _ => out.plugins_unknown = true,
            }
        }
        out.defines_plugins = !ns.dynamic
            && !out.skips_at_import
            && ns.get("pytest_plugins").is_some()
            && !ns.deletions.contains_key("pytest_plugins")
            && !ns.globals.contains("pytest_plugins")
            && module
                .body
                .iter()
                .any(|stmt| assigns(stmt, "pytest_plugins"));
        let mut from_imports = Vec::new();
        module_imports(&module.body, &mut from_imports, &mut out.star_imports);
        out.from_imports = from_imports
            .into_iter()
            .filter(|(local, ..)| {
                ns.get(local)
                    .is_some_and(|b| matches!(b.binding, Binding::Import(_)))
            })
            .collect();
        if let Some(bound) = ns.get("__all__")
            && let Binding::Value(expr) = bound.binding
            && !ns.is_mutable("__all__")
            && let Ok(elems) = lookup.at(bound.range.end()).sequence(expr)
        {
            out.all = elems
                .into_iter()
                .map(|elem| match elem {
                    Elem::Expr(expr, lk) => lk.string(expr),
                    Elem::Value(_) => None,
                })
                .collect();
        }
        for (name, bound) in ns.iter() {
            let Binding::Class(class) = bound.binding else {
                continue;
            };
            let lookup = lookup.at(class.start());
            let bases = class
                .arguments
                .iter()
                .flat_map(|args| args.args.iter())
                .map(|base| {
                    let target = match base {
                        Expr::Subscript(sub) => &*sub.value,
                        other => other,
                    };
                    if let Expr::Name(base) = target
                        && let Some(found) = ns.get(base.id.as_str())
                        && let Binding::Class(_) = found.binding
                    {
                        return if found.conditional {
                            BaseRef::Unknown
                        } else {
                            BaseRef::Local(base.id.to_string())
                        };
                    }
                    lookup.path(target).map_or(BaseRef::Unknown, BaseRef::Path)
                })
                .collect();
            out.classes.insert(name.to_string(), bases);
        }
        for (name, bound) in ns.iter() {
            let set = !matches!(ns.test_attr(name), Flag::Unset | Flag::Set(false));
            let tests = match bound.binding {
                Binding::Class(class) => set || class_sets_test(&class.body),
                Binding::Function(def) => {
                    if crate::module::makes_classes(&def.body) {
                        out.factories.insert(name.to_string());
                    }
                    set
                }
                _ => set,
            };
            if tests {
                out.tests.insert(name.to_string());
            }
        }
        for (name, _) in ns.iter() {
            let plain = ns.bindings(name).iter().all(|bound| match bound.binding {
                Binding::Function(_) => true,
                Binding::Value(expr) => not_a_class(lookup, expr),
                _ => false,
            });
            if plain {
                out.plain.insert(name.to_string());
            }
        }
        let mut registrations = Registrations {
            ns,
            lookup,
            registered: Vec::new(),
            unknown: false,
        };
        for stmt in &module.body {
            registrations.visit_stmt(stmt);
        }
        out.registered = registrations.registered;
        out.registers_unknown = registrations.unknown;
        // Only support modules configure pytest.
        if fixtures
            && [
                "addinivalue_line",
                "getini",
                "_inicache",
                "inicfg",
                "_override_ini",
                ".option",
            ]
            .iter()
            .any(|word| source.contains(word))
        {
            out.ini = ini_changes(module, lookup);
        }
        // A test module changes `sys.path` with `sys` or `site` imported at
        // its top: its imports are cheaper to look at than its text.
        let may_change_path = if fixtures {
            ["sys.path", "addsitedir", "from sys import"]
                .iter()
                .any(|word| source.contains(word))
        } else {
            ns.iter().any(|(_, bound)| {
                matches!(&bound.binding, Binding::Import(path)
                    if path == "sys" || path == "site" || path.starts_with("sys.") || path.starts_with("site."))
            })
        };
        if may_change_path {
            let mut visitor = SysPathVisitor {
                lookup,
                // Only its directory matters to what it adds.
                file: dir.join("__file__.py"),
                out: SysPath::default(),
            };
            for stmt in &module.body {
                visitor.visit_stmt(stmt);
            }
            out.sys_path = visitor.out;
        }
        out
    }
}

/// What [`Supports::project_name`] asks of the project file and name a
/// path leads to (with the files and names being visited).
type NameCheck<'c> = dyn Fn(&Supports, &Summary, &str, &mut Vec<(PathBuf, String)>) -> bool + 'c;

/// `module.attribute`, for a `from module import attribute` (the module
/// with its leading dots).
fn joined(module: &str, attribute: &str) -> String {
    if module.ends_with('.') {
        format!("{module}{attribute}")
    } else {
        format!("{module}.{attribute}")
    }
}

/// Whether a class body sets `__test__` to something other than `False`
/// (in its own statements or the branches they hold).
fn class_sets_test(body: &[Stmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Assign(assign) => {
            assign
                .targets
                .iter()
                .any(|t| matches!(t, Expr::Name(n) if n.id.as_str() == "__test__"))
                && !matches!(&*assign.value, Expr::BooleanLiteral(b) if !b.value)
        }
        Stmt::AnnAssign(assign) => {
            matches!(&*assign.target, Expr::Name(n) if n.id.as_str() == "__test__")
                && !matches!(assign.value.as_deref(), Some(Expr::BooleanLiteral(b)) if !b.value)
        }
        Stmt::If(s) => {
            class_sets_test(&s.body) || s.elif_else_clauses.iter().any(|c| class_sets_test(&c.body))
        }
        Stmt::Try(s) => {
            class_sets_test(&s.body)
                || class_sets_test(&s.orelse)
                || class_sets_test(&s.finalbody)
                || s.handlers.iter().any(|h| {
                    h.as_except_handler()
                        .is_some_and(|h| class_sets_test(&h.body))
                })
        }
        Stmt::With(s) => class_sets_test(&s.body),
        Stmt::For(s) => class_sets_test(&s.body) || class_sets_test(&s.orelse),
        Stmt::While(s) => class_sets_test(&s.body) || class_sets_test(&s.orelse),
        _ => false,
    })
}

/// Whether the module's code may bind names in its own namespace in ways
/// aye can't read: it calls `globals()`, `vars()` or `locals()` (at module
/// level) without arguments anywhere, `exec` at module level, or `setattr`
/// on `sys.modules[…]`. Setting attributes of other objects
/// (`setattr(item, …)`) binds none.
fn binds_names_at_runtime<'a>(module: &'a ModModule, lookup: Lookup<'_, 'a>) -> bool {
    struct Finder<'s, 'a> {
        lookup: Lookup<'s, 'a>,
        depth: usize,
        found: bool,
    }
    impl<'a> Visitor<'a> for Finder<'_, 'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            let nested = matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_));
            self.depth += usize::from(nested);
            visitor::walk_stmt(self, stmt);
            self.depth -= usize::from(nested);
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr
                && let Some(path) = self.lookup.path(&call.func)
            {
                let no_args = call.arguments.is_empty();
                self.found |= match path.as_str() {
                    "builtins.globals" | "builtins.vars" => no_args,
                    "builtins.locals" => no_args && self.depth == 0,
                    "builtins.exec" => self.depth == 0,
                    "builtins.setattr" | "builtins.delattr" => {
                        call.arguments.args.first().is_some_and(|target| {
                            matches!(target, Expr::Subscript(sub)
                                if self.lookup.path(&sub.value).is_some_and(|p| p == "sys.modules"))
                        })
                    }
                    _ => false,
                };
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut finder = Finder {
        lookup,
        depth: 0,
        found: false,
    };
    for stmt in &module.body {
        finder.visit_stmt(stmt);
    }
    finder.found
}

/// Whether a value is surely no class: a literal or display, an operator's
/// result, a method of a literal (`"\n".join(…)`), or a typing form
/// (`Literal[…]`, `Callable[…]`).
fn not_a_class<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> bool {
    match expr {
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
        | Expr::Lambda(_)
        | Expr::Compare(_)
        | Expr::UnaryOp(_) => true,
        Expr::BinOp(op) => not_a_class(lookup, &op.left) || not_a_class(lookup, &op.right),
        Expr::Call(call) => match &*call.func {
            Expr::Attribute(attr)
                if matches!(
                    &*attr.value,
                    Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::FString(_)
                ) =>
            {
                true
            }
            func => lookup.path(func).is_some_and(|path| {
                // Builtins that make data, never a class.
                path.strip_prefix("builtins.").is_some_and(|name| {
                    matches!(
                        name,
                        "str"
                            | "int"
                            | "float"
                            | "bool"
                            | "bytes"
                            | "bytearray"
                            | "complex"
                            | "list"
                            | "tuple"
                            | "dict"
                            | "set"
                            | "frozenset"
                            | "len"
                            | "sorted"
                            | "range"
                            | "repr"
                            | "format"
                            | "abs"
                            | "round"
                            | "hash"
                            | "id"
                            | "ord"
                            | "chr"
                            | "hex"
                            | "oct"
                            | "bin"
                            | "enumerate"
                            | "zip"
                            | "map"
                            | "filter"
                            | "reversed"
                            | "sum"
                            | "any"
                            | "all"
                            | "divmod"
                            | "pow"
                            | "slice"
                            | "object"
                    )
                })
            }),
        },
        // Generic aliases and typing forms (`Literal[…]`, `list[int]`).
        Expr::Subscript(sub) => lookup.path(&sub.value).is_some_and(|path| {
            [
                "typing.",
                "typing_extensions.",
                "collections.abc.",
                "builtins.",
            ]
            .iter()
            .any(|prefix| path.starts_with(prefix))
        }),
        _ => false,
    }
}

/// `from … import …` statements run while a module is imported (not in its
/// functions or classes): `(local, module, attribute)` for each name, and
/// the sources of star imports.
fn module_imports(body: &[Stmt], out: &mut Vec<(String, String, String)>, stars: &mut Vec<String>) {
    for stmt in body {
        match stmt {
            Stmt::ImportFrom(import) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(import.level as usize),
                    import.module.as_ref().map_or("", |m| m.id.as_str())
                );
                for alias in &import.names {
                    let name = alias.name.id.as_str();
                    if name == "*" {
                        if !stars.contains(&module) {
                            stars.push(module.clone());
                        }
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    out.push((local.to_string(), module.clone(), name.to_string()));
                }
            }
            Stmt::If(s) => {
                module_imports(&s.body, out, stars);
                for clause in &s.elif_else_clauses {
                    module_imports(&clause.body, out, stars);
                }
            }
            Stmt::Try(s) => {
                module_imports(&s.body, out, stars);
                module_imports(&s.orelse, out, stars);
                module_imports(&s.finalbody, out, stars);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        module_imports(&handler.body, out, stars);
                    }
                }
            }
            Stmt::With(s) => module_imports(&s.body, out, stars),
            Stmt::For(s) => {
                module_imports(&s.body, out, stars);
                module_imports(&s.orelse, out, stars);
            }
            Stmt::While(s) => {
                module_imports(&s.body, out, stars);
                module_imports(&s.orelse, out, stars);
            }
            Stmt::Match(s) => {
                for case in &s.cases {
                    module_imports(&case.body, out, stars);
                }
            }
            _ => {}
        }
    }
}

/// Calls that register plugins, anywhere in a module (`pytest_configure`
/// usually): `….pluginmanager.register(x)` and
/// `….pluginmanager.import_plugin("name")`.
struct Registrations<'s, 'a> {
    ns: &'s Namespace<'a>,
    lookup: Lookup<'s, 'a>,
    registered: Vec<String>,
    unknown: bool,
}

impl<'a> Visitor<'a> for Registrations<'_, 'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(method) = &*call.func
            && is_pluginmanager(&method.value)
        {
            let first = call.arguments.args.first();
            match method.attr.id.as_str() {
                "register" => match first {
                    // A module imported at module level.
                    Some(Expr::Name(name)) => match self.ns.get(name.id.as_str()) {
                        Some(bound) if matches!(bound.binding, Binding::Import(_)) => {
                            if let Binding::Import(path) = &bound.binding {
                                self.registered.push(path.clone());
                            }
                        }
                        _ => self.unknown = true,
                    },
                    _ => self.unknown = true,
                },
                "import_plugin" | "consider_pluginarg" => {
                    match first.and_then(|arg| self.lookup.string(arg)) {
                        // `no:name` blocks a plugin rather than loading one.
                        Some(name) if name.starts_with("no:") => {}
                        Some(name) => self.registered.push(name),
                        None => self.unknown = true,
                    }
                }
                "consider_module"
                | "consider_namespace_packages"
                | "load_setuptools_entrypoints" => {
                    self.unknown = true;
                }
                _ => {}
            }
        }
        visitor::walk_expr(self, expr);
    }
}

/// Every name a module may request as a fixture: its functions'
/// parameters, and its strings that are identifiers (`usefixtures("db")`).
pub(crate) fn requested_names(module: &ModModule) -> HashSet<String> {
    struct Names(HashSet<String>);
    impl<'a> Visitor<'a> for Names {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if let Stmt::FunctionDef(def) = stmt {
                let params = &def.parameters;
                for p in params
                    .posonlyargs
                    .iter()
                    .chain(&params.args)
                    .chain(&params.kwonlyargs)
                {
                    self.0.insert(p.parameter.name.id.to_string());
                }
            }
            visitor::walk_stmt(self, stmt);
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::StringLiteral(literal) = expr {
                let text = literal.value.to_str();
                for part in text.split(|c: char| c == ',' || c.is_whitespace()) {
                    if !part.is_empty() {
                        self.0.insert(part.to_string());
                    }
                }
            }
            visitor::walk_expr(self, expr);
        }
    }
    let mut names = Names(HashSet::new());
    for stmt in &module.body {
        names.visit_stmt(stmt);
    }
    names.0
}

/// `pluginmanager`, or `<anything>.pluginmanager`.
fn is_pluginmanager(expr: &Expr) -> bool {
    match expr {
        Expr::Name(name) => name.id.as_str() == "pluginmanager",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "pluginmanager",
        _ => false,
    }
}

fn ignore_list<'a>(ns: &Namespace<'a>, lookup: Lookup<'_, 'a>, name: &str) -> Option<IgnoreList> {
    let bound = ns.get(name);
    let ops: Vec<_> = ns.ops.iter().filter(|op| op.name == name).collect();
    if bound.is_none() && ops.is_empty() {
        return None;
    }
    let mut list = IgnoreList {
        unknown: ns.dynamic || ns.globals.contains(name),
        ..IgnoreList::default()
    };
    if let Some(bound) = bound
        && !matches!(bound.binding, Binding::Value(_))
    {
        list.unknown = true;
    }
    let strings = |expr: &Expr, at: TextSize| -> Option<Vec<String>> {
        let lookup = lookup.at(at);
        let elems = lookup.sequence(expr).ok()?;
        elems
            .into_iter()
            .map(|elem| match elem {
                Elem::Expr(expr, lk) => lk.string(expr),
                Elem::Value(_) => None,
            })
            .collect()
    };
    for op in ops {
        let added = match op.kind {
            OpKind::Assign(expr) => {
                if op.conditional {
                    for entry in &mut list.entries {
                        entry.1 = true;
                    }
                } else {
                    list.entries.clear();
                }
                strings(expr, op.at)
            }
            OpKind::Append(expr) => lookup.at(op.at).string(expr).map(|s| vec![s]),
            OpKind::Extend(expr) | OpKind::AugAdd(expr) => strings(expr, op.at),
            OpKind::Mutate => None,
        };
        match added {
            Some(entries) => list
                .entries
                .extend(entries.into_iter().map(|e| (e, op.conditional))),
            None => list.unknown = true,
        }
    }
    Some(list)
}

/// What a module's namespace holds once it has run, as far as collection
/// is concerned: the fixtures and hooks it defines, and those it imports
/// from project files.
#[derive(Debug, Clone, Default)]
pub(crate) struct Exports {
    pub fixtures: Vec<FixtureDef>,
    pub hooks: BTreeSet<String>,
    /// Names imported from modules that are not project files: each may be
    /// a fixture.
    pub maybe_fixtures: BTreeSet<String>,
    /// A star import from a module that is not a project file: any name
    /// may be a fixture.
    pub any_fixture: bool,
    pub collect_file_touches_py: bool,
    pub skips_at_import: bool,
    /// Its `pytest_collection_modifyitems`, or a module's it star-imports,
    /// may add tests ([`Summary::makes_items`]).
    pub makes_items: bool,
}

/// Summaries of support modules, shared across files.
#[derive(Debug, Default)]
pub(crate) struct Supports {
    /// By path and a hash of the source.
    parsed: Mutex<HashMap<(PathBuf, u64), Arc<Summary>>>,
    /// `conftest.py` in a directory, read from disk.
    dirs: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// Plugin modules, read from disk.
    plugins: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// [`Supports::expand`] results, by summary.
    expanded: Mutex<HashMap<usize, Vec<Arc<Summary>>>>,
    /// [`Supports::exports`] results, by summary.
    exported: Mutex<HashMap<usize, Arc<Exports>>>,
    /// Module files by name and importing directory.
    resolved: Mutex<HashMap<(String, PathBuf), Option<PathBuf>>>,
    /// The import mode puts a module's own directory after `sys.path`
    /// (`append`), or not at all (`importlib`): see [`resolve_from`].
    pub bases_first: bool,
    /// [`package_base`] by directory.
    package_bases: Mutex<HashMap<PathBuf, PathBuf>>,
}

fn parse_file(path: &Path) -> Option<Summary> {
    let source = std::fs::read_to_string(path).ok()?;
    let parsed = ruff_python_parser::parse_module(&source).ok()?;
    let dir = path.parent()?;
    Some(Summary::build(dir, &source, parsed.syntax()))
}

impl Supports {
    /// The summary of a support file handed to the recognizer.
    pub(crate) fn of(&self, file: &SourceFile<'_>) -> Arc<Summary> {
        let key = (file.abs.to_path_buf(), Fnv64::hash(file.source.as_bytes()));
        if let Some(found) = self.parsed.lock().expect("lock").get(&key) {
            return Arc::clone(found);
        }
        let dir = file.abs.parent().unwrap_or(file.abs);
        let summary = Arc::new(Summary::build(dir, file.source, file.module));
        // Another thread may have summarized it meanwhile: the summary the
        // map keeps is the one returned, so that it lives as long as `self`
        // and its address keys `expanded` and `exported` (a summary
        // dropped here could have its address reused by another file's).
        Arc::clone(
            self.parsed
                .lock()
                .expect("lock")
                .entry(key)
                .or_insert(summary),
        )
    }

    /// The summary of `dir/conftest.py`, read from disk, if there is one.
    pub(crate) fn conftest_in(&self, dir: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.dirs.lock().expect("lock").get(dir) {
            return found.clone();
        }
        let path = dir.join("conftest.py");
        let summary = path.is_file().then(|| {
            Arc::new(parse_file(&path).unwrap_or_else(|| Summary {
                dir: dir.to_path_buf(),
                ..Summary::default()
            }))
        });
        // The map's summary, whichever thread put it there (see `of`).
        self.dirs
            .lock()
            .expect("lock")
            .entry(dir.to_path_buf())
            .or_insert(summary)
            .clone()
    }

    fn plugin_at(&self, path: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.plugins.lock().expect("lock").get(path) {
            return found.clone();
        }
        let summary = parse_file(path).map(Arc::new);
        // The map's summary, whichever thread put it there (see `of`).
        self.plugins
            .lock()
            .expect("lock")
            .entry(path.to_path_buf())
            .or_insert(summary)
            .clone()
    }

    /// `summary` and the plugin modules it loads through `pytest_plugins`,
    /// transitively. Plugins that are not files of the project (installed
    /// packages) are not seen, like any installed plugin.
    pub(crate) fn expand(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Vec<Arc<Summary>> {
        if summary.plugins.is_empty() && summary.registered.is_empty() {
            return vec![Arc::clone(summary)];
        }
        // Summaries live as long as `self` caches them, so their address
        // identifies them.
        let key = Arc::as_ptr(summary) as usize;
        if let Some(found) = self.expanded.lock().expect("lock").get(&key) {
            return found.clone();
        }
        let out = self.expand_uncached(summary, bases);
        self.expanded.lock().expect("lock").insert(key, out.clone());
        out
    }

    fn expand_uncached(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Vec<Arc<Summary>> {
        let mut out = vec![Arc::clone(summary)];
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack: Vec<(Arc<Summary>, usize)> = vec![(Arc::clone(summary), 0)];
        while let Some((current, depth)) = stack.pop() {
            if depth > 8 {
                continue;
            }
            for plugin in current.plugins.iter().chain(&current.registered) {
                let Some(path) = self.resolve(plugin, &current.dir, bases) else {
                    continue;
                };
                if !seen.insert(path.clone()) {
                    continue;
                }
                if let Some(found) = self.plugin_at(&path) {
                    out.push(Arc::clone(&found));
                    stack.push((found, depth + 1));
                }
            }
        }
        out
    }
}

/// What an attribute of a module is, as far as fixtures go.
#[derive(Debug, Clone)]
enum Attribute {
    Fixture(FixtureDef),
    /// It comes from a module that is not a project file.
    Maybe,
    No,
}

impl Supports {
    /// The summary of a project module, by its dotted name as code in
    /// `dir` imports it (relative names allowed).
    pub(crate) fn module(&self, name: &str, dir: &Path, bases: &[PathBuf]) -> Option<Arc<Summary>> {
        let path = self.resolve(name, dir, bases)?;
        self.plugin_at(&path)
    }

    /// [`package_base`], cached.
    pub(crate) fn package_base(&self, dir: &Path) -> PathBuf {
        if let Some(found) = self.package_bases.lock().expect("lock").get(dir) {
            return found.clone();
        }
        let found = package_base(dir);
        self.package_bases
            .lock()
            .expect("lock")
            .insert(dir.to_path_buf(), found.clone());
        found
    }

    /// Forget the modules looked up so far, and what was read through
    /// them: the places they are looked up in have grown.
    pub(crate) fn forget_resolutions(&self) {
        self.resolved.lock().expect("lock").clear();
        self.expanded.lock().expect("lock").clear();
        self.exported.lock().expect("lock").clear();
    }

    /// [`resolve_from`], cached: files are looked for once per name and
    /// directory.
    fn resolve(&self, name: &str, dir: &Path, bases: &[PathBuf]) -> Option<PathBuf> {
        let key = (name.to_string(), dir.to_path_buf());
        if let Some(found) = self.resolved.lock().expect("lock").get(&key) {
            return found.clone();
        }
        let own = self.package_base(dir);
        let found = resolve_from(name, dir, &own, bases, self.bases_first);
        self.resolved
            .lock()
            .expect("lock")
            .insert(key, found.clone());
        found
    }

    /// What `summary`'s namespace holds, following its imports into project
    /// files. `summary` must be one these caches hold (a conftest's, or a
    /// plugin's): results are kept by its address.
    pub(crate) fn exports(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Arc<Exports> {
        let key = Arc::as_ptr(summary) as usize;
        if let Some(found) = self.exported.lock().expect("lock").get(&key) {
            return Arc::clone(found);
        }
        let out = Arc::new(self.exports_of(summary, bases, |_| true));
        self.exported
            .lock()
            .expect("lock")
            .insert(key, Arc::clone(&out));
        out
    }

    /// [`Supports::exports`] for a summary of any lifetime, uncached, with
    /// only the names `wanted` keeps followed through `from … import`.
    pub(crate) fn exports_of(
        &self,
        summary: &Summary,
        bases: &[PathBuf],
        wanted: impl Fn(&str) -> bool,
    ) -> Exports {
        let mut visiting = Vec::new();
        self.exports_inner(summary, bases, &wanted, &mut visiting)
    }

    fn exports_inner(
        &self,
        summary: &Summary,
        bases: &[PathBuf],
        wanted: &dyn Fn(&str) -> bool,
        visiting: &mut Vec<PathBuf>,
    ) -> Exports {
        let mut out = Exports {
            fixtures: summary.fixtures.clone(),
            hooks: summary.hooks.clone(),
            collect_file_touches_py: summary.collect_file_touches_py,
            skips_at_import: summary.skips_at_import,
            makes_items: summary.makes_items,
            ..Exports::default()
        };
        for (local, module, attribute) in &summary.from_imports {
            if !wanted(local) || crate::module::no_tests(module) {
                continue;
            }
            match self.attribute(module, attribute, &summary.dir, bases, visiting) {
                Attribute::Fixture(mut fixture) => {
                    if !fixture.explicit_name {
                        fixture.name.clone_from(local);
                    }
                    fixture.attribute.clone_from(local);
                    out.fixtures.push(fixture);
                }
                Attribute::Maybe => {
                    out.maybe_fixtures.insert(local.clone());
                }
                Attribute::No => {}
            }
        }
        for module in &summary.star_imports {
            if crate::module::no_tests(module) {
                continue;
            }
            let Some(path) = self.resolve(module, &summary.dir, bases) else {
                out.any_fixture = true;
                continue;
            };
            let Some(source) = self.plugin_at(&path) else {
                out.any_fixture = true;
                continue;
            };
            if visiting.contains(&path) || visiting.len() > 8 {
                continue;
            }
            visiting.push(path);
            let theirs = self.exports_inner(&source, bases, &|_| true, visiting);
            visiting.pop();
            let public = |name: &str| match &source.all {
                Some(all) => all.iter().any(|n| n == name),
                None => !name.starts_with('_'),
            };
            out.fixtures.extend(
                theirs
                    .fixtures
                    .iter()
                    .filter(|f| public(&f.attribute))
                    .cloned(),
            );
            out.hooks
                .extend(theirs.hooks.iter().filter(|h| public(h)).cloned());
            out.maybe_fixtures
                .extend(theirs.maybe_fixtures.iter().filter(|n| public(n)).cloned());
            out.any_fixture |= theirs.any_fixture;
            out.collect_file_touches_py |= theirs.collect_file_touches_py;
            out.skips_at_import |= theirs.skips_at_import;
            out.makes_items |= theirs.makes_items;
        }
        out
    }

    /// Whether the class `path` names, as code in `dir` refers to it, may be
    /// a unittest case: one of pytest's or unittest's own, or a project class
    /// deriving from one or from a class aye can't follow. Classes from
    /// packages that ship no test cases are not.
    pub(crate) fn may_be_unittest(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.may_be_unittest_inner(path, dir, bases, &mut visiting)
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names
    /// something a project file defines that may be a unittest case: a
    /// class whose bases may make it one, or a name aye can't tell is no
    /// class. Modules are not, nor what installed packages define unless
    /// it is a known unittest base or named like one (`…TestCase`).
    pub(crate) fn project_unittest(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.defined_unittest(path, dir, bases, &mut visiting, true)
    }

    /// Whether some leading part of the dotted `path` (as code in `dir`
    /// imports it), its last aside (the name imported from it), is a
    /// project module.
    pub(crate) fn resolvable(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        (1..parts.len()).any(|split| {
            let module = format!("{dots}{}", parts[..split].join("."));
            self.resolve(&module, dir, bases).is_some()
        })
    }

    /// [`Supports::project_unittest`]; `first` while `path` is the name a
    /// test module imports (it must lead to a project file).
    fn defined_unittest(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        first: bool,
    ) -> bool {
        // What a test module imports must come from a project file.
        if !first && is_unittest_base(path) {
            return true;
        }
        if crate::module::no_tests(path) {
            return false;
        }
        if visiting.len() > 16 {
            return true;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [class] = &parts[split..] else {
                return true;
            };
            let Some(source) = self.plugin_at(&file) else {
                return true;
            };
            let bound = source.plain.contains(*class)
                || source.classes.contains_key(*class)
                || source.from_imports.iter().any(|(local, ..)| local == class);
            if !bound {
                // Not bound by the module: a submodule (`from pkg import
                // sub`), or bound in ways aye can't read.
                return self.resolve(path, dir, bases).is_none();
            }
            let key = (file, (*class).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = self.defined_in(&source, class, bases, visiting, 0);
            visiting.pop();
            return found;
        }
        // An installed package: only what looks like a unittest case.
        !first && parts.last().is_some_and(|name| name.ends_with("TestCase"))
    }

    /// [`Supports::project_unittest`] for a name `source` binds.
    fn defined_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return true;
        }
        if source.plain.contains(name) {
            return false;
        }
        if let Some(refs) = source.classes.get(name) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(local) => self.defined_in(source, local, bases, visiting, depth + 1),
                BaseRef::Path(path) => {
                    self.defined_unittest(path, &source.dir, bases, visiting, false)
                }
                BaseRef::Unknown => true,
            });
        }
        if let Some((_, module, attribute)) =
            source.from_imports.iter().find(|(local, ..)| local == name)
        {
            let path = if module.ends_with('.') {
                format!("{module}{attribute}")
            } else {
                format!("{module}.{attribute}")
            };
            return self.defined_unittest(&path, &source.dir, bases, visiting, false);
        }
        true
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names a
    /// class or function of a project file whose `__test__` may be true,
    /// directly or through the project classes it inherits from. Names
    /// aye can't read, and installed packages, don't count.
    pub(crate) fn project_test(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.project_name(
            path,
            dir,
            bases,
            &mut visiting,
            &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
        )
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names a
    /// function of a project file that may make classes.
    pub(crate) fn project_factory(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.project_name(
            path,
            dir,
            bases,
            &mut visiting,
            &|this, source, name, visiting| this.factory_in(source, name, bases, visiting),
        )
    }

    /// The project file and name `path` leads to, put to `check`; `false`
    /// when it names no project file's attribute.
    fn project_name(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        check: &NameCheck<'_>,
    ) -> bool {
        if crate::module::no_tests(path) || visiting.len() > 16 {
            return false;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [name] = &parts[split..] else {
                return false;
            };
            let Some(source) = self.plugin_at(&file) else {
                return false;
            };
            let key = (file, (*name).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = check(self, &source, name, visiting);
            visiting.pop();
            return found;
        }
        false
    }

    /// [`Supports::project_test`] for a name `source` binds.
    fn test_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return false;
        }
        if source.tests.contains(name) {
            return true;
        }
        if let Some(refs) = source.classes.get(name) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(local) => self.test_in(source, local, bases, visiting, depth + 1),
                BaseRef::Path(path) => self.project_name(
                    path,
                    &source.dir,
                    bases,
                    visiting,
                    &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
                ),
                BaseRef::Unknown => false,
            });
        }
        match source.from_imports.iter().find(|(local, ..)| local == name) {
            Some((_, module, attribute)) => self.project_name(
                &joined(module, attribute),
                &source.dir,
                bases,
                visiting,
                &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
            ),
            None => false,
        }
    }

    /// [`Supports::project_factory`] for a name `source` binds.
    fn factory_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
    ) -> bool {
        if source.factories.contains(name) {
            return true;
        }
        match source.from_imports.iter().find(|(local, ..)| local == name) {
            Some((_, module, attribute)) => self.project_name(
                &joined(module, attribute),
                &source.dir,
                bases,
                visiting,
                &|this, source, name, visiting| this.factory_in(source, name, bases, visiting),
            ),
            None => false,
        }
    }

    fn may_be_unittest_inner(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
    ) -> bool {
        if is_unittest_base(path) {
            return true;
        }
        if crate::module::no_tests(path) {
            return false;
        }
        if visiting.len() > 16 {
            return true;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        // The longest prefix that is a project module holds the class.
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [class] = &parts[split..] else {
                return true;
            };
            let Some(source) = self.plugin_at(&file) else {
                return true;
            };
            let key = (file, (*class).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = self.class_may_be_unittest(&source, class, bases, visiting, 0);
            visiting.pop();
            return found;
        }
        // Not a project file: an installed package aye can't read.
        true
    }

    /// [`Supports::may_be_unittest`] for a class of `source`.
    fn class_may_be_unittest(
        &self,
        source: &Summary,
        class: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return true;
        }
        if source.plain.contains(class) {
            return false;
        }
        if let Some(refs) = source.classes.get(class) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(name) => {
                    self.class_may_be_unittest(source, name, bases, visiting, depth + 1)
                }
                BaseRef::Path(path) => {
                    self.may_be_unittest_inner(path, &source.dir, bases, visiting)
                }
                BaseRef::Unknown => true,
            });
        }
        // Imported into the module, under that name.
        if let Some((_, module, attribute)) = source
            .from_imports
            .iter()
            .find(|(local, ..)| local == class)
        {
            let path = if module.ends_with('.') {
                format!("{module}{attribute}")
            } else {
                format!("{module}.{attribute}")
            };
            return self.may_be_unittest_inner(&path, &source.dir, bases, visiting);
        }
        // Bound some other way, or through a star import.
        true
    }

    /// What `module.attribute` is, as code in `dir` imports it, following
    /// re-exports through the project's files only as far as needed.
    fn attribute(
        &self,
        module: &str,
        attribute: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<PathBuf>,
    ) -> Attribute {
        let Some(path) = self.resolve(module, dir, bases) else {
            return Attribute::Maybe;
        };
        let Some(source) = self.plugin_at(&path) else {
            return Attribute::Maybe;
        };
        if visiting.contains(&path) || visiting.len() > 8 {
            return Attribute::No;
        }
        if let Some(fixture) = source.fixtures.iter().find(|f| f.attribute == attribute) {
            return Attribute::Fixture(fixture.clone());
        }
        visiting.push(path);
        let found = if let Some((_, from, name)) = source
            .from_imports
            .iter()
            .find(|(local, ..)| local == attribute)
        {
            if crate::module::no_tests(from) {
                Attribute::No
            } else {
                self.attribute(from, name, &source.dir, bases, visiting)
            }
        } else {
            let mut found = Attribute::No;
            let public = match &source.all {
                Some(all) => all.iter().any(|n| n == attribute),
                None => !attribute.starts_with('_'),
            };
            if public {
                for star in source.star_imports.iter().rev() {
                    if crate::module::no_tests(star) {
                        continue;
                    }
                    match self.attribute(star, attribute, &source.dir, bases, visiting) {
                        Attribute::No => {}
                        other => {
                            found = other;
                            break;
                        }
                    }
                }
            }
            found
        };
        visiting.pop();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(source: &str) -> Summary {
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        Summary::build(Path::new("/repo"), source, parsed.syntax())
    }

    #[test]
    fn hooks_and_fixtures() {
        let s = summary(
            r#"
import pytest
from pytest import fixture

def pytest_collection_modifyitems(items): pass

@pytest.fixture(params=[1, 2])
def number(request): return request.param

@fixture(autouse=True)
def setup(tmp_path, number): pass

@pytest.fixture(name="db")
def _db(): pass

@pytest.fixture(params=None)
def plain(): pass

def helper(): pass
"#,
        );
        assert_eq!(
            s.hooks.iter().collect::<Vec<_>>(),
            ["pytest_collection_modifyitems"]
        );
        let fixtures: Vec<_> = s
            .fixtures
            .iter()
            .map(|f| (f.name.as_str(), f.params, f.autouse, f.args.clone()))
            .collect();
        assert_eq!(
            fixtures,
            [
                ("number", true, false, vec!["request".to_string()]),
                (
                    "setup",
                    false,
                    true,
                    vec!["tmp_path".to_string(), "number".to_string()]
                ),
                ("db", false, false, vec![]),
                ("plain", false, false, vec![]),
            ]
        );
    }

    #[test]
    fn collect_file_that_only_handles_yaml() {
        let s = summary(
            "def pytest_collect_file(parent, file_path):\n    if file_path.suffix == '.yaml':\n        return None\n",
        );
        assert!(!s.collect_file_touches_py);
        let s = summary(
            "def pytest_collect_file(parent, file_path):\n    if file_path.suffix == '.py':\n        return None\n",
        );
        assert!(s.collect_file_touches_py);
    }

    #[test]
    fn collect_ignore_lists() {
        let s = summary(
            "import sys\ncollect_ignore = ['setup.py']\nif sys.version_info < (3, 9):\n    collect_ignore.append('old')\ncollect_ignore_glob = ['*_py2.py']\n",
        );
        assert_eq!(
            s.collect_ignore.unwrap().entries,
            [("setup.py".to_string(), false), ("old".to_string(), true)]
        );
        assert_eq!(
            s.collect_ignore_glob.unwrap().entries,
            [("*_py2.py".to_string(), false)]
        );
        let s = summary("collect_ignore = make()\n");
        assert!(s.collect_ignore.unwrap().unknown);
        assert!(summary("x = 1\n").collect_ignore.is_none());
    }

    #[test]
    fn pytest_plugins() {
        let s = summary("pytest_plugins = ['a.b', 'c']\n");
        assert_eq!(s.plugins, ["a.b", "c"]);
        let s = summary("pytest_plugins = 'a'\n");
        assert_eq!(s.plugins, ["a"]);
        let s = summary("pytest_plugins = compute()\n");
        assert!(s.plugins_unknown);
    }

    #[test]
    fn plugins_resolve_to_project_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("tests/plugins")).unwrap();
        std::fs::write(
            root.join("conftest.py"),
            "pytest_plugins = ['tests.plugins.extra', 'pytest_cov']\n",
        )
        .unwrap();
        std::fs::write(root.join("tests/__init__.py"), "").unwrap();
        std::fs::write(root.join("tests/plugins/__init__.py"), "").unwrap();
        std::fs::write(
            root.join("tests/plugins/extra.py"),
            "def pytest_generate_tests(metafunc): pass\n",
        )
        .unwrap();
        let supports = Supports::default();
        let conftest = supports.conftest_in(root).unwrap();
        let expanded = supports.expand(&conftest, &[root.to_path_buf()]);
        assert_eq!(expanded.len(), 2);
        assert!(expanded[1].hooks.contains("pytest_generate_tests"));
        assert!(supports.conftest_in(&root.join("tests")).is_none());
    }

    /// Summaries are told apart by their address in `expanded` and
    /// `exported`, so every caller must get the one the cache keeps: one
    /// that a thread built and dropped, having lost the race to cache it,
    /// could leave its address to another file's summary, which would then
    /// read the first file's cached exports.
    #[test]
    fn threads_summarizing_one_file_get_the_cached_summary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("conftest.py"), "import pytest\n").unwrap();
        let abs = root.join("conftest.py");
        let source = "import pytest\npytest.importorskip('missing')\n";
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let lines = aye_adapter::LineIndex::from_source_text(source);
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("conftest.py"),
            abs: &abs,
            source,
            module: parsed.syntax(),
            lines: &lines,
        };
        for _ in 0..50 {
            let supports = Supports::default();
            let barrier = std::sync::Barrier::new(8);
            let found: Vec<[Arc<Summary>; 3]> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            [
                                supports.of(&file),
                                supports.conftest_in(root).unwrap(),
                                supports.plugin_at(&abs).unwrap(),
                            ]
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for kind in 0..3 {
                assert!(found.iter().all(|f| Arc::ptr_eq(&f[kind], &found[0][kind])));
            }
            assert!(Arc::ptr_eq(&found[0][0], &supports.of(&file)));
            assert!(found[0][0].skips_at_import);
        }
    }

    #[test]
    fn methods_drop_self() {
        let source =
            "class A:\n    def f(self, a, b=1, *, c, d=2): pass\n    def g(a, /, b): pass\n";
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let class = parsed.syntax().body[0].as_class_def_stmt().unwrap();
        let f = class.body[0].as_function_def_stmt().unwrap();
        assert_eq!(funcargnames(f, true), ["a", "c"]);
        assert_eq!(funcargnames(f, false), ["self", "a", "c"]);
        assert_eq!(default_argnames(f), ["b", "d"]);
        // A positional-only `self` is not kept, so pytest drops `b`.
        let g = class.body[1].as_function_def_stmt().unwrap();
        assert_eq!(funcargnames(g, true), Vec::<String>::new());
    }
}
