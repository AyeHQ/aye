//! What in a namespace may change what doctest finds there: decorators,
//! metaclasses, expressions that alias, wrap or make objects with doctests
//! or change a namespace, calls made while the module imports, and rules
//! over the whole file; each a reason the file is refined.

use std::collections::{HashMap, HashSet};

use aye_adapter::Ranged;
use aye_adapter::ast::{self, Expr, Stmt};

use super::Finder;
use super::alias::{decorator_aliases, may_alias, returns_alias};
use super::namespace::{Binding, Kind, Namespace, docstring_of, root_name};
use super::tables::{
    IMPORT_ENDERS, METHOD_ONLY, NAMESPACE_CHANGERS, PLAIN_METACLASSES, SAME_CLASS, SAME_FUNCTION,
    SPECIAL_MODULE_NAMES,
};

/// Visits everything below an expression or a body, outside nothing.
pub(super) struct Scan<'s, 'a> {
    /// Names whose objects have doctests.
    pub(super) bearing: &'s HashSet<&'a str>,
    /// A non-callee reference to a bearing name.
    pub(super) mentions: bool,
    /// A call to one of [`NAMESPACE_CHANGERS`], `sys.modules`, or
    /// `type()` with three arguments.
    pub(super) changes_namespace: bool,
    /// A call to one of [`IMPORT_ENDERS`].
    pub(super) ends_import: bool,
    /// A string containing `>>>` inside a call.
    pub(super) doc_in_call: bool,
    /// Callees that are plain names.
    pub(super) callees: Vec<&'a str>,
    pub(super) depth_in_call: usize,
}

impl<'s, 'a> Scan<'s, 'a> {
    pub(super) fn new(bearing: &'s HashSet<&'a str>) -> Self {
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

impl<'a> Finder<'_, 'a> {
    /// Decorators on a definition with doctests must keep it as it is.
    pub(super) fn check_decorators(&mut self, binding: &Binding<'a>, class: bool, qualified: &str) {
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
    pub(super) fn check_class(&mut self, class_def: &'a ast::StmtClassDef, prefix: &str) {
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

    /// The namespace's expressions may alias, wrap or add objects with
    /// doctests, or change the namespace.
    pub(super) fn check_expressions(
        &mut self,
        ns: &Namespace<'a>,
        prefix: &str,
        bearing: &HashSet<&'a str>,
    ) {
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
    pub(super) fn whole_file(&mut self, bearing: &HashSet<&'a str>) {
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
