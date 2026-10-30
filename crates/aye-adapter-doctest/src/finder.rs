//! What `doctest.DocTestFinder().find(module, module.__name__)` returns for
//! a module, read from its AST: one test per docstring with examples, on the
//! module, its functions and classes, and recursively their methods,
//! properties and nested classes, named by the keys of the namespaces that
//! hold them (`pkg.mod.Class.method`).
//!
//! The finder walks `module.__dict__` and each class's `__dict__`, so what
//! counts is what those namespaces hold once the module is imported. Wherever
//! the source doesn't say that for sure, the file is refined: definitions
//! that may not run, names bound twice, decorators that may replace an
//! object, aliases and wrappers of objects with doctests (the finder visits
//! an object once, under the first name its namespace lists, and a wrapper
//! is a new object), code that changes a namespace at runtime (`setattr`,
//! `globals()`, `exec`, `__doc__` and `__module__` assignments, `type()`
//! with three arguments, functions doing so that run at import), `__test__`,
//! metaclasses and `__init_subclass__`, and docstrings `doctest` can't
//! parse.

use std::collections::{HashMap, HashSet};

use aye_adapter::ast::{self, Expr, Stmt};
use aye_adapter::{Ranged, SourceFile, TextRange, TextSize};
use aye_python_semantic::flow::is_main_guard;

use crate::parser;

/// One test the finder makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// `pkg.mod.Class.method`.
    pub name: String,
    /// The docstring.
    pub range: TextRange,
}

/// What the finder makes of a module.
#[derive(Debug, Clone, Default)]
pub(crate) struct Finding {
    /// Sorted by name, as `DocTestFinder.find` sorts them.
    pub tests: Vec<Found>,
    /// Why the tests can only be known by importing the module.
    pub refine: Option<String>,
    /// Namespace-level `from … import …` statements: their level and module.
    pub from_imports: Vec<(u32, Option<String>)>,
}

/// Decorators that return the function they are given, with its docstring:
/// anywhere, and on methods only.
const SAME_FUNCTION: [&str; 8] = [
    "abc.abstractmethod",
    "typing.final",
    "typing.override",
    "typing_extensions.final",
    "typing_extensions.override",
    "typing.no_type_check",
    "builtins.staticmethod",
    "builtins.classmethod",
];
const METHOD_ONLY: [&str; 3] = [
    "builtins.staticmethod",
    "builtins.classmethod",
    "builtins.property",
];

/// Class decorators that keep the class's members and docstrings (a
/// dataclass with `slots=True` is a new class with the same `__dict__`).
const SAME_CLASS: [&str; 8] = [
    "dataclasses.dataclass",
    "functools.total_ordering",
    "typing.final",
    "typing_extensions.final",
    "enum.unique",
    "typing.runtime_checkable",
    "typing_extensions.runtime_checkable",
    "typing.type_check_only",
];

/// Metaclasses that add nothing doctest finds.
const PLAIN_METACLASSES: [&str; 4] = [
    "builtins.type",
    "abc.ABCMeta",
    "enum.EnumMeta",
    "enum.EnumType",
];

/// Builtins that change a namespace by name, or hand it out.
const NAMESPACE_CHANGERS: [&str; 7] = [
    "setattr", "delattr", "globals", "locals", "vars", "exec", "eval",
];

/// pytest's functions that end a module's import: a call to one of these
/// (or to a method of that name, as `pytest.skip`) at import may skip the
/// module or fail its collection, wherever it is.
const IMPORT_ENDERS: [&str; 5] = ["skip", "importorskip", "xfail", "fail", "exit"];

/// Names that, bound in a namespace, change what the finder sees.
const SPECIAL_MODULE_NAMES: [&str; 3] = ["__doc__", "__name__", "__test__"];
const SPECIAL_CLASS_NAMES: [&str; 2] = ["__doc__", "__module__"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// Runs whenever the module imports.
    Always,
    /// May not run.
    Maybe,
}

#[derive(Debug, Clone, Copy)]
enum Kind<'a> {
    Def(&'a ast::StmtFunctionDef),
    Class(&'a ast::StmtClassDef),
    /// `@x.setter` / `@x.deleter` redefining property `x`: the property's
    /// docstring stays its getter's.
    Accessor,
    /// A `@typing.overload` stub, which the implementation defined after it
    /// replaces.
    Overload,
    Other,
}

#[derive(Debug, Clone, Copy)]
struct Binding<'a> {
    name: &'a str,
    flow: Flow,
    kind: Kind<'a>,
    at: TextSize,
}

/// A namespace's statements, as the finder needs them.
#[derive(Default)]
struct Namespace<'a> {
    bindings: Vec<Binding<'a>>,
    /// Expressions whose value is bound to a name (assignment values, loop
    /// iterables, context managers), and whether its items are (unpacking,
    /// loops).
    values: Vec<(&'a Expr, bool)>,
    /// Decorators, whose arguments a wrapper may copy a docstring from,
    /// but those of property accessors.
    decorators: Vec<&'a Expr>,
    /// Every expression evaluated in the namespace, values included.
    evaluated: Vec<&'a Expr>,
    /// Assignment targets that aren't plain names.
    targets: Vec<&'a Expr>,
    /// Positions of `from … import *`.
    stars: Vec<TextSize>,
    /// A `raise` that always runs.
    raises: bool,
    /// A `raise` of a skip exception (`unittest.SkipTest`,
    /// `pytest.skip.Exception`), which may run.
    skips: bool,
}

/// The module's imports: each name, and the qualified names imports bind it
/// to (empty when something else binds it too).
#[derive(Default)]
struct Imports {
    origins: HashMap<String, Vec<String>>,
    other: HashSet<String>,
}

impl Imports {
    /// The qualified names a dotted expression may refer to: `abc.abstractmethod`
    /// for `abc.abstractmethod` after `import abc`, or `abstractmethod` after
    /// `from abc import abstractmethod`; builtins as `builtins.NAME`. Empty
    /// when anything else may bind its first name.
    fn resolve(&self, expr: &Expr) -> Vec<String> {
        let Some(dotted) = dotted(expr) else {
            return Vec::new();
        };
        let (head, rest) = dotted
            .split_once('.')
            .map_or((dotted.as_str(), ""), |(h, r)| (h, r));
        if self.other.contains(head) {
            return Vec::new();
        }
        let with_rest = |origin: &str| {
            if rest.is_empty() {
                origin.to_string()
            } else {
                format!("{origin}.{rest}")
            }
        };
        match self.origins.get(head) {
            Some(origins) => origins.iter().map(|o| with_rest(o)).collect(),
            None => vec![with_rest(&format!("builtins.{head}"))],
        }
    }

    fn is_one_of(&self, expr: &Expr, allowed: &[&str]) -> bool {
        let resolved = self.resolve(expr);
        !resolved.is_empty() && resolved.iter().all(|r| allowed.contains(&r.as_str()))
    }
}

/// `a.b.c` of a name or attribute chain.
fn dotted(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        Expr::Attribute(attr) => Some(format!("{}.{}", dotted(&attr.value)?, attr.attr.as_str())),
        _ => None,
    }
}

/// The first name of a name, attribute or subscript chain.
fn root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str()),
        Expr::Attribute(attr) => root_name(&attr.value),
        Expr::Subscript(sub) => root_name(&sub.value),
        Expr::Call(call) => root_name(&call.func),
        _ => None,
    }
}

/// The docstring statement of a body.
fn docstring_of(body: &[Stmt]) -> Option<&ast::ExprStringLiteral> {
    match body.first() {
        Some(Stmt::Expr(stmt)) => match stmt.value.as_ref() {
            Expr::StringLiteral(literal) => Some(literal),
            _ => None,
        },
        _ => None,
    }
}

/// A constant condition: `True`, `False`, `1`, `0`.
fn constant(test: &Expr) -> Option<bool> {
    match test {
        Expr::BooleanLiteral(b) => Some(b.value),
        Expr::NumberLiteral(n) => match &n.value {
            ast::Number::Int(i) => Some(i.as_u64() != Some(0)),
            _ => None,
        },
        _ => None,
    }
}

/// Names a pattern binds.
fn pattern_names<'a>(pattern: &'a ast::Pattern, out: &mut Vec<&'a str>) {
    match pattern {
        ast::Pattern::MatchValue(_) | ast::Pattern::MatchSingleton(_) => {}
        ast::Pattern::MatchSequence(p) => {
            for p in &p.patterns {
                pattern_names(p, out);
            }
        }
        ast::Pattern::MatchMapping(p) => {
            for p in &p.patterns {
                pattern_names(p, out);
            }
            if let Some(rest) = &p.rest {
                out.push(rest.as_str());
            }
        }
        ast::Pattern::MatchClass(p) => {
            for p in &p.arguments.patterns {
                pattern_names(p, out);
            }
            for k in &p.arguments.keywords {
                pattern_names(&k.pattern, out);
            }
        }
        ast::Pattern::MatchStar(p) => {
            if let Some(name) = &p.name {
                out.push(name.as_str());
            }
        }
        ast::Pattern::MatchAs(p) => {
            if let Some(inner) = &p.pattern {
                pattern_names(inner, out);
            }
            if let Some(name) = &p.name {
                out.push(name.as_str());
            }
        }
        ast::Pattern::MatchOr(p) => {
            for p in &p.patterns {
                pattern_names(p, out);
            }
        }
    }
}

/// The names an assignment target binds, and its non-name parts.
fn target_names<'a>(target: &'a Expr, names: &mut Vec<&'a str>, others: &mut Vec<&'a Expr>) {
    match target {
        Expr::Name(name) => names.push(name.id.as_str()),
        Expr::Tuple(t) => {
            for e in &t.elts {
                target_names(e, names, others);
            }
        }
        Expr::List(l) => {
            for e in &l.elts {
                target_names(e, names, others);
            }
        }
        Expr::Starred(s) => target_names(&s.value, names, others),
        other => others.push(other),
    }
}

/// Walrus targets in an expression (outside lambdas), which bind in the
/// namespace evaluating it, and their values.
struct Walruses<'a>(Vec<(&'a str, &'a Expr)>);

impl<'a> ast::visitor::Visitor<'a> for Walruses<'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Named(named) => {
                if let Expr::Name(name) = named.target.as_ref() {
                    self.0.push((name.id.as_str(), &named.value));
                }
                ast::visitor::walk_expr(self, expr);
            }
            // A lambda's body runs when it is called, but its defaults are
            // evaluated where it is made.
            Expr::Lambda(lambda) => {
                if let Some(parameters) = &lambda.parameters {
                    for default in defaults(parameters) {
                        self.visit_expr(default);
                    }
                }
            }
            _ => ast::visitor::walk_expr(self, expr),
        }
    }
}

/// The default values of `parameters`, evaluated where the function or
/// lambda is made.
fn defaults(parameters: &ast::Parameters) -> impl Iterator<Item = &Expr> {
    parameters
        .posonlyargs
        .iter()
        .chain(&parameters.args)
        .chain(&parameters.kwonlyargs)
        .filter_map(|p| p.default.as_deref())
}

/// The annotations of a function: its parameters' and its return's.
fn annotations(def: &ast::StmtFunctionDef) -> impl Iterator<Item = &Expr> {
    let parameters = &def.parameters;
    parameters
        .posonlyargs
        .iter()
        .chain(&parameters.args)
        .chain(&parameters.kwonlyargs)
        .filter_map(|p| p.parameter.annotation.as_deref())
        .chain(
            parameters
                .vararg
                .iter()
                .chain(parameters.kwarg.iter())
                .filter_map(|p| p.annotation.as_deref()),
        )
        .chain(def.returns.as_deref())
}

impl<'a> Namespace<'a> {
    fn bind(&mut self, name: &'a str, flow: Flow, kind: Kind<'a>, at: TextSize) {
        self.bindings.push(Binding {
            name,
            flow,
            kind,
            at,
        });
    }

    fn evaluate(&mut self, expr: &'a Expr, flow: Flow) {
        self.evaluated.push(expr);
        let mut walruses = Walruses(Vec::new());
        ast::visitor::Visitor::visit_expr(&mut walruses, expr);
        for (name, value) in walruses.0 {
            self.bind(name, flow, Kind::Other, expr.start());
            self.values.push((value, false));
        }
    }

    fn value(&mut self, expr: &'a Expr, flow: Flow, unpacked: bool) {
        self.values.push((expr, unpacked));
        self.evaluate(expr, flow);
    }

    /// An annotation: evaluated where it is written, unless `from
    /// __future__ import annotations` (or Python 3.14) defers it, so the
    /// names its `:=` bind may or may not be bound.
    fn annotation(&mut self, expr: &'a Expr) {
        let mut walruses = Walruses(Vec::new());
        ast::visitor::Visitor::visit_expr(&mut walruses, expr);
        for (name, value) in walruses.0 {
            self.bind(name, Flow::Maybe, Kind::Other, expr.start());
            self.values.push((value, false));
        }
    }

    fn assign(&mut self, target: &'a Expr, flow: Flow) {
        let mut names = Vec::new();
        let mut others = Vec::new();
        target_names(target, &mut names, &mut others);
        for name in names {
            self.bind(name, flow, Kind::Other, target.start());
        }
        for other in others {
            self.targets.push(other);
            self.evaluate(other, flow);
        }
    }

    /// Gather the bindings and expressions of `body`, run with `flow`.
    /// `overload` tells `@typing.overload`.
    fn gather(
        &mut self,
        body: &'a [Stmt],
        flow: Flow,
        class: bool,
        overload: &dyn Fn(&Expr) -> bool,
    ) {
        for stmt in body {
            match stmt {
                Stmt::FunctionDef(def) => {
                    let kind = if class && is_accessor(def, &self.bindings) {
                        Kind::Accessor
                    } else if def.decorator_list.iter().any(|d| overload(&d.expression)) {
                        Kind::Overload
                    } else {
                        Kind::Def(def)
                    };
                    for decorator in &def.decorator_list {
                        if matches!(kind, Kind::Def(_)) {
                            self.decorators.push(&decorator.expression);
                        }
                        self.evaluate(&decorator.expression, flow);
                    }
                    // Defaults and annotations are evaluated where the
                    // function is made: their `:=` bind here.
                    for default in defaults(&def.parameters) {
                        self.evaluate(default, flow);
                    }
                    for annotation in annotations(def) {
                        self.annotation(annotation);
                    }
                    self.bind(def.name.as_str(), flow, kind, def.start());
                }
                Stmt::ClassDef(class_def) => {
                    for decorator in &class_def.decorator_list {
                        self.decorators.push(&decorator.expression);
                        self.evaluate(&decorator.expression, flow);
                    }
                    // Bases are inherited from, not bound: inherited
                    // members aren't in the class's `__dict__`.
                    if let Some(arguments) = &class_def.arguments {
                        for base in &arguments.args {
                            self.evaluate(base, flow);
                        }
                        for keyword in &arguments.keywords {
                            self.evaluate(&keyword.value, flow);
                        }
                    }
                    self.bind(
                        class_def.name.as_str(),
                        flow,
                        Kind::Class(class_def),
                        class_def.start(),
                    );
                }
                Stmt::Assign(assign) => {
                    // Only a name bound to the value can alias it.
                    if assign.targets.iter().any(binds_name) {
                        let unpacked = assign.targets.iter().any(|t| {
                            matches!(t, Expr::Tuple(_) | Expr::List(_) | Expr::Starred(_))
                        });
                        self.value(&assign.value, flow, unpacked);
                    } else {
                        self.evaluate(&assign.value, flow);
                    }
                    for target in &assign.targets {
                        self.assign(target, flow);
                    }
                }
                Stmt::AugAssign(assign) => {
                    self.value(&assign.value, flow, false);
                    self.assign(&assign.target, flow);
                }
                Stmt::AnnAssign(assign) => {
                    self.annotation(&assign.annotation);
                    if let Some(value) = &assign.value {
                        self.value(value, flow, false);
                        self.assign(&assign.target, flow);
                    } else if !matches!(assign.target.as_ref(), Expr::Name(_)) {
                        self.evaluate(&assign.target, flow);
                    }
                }
                Stmt::TypeAlias(alias) => {
                    if let Expr::Name(name) = alias.name.as_ref() {
                        self.bind(name.id.as_str(), flow, Kind::Other, alias.start());
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let name = alias.asname.as_ref().map_or_else(
                            || alias.name.as_str().split('.').next().unwrap_or_default(),
                            ast::Identifier::as_str,
                        );
                        self.bind(name, flow, Kind::Other, import.start());
                    }
                }
                Stmt::ImportFrom(import) => {
                    for alias in &import.names {
                        if alias.name.as_str() == "*" {
                            self.stars.push(import.start());
                        } else {
                            let name = alias
                                .asname
                                .as_ref()
                                .map_or_else(|| alias.name.as_str(), ast::Identifier::as_str);
                            self.bind(name, flow, Kind::Other, import.start());
                        }
                    }
                }
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        self.assign(target, flow);
                    }
                }
                Stmt::Expr(expr) => self.evaluate(&expr.value, flow),
                Stmt::If(stmt_if) => {
                    if is_main_guard(&stmt_if.test) {
                        // Never runs on import; a plain `else`, always.
                        let mut flow = flow;
                        for clause in &stmt_if.elif_else_clauses {
                            if let Some(test) = &clause.test {
                                self.evaluate(test, flow);
                                flow = Flow::Maybe;
                            }
                            self.gather(&clause.body, flow, class, overload);
                        }
                        continue;
                    }
                    self.evaluate(&stmt_if.test, flow);
                    match constant(&stmt_if.test) {
                        Some(true) => self.gather(&stmt_if.body, flow, class, overload),
                        Some(false) => {}
                        None => self.gather(&stmt_if.body, Flow::Maybe, class, overload),
                    }
                    for clause in &stmt_if.elif_else_clauses {
                        if let Some(test) = &clause.test {
                            self.evaluate(test, Flow::Maybe);
                        }
                        if constant(&stmt_if.test) != Some(true) {
                            self.gather(&clause.body, Flow::Maybe, class, overload);
                        }
                    }
                }
                Stmt::For(stmt_for) => {
                    self.value(&stmt_for.iter, flow, true);
                    self.assign(&stmt_for.target, Flow::Maybe);
                    self.gather(&stmt_for.body, Flow::Maybe, class, overload);
                    self.gather(&stmt_for.orelse, Flow::Maybe, class, overload);
                }
                Stmt::While(stmt_while) => {
                    self.evaluate(&stmt_while.test, flow);
                    self.gather(&stmt_while.body, Flow::Maybe, class, overload);
                    self.gather(&stmt_while.orelse, Flow::Maybe, class, overload);
                }
                Stmt::With(with) => {
                    for item in &with.items {
                        self.value(&item.context_expr, flow, false);
                        if let Some(vars) = &item.optional_vars {
                            self.assign(vars, Flow::Maybe);
                        }
                    }
                    self.gather(&with.body, Flow::Maybe, class, overload);
                }
                Stmt::Try(stmt_try) => {
                    self.gather(&stmt_try.body, Flow::Maybe, class, overload);
                    for handler in &stmt_try.handlers {
                        let ast::ExceptHandler::ExceptHandler(handler) = handler;
                        if let Some(type_) = &handler.type_ {
                            self.evaluate(type_, Flow::Maybe);
                        }
                        if let Some(name) = &handler.name {
                            self.bind(name.as_str(), Flow::Maybe, Kind::Other, handler.start());
                        }
                        self.gather(&handler.body, Flow::Maybe, class, overload);
                    }
                    self.gather(&stmt_try.orelse, Flow::Maybe, class, overload);
                    // Runs whenever the import gets past the `try`.
                    self.gather(&stmt_try.finalbody, flow, class, overload);
                }
                Stmt::Match(stmt_match) => {
                    self.evaluate(&stmt_match.subject, flow);
                    for case in &stmt_match.cases {
                        let mut names = Vec::new();
                        pattern_names(&case.pattern, &mut names);
                        for name in names {
                            self.bind(name, Flow::Maybe, Kind::Other, case.start());
                        }
                        if let Some(guard) = &case.guard {
                            self.evaluate(guard, Flow::Maybe);
                        }
                        self.gather(&case.body, Flow::Maybe, class, overload);
                    }
                }
                Stmt::Raise(raise) => {
                    if flow == Flow::Always {
                        self.raises = true;
                    }
                    if raise.exc.as_deref().is_some_and(names_skip) {
                        self.skips = true;
                    }
                }
                Stmt::Assert(stmt) => self.evaluate(&stmt.test, flow),
                Stmt::Return(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Pass(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::IpyEscapeCommand(_) => {}
            }
        }
    }
}

/// Whether a raised expression is a skip exception: `SkipTest`,
/// `unittest.SkipTest(...)`, `pytest.skip.Exception(...)`, `Skipped`.
fn names_skip(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => names_skip(&call.func),
        Expr::Name(name) => matches!(name.id.as_str(), "SkipTest" | "Skipped"),
        Expr::Attribute(attr) => match attr.attr.as_str() {
            "SkipTest" | "Skipped" => true,
            "Exception" => {
                matches!(attr.value.as_ref(), Expr::Attribute(a) if a.attr.as_str() == "skip")
            }
            _ => false,
        },
        _ => false,
    }
}

/// Whether an assignment target binds a name.
fn binds_name(target: &Expr) -> bool {
    match target {
        Expr::Name(_) => true,
        Expr::Tuple(t) => t.elts.iter().any(binds_name),
        Expr::List(l) => l.elts.iter().any(binds_name),
        Expr::Starred(s) => binds_name(&s.value),
        _ => false,
    }
}

/// Whether a decorator is given an object with doctests, or belongs to
/// one, so that what it returns may copy its docstring (`functools.wraps(f)`,
/// `@x.setter` on a function not named `x`).
fn decorator_aliases(expr: &Expr, bearing: &HashSet<&str>) -> bool {
    match expr {
        Expr::Call(call) => {
            let mut scan = Scan::new(bearing);
            ast::visitor::walk_arguments(&mut scan, &call.arguments);
            scan.mentions || decorator_aliases(&call.func, bearing)
        }
        Expr::Attribute(_) => root_name(expr).is_some_and(|n| bearing.contains(n)),
        _ => false,
    }
}

/// `@x.setter` or `@x.deleter` on `def x`, with `x` bound in the class
/// body already (to a property, presumably; if not, the decorator fails).
fn is_accessor(def: &ast::StmtFunctionDef, bindings: &[Binding<'_>]) -> bool {
    let [decorator] = def.decorator_list.as_slice() else {
        return false;
    };
    let Expr::Attribute(attr) = &decorator.expression else {
        return false;
    };
    let Expr::Name(base) = attr.value.as_ref() else {
        return false;
    };
    matches!(attr.attr.as_str(), "setter" | "deleter")
        && base.id.as_str() == def.name.as_str()
        && bindings.iter().any(|b| b.name == def.name.as_str())
}

/// Visits everything below an expression or a body, outside nothing.
struct Scan<'s, 'a> {
    /// Names whose objects have doctests.
    bearing: &'s HashSet<&'a str>,
    /// A non-callee reference to a bearing name.
    mentions: bool,
    /// A call to one of [`NAMESPACE_CHANGERS`], `sys.modules`, or
    /// `type()` with three arguments.
    changes_namespace: bool,
    /// A call to one of [`IMPORT_ENDERS`].
    ends_import: bool,
    /// A string containing `>>>` inside a call.
    doc_in_call: bool,
    /// Callees that are plain names.
    callees: Vec<&'a str>,
    depth_in_call: usize,
}

impl<'s, 'a> Scan<'s, 'a> {
    fn new(bearing: &'s HashSet<&'a str>) -> Self {
        Self {
            bearing,
            mentions: false,
            changes_namespace: false,
            ends_import: false,
            doc_in_call: false,
            callees: Vec::new(),
            depth_in_call: 0,
        }
    }
}

impl<'a> ast::visitor::Visitor<'a> for Scan<'_, 'a> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Name(name) => {
                if self.bearing.contains(name.id.as_str()) {
                    self.mentions = true;
                }
            }
            Expr::Call(call) => {
                // The callee itself is called, not passed on.
                match call.func.as_ref() {
                    Expr::Name(name) => {
                        let id = name.id.as_str();
                        self.callees.push(id);
                        if NAMESPACE_CHANGERS.contains(&id)
                            || (id == "type" && call.arguments.args.len() == 3)
                        {
                            self.changes_namespace = true;
                        }
                        self.ends_import |= IMPORT_ENDERS.contains(&id);
                    }
                    Expr::Attribute(attr) => {
                        self.ends_import |= IMPORT_ENDERS.contains(&attr.attr.as_str());
                        // `C.make(...)`: the method is called, `C` isn't passed.
                        if !matches!(attr.value.as_ref(), Expr::Name(_)) {
                            self.visit_expr(&attr.value);
                        }
                    }
                    other => self.visit_expr(other),
                }
                self.depth_in_call += 1;
                ast::visitor::walk_arguments(self, &call.arguments);
                self.depth_in_call -= 1;
            }
            Expr::Attribute(attr) => {
                if attr.attr.as_str() == "modules"
                    && matches!(attr.value.as_ref(), Expr::Name(n) if n.id.as_str() == "sys")
                {
                    self.changes_namespace = true;
                }
                ast::visitor::walk_expr(self, expr);
            }
            Expr::StringLiteral(literal) => {
                let value = literal.value.to_str();
                if self.depth_in_call > 0 && value.contains(">>>") {
                    self.doc_in_call = true;
                }
            }
            _ => ast::visitor::walk_expr(self, expr),
        }
    }

    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Global(_) | Stmt::Nonlocal(_) => self.changes_namespace = true,
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if changes_object(target) {
                        self.changes_namespace = true;
                    }
                }
                ast::visitor::walk_stmt(self, stmt);
            }
            _ => ast::visitor::walk_stmt(self, stmt),
        }
    }
}

/// An assignment target that changes what doctest sees of an object:
/// `X.__doc__`, `X.__module__`, `X.__dict__[...]`.
fn changes_object(target: &Expr) -> bool {
    match target {
        Expr::Attribute(attr) => {
            matches!(
                attr.attr.as_str(),
                "__doc__" | "__module__" | "__name__" | "__qualname__"
            )
        }
        Expr::Subscript(sub) => {
            matches!(sub.value.as_ref(), Expr::Attribute(a) if a.attr.as_str() == "__dict__")
        }
        Expr::Tuple(t) => t.elts.iter().any(changes_object),
        Expr::List(l) => l.elts.iter().any(changes_object),
        Expr::Starred(s) => changes_object(&s.value),
        _ => false,
    }
}

/// Builtins whose result is never a routine, a class or a property,
/// whatever they are given.
const INERT_BUILTINS: [&str; 37] = [
    "builtins.abs",
    "builtins.all",
    "builtins.any",
    "builtins.ascii",
    "builtins.bin",
    "builtins.bool",
    "builtins.bytearray",
    "builtins.bytes",
    "builtins.callable",
    "builtins.chr",
    "builtins.complex",
    "builtins.dict",
    "builtins.dir",
    "builtins.divmod",
    "builtins.enumerate",
    "builtins.filter",
    "builtins.float",
    "builtins.format",
    "builtins.frozenset",
    "builtins.hasattr",
    "builtins.hash",
    "builtins.hex",
    "builtins.id",
    "builtins.int",
    "builtins.isinstance",
    "builtins.issubclass",
    "builtins.iter",
    "builtins.len",
    "builtins.list",
    "builtins.map",
    "builtins.print",
    "builtins.range",
    "builtins.repr",
    "builtins.set",
    "builtins.sorted",
    "builtins.str",
    "builtins.tuple",
];

/// What calling something gives, as far as aliasing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Calling {
    /// Never something doctest finds: a builtin making a value, an
    /// instance of a plain class of the module.
    Inert,
    /// Something doctest may find with a docstring of its own: an instance
    /// of a class with doctests that may be a descriptor (an instance's
    /// `__doc__` is its class's).
    Odd,
    /// A wrapper of what it is given, perhaps.
    Arguments,
}

/// Whether an expression's value may be an object with doctests (an alias)
/// or wrap one (a call given one). `calling` tells what a call gives;
/// `unpacked`, whether a tuple's or a list's items are bound (a container
/// itself is never what doctest finds).
fn may_alias(
    expr: &Expr,
    bearing: &HashSet<&str>,
    calling: &dyn Fn(&Expr) -> Calling,
    unpacked: bool,
) -> bool {
    let alias = |e: &Expr| may_alias(e, bearing, calling, unpacked);
    match expr {
        Expr::Name(name) => bearing.contains(name.id.as_str()),
        Expr::Attribute(_) => root_name(expr).is_some_and(|n| bearing.contains(n)),
        Expr::Call(call) => match calling(&call.func) {
            Calling::Inert => false,
            Calling::Odd => true,
            Calling::Arguments => {
                let mut scan = Scan::new(bearing);
                ast::visitor::Visitor::visit_expr(&mut scan, expr);
                scan.mentions
            }
        },
        Expr::Subscript(_) => {
            let mut scan = Scan::new(bearing);
            ast::visitor::Visitor::visit_expr(&mut scan, expr);
            scan.mentions
        }
        Expr::If(e) => alias(&e.body) || alias(&e.orelse),
        Expr::BoolOp(e) => e.values.iter().any(alias),
        Expr::Named(e) => alias(&e.value),
        Expr::Starred(e) => alias(&e.value),
        Expr::Await(e) => alias(&e.value),
        Expr::Tuple(t) => unpacked && t.elts.iter().any(alias),
        Expr::List(l) => unpacked && l.elts.iter().any(alias),
        _ => false,
    }
}

/// Whether instances of a module class are surely plain objects, which
/// doctest never takes for routines: the class and its bases (all classes
/// of the module, or `object`) define no `__get__` or `__new__` and have no
/// metaclass or decorator.
fn plain_instances(name: &str, defs: &HashMap<&str, Kind<'_>>, seen: &mut HashSet<String>) -> bool {
    if !seen.insert(name.to_string()) {
        return false;
    }
    let Some(Kind::Class(class_def)) = defs.get(name) else {
        return false;
    };
    if !class_def.decorator_list.is_empty() || !class_def.keywords().is_empty() {
        return false;
    }
    let special = class_def.body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(def) => matches!(def.name.as_str(), "__get__" | "__new__"),
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .any(|t| matches!(t, Expr::Name(n) if matches!(n.id.as_str(), "__get__" | "__new__"))),
        _ => false,
    });
    if special {
        return false;
    }
    class_def.bases().iter().all(|base| match base {
        Expr::Name(n) if n.id.as_str() == "object" => true,
        Expr::Name(n) => plain_instances(n.id.as_str(), defs, seen),
        _ => false,
    })
}

/// Reads a module's docstrings.
pub(crate) struct Finder<'f, 'a> {
    file: &'f SourceFile<'a>,
    /// The file has carriage returns: line endings to fold.
    has_cr: bool,
    reasons: Vec<String>,
    imports: Imports,
    /// Module functions and classes (the last definition of each name,
    /// wherever it is), for calls made at import time.
    module_defs: HashMap<&'a str, Kind<'a>>,
}

impl<'f, 'a> Finder<'f, 'a> {
    pub(crate) fn new(file: &'f SourceFile<'a>) -> Self {
        Self {
            file,
            has_cr: file.source.contains('\r'),
            reasons: Vec::new(),
            imports: Imports::default(),
            module_defs: HashMap::new(),
        }
    }

    fn refine(&mut self, reason: String) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }

    /// A docstring's value as Python sees it, line endings folded.
    fn doc_value(&mut self, literal: &ast::ExprStringLiteral, owner: &str) -> Option<String> {
        let value = literal.value.to_str();
        if !value.contains(">>>") {
            return None;
        }
        if !(value.contains('\r') && self.has_cr) {
            return Some(value.to_string());
        }
        // A carriage return may come from a line ending, which Python folds,
        // or from an escape, which it keeps.
        let escapes = literal
            .value
            .iter()
            .any(|part| !part.flags.prefix().is_raw() && self.file.text(part.range).contains('\\'));
        if escapes {
            self.refine(format!(
                "the docstring of `{owner}` has escapes and the file has carriage returns"
            ));
        }
        Some(parser::universal_newlines(value))
    }

    /// Whether `body`'s docstring has examples (refining when unsure).
    fn has_examples(&mut self, body: &[Stmt], owner: &str) -> Option<TextRange> {
        let literal = docstring_of(body)?;
        let value = self.doc_value(literal, owner)?;
        let doc = parser::docstring(&value);
        if let Some(why) = doc.unsure {
            self.refine(format!("the docstring of `{owner}`: {why}"));
        }
        doc.examples.then(|| literal.range())
    }

    /// The module's tests, named from `module` (its `__name__`).
    pub(crate) fn find(mut self, module: &str) -> Finding {
        let body = &self.file.module.body;
        self.read_imports(body);
        let mut tests = Vec::new();
        if let Some(range) = self.has_examples(body, module) {
            tests.push(Found {
                name: module.to_string(),
                range,
            });
        }
        let mut from_imports = Vec::new();
        collect_from_imports(body, &mut from_imports);
        let outer = HashSet::new();
        let (found, bearing) = self.namespace(module, body, false, &outer);
        tests.extend(found);
        self.whole_file(&bearing);
        tests.sort_by(|a, b| a.name.cmp(&b.name));
        Finding {
            tests,
            refine: (!self.reasons.is_empty()).then(|| self.reasons.join("; ")),
            from_imports,
        }
    }

    /// Every import in the module, wherever it is, for resolving
    /// decorators: a name some import binds and nothing else does.
    fn read_imports(&mut self, body: &'a [Stmt]) {
        let mut ns = Namespace::default();
        ns.gather(body, Flow::Always, false, &|_| false);
        let mut imported: HashSet<&str> = HashSet::new();
        for stmt in body {
            record_imports(stmt, &mut self.imports, &mut imported);
        }
        for binding in &ns.bindings {
            if !imported.contains(binding.name) {
                self.imports.other.insert(binding.name.to_string());
            }
            if let Kind::Def(_) | Kind::Class(_) = binding.kind {
                self.module_defs.insert(binding.name, binding.kind);
            }
        }
    }

    /// One namespace (the module's or a class's): its tests, and the names
    /// in it whose objects have doctests.
    fn namespace(
        &mut self,
        prefix: &str,
        body: &'a [Stmt],
        class: bool,
        outer: &HashSet<&'a str>,
    ) -> (Vec<Found>, HashSet<&'a str>) {
        let mut ns = Namespace::default();
        let imports = &self.imports;
        let overload =
            |e: &Expr| imports.is_one_of(e, &["typing.overload", "typing_extensions.overload"]);
        ns.gather(body, Flow::Always, class, &overload);
        // Each definition's tests, wherever it is.
        let mut tests_of: Vec<Vec<Found>> = Vec::with_capacity(ns.bindings.len());
        let mut bearing: HashSet<&'a str> = HashSet::new();
        for binding in &ns.bindings {
            let name = format!("{prefix}.{}", binding.name);
            let found = match binding.kind {
                Kind::Def(def) => self
                    .has_examples(&def.body, &name)
                    .map(|range| vec![Found { name, range }])
                    .unwrap_or_default(),
                Kind::Class(class_def) => {
                    let mut found: Vec<Found> = self
                        .has_examples(&class_def.body, &name)
                        .map(|range| Found {
                            name: name.clone(),
                            range,
                        })
                        .into_iter()
                        .collect();
                    let mut visible: HashSet<&'a str> = outer.clone();
                    visible.extend(bearing.iter().copied());
                    let (members, _) = self.namespace(&name, &class_def.body, true, &visible);
                    found.extend(members);
                    found
                }
                Kind::Accessor | Kind::Overload | Kind::Other => Vec::new(),
            };
            if !found.is_empty() {
                bearing.insert(binding.name);
            }
            tests_of.push(found);
        }
        let special: &[&str] = if class {
            &SPECIAL_CLASS_NAMES
        } else {
            &SPECIAL_MODULE_NAMES
        };
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for binding in &ns.bindings {
            if !matches!(binding.kind, Kind::Accessor | Kind::Overload) {
                *counts.entry(binding.name).or_default() += 1;
            }
            if special.contains(&binding.name) {
                self.refine(format!("`{prefix}` binds `{}`", binding.name));
            }
        }
        let mut tests = Vec::new();
        for (binding, found) in ns.bindings.iter().zip(&mut tests_of) {
            if found.is_empty() {
                continue;
            }
            let qualified = format!("{prefix}.{}", binding.name);
            if binding.flow == Flow::Maybe {
                self.refine(format!("`{qualified}` is defined conditionally"));
            }
            if counts.get(binding.name).copied().unwrap_or(0) > 1 {
                self.refine(format!("`{qualified}` is bound more than once"));
            }
            if ns.stars.iter().any(|star| *star > binding.at) {
                self.refine(format!("a star import may rebind `{qualified}`"));
            }
            self.check_decorators(binding, class, &qualified);
            if binding.flow == Flow::Always {
                tests.append(found);
            }
        }
        for binding in &ns.bindings {
            if let Kind::Class(class_def) = binding.kind {
                self.check_class(class_def, prefix);
            }
        }
        let mut visible: HashSet<&'a str> = outer.clone();
        visible.extend(bearing.iter().copied());
        self.check_expressions(&ns, prefix, &visible);
        if ns.raises {
            self.refine(format!("`{prefix}` raises while it is created"));
        }
        if ns.skips {
            self.refine(format!("`{prefix}` may raise a skip while it is created"));
        }
        (tests, bearing)
    }

    /// Decorators on a definition with doctests must keep it as it is.
    fn check_decorators(&mut self, binding: &Binding<'a>, class: bool, qualified: &str) {
        let decorators = match binding.kind {
            Kind::Def(def) => &def.decorator_list,
            Kind::Class(class_def) => &class_def.decorator_list,
            Kind::Accessor | Kind::Overload | Kind::Other => return,
        };
        for decorator in decorators {
            let expression = &decorator.expression;
            let ok = match binding.kind {
                Kind::Def(_) => {
                    let mut allowed: Vec<&str> = SAME_FUNCTION
                        .iter()
                        .copied()
                        .filter(|d| class || !METHOD_ONLY.contains(d))
                        .collect();
                    if class {
                        allowed.push("builtins.property");
                    }
                    self.imports.is_one_of(expression, &allowed)
                }
                Kind::Class(_) => {
                    let target = match expression {
                        Expr::Call(call) => call.func.as_ref(),
                        other => other,
                    };
                    self.imports.is_one_of(target, &SAME_CLASS)
                }
                Kind::Accessor | Kind::Overload | Kind::Other => true,
            };
            if !ok {
                let shown = self.file.text(expression.range()).to_string();
                self.refine(format!("`{qualified}` is decorated with `{shown}`"));
            }
        }
    }

    /// Metaclasses and `__init_subclass__` may add members.
    fn check_class(&mut self, class_def: &'a ast::StmtClassDef, prefix: &str) {
        for keyword in class_def.keywords() {
            if keyword
                .arg
                .as_ref()
                .is_some_and(|a| a.as_str() == "metaclass")
                && !self.imports.is_one_of(&keyword.value, &PLAIN_METACLASSES)
            {
                self.refine(format!(
                    "`{prefix}.{}` has a metaclass",
                    class_def.name.as_str()
                ));
            }
        }
    }

    /// What calling `func` gives. A module function's result is what it
    /// returns (`check_calls` reads its body), so a wrapper at most of what
    /// it is given; a plain module class's is a plain instance; a class
    /// with doctests may make descriptors with its docstring.
    fn calling(
        &self,
        func: &Expr,
        bearing: &HashSet<&str>,
        defs: &HashMap<&str, Kind<'_>>,
    ) -> Calling {
        let Expr::Name(name) = func else {
            return Calling::Arguments;
        };
        let name = name.id.as_str();
        let kind = defs.get(name);
        let class = matches!(kind, Some(Kind::Class(_)));
        if class && plain_instances(name, defs, &mut HashSet::new()) {
            return Calling::Inert;
        }
        // A class with doctests, or a name with doctests that isn't one of
        // the module's functions (a nested class of a class body).
        if bearing.contains(name) && (class || kind.is_none()) {
            return Calling::Odd;
        }
        if kind.is_none() && self.imports.is_one_of(func, &INERT_BUILTINS) {
            return Calling::Inert;
        }
        Calling::Arguments
    }

    /// The namespace's expressions may alias, wrap or add objects with
    /// doctests, or change the namespace.
    fn check_expressions(&mut self, ns: &Namespace<'a>, prefix: &str, bearing: &HashSet<&'a str>) {
        // The functions and classes a call may name: the namespace's own
        // (a class body's helpers), then the module's.
        let mut defs = self.module_defs.clone();
        for binding in &ns.bindings {
            if let Kind::Def(_) | Kind::Class(_) = binding.kind {
                defs.insert(binding.name, binding.kind);
            }
        }
        let calling = |func: &Expr| self.calling(func, bearing, &defs);
        let aliases: Vec<&Expr> = ns
            .values
            .iter()
            .copied()
            .filter(|(value, unpacked)| may_alias(value, bearing, &calling, *unpacked))
            .map(|(value, _)| value)
            .collect();
        for value in aliases {
            let shown = self.file.text(value.range()).to_string();
            self.refine(format!(
                "`{shown}` in `{prefix}` may alias or wrap an object with doctests"
            ));
        }
        for decorator in &ns.decorators {
            if decorator_aliases(decorator, bearing) {
                let shown = self.file.text(decorator.range()).to_string();
                self.refine(format!(
                    "`@{shown}` in `{prefix}` may copy the docstring of an object with doctests"
                ));
            }
        }
        for target in &ns.targets {
            let local = root_name(target).is_some_and(|n| defs.contains_key(n));
            if local || changes_object(target) {
                let shown = self.file.text(target.range()).to_string();
                self.refine(format!("`{prefix}` assigns to `{shown}`"));
            }
        }
        let mut called: Vec<&'a str> = Vec::new();
        for expr in &ns.evaluated {
            let mut scan = Scan::new(bearing);
            ast::visitor::Visitor::visit_expr(&mut scan, expr);
            if scan.changes_namespace {
                let shown = self.file.text(expr.range()).to_string();
                self.refine(format!("`{shown}` in `{prefix}` may change a namespace"));
            }
            if scan.doc_in_call {
                let shown = self.file.text(expr.range()).to_string();
                self.refine(format!("`{shown}` in `{prefix}` may make a docstring"));
            }
            if scan.ends_import {
                let shown = self.file.text(expr.range()).to_string();
                self.refine(format!(
                    "`{shown}` in `{prefix}` may skip the module or fail its import"
                ));
            }
            called.extend(scan.callees);
        }
        // Decorators are called too.
        for binding in &ns.bindings {
            let decorators = match binding.kind {
                Kind::Def(def) => &def.decorator_list,
                Kind::Class(class_def) => &class_def.decorator_list,
                Kind::Accessor | Kind::Overload | Kind::Other => continue,
            };
            for decorator in decorators {
                if let Expr::Name(name) = &decorator.expression {
                    called.push(name.id.as_str());
                }
            }
        }
        self.check_calls(&called, prefix, bearing, &defs);
    }

    /// Module functions and classes called while the module imports (and
    /// those they call) must not change a namespace or make docstrings.
    fn check_calls(
        &mut self,
        called: &[&'a str],
        prefix: &str,
        bearing: &HashSet<&'a str>,
        defs: &HashMap<&'a str, Kind<'a>>,
    ) {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut queue: Vec<&'a str> = called.to_vec();
        while let Some(name) = queue.pop() {
            if !seen.insert(name) {
                continue;
            }
            let Some(kind) = defs.get(name).copied() else {
                continue;
            };
            // A function's result may be bound, so what it returns matters;
            // a class's is an instance (`may_alias` checks those), unless
            // `__new__` returns something else.
            let bodies: Vec<(&'a [Stmt], bool)> = match kind {
                Kind::Def(def) => vec![(&def.body[..], true)],
                Kind::Class(class_def) => class_def
                    .body
                    .iter()
                    .filter_map(|stmt| match stmt {
                        Stmt::FunctionDef(def) => {
                            Some((&def.body[..], def.name.as_str() == "__new__"))
                        }
                        _ => None,
                    })
                    .collect(),
                Kind::Accessor | Kind::Overload | Kind::Other => continue,
            };
            let calling = |func: &Expr| self.calling(func, bearing, defs);
            let mut flagged = false;
            for (body, returns) in bodies {
                let mut scan = Scan::new(bearing);
                // The function's own docstring is no string in a call.
                let start = usize::from(docstring_of(body).is_some());
                for stmt in &body[start..] {
                    ast::visitor::Visitor::visit_stmt(&mut scan, stmt);
                }
                let makes_doc = body[start..].iter().any(contains_doctest_string);
                let aliases = returns && returns_alias(body, bearing, &calling);
                flagged |= scan.changes_namespace || scan.ends_import || aliases || makes_doc;
                queue.extend(scan.callees);
            }
            if flagged {
                self.refine(format!(
                    "`{prefix}` calls `{name}` while it is created, which may change what doctest finds"
                ));
            }
        }
    }

    /// Rules that look at the whole file: `global` statements naming
    /// objects with doctests or the module's `__test__`, `__doc__` or
    /// `__name__`, and subclasses of a class defining `__init_subclass__`
    /// (which may add members to them).
    fn whole_file(&mut self, bearing: &HashSet<&'a str>) {
        struct Whole<'s, 'a> {
            bearing: &'s HashSet<&'a str>,
            globals: Vec<String>,
            /// Every class: its name, its bases' names (`None` for a base
            /// that isn't a plain name, so a class of another module),
            /// whether it defines `__init_subclass__`.
            classes: Vec<(&'a str, Vec<Option<&'a str>>, bool)>,
        }
        impl<'a> ast::visitor::Visitor<'a> for Whole<'_, 'a> {
            fn visit_stmt(&mut self, stmt: &'a Stmt) {
                match stmt {
                    Stmt::Global(global) => {
                        for name in &global.names {
                            if self.bearing.contains(name.as_str())
                                || SPECIAL_MODULE_NAMES.contains(&name.as_str())
                            {
                                self.globals.push(name.as_str().to_string());
                            }
                        }
                    }
                    Stmt::ClassDef(class_def) => {
                        let bases = class_def
                            .bases()
                            .iter()
                            .map(|base| match base {
                                Expr::Name(n) => Some(n.id.as_str()),
                                _ => None,
                            })
                            .collect();
                        let hook = class_def.body.iter().any(|stmt| {
                            matches!(stmt, Stmt::FunctionDef(def) if def.name.as_str() == "__init_subclass__")
                        });
                        self.classes.push((class_def.name.as_str(), bases, hook));
                    }
                    _ => {}
                }
                ast::visitor::walk_stmt(self, stmt);
            }
        }
        let mut whole = Whole {
            bearing,
            globals: Vec::new(),
            classes: Vec::new(),
        };
        for stmt in &self.file.module.body {
            ast::visitor::Visitor::visit_stmt(&mut whole, stmt);
        }
        for name in whole.globals {
            self.refine(format!("a function declares `global {name}`"));
        }
        // Classes whose subclasses run an `__init_subclass__` of the module.
        let mut hooked: HashSet<&str> = whole
            .classes
            .iter()
            .filter(|(_, _, hook)| *hook)
            .map(|(name, _, _)| *name)
            .collect();
        if hooked.is_empty() {
            return;
        }
        loop {
            let before = hooked.len();
            for (name, bases, _) in &whole.classes {
                if bases.iter().any(|b| b.is_some_and(|b| hooked.contains(b))) {
                    hooked.insert(name);
                }
            }
            if hooked.len() == before {
                break;
            }
        }
        let subclassed = whole
            .classes
            .iter()
            .find(|(_, bases, _)| bases.iter().any(|b| b.is_some_and(|b| hooked.contains(b))));
        if let Some((name, _, _)) = subclassed {
            self.refine(format!(
                "`{name}` subclasses a class whose `__init_subclass__` may add members"
            ));
        }
    }
}

/// Imports a statement (or the statements nested in it, outside functions
/// and classes) make, by name.
fn record_imports<'a>(stmt: &'a Stmt, imports: &mut Imports, imported: &mut HashSet<&'a str>) {
    match stmt {
        Stmt::Import(import) => {
            for alias in &import.names {
                let (name, origin) = if let Some(asname) = &alias.asname {
                    (asname.as_str(), alias.name.as_str().to_string())
                } else {
                    let head = alias.name.as_str().split('.').next().unwrap_or_default();
                    (head, head.to_string())
                };
                imported.insert(name);
                imports
                    .origins
                    .entry(name.to_string())
                    .or_default()
                    .push(origin);
            }
        }
        Stmt::ImportFrom(import) => {
            let module = import.module.as_ref().map(ast::Identifier::as_str);
            for alias in &import.names {
                if alias.name.as_str() == "*" {
                    continue;
                }
                let name = alias
                    .asname
                    .as_ref()
                    .map_or_else(|| alias.name.as_str(), ast::Identifier::as_str);
                imported.insert(name);
                let origin = match (import.level, module) {
                    (0, Some(module)) => format!("{module}.{}", alias.name.as_str()),
                    // A relative import is a project module.
                    _ => format!(".{}", alias.name.as_str()),
                };
                imports
                    .origins
                    .entry(name.to_string())
                    .or_default()
                    .push(origin);
            }
        }
        Stmt::If(s) => {
            for stmt in &s.body {
                record_imports(stmt, imports, imported);
            }
            for clause in &s.elif_else_clauses {
                for stmt in &clause.body {
                    record_imports(stmt, imports, imported);
                }
            }
        }
        Stmt::Try(s) => {
            for body in [&s.body, &s.orelse, &s.finalbody] {
                for stmt in body {
                    record_imports(stmt, imports, imported);
                }
            }
            for handler in &s.handlers {
                let ast::ExceptHandler::ExceptHandler(handler) = handler;
                for stmt in &handler.body {
                    record_imports(stmt, imports, imported);
                }
            }
        }
        Stmt::With(s) => {
            for stmt in &s.body {
                record_imports(stmt, imports, imported);
            }
        }
        _ => {}
    }
}

/// Namespace-level `from … import …`, wherever they are outside functions
/// and classes.
fn collect_from_imports(body: &[Stmt], out: &mut Vec<(u32, Option<String>)>) {
    for stmt in body {
        match stmt {
            Stmt::ImportFrom(import) => out.push((
                import.level,
                import.module.as_ref().map(|m| m.as_str().to_string()),
            )),
            Stmt::If(s) => {
                collect_from_imports(&s.body, out);
                for clause in &s.elif_else_clauses {
                    collect_from_imports(&clause.body, out);
                }
            }
            Stmt::Try(s) => {
                collect_from_imports(&s.body, out);
                collect_from_imports(&s.orelse, out);
                collect_from_imports(&s.finalbody, out);
                for handler in &s.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    collect_from_imports(&handler.body, out);
                }
            }
            Stmt::With(s) => collect_from_imports(&s.body, out),
            Stmt::For(s) => {
                collect_from_imports(&s.body, out);
                collect_from_imports(&s.orelse, out);
            }
            Stmt::While(s) => {
                collect_from_imports(&s.body, out);
                collect_from_imports(&s.orelse, out);
            }
            _ => {}
        }
    }
}

/// Whether a function body may return an object with doctests, or a
/// wrapper of one, directly or through its local variables (outside nested
/// functions, classes and lambdas).
fn returns_alias(
    body: &[Stmt],
    bearing: &HashSet<&str>,
    calling: &dyn Fn(&Expr) -> Calling,
) -> bool {
    /// Return values, and assignments as (target names, value, whether
    /// its items are bound).
    struct Flow<'a> {
        returns: Vec<&'a Expr>,
        assigns: Vec<(Vec<&'a str>, &'a Expr, bool)>,
    }
    impl<'a> ast::visitor::Visitor<'a> for Flow<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::Return(ret) => {
                    if let Some(value) = &ret.value {
                        self.returns.push(value);
                    }
                }
                Stmt::Assign(assign) => {
                    let mut names = Vec::new();
                    let mut others = Vec::new();
                    for target in &assign.targets {
                        target_names(target, &mut names, &mut others);
                    }
                    let unpacked = assign
                        .targets
                        .iter()
                        .any(|t| matches!(t, Expr::Tuple(_) | Expr::List(_) | Expr::Starred(_)));
                    self.assigns.push((names, &assign.value, unpacked));
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        let mut names = Vec::new();
                        target_names(&assign.target, &mut names, &mut Vec::new());
                        self.assigns.push((names, value, false));
                    }
                }
                Stmt::For(stmt_for) => {
                    let mut names = Vec::new();
                    target_names(&stmt_for.target, &mut names, &mut Vec::new());
                    self.assigns.push((names, &stmt_for.iter, true));
                    ast::visitor::walk_body(self, &stmt_for.body);
                    ast::visitor::walk_body(self, &stmt_for.orelse);
                }
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) => {}
                _ => ast::visitor::walk_stmt(self, stmt),
            }
        }
        fn visit_expr(&mut self, _expr: &'a Expr) {}
    }
    let mut flow = Flow {
        returns: Vec::new(),
        assigns: Vec::new(),
    };
    ast::visitor::walk_body(&mut flow, body);
    // Local names that may hold an object with doctests, to a fixpoint.
    let mut tainted: HashSet<&str> = bearing.clone();
    loop {
        let before = tainted.len();
        for (names, value, unpacked) in &flow.assigns {
            // A container holding one taints its name too: a loop or a
            // subscript may take it out again.
            if may_alias(value, &tainted, calling, *unpacked) || contains_listed(value, &tainted) {
                tainted.extend(names.iter().copied());
            }
        }
        if tainted.len() == before {
            break;
        }
    }
    flow.returns
        .iter()
        .any(|value| may_alias(value, &tainted, calling, false))
}

/// Whether a container literal lists a name (`[f, g]`), which a loop or a
/// subscript may take out again.
fn contains_listed(expr: &Expr, names: &HashSet<&str>) -> bool {
    let items: &[Expr] = match expr {
        Expr::List(l) => &l.elts,
        Expr::Tuple(t) => &t.elts,
        Expr::Set(s) => &s.elts,
        _ => return false,
    };
    items.iter().any(|item| match item {
        Expr::Name(n) => names.contains(n.id.as_str()),
        other => contains_listed(other, names),
    })
}

/// Whether a statement holds a string with `>>>` anywhere (a nested
/// function's docstring included).
fn contains_doctest_string(stmt: &Stmt) -> bool {
    struct Strings(bool);
    impl<'a> ast::visitor::Visitor<'a> for Strings {
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::StringLiteral(s) = expr
                && s.value.to_str().contains(">>>")
            {
                self.0 = true;
            }
            ast::visitor::walk_expr(self, expr);
        }
    }
    let mut strings = Strings(false);
    ast::visitor::Visitor::visit_stmt(&mut strings, stmt);
    strings.0
}

/// pytest's functions that skip what it is collecting when a conftest calls
/// them while loading: `skip` and `importorskip` (pytest's, or a method of
/// that name).
const SKIPPERS: [&str; 2] = ["skip", "importorskip"];

/// Whether importing `module` may raise pytest's `skip`, as a conftest that
/// makes pytest skip the directory it is loaded for does: a call to one of
/// [`SKIPPERS`], or a `raise` of `Skipped` or `pytest.skip.Exception`, in
/// what runs at import (the module's statements in any branch, class
/// bodies, decorators, default values) or in a module function called by
/// name from there. `unittest.SkipTest` is no skip there: pytest fails the
/// directory.
pub(crate) fn may_skip_while_imported(module: &ast::ModModule) -> bool {
    struct AtImport<'a> {
        skips: bool,
        callees: Vec<&'a str>,
    }
    impl<'a> ast::visitor::Visitor<'a> for AtImport<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                // Only what makes the function runs now.
                Stmt::FunctionDef(def) => {
                    for decorator in &def.decorator_list {
                        self.visit_expr(&decorator.expression);
                    }
                    for default in defaults(&def.parameters) {
                        self.visit_expr(default);
                    }
                }
                Stmt::Raise(raise) => {
                    if raise
                        .exc
                        .as_deref()
                        .is_some_and(|exc| names_skip(exc) && !names_skip_test(exc))
                    {
                        self.skips = true;
                    }
                    ast::visitor::walk_stmt(self, stmt);
                }
                _ => ast::visitor::walk_stmt(self, stmt),
            }
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr {
                match call.func.as_ref() {
                    Expr::Name(name) => {
                        self.skips |= SKIPPERS.contains(&name.id.as_str());
                        self.callees.push(name.id.as_str());
                    }
                    Expr::Attribute(attr) => {
                        self.skips |= SKIPPERS.contains(&attr.attr.as_str());
                    }
                    _ => {}
                }
            }
            ast::visitor::walk_expr(self, expr);
        }
    }

    let mut functions: HashMap<&str, &[Stmt]> = HashMap::new();
    let mut ns = Namespace::default();
    ns.gather(&module.body, Flow::Always, false, &|_| false);
    for binding in &ns.bindings {
        if let Kind::Def(def) = binding.kind {
            functions.insert(binding.name, &def.body);
        }
    }
    let mut scan = AtImport {
        skips: false,
        callees: Vec::new(),
    };
    ast::visitor::Visitor::visit_body(&mut scan, &module.body);
    let mut scanned: HashSet<&str> = HashSet::new();
    while !scan.skips {
        let Some(callee) = scan.callees.pop() else {
            break;
        };
        if scanned.insert(callee)
            && let Some(body) = functions.get(callee)
        {
            ast::visitor::Visitor::visit_body(&mut scan, body);
        }
    }
    scan.skips
}

/// Whether a raised expression is unittest's `SkipTest`.
fn names_skip_test(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => names_skip_test(&call.func),
        Expr::Name(name) => name.id.as_str() == "SkipTest",
        Expr::Attribute(attr) => attr.attr.as_str() == "SkipTest",
        _ => false,
    }
}
