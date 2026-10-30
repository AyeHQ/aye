//! One support module, summarised ([`Summary`]): what its code does to
//! collection, and what it defines, for questions about the names other
//! modules import from it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{Expr, ModModule, Stmt};
use aye_adapter::{Ranged, TextSize};
use aye_python_semantic::eval::Elem;
use aye_python_semantic::flow::{guarded_by_try, import_time_skip};
use aye_python_semantic::namespace::{Binding, Flag, Namespace, OpKind};
use aye_python_semantic::resolve::Lookup;
use aye_python_semantic::sys_path::SysPath;

use super::fixtures::{FixtureDef, fixture_of};
use super::hooks::makes_items;
use super::ini_changes::{IniChanges, ini_changes};
use crate::PYTEST;

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

impl Summary {
    pub(crate) fn build(dir: &Path, source: &str, module: &ModModule) -> Self {
        Self::of_namespace(dir, source, module, &Namespace::build(&module.body, PYTEST))
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
            // Only its directory matters to what it adds.
            out.sys_path = SysPath::read(&module.body, lookup, dir.join("__file__.py"));
        }
        out
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
}
