//! What runs while a module is imported: the calls in its expressions
//! ([`Scan`]), and what calling one of its functions does
//! ([`function_info`]).

use std::collections::{BTreeMap, HashSet};

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use aye_python_stdlib::builtins::{DYNAMIC_CALLS, READING_BUILTINS};

use super::builder::{dotted, imported_path, literal_like};
use super::{Binding, FunctionInfo, Touch};

/// Expressions run at import: dynamic calls, calls given a class of the
/// module (which may change it), calls of local functions with side
/// effects, and `:=` targets.
pub(super) struct Scan<'n> {
    pub(super) names: &'n BTreeMap<String, Binding>,
    pub(super) reason: Option<String>,
    pub(super) walrus: Vec<String>,
    /// What the calls may change in, or hand to, other modules.
    pub(super) touches: Vec<Touch>,
}

impl<'a> Visitor<'a> for Scan<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => self.call(call),
            Expr::Named(named) => {
                if let Expr::Name(name) = &*named.target {
                    self.walrus.push(name.id.as_str().to_string());
                }
            }
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

impl Scan<'_> {
    fn note(&mut self, reason: String) {
        self.reason.get_or_insert(reason);
    }

    fn call(&mut self, call: &ExprCall) {
        let callee = &*call.func;
        let mut reading = false;
        match callee {
            Expr::Name(name) => {
                let id = name.id.as_str();
                match self.names.get(id) {
                    None if DYNAMIC_CALLS.contains(&id) => {
                        self.note(format!("the module calls {id}() while it is imported"));
                        match id {
                            "setattr" | "delattr" => {
                                if let Some(first) = call.arguments.args.first() {
                                    self.object(first, &format!("{id}()"));
                                }
                            }
                            "exec" | "eval" | "__import__" => {
                                self.touches.push(Touch::Unknown(format!(
                                    "the module calls {id}() while it is imported"
                                )));
                            }
                            _ => {}
                        }
                    }
                    None => reading = READING_BUILTINS.contains(&id),
                    Some(Binding::Function(info)) => {
                        if let Some(effect) = &info.side_effects {
                            let reason =
                                format!("{id}(), called while the module is imported, {effect}");
                            if info.foreign {
                                self.touches.push(Touch::Unknown(reason.clone()));
                            }
                            self.note(reason);
                        }
                    }
                    Some(Binding::Module(path) | Binding::Import(path)) => {
                        reading = path.starts_with("unittest.") || path.starts_with("typing.");
                        if !reading {
                            self.touches.push(Touch::Call(path.clone()));
                        }
                    }
                    _ => {}
                }
            }
            Expr::Attribute(attr) => {
                let head = dotted(callee).and_then(|d| d.split('.').next().map(str::to_string));
                match head.as_deref().and_then(|h| self.names.get(h)) {
                    Some(Binding::Class(_)) => {
                        self.note(format!(
                            "{} is called while the module is imported",
                            dotted(callee).unwrap_or_default()
                        ));
                    }
                    Some(Binding::Module(path) | Binding::Import(path)) => {
                        reading =
                            path == "unittest" || path.starts_with("unittest.") || path == "typing";
                        if !reading {
                            // A method of another module's object (a
                            // classmethod adding tests), or its function.
                            if let Some(path) = imported_path(self.names, callee) {
                                self.touches.push(Touch::Call(path));
                            }
                            self.object(&attr.value, "a method call");
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if reading {
            return;
        }
        let given = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value));
        for arg in given {
            let arg = match arg {
                Expr::Starred(starred) => &*starred.value,
                other => other,
            };
            if let Expr::Name(name) = arg
                && matches!(self.names.get(name.id.as_str()), Some(Binding::Class(_)))
            {
                self.note(format!(
                    "the class {} is passed to a call while the module is imported",
                    name.id
                ));
            }
            // Another module's object given to a call may be changed by it.
            if let Some(path) = imported_path(self.names, arg) {
                self.touches.push(Touch::Object(path));
            }
        }
    }

    /// An object a call may change (`setattr`'s first argument, a method's
    /// receiver), when it is another module's.
    fn object(&mut self, expr: &Expr, how: &str) {
        if let Some(path) = imported_path(self.names, expr) {
            self.touches.push(Touch::Object(path));
        } else if dotted(expr).is_none() && how.ends_with("()") {
            self.touches.push(Touch::Unknown(format!(
                "{how} changes an object aye can't name while the module is imported"
            )));
        }
    }
}

/// Read a module-level function's body: what calling it may do and return.
pub(super) fn function_info(def: &StmtFunctionDef) -> FunctionInfo {
    struct Reader<'a> {
        locals: HashSet<&'a str>,
        params: HashSet<&'a str>,
        may_return_class: bool,
        side_effects: Option<String>,
        changes_argument: bool,
        foreign: bool,
        returns: Vec<Option<&'a Expr>>,
        yields: bool,
    }
    impl<'a> Visitor<'a> for Reader<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                // Nested functions don't run unless called; a nested class
                // may be returned.
                Stmt::ClassDef(_) => {
                    self.may_return_class = true;
                    return;
                }
                Stmt::FunctionDef(_) => return,
                Stmt::Return(ret) => self.returns.push(ret.value.as_deref()),
                Stmt::Global(_) | Stmt::Nonlocal(_) => {
                    self.side_effects
                        .get_or_insert("rebinds names of the module".into());
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.target(target);
                    }
                }
                Stmt::AnnAssign(assign) => self.target(&assign.target),
                Stmt::AugAssign(assign) => self.target(&assign.target),
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        self.target(target);
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Lambda(_) => return,
                Expr::Yield(_) | Expr::YieldFrom(_) => self.yields = true,
                Expr::Call(call) => {
                    if let Expr::Name(name) = &*call.func {
                        let id = name.id.as_str();
                        if DYNAMIC_CALLS.contains(&id) && !self.locals.contains(id) {
                            self.side_effects.get_or_insert(format!("calls {id}()"));
                            if matches!(id, "setattr" | "delattr" | "exec" | "eval" | "__import__")
                            {
                                self.foreign = true;
                            }
                        }
                        if id == "type" && call.arguments.args.len() == 3 {
                            self.may_return_class = true;
                        }
                    }
                    if let Expr::Attribute(attr) = &*call.func
                        && attr.attr.id.as_str() == "new_class"
                    {
                        self.may_return_class = true;
                    }
                }
                Expr::Named(named) => {
                    if let Expr::Name(name) = &*named.target {
                        self.locals.insert(name.id.as_str());
                    }
                }
                _ => {}
            }
            visitor::walk_expr(self, expr);
        }
    }
    impl<'a> Reader<'a> {
        fn target(&mut self, target: &'a Expr) {
            let mut root = target;
            loop {
                match root {
                    Expr::Attribute(attr) => root = &attr.value,
                    Expr::Subscript(sub) => root = &sub.value,
                    _ => break,
                }
            }
            match (target, root) {
                (Expr::Name(name), _) => {
                    self.locals.insert(name.id.as_str());
                }
                (Expr::Tuple(tuple), _) => {
                    for elt in &tuple.elts {
                        self.target(elt);
                    }
                }
                (Expr::List(list), _) => {
                    for elt in &list.elts {
                        self.target(elt);
                    }
                }
                (Expr::Starred(starred), _) => self.target(&starred.value),
                (_, Expr::Name(name)) if self.locals.contains(name.id.as_str()) => {}
                (_, Expr::Name(name)) if self.params.contains(name.id.as_str()) => {
                    self.changes_argument = true;
                }
                _ => {
                    self.side_effects
                        .get_or_insert("changes attributes of objects it doesn't make".into());
                    self.foreign = true;
                }
            }
        }
    }

    let params: Vec<&str> = def
        .parameters
        .posonlyargs
        .iter()
        .chain(&def.parameters.args)
        .map(|p| p.parameter.name.id.as_str())
        .collect();
    let mut reader = Reader {
        locals: HashSet::new(),
        params: params.iter().copied().collect(),
        may_return_class: false,
        side_effects: None,
        changes_argument: false,
        foreign: false,
        returns: Vec::new(),
        yields: false,
    };
    // Assignments to locals made before an attribute assignment count:
    // read the body once to collect them, then again for effects.
    collect_locals(&def.body, &mut reader.locals);
    for stmt in &def.body {
        reader.visit_stmt(stmt);
    }
    let first = params.first().copied();
    // Nested functions wrapping the first argument with `functools.wraps`.
    let wrappers: HashSet<&str> = def
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::FunctionDef(inner)
                if first.is_some()
                    && inner
                        .decorator_list
                        .iter()
                        .any(|d| wraps(&d.expression, first.unwrap_or_default())) =>
            {
                Some(inner.name.id.as_str())
            }
            _ => None,
        })
        .collect();
    let returns_value = |value: Option<&Expr>| value.is_none_or(literal_like);
    let mut may_return_class = reader.may_return_class;
    if !reader.yields && reader.returns.iter().any(|value| !returns_value(*value)) {
        may_return_class = true;
    }
    let keeps = first.is_some()
        && !reader.returns.is_empty()
        && reader.side_effects.is_none()
        && reader.returns.iter().all(|value| {
            matches!(value, Some(Expr::Name(name))
                if Some(name.id.as_str()) == first || wrappers.contains(name.id.as_str()))
        });
    FunctionInfo {
        may_return_class,
        side_effects: reader.side_effects,
        changes_argument: reader.changes_argument,
        keeps,
        foreign: reader.foreign,
    }
}

/// `functools.wraps(name)` or `wraps(name)`.
fn wraps(expr: &Expr, name: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let named = match &*call.func {
        Expr::Name(n) => n.id.as_str() == "wraps",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "wraps",
        _ => false,
    };
    named && matches!(call.arguments.args.first(), Some(Expr::Name(arg)) if arg.id.as_str() == name)
}

fn collect_locals<'a>(body: &'a [Stmt], out: &mut HashSet<&'a str>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(name) = target {
                        out.insert(name.id.as_str());
                    }
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Expr::Name(name) = &*assign.target {
                    out.insert(name.id.as_str());
                }
            }
            Stmt::For(s) => {
                if let Expr::Name(name) = &*s.target {
                    out.insert(name.id.as_str());
                }
                collect_locals(&s.body, out);
            }
            Stmt::With(s) => {
                for item in &s.items {
                    if let Some(Expr::Name(name)) = item.optional_vars.as_deref() {
                        out.insert(name.id.as_str());
                    }
                }
                collect_locals(&s.body, out);
            }
            Stmt::If(s) => {
                collect_locals(&s.body, out);
                for clause in &s.elif_else_clauses {
                    collect_locals(&clause.body, out);
                }
            }
            Stmt::Try(s) => {
                collect_locals(&s.body, out);
                collect_locals(&s.orelse, out);
                collect_locals(&s.finalbody, out);
            }
            Stmt::While(s) => collect_locals(&s.body, out),
            _ => {}
        }
    }
}
