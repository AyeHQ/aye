//! The walk of a module's statements, in order, as Python runs them: what
//! each binds, what it imports, and what may make the namespace unreadable.

use std::collections::BTreeMap;

use aye_adapter::TextRange;
use aye_adapter::ast::visitor::Visitor;
use aye_adapter::ast::{self, Expr, ExprCall, Stmt, StmtFunctionDef};
use aye_python_semantic::flow::is_main_guard;

use super::scan::{Scan, function_info};
use super::{Binding, Builder, Touch, is_dunder, is_stdlib};

/// `expr` as a dotted path through an imported name (a module's, or one
/// imported from a module): `base.Base` for `base.Base` after `import base`.
pub(super) fn imported_path(names: &BTreeMap<String, Binding>, expr: &Expr) -> Option<String> {
    let dotted = dotted(expr)?;
    let (head, rest) = dotted.split_once('.').unwrap_or((dotted.as_str(), ""));
    match names.get(head) {
        Some(Binding::Module(path) | Binding::Import(path)) => Some(if rest.is_empty() {
            path.clone()
        } else {
            format!("{path}.{rest}")
        }),
        _ => None,
    }
}

/// `name` as a dotted path through `names`, or `None`.
pub(super) fn dotted(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        Expr::Attribute(attr) => Some(format!("{}.{}", dotted(&attr.value)?, attr.attr.id)),
        _ => None,
    }
}

/// Literals, and operations and containers of them: values that are no
/// class and not callable.
pub(super) fn literal_like(expr: &Expr) -> bool {
    match expr {
        Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::FString(_)
        | Expr::TString(_) => true,
        Expr::List(list) => list.elts.iter().all(literal_like),
        Expr::Tuple(tuple) => tuple.elts.iter().all(literal_like),
        Expr::Set(set) => set.elts.iter().all(literal_like),
        Expr::Dict(dict) => dict
            .items
            .iter()
            .all(|item| item.key.as_ref().is_none_or(literal_like) && literal_like(&item.value)),
        Expr::BinOp(op) => literal_like(&op.left) && literal_like(&op.right),
        Expr::UnaryOp(op) => literal_like(&op.operand),
        Expr::BoolOp(op) => op.values.iter().all(literal_like),
        Expr::Compare(cmp) => literal_like(&cmp.left) && cmp.comparators.iter().all(literal_like),
        _ => false,
    }
}

/// Values that are no class and not callable whatever they hold:
/// containers, comprehensions, strings.
pub(super) fn not_callable(expr: &Expr) -> bool {
    literal_like(expr)
        || matches!(
            expr,
            Expr::List(_)
                | Expr::Tuple(_)
                | Expr::Set(_)
                | Expr::Dict(_)
                | Expr::ListComp(_)
                | Expr::SetComp(_)
                | Expr::DictComp(_)
                | Expr::Generator(_)
        )
}

/// `TYPE_CHECKING` or `typing.TYPE_CHECKING`: false at runtime.
fn is_type_checking(test: &Expr) -> bool {
    match test {
        Expr::Name(name) => name.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "TYPE_CHECKING",
        _ => false,
    }
}

impl Builder<'_> {
    pub(super) fn line(&self, range: TextRange) -> u32 {
        u32::try_from(self.lines.line_index(range.start()).get()).unwrap_or(u32::MAX)
    }

    fn dynamic(&mut self, reason: impl Into<String>) {
        if self.dynamic.is_none() {
            self.dynamic = Some(reason.into());
        }
    }

    fn bind(&mut self, name: &str, binding: Binding, conditional: bool) {
        let binding = if conditional {
            let old = self.names.get(name).cloned().unwrap_or(Binding::Unbound);
            let mut options = Vec::new();
            for option in [old, binding] {
                match option {
                    Binding::Maybe(inner) => options.extend(inner),
                    other => options.push(other),
                }
            }
            Binding::Maybe(options)
        } else {
            binding
        };
        if name == "load_tests" {
            self.dynamic("the module defines load_tests, which decides its tests");
        }
        if matches!(name, "__getattr__" | "__dir__") {
            self.dynamic(format!(
                "the module defines {name}, which changes what dir() lists"
            ));
        }
        self.names.insert(name.to_string(), binding);
    }

    fn unbind(&mut self, name: &str, conditional: bool) {
        if conditional {
            self.bind(name, Binding::Unbound, true);
        } else {
            self.names.remove(name);
        }
    }

    /// The binding of the head of a dotted path, joined with its rest.
    pub(super) fn path_of(&self, expr: &Expr) -> Option<String> {
        let dotted = dotted(expr)?;
        let (head, rest) = dotted
            .split_once('.')
            .map_or((dotted.as_str(), ""), |(h, r)| (h, r));
        let join = |base: &str| {
            if rest.is_empty() {
                base.to_string()
            } else {
                format!("{base}.{rest}")
            }
        };
        match self.names.get(head) {
            Some(Binding::Module(path) | Binding::Import(path)) => Some(join(path)),
            Some(_) => None,
            // A builtin.
            None => Some(join(&format!("builtins.{head}"))),
        }
    }

    pub(super) fn walk(&mut self, body: &[Stmt], conditional: bool) {
        for stmt in body {
            self.stmt(stmt, conditional);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, conditional: bool) {
        match stmt {
            Stmt::FunctionDef(def) => {
                self.decorators(&def.decorator_list);
                for default in function_defaults(def) {
                    self.scan(default);
                }
                let info = function_info(def);
                self.bind(def.name.id.as_str(), Binding::Function(info), conditional);
            }
            Stmt::ClassDef(class) => {
                self.decorators(&class.decorator_list);
                if let Some(arguments) = &class.arguments {
                    for arg in &arguments.args {
                        self.scan(arg);
                    }
                }
                let index = self.class(class);
                self.bind(class.name.id.as_str(), Binding::Class(index), conditional);
            }
            Stmt::Assign(assign) => {
                self.scan(&assign.value);
                let value = self.value(&assign.value);
                for target in &assign.targets {
                    self.assign(target, &assign.value, &value, conditional);
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    self.scan(value);
                    let binding = self.value(value);
                    self.assign(&assign.target, value, &binding, conditional);
                }
            }
            Stmt::AugAssign(assign) => {
                self.scan(&assign.value);
                match &*assign.target {
                    Expr::Name(name) => {
                        let binding = match self.names.get(name.id.as_str()) {
                            Some(Binding::Constant | Binding::Plain) => Binding::Plain,
                            _ => Binding::Unknown(format!("{} is changed in place", name.id)),
                        };
                        self.bind(name.id.as_str(), binding, conditional);
                    }
                    target => self.mutation(target),
                }
            }
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    match target {
                        Expr::Name(name) => self.unbind(name.id.as_str(), conditional),
                        other => self.mutation(other),
                    }
                }
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let dotted = alias.name.id.as_str();
                    self.imports.push(dotted.to_string());
                    // `import a.b` binds `a`; `import a.b as c` binds `c`.
                    let (local, path) = if let Some(asname) = &alias.asname {
                        (asname.id.as_str(), dotted.to_string())
                    } else {
                        let head = dotted.split('.').next().unwrap_or(dotted);
                        (head, head.to_string())
                    };
                    self.bind(local, Binding::Module(path), conditional);
                }
            }
            Stmt::ImportFrom(import) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(import.level as usize),
                    import.module.as_ref().map_or("", |m| m.id.as_str())
                );
                self.imports.push(module.clone());
                for alias in &import.names {
                    let name = alias.name.id.as_str();
                    if name == "*" {
                        if import.level > 0 || !is_stdlib(&module) {
                            self.dynamic(format!("the module star-imports {module}"));
                        }
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    let path = if module.ends_with('.') {
                        format!("{module}{name}")
                    } else {
                        format!("{module}.{name}")
                    };
                    self.imports.push(path.clone());
                    self.bind(local, Binding::Import(path), conditional);
                }
            }
            Stmt::Expr(expr) => self.scan(&expr.value),
            Stmt::If(s) => {
                let skipped = is_main_guard(&s.test) || is_type_checking(&s.test);
                if !skipped {
                    self.scan(&s.test);
                    self.walk(&s.body, true);
                }
                let only_else =
                    s.elif_else_clauses.len() == 1 && s.elif_else_clauses[0].test.is_none();
                for clause in &s.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.scan(test);
                    }
                    // `if __name__ == "__main__": … else: …` runs the else.
                    self.walk(&clause.body, conditional || !(skipped && only_else));
                }
            }
            Stmt::Try(s) => {
                self.walk(&s.body, true);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        if let Some(kind) = &handler.type_ {
                            self.scan(kind);
                        }
                        self.walk(&handler.body, true);
                    }
                }
                self.walk(&s.orelse, true);
                self.walk(&s.finalbody, conditional);
            }
            Stmt::With(s) => {
                for item in &s.items {
                    self.scan(&item.context_expr);
                    if let Some(vars) = &item.optional_vars {
                        self.bind_target(vars, &Binding::Unknown("bound by with".into()), true);
                    }
                }
                self.walk(&s.body, true);
            }
            Stmt::For(s) => {
                self.scan(&s.iter);
                self.bind_target(&s.target, &Binding::Unknown("bound by a loop".into()), true);
                self.walk(&s.body, true);
                self.walk(&s.orelse, true);
            }
            Stmt::While(s) => {
                self.scan(&s.test);
                self.walk(&s.body, true);
                self.walk(&s.orelse, true);
            }
            Stmt::Match(s) => {
                self.scan(&s.subject);
                for case in &s.cases {
                    let mut captures = Vec::new();
                    pattern_captures(&case.pattern, &mut captures);
                    for name in captures {
                        self.bind(&name, Binding::Unknown("bound by match".into()), true);
                    }
                    self.walk(&case.body, true);
                }
            }
            Stmt::Raise(_) => self.dynamic("the module may raise while it is imported (a skip?)"),
            Stmt::TypeAlias(alias) => {
                if let Expr::Name(name) = &*alias.name {
                    self.bind(name.id.as_str(), Binding::Plain, conditional);
                }
            }
            Stmt::Assert(s) => self.scan(&s.test),
            _ => {}
        }
    }

    fn assign(&mut self, target: &Expr, value: &Expr, binding: &Binding, conditional: bool) {
        match (target, value) {
            (Expr::Tuple(targets), Expr::Tuple(values))
                if targets.elts.len() == values.elts.len()
                    && !targets.elts.iter().any(|t| matches!(t, Expr::Starred(_))) =>
            {
                for (target, value) in targets.elts.iter().zip(&values.elts) {
                    let binding = self.value(value);
                    self.assign(target, value, &binding, conditional);
                }
            }
            _ => self.bind_target(target, binding, conditional),
        }
    }

    fn bind_target(&mut self, target: &Expr, binding: &Binding, conditional: bool) {
        match target {
            Expr::Name(name) => {
                let binding = if is_dunder(name.id.as_str())
                    && !matches!(name.id.as_str(), "__getattr__" | "__dir__")
                {
                    Binding::Constant
                } else {
                    binding.clone()
                };
                self.bind(name.id.as_str(), binding, conditional);
            }
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.bind_target(
                        elt,
                        &Binding::Unknown("bound by unpacking".into()),
                        conditional,
                    );
                }
            }
            Expr::List(list) => {
                for elt in &list.elts {
                    self.bind_target(
                        elt,
                        &Binding::Unknown("bound by unpacking".into()),
                        conditional,
                    );
                }
            }
            Expr::Starred(starred) => {
                self.bind_target(&starred.value, &Binding::Constant, conditional);
            }
            other => self.mutation(other),
        }
    }

    /// An assignment to an attribute or an item: a class's attributes may
    /// change, and so may the module's (`sys.modules[…]`).
    fn mutation(&mut self, target: &Expr) {
        let mut root = target;
        loop {
            match root {
                Expr::Attribute(attr) => root = &attr.value,
                Expr::Subscript(sub) => root = &sub.value,
                _ => break,
            }
        }
        let Expr::Name(name) = root else {
            self.scan(target);
            return;
        };
        let shown = || format!("{} is changed while the module is imported", name.id);
        match self.names.get(name.id.as_str()) {
            Some(Binding::Module(path)) => {
                if path == "sys" {
                    self.dynamic(shown());
                }
            }
            Some(Binding::Constant | Binding::Plain | Binding::Function(_)) | None => {}
            Some(_) => self.dynamic(shown()),
        }
        // Through another module (`base.Base.test_x = f`, `del
        // base.Base.test_y`): its classes may change.
        if matches!(
            self.names.get(name.id.as_str()),
            Some(Binding::Module(_) | Binding::Import(_))
        ) {
            let owner = match target {
                Expr::Attribute(attr) => Some(&*attr.value),
                Expr::Subscript(sub) => Some(&*sub.value),
                _ => None,
            };
            match owner.and_then(|owner| imported_path(&self.names, owner)) {
                Some(path) => self.touches.push(Touch::Object(path)),
                None => self.touches.push(Touch::Unknown(shown())),
            }
            if let Some(path) = imported_path(&self.names, target) {
                self.touches.push(Touch::Bound(path));
            }
        }
        self.scan(target);
    }

    /// Expressions run while the module is imported: calls that rebind
    /// names or may change classes.
    fn scan(&mut self, expr: &Expr) {
        let mut found = Scan {
            names: &self.names,
            reason: None,
            walrus: Vec::new(),
            touches: Vec::new(),
        };
        found.visit_expr(expr);
        let Scan {
            reason,
            walrus,
            touches,
            ..
        } = found;
        self.touches.extend(touches);
        if let Some(reason) = reason {
            self.dynamic(reason);
        }
        for name in walrus {
            self.bind(&name, Binding::Unknown("bound by :=".into()), true);
        }
    }

    /// Module-level decorators run at import: a local decorator with side
    /// effects, or one reaching into a class, may change classes.
    fn decorators(&mut self, decorators: &[ast::Decorator]) {
        for decorator in decorators {
            self.scan(&decorator.expression);
            let target = match &decorator.expression {
                Expr::Call(call) => &*call.func,
                other => other,
            };
            if let Expr::Name(name) = target
                && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
                && let Some(effect) = &info.side_effects
            {
                let reason = format!("the decorator {} {effect}", name.id);
                if info.foreign {
                    self.touches.push(Touch::Unknown(reason.clone()));
                }
                self.dynamic(reason);
            }
        }
    }

    /// What a value is, as a binding.
    fn value(&self, expr: &Expr) -> Binding {
        if literal_like(expr) || not_callable(expr) {
            return Binding::Constant;
        }
        match expr {
            Expr::Lambda(_) => Binding::Plain,
            Expr::Name(name) => self
                .names
                .get(name.id.as_str())
                .cloned()
                .unwrap_or(Binding::Plain),
            Expr::Attribute(_) => {
                let Some(dotted) = dotted(expr) else {
                    return Binding::Unknown("an attribute of a computed value".into());
                };
                let head = dotted.split('.').next().unwrap_or_default();
                match self.names.get(head) {
                    Some(Binding::Module(_) | Binding::Import(_)) => {
                        Binding::Import(self.path_of(expr).unwrap_or(dotted))
                    }
                    Some(Binding::Constant | Binding::Function(_)) | None => Binding::Plain,
                    Some(_) => Binding::Unknown(format!("{dotted} may be a class")),
                }
            }
            Expr::Call(call) => self.call_value(call),
            Expr::Subscript(sub) => match self.value(&sub.value) {
                Binding::Constant => Binding::Constant,
                Binding::Module(_) => Binding::Plain,
                Binding::Import(path) if is_stdlib(&path) => Binding::Plain,
                _ => Binding::Unknown("an item of a computed value".into()),
            },
            Expr::If(ternary) => {
                Binding::Maybe(vec![self.value(&ternary.body), self.value(&ternary.orelse)])
            }
            Expr::Named(named) => self.value(&named.value),
            Expr::BinOp(_) | Expr::UnaryOp(_) | Expr::BoolOp(_) | Expr::Compare(_) => {
                Binding::Plain
            }
            _ => Binding::Unknown("a value computed at import".into()),
        }
    }

    fn call_value(&self, call: &ExprCall) -> Binding {
        match &*call.func {
            Expr::Name(name) => match self.names.get(name.id.as_str()) {
                None => match name.id.as_str() {
                    "type" => Binding::Unknown("type() may make a class".into()),
                    "getattr" => Binding::Unknown("getattr() may return a class".into()),
                    _ => Binding::Plain,
                },
                Some(Binding::Class(index)) => {
                    if self.classes[*index].metaclass {
                        Binding::Unknown(format!("{} makes classes", name.id))
                    } else {
                        Binding::Plain
                    }
                }
                Some(Binding::Function(info)) => {
                    if info.may_return_class {
                        Binding::Unknown(format!("{}() may return a class", name.id))
                    } else {
                        Binding::Plain
                    }
                }
                Some(Binding::Import(path) | Binding::Module(path)) => {
                    Binding::Returned(path.clone())
                }
                Some(Binding::Constant | Binding::Plain) => Binding::Plain,
                Some(_) => Binding::Unknown(format!("{}() may return a class", name.id)),
            },
            func @ Expr::Attribute(attr) => {
                let head = dotted(func).and_then(|d| d.split('.').next().map(str::to_string));
                match head.as_deref().and_then(|h| self.names.get(h)) {
                    Some(Binding::Module(_) | Binding::Import(_)) => self
                        .path_of(func)
                        .map_or_else(|| Binding::Unknown("a call".into()), Binding::Returned),
                    Some(Binding::Class(_) | Binding::Unknown(_) | Binding::Maybe(_)) => {
                        Binding::Unknown(format!("{}() may return a class", attr.attr.id))
                    }
                    _ => Binding::Plain,
                }
            }
            _ => Binding::Unknown("a call to a computed callable".into()),
        }
    }
}

fn function_defaults(def: &StmtFunctionDef) -> impl Iterator<Item = &Expr> {
    let params = &def.parameters;
    params
        .posonlyargs
        .iter()
        .chain(&params.args)
        .chain(&params.kwonlyargs)
        .filter_map(|p| p.default.as_deref())
}

pub(super) fn pattern_captures(pattern: &ast::Pattern, out: &mut Vec<String>) {
    use ast::Pattern;
    match pattern {
        Pattern::MatchAs(p) => {
            if let Some(name) = &p.name {
                out.push(name.id.as_str().to_string());
            }
            if let Some(inner) = &p.pattern {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchStar(p) => {
            if let Some(name) = &p.name {
                out.push(name.id.as_str().to_string());
            }
        }
        Pattern::MatchMapping(p) => {
            if let Some(rest) = &p.rest {
                out.push(rest.id.as_str().to_string());
            }
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchSequence(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchClass(p) => {
            for inner in &p.arguments.patterns {
                pattern_captures(inner, out);
            }
            for keyword in &p.arguments.keywords {
                pattern_captures(&keyword.pattern, out);
            }
        }
        Pattern::MatchOr(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchValue(_) | Pattern::MatchSingleton(_) => {}
    }
}
