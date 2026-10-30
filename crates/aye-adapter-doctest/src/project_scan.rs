//! What the project's Python files say about `__module__`, read once: an
//! object a project module defines may claim to belong to another module.

use aye_adapter::Recognizer;

use crate::DoctestRecognizer;

/// The project's Python files, read once: whether any assigns `__module__`,
/// whether any assigns it a value aye can't read (computed rather than a
/// quoted name or a function's parameter), and every dotted name any
/// spells in quotes.
#[derive(Debug, Default)]
pub(crate) struct ProjectScan {
    assigns_module: bool,
    computes_module: bool,
    quoted: std::collections::HashSet<String>,
}

/// The dotted names `source` spells in quotes (`"pkg.mod"`, `'pkg'`).
fn quoted_names(source: &str, out: &mut std::collections::HashSet<String>) {
    let bytes = source.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let quote = bytes[at];
        at += 1;
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        let start = at;
        while at < bytes.len()
            && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'_' | b'.'))
        {
            at += 1;
        }
        if at < bytes.len() && bytes[at] == quote && at > start {
            out.insert(source[start..at].to_string());
        }
    }
}

/// Whether a source that [`assigns_module`] assigns it a value aye can't
/// read: neither a string nor a parameter of the function doing it (whose
/// callers spell the name in quotes, as `numpy`'s `set_module("numpy")`).
/// `f.__module__ = __package__`, `__module__ = __name__.rpartition(".")[0]`
/// in a class body, and `setattr` or `type()` given anything but a string,
/// are computed; so is a source that doesn't parse.
pub(crate) fn computes_module(source: &str) -> bool {
    use aye_adapter::ast::visitor::{self, Visitor};
    use aye_adapter::ast::{Expr, Stmt};

    struct Assignments<'a> {
        params: Vec<Vec<&'a str>>,
        computed: bool,
    }
    impl<'a> Assignments<'a> {
        fn value(&mut self, value: &'a Expr) {
            let known = match value {
                Expr::StringLiteral(_) => true,
                // A parameter of the function, or of one enclosing it (a
                // decorator factory's argument).
                Expr::Name(name) => self
                    .params
                    .iter()
                    .any(|params| params.contains(&name.id.as_str())),
                _ => false,
            };
            self.computed |= !known;
        }
        fn target(&mut self, target: &'a Expr, value: &'a Expr) {
            let module = match target {
                Expr::Attribute(attr) => attr.attr.as_str() == "__module__",
                Expr::Name(name) => name.id.as_str() == "__module__",
                _ => false,
            };
            if module {
                self.value(value);
            }
        }
    }
    impl<'a> Visitor<'a> for Assignments<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::FunctionDef(def) => {
                    let p = &def.parameters;
                    let names = p
                        .posonlyargs
                        .iter()
                        .chain(&p.args)
                        .chain(&p.kwonlyargs)
                        .map(|a| a.parameter.name.as_str())
                        .chain(p.vararg.iter().map(|a| a.name.as_str()))
                        .chain(p.kwarg.iter().map(|a| a.name.as_str()))
                        .collect();
                    self.params.push(names);
                    visitor::walk_stmt(self, stmt);
                    self.params.pop();
                    return;
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.target(target, &assign.value);
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        self.target(&assign.target, value);
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Call(call) => {
                    let args = &call.arguments.args;
                    let names_module = |e: &Expr| matches!(e, Expr::StringLiteral(s) if s.value.to_str() == "__module__");
                    if args.len() == 3 && names_module(&args[1]) {
                        self.value(&args[2]);
                    }
                }
                Expr::Dict(dict) => {
                    for item in &dict.items {
                        if let Some(Expr::StringLiteral(key)) = &item.key
                            && key.value.to_str() == "__module__"
                        {
                            self.value(&item.value);
                        }
                    }
                }
                _ => {}
            }
            visitor::walk_expr(self, expr);
        }
    }

    let Ok(parsed) = ruff_python_parser::parse_module(source) else {
        return true;
    };
    let mut found = Assignments {
        params: Vec::new(),
        computed: false,
    };
    found.visit_body(&parsed.syntax().body);
    found.computed
}

/// Whether a source assigns `__module__` (`X.__module__ = …`, `__module__
/// = …` in a class body) or names it in a string (`setattr`, `type()`).
pub(crate) fn assigns_module(source: &str) -> bool {
    let mut rest = source;
    while let Some(at) = rest.find("__module__") {
        if matches!(rest[..at].chars().next_back(), Some('"' | '\'')) {
            return true;
        }
        let after = &rest[at + "__module__".len()..];
        let trimmed = after.trim_start_matches([' ', '\t']);
        if trimmed.starts_with('=') && !trimmed.starts_with("==") {
            return true;
        }
        rest = after;
    }
    false
}

impl DoctestRecognizer {
    /// The project's files, read once: does any assign `__module__`, and
    /// which dotted names do they spell in quotes.
    fn project_scan(&self) -> &ProjectScan {
        self.scan.get_or_init(|| {
            let mut scan = ProjectScan::default();
            let mut stack = vec![self.root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                    } else if path.extension().is_some_and(|e| e == "py")
                        && let Ok(source) = std::fs::read_to_string(&path)
                    {
                        if assigns_module(&source) {
                            scan.assigns_module = true;
                            scan.computes_module |= computes_module(&source);
                        }
                        quoted_names(&source, &mut scan.quoted);
                    }
                }
            }
            scan
        })
    }

    /// Whether an object a project module defines may claim, through its
    /// `__module__`, to belong to the module `name`: some project file
    /// assigns `__module__`, and spells `name` in quotes or computes the
    /// value.
    pub(crate) fn may_be_claimed(&self, name: &str) -> bool {
        let scan = self.project_scan();
        scan.assigns_module && (scan.computes_module || scan.quoted.contains(name))
    }
}
