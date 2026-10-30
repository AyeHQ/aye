//! What a body does while it runs: whether a function is a generator,
//! which bindings exclude each other, and what may skip or raise while
//! a module is imported.

use ruff_python_ast::{self as ast, Expr, Number, Operator, Stmt, UnaryOp};

use crate::eval::NO_ID;
use crate::namespace::Bound;
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

/// A `pytest.skip` or `pytest.importorskip` call that runs while the module
/// is imported, in its body or a class body: pytest then skips the whole
/// module, depending on the environment.
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
                && matches!(path.as_str(), "pytest.skip" | "pytest.importorskip")
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
/// module for `unittest.SkipTest`), or an `assert` of a false literal.
/// pytest reports the error as the module's collection error.
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
