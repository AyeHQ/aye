//! What a body does while it runs: whether a function is a generator,
//! which bindings exclude each other, what may skip or raise while a
//! module is imported, which of a module's or class body's statements
//! may not run, and the `__main__` guard.

use ruff_python_ast::{self as ast, CmpOp, Expr, Number, Operator, Stmt, UnaryOp};
use ruff_text_size::{Ranged, TextSize};

use crate::eval::NO_ID;
use crate::namespace::{Bound, Namespace};
use crate::resolve::Lookup;

/// Whether a function body makes it a generator (a `yield` outside nested
/// functions, lambdas and classes).
pub fn is_generator(body: &[Stmt]) -> bool {
    use ast::visitor::{Visitor, walk_expr, walk_stmt};

    struct Finder(bool);
    impl<'a> Visitor<'a> for Finder {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if !matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Yield(_) | Expr::YieldFrom(_) => self.0 = true,
                Expr::Lambda(_) => {}
                _ => walk_expr(self, expr),
            }
        }
    }

    let mut finder = Finder(false);
    for stmt in body {
        finder.visit_stmt(stmt);
        if finder.0 {
            return true;
        }
    }
    false
}

/// Whether two bindings sit in different branches of one `if`, `try` or
/// `match`, so at most one of them runs.
pub fn exclusive(a: &Bound<'_>, b: &Bound<'_>) -> bool {
    a.branches
        .iter()
        .zip(&b.branches)
        .take_while(|(x, y)| x.0 == y.0)
        .any(|(x, y)| x.1 != y.1)
}

/// `__name__ == "__main__"`, either way round: false when imported.
pub fn is_main_guard(test: &Expr) -> bool {
    let Expr::Compare(cmp) = test else {
        return false;
    };
    if cmp.ops.as_ref() != [CmpOp::Eq] || cmp.comparators.len() != 1 {
        return false;
    }
    let is_name = |e: &Expr| matches!(e, Expr::Name(n) if n.id.as_str() == "__name__");
    let is_main = |e: &Expr| matches!(e, Expr::StringLiteral(s) if s.value.to_str() == "__main__");
    let right = &cmp.comparators[0];
    (is_name(&cmp.left) && is_main(right)) || (is_main(&cmp.left) && is_name(right))
}

/// A call to one of the framework's
/// [`skips`](crate::framework::Framework::skips)
/// (`pytest.skip`, `pytest.importorskip`) that runs while the module is
/// imported, in its body or a class body: the framework then skips the
/// whole module, depending on the environment.
pub fn import_time_skip<'a>(body: &'a [Stmt], lookup: Lookup<'_, 'a>) -> Option<String> {
    use ast::visitor::{Visitor, walk_expr, walk_stmt};

    struct Finder<'s, 'a> {
        lookup: Lookup<'s, 'a>,
        found: Option<String>,
    }
    impl<'a> Visitor<'a> for Finder<'_, 'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if !matches!(stmt, Stmt::FunctionDef(_)) {
                walk_stmt(self, stmt);
            }
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.found.is_some() || matches!(expr, Expr::Lambda(_)) {
                return;
            }
            if let Expr::Call(call) = expr
                && let Some(path) = self.lookup.path(&call.func)
                && self.lookup.module.framework.skips.contains(&path.as_str())
            {
                self.found = Some(path);
                return;
            }
            walk_expr(self, expr);
        }
    }

    let mut finder = Finder {
        lookup,
        found: None,
    };
    for stmt in body {
        finder.visit_stmt(stmt);
    }
    finder.found
}

/// Code run while the module is imported (outside functions) that may
/// raise, described: a call to a callable aye evaluates, with literal
/// arguments it can't confirm are accepted (`X = range(0, 3, 0)`), a
/// division of literals by zero, a `raise` statement (pytest skips the
/// module for `unittest.SkipTest`), or an `assert` of a false literal: the
/// module fails to import (pytest reports a collection error).
pub fn import_time_raise<'a>(body: &'a [Stmt], lookup: Lookup<'_, 'a>) -> Option<String> {
    use ast::visitor::{Visitor, walk_expr, walk_stmt};

    /// Arguments aye reads without names: literals, and displays of them.
    fn literal(expr: &Expr) -> bool {
        match expr {
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_) => true,
            Expr::UnaryOp(op) => literal(&op.operand),
            Expr::List(list) => list.elts.iter().all(literal),
            Expr::Tuple(tuple) => tuple.elts.iter().all(literal),
            Expr::Set(set) => set.elts.iter().all(literal),
            Expr::Dict(dict) => dict
                .items
                .iter()
                .all(|item| item.key.as_ref().is_some_and(literal) && literal(&item.value)),
            _ => false,
        }
    }

    struct Finder<'s, 'a> {
        lookup: Lookup<'s, 'a>,
        found: Option<String>,
    }
    /// A number literal, maybe negated, and whether it is zero.
    fn number(expr: &Expr) -> Option<bool> {
        match expr {
            Expr::NumberLiteral(n) => Some(match &n.value {
                Number::Int(i) => i.as_u64() == Some(0),
                Number::Float(f) => *f == 0.0,
                Number::Complex { real, imag } => *real == 0.0 && *imag == 0.0,
            }),
            Expr::BooleanLiteral(b) => Some(!b.value),
            Expr::UnaryOp(op) if matches!(op.op, UnaryOp::USub | UnaryOp::UAdd) => {
                number(&op.operand)
            }
            _ => None,
        }
    }

    /// A literal Python takes as false.
    fn falsy(expr: &Expr) -> bool {
        match expr {
            Expr::BooleanLiteral(b) => !b.value,
            Expr::NoneLiteral(_) => true,
            Expr::StringLiteral(s) => s.value.is_empty(),
            Expr::BytesLiteral(b) => b.value.is_empty(),
            Expr::List(list) => list.elts.is_empty(),
            Expr::Tuple(tuple) => tuple.elts.is_empty(),
            Expr::Dict(dict) => dict.items.is_empty(),
            other => number(other) == Some(true),
        }
    }

    impl<'a> Visitor<'a> for Finder<'_, 'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if self.found.is_some() {
                return;
            }
            match stmt {
                Stmt::FunctionDef(_) => return,
                Stmt::Raise(_) => {
                    self.found = Some("a raise statement".into());
                    return;
                }
                Stmt::Assert(assert) if falsy(&assert.test) => {
                    self.found = Some("an assert of a false literal".into());
                    return;
                }
                _ => {}
            }
            walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if self.found.is_some()
                || matches!(
                    expr,
                    Expr::Lambda(_)
                        | Expr::Generator(_)
                        | Expr::ListComp(_)
                        | Expr::SetComp(_)
                        | Expr::DictComp(_)
                )
            {
                return;
            }
            if let Expr::BinOp(op) = expr
                && matches!(op.op, Operator::Div | Operator::FloorDiv | Operator::Mod)
                && number(&op.left).is_some()
                && number(&op.right) == Some(true)
            {
                self.found = Some("a division by zero".into());
                return;
            }
            if let Expr::Call(call) = expr
                && let Some(path) = self.lookup.trusted_path(&call.func)
                // Third-party callables have arguments aye doesn't check.
                && (matches!(path.as_str(), "builtins.float" | "re.compile")
                    || (NO_ID.contains(&path.as_str())
                        && !path.starts_with("numpy.")
                        && !path.starts_with("pandas.")))
                && call.arguments.args.iter().all(literal)
                && call.arguments.keywords.iter().all(|k| literal(&k.value))
                && self.lookup.value(expr).is_err()
            {
                self.found = Some(format!("a {path}(…) call"));
                return;
            }
            walk_expr(self, expr);
        }
    }

    let mut finder = Finder {
        lookup,
        found: None,
    };
    for stmt in body {
        finder.visit_stmt(stmt);
    }
    finder.found
}

/// What a module or class body may skip at runtime.
pub struct BodyFacts<'a> {
    /// [`conditional_deletes`].
    pub deletes: Vec<(&'a str, TextSize)>,
    /// [`guarded_by_try`].
    pub guarded: Vec<TextSize>,
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
pub fn conditional_deletes(body: &[Stmt]) -> Vec<(&str, TextSize)> {
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
pub fn guarded_by_try(body: &[Stmt]) -> Vec<TextSize> {
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
pub fn maybe_deleted<'a>(ns: &Namespace<'a>, body: &'a [Stmt]) -> Vec<(&'a str, Bound<'a>)> {
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
