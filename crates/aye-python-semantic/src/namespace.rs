//! Namespaces as Python builds them while it runs a module or class body.
//!
//! A name counts as a constant only when nothing that runs while the module
//! is imported (and collected) can change its value. [`Namespace::build`]
//! therefore runs a conservative escape analysis over the whole body: every
//! use of a name other than a read it understands (an argument of a call
//! the framework keeps, such as pytest's marks, an operand, an element of a
//! literal bound to another name, which then shares its fate) marks the
//! value as possibly changed. Function bodies count only when they may run
//! during the import: when their name is used by code that runs, or their
//! decorators, dunder name or class make Python or the framework call them.
//!
//! A value bound in a class body is reachable as an attribute of anything
//! (`H.x`, `cls.x`, `P().x`), so it shares the fate of every attribute of
//! its name; and through its class: a class handed to code aye can't
//! follow (a decorator, a metaclass, a base from elsewhere, `vars(H)`, an
//! unknown call) may have any attribute changed. Code that reaches module
//! namespaces in ways aye can't follow (`sys.modules[name]` for a computed
//! name, frames and their `f_globals`, `__globals__`, `gc`, importing a
//! computed name) makes the namespace dynamic: no name is a constant, and
//! the module refines itself. `sys.modules["m"]` and `import_module("m")`
//! of literal names are recorded, so that a module looking itself up is
//! caught by its caller (which knows its name).
//!
//! The same walk records which modules code that runs may change: an
//! attribute stored to or deleted (`re.compile = …`, `m.x = …` through an
//! alias, `a.b.x = …` changing `a.b` and the module `b`), a module handed
//! to code aye can't follow (`setattr(re, …)`, `vars(re)[…]`,
//! `re.__dict__`), `sys.modules["re"]`, `__builtins__`. Evaluation reads no
//! path through a module that may change, nor a name some function
//! declares `global`. Support files in scope add their
//! own ([`Namespace::assume_patched`]); other test modules of the session,
//! installed plugins and `sitecustomize` are not seen.
//!
//! It also records what recognition needs to know which tests exist: the
//! names and classes whose value code aye can't follow may get hold of
//! ([`Namespace::handed`], [`Namespace::class_handed`],
//! [`Namespace::attribute_handed`]): handed to an unknown call, iterated
//! over, held by a name whose attributes are set or deleted. A class's own
//! decorators, keywords and bases, `super(K, …)`, and the body's own
//! `NAME.attr = …` statements (recognition reads them through
//! [`Namespace::attributes`]) don't count. And the names functions that may
//! run during the import bind or delete through `global`
//! ([`Namespace::global_writes`]).

use std::collections::{HashMap, HashSet};

use ruff_python_ast::visitor::{Visitor, walk_expr};
use ruff_python_ast::{Expr, Identifier, Operator, Pattern, Stmt, StmtClassDef, StmtFunctionDef};
use ruff_text_size::{Ranged, TextRange, TextSize};

use crate::escape::{Escaped, Escapes, Found};
use crate::framework::Framework;
use crate::hash::{FastMap, FastSet};

/// A `__test__` setting: absent, a literal, or computed at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Unset,
    Set(bool),
    Computed,
}

/// What a name is bound to, as far as the source tells.
#[derive(Debug, Clone)]
pub enum Binding<'a> {
    Function(&'a StmtFunctionDef),
    Class(&'a StmtClassDef),
    /// `name = expr`.
    Value(&'a Expr),
    /// An import: the dotted path it names (`pytest.mark`, `.helpers.x`).
    Import(String),
    /// Bound some other way: unpacking, `for`, `with ... as`, augmented
    /// assignment.
    Other,
}

/// One binding of a name, where it happens.
#[derive(Debug, Clone)]
pub struct Bound<'a> {
    pub binding: Binding<'a>,
    /// The statement that binds it.
    pub range: TextRange,
    /// The range to point at: the name in a `def`, `class` or assignment.
    pub name_range: TextRange,
    /// Inside an `if`, `except`, loop or `match`: may not run.
    pub conditional: bool,
    /// Branches taken to get here, to tell exclusive definitions apart.
    pub branches: Vec<(usize, usize)>,
}

/// An operation on a name that may change its value without rebinding it.
#[derive(Debug, Clone)]
pub struct NameOp<'a> {
    pub name: &'a str,
    pub kind: OpKind<'a>,
    pub at: TextSize,
    pub conditional: bool,
}

#[derive(Debug, Clone)]
pub enum OpKind<'a> {
    Assign(&'a Expr),
    Append(&'a Expr),
    Extend(&'a Expr),
    AugAdd(&'a Expr),
    /// Any other mutation (`x[0] = …`, `x.update(…)`, `del x[0]`).
    Mutate,
}

/// A module or class namespace, in insertion order (a Python dict: a name
/// rebound keeps its first position; deleted and bound again, it moves to
/// the end).
#[derive(Debug, Default)]
pub struct Namespace<'a> {
    slots: Vec<Option<&'a str>>,
    index: FastMap<&'a str, usize>,
    history: FastMap<&'a str, Vec<Bound<'a>>>,
    /// `from x import *` sources.
    pub star_imports: Vec<String>,
    /// Names are bound in ways no static reading can follow (`globals()`).
    pub dynamic: bool,
    /// `name.__test__ = …`: the literal, or `None` when not a literal.
    pub test_attrs: HashMap<&'a str, Option<bool>>,
    /// `name.attr = …`: the attributes set on each name.
    pub attributes: HashMap<&'a str, Vec<&'a str>>,
    pub ops: Vec<NameOp<'a>>,
    /// Declared `global` in some function: its value may change at runtime.
    pub globals: HashSet<&'a str>,
    /// Names changed in place (`x.append(…)`, `x[0] = …`, `x += …`).
    mutated: FastSet<&'a str>,
    /// Where each name is deleted.
    pub deletions: HashMap<&'a str, Vec<TextSize>>,
    /// Starts of the `del` statements that may not run.
    conditional_deletions: FastSet<TextSize>,
    /// Where the body starts.
    pub(crate) origin: Option<TextSize>,
    /// What the escape analysis found.
    pub(crate) escapes: Escaped<'a>,
    /// What the framework collecting the module does with it.
    pub(crate) framework: Framework,
    /// Code that runs reaches module namespaces in ways aye can't follow
    /// (`sys.modules[name]` for a computed name, frames, `gc`): this
    /// module's names may change, and any module's attributes.
    pub reflective: bool,
}

pub(crate) const MUTATORS: [&str; 14] = [
    "append",
    "extend",
    "insert",
    "remove",
    "pop",
    "clear",
    "update",
    "add",
    "discard",
    "setdefault",
    "sort",
    "reverse",
    "popitem",
    "__setitem__",
];

impl<'a> Namespace<'a> {
    /// The namespace a body builds, with the escape analysis's verdicts,
    /// for the framework that collects the module.
    pub fn build(body: &'a [Stmt], framework: Framework) -> Self {
        let mut ns = Self::without_escapes(body);
        ns.framework = framework;
        let found = Escapes::analyze(&ns, body);
        ns.apply(found);
        ns
    }

    /// The bindings a body makes, without the escape analysis.
    pub(crate) fn without_escapes(body: &'a [Stmt]) -> Self {
        let mut ns = Self {
            origin: body.first().map(Ranged::start),
            ..Self::default()
        };
        let mut branches = Vec::new();
        ns.walk(body, false, &mut branches);
        ns
    }

    fn apply(&mut self, found: Found<'a>) {
        self.dynamic |= found.dynamic || found.reflective;
        self.reflective |= found.reflective;
        // Changes the statements here don't show, as ops, so that readers
        // of `ops` (`collect_ignore`, `pytestmark`) give up on them too.
        let mut untracked: Vec<&'a str> = found
            .untracked
            .into_iter()
            .filter(|name| self.history.contains_key(name))
            .collect();
        untracked.sort_unstable();
        for name in untracked {
            self.ops.push(NameOp {
                name,
                kind: OpKind::Mutate,
                at: TextSize::new(u32::MAX),
                conditional: false,
            });
        }
        self.escapes = found.escaped;
    }

    /// Names in namespace order with their final binding.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &Bound<'a>)> + '_ {
        self.slots.iter().flatten().filter_map(|name| {
            let bound = self.history.get(name)?.last()?;
            Some((*name, bound))
        })
    }

    /// The final binding of `name`.
    pub fn get(&self, name: &str) -> Option<&Bound<'a>> {
        self.index.get(name)?;
        self.history.get(name)?.last()
    }

    /// Every binding of `name`, in source order (including ones a `del`
    /// removed).
    pub fn bindings(&self, name: &str) -> &[Bound<'a>] {
        self.history.get(name).map_or(&[], Vec::as_slice)
    }

    /// `name.__test__ = …` in this namespace.
    pub fn test_attr(&self, name: &str) -> Flag {
        match self.test_attrs.get(name) {
            None => Flag::Unset,
            Some(Some(value)) => Flag::Set(*value),
            Some(None) => Flag::Computed,
        }
    }

    /// Modules whose attributes code that runs may change (dotted paths).
    pub fn patched_modules(&self) -> impl Iterator<Item = &String> + '_ {
        self.escapes.patched.iter()
    }

    /// Modules that code that runs imports or looks up by a literal name.
    pub fn imported_modules(&self) -> &[String] {
        &self.escapes.modules
    }

    /// Whether code aye can't follow may get hold of what `name` is bound
    /// to, and set or delete its attributes: handed to an unknown call,
    /// iterated over, held by a name whose attributes are set or deleted.
    pub fn handed(&self, name: &str) -> bool {
        self.escapes.handed.contains(name)
    }

    /// [`Namespace::handed`] for an attribute of any class
    /// (`TestK.test_a` handed on): a method of that name.
    pub fn attribute_handed(&self, name: &str) -> bool {
        self.escapes.handed_attributes.contains(name)
    }

    /// [`Namespace::handed`] for the class a class statement makes.
    pub fn class_handed(&self, class: &StmtClassDef) -> bool {
        let origin = class.body.first().map_or(class.start(), Ranged::start);
        self.escapes.handed_classes.contains(&origin)
    }

    /// Names functions that may run during the import bind or delete
    /// through `global`, each with the value assigned, if any.
    pub fn global_writes(&self) -> &[(&'a str, Option<&'a Expr>)] {
        &self.escapes.global_writes
    }

    /// Take modules other code changes (support files) as changed here:
    /// values that depend on them are no longer evaluated.
    pub fn assume_patched(&mut self, paths: impl IntoIterator<Item = String>, all: bool) {
        self.escapes.patched.extend(paths);
        self.escapes.patches_all |= all;
    }

    /// Take the module's names as changed by code elsewhere (code that
    /// looks the module up by name, say).
    pub fn assume_reached(&mut self) {
        self.dynamic = true;
    }

    /// Whether what `path` names (a dotted path through imports or
    /// builtins) may differ from what the module it names defines: it, or
    /// a module above it, may be changed by code that runs.
    pub(crate) fn untrusted(&self, path: &str) -> bool {
        if self.escapes.patches_all {
            return true;
        }
        let mut end = 0;
        loop {
            end = path[end..].find('.').map_or(path.len(), |at| end + at);
            if self.escapes.patched.contains(&path[..end]) {
                return true;
            }
            if end == path.len() {
                return false;
            }
            end += 1;
        }
    }

    /// Whether the name's value may change without a rebinding.
    pub fn is_mutable(&self, name: &str) -> bool {
        self.changed(name)
            || self.escapes.changed.contains(name)
            || self.escapes.attributes.contains(name)
    }

    /// Whether the name's value may change through this body's own
    /// statements, or in ways no static reading follows.
    pub(crate) fn changed(&self, name: &str) -> bool {
        self.dynamic || self.globals.contains(name) || self.mutated.contains(name)
    }

    /// Whether `name` is deleted at or after `from` and before `to`:
    /// `Some(false)` when surely, `Some(true)` when by a `del` that may not
    /// run.
    pub(crate) fn deleted(&self, name: &str, from: TextSize, to: TextSize) -> Option<bool> {
        let mut found = None;
        for at in self.deletions.get(name)? {
            if *at >= from && *at < to {
                let maybe = self.conditional_deletions.contains(at);
                found = Some(found.unwrap_or(true) && maybe);
            }
        }
        found
    }

    fn bind(&mut self, name: &'a str, bound: Bound<'a>) {
        if !self.index.contains_key(name) {
            self.index.insert(name, self.slots.len());
            self.slots.push(Some(name));
        }
        self.history.entry(name).or_default().push(bound);
    }

    fn delete(&mut self, name: &str) {
        if let Some(slot) = self.index.remove(name) {
            self.slots[slot] = None;
        }
    }

    fn bind_target(
        &mut self,
        target: &'a Expr,
        binding: Binding<'a>,
        stmt: TextRange,
        conditional: bool,
        branches: &[(usize, usize)],
    ) {
        match target {
            Expr::Name(name) => self.bind(
                name.id.as_str(),
                Bound {
                    binding,
                    range: stmt,
                    name_range: name.range(),
                    conditional,
                    branches: branches.to_vec(),
                },
            ),
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.bind_target(elt, Binding::Other, stmt, conditional, branches);
                }
            }
            Expr::List(list) => {
                for elt in &list.elts {
                    self.bind_target(elt, Binding::Other, stmt, conditional, branches);
                }
            }
            Expr::Starred(starred) => {
                self.bind_target(&starred.value, Binding::Other, stmt, conditional, branches);
            }
            Expr::Attribute(attr) => {
                if let Expr::Name(owner) = &*attr.value {
                    let names = self.attributes.entry(owner.id.as_str()).or_default();
                    if !names.contains(&attr.attr.id.as_str()) {
                        names.push(attr.attr.id.as_str());
                    }
                    if attr.attr.id.as_str() == "__test__" {
                        // One that may not run leaves the value unknown.
                        let literal = match &binding {
                            Binding::Value(Expr::BooleanLiteral(b)) if !conditional => {
                                Some(b.value)
                            }
                            _ => None,
                        };
                        self.test_attrs.insert(owner.id.as_str(), literal);
                    }
                }
            }
            Expr::Subscript(sub) => {
                if let Expr::Name(owner) = &*sub.value {
                    self.op(owner.id.as_str(), OpKind::Mutate, stmt.start(), conditional);
                } else if is_namespace_call(&sub.value) {
                    self.dynamic = true;
                }
            }
            _ => {}
        }
    }

    fn op(&mut self, name: &'a str, kind: OpKind<'a>, at: TextSize, conditional: bool) {
        if !matches!(kind, OpKind::Assign(_)) {
            self.mutated.insert(name);
        }
        self.ops.push(NameOp {
            name,
            kind,
            at,
            conditional,
        });
    }

    fn walk(&mut self, body: &'a [Stmt], conditional: bool, branches: &mut Vec<(usize, usize)>) {
        for stmt in body {
            self.statement(stmt, conditional, branches);
        }
    }

    fn branch(
        &mut self,
        stmt: &Stmt,
        index: usize,
        body: &'a [Stmt],
        conditional: bool,
        branches: &mut Vec<(usize, usize)>,
    ) {
        branches.push((stmt.start().to_usize(), index));
        self.walk(body, conditional, branches);
        branches.pop();
    }

    fn statement(&mut self, stmt: &'a Stmt, conditional: bool, branches: &mut Vec<(usize, usize)>) {
        let range = stmt.range();
        // `:=` in the expressions the statement evaluates binds here (not
        // in lambda bodies), before the statement's own bindings.
        let mut walruses = Vec::new();
        for (expr, maybe) in evaluated(stmt) {
            walrus_targets(expr, maybe, &mut walruses);
        }
        for (target, maybe) in walruses {
            self.bind_target(
                target,
                Binding::Other,
                range,
                conditional || maybe,
                branches,
            );
        }
        match stmt {
            Stmt::FunctionDef(def) => {
                scan_globals(&def.body, &mut self.globals);
                self.bind(
                    def.name.id.as_str(),
                    Bound {
                        binding: Binding::Function(def),
                        range,
                        name_range: def.name.range(),
                        conditional,
                        branches: branches.clone(),
                    },
                );
            }
            Stmt::ClassDef(class) => {
                scan_globals(&class.body, &mut self.globals);
                self.bind(
                    class.name.id.as_str(),
                    Bound {
                        binding: Binding::Class(class),
                        range,
                        name_range: class.name.range(),
                        conditional,
                        branches: branches.clone(),
                    },
                );
            }
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(name) = target {
                        self.op(
                            name.id.as_str(),
                            OpKind::Assign(&assign.value),
                            range.start(),
                            conditional,
                        );
                    }
                    self.bind_target(
                        target,
                        Binding::Value(&assign.value),
                        range,
                        conditional,
                        branches,
                    );
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    if let Expr::Name(name) = &*assign.target {
                        self.op(
                            name.id.as_str(),
                            OpKind::Assign(value),
                            range.start(),
                            conditional,
                        );
                    }
                    self.bind_target(
                        &assign.target,
                        Binding::Value(value),
                        range,
                        conditional,
                        branches,
                    );
                }
            }
            Stmt::AugAssign(assign) => match &*assign.target {
                Expr::Name(name) => {
                    let kind = if matches!(assign.op, Operator::Add) {
                        OpKind::AugAdd(&assign.value)
                    } else {
                        OpKind::Mutate
                    };
                    self.op(name.id.as_str(), kind, range.start(), conditional);
                }
                Expr::Subscript(sub) => {
                    if let Expr::Name(owner) = &*sub.value {
                        self.op(
                            owner.id.as_str(),
                            OpKind::Mutate,
                            range.start(),
                            conditional,
                        );
                    }
                }
                _ => {}
            },
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    match target {
                        Expr::Name(name) => {
                            self.delete(name.id.as_str());
                            self.deletions
                                .entry(name.id.as_str())
                                .or_default()
                                .push(range.start());
                            if conditional {
                                self.conditional_deletions.insert(range.start());
                            }
                        }
                        Expr::Subscript(sub) => {
                            if let Expr::Name(owner) = &*sub.value {
                                self.op(
                                    owner.id.as_str(),
                                    OpKind::Mutate,
                                    range.start(),
                                    conditional,
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let dotted = alias.name.id.as_str();
                    let (local, path) = if let Some(asname) = &alias.asname {
                        (asname.id.as_str(), dotted.to_string())
                    } else {
                        // `import a.b` binds `a`.
                        let head = dotted.split('.').next().unwrap_or(dotted);
                        (&dotted[..head.len()], head.to_string())
                    };
                    self.bind(
                        local,
                        Bound {
                            binding: Binding::Import(path),
                            range,
                            name_range: alias.range(),
                            conditional,
                            branches: branches.clone(),
                        },
                    );
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
                        self.star_imports.push(module.clone());
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    let path = if module.ends_with('.') || module.is_empty() {
                        format!("{module}{name}")
                    } else {
                        format!("{module}.{name}")
                    };
                    self.bind(
                        local,
                        Bound {
                            binding: Binding::Import(path),
                            range,
                            name_range: alias.range(),
                            conditional,
                            branches: branches.clone(),
                        },
                    );
                }
            }
            Stmt::Expr(expr) => self.expression_statement(&expr.value, conditional),
            Stmt::If(s) => {
                self.branch(stmt, 0, &s.body, true, branches);
                for (i, clause) in s.elif_else_clauses.iter().enumerate() {
                    self.branch(stmt, i + 1, &clause.body, true, branches);
                }
            }
            Stmt::Try(s) => {
                self.branch(stmt, 0, &s.body, conditional, branches);
                for (i, handler) in s.handlers.iter().enumerate() {
                    if let Some(handler) = handler.as_except_handler() {
                        branches.push((stmt.start().to_usize(), i + 1));
                        // `except E as name` binds the name, and deletes it
                        // when the handler ends.
                        if let Some(name) = &handler.name {
                            self.bind_identifier(name, range, branches);
                        }
                        self.walk(&handler.body, true, branches);
                        branches.pop();
                    }
                }
                self.branch(stmt, 0, &s.orelse, true, branches);
                self.walk(&s.finalbody, conditional, branches);
            }
            Stmt::With(s) => {
                for item in &s.items {
                    if let Some(vars) = &item.optional_vars {
                        self.bind_target(vars, Binding::Other, range, conditional, branches);
                    }
                }
                self.walk(&s.body, conditional, branches);
            }
            Stmt::For(s) => {
                self.bind_target(&s.target, Binding::Other, range, true, branches);
                self.walk(&s.body, true, branches);
                self.walk(&s.orelse, true, branches);
            }
            Stmt::While(s) => {
                self.walk(&s.body, true, branches);
                self.walk(&s.orelse, true, branches);
            }
            Stmt::Match(s) => {
                for (i, case) in s.cases.iter().enumerate() {
                    branches.push((stmt.start().to_usize(), i));
                    let mut captures = Vec::new();
                    pattern_captures(&case.pattern, &mut captures);
                    for name in captures {
                        self.bind_identifier(name, range, branches);
                    }
                    self.walk(&case.body, true, branches);
                    branches.pop();
                }
            }
            Stmt::TypeAlias(alias) => {
                self.bind_target(&alias.name, Binding::Other, range, conditional, branches);
            }
            _ => {}
        }
    }

    fn expression_statement(&mut self, expr: &'a Expr, conditional: bool) {
        let Expr::Call(call) = expr else {
            return;
        };
        match &*call.func {
            Expr::Attribute(attr) => {
                if let Expr::Name(owner) = &*attr.value {
                    let method = attr.attr.id.as_str();
                    let first = call.arguments.args.first();
                    let kind = match (method, first) {
                        ("append", Some(arg)) => OpKind::Append(arg),
                        ("extend", Some(arg)) => OpKind::Extend(arg),
                        _ if MUTATORS.contains(&method) => OpKind::Mutate,
                        _ => return,
                    };
                    self.op(owner.id.as_str(), kind, expr.start(), conditional);
                } else if is_namespace_call(&attr.value) {
                    self.dynamic = true;
                }
            }
            Expr::Name(name) if matches!(name.id.as_str(), "exec" | "setattr") => {
                self.dynamic = true;
            }
            _ => {}
        }
    }

    /// A name bound in a branch that may not run (a `match` capture, an
    /// `except … as` name).
    fn bind_identifier(
        &mut self,
        name: &'a Identifier,
        stmt: TextRange,
        branches: &[(usize, usize)],
    ) {
        self.bind(
            name.id.as_str(),
            Bound {
                binding: Binding::Other,
                range: stmt,
                name_range: name.range(),
                conditional: true,
                branches: branches.to_vec(),
            },
        );
    }
}

/// The expressions a statement evaluates itself (not those of the bodies it
/// holds), each with whether it may not be evaluated.
pub(crate) fn evaluated(stmt: &Stmt) -> Vec<(&Expr, bool)> {
    let mut out: Vec<(&Expr, bool)> = Vec::new();
    match stmt {
        Stmt::FunctionDef(def) => {
            out.extend(def.decorator_list.iter().map(|d| (&d.expression, false)));
            for param in def.parameters.iter_non_variadic_params() {
                out.extend(param.default.as_deref().map(|d| (d, false)));
            }
            for param in &def.parameters {
                out.extend(
                    param
                        .as_parameter()
                        .annotation
                        .as_deref()
                        .map(|a| (a, false)),
                );
            }
            out.extend(def.returns.as_deref().map(|r| (r, false)));
        }
        Stmt::ClassDef(class) => {
            out.extend(class.decorator_list.iter().map(|d| (&d.expression, false)));
            if let Some(arguments) = &class.arguments {
                out.extend(arguments.args.iter().map(|a| (a, false)));
                out.extend(arguments.keywords.iter().map(|k| (&k.value, false)));
            }
        }
        Stmt::Assign(s) => {
            out.push((&s.value, false));
            out.extend(s.targets.iter().map(|t| (t, false)));
        }
        Stmt::AnnAssign(s) => {
            out.extend(s.value.as_deref().map(|v| (v, false)));
            out.push((&s.annotation, false));
            out.push((&s.target, false));
        }
        Stmt::AugAssign(s) => {
            out.push((&s.value, false));
            out.push((&s.target, false));
        }
        Stmt::Delete(s) => out.extend(s.targets.iter().map(|t| (t, false))),
        Stmt::Expr(s) => out.push((&s.value, false)),
        Stmt::Return(s) => out.extend(s.value.as_deref().map(|v| (v, false))),
        Stmt::Raise(s) => {
            out.extend(s.exc.as_deref().map(|e| (e, false)));
            out.extend(s.cause.as_deref().map(|c| (c, false)));
        }
        Stmt::Assert(s) => {
            out.push((&s.test, false));
            out.extend(s.msg.as_deref().map(|m| (m, true)));
        }
        Stmt::If(s) => {
            out.push((&s.test, false));
            out.extend(
                s.elif_else_clauses
                    .iter()
                    .filter_map(|c| c.test.as_ref())
                    .map(|t| (t, true)),
            );
        }
        Stmt::While(s) => out.push((&s.test, false)),
        Stmt::For(s) => {
            out.push((&s.iter, false));
            out.push((&s.target, false));
        }
        Stmt::With(s) => {
            for item in &s.items {
                out.push((&item.context_expr, false));
                out.extend(item.optional_vars.as_deref().map(|v| (v, false)));
            }
        }
        Stmt::Match(s) => {
            out.push((&s.subject, false));
            out.extend(
                s.cases
                    .iter()
                    .filter_map(|c| c.guard.as_deref())
                    .map(|g| (g, true)),
            );
        }
        Stmt::Try(s) => {
            for handler in &s.handlers {
                if let Some(handler) = handler.as_except_handler() {
                    out.extend(handler.type_.as_deref().map(|t| (t, true)));
                }
            }
        }
        _ => {}
    }
    out
}

/// The targets of the `:=` in an expression that bind in its scope (not in
/// lambda bodies), each with whether it may not run.
pub(crate) fn walrus_targets<'a>(expr: &'a Expr, maybe: bool, out: &mut Vec<(&'a Expr, bool)>) {
    struct Walruses<'o, 'a> {
        maybe: bool,
        out: &'o mut Vec<(&'a Expr, bool)>,
    }
    impl<'a> Walruses<'_, 'a> {
        fn visit_maybe(&mut self, expr: &'a Expr, maybe: bool) {
            let outer = self.maybe;
            self.maybe = outer || maybe;
            self.visit_expr(expr);
            self.maybe = outer;
        }
    }
    impl<'a> Visitor<'a> for Walruses<'_, 'a> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Named(named) => {
                    self.visit_expr(&named.value);
                    self.out.push((&named.target, self.maybe));
                }
                // Only the defaults are evaluated here.
                Expr::Lambda(lambda) => {
                    for param in lambda
                        .parameters
                        .iter()
                        .flat_map(|p| p.iter_non_variadic_params())
                    {
                        if let Some(default) = &param.default {
                            self.visit_expr(default);
                        }
                    }
                }
                Expr::BoolOp(op) => {
                    for (i, value) in op.values.iter().enumerate() {
                        self.visit_maybe(value, i > 0);
                    }
                }
                Expr::If(op) => {
                    self.visit_expr(&op.test);
                    self.visit_maybe(&op.body, true);
                    self.visit_maybe(&op.orelse, true);
                }
                Expr::ListComp(_) | Expr::SetComp(_) | Expr::DictComp(_) | Expr::Generator(_) => {
                    let outer = std::mem::replace(&mut self.maybe, true);
                    walk_expr(self, expr);
                    self.maybe = outer;
                }
                // Literals hold no `:=`; skip the walk.
                Expr::StringLiteral(_)
                | Expr::BytesLiteral(_)
                | Expr::NumberLiteral(_)
                | Expr::BooleanLiteral(_)
                | Expr::NoneLiteral(_)
                | Expr::EllipsisLiteral(_)
                | Expr::Name(_) => {}
                _ => walk_expr(self, expr),
            }
        }
    }
    Walruses { maybe, out }.visit_expr(expr);
}

/// The names a `match` pattern captures.
pub(crate) fn pattern_captures<'a>(pattern: &'a Pattern, out: &mut Vec<&'a Identifier>) {
    match pattern {
        Pattern::MatchValue(_) | Pattern::MatchSingleton(_) => {}
        Pattern::MatchSequence(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchMapping(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
            out.extend(p.rest.as_ref());
        }
        Pattern::MatchClass(p) => {
            for inner in &p.arguments.patterns {
                pattern_captures(inner, out);
            }
            for keyword in &p.arguments.keywords {
                pattern_captures(&keyword.pattern, out);
            }
        }
        Pattern::MatchStar(p) => out.extend(p.name.as_ref()),
        Pattern::MatchAs(p) => {
            if let Some(inner) = &p.pattern {
                pattern_captures(inner, out);
            }
            out.extend(p.name.as_ref());
        }
        Pattern::MatchOr(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
    }
}

/// The expressions directly inside an expression, in evaluation order
/// (including the parts of comprehensions, arguments, parameters and
/// f-string interpolations).
pub(crate) fn children(expr: &Expr) -> Vec<&Expr> {
    struct Children<'a>(Vec<&'a Expr>);
    impl<'a> Visitor<'a> for Children<'a> {
        fn visit_expr(&mut self, expr: &'a Expr) {
            self.0.push(expr);
        }
    }
    let mut found = Children(Vec::new());
    walk_expr(&mut found, expr);
    found.0
}

/// `globals()`, `locals()`, `vars()`.
fn is_namespace_call(expr: &Expr) -> bool {
    matches!(expr, Expr::Call(call) if call.arguments.is_empty()
        && matches!(&*call.func, Expr::Name(n) if matches!(n.id.as_str(), "globals" | "locals" | "vars")))
}

fn scan_globals<'a>(body: &'a [Stmt], out: &mut HashSet<&'a str>) {
    for stmt in body {
        match stmt {
            Stmt::Global(global) => out.extend(global.names.iter().map(|n| n.id.as_str())),
            Stmt::FunctionDef(def) => scan_globals(&def.body, out),
            Stmt::ClassDef(class) => scan_globals(&class.body, out),
            Stmt::If(s) => {
                scan_globals(&s.body, out);
                for clause in &s.elif_else_clauses {
                    scan_globals(&clause.body, out);
                }
            }
            Stmt::Try(s) => {
                scan_globals(&s.body, out);
                scan_globals(&s.orelse, out);
                scan_globals(&s.finalbody, out);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        scan_globals(&handler.body, out);
                    }
                }
            }
            Stmt::With(s) => scan_globals(&s.body, out),
            Stmt::For(s) => {
                scan_globals(&s.body, out);
                scan_globals(&s.orelse, out);
            }
            Stmt::While(s) => {
                scan_globals(&s.body, out);
                scan_globals(&s.orelse, out);
            }
            _ => {}
        }
    }
}
