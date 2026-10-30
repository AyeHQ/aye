//! What a module's code puts on `sys.path`, as far as its source tells.

use std::path::{Path, PathBuf};

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{Expr, Stmt};

use crate::eval::Value;
use crate::namespace::{Binding, Bound};
use crate::resolve::{Lookup, Resolved};

/// The directories a module's code puts on `sys.path` (`sys.path.insert`,
/// `append`, `extend`, `+=`, slice assignment, `site.addsitedir`), as far
/// as aye can tell them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysPath {
    /// The directories it can tell: built from `__file__` and literals
    /// through `os.path` and `pathlib` (`abspath(join(dirname(__file__),
    /// "src"))`, `Path(__file__).parent / "support"`), absolute.
    pub added: Vec<PathBuf>,
    /// It adds others: a relative or computed path, or a whole new list.
    pub unknown: bool,
}

/// The visitor behind [`SysPath`]: module-level code and hooks (functions
/// named `pytest_*`), which run before or while pytest collects.
pub struct SysPathVisitor<'s, 'a> {
    pub lookup: Lookup<'s, 'a>,
    pub file: PathBuf,
    pub out: SysPath,
}

impl<'a> SysPathVisitor<'_, 'a> {
    fn is_sys_path(&self, expr: &'a Expr) -> bool {
        self.lookup.path(expr).is_some_and(|p| p == "sys.path")
    }

    fn add(&mut self, expr: &'a Expr) {
        match self.directory(expr, 0) {
            Some(dir) => {
                if !self.out.added.contains(&dir) {
                    self.out.added.push(dir);
                }
            }
            None => self.out.unknown = true,
        }
    }

    fn add_all(&mut self, expr: &'a Expr) {
        match expr {
            Expr::List(list) => list.elts.iter().for_each(|e| self.add(e)),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| self.add(e)),
            _ => self.out.unknown = true,
        }
    }

    /// The absolute directory `expr` evaluates to, when aye can tell.
    fn directory(&self, expr: &'a Expr, depth: u8) -> Option<PathBuf> {
        if depth > 8 {
            return None;
        }
        let normal = |path: PathBuf| Some(aye_python_stdlib::os_path::normalize_absolute(&path));
        match expr {
            Expr::Name(name) if name.id.as_str() == "__file__" => Some(self.file.clone()),
            Expr::StringLiteral(literal) => {
                let path = PathBuf::from(literal.value.to_str());
                // Relative to the working directory, which aye can't tell.
                path.is_absolute().then_some(path)
            }
            Expr::Name(_) => match self.lookup.resolve(expr) {
                Resolved::Local(Bound {
                    binding: Binding::Value(value),
                    conditional: false,
                    ..
                }) => self.directory(value, depth + 1),
                _ => None,
            },
            Expr::Attribute(attr) if attr.attr.id.as_str() == "parent" => self
                .directory(&attr.value, depth + 1)?
                .parent()
                .map(Path::to_path_buf),
            Expr::Subscript(sub) => {
                // `Path(__file__).parents[1]`.
                let Expr::Attribute(attr) = &*sub.value else {
                    return None;
                };
                if attr.attr.id.as_str() != "parents" {
                    return None;
                }
                let Ok(Value::Int(index)) = self.lookup.value(&sub.slice) else {
                    return None;
                };
                let index: usize = index.parse().ok()?;
                let base = self.directory(&attr.value, depth + 1)?;
                base.ancestors().nth(index + 1).map(Path::to_path_buf)
            }
            Expr::BinOp(op) if matches!(op.op, ruff_python_ast::Operator::Div) => {
                let base = self.directory(&op.left, depth + 1)?;
                let part = self.part(&op.right, depth)?;
                normal(base.join(part))
            }
            Expr::Call(call) => {
                let args = &call.arguments.args;
                if let Expr::Attribute(method) = &*call.func
                    && self.lookup.path(&call.func).is_none()
                {
                    let base = || self.directory(&method.value, depth + 1);
                    return match method.attr.id.as_str() {
                        "resolve" | "absolute" | "expanduser" if args.is_empty() => normal(base()?),
                        "joinpath" => {
                            let mut path = base()?;
                            for arg in args {
                                path.push(self.part(arg, depth)?);
                            }
                            normal(path)
                        }
                        _ => None,
                    };
                }
                let path = self.lookup.path(&call.func)?;
                match (path.as_str(), &args[..]) {
                    ("os.path.dirname", [arg]) => self
                        .directory(arg, depth + 1)?
                        .parent()
                        .map(Path::to_path_buf),
                    (
                        "os.path.abspath" | "os.path.realpath" | "os.path.normpath" | "os.fspath"
                        | "builtins.str" | "pathlib.Path" | "pathlib.PurePath",
                        [arg],
                    ) => normal(self.directory(arg, depth + 1)?),
                    ("os.path.join", [first, rest @ ..]) => {
                        let mut path = self.directory(first, depth + 1)?;
                        for arg in rest {
                            path.push(self.part(arg, depth)?);
                        }
                        normal(path)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// A part joined onto a directory: a literal, or a directory.
    fn part(&self, expr: &'a Expr, depth: u8) -> Option<PathBuf> {
        match self.lookup.string(expr) {
            Some(text) => Some(PathBuf::from(text)),
            None => self.directory(expr, depth + 1),
        }
    }
}

impl<'a> Visitor<'a> for SysPathVisitor<'_, 'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            // Only hooks run while pytest collects; fixtures and helpers
            // run later, if at all.
            Stmt::FunctionDef(def) if !def.name.id.as_str().starts_with("pytest_") => return,
            Stmt::ClassDef(_) => return,
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    match target {
                        // `sys.path[:0] = [...]`.
                        Expr::Subscript(sub) if self.is_sys_path(&sub.value) => {
                            self.add_all(&assign.value);
                        }
                        // A new list altogether.
                        target if self.is_sys_path(target) => self.out.unknown = true,
                        _ => {}
                    }
                }
            }
            Stmt::AugAssign(assign) if self.is_sys_path(&assign.target) => {
                self.add_all(&assign.value);
            }
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr {
            let args = &call.arguments.args;
            if let Expr::Attribute(method) = &*call.func
                && self.is_sys_path(&method.value)
            {
                match (method.attr.id.as_str(), &args[..]) {
                    ("insert", [_, path]) | ("append", [path]) => self.add(path),
                    ("extend", [paths]) => self.add_all(paths),
                    ("insert" | "append" | "extend" | "__setitem__", _) => {
                        self.out.unknown = true;
                    }
                    _ => {}
                }
            } else if self
                .lookup
                .path(&call.func)
                .is_some_and(|p| p == "site.addsitedir")
            {
                match args.first() {
                    Some(path) => self.add(path),
                    None => self.out.unknown = true,
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}
