//! A namespace as the finder reads it: what a module's or a class's body
//! binds, where and how surely ([`Namespace`], each name a [`Binding`]),
//! and the qualified names the module's imports bind names to
//! ([`Imports`]).

use std::collections::{HashMap, HashSet};

use aye_adapter::ast::{self, Expr, Stmt};
use aye_adapter::{Ranged, TextSize};
use aye_python_semantic::flow::is_main_guard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Flow {
    /// Runs whenever the module imports.
    Always,
    /// May not run.
    Maybe,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Kind<'a> {
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
pub(super) struct Binding<'a> {
    pub(super) name: &'a str,
    pub(super) flow: Flow,
    pub(super) kind: Kind<'a>,
    pub(super) at: TextSize,
}

/// A namespace's statements, as the finder needs them.
#[derive(Default)]
pub(super) struct Namespace<'a> {
    pub(super) bindings: Vec<Binding<'a>>,
    /// Expressions whose value is bound to a name (assignment values, loop
    /// iterables, context managers), and whether its items are (unpacking,
    /// loops).
    pub(super) values: Vec<(&'a Expr, bool)>,
    /// Decorators, whose arguments a wrapper may copy a docstring from,
    /// but those of property accessors.
    pub(super) decorators: Vec<&'a Expr>,
    /// Every expression evaluated in the namespace, values included.
    pub(super) evaluated: Vec<&'a Expr>,
    /// Assignment targets that aren't plain names.
    pub(super) targets: Vec<&'a Expr>,
    /// Positions of `from … import *`.
    pub(super) stars: Vec<TextSize>,
    /// A `raise` that always runs.
    pub(super) raises: bool,
    /// A `raise` of a skip exception (`unittest.SkipTest`,
    /// `pytest.skip.Exception`), which may run.
    pub(super) skips: bool,
}

/// The module's imports: each name, and the qualified names imports bind it
/// to (empty when something else binds it too).
#[derive(Default)]
pub(super) struct Imports {
    pub(super) origins: HashMap<String, Vec<String>>,
    pub(super) other: HashSet<String>,
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

    pub(super) fn is_one_of(&self, expr: &Expr, allowed: &[&str]) -> bool {
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
pub(super) fn root_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str()),
        Expr::Attribute(attr) => root_name(&attr.value),
        Expr::Subscript(sub) => root_name(&sub.value),
        Expr::Call(call) => root_name(&call.func),
        _ => None,
    }
}

/// The docstring statement of a body.
pub(super) fn docstring_of(body: &[Stmt]) -> Option<&ast::ExprStringLiteral> {
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
pub(super) fn target_names<'a>(
    target: &'a Expr,
    names: &mut Vec<&'a str>,
    others: &mut Vec<&'a Expr>,
) {
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
pub(super) fn defaults(parameters: &ast::Parameters) -> impl Iterator<Item = &Expr> {
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
    pub(super) fn gather(
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
pub(super) fn names_skip(expr: &Expr) -> bool {
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

/// Imports a statement (or the statements nested in it, outside functions
/// and classes) make, by name.
pub(super) fn record_imports<'a>(
    stmt: &'a Stmt,
    imports: &mut Imports,
    imported: &mut HashSet<&'a str>,
) {
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
pub(super) fn collect_from_imports(body: &[Stmt], out: &mut Vec<(u32, Option<String>)>) {
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
