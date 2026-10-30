//! Aliases and wrappers: whether a value may be an object with doctests,
//! or wrap one, which the finder would then visit under another name or
//! find as another object.

use std::collections::{HashMap, HashSet};

use aye_adapter::ast::{self, Expr, Stmt};

use super::Finder;
use super::checks::Scan;
use super::namespace::{Kind, root_name, target_names};
use super::tables::INERT_BUILTINS;

/// Whether a decorator is given an object with doctests, or belongs to
/// one, so that what it returns may copy its docstring (`functools.wraps(f)`,
/// `@x.setter` on a function not named `x`).
pub(super) fn decorator_aliases(expr: &Expr, bearing: &HashSet<&str>) -> bool {
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

/// What calling something gives, as far as aliasing goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Calling {
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
pub(super) fn may_alias(
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

impl Finder<'_, '_> {
    /// What calling `func` gives. A module function's result is what it
    /// returns (`check_calls` reads its body), so a wrapper at most of what
    /// it is given; a plain module class's is a plain instance; a class
    /// with doctests may make descriptors with its docstring.
    pub(super) fn calling(
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
}

/// Whether a function body may return an object with doctests, or a
/// wrapper of one, directly or through its local variables (outside nested
/// functions, classes and lambdas).
pub(super) fn returns_alias(
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
