//! A class statement, read: its bases where it runs, its decorators, and
//! its body's members as the body leaves them ([`ClassSummary`]).
//!
//! [`ClassSummary`]: super::ClassSummary

use std::collections::BTreeMap;

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{Expr, Stmt, StmtClassDef, StmtFunctionDef};
use aye_adapter::{Ranged, TextRange};
use aye_python_stdlib::unittest::{
    CLASS_DECORATORS, KEEPING_DECORATORS, PLAIN_CLASS_DECORATORS, PROPERTY_DECORATORS,
    TEST_METHOD_PREFIX,
};

use super::builder::{not_callable, pattern_captures};
use super::tables::classify;
use super::{Base, Binding, Builder, ClassSummary, Member};

impl Builder<'_> {
    pub(super) fn class(&mut self, class: &StmtClassDef) -> usize {
        let mut summary = ClassSummary {
            name: class.name.id.as_str().to_string(),
            range: class.name.range(),
            line: self.line(class.name.range()),
            bases: Ok(Vec::new()),
            members: BTreeMap::new(),
            dynamic: None,
            init_subclass: false,
            metaclass: false,
        };
        let mut bases = Vec::new();
        let mut failed = None;
        if let Some(arguments) = &class.arguments {
            for base in &arguments.args {
                match self.base(base) {
                    Ok(base) => {
                        if matches!(&base, Base::Plain(p) if p == "builtins.type") {
                            summary.metaclass = true;
                        }
                        bases.push(base);
                    }
                    Err(reason) => {
                        failed.get_or_insert(format!("{} {reason}", summary.name));
                    }
                }
            }
            for keyword in &arguments.keywords {
                let reason = match keyword.arg.as_ref().map(|a| a.id.as_str()) {
                    Some("metaclass") => {
                        let path = self.path_of(&keyword.value);
                        if matches!(path.as_deref(), Some("abc.ABCMeta")) {
                            continue;
                        }
                        "has a metaclass, which may add attributes".to_string()
                    }
                    Some(other) => format!("passes {other}= to __init_subclass__"),
                    None => "passes keywords to __init_subclass__".to_string(),
                };
                summary
                    .dynamic
                    .get_or_insert(format!("{} {reason}", summary.name));
            }
        }
        let plain = failed.is_none()
            && bases
                .iter()
                .all(|base| matches!(base, Base::Object | Base::Plain(_)));
        summary.bases = failed.map_or(Ok(bases), Err);
        for decorator in &class.decorator_list {
            if let Err(reason) = self.class_decorator(&decorator.expression, plain) {
                summary
                    .dynamic
                    .get_or_insert(format!("{} {reason}", summary.name));
            }
        }
        let mut members = BTreeMap::new();
        let mut dynamic = None;
        self.class_body(
            &class.body,
            &summary.name,
            &mut members,
            &mut dynamic,
            &mut summary.init_subclass,
            false,
        );
        summary.members = members;
        if summary.dynamic.is_none() {
            summary.dynamic = dynamic;
        }
        self.classes.push(summary);
        self.classes.len() - 1
    }

    fn base(&self, expr: &Expr) -> Result<Base, String> {
        match expr {
            Expr::Name(name) => match self.names.get(name.id.as_str()) {
                Some(Binding::Class(index)) => Ok(Base::Local(*index)),
                Some(Binding::Import(path)) => classify(path),
                Some(_) => Err(format!("inherits from {}, bound at runtime", name.id)),
                None if name.id.as_str() == "object" => Ok(Base::Object),
                None => Ok(Base::Plain(format!("builtins.{}", name.id))),
            },
            Expr::Attribute(_) => match self.path_of(expr) {
                Some(path) if !path.starts_with("builtins.") => classify(&path),
                _ => Err("inherits from an attribute of a computed value".into()),
            },
            Expr::Subscript(sub) => self.base(&sub.value),
            _ => Err("has bases computed at runtime".into()),
        }
    }

    /// Whether a class decorator keeps the class's test names; `plain` says
    /// the class's bases can't make it a `TestCase`.
    fn class_decorator(&self, expr: &Expr, plain: bool) -> Result<(), String> {
        let callee = match expr {
            Expr::Call(call) => &*call.func,
            other => other,
        };
        if let Expr::Name(name) = callee
            && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
        {
            return if info.keeps && !info.changes_argument {
                Ok(())
            } else {
                Err(format!(
                    "is decorated with {}, which may change it",
                    name.id
                ))
            };
        }
        match self.path_of(callee) {
            Some(path)
                if CLASS_DECORATORS.contains(&path.as_str())
                    || path.starts_with("pytest.mark.")
                    || (plain && PLAIN_CLASS_DECORATORS.contains(&path.as_str())) =>
            {
                Ok(())
            }
            Some(path) => Err(format!(
                "is decorated with {}",
                path.trim_start_matches("builtins.")
            )),
            None => Err("is decorated with a computed decorator".into()),
        }
    }

    fn class_body(
        &self,
        body: &[Stmt],
        class: &str,
        members: &mut BTreeMap<String, Member>,
        dynamic: &mut Option<String>,
        init_subclass: &mut bool,
        conditional: bool,
    ) {
        let interesting = |name: &str| name.starts_with(TEST_METHOD_PREFIX) || name == "runTest";
        let put = |members: &mut BTreeMap<String, Member>, name: &str, member: Member| {
            let member = if conditional && interesting(name) {
                Member::Unknown(format!("{class}.{name} is defined conditionally"))
            } else {
                member
            };
            members.insert(name.to_string(), member);
        };
        for stmt in body {
            if let Some(reason) = class_body_dynamics(stmt) {
                dynamic.get_or_insert(format!("{class} {reason}"));
            }
            match stmt {
                Stmt::FunctionDef(def) => {
                    if def.name.id.as_str() == "__init_subclass__" {
                        *init_subclass = true;
                    }
                    let member = self.method(def, class, members);
                    put(members, def.name.id.as_str(), member);
                }
                Stmt::ClassDef(nested) => {
                    let line = self.line(nested.name.range());
                    put(
                        members,
                        nested.name.id.as_str(),
                        Member::Callable {
                            range: nested.name.range(),
                            line,
                        },
                    );
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.class_assign(
                            target,
                            Some(&assign.value),
                            class,
                            members,
                            dynamic,
                            conditional,
                        );
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        self.class_assign(
                            &assign.target,
                            Some(value),
                            class,
                            members,
                            dynamic,
                            conditional,
                        );
                    }
                }
                Stmt::AugAssign(assign) => {
                    self.class_assign(&assign.target, None, class, members, dynamic, conditional);
                }
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        if let Expr::Name(name) = target {
                            if conditional && interesting(name.id.as_str()) {
                                put(members, name.id.as_str(), Member::NotCallable);
                            } else {
                                members.remove(name.id.as_str());
                            }
                        }
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let local = alias.asname.as_ref().map_or_else(
                            || alias.name.id.as_str().split('.').next().unwrap_or_default(),
                            |a| a.id.as_str(),
                        );
                        put(members, local, Member::NotCallable);
                    }
                }
                Stmt::ImportFrom(import) => {
                    let module = format!(
                        "{}{}",
                        ".".repeat(import.level as usize),
                        import.module.as_ref().map_or("", |m| m.id.as_str())
                    );
                    for alias in &import.names {
                        let name = alias.name.id.as_str();
                        if name == "*" {
                            dynamic.get_or_insert(format!("{class} star-imports {module}"));
                            continue;
                        }
                        let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                        let path = if module.ends_with('.') {
                            format!("{module}{name}")
                        } else {
                            format!("{module}.{name}")
                        };
                        put(members, local, Member::Import(path));
                    }
                }
                Stmt::If(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    for clause in &s.elif_else_clauses {
                        self.class_body(&clause.body, class, members, dynamic, init_subclass, true);
                    }
                }
                Stmt::Try(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    for handler in &s.handlers {
                        if let Some(handler) = handler.as_except_handler() {
                            self.class_body(
                                &handler.body,
                                class,
                                members,
                                dynamic,
                                init_subclass,
                                true,
                            );
                        }
                    }
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                    self.class_body(
                        &s.finalbody,
                        class,
                        members,
                        dynamic,
                        init_subclass,
                        conditional,
                    );
                }
                Stmt::With(s) => {
                    for item in &s.items {
                        if let Some(vars) = &item.optional_vars {
                            class_targets(vars, class, members, "is bound by with");
                        }
                    }
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                }
                Stmt::For(s) => {
                    class_targets(&s.target, class, members, "is bound by a loop");
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                }
                Stmt::While(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                }
                Stmt::Match(s) => {
                    for case in &s.cases {
                        let mut captures = Vec::new();
                        pattern_captures(&case.pattern, &mut captures);
                        for name in captures {
                            if interesting(&name) {
                                members.insert(
                                    name.clone(),
                                    Member::Unknown(format!("{class}.{name} is bound by match")),
                                );
                            }
                        }
                        self.class_body(&case.body, class, members, dynamic, init_subclass, true);
                    }
                }
                _ => {}
            }
        }
    }

    fn class_assign(
        &self,
        target: &Expr,
        value: Option<&Expr>,
        class: &str,
        members: &mut BTreeMap<String, Member>,
        dynamic: &mut Option<String>,
        conditional: bool,
    ) {
        match target {
            Expr::Name(name) => {
                let id = name.id.as_str();
                if matches!(
                    id,
                    "__qualname__" | "__name__" | "__module__" | "__class__" | "__dict__"
                ) {
                    dynamic.get_or_insert(format!("{class} sets {id}"));
                }
                let member = match value {
                    Some(value) => self.member_value(value, name.range(), members),
                    None => Member::Unknown(format!("{class}.{id} is changed in place")),
                };
                let member =
                    if conditional && (id.starts_with(TEST_METHOD_PREFIX) || id == "runTest") {
                        Member::Unknown(format!("{class}.{id} is defined conditionally"))
                    } else {
                        member
                    };
                members.insert(id.to_string(), member);
            }
            Expr::Tuple(_) | Expr::List(_) | Expr::Starred(_) => {
                class_targets(target, class, members, "is bound by unpacking");
            }
            _ => {}
        }
    }

    /// What `name = value` in a class body makes the attribute: callable,
    /// not, or unknown.
    fn member_value(
        &self,
        value: &Expr,
        target: TextRange,
        members: &BTreeMap<String, Member>,
    ) -> Member {
        let callable = || Member::Callable {
            range: target,
            line: self.line(target),
        };
        if not_callable(value) {
            return Member::NotCallable;
        }
        match value {
            Expr::Lambda(_) => callable(),
            Expr::Name(name) => {
                let id = name.id.as_str();
                if let Some(member) = members.get(id) {
                    return match member {
                        Member::Callable { .. } => callable(),
                        other => other.clone(),
                    };
                }
                match self.names.get(id) {
                    Some(Binding::Function(_) | Binding::Class(_)) | None => callable(),
                    Some(Binding::Module(_) | Binding::Constant) => Member::NotCallable,
                    Some(Binding::Import(path)) => Member::Import(path.clone()),
                    Some(_) => Member::Unknown(format!("{id} may or may not be callable")),
                }
            }
            Expr::Call(call) => match self.path_of(&call.func).as_deref() {
                Some("builtins.staticmethod" | "builtins.classmethod" | "functools.partial") => {
                    callable()
                }
                Some(
                    "builtins.property"
                    | "builtins.list"
                    | "builtins.dict"
                    | "builtins.set"
                    | "builtins.tuple"
                    | "builtins.frozenset"
                    | "builtins.str"
                    | "builtins.int"
                    | "builtins.float"
                    | "builtins.bool"
                    | "builtins.bytes"
                    | "builtins.range"
                    | "functools.cached_property",
                ) => Member::NotCallable,
                _ => Member::Unknown("a value computed when the class is made".into()),
            },
            _ => Member::Unknown("a value computed when the class is made".into()),
        }
    }

    /// What a decorated method is: callable unless its outermost known
    /// decorator makes a descriptor that isn't (`property`); unknown when a
    /// decorator may replace it.
    fn method(
        &self,
        def: &StmtFunctionDef,
        class: &str,
        members: &BTreeMap<String, Member>,
    ) -> Member {
        let range = def.name.range();
        let mut member = Member::Callable {
            range,
            line: self.line(range),
        };
        for decorator in def.decorator_list.iter().rev() {
            let callee = match &decorator.expression {
                Expr::Call(call) => &*call.func,
                other => other,
            };
            if let Expr::Name(name) = callee
                && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
            {
                if info.keeps {
                    continue;
                }
                return Member::Unknown(format!(
                    "{class}.{} is decorated with {}",
                    def.name.id, name.id
                ));
            }
            if let Expr::Attribute(attr) = callee
                && let Expr::Name(owner) = &*attr.value
                && matches!(attr.attr.id.as_str(), "setter" | "getter" | "deleter")
                && members.contains_key(owner.id.as_str())
            {
                member = Member::NotCallable;
                continue;
            }
            match self.path_of(callee) {
                Some(path)
                    if KEEPING_DECORATORS.contains(&path.trim_start_matches("builtins."))
                        || path.starts_with("pytest.mark.") =>
                {
                    if !matches!(member, Member::NotCallable)
                        || path.contains("skip")
                        || path.contains("patch")
                    {
                        member = Member::Callable {
                            range,
                            line: self.line(range),
                        };
                    }
                }
                Some(path)
                    if PROPERTY_DECORATORS.contains(&path.trim_start_matches("builtins.")) =>
                {
                    member = Member::NotCallable;
                }
                Some(path) => {
                    return Member::Unknown(format!(
                        "{class}.{} is decorated with {}",
                        def.name.id,
                        path.trim_start_matches("builtins.")
                    ));
                }
                None => {
                    return Member::Unknown(format!(
                        "{class}.{} has a computed decorator",
                        def.name.id
                    ));
                }
            }
        }
        member
    }
}

/// Targets in a class body bound in ways the source doesn't value.
fn class_targets(target: &Expr, class: &str, members: &mut BTreeMap<String, Member>, how: &str) {
    match target {
        Expr::Name(name) => {
            members.insert(
                name.id.as_str().to_string(),
                Member::Unknown(format!("{class}.{} {how}", name.id)),
            );
        }
        Expr::Tuple(tuple) => {
            for elt in &tuple.elts {
                class_targets(elt, class, members, how);
            }
        }
        Expr::List(list) => {
            for elt in &list.elts {
                class_targets(elt, class, members, how);
            }
        }
        Expr::Starred(starred) => class_targets(&starred.value, class, members, how),
        _ => {}
    }
}

/// What in a class body's statement (not in its methods) may add attributes
/// the source doesn't spell: `exec`, `locals()`, `vars()`, `setattr`.
fn class_body_dynamics(stmt: &Stmt) -> Option<String> {
    struct Finder(Option<String>);
    impl<'a> Visitor<'a> for Finder {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                return;
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr
                && let Expr::Name(name) = &*call.func
                && matches!(
                    name.id.as_str(),
                    "exec" | "eval" | "locals" | "vars" | "setattr" | "globals"
                )
            {
                self.0
                    .get_or_insert(format!("calls {}() in its body", name.id));
            }
            if matches!(expr, Expr::Lambda(_)) {
                return;
            }
            visitor::walk_expr(self, expr);
        }
    }
    match stmt {
        Stmt::FunctionDef(def) => {
            let mut finder = Finder(None);
            for decorator in &def.decorator_list {
                finder.visit_expr(&decorator.expression);
            }
            finder.0
        }
        Stmt::ClassDef(_) => None,
        other => {
            let mut finder = Finder(None);
            finder.visit_stmt(other);
            finder.0
        }
    }
}
