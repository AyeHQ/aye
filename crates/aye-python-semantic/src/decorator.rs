//! Decorators, as far as their source shows what they do with what they
//! decorate.

use ruff_python_ast::{self as ast, Expr};

use crate::flow::is_generator;

/// Whether a one-argument function returns its argument, or a nested
/// function decorated only with `functools.wraps(argument)`, on every path,
/// and uses the argument for nothing but calling it: it then keeps the
/// decorated object's name, marks, parameters and tests.
pub fn returns_its_argument(def: &ast::StmtFunctionDef) -> bool {
    use ast::Stmt;

    let params = &def.parameters;
    let positional: Vec<_> = params.posonlyargs.iter().chain(&params.args).collect();
    if def.is_async
        || is_generator(&def.body)
        || positional.len() != 1
        || params.vararg.is_some()
        || params.kwarg.is_some()
        || !params.kwonlyargs.is_empty()
    {
        return false;
    }
    let arg = positional[0].parameter.name.id.as_str();
    // Nested functions that wrap the argument.
    let mut wrappers: Vec<&str> = Vec::new();
    for stmt in &def.body {
        if let Stmt::FunctionDef(inner) = stmt
            && !inner.decorator_list.is_empty()
            && inner
                .decorator_list
                .iter()
                .all(|d| is_wraps_of(&d.expression, arg))
        {
            wrappers.push(inner.name.id.as_str());
        }
    }
    if !matches!(def.body.last(), Some(Stmt::Return(_))) {
        return false;
    }
    let mut check = ArgUses {
        arg,
        wrappers: &wrappers,
        ok: true,
        depth: 0,
    };
    for stmt in &def.body {
        ast::visitor::Visitor::visit_stmt(&mut check, stmt);
    }
    check.ok
}

/// `functools.wraps(arg)` or `wraps(arg)`.
fn is_wraps_of(expr: &Expr, arg: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let named = match &*call.func {
        Expr::Name(name) => name.id.as_str() == "wraps",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "wraps",
        _ => false,
    };
    named
        && call.arguments.keywords.is_empty()
        && matches!(&call.arguments.args[..], [Expr::Name(n)] if n.id.as_str() == arg)
}

/// Checks a decorator's body: its argument is only called, passed to
/// `wraps`, or returned; every `return` of the decorator itself hands back
/// the argument or a wrapper.
struct ArgUses<'w> {
    arg: &'w str,
    wrappers: &'w [&'w str],
    ok: bool,
    /// Nesting in functions and lambdas, whose returns are their own.
    depth: usize,
}

impl<'a> ast::visitor::Visitor<'a> for ArgUses<'_> {
    fn visit_stmt(&mut self, stmt: &'a ast::Stmt) {
        use ast::Stmt;
        use ast::visitor::walk_stmt;
        match stmt {
            Stmt::Return(ret) if self.depth == 0 => {
                let handed = match ret.value.as_deref() {
                    Some(Expr::Name(name)) => {
                        let name = name.id.as_str();
                        name == self.arg || self.wrappers.contains(&name)
                    }
                    _ => false,
                };
                if !handed {
                    self.ok = false;
                }
            }
            Stmt::FunctionDef(inner) => {
                // `wraps(arg)` decorators are fine; anything else is
                // checked like the rest of the body.
                for d in &inner.decorator_list {
                    if !is_wraps_of(&d.expression, self.arg) {
                        self.visit_expr(&d.expression);
                    }
                }
                self.depth += 1;
                for s in &inner.body {
                    self.visit_stmt(s);
                }
                self.depth -= 1;
            }
            Stmt::ClassDef(_) | Stmt::Global(_) | Stmt::Nonlocal(_) => self.ok = false,
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        use ast::visitor::walk_expr;
        match expr {
            Expr::Name(name) if name.id.as_str() == self.arg => {
                // Any use not handled below (passed along, stored,
                // rebound) may change the object.
                self.ok = false;
            }
            Expr::Call(call) if matches!(&*call.func, Expr::Name(n) if n.id.as_str() == self.arg) =>
            {
                for arg in &call.arguments.args {
                    self.visit_expr(arg);
                }
                for keyword in &call.arguments.keywords {
                    self.visit_expr(&keyword.value);
                }
            }
            Expr::Lambda(_) => {
                self.depth += 1;
                walk_expr(self, expr);
                self.depth -= 1;
            }
            _ => walk_expr(self, expr),
        }
    }
}
