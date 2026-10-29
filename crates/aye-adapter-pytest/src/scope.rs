//! Namespaces as Python builds them while it runs a module or class body,
//! and static evaluation of the expressions pytest turns into ids.
//!
//! A name counts as a constant only when nothing that runs while the module
//! is imported (and collected) can change its value. [`Namespace::build`]
//! therefore runs a conservative escape analysis over the whole body: every
//! use of a name other than a read it understands (an argument of a
//! `pytest.mark` call, an operand, an element of a literal bound to another
//! name, which then shares its fate) marks the value as possibly changed.
//! Function bodies count only when they may run during the import: when
//! their name is used by code that runs, or their decorators, dunder name or
//! class make Python or pytest call them.
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
//! path through a module that may change ([`Lookup::trusted_path`]), nor a
//! name some function declares `global`. Support files in scope add their
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

use aye_adapter::ast::visitor::{Visitor, walk_expr};
use aye_adapter::ast::{
    self, Expr, ExprAttribute, ExprCall, ExprContext, Identifier, Number, Operator, Parameters,
    Pattern, Stmt, StmtClassDef, StmtFunctionDef, UnaryOp,
};
use aye_adapter::{Ranged, TextRange, TextSize};

use crate::ids::{Value, int_literal_decimal};

/// A `__test__` setting: absent, a literal, or computed at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flag {
    Unset,
    Set(bool),
    Computed,
}

/// What a name is bound to, as far as the source tells.
#[derive(Debug, Clone)]
pub(crate) enum Binding<'a> {
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
pub(crate) struct Bound<'a> {
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
pub(crate) struct NameOp<'a> {
    pub name: &'a str,
    pub kind: OpKind<'a>,
    pub at: TextSize,
    pub conditional: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum OpKind<'a> {
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
pub(crate) struct Namespace<'a> {
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
    origin: Option<TextSize>,
    /// What the escape analysis found.
    escapes: Escaped<'a>,
    /// Code that runs reaches module namespaces in ways aye can't follow
    /// (`sys.modules[name]` for a computed name, frames, `gc`): this
    /// module's names may change, and any module's attributes.
    pub reflective: bool,
}

/// The escape analysis's verdicts on one body (see [`Escapes`]).
#[derive(Debug, Default)]
struct Escaped<'a> {
    /// The body's names whose value code aye can't follow may change, with
    /// every name that may share that value.
    changed: FastSet<&'a str>,
    /// The same for the names of each class body walked, by where the
    /// body starts.
    classes: FastMap<TextSize, FastSet<&'a str>>,
    /// Attributes whose value may change, read through any object: class
    /// attributes of these names may change.
    attributes_changed: FastSet<&'a str>,
    /// Names whose `__name__` may be reassigned.
    renamed: FastSet<&'a str>,
    /// Some `__name__` is reassigned through something other than a name.
    renamed_unknown: bool,
    /// Attributes assigned or deleted on some object (`Klass.attr = …`):
    /// class attributes of these names may be rebound.
    attributes: FastSet<&'a str>,
    /// Class bodies (by where they start) whose class code aye can't follow
    /// gets (a decorator, a metaclass, a base from elsewhere, or handed
    /// on): any of their attributes may change.
    classes_all: FastSet<TextSize>,
    /// Modules (dotted paths) whose attributes code that runs may change:
    /// set or deleted through a name bound to them, or the module handed to
    /// code aye can't follow (`setattr`, `vars(m)[…]`, `m.__dict__`).
    patched: FastSet<String>,
    /// Any module's attributes may change (see [`Namespace::reflective`]).
    patches_all: bool,
    /// Modules that code that runs imports or looks up by a literal name
    /// (`sys.modules["x"]`, `importlib.import_module("x")`), as written.
    modules: Vec<String>,
    /// Names of the body whose value code aye can't follow may get hold
    /// of (handed to an unknown call, a loop over it, an alias whose
    /// attributes are set, `del NAME.attr`): a class may gain or lose
    /// members, a function its `__test__` or marks. A class's own
    /// decorators, keywords and bases don't count here, nor `NAME.attr =
    /// …` statements of the body, which recognition reads.
    handed: FastSet<&'a str>,
    /// The same for every class statement walked, by where its body
    /// starts.
    handed_classes: FastSet<TextSize>,
    /// The same for attributes read through any object (`TestK.test_a`):
    /// a method of that name may change.
    handed_attributes: FastSet<&'a str>,
    /// Names functions that may run during the import declare `global`
    /// and bind or delete, with the value assigned (`None` when not an
    /// assignment).
    global_writes: Vec<(&'a str, Option<&'a Expr>)>,
}

const MUTATORS: [&str; 14] = [
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
    /// The namespace a body builds, with the escape analysis's verdicts.
    pub(crate) fn build(body: &'a [Stmt]) -> Self {
        let mut ns = Self::without_escapes(body);
        let found = Escapes::analyze(&ns, body);
        ns.apply(found);
        ns
    }

    /// The bindings a body makes, without the escape analysis.
    fn without_escapes(body: &'a [Stmt]) -> Self {
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
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&'a str, &Bound<'a>)> + '_ {
        self.slots.iter().flatten().filter_map(|name| {
            let bound = self.history.get(name)?.last()?;
            Some((*name, bound))
        })
    }

    /// The final binding of `name`.
    pub(crate) fn get(&self, name: &str) -> Option<&Bound<'a>> {
        self.index.get(name)?;
        self.history.get(name)?.last()
    }

    /// Every binding of `name`, in source order (including ones a `del`
    /// removed).
    pub(crate) fn bindings(&self, name: &str) -> &[Bound<'a>] {
        self.history.get(name).map_or(&[], Vec::as_slice)
    }

    /// `name.__test__ = …` in this namespace.
    pub(crate) fn test_attr(&self, name: &str) -> Flag {
        match self.test_attrs.get(name) {
            None => Flag::Unset,
            Some(Some(value)) => Flag::Set(*value),
            Some(None) => Flag::Computed,
        }
    }

    /// Modules whose attributes code that runs may change (dotted paths).
    pub(crate) fn patched_modules(&self) -> impl Iterator<Item = &String> + '_ {
        self.escapes.patched.iter()
    }

    /// Modules that code that runs imports or looks up by a literal name.
    pub(crate) fn imported_modules(&self) -> &[String] {
        &self.escapes.modules
    }

    /// Whether code aye can't follow may get hold of what `name` is bound
    /// to, and set or delete its attributes (see [`Escaped::handed`]).
    pub(crate) fn handed(&self, name: &str) -> bool {
        self.escapes.handed.contains(name)
    }

    /// [`Namespace::handed`] for an attribute of any class
    /// (`TestK.test_a` handed on): a method of that name.
    pub(crate) fn attribute_handed(&self, name: &str) -> bool {
        self.escapes.handed_attributes.contains(name)
    }

    /// [`Namespace::handed`] for the class a class statement makes.
    pub(crate) fn class_handed(&self, class: &StmtClassDef) -> bool {
        let origin = class.body.first().map_or(class.start(), Ranged::start);
        self.escapes.handed_classes.contains(&origin)
    }

    /// Names functions that may run during the import bind or delete
    /// through `global`, each with the value assigned, if any.
    pub(crate) fn global_writes(&self) -> &[(&'a str, Option<&'a Expr>)] {
        &self.escapes.global_writes
    }

    /// Take modules other code changes (support files) as changed here:
    /// values that depend on them are no longer evaluated.
    pub(crate) fn assume_patched(&mut self, paths: impl IntoIterator<Item = String>, all: bool) {
        self.escapes.patched.extend(paths);
        self.escapes.patches_all |= all;
    }

    /// Take the module's names as changed by code elsewhere (see
    /// [`crate::module::reached`]).
    pub(crate) fn assume_reached(&mut self) {
        self.dynamic = true;
    }

    /// Whether what `path` names (a dotted path through imports or
    /// builtins) may differ from what the module it names defines: it, or
    /// a module above it, may be changed by code that runs.
    fn untrusted(&self, path: &str) -> bool {
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
    pub(crate) fn is_mutable(&self, name: &str) -> bool {
        self.changed(name)
            || self.escapes.changed.contains(name)
            || self.escapes.attributes.contains(name)
    }

    /// Whether the name's value may change through this body's own
    /// statements, or in ways no static reading follows.
    fn changed(&self, name: &str) -> bool {
        self.dynamic || self.globals.contains(name) || self.mutated.contains(name)
    }

    /// Whether `name` is deleted at or after `from` and before `to`:
    /// `Some(false)` when surely, `Some(true)` when by a `del` that may not
    /// run.
    fn deleted(&self, name: &str, from: TextSize, to: TextSize) -> Option<bool> {
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
fn evaluated(stmt: &Stmt) -> Vec<(&Expr, bool)> {
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
fn walrus_targets<'a>(expr: &'a Expr, maybe: bool, out: &mut Vec<(&'a Expr, bool)>) {
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
fn pattern_captures<'a>(pattern: &'a Pattern, out: &mut Vec<&'a Identifier>) {
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
fn children(expr: &Expr) -> Vec<&Expr> {
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

// ----------------------------------------------------------- escape analysis

/// Rustc's `FxHasher`: the escape analysis hashes many short names, for
/// which `SipHash` is slow (and resistance to collisions pointless).
#[derive(Default, Clone, Copy)]
struct FxHasher(u64);

impl FxHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type FastSet<T> = HashSet<T, std::hash::BuildHasherDefault<FxHasher>>;
type FastMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// Methods of the builtin containers and strings that neither change their
/// receiver nor keep their arguments.
const PURE_METHODS: &[&str] = &[
    "capitalize",
    "casefold",
    "center",
    "copy",
    "count",
    "decode",
    "difference",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "format_map",
    "get",
    "hex",
    "index",
    "intersection",
    "isalnum",
    "isalpha",
    "isascii",
    "isdecimal",
    "isdigit",
    "isdisjoint",
    "isidentifier",
    "islower",
    "isnumeric",
    "isprintable",
    "isspace",
    "issubset",
    "issuperset",
    "istitle",
    "isupper",
    "items",
    "join",
    "keys",
    "ljust",
    "lower",
    "lstrip",
    "partition",
    "removeprefix",
    "removesuffix",
    "replace",
    "rfind",
    "rindex",
    "rjust",
    "rpartition",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "swapcase",
    "symmetric_difference",
    "title",
    "union",
    "upper",
    "values",
    "zfill",
];

/// Builtins that read their arguments without keeping or changing them
/// (they may call the arguments' dunder methods).
const READ_BUILTINS: &[&str] = &[
    "builtins.abs",
    "builtins.all",
    "builtins.any",
    "builtins.ascii",
    "builtins.bin",
    "builtins.bool",
    "builtins.callable",
    "builtins.chr",
    "builtins.complex",
    "builtins.divmod",
    "builtins.float",
    "builtins.format",
    "builtins.hasattr",
    "builtins.hash",
    "builtins.hex",
    "builtins.id",
    "builtins.int",
    "builtins.isinstance",
    "builtins.issubclass",
    "builtins.len",
    "builtins.oct",
    "builtins.ord",
    "builtins.pow",
    "builtins.print",
    "builtins.range",
    "builtins.repr",
    "builtins.round",
    "builtins.str",
    "builtins.sum",
    "builtins.type",
    "copy.deepcopy",
];

/// Builtins whose result holds their positional arguments or their
/// elements (when called without keywords, which may pass callables).
const FLOW_BUILTINS: &[&str] = &[
    "builtins.dict",
    "builtins.enumerate",
    "builtins.frozenset",
    "builtins.getattr",
    "builtins.iter",
    "builtins.list",
    "builtins.max",
    "builtins.min",
    "builtins.next",
    "builtins.reversed",
    "builtins.set",
    "builtins.sorted",
    "builtins.tuple",
    "builtins.vars",
    "builtins.zip",
    "copy.copy",
];

/// Dunder methods that run only once their class is used: they need an
/// explicit call, an instance, a subclass or a subscript.
const EXPLICIT_DUNDERS: &[&str] = &[
    "__call__",
    "__class_getitem__",
    "__init__",
    "__init_subclass__",
    "__new__",
    "__post_init__",
    "__set_name__",
];

/// Dunder methods whose arguments after the first never carry a value the
/// module's code holds: they get nothing, names, keys of their own, or
/// values already counted as escaping (`obj.x = v`, `obj[k] = v`).
const NO_OPERAND_DUNDERS: &[&str] = &[
    "__abs__",
    "__aenter__",
    "__aexit__",
    "__aiter__",
    "__anext__",
    "__await__",
    "__bool__",
    "__bytes__",
    "__ceil__",
    "__complex__",
    "__copy__",
    "__del__",
    "__delattr__",
    "__delete__",
    "__dir__",
    "__enter__",
    "__exit__",
    "__float__",
    "__floor__",
    "__fspath__",
    "__get__",
    "__getattr__",
    "__getattribute__",
    "__hash__",
    "__index__",
    "__int__",
    "__invert__",
    "__iter__",
    "__len__",
    "__length_hint__",
    "__neg__",
    "__next__",
    "__pos__",
    "__reduce__",
    "__repr__",
    "__reversed__",
    "__round__",
    "__set__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__trunc__",
];

/// Attributes that reach a module's namespace from a frame or a function:
/// code that reads them may change any module's names.
fn is_frame_attribute(name: &str) -> bool {
    matches!(
        name,
        "__globals__"
            | "ag_frame"
            | "cr_frame"
            | "f_back"
            | "f_builtins"
            | "f_globals"
            | "f_locals"
            | "gi_frame"
            | "tb_frame"
    )
}

/// The last part of each of [`REFLECTIVE`]'s paths, to resolve only the
/// names that may be one.
fn may_be_reflective(name: &str) -> bool {
    matches!(
        name,
        "__import__"
            | "_current_frames"
            | "_getframe"
            | "currentframe"
            | "get_objects"
            | "get_referents"
            | "get_referrers"
            | "getinnerframes"
            | "getmodule"
            | "getouterframes"
            | "import_module"
            | "modules"
            | "reload"
            | "stack"
            | "trace"
    )
}

/// What reaches module namespaces (this module's, or any module's
/// attributes) in ways aye can't follow: modules by computed name, frames,
/// the garbage collector's view of every object.
const REFLECTIVE: &[&str] = &[
    "__import__",
    "builtins.__import__",
    "gc.get_objects",
    "gc.get_referents",
    "gc.get_referrers",
    "importlib.__import__",
    "importlib.import_module",
    "importlib.reload",
    "inspect.currentframe",
    "inspect.getinnerframes",
    "inspect.getmodule",
    "inspect.getouterframes",
    "inspect.stack",
    "inspect.trace",
    "sys._current_frames",
    "sys._getframe",
    "sys.modules",
];

fn is_reflective(path: &str) -> bool {
    REFLECTIVE.contains(&path)
}

/// Whether `expr` is `sys.modules`.
fn is_sys_modules<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> bool {
    let named = match expr {
        Expr::Attribute(attr) => attr.attr.id.as_str() == "modules",
        Expr::Name(name) => name.id.as_str() == "modules",
        _ => false,
    };
    named && scope.path(expr).as_deref() == Some("sys.modules")
}

/// Whether a class base is one whose effect on its subclasses aye follows:
/// a class of the module (its `__init_subclass__` is walked), or one of the
/// standard library's that leaves subclasses' attributes alone.
fn followed_base<'a>(base: &'a Expr, scope: Lookup<'_, 'a>) -> bool {
    let target = match base {
        Expr::Subscript(sub) => &*sub.value,
        other => other,
    };
    match scope.resolve(target) {
        Resolved::Local(Bound {
            binding: Binding::Class(_),
            ..
        }) => true,
        Resolved::Path(path) => [
            "builtins.",
            "unittest.",
            "abc.",
            "typing.",
            "typing_extensions.",
            "collections.abc.",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix)),
        _ => false,
    }
}

fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// Decorators known to neither call the function they get nor rename it.
fn harmless_decorator(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    [
        "pytest.",
        "_pytest.",
        "unittest.mock.patch",
        "mock.patch",
        "unittest.skip",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || matches!(
            path,
            "abc.abstractmethod"
                | "builtins.classmethod"
                | "builtins.staticmethod"
                | "dataclasses.dataclass"
                | "functools.total_ordering"
                | "functools.wraps"
                | "typing.final"
                | "typing.overload"
                | "typing.override"
                | "typing_extensions.final"
                | "typing_extensions.overload"
                | "typing_extensions.override"
                | "unittest.expectedFailure"
        )
}

/// The dotted path a callee names, when its first name is bound to an
/// import or is a builtin (empty for imports `callee` has nothing to say
/// about). Other names resolve to themselves, which for a function's
/// parameters and locals says nothing.
fn known_path<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> Option<String> {
    let mut root = expr;
    while let Expr::Attribute(attr) = root {
        root = &attr.value;
    }
    let Expr::Name(name) = root else {
        return None;
    };
    let name = name.id.as_str();
    match scope.binding(name) {
        None if builtin_name(name).is_none() => None,
        // Only paths under these matter to `callee`; skip building others.
        Some(Bound {
            binding: Binding::Import(path),
            ..
        }) if !["pytest", "_pytest", "copy"]
            .contains(&path.split('.').next().unwrap_or_default()) =>
        {
            Some(String::new())
        }
        _ => scope.path(expr),
    }
}

/// What a decorator expression resolves to (the callee, for a call).
fn decorator_path<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> Option<String> {
    match expr {
        Expr::Call(call) => scope.path(&call.func),
        other => scope.path(other),
    }
}

/// What a name in some code refers to, for the escape analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key<'a> {
    /// A name of the analyzed body.
    Global(&'a str),
    /// A name bound in the body of the class at that index.
    Class(usize, &'a str),
    /// A local of the function (or lambda) starting there.
    Local(TextSize, &'a str),
    /// An attribute read through some object (`TestK.CASES`): a class
    /// attribute of any class.
    Attr(&'a str),
}

/// Where a value goes once an expression has produced it.
#[derive(Clone, Default)]
struct Sink<'a> {
    /// Code aye can't follow may keep or change it.
    escape: bool,
    /// It goes to an operator or a builtin, which may call one of the
    /// module's dunder methods with it.
    operand: bool,
    /// Names that hold it from now on (bound to it, or to a container
    /// holding it).
    binds: Vec<Key<'a>>,
    /// Its attributes are set or deleted (it owns an attribute that is
    /// stored to): a module it is changes.
    patch: bool,
    /// Its attributes are set or deleted other than by a statement of the
    /// analyzed body naming it (`K.attr = …`, which the namespace records
    /// in [`Namespace::attributes`]): a class or function it is may gain
    /// or lose tests in ways aye doesn't read.
    touch: bool,
}

impl Sink<'_> {
    fn inert() -> Self {
        Self::default()
    }

    fn operand() -> Self {
        Self {
            operand: true,
            ..Self::default()
        }
    }

    fn escape() -> Self {
        Self {
            escape: true,
            ..Self::default()
        }
    }

    fn with_operand(mut self) -> Self {
        self.operand = true;
        self
    }

    /// The owner of an attribute stored to or deleted.
    fn patched() -> Self {
        Self {
            operand: true,
            patch: true,
            touch: true,
            ..Self::default()
        }
    }

    /// Whether whoever gets the value keeps it.
    fn held(&self) -> bool {
        self.escape || !self.binds.is_empty()
    }

    /// Where the parts of a value go (its elements, the operands it was
    /// computed from): wherever the value is held.
    fn part(&self) -> Self {
        Self {
            escape: self.escape,
            operand: false,
            binds: self.binds.clone(),
            patch: self.patch,
            touch: self.touch,
        }
    }
}

/// What one body of code does with names.
#[derive(Default)]
struct Facts<'a> {
    /// Names it reads (or calls), by name.
    loads: Vec<&'a str>,
    /// Values code aye can't follow may change or keep.
    escapes: Vec<Key<'a>>,
    /// Values changed in place by statements [`Namespace::statement`]
    /// records as ops.
    tracked: Vec<Key<'a>>,
    /// Values going to an operator or builtin.
    operands: Vec<Key<'a>>,
    /// Pairs of names that may hold the same value.
    unions: Vec<(Key<'a>, Key<'a>)>,
    /// Names whose `__name__` is reassigned.
    renamed: Vec<Key<'a>>,
    /// Attributes assigned or deleted.
    attributes: Vec<&'a str>,
    renamed_unknown: bool,
    /// The namespace may change in ways no static reading follows.
    dynamic: bool,
    /// Values whose attributes are set or deleted.
    patched: Vec<Key<'a>>,
    /// Values whose attributes are set or deleted other than by a
    /// statement of the analyzed body naming them (see [`Sink::touch`]).
    touched: Vec<Key<'a>>,
    /// Names whose attributes a statement of the analyzed body sets
    /// (`K.attr = …`).
    direct: Vec<Key<'a>>,
    /// Classes handed to code aye can't follow by their own statement: a
    /// decorator, a keyword, a base from elsewhere.
    defined: Vec<Key<'a>>,
    /// Names a function declares `global` and binds or deletes, with the
    /// value assigned (`None` for a `def`, `class`, `del`, import or any
    /// other binding).
    global_writes: Vec<(&'a str, Option<&'a Expr>)>,
    /// Modules changed through a literal name (`sys.modules["m"].x = …`,
    /// `__builtins__`).
    patched_paths: Vec<String>,
    /// Names bound by imports, and the dotted path each names.
    imports: Vec<(Key<'a>, String)>,
    /// Modules imported or looked up by a literal name.
    modules: Vec<String>,
    /// Module namespaces are reached in ways aye can't follow.
    reflective: bool,
}

/// When a body of code runs during the import.
#[derive(Clone, Copy)]
enum Trigger<'a> {
    /// The analyzed body itself.
    Root,
    /// Whenever the code defining it runs: lambdas, functions whose
    /// decorators may call them, dunder methods Python or pytest calls on
    /// their own, hooks.
    Always,
    /// A function: when code that runs uses its name.
    Named(&'a str),
    /// A method: when its class is used, or code that runs uses its name.
    Method(usize, &'a str),
}

struct Unit<'a> {
    parent: usize,
    /// Where the function starts (`None` for the analyzed body).
    id: Option<TextSize>,
    /// A function whose body is walked once it may run.
    def: Option<&'a StmtFunctionDef>,
    /// Walked: it may run during the import.
    runs: bool,
    /// The function's locals: its parameters and the names its body binds
    /// (not declared `global` or `nonlocal`).
    locals: FastSet<&'a str>,
    trigger: Trigger<'a>,
    facts: Facts<'a>,
    /// For a dunder method Python calls with operands (`__eq__`,
    /// `__add__`…): its parameters after the first.
    operand_params: Vec<&'a str>,
}

struct ClassUse<'a> {
    name: &'a str,
    /// The name the class statement binds.
    key: Key<'a>,
    /// Where its body starts: how its namespace is found again.
    origin: TextSize,
    /// The unit whose code runs the class statement.
    unit: usize,
    /// Used whatever its name's uses: decorated, with keywords, or with
    /// bases that may call into it.
    intrinsic: bool,
}

/// Names that may hold the same value, as a union-find.
#[derive(Default)]
struct Groups<'a> {
    index: FastMap<Key<'a>, usize>,
    keys: Vec<Key<'a>>,
    parent: Vec<usize>,
}

impl<'a> Groups<'a> {
    fn root(&mut self, key: Key<'a>) -> usize {
        let mut at = if let Some(at) = self.index.get(&key) {
            *at
        } else {
            let at = self.keys.len();
            self.index.insert(key, at);
            self.keys.push(key);
            self.parent.push(at);
            at
        };
        while self.parent[at] != at {
            self.parent[at] = self.parent[self.parent[at]];
            at = self.parent[at];
        }
        at
    }

    fn union(&mut self, a: Key<'a>, b: Key<'a>) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent[a] = b;
        }
    }
}

/// The escape analysis's result for one body.
struct Found<'a> {
    escaped: Escaped<'a>,
    /// Names of the body changed in ways its statements don't record as
    /// ops.
    untracked: FastSet<&'a str>,
    dynamic: bool,
    /// See [`Namespace::reflective`].
    reflective: bool,
}

/// How a call treats its arguments.
enum Callee {
    /// Keeps them, never changes them (`pytest.mark.*`, `pytest.param`).
    Inert,
    /// Reads them.
    Operand,
    /// Its result holds them.
    Flow,
    /// Anything may happen to them.
    Unknown,
}

/// The escape analysis: a walk over every expression of a body, class
/// bodies included, recording what each body of code does with names, and
/// over the bodies of the functions it defines once they may run during the
/// import; then which values code aye can't follow may change.
///
/// It is conservative: names that may hold the same value (an alias, an
/// element of a literal, a loop variable, a parameter and its default)
/// share their fate.
struct Escapes<'s, 'a> {
    /// The analyzed body's namespace, for resolving names.
    module: &'s Namespace<'a>,
    units: Vec<Unit<'a>>,
    classes: Vec<ClassUse<'a>>,
    /// The unit being walked.
    unit: usize,
    /// The class whose body is being walked (not one of its methods).
    class: Option<usize>,
    /// Where the statement being walked starts.
    at: TextSize,
}

impl<'s, 'a> Escapes<'s, 'a> {
    fn analyze(ns: &'s Namespace<'a>, body: &'a [Stmt]) -> Found<'a> {
        let mut this = Self {
            module: ns,
            units: vec![Unit {
                parent: 0,
                id: None,
                def: None,
                runs: true,
                locals: FastSet::default(),
                trigger: Trigger::Root,
                facts: Facts::default(),
                operand_params: Vec::new(),
            }],
            classes: Vec::new(),
            unit: 0,
            class: None,
            at: TextSize::default(),
        };
        let scope = Lookup {
            module: ns,
            class: None,
            source: "",
            at: TextSize::new(u32::MAX),
        };
        this.body(body, scope);
        this.run_functions();
        this.finish()
    }

    /// Walk the bodies of the functions that may run, until no more may.
    fn run_functions(&mut self) {
        let mut loaded: FastSet<&'a str> = self.units[0].facts.loads.iter().copied().collect();
        // Passes in definition order: a pass that walks nothing ends it.
        let mut more = true;
        while more {
            more = false;
            let mut index = 1;
            while index < self.units.len() {
                let unit = &self.units[index];
                let fires = !unit.runs
                    && self.units[unit.parent].runs
                    && match unit.trigger {
                        Trigger::Root | Trigger::Always => true,
                        Trigger::Named(name) => loaded.contains(name),
                        Trigger::Method(class, name) => {
                            let class = &self.classes[class];
                            loaded.contains(name)
                                || (self.units[class.unit].runs
                                    && (class.intrinsic || loaded.contains(class.name)))
                        }
                    };
                if fires {
                    self.run_function(index);
                    loaded.extend(self.units[index].facts.loads.iter().copied());
                    more = true;
                }
                index += 1;
            }
        }
    }

    fn run_function(&mut self, index: usize) {
        let def = self.units[index].def.expect("only functions wait");
        self.units[index].locals = function_locals(Some(&def.parameters), &def.body);
        self.units[index].runs = true;
        let (unit, class) = (self.unit, self.class.take());
        self.unit = index;
        let scope = Lookup {
            module: self.module,
            class: None,
            source: "",
            at: TextSize::new(u32::MAX),
        };
        self.body(&def.body, scope);
        self.unit = unit;
        self.class = class;
    }

    /// Statements of the analyzed body itself, outside class bodies: the
    /// ones [`Namespace::statement`] walks.
    fn top(&self) -> bool {
        self.unit == 0 && self.class.is_none()
    }

    fn facts(&mut self) -> &mut Facts<'a> {
        &mut self.units[self.unit].facts
    }

    /// What a name refers to outside class bodies: the local of the
    /// innermost function binding it, else the analyzed body's name.
    fn outer(&self, name: &'a str) -> Key<'a> {
        let mut unit = self.unit;
        loop {
            let found = &self.units[unit];
            if let Some(id) = found.id
                && found.locals.contains(name)
            {
                return Key::Local(id, name);
            }
            if unit == 0 {
                return Key::Global(name);
            }
            unit = found.parent;
        }
    }

    /// What a name read here refers to: in a class body, the class's own
    /// binding when it surely exists, the enclosing scope's when it surely
    /// doesn't, else both.
    fn keys(&self, name: &'a str, scope: Lookup<'_, 'a>) -> [Option<Key<'a>>; 2] {
        let (Some(class), Some(ns)) = (self.class, scope.class) else {
            return [Some(self.outer(name)), None];
        };
        let before: Vec<&Bound<'a>> = ns
            .bindings(name)
            .iter()
            .filter(|b| b.range.end() <= self.at)
            .collect();
        match before.first() {
            None => [Some(self.outer(name)), None],
            Some(first)
                if !before.iter().any(|b| b.conditional)
                    && ns.deleted(name, first.range.start(), self.at).is_none() =>
            {
                [Some(Key::Class(class, name)), None]
            }
            Some(_) => [Some(Key::Class(class, name)), Some(self.outer(name))],
        }
    }

    /// What a name bound here is.
    fn bound(&self, name: &'a str) -> Key<'a> {
        match self.class {
            Some(class) => Key::Class(class, name),
            None => self.outer(name),
        }
    }

    /// A name a statement binds or deletes, `value` when it assigns one:
    /// recorded when a function binds it after declaring it `global`.
    fn global_write(&mut self, name: &'a str, value: Option<&'a Expr>) {
        if self.unit != 0
            && self.class.is_none()
            && self.units[self.unit].def.is_some()
            && matches!(self.outer(name), Key::Global(_))
        {
            self.facts().global_writes.push((name, value));
        }
    }

    /// [`Escapes::global_write`] for the names an assignment target binds.
    fn global_targets(&mut self, target: &'a Expr, value: Option<&'a Expr>) {
        match target {
            Expr::Name(name) => self.global_write(name.id.as_str(), value),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| self.global_targets(e, None)),
            Expr::List(list) => list.elts.iter().for_each(|e| self.global_targets(e, None)),
            Expr::Starred(starred) => self.global_targets(&starred.value, None),
            _ => {}
        }
    }

    /// A name read, its value going to `sink`.
    fn load(&mut self, name: &'a str, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        self.facts().loads.push(name);
        for key in self.keys(name, scope).into_iter().flatten() {
            self.flow(key, sink);
        }
    }

    /// The value `key` holds goes to `sink`.
    fn flow(&mut self, key: Key<'a>, sink: &Sink<'a>) {
        let facts = self.facts();
        if sink.escape {
            facts.escapes.push(key);
        }
        if sink.patch {
            facts.patched.push(key);
        }
        if sink.touch {
            facts.touched.push(key);
        }
        if sink.operand {
            facts.operands.push(key);
        }
        for bound in &sink.binds {
            facts.unions.push((key, *bound));
        }
    }

    /// The value of `name` changed in place, by a statement of the analyzed
    /// body when `tracked`.
    fn changed(&mut self, name: &'a str, tracked: bool, scope: Lookup<'_, 'a>) {
        if name == "__builtins__" {
            // The builtins' namespace (a module's `__builtins__` is its
            // dict).
            self.facts().patched_paths.push("builtins".to_string());
        }
        let tracked = tracked && self.top();
        let keys = self.keys(name, scope).into_iter().flatten();
        let facts = self.facts();
        facts.loads.push(name);
        if tracked {
            facts.tracked.extend(keys);
        } else {
            facts.escapes.extend(keys);
        }
    }

    /// Where the value assigned to `targets` goes: the names they bind, and
    /// code aye can't follow when one is an attribute or a subscript.
    fn binding_sink(&self, targets: &'a [Expr]) -> Sink<'a> {
        fn collect<'a>(this: &Escapes<'_, 'a>, target: &'a Expr, sink: &mut Sink<'a>) {
            match target {
                Expr::Name(name) => sink.binds.push(this.bound(name.id.as_str())),
                Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| collect(this, e, sink)),
                Expr::List(list) => list.elts.iter().for_each(|e| collect(this, e, sink)),
                Expr::Starred(starred) => collect(this, &starred.value, sink),
                _ => sink.escape = true,
            }
        }
        let mut sink = Sink::default();
        for target in targets {
            collect(self, target, &mut sink);
        }
        sink
    }

    /// Walk the body of the lambda starting at `id`, which runs whenever the
    /// code defining it does.
    fn nested(&mut self, id: TextSize, locals: FastSet<&'a str>, walk: impl FnOnce(&mut Self)) {
        let index = self.units.len();
        self.units.push(Unit {
            parent: self.unit,
            id: Some(id),
            def: None,
            runs: true,
            locals,
            trigger: Trigger::Always,
            facts: Facts::default(),
            operand_params: Vec::new(),
        });
        let (unit, class) = (self.unit, self.class.take());
        self.unit = index;
        walk(self);
        self.unit = unit;
        self.class = class;
    }

    fn body(&mut self, body: &'a [Stmt], scope: Lookup<'_, 'a>) {
        for stmt in body {
            self.at = stmt.start();
            self.stmt(stmt, scope);
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt, scope: Lookup<'_, 'a>) {
        match stmt {
            Stmt::FunctionDef(def) => self.function(def, scope),
            Stmt::ClassDef(class) => self.class(class, scope),
            Stmt::Return(s) => {
                if let Some(value) = &s.value {
                    self.expr(value, &Sink::escape(), scope);
                }
            }
            Stmt::Delete(s) => {
                for target in &s.targets {
                    self.global_targets(target, None);
                    self.store(target, scope, true);
                }
            }
            Stmt::Assign(s) => self.assign(&s.targets, &s.value, scope),
            Stmt::AnnAssign(s) => {
                self.expr(&s.annotation, &Sink::operand(), scope);
                if let Some(value) = &s.value {
                    self.assign(std::slice::from_ref(&*s.target), value, scope);
                }
            }
            Stmt::AugAssign(s) => {
                self.expr(&s.value, &Sink::escape(), scope);
                self.global_targets(&s.target, Some(&s.value));
                match &*s.target {
                    Expr::Name(name) => self.changed(name.id.as_str(), true, scope),
                    target => self.store(target, scope, true),
                }
            }
            Stmt::For(s) => {
                let sink = self
                    .binding_sink(std::slice::from_ref(&*s.target))
                    .with_operand();
                self.expr(&s.iter, &sink, scope);
                self.global_targets(&s.target, None);
                self.store(&s.target, scope, true);
                self.body(&s.body, scope);
                self.body(&s.orelse, scope);
            }
            Stmt::While(s) => {
                self.expr(&s.test, &Sink::operand(), scope);
                self.body(&s.body, scope);
                self.body(&s.orelse, scope);
            }
            Stmt::If(s) => {
                self.expr(&s.test, &Sink::operand(), scope);
                self.body(&s.body, scope);
                for clause in &s.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.expr(test, &Sink::operand(), scope);
                    }
                    self.body(&clause.body, scope);
                }
            }
            Stmt::With(s) => {
                for item in &s.items {
                    self.expr(&item.context_expr, &Sink::escape(), scope);
                    if let Some(vars) = &item.optional_vars {
                        self.global_targets(vars, None);
                        self.store(vars, scope, true);
                    }
                }
                self.body(&s.body, scope);
            }
            Stmt::Match(s) => {
                let mut captures = Vec::new();
                for case in &s.cases {
                    pattern_captures(&case.pattern, &mut captures);
                }
                let sink = Sink {
                    operand: true,
                    binds: captures
                        .iter()
                        .map(|name| self.bound(name.id.as_str()))
                        .collect(),
                    ..Sink::default()
                };
                self.expr(&s.subject, &sink, scope);
                for case in &s.cases {
                    self.pattern(&case.pattern, scope);
                    if let Some(guard) = &case.guard {
                        self.expr(guard, &Sink::operand(), scope);
                    }
                    self.body(&case.body, scope);
                }
            }
            Stmt::Raise(s) => {
                for expr in s.exc.iter().chain(&s.cause) {
                    self.expr(expr, &Sink::escape(), scope);
                }
            }
            Stmt::Try(s) => {
                self.body(&s.body, scope);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        if let Some(kind) = &handler.type_ {
                            self.expr(kind, &Sink::operand(), scope);
                        }
                        self.body(&handler.body, scope);
                    }
                }
                self.body(&s.orelse, scope);
                self.body(&s.finalbody, scope);
            }
            Stmt::Assert(s) => {
                for expr in std::iter::once(&*s.test).chain(s.msg.as_deref()) {
                    self.expr(expr, &Sink::operand(), scope);
                }
            }
            Stmt::Expr(s) => self.expression_statement(&s.value, scope),
            Stmt::Import(s) => {
                for alias in &s.names {
                    let dotted = alias.name.id.as_str();
                    // `import a.b` binds `a`.
                    let head = dotted.split('.').next().unwrap_or(dotted);
                    let (local, path) = alias
                        .asname
                        .as_ref()
                        .map_or((head, head), |asname| (asname.id.as_str(), dotted));
                    let key = self.bound(local);
                    self.global_write(local, None);
                    let facts = self.facts();
                    facts.modules.push(dotted.to_string());
                    facts.imports.push((key, path.to_string()));
                }
            }
            Stmt::ImportFrom(s) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(s.level as usize),
                    s.module.as_ref().map_or("", |m| m.id.as_str())
                );
                self.facts().modules.push(module.clone());
                for alias in &s.names {
                    let name = alias.name.id.as_str();
                    if name == "*" {
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    let path = if module.ends_with('.') {
                        format!("{module}{name}")
                    } else {
                        format!("{module}.{name}")
                    };
                    let key = self.bound(local);
                    self.global_write(local, None);
                    let facts = self.facts();
                    facts.modules.push(path.clone());
                    facts.imports.push((key, path));
                }
            }
            // A type alias's value is evaluated lazily.
            Stmt::TypeAlias(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
    }

    /// `NAME.mutator(…)` as a statement of the analyzed body is one of
    /// [`Namespace::statement`]'s ops.
    fn expression_statement(&mut self, expr: &'a Expr, scope: Lookup<'_, 'a>) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(attr) = &*call.func
            && let Expr::Name(owner) = &*attr.value
            && MUTATORS.contains(&attr.attr.id.as_str())
            && self.top()
        {
            self.changed(owner.id.as_str(), true, scope);
            self.arguments(&call.arguments, &Sink::escape(), scope);
            return;
        }
        self.expr(expr, &Sink::inert(), scope);
    }

    fn assign(&mut self, targets: &'a [Expr], value: &'a Expr, scope: Lookup<'_, 'a>) {
        let sink = self.binding_sink(targets);
        self.expr(value, &sink, scope);
        for target in targets {
            self.global_targets(target, Some(value));
            match target {
                // `K.attr = …` in the analyzed body: the namespace records
                // it, so what it does to a class or function is read.
                Expr::Attribute(attr) if self.top() && matches!(&*attr.value, Expr::Name(_)) => {
                    self.set_attribute(attr, scope, true);
                }
                _ => self.store(target, scope, true),
            }
        }
    }

    /// An assignment or deletion target; `tracked` when
    /// [`Namespace::statement`] records it (not in a comprehension).
    fn store(&mut self, target: &'a Expr, scope: Lookup<'_, 'a>, tracked: bool) {
        match target {
            Expr::Name(_) => {}
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.store(elt, scope, tracked);
                }
            }
            Expr::List(list) => {
                for elt in &list.elts {
                    self.store(elt, scope, tracked);
                }
            }
            Expr::Starred(starred) => self.store(&starred.value, scope, tracked),
            Expr::Attribute(attr) => self.set_attribute(attr, scope, false),
            Expr::Subscript(sub) if self.module_lookup(sub, scope) => {}
            Expr::Subscript(sub) => {
                match &*sub.value {
                    Expr::Name(owner) => self.changed(owner.id.as_str(), tracked, scope),
                    owner => self.expr(owner, &Sink::escape(), scope),
                }
                self.expr(&sub.slice, &Sink::operand(), scope);
            }
            other => self.expr(other, &Sink::escape(), scope),
        }
    }

    /// `owner.attr = …`, `owner.attr += …` or `del owner.attr`; `direct`
    /// for `NAME.attr = …` as a statement of the analyzed body.
    fn set_attribute(&mut self, attr: &'a ExprAttribute, scope: Lookup<'_, 'a>, direct: bool) {
        let name = attr.attr.id.as_str();
        self.facts().attributes.push(name);
        if name == "__name__" {
            match &*attr.value {
                Expr::Name(owner) => {
                    let keys = self.keys(owner.id.as_str(), scope);
                    self.facts().renamed.extend(keys.into_iter().flatten());
                }
                _ => self.facts().renamed_unknown = true,
            }
        }
        // `a.b.x = …` changes what `a.b` is: the module `a.b` names, or
        // (the way modules usually hold one) the module `b`.
        if matches!(&*attr.value, Expr::Attribute(_))
            && let Some(owner) = scope.path(&attr.value)
        {
            let last = owner.rsplit('.').next().unwrap_or(&owner).to_string();
            let facts = self.facts();
            facts.patched_paths.push(owner);
            facts.patched_paths.push(last);
        }
        // `m.x = …` on a name only ever bound to imports changes `m.x`.
        if let Expr::Name(owner) = &*attr.value
            && let [Some(Key::Global(global)), None] = self.keys(owner.id.as_str(), scope)
        {
            let bindings = self.module.bindings(global);
            let paths: Vec<String> = bindings
                .iter()
                .filter_map(|b| match &b.binding {
                    Binding::Import(path) => Some(format!("{path}.{name}")),
                    _ => None,
                })
                .collect();
            if !paths.is_empty() && paths.len() == bindings.len() {
                self.facts().patched_paths.extend(paths);
                self.load(owner.id.as_str(), &Sink::operand(), scope);
                return;
            }
        }
        if direct && let Expr::Name(owner) = &*attr.value {
            let key = self.outer(owner.id.as_str());
            self.facts().direct.push(key);
            let sink = Sink {
                touch: false,
                ..Sink::patched()
            };
            self.load(owner.id.as_str(), &sink, scope);
            return;
        }
        self.expr(&attr.value, &Sink::patched(), scope);
    }

    /// The defaults and annotations of the function starting at `id`.
    fn parameters(&mut self, parameters: &'a Parameters, id: TextSize, scope: Lookup<'_, 'a>) {
        for param in parameters.iter_non_variadic_params() {
            if let Some(default) = &param.default {
                let sink = Sink {
                    binds: vec![Key::Local(id, param.parameter.name.id.as_str())],
                    ..Sink::default()
                };
                self.expr(default, &sink, scope);
            }
        }
        for param in parameters {
            if let Some(annotation) = &param.as_parameter().annotation {
                self.expr(annotation, &Sink::operand(), scope);
            }
        }
    }

    fn function(&mut self, def: &'a StmtFunctionDef, scope: Lookup<'_, 'a>) {
        let mut harmless = true;
        for decorator in &def.decorator_list {
            self.expr(&decorator.expression, &Sink::inert(), scope);
            harmless &= harmless_decorator(decorator_path(&decorator.expression, scope).as_deref());
        }
        let id = def.start();
        self.parameters(&def.parameters, id, scope);
        if let Some(returns) = &def.returns {
            self.expr(returns, &Sink::operand(), scope);
        }
        let name = def.name.id.as_str();
        self.global_write(name, None);
        // Python calls dunder methods on its own (and pytest instantiates
        // test classes and reads their attributes while collecting); pytest
        // calls hooks.
        let implicit = is_dunder(name) && !EXPLICIT_DUNDERS.contains(&name);
        let trigger = if !harmless || implicit || (self.top() && name.starts_with("pytest_")) {
            Trigger::Always
        } else if let Some(class) = self.class {
            Trigger::Method(class, name)
        } else {
            Trigger::Named(name)
        };
        let operand_params = if implicit && !NO_OPERAND_DUNDERS.contains(&name) {
            def.parameters
                .iter()
                .skip(1)
                .map(|p| p.as_parameter().name.id.as_str())
                .collect()
        } else {
            Vec::new()
        };
        // Its body is walked if it may run (`run_functions`).
        self.units.push(Unit {
            parent: self.unit,
            id: Some(id),
            def: Some(def),
            runs: false,
            locals: FastSet::default(),
            trigger,
            facts: Facts::default(),
            operand_params,
        });
    }

    fn class(&mut self, class: &'a StmtClassDef, scope: Lookup<'_, 'a>) {
        let key = self.bound(class.name.id.as_str());
        let mut intrinsic = false;
        // The class goes to code aye can't follow, which may change any of
        // its attributes: a decorator, a metaclass, a base from elsewhere
        // (its `__init_subclass__`).
        let mut handed = false;
        for decorator in &class.decorator_list {
            self.expr(&decorator.expression, &Sink::inert(), scope);
            let harmless =
                harmless_decorator(decorator_path(&decorator.expression, scope).as_deref());
            intrinsic |= !harmless;
            handed |= !harmless;
        }
        if let Some(arguments) = &class.arguments {
            for base in &*arguments.args {
                self.expr(base, &Sink::operand(), scope);
                // A base may call into its subclasses (`__init_subclass__`,
                // a metaclass); builtin and unittest bases don't.
                intrinsic |= !scope
                    .path(base)
                    .is_some_and(|p| p.starts_with("builtins.") || p.starts_with("unittest."));
                handed |= !followed_base(base, scope);
            }
            for keyword in &arguments.keywords {
                self.expr(&keyword.value, &Sink::escape(), scope);
                intrinsic = true;
                handed = true;
            }
        }
        if handed {
            self.facts().defined.push(key);
        }
        self.global_write(class.name.id.as_str(), None);
        let index = self.classes.len();
        self.classes.push(ClassUse {
            key,
            name: class.name.id.as_str(),
            origin: class.body.first().map_or(class.start(), Ranged::start),
            unit: self.unit,
            intrinsic,
        });
        let ns = Namespace::without_escapes(&class.body);
        let inner = Lookup {
            class: Some(&ns),
            ..scope
        };
        let (outer, at) = (self.class.replace(index), self.at);
        self.body(&class.body, inner);
        self.class = outer;
        self.at = at;
    }

    fn arguments(&mut self, arguments: &'a ast::Arguments, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        for arg in &*arguments.args {
            self.expr(arg, sink, scope);
        }
        for keyword in &arguments.keywords {
            self.expr(&keyword.value, sink, scope);
        }
    }

    fn expr(&mut self, expr: &'a Expr, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        match expr {
            Expr::Name(name) => {
                if matches!(name.ctx, ExprContext::Load) {
                    let id = name.id.as_str();
                    if id == "__builtins__" {
                        self.facts().patched_paths.push("builtins".to_string());
                    } else if may_be_reflective(id)
                        && scope.path(expr).is_some_and(|p| is_reflective(&p))
                    {
                        self.facts().reflective = true;
                    }
                    self.load(id, sink, scope);
                }
            }
            Expr::Attribute(attr) => self.attribute(attr, sink, scope),
            Expr::Subscript(sub) if self.module_lookup(sub, scope) => {}
            Expr::Subscript(sub) => {
                self.expr(&sub.value, &sink.part().with_operand(), scope);
                self.expr(&sub.slice, &Sink::operand(), scope);
            }
            Expr::Starred(starred) => self.expr(&starred.value, sink, scope),
            Expr::List(list) => {
                for elt in &list.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Set(set) => {
                for elt in &set.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Dict(dict) => {
                for item in &dict.items {
                    if let Some(key) = &item.key {
                        self.expr(key, sink, scope);
                    }
                    self.expr(&item.value, sink, scope);
                }
            }
            Expr::BoolOp(op) => {
                let sink = sink.clone().with_operand();
                for value in &op.values {
                    self.expr(value, &sink, scope);
                }
            }
            Expr::Named(named) => {
                let mut sink = sink.clone();
                if let Expr::Name(target) = &*named.target {
                    sink.binds.push(self.bound(target.id.as_str()));
                }
                self.expr(&named.value, &sink, scope);
            }
            Expr::BinOp(op) => {
                let sink = sink.part().with_operand();
                self.expr(&op.left, &sink, scope);
                self.expr(&op.right, &sink, scope);
            }
            Expr::UnaryOp(op) => self.expr(&op.operand, &Sink::operand(), scope),
            Expr::Lambda(lambda) => {
                let id = lambda.start();
                if let Some(parameters) = &lambda.parameters {
                    self.parameters(parameters, id, scope);
                }
                let inner = Lookup {
                    class: None,
                    ..scope
                };
                let locals = function_locals(lambda.parameters.as_deref(), &[]);
                self.nested(id, locals, |this| {
                    this.expr(&lambda.body, &Sink::escape(), inner);
                });
            }
            Expr::If(op) => {
                self.expr(&op.test, &Sink::operand(), scope);
                self.expr(&op.body, sink, scope);
                self.expr(&op.orelse, sink, scope);
            }
            Expr::ListComp(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::SetComp(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::Generator(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::DictComp(comp) => {
                self.generators(&comp.generators, scope);
                if let Some(key) = &comp.key {
                    self.expr(key, sink, scope);
                }
                self.expr(&comp.value, sink, scope);
            }
            Expr::Await(op) => self.expr(&op.value, &Sink::escape(), scope),
            Expr::Yield(op) => {
                if let Some(value) = &op.value {
                    self.expr(value, &Sink::escape(), scope);
                }
            }
            Expr::YieldFrom(op) => self.expr(&op.value, &Sink::escape(), scope),
            Expr::Compare(op) => {
                self.expr(&op.left, &Sink::operand(), scope);
                for (cmp, comparator) in op.ops.iter().zip(&*op.comparators) {
                    // `"m" in sys.modules` only asks.
                    if matches!(cmp, ast::CmpOp::In | ast::CmpOp::NotIn)
                        && is_sys_modules(comparator, scope)
                    {
                        continue;
                    }
                    self.expr(comparator, &Sink::operand(), scope);
                }
            }
            Expr::Call(call) => self.call(call, sink, scope),
            Expr::FString(_) => {
                for part in children(expr) {
                    self.expr(part, &Sink::operand(), scope);
                }
            }
            // A template keeps the values it interpolates.
            Expr::TString(_) => {
                let sink = sink.part().with_operand();
                for part in children(expr) {
                    self.expr(part, &sink, scope);
                }
            }
            Expr::Slice(slice) => {
                for part in [&slice.lower, &slice.upper, &slice.step]
                    .into_iter()
                    .flatten()
                {
                    self.expr(part, &Sink::operand(), scope);
                }
            }
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::IpyEscapeCommand(_) => {}
        }
    }

    fn generators(&mut self, generators: &'a [ast::Comprehension], scope: Lookup<'_, 'a>) {
        for generator in generators {
            let sink = self
                .binding_sink(std::slice::from_ref(&generator.target))
                .with_operand();
            self.expr(&generator.iter, &sink, scope);
            self.store(&generator.target, scope, false);
            for condition in &generator.ifs {
                self.expr(condition, &Sink::operand(), scope);
            }
        }
    }

    /// `owner.attr`, read.
    fn attribute(&mut self, attr: &'a ExprAttribute, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        let name = attr.attr.id.as_str();
        if is_frame_attribute(name) {
            // A frame's or a function's globals: a module's namespace.
            self.facts().reflective = true;
        } else if name == "__builtins__" {
            self.facts().patched_paths.push("builtins".to_string());
        } else if may_be_reflective(name)
            && scope
                .path(&attr.value)
                .is_some_and(|owner| is_reflective(&format!("{owner}.{name}")))
        {
            self.facts().reflective = true;
        }
        // A class attribute read through its class (`TestK.CASES`).
        self.flow(Key::Attr(name), sink);
        // A method kept for later (`add = CASES.append`) may change its
        // owner when called; the names every class and function has are
        // strings.
        let string = matches!(name, "__name__" | "__qualname__" | "__module__" | "__doc__");
        let owner = if sink.held() && (MUTATORS.contains(&name) || (is_dunder(name) && !string)) {
            Sink::escape()
        } else {
            Sink::operand()
        };
        self.expr(&attr.value, &owner, scope);
    }

    /// `sys.modules[…]`, read or written: the module a literal names may
    /// change (and may be this one); a computed name may be any.
    fn module_lookup(&mut self, sub: &'a ast::ExprSubscript, scope: Lookup<'_, 'a>) -> bool {
        if !is_sys_modules(&sub.value, scope) {
            return false;
        }
        match &*sub.slice {
            Expr::StringLiteral(name) => self.module_named(name.value.to_str()),
            slice => {
                self.expr(slice, &Sink::operand(), scope);
                self.facts().reflective = true;
            }
        }
        true
    }

    /// A module looked up by a literal name: it may change, and it may be
    /// the analyzed module itself.
    fn module_named(&mut self, name: &str) {
        let facts = self.facts();
        facts.modules.push(name.to_string());
        facts.patched_paths.push(name.to_string());
        // `__import__("a.b")` returns `a`.
        if let Some((head, _)) = name.split_once('.') {
            facts.patched_paths.push(head.to_string());
        }
    }

    /// Calls that look modules up by name: `sys.modules.get("m")`,
    /// `importlib.import_module("m")`, `__import__("m")`, and
    /// `patch.dict(sys.modules, …)`, which only adds and removes entries.
    /// `true` when handled.
    fn module_call(&mut self, call: &'a ExprCall, scope: Lookup<'_, 'a>) -> bool {
        let (method, owner) = match &*call.func {
            Expr::Attribute(attr) => (attr.attr.id.as_str(), Some(&*attr.value)),
            Expr::Name(name) => (name.id.as_str(), None),
            _ => return false,
        };
        let args = &call.arguments;
        let literal = match args.args.first() {
            Some(Expr::StringLiteral(name)) => Some(name.value.to_str()),
            _ => None,
        };
        let rest = |this: &mut Self| {
            for arg in args.args.iter().skip(1) {
                this.expr(arg, &Sink::escape(), scope);
            }
            for keyword in &args.keywords {
                this.expr(&keyword.value, &Sink::escape(), scope);
            }
        };
        match method {
            "get" | "pop" | "setdefault" if owner.is_some_and(|o| is_sys_modules(o, scope)) => {
                match literal {
                    Some(name) => self.module_named(name),
                    None => self.facts().reflective = true,
                }
                rest(self);
                true
            }
            "import_module" | "__import__" => {
                let path = scope.path(&call.func);
                if !path.as_deref().is_some_and(|p| {
                    matches!(
                        p,
                        "importlib.import_module"
                            | "importlib.__import__"
                            | "builtins.__import__"
                            | "__import__"
                    )
                }) {
                    return false;
                }
                let Some(name) = literal else {
                    self.facts().reflective = true;
                    return false;
                };
                self.module_named(name);
                rest(self);
                true
            }
            "dict"
                if args.args.first().is_some_and(|a| is_sys_modules(a, scope))
                    && scope
                        .path(&call.func)
                        .is_some_and(|p| p.ends_with("patch.dict")) =>
            {
                rest(self);
                true
            }
            _ => false,
        }
    }

    fn call(&mut self, call: &'a ExprCall, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        if self.module_call(call, scope) {
            return;
        }
        let path = known_path(&call.func, scope);
        let arguments = &call.arguments;
        match path.as_deref() {
            // The namespace itself: changing it binds names.
            Some("builtins.globals" | "builtins.locals" | "builtins.vars")
                if arguments.is_empty() =>
            {
                if sink.held() {
                    self.facts().dynamic = true;
                }
            }
            Some("builtins.exec" | "builtins.eval") => self.facts().dynamic = true,
            Some("builtins.setattr" | "builtins.delattr") => self.setattr(arguments, scope),
            _ => {}
        }
        let callee = callee(call, path.as_deref());
        match &*call.func {
            Expr::Name(name) => self.load(name.id.as_str(), &Sink::inert(), scope),
            // A method of a value.
            Expr::Attribute(attr) if path.is_none() => {
                let owner = if PURE_METHODS.contains(&attr.attr.id.as_str()) {
                    sink.part().with_operand()
                } else {
                    Sink::escape()
                };
                self.expr(&attr.value, &owner, scope);
            }
            func @ Expr::Attribute(_) => self.expr(func, &Sink::inert(), scope),
            // A computed callee (`getattr(x, "append")(…)`) may be a method
            // of anything it was computed from.
            func => self.expr(func, &Sink::escape(), scope),
        }
        let sink = match callee {
            Callee::Inert => Sink::inert(),
            Callee::Operand => Sink::operand(),
            Callee::Flow => sink.part().with_operand(),
            Callee::Unknown => Sink::escape(),
        };
        // `super(K, obj)` only reads `K`'s MRO; what it calls gets `obj`.
        if path.as_deref() == Some("builtins.super")
            && let [first, rest @ ..] = &*arguments.args
            && arguments.keywords.is_empty()
        {
            self.expr(first, &Sink::operand(), scope);
            for arg in rest {
                self.expr(arg, &sink, scope);
            }
            return;
        }
        self.arguments(arguments, &sink, scope);
    }

    /// `setattr(owner, "attr", value)` and `delattr(owner, "attr")`.
    fn setattr(&mut self, arguments: &'a ast::Arguments, scope: Lookup<'_, 'a>) {
        let attr = match arguments.args.get(1) {
            Some(Expr::StringLiteral(s)) => Some(s.value.to_str()),
            _ => None,
        };
        if let (Some(Expr::Name(owner)), Some(attr)) = (arguments.args.first(), attr) {
            self.facts().attributes.push(attr);
            if attr == "__name__" {
                let keys = self.keys(owner.id.as_str(), scope);
                self.facts().renamed.extend(keys.into_iter().flatten());
            }
        } else {
            let facts = self.facts();
            facts.dynamic = true;
            facts.renamed_unknown = true;
        }
    }

    fn pattern(&mut self, pattern: &'a Pattern, scope: Lookup<'_, 'a>) {
        match pattern {
            Pattern::MatchValue(p) => self.expr(&p.value, &Sink::operand(), scope),
            Pattern::MatchSingleton(_) | Pattern::MatchStar(_) => {}
            Pattern::MatchSequence(p) => {
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchMapping(p) => {
                for key in &p.keys {
                    self.expr(key, &Sink::operand(), scope);
                }
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchClass(p) => {
                self.expr(&p.cls, &Sink::operand(), scope);
                for inner in &p.arguments.patterns {
                    self.pattern(inner, scope);
                }
                for keyword in &p.arguments.keywords {
                    self.pattern(&keyword.pattern, scope);
                }
            }
            Pattern::MatchAs(p) => {
                if let Some(inner) = &p.pattern {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchOr(p) => {
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
        }
    }

    /// What the bodies that may run do to names.
    fn finish(self) -> Found<'a> {
        let running: Vec<&Unit<'a>> = self.units.iter().filter(|unit| unit.runs).collect();

        let mut groups = Groups::default();
        for unit in &running {
            for (a, b) in &unit.facts.unions {
                groups.union(*a, *b);
            }
        }
        // Classes bound in class bodies are attributes too (`K.Inner`).
        for class in &self.classes {
            if matches!(class.key, Key::Class(..)) {
                groups.root(class.key);
            }
        }
        // A value bound in a class body is reachable as an attribute of
        // anything (`H.x`, `cls.x`, `self.x`, `P().x`).
        let members: Vec<Key<'a>> = groups
            .keys
            .iter()
            .copied()
            .filter(|key| matches!(key, Key::Class(..)))
            .collect();
        for key in &members {
            if let Key::Class(_, name) = key {
                groups.union(*key, Key::Attr(name));
            }
        }
        // Locals no union ties to another name (most of them) stay out of
        // the groups: what happens to them stays with them.
        let lonely = |groups: &Groups<'a>, key: &Key<'a>| {
            matches!(key, Key::Local(..)) && !groups.index.contains_key(key)
        };
        // Groups changed by code aye can't follow, and the names of the
        // analyzed body its statements change, by group.
        let mut changed: FastSet<usize> = FastSet::default();
        let mut changed_alone: FastSet<Key<'a>> = FastSet::default();
        // The same, without what classes' own statements hand on, and with
        // what has attributes set by code recognition doesn't read.
        let mut strict: FastSet<usize> = FastSet::default();
        let mut strict_alone: FastSet<Key<'a>> = FastSet::default();
        let mut tracked: FastMap<usize, FastSet<Key<'a>>> = FastMap::default();
        for unit in &running {
            for key in &unit.facts.escapes {
                if lonely(&groups, key) {
                    changed_alone.insert(*key);
                    strict_alone.insert(*key);
                } else {
                    changed.insert(groups.root(*key));
                    strict.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.defined {
                if lonely(&groups, key) {
                    changed_alone.insert(*key);
                } else {
                    changed.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.touched {
                if lonely(&groups, key) {
                    strict_alone.insert(*key);
                } else {
                    strict.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.tracked {
                tracked.entry(groups.root(*key)).or_default().insert(*key);
            }
        }
        // `NAME.attr = …` statements, by group: through another name of
        // the group (an alias, a loop variable), the attribute is set on
        // whatever it holds.
        let mut direct: FastMap<usize, FastSet<Key<'a>>> = FastMap::default();
        for unit in &running {
            for key in &unit.facts.direct {
                direct.entry(groups.root(*key)).or_default().insert(*key);
            }
        }
        // A class handed to code aye can't follow (`vars(H)`, `getattr(H,
        // …)`, a decorator) takes what its body binds along.
        let handed = |groups: &mut Groups<'a>, changed: &FastSet<usize>, key: Key<'a>| {
            if lonely(groups, &key) {
                changed_alone.contains(&key)
            } else {
                changed.contains(&groups.root(key))
            }
        };
        let mut more = true;
        while more {
            more = false;
            for key in &members {
                let Key::Class(class, _) = key else {
                    continue;
                };
                let class = &self.classes[*class];
                if self.units[class.unit].runs && handed(&mut groups, &changed, class.key) {
                    more |= changed.insert(groups.root(*key));
                }
            }
        }
        // A dunder method Python calls with operands, which lets one of them
        // escape: every operand may.
        let hostile = running
            .iter()
            .flat_map(|unit| {
                let id = unit.id.unwrap_or_default();
                unit.operand_params.iter().map(move |p| Key::Local(id, p))
            })
            .any(|param| {
                if lonely(&groups, &param) {
                    return changed_alone.contains(&param);
                }
                let root = groups.root(param);
                changed.contains(&root) || tracked.contains_key(&root)
            });
        if hostile {
            for unit in &running {
                for key in &unit.facts.operands {
                    if !lonely(&groups, key) {
                        changed.insert(groups.root(*key));
                        strict.insert(groups.root(*key));
                    }
                }
            }
        }
        let handed_key = |groups: &mut Groups<'a>, key: Key<'a>| {
            if lonely(groups, &key) {
                return strict_alone.contains(&key);
            }
            let root = groups.root(key);
            strict.contains(&root)
                || direct
                    .get(&root)
                    .is_some_and(|keys| keys.iter().any(|k| *k != key))
        };
        let mut escaped = Escaped::default();
        let mut renamed: FastSet<usize> = FastSet::default();
        for unit in &running {
            for key in &unit.facts.renamed {
                match key {
                    Key::Local(_, name) if lonely(&groups, key) => {
                        escaped.renamed.insert(name);
                    }
                    _ => {
                        renamed.insert(groups.root(*key));
                    }
                }
            }
        }

        let origins: Vec<TextSize> = self.classes.iter().map(|c| c.origin).collect();
        let mut untracked = FastSet::default();
        for key in groups.keys.clone() {
            let root = groups.root(key);
            let may_change = changed.contains(&root) || tracked.contains_key(&root);
            let name = match key {
                Key::Global(name) => {
                    let by_others = tracked
                        .get(&root)
                        .is_some_and(|keys| keys.iter().any(|other| *other != key));
                    if changed.contains(&root) || by_others {
                        untracked.insert(name);
                    }
                    if may_change {
                        escaped.changed.insert(name);
                    }
                    name
                }
                Key::Class(class, name) => {
                    if may_change {
                        escaped
                            .classes
                            .entry(origins[class])
                            .or_default()
                            .insert(name);
                    }
                    name
                }
                Key::Attr(name) => {
                    if may_change {
                        escaped.attributes_changed.insert(name);
                    }
                    name
                }
                Key::Local(_, name) => name,
            };
            if renamed.contains(&root) {
                escaped.renamed.insert(name);
            }
        }
        // Every class walked gets an entry, so that its namespace knows the
        // module's analysis covered it.
        for origin in origins {
            escaped.classes.entry(origin).or_default();
        }
        // Modules whose attributes may change: a name bound to one is
        // handed to code aye can't follow, or has attributes stored to.
        let mut patched_roots: FastSet<usize> = FastSet::default();
        let mut patched_alone: FastSet<Key<'a>> = FastSet::default();
        for unit in &running {
            for key in &unit.facts.patched {
                if lonely(&groups, key) {
                    patched_alone.insert(*key);
                } else {
                    patched_roots.insert(groups.root(*key));
                }
            }
        }
        for unit in &running {
            for (key, path) in &unit.facts.imports {
                let hit = if lonely(&groups, key) {
                    changed_alone.contains(key) || patched_alone.contains(key)
                } else {
                    let root = groups.root(*key);
                    changed.contains(&root)
                        || patched_roots.contains(&root)
                        || tracked.contains_key(&root)
                };
                if hit {
                    escaped.patched.insert(path.clone());
                }
            }
            escaped
                .patched
                .extend(unit.facts.patched_paths.iter().cloned());
            escaped.modules.extend(unit.facts.modules.iter().cloned());
        }
        let reflective = running.iter().any(|u| u.facts.reflective);
        escaped.patches_all = reflective;
        // Classes handed to code aye can't follow: any attribute may change.
        for class in &self.classes {
            if self.units[class.unit].runs && handed(&mut groups, &changed, class.key) {
                escaped.classes_all.insert(class.origin);
            }
            if self.units[class.unit].runs && handed_key(&mut groups, class.key) {
                escaped.handed_classes.insert(class.origin);
            }
        }
        for key in groups.keys.clone() {
            match key {
                Key::Global(name) if handed_key(&mut groups, key) => {
                    escaped.handed.insert(name);
                }
                Key::Attr(name) if handed_key(&mut groups, key) => {
                    escaped.handed_attributes.insert(name);
                }
                _ => {}
            }
        }
        escaped.global_writes = running
            .iter()
            .flat_map(|u| u.facts.global_writes.iter().copied())
            .collect();
        escaped.renamed_unknown = running.iter().any(|u| u.facts.renamed_unknown);
        escaped.attributes = running
            .iter()
            .flat_map(|u| u.facts.attributes.iter().copied())
            .collect();
        Found {
            escaped,
            untracked,
            dynamic: running.iter().any(|u| u.facts.dynamic),
            reflective,
        }
    }
}

/// A function's locals: its parameters and the names its body binds, but
/// not those its own `global` and `nonlocal` statements declare.
fn function_locals<'a>(parameters: Option<&'a Parameters>, body: &'a [Stmt]) -> FastSet<&'a str> {
    fn targets<'a>(target: &'a Expr, out: &mut FastSet<&'a str>) {
        match target {
            Expr::Name(name) => {
                out.insert(name.id.as_str());
            }
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| targets(e, out)),
            Expr::List(list) => list.elts.iter().for_each(|e| targets(e, out)),
            Expr::Starred(starred) => targets(&starred.value, out),
            _ => {}
        }
    }
    fn walk<'a>(body: &'a [Stmt], out: &mut FastSet<&'a str>, declared: &mut FastSet<&'a str>) {
        for stmt in body {
            let mut walruses = Vec::new();
            for (expr, _) in evaluated(stmt) {
                walrus_targets(expr, false, &mut walruses);
            }
            for (target, _) in walruses {
                targets(target, out);
            }
            match stmt {
                Stmt::FunctionDef(def) => {
                    out.insert(def.name.id.as_str());
                }
                Stmt::ClassDef(class) => {
                    out.insert(class.name.id.as_str());
                }
                Stmt::Assign(s) => s.targets.iter().for_each(|t| targets(t, out)),
                Stmt::AnnAssign(s) => targets(&s.target, out),
                Stmt::AugAssign(s) => targets(&s.target, out),
                Stmt::Delete(s) => s.targets.iter().for_each(|t| targets(t, out)),
                Stmt::TypeAlias(s) => targets(&s.name, out),
                Stmt::Import(s) => {
                    for alias in &s.names {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).id.as_str();
                        out.insert(local.split('.').next().unwrap_or(local));
                    }
                }
                Stmt::ImportFrom(s) => {
                    for alias in &s.names {
                        out.insert(alias.asname.as_ref().unwrap_or(&alias.name).id.as_str());
                    }
                }
                Stmt::Global(s) => declared.extend(s.names.iter().map(|n| n.id.as_str())),
                Stmt::Nonlocal(s) => declared.extend(s.names.iter().map(|n| n.id.as_str())),
                Stmt::For(s) => {
                    targets(&s.target, out);
                    walk(&s.body, out, declared);
                    walk(&s.orelse, out, declared);
                }
                Stmt::While(s) => {
                    walk(&s.body, out, declared);
                    walk(&s.orelse, out, declared);
                }
                Stmt::If(s) => {
                    walk(&s.body, out, declared);
                    for clause in &s.elif_else_clauses {
                        walk(&clause.body, out, declared);
                    }
                }
                Stmt::With(s) => {
                    for item in &s.items {
                        if let Some(vars) = &item.optional_vars {
                            targets(vars, out);
                        }
                    }
                    walk(&s.body, out, declared);
                }
                Stmt::Try(s) => {
                    walk(&s.body, out, declared);
                    for handler in &s.handlers {
                        if let Some(handler) = handler.as_except_handler() {
                            if let Some(name) = &handler.name {
                                out.insert(name.id.as_str());
                            }
                            walk(&handler.body, out, declared);
                        }
                    }
                    walk(&s.orelse, out, declared);
                    walk(&s.finalbody, out, declared);
                }
                Stmt::Match(s) => {
                    for case in &s.cases {
                        let mut captures = Vec::new();
                        pattern_captures(&case.pattern, &mut captures);
                        out.extend(captures.iter().map(|n| n.id.as_str()));
                        walk(&case.body, out, declared);
                    }
                }
                _ => {}
            }
        }
    }

    let mut locals = FastSet::default();
    let mut declared = FastSet::default();
    walk(body, &mut locals, &mut declared);
    for param in parameters.into_iter().flatten() {
        locals.insert(param.as_parameter().name.id.as_str());
    }
    locals.retain(|name| !declared.contains(name));
    locals
}

/// How a call treats its arguments.
fn callee(call: &ExprCall, path: Option<&str>) -> Callee {
    if let Expr::Attribute(attr) = &*call.func {
        // `str` and `bytes` methods keep and change none of their arguments.
        if matches!(
            &*attr.value,
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::FString(_)
        ) {
            return Callee::Operand;
        }
        if path.is_none() {
            return if PURE_METHODS.contains(&attr.attr.id.as_str()) {
                Callee::Operand
            } else {
                Callee::Unknown
            };
        }
    }
    match path {
        Some(p)
            if p.starts_with("pytest.mark.") || matches!(p, "pytest.param" | "pytest.fixture") =>
        {
            Callee::Inert
        }
        Some(p) if READ_BUILTINS.contains(&p) => Callee::Operand,
        Some(p) if FLOW_BUILTINS.contains(&p) && call.arguments.keywords.is_empty() => Callee::Flow,
        _ => Callee::Unknown,
    }
}

// ---------------------------------------------------------------- resolution

/// Callables whose results have no id of their own: not strings, bytes,
/// numbers, booleans, patterns or enums, and no `__name__`. pytest falls
/// back to `argname + index` for them.
const NO_ID: &[&str] = &[
    "builtins.object",
    "builtins.dict",
    "builtins.list",
    "builtins.set",
    "builtins.frozenset",
    "builtins.tuple",
    "builtins.bytearray",
    "builtins.slice",
    "builtins.range",
    "collections.Counter",
    "collections.OrderedDict",
    "collections.defaultdict",
    "collections.deque",
    "datetime.date",
    "datetime.datetime",
    "datetime.time",
    "datetime.timedelta",
    "datetime.timezone",
    "decimal.Decimal",
    "fractions.Fraction",
    "pathlib.Path",
    "pathlib.PosixPath",
    "pathlib.PurePath",
    "pathlib.PurePosixPath",
    "pathlib.PureWindowsPath",
    "pathlib.WindowsPath",
    "uuid.UUID",
    "numpy.arange",
    "numpy.array",
    "numpy.asarray",
    "numpy.empty",
    "numpy.full",
    "numpy.linspace",
    "numpy.ones",
    "numpy.zeros",
    "pandas.Categorical",
    "pandas.DataFrame",
    "pandas.DatetimeIndex",
    "pandas.Index",
    "pandas.Interval",
    "pandas.MultiIndex",
    "pandas.Period",
    "pandas.RangeIndex",
    "pandas.Series",
    "pandas.Timedelta",
    "pandas.Timestamp",
    "pandas.array",
];

/// Python builtins that have a `__name__`, with that name: those of every
/// Python pytest 9.1.1 supports (3.10 to 3.14). `ExceptionGroup` and
/// `BaseExceptionGroup` (3.11) and `PythonFinalizationError` (3.13) are
/// left out: whether they exist depends on the version.
fn builtin_name(name: &str) -> Option<&'static str> {
    const BUILTINS: &[&str] = &[
        "ArithmeticError",
        "AssertionError",
        "AttributeError",
        "BaseException",
        "BlockingIOError",
        "BrokenPipeError",
        "BufferError",
        "BytesWarning",
        "ChildProcessError",
        "ConnectionAbortedError",
        "ConnectionError",
        "ConnectionRefusedError",
        "ConnectionResetError",
        "DeprecationWarning",
        "EOFError",
        "EncodingWarning",
        "Exception",
        "FileExistsError",
        "FileNotFoundError",
        "FloatingPointError",
        "FutureWarning",
        "GeneratorExit",
        "ImportError",
        "ImportWarning",
        "IndentationError",
        "IndexError",
        "InterruptedError",
        "IsADirectoryError",
        "KeyError",
        "KeyboardInterrupt",
        "LookupError",
        "MemoryError",
        "ModuleNotFoundError",
        "NameError",
        "NotADirectoryError",
        "NotImplementedError",
        "OSError",
        "OverflowError",
        "PendingDeprecationWarning",
        "PermissionError",
        "ProcessLookupError",
        "RecursionError",
        "ReferenceError",
        "ResourceWarning",
        "RuntimeError",
        "RuntimeWarning",
        "StopAsyncIteration",
        "StopIteration",
        "SyntaxError",
        "SyntaxWarning",
        "SystemError",
        "SystemExit",
        "TabError",
        "TimeoutError",
        "TypeError",
        "UnboundLocalError",
        "UnicodeDecodeError",
        "UnicodeEncodeError",
        "UnicodeError",
        "UnicodeTranslateError",
        "UnicodeWarning",
        "UserWarning",
        "ValueError",
        "Warning",
        "ZeroDivisionError",
        "abs",
        "aiter",
        "all",
        "anext",
        "any",
        "ascii",
        "bin",
        "bool",
        "breakpoint",
        "bytearray",
        "bytes",
        "callable",
        "chr",
        "classmethod",
        "compile",
        "complex",
        "delattr",
        "dict",
        "dir",
        "divmod",
        "enumerate",
        "eval",
        "exec",
        "filter",
        "float",
        "format",
        "frozenset",
        "getattr",
        "globals",
        "hasattr",
        "hash",
        "hex",
        "id",
        "input",
        "int",
        "isinstance",
        "issubclass",
        "iter",
        "len",
        "list",
        "locals",
        "map",
        "max",
        "memoryview",
        "min",
        "next",
        "object",
        "oct",
        "open",
        "ord",
        "pow",
        "print",
        "property",
        "range",
        "repr",
        "reversed",
        "round",
        "set",
        "setattr",
        "slice",
        "sorted",
        "staticmethod",
        "str",
        "sum",
        "super",
        "tuple",
        "type",
        "vars",
        "zip",
    ];
    match name {
        "EnvironmentError" | "IOError" => Some("OSError"),
        _ => BUILTINS.iter().find(|b| **b == name).copied(),
    }
}

/// What an expression refers to.
#[derive(Debug, Clone)]
pub(crate) enum Resolved<'a> {
    /// A dotted path through imports or builtins (`pytest.mark.slow`,
    /// `builtins.int`). Unbound names resolve to themselves.
    Path(String),
    /// A definition or value in this file.
    Local(Bound<'a>),
    Unknown,
}

/// The namespaces a name is looked up in at some point of the source: the
/// class body being run, if any, then the module. (Class bodies do not
/// nest for lookup.)
#[derive(Clone, Copy)]
pub(crate) struct Lookup<'s, 'a> {
    pub module: &'s Namespace<'a>,
    pub class: Option<&'s Namespace<'a>>,
    pub source: &'a str,
    /// Where the expression is evaluated.
    pub at: TextSize,
}

/// Why a value could not be known statically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unknown;

/// One element of a statically known sequence: an expression and where to
/// evaluate it, or a value already known.
#[derive(Clone)]
pub(crate) enum Elem<'s, 'a> {
    Expr(&'a Expr, Lookup<'s, 'a>),
    Value(Value),
}

const DEPTH: u8 = 8;

impl<'s, 'a> Lookup<'s, 'a> {
    pub(crate) fn at(self, at: TextSize) -> Self {
        Self { at, ..self }
    }

    fn scopes(&self) -> impl Iterator<Item = &'s Namespace<'a>> {
        self.class.into_iter().chain(std::iter::once(self.module))
    }

    /// The binding a name has at `self.at`, if it is bound in the file.
    pub(crate) fn binding(&self, name: &str) -> Option<Bound<'a>> {
        for ns in self.scopes() {
            if let Some(bound) = ns.get(name) {
                return Some(bound.clone());
            }
        }
        None
    }

    /// Whether a name may come from a star import.
    pub(crate) fn maybe_star(&self) -> bool {
        self.scopes().any(|ns| !ns.star_imports.is_empty())
    }

    /// Resolve a name or attribute chain.
    pub(crate) fn resolve(&self, expr: &'a Expr) -> Resolved<'a> {
        self.resolve_depth(expr, 0)
    }

    fn resolve_depth(&self, expr: &'a Expr, depth: u8) -> Resolved<'a> {
        if depth > DEPTH {
            return Resolved::Unknown;
        }
        match expr {
            Expr::Name(name) => {
                let id = name.id.as_str();
                match self.binding(id) {
                    Some(Bound {
                        binding: Binding::Import(path),
                        ..
                    }) => Resolved::Path(path),
                    Some(
                        bound @ Bound {
                            binding: Binding::Value(value),
                            ..
                        },
                    ) => match value {
                        Expr::Name(_) | Expr::Attribute(_) => {
                            match self.resolve_depth(value, depth + 1) {
                                Resolved::Unknown => Resolved::Local(bound),
                                other => other,
                            }
                        }
                        _ => Resolved::Local(bound),
                    },
                    Some(bound) => Resolved::Local(bound),
                    None if builtin_name(id).is_some() => Resolved::Path(format!("builtins.{id}")),
                    None if self.maybe_star() => Resolved::Unknown,
                    None => Resolved::Path(id.to_string()),
                }
            }
            Expr::Attribute(attr) => match self.resolve_depth(&attr.value, depth + 1) {
                Resolved::Path(path) => Resolved::Path(format!("{path}.{}", attr.attr.id)),
                _ => Resolved::Unknown,
            },
            _ => Resolved::Unknown,
        }
    }

    /// The dotted path of an expression, if it names something imported.
    pub(crate) fn path(&self, expr: &'a Expr) -> Option<String> {
        match self.resolve(expr) {
            Resolved::Path(path) => Some(path),
            _ => None,
        }
    }

    /// [`Lookup::path`] for evaluating values: `None` when what the path
    /// names may not be what its module defines, because code that runs
    /// may change the module (or one above it, or `builtins`), or a
    /// function may rebind the name (`global`).
    fn trusted_path(&self, expr: &'a Expr) -> Option<String> {
        let path = self.path(expr)?;
        let mut root = expr;
        while let Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        if let Expr::Name(name) = root
            && self
                .scopes()
                .any(|ns| ns.globals.contains(name.id.as_str()))
        {
            return None;
        }
        (!self.module.untrusted(&path)).then_some(path)
    }

    /// The value bound to a name at `self.at`, when it is a plain constant:
    /// bound unconditionally before this point, last by an assignment, not
    /// deleted since, and never changed (the module's escape analysis finds
    /// nothing that may change it, unless the value can't change).
    pub(crate) fn constant(&self, name: &str) -> Option<(&'a Expr, Lookup<'s, 'a>)> {
        for (level, ns) in self.scopes().enumerate() {
            let in_class = level == 0 && self.class.is_some();
            let bindings = ns.bindings(name);
            if bindings.is_empty() {
                continue;
            }
            let before: Vec<&Bound<'a>> = bindings
                .iter()
                .filter(|b| b.range.end() <= self.at)
                .collect();
            let Some(last) = before.last() else {
                // Bound in this namespace, but only later: Python looks
                // further out in a class body, and fails in a module.
                if in_class {
                    continue;
                }
                return None;
            };
            match ns.deleted(name, last.range.end(), self.at) {
                None => {}
                // Unbound again: in a class body, the module's binding (or
                // the builtin) shows through.
                Some(false) if in_class => continue,
                Some(_) => return None,
            }
            if before.iter().any(|b| b.conditional) || ns.changed(name) {
                return None;
            }
            let Binding::Value(expr) = last.binding else {
                return None;
            };
            let scope = if in_class {
                *self
            } else {
                Lookup {
                    class: None,
                    ..*self
                }
            };
            let found = &self.module.escapes;
            let (changed, rebound) = if in_class {
                (
                    self.class_changed(ns, name) || found.attributes_changed.contains(name),
                    found.attributes.contains(name),
                )
            } else {
                (found.changed.contains(name), false)
            };
            if rebound
                || found.renamed.contains(name)
                || (found.renamed_unknown && matches!(expr, Expr::Lambda(_)))
                || (changed && !scope.immutable(expr))
            {
                return None;
            }
            return Some((expr, scope.at(last.range.start())));
        }
        None
    }

    /// Whether the escape analysis says a name of a class body may change:
    /// the module's, which walked the class body knowing the module's
    /// names, or else the class body's own.
    fn class_changed(&self, class: &Namespace<'a>, name: &str) -> bool {
        let found = &self.module.escapes;
        if class
            .origin
            .is_some_and(|at| found.classes_all.contains(&at))
        {
            return true;
        }
        match class.origin.and_then(|at| found.classes.get(&at)) {
            Some(names) => names.contains(name),
            None => class.escapes.changed.contains(name),
        }
    }

    /// Whether the value an expression makes can't change once made (its
    /// names aside, which are looked up again): strings, numbers, tuples of
    /// those, `range(…)`, marks.
    fn immutable(&self, expr: &'a Expr) -> bool {
        match expr {
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::FString(_) => true,
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .all(|elt| matches!(elt, Expr::Name(_)) || self.immutable(elt)),
            Expr::UnaryOp(op) => self.immutable(&op.operand),
            Expr::BinOp(op) => self.immutable(&op.left) && self.immutable(&op.right),
            Expr::Call(call) => matches!(
                self.trusted_path(&call.func).as_deref(),
                Some("builtins.range" | "builtins.float" | "re.compile")
            ),
            Expr::Attribute(_) => self
                .path(expr)
                .is_some_and(|p| p.starts_with("pytest.mark.")),
            _ => false,
        }
    }

    /// Whether the `__name__` of what `name` refers to may differ from its
    /// definition's: reassigned, or handed to code aye can't follow.
    fn renamed(&self, name: &str) -> bool {
        let found = &self.module.escapes;
        found.renamed_unknown
            || found.renamed.contains(name)
            || found.changed.contains(name)
            || self
                .class
                .is_some_and(|class| self.class_changed(class, name))
            || self.scopes().any(|ns| {
                ns.dynamic
                    || ns
                        .attributes
                        .get(name)
                        .is_some_and(|attrs| attrs.contains(&"__name__"))
            })
    }

    /// Evaluate an expression to a value pytest can make an id from.
    pub(crate) fn value(&self, expr: &'a Expr) -> Result<Value, Unknown> {
        self.value_depth(expr, 0)
    }

    fn value_depth(&self, expr: &'a Expr, depth: u8) -> Result<Value, Unknown> {
        if depth > DEPTH {
            return Err(Unknown);
        }
        match expr {
            Expr::StringLiteral(s) => literal_text(s.value.to_str().to_string()),
            Expr::BytesLiteral(b) => Ok(Value::Bytes(b.value.bytes().collect())),
            Expr::NumberLiteral(n) => Ok(match &n.value {
                Number::Int(int) => {
                    let digits = match int.as_u64() {
                        Some(small) => small.to_string(),
                        None => int_literal_decimal(&self.source[n.range()]).ok_or(Unknown)?,
                    };
                    // `str()` refuses longer ints when the limit is
                    // lowered (`sys.set_int_max_str_digits`, 4300 by
                    // default), and Python refuses decimal literals beyond
                    // it.
                    if digits.len() > MAX_INT_DIGITS {
                        return Err(Unknown);
                    }
                    Value::Int(digits)
                }
                Number::Float(f) => Value::Float(*f),
                Number::Complex { real, imag } => Value::Complex(*real, *imag),
            }),
            Expr::BooleanLiteral(b) => Ok(Value::Bool(b.value)),
            Expr::NoneLiteral(_) => Ok(Value::None),
            // Containers have no id of their own, but building one may
            // raise (`[*1]`, `{[1]: 2}`): only those surely built are known.
            Expr::EllipsisLiteral(_) => Ok(Value::Opaque),
            Expr::List(list) => opaque(self.elements(&list.elts, false, depth)),
            Expr::Tuple(tuple) => opaque(self.elements(&tuple.elts, false, depth)),
            Expr::Set(set) => opaque(self.elements(&set.elts, true, depth)),
            Expr::Dict(dict) => opaque(self.mapping(dict, depth)),
            Expr::ListComp(comp) => {
                opaque(self.comprehension(&comp.elt, &comp.generators, false, depth))
            }
            Expr::SetComp(comp) => {
                opaque(self.comprehension(&comp.elt, &comp.generators, true, depth))
            }
            // `Expr::DictComp` is never read: see the default below.
            Expr::Lambda(lambda) => {
                // Its defaults are evaluated when it is made.
                let defaults_safe = lambda.parameters.as_deref().is_none_or(|parameters| {
                    parameters
                        .iter_non_variadic_params()
                        .filter_map(|p| p.default.as_deref())
                        .all(|default| self.safe(default, depth + 1))
                });
                if defaults_safe {
                    Ok(Value::Named("<lambda>".to_string()))
                } else {
                    Err(Unknown)
                }
            }
            // A generator object's `__name__`. Its first iterable is
            // iterated at once; the rest runs later.
            Expr::Generator(generator) => match generator.generators.first() {
                Some(first)
                    if !first.is_async && self.iterable(&first.iter, depth + 1).is_some() =>
                {
                    Ok(Value::Named("<genexpr>".to_string()))
                }
                _ => Err(Unknown),
            },
            Expr::FString(f) => {
                let mut text = String::new();
                for part in &f.value {
                    match part {
                        ast::FStringPart::Literal(lit) => text.push_str(lit.as_str()),
                        ast::FStringPart::FString(fstring) => {
                            for element in &fstring.elements {
                                match element.as_literal() {
                                    Some(lit) => text.push_str(&lit.value),
                                    None => return Err(Unknown),
                                }
                            }
                        }
                    }
                }
                literal_text(text)
            }
            Expr::UnaryOp(op) => {
                let operand = self.value_depth(&op.operand, depth + 1)?;
                match (op.op, operand) {
                    (UnaryOp::UAdd, v @ (Value::Int(_) | Value::Float(_) | Value::Complex(..))) => {
                        Ok(v)
                    }
                    (UnaryOp::UAdd, Value::Bool(b)) => Ok(Value::Int(u8::from(b).to_string())),
                    (UnaryOp::USub, Value::Int(digits)) => Ok(Value::Int(negate(&digits))),
                    (UnaryOp::USub, Value::Bool(b)) => {
                        Ok(Value::Int(if b { "-1" } else { "0" }.to_string()))
                    }
                    (UnaryOp::USub, Value::Float(f)) => Ok(Value::Float(-f)),
                    (UnaryOp::USub, Value::Complex(re, im)) => Ok(Value::Complex(-re, -im)),
                    _ => Err(Unknown),
                }
            }
            Expr::BinOp(bin) => {
                let left = self.value_depth(&bin.left, depth + 1)?;
                let right = self.value_depth(&bin.right, depth + 1)?;
                match (bin.op, left, right) {
                    (Operator::Add, Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
                    (Operator::Add, Value::Bytes(mut a), Value::Bytes(b)) => {
                        a.extend(b);
                        Ok(Value::Bytes(a))
                    }
                    (op @ (Operator::Add | Operator::Sub), left, Value::Complex(re, im)) => {
                        // Python 3.14 adds a real number to a complex one
                        // part by part (C99 Annex G); earlier versions make
                        // it `complex(a, 0.0)` first. Only the sign of a
                        // zero imaginary part differs: `1 + -0j`, `1 - 0j`.
                        let add = matches!(op, Operator::Add);
                        if !matches!(left, Value::Complex(..))
                            && im == 0.0
                            && im.is_sign_negative() == add
                        {
                            return Err(Unknown);
                        }
                        let (a_re, a_im) = complex_parts(&left).ok_or(Unknown)?;
                        Ok(if matches!(op, Operator::Add) {
                            Value::Complex(a_re + re, a_im + im)
                        } else {
                            Value::Complex(a_re - re, a_im - im)
                        })
                    }
                    _ => Err(Unknown),
                }
            }
            Expr::Name(name) => {
                if let Some((value, scope)) = self.constant(name.id.as_str()) {
                    return scope.value_depth(value, depth + 1);
                }
                match self.resolve(expr) {
                    Resolved::Path(path) => match path.strip_prefix("builtins.") {
                        Some(builtin) if self.trusted_path(expr).is_some() => builtin_name(builtin)
                            .map(|n| Value::Named(n.to_string()))
                            .ok_or(Unknown),
                        _ => Err(Unknown),
                    },
                    Resolved::Local(bound) => self.named(&bound),
                    Resolved::Unknown => Err(Unknown),
                }
            }
            Expr::Call(call) => self.call_value(call, depth),
            // Module attributes (`math.pi`) are not read: any module of
            // the session may change them while it is imported (`math.pi
            // = 3` in another test module), which aye doesn't follow.
            _ => Err(Unknown),
        }
    }

    /// The `__name__` of a function or class defined in this file, when the
    /// name still refers to it.
    fn named(&self, bound: &Bound<'a>) -> Result<Value, Unknown> {
        if bound.range.end() > self.at || bound.conditional {
            return Err(Unknown);
        }
        let (name, decorators) = match bound.binding {
            Binding::Function(def) => (def.name.id.as_str(), &def.decorator_list),
            Binding::Class(class) => (class.name.id.as_str(), &class.decorator_list),
            _ => return Err(Unknown),
        };
        if self.renamed(name) {
            return Err(Unknown);
        }
        if decorators.iter().all(|d| {
            self.path(&d.expression)
                .is_some_and(|p| p.starts_with("pytest.mark."))
        }) {
            Ok(Value::Named(name.to_string()))
        } else {
            Err(Unknown)
        }
    }

    fn call_value(&self, call: &'a ExprCall, depth: u8) -> Result<Value, Unknown> {
        let path = self.trusted_path(&call.func).ok_or(Unknown)?;
        let args = &call.arguments;
        match path.as_str() {
            "re.compile" if !args.args.is_empty() && args.args.len() <= 2 => {
                if args.args.iter().any(Expr::is_starred_expr)
                    || args
                        .keywords
                        .iter()
                        .any(|k| k.arg.as_ref().is_none_or(|a| a.id.as_str() != "flags"))
                {
                    return Err(Unknown);
                }
                let flags: Vec<&Expr> = args.args[1..]
                    .iter()
                    .chain(args.keywords.iter().map(|k| &k.value))
                    .collect();
                if flags.len() > 1 || !flags.iter().all(|f| self.regex_flags(f)) {
                    return Err(Unknown);
                }
                match self.value_depth(&args.args[0], depth + 1)? {
                    pattern @ (Value::Str(_) | Value::Bytes(_)) if regex_compiles(&pattern) => {
                        Ok(Value::Pattern(Box::new(pattern)))
                    }
                    _ => Err(Unknown),
                }
            }
            "builtins.float" if args.args.len() == 1 && args.keywords.is_empty() => {
                match self.value_depth(&args.args[0], depth + 1)? {
                    Value::Str(text) => parse_float(&text).map(Value::Float).ok_or(Unknown),
                    Value::Float(f) => Ok(Value::Float(f)),
                    // An int too large for a float raises `OverflowError`.
                    Value::Int(digits) => match digits.parse::<f64>() {
                        Ok(f) if f.is_finite() => Ok(Value::Float(f)),
                        _ => Err(Unknown),
                    },
                    _ => Err(Unknown),
                }
            }
            _ if NO_ID.contains(&path.as_str()) => opaque(self.constructs(&path, call, depth)),
            _ => Err(Unknown),
        }
    }

    /// Whether evaluating `expr` here surely doesn't raise: aye knows its
    /// value, or it is a name surely bound by then, or an attribute of an
    /// imported module (which raises only if the module lacks it, as an
    /// import would).
    fn safe(&self, expr: &'a Expr, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        match expr {
            Expr::Starred(_) => false,
            Expr::Name(name) => {
                self.value_depth(expr, depth).is_ok() || self.bound_here(name.id.as_str())
            }
            Expr::Attribute(_) => {
                let mut root = expr;
                while let Expr::Attribute(attr) = root {
                    root = &attr.value;
                }
                matches!(root, Expr::Name(name)
                if self.bound_here(name.id.as_str())
                    && matches!(
                        self.binding(name.id.as_str()),
                        Some(Bound { binding: Binding::Import(_), .. })
                    ))
            }
            _ => self.value_depth(expr, depth).is_ok(),
        }
    }

    /// Whether `name` is surely bound where it is read (so reading it
    /// doesn't raise): bound unconditionally before, and never deleted
    /// since nor by a function; or a builtin.
    fn bound_here(&self, name: &str) -> bool {
        for (level, ns) in self.scopes().enumerate() {
            let in_class = level == 0 && self.class.is_some();
            let before: Vec<&Bound<'a>> = ns
                .bindings(name)
                .iter()
                .filter(|b| b.range.end() <= self.at)
                .collect();
            let Some(last) = before.last() else {
                if in_class || ns.bindings(name).is_empty() {
                    continue;
                }
                return false;
            };
            match ns.deleted(name, last.range.end(), self.at) {
                None => {}
                Some(false) if in_class => continue,
                Some(_) => return false,
            }
            if ns.dynamic || ns.globals.contains(name) {
                return false;
            }
            return before.iter().any(|b| !b.conditional);
        }
        builtin_name(name).is_some() && !self.module.globals.contains(name)
    }

    /// Whether a value is surely hashable (a set element, a dict key).
    fn hashable(&self, expr: &'a Expr, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        match expr {
            Expr::EllipsisLiteral(_) | Expr::Lambda(_) | Expr::Generator(_) => {
                self.value_depth(expr, depth).is_ok()
            }
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .all(|elt| !elt.is_starred_expr() && self.hashable(elt, depth + 1)),
            Expr::Name(name) => match self.constant(name.id.as_str()) {
                Some((value, scope)) => scope.hashable(value, depth + 1),
                None => self
                    .value_depth(expr, depth)
                    .is_ok_and(|v| hashable_value(&v)),
            },
            Expr::Call(call) => {
                self.value_depth(expr, depth).is_ok()
                    && self
                        .trusted_path(&call.func)
                        .is_some_and(|path| HASHABLE_CALLS.contains(&path.as_str()))
            }
            _ => self
                .value_depth(expr, depth)
                .is_ok_and(|v| hashable_value(&v)),
        }
    }

    /// The elements of a statically known iterable, when iterating it
    /// surely doesn't raise.
    fn iterable(&self, expr: &'a Expr, depth: u8) -> Option<Vec<Elem<'s, 'a>>> {
        if depth > DEPTH {
            return None;
        }
        let elems = self.sequence_depth(expr, depth).ok()?;
        elems
            .iter()
            .all(|elem| match elem {
                Elem::Value(_) => true,
                Elem::Expr(expr, lookup) => lookup.safe(expr, depth + 1),
            })
            .then_some(elems)
    }

    /// Whether the elements of a list, tuple or set display (with `*`
    /// unpacking) are surely built, and hashable when `hashed`.
    fn elements(&self, elts: &'a [Expr], hashed: bool, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        elts.iter().all(|elt| match elt {
            Expr::Starred(starred) => {
                self.iterable(&starred.value, depth + 1)
                    .is_some_and(|elems| {
                        !hashed
                            || elems.iter().all(|elem| match elem {
                                Elem::Value(value) => hashable_value(value),
                                Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                            })
                    })
            }
            elt => self.safe(elt, depth + 1) && (!hashed || self.hashable(elt, depth + 1)),
        })
    }

    /// Whether a dict display is surely built: hashable keys, and `**`
    /// of dict displays.
    fn mapping(&self, dict: &'a ast::ExprDict, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        dict.items.iter().all(|item| {
            let Some(key) = &item.key else {
                let (value, lookup) = crate::marks::deref(*self, &item.value);
                return matches!(value, Expr::Dict(inner) if lookup.mapping(inner, depth + 1));
            };
            self.safe(key, depth + 1)
                && self.hashable(key, depth + 1)
                && self.safe(&item.value, depth + 1)
        })
    }

    /// Whether a list or set comprehension is surely built: one `for` over
    /// a known iterable, no condition, making its elements or a literal.
    fn comprehension(
        &self,
        elt: &'a Expr,
        generators: &'a [ast::Comprehension],
        hashed: bool,
        depth: u8,
    ) -> bool {
        let [generator] = generators else {
            return false;
        };
        let Expr::Name(target) = &generator.target else {
            return false;
        };
        if generator.is_async || !generator.ifs.is_empty() {
            return false;
        }
        let Some(elems) = self.iterable(&generator.iter, depth + 1) else {
            return false;
        };
        match elt {
            Expr::Name(name) if name.id == target.id => {
                !hashed
                    || elems.iter().all(|elem| match elem {
                        Elem::Value(value) => hashable_value(value),
                        Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                    })
            }
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_) => self.safe(elt, depth + 1),
            _ => false,
        }
    }

    /// Whether a call to one of [`NO_ID`]'s callables surely makes its
    /// object: arguments it accepts, of types it accepts, with values it
    /// accepts. Anything aye can't tell is no.
    fn constructs(&self, path: &str, call: &'a ExprCall, depth: u8) -> bool {
        let arguments = &call.arguments;
        if arguments.args.iter().any(Expr::is_starred_expr)
            || arguments.keywords.iter().any(|k| k.arg.is_none())
        {
            return false;
        }
        let args: Vec<&'a Expr> = arguments.args.iter().collect();
        let keywords: Vec<(&str, &'a Expr)> = arguments
            .keywords
            .iter()
            .filter_map(|k| Some((k.arg.as_ref()?.id.as_str(), &k.value)))
            .collect();
        let value = |expr: &'a Expr| self.value_depth(expr, depth + 1).ok();
        let int = |expr: &'a Expr| match value(expr) {
            Some(Value::Int(digits)) => digits.parse::<i128>().ok(),
            Some(Value::Bool(b)) => Some(i128::from(b)),
            _ => None,
        };
        match path {
            "builtins.object" | "collections.defaultdict" => args.is_empty() && keywords.is_empty(),
            "builtins.range" => {
                if !keywords.is_empty() || !(1..=3).contains(&args.len()) {
                    return false;
                }
                // Any size of int; a step of 0 raises.
                let ints: Vec<Option<bool>> = args
                    .iter()
                    .map(|arg| match value(arg) {
                        Some(Value::Int(digits)) => {
                            Some(digits.trim_start_matches('-').bytes().all(|b| b == b'0'))
                        }
                        Some(Value::Bool(b)) => Some(!b),
                        _ => None,
                    })
                    .collect();
                ints.iter().all(Option::is_some) && !(args.len() == 3 && ints[2] == Some(true))
            }
            "builtins.list" | "builtins.tuple" | "collections.deque" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => self.iterated(arg, false, depth + 1),
                        _ => false,
                    }
            }
            "builtins.set" | "builtins.frozenset" | "collections.Counter" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => self.iterated(arg, true, depth + 1),
                        _ => false,
                    }
            }
            "builtins.dict" | "collections.OrderedDict" => {
                keywords.iter().all(|(_, v)| self.safe(v, depth + 1))
                    && match args.as_slice() {
                        [] => true,
                        [arg] => {
                            let (arg, lookup) = crate::marks::deref(*self, arg);
                            match arg {
                                Expr::Dict(dict) => lookup.mapping(dict, depth + 1),
                                // Pairs, each a two-item display.
                                _ => lookup.iterable(arg, depth + 1).is_some_and(|elems| {
                                    elems.iter().all(|elem| match elem {
                                        Elem::Expr(Expr::Tuple(pair), lookup) => {
                                            pair.elts.len() == 2
                                                && lookup.elements(&pair.elts, false, depth + 1)
                                                && lookup.hashable(&pair.elts[0], depth + 1)
                                        }
                                        Elem::Expr(Expr::List(pair), lookup) => {
                                            pair.elts.len() == 2
                                                && lookup.elements(&pair.elts, false, depth + 1)
                                                && lookup.hashable(&pair.elts[0], depth + 1)
                                        }
                                        _ => false,
                                    })
                                }),
                            }
                        }
                        _ => false,
                    }
            }
            "builtins.bytearray" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => match value(arg) {
                            Some(Value::Bytes(_)) => true,
                            Some(Value::Int(_) | Value::Bool(_)) => {
                                int(arg).is_some_and(|n| (0..=1_000_000).contains(&n))
                            }
                            _ => self.iterable(arg, depth + 1).is_some_and(|elems| {
                                elems.iter().all(|elem| {
                                    let byte = match elem {
                                        Elem::Value(Value::Int(digits)) => {
                                            digits.parse::<i64>().ok()
                                        }
                                        Elem::Expr(expr, lookup) => {
                                            match lookup.value_depth(expr, depth + 1) {
                                                Ok(Value::Int(digits)) => digits.parse().ok(),
                                                Ok(Value::Bool(b)) => Some(i64::from(b)),
                                                _ => None,
                                            }
                                        }
                                        Elem::Value(_) => None,
                                    };
                                    byte.is_some_and(|b| (0..=255).contains(&b))
                                })
                            }),
                        },
                        _ => false,
                    }
            }
            "builtins.slice" => {
                keywords.is_empty()
                    && (1..=3).contains(&args.len())
                    && args.iter().all(|arg| self.safe(arg, depth + 1))
            }
            "decimal.Decimal" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => match value(arg) {
                            Some(Value::Int(_) | Value::Float(_) | Value::Bool(_)) => true,
                            Some(Value::Str(text)) => decimal_parses(&text),
                            _ => false,
                        },
                        _ => false,
                    }
            }
            "fractions.Fraction" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [n] => int(n).is_some(),
                        [n, d] => int(n).is_some() && int(d).is_some_and(|d| d != 0),
                        _ => false,
                    }
            }
            "pathlib.Path"
            | "pathlib.PurePath"
            | "pathlib.PurePosixPath"
            | "pathlib.PureWindowsPath"
            | "pathlib.PosixPath"
            | "pathlib.WindowsPath" => {
                // A concrete path of the other platform can't be made.
                let platform = match path {
                    "pathlib.PosixPath" => !cfg!(windows),
                    "pathlib.WindowsPath" => cfg!(windows),
                    _ => true,
                };
                platform
                    && keywords.is_empty()
                    && args
                        .iter()
                        .all(|arg| matches!(value(arg), Some(Value::Str(_))))
            }
            "uuid.UUID" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [arg] if matches!(value(arg), Some(Value::Str(text)) if uuid_parses(&text)))
            }
            "datetime.date" | "datetime.datetime" | "datetime.time" => {
                let ints: Option<Vec<i128>> = args.iter().map(|arg| int(arg)).collect();
                keywords.is_empty() && ints.is_some_and(|ints| date_time_valid(path, &ints))
            }
            "datetime.timedelta" => {
                const UNITS: [&str; 7] = [
                    "days",
                    "seconds",
                    "microseconds",
                    "milliseconds",
                    "minutes",
                    "hours",
                    "weeks",
                ];
                args.len() <= UNITS.len()
                    && keywords
                        .iter()
                        .all(|(name, _)| UNITS[args.len()..].contains(name))
                    && args
                        .iter()
                        .chain(keywords.iter().map(|(_, v)| v))
                        .all(|arg| match value(arg) {
                            Some(Value::Int(_) | Value::Bool(_)) => {
                                int(arg).is_some_and(|n| n.abs() <= 1_000_000)
                            }
                            Some(Value::Float(f)) => f.abs() <= 1e6,
                            _ => false,
                        })
            }
            "numpy.zeros" | "numpy.ones" | "numpy.empty" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [shape] if self.shape(shape, depth))
            }
            "numpy.full" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [shape, fill] if self.shape(shape, depth)
                        && matches!(value(fill), Some(Value::Int(_) | Value::Float(_) | Value::Bool(_))))
            }
            "numpy.arange" => {
                let numbers: Option<Vec<f64>> = args
                    .iter()
                    .map(|arg| match value(arg) {
                        Some(Value::Int(digits)) => digits.parse::<f64>().ok(),
                        Some(Value::Float(f)) => Some(f),
                        _ => None,
                    })
                    .collect();
                keywords.is_empty()
                    && numbers.is_some_and(|n| {
                        let (first, end, stride) = match n.as_slice() {
                            [end] => (0.0, *end, 1.0),
                            [first, end] => (*first, *end, 1.0),
                            [first, end, stride] => (*first, *end, *stride),
                            _ => return false,
                        };
                        stride != 0.0
                            && [first, end, stride].iter().all(|x| x.is_finite())
                            && ((end - first) / stride) < 1e7
                    })
            }
            "numpy.linspace" => {
                let number = |arg: &'a Expr| {
                    matches!(
                        value(arg),
                        Some(Value::Int(_) | Value::Float(_) | Value::Bool(_))
                    )
                };
                keywords.is_empty()
                    && match args.as_slice() {
                        [a, b] => number(a) && number(b),
                        [a, b, n] => {
                            number(a)
                                && number(b)
                                && int(n).is_some_and(|n| (0..10_000_000).contains(&n))
                        }
                        _ => false,
                    }
            }
            "numpy.array" | "numpy.asarray" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [arg] if self.rectangular(arg, depth + 1).is_some())
            }
            // pandas parses and checks its arguments in ways aye doesn't
            // follow.
            _ => false,
        }
    }

    /// Whether iterating `expr` surely works, and gives hashable elements
    /// when `hashed`: a known sequence, or a set or dict display.
    fn iterated(&self, expr: &'a Expr, hashed: bool, depth: u8) -> bool {
        let (expr, lookup) = crate::marks::deref(*self, expr);
        match expr {
            Expr::Set(set) => lookup.elements(&set.elts, true, depth),
            Expr::Dict(dict) => lookup.mapping(dict, depth),
            _ => lookup.iterable(expr, depth).is_some_and(|elems| {
                !hashed
                    || elems.iter().all(|elem| match elem {
                        Elem::Value(value) => hashable_value(value),
                        Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                    })
            }),
        }
    }

    /// A numpy shape: a non-negative int, or a tuple or list of them.
    fn shape(&self, expr: &'a Expr, depth: u8) -> bool {
        let dimension = |expr: &'a Expr| match self.value_depth(expr, depth + 1) {
            Ok(Value::Int(digits)) => digits.parse::<u32>().is_ok_and(|n| n <= 10_000_000),
            _ => false,
        };
        match expr {
            Expr::Tuple(tuple) => tuple.elts.iter().all(dimension),
            Expr::List(list) => list.elts.iter().all(dimension),
            other => dimension(other),
        }
    }

    /// The shape of nested lists and tuples of numbers, booleans or
    /// strings that numpy makes an array of (it refuses ragged ones).
    fn rectangular(&self, expr: &'a Expr, depth: u8) -> Option<Vec<usize>> {
        if depth > DEPTH {
            return None;
        }
        let elts = match expr {
            Expr::List(list) => &list.elts,
            Expr::Tuple(tuple) => &tuple.elts,
            other => {
                return matches!(
                    self.value_depth(other, depth),
                    Ok(Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_))
                )
                .then(Vec::new);
            }
        };
        let mut inner: Option<Vec<usize>> = None;
        for elt in elts {
            let shape = self.rectangular(elt, depth + 1)?;
            match &inner {
                Some(found) if *found != shape => return None,
                Some(_) => {}
                None => inner = Some(shape),
            }
        }
        let mut shape = vec![elts.len()];
        shape.extend(inner.unwrap_or_default());
        Some(shape)
    }

    /// Whether `flags` for `re.compile` are known to be accepted with any
    /// pattern: `0`, `re.I`, `re.M`, `re.S` and their unions.
    fn regex_flags(&self, flags: &'a Expr) -> bool {
        match flags {
            Expr::NumberLiteral(n) => matches!(&n.value, Number::Int(i) if i.as_u64() == Some(0)),
            Expr::BinOp(op) if matches!(op.op, Operator::BitOr) => {
                self.regex_flags(&op.left) && self.regex_flags(&op.right)
            }
            other => matches!(
                self.trusted_path(other).as_deref(),
                Some("re.I" | "re.IGNORECASE" | "re.M" | "re.MULTILINE" | "re.S" | "re.DOTALL")
            ),
        }
    }

    /// The elements of a statically known iterable: list, tuple and dict
    /// literals (keys), strings, `range(...)`, concatenations, and names
    /// bound to those.
    pub(crate) fn sequence(&self, expr: &'a Expr) -> Result<Vec<Elem<'s, 'a>>, Unknown> {
        self.sequence_depth(expr, 0)
    }

    fn sequence_depth(&self, expr: &'a Expr, depth: u8) -> Result<Vec<Elem<'s, 'a>>, Unknown> {
        if depth > DEPTH {
            return Err(Unknown);
        }
        let elements = |elts: &'a [Expr]| -> Result<Vec<Elem<'s, 'a>>, Unknown> {
            let mut out = Vec::with_capacity(elts.len());
            for elt in elts {
                if let Expr::Starred(starred) = elt {
                    out.extend(self.sequence_depth(&starred.value, depth + 1)?);
                } else {
                    out.push(Elem::Expr(elt, *self));
                }
            }
            Ok(out)
        };
        match expr {
            Expr::List(list) => elements(&list.elts),
            Expr::Tuple(tuple) => elements(&tuple.elts),
            // A dict keeps one entry per distinct key, the first key written
            // in the first key's place: `{1: …, True: …, 1.0: …}` has one.
            Expr::Dict(dict) => {
                let mut seen: HashSet<DictKey> = HashSet::new();
                let mut out = Vec::new();
                for item in &dict.items {
                    let key = item.key.as_ref().ok_or(Unknown)?;
                    let value = self.value_depth(key, depth + 1)?;
                    // Its values are evaluated too, and may raise.
                    if !self.safe(&item.value, depth + 1) {
                        return Err(Unknown);
                    }
                    if seen.insert(DictKey::of(&value).ok_or(Unknown)?) {
                        out.push(Elem::Expr(key, *self));
                    }
                }
                // Huge ints and huge floats may be equal; not compared here.
                if seen.iter().any(|k| matches!(k, DictKey::BigInt(_)))
                    && seen.iter().any(|k| matches!(k, DictKey::BigFloat(_)))
                {
                    return Err(Unknown);
                }
                Ok(out)
            }
            Expr::StringLiteral(_) => match self.value(expr)? {
                Value::Str(text) => Ok(text
                    .chars()
                    .map(|c| Elem::Value(Value::Str(c.to_string())))
                    .collect()),
                _ => Err(Unknown),
            },
            Expr::BytesLiteral(b) => Ok(b
                .value
                .bytes()
                .map(|byte| Elem::Value(Value::Int(byte.to_string())))
                .collect()),
            Expr::BinOp(bin) if matches!(bin.op, Operator::Add) => {
                if !is_sequence_literal_like(&bin.left) && !is_sequence_literal_like(&bin.right) {
                    return Err(Unknown);
                }
                // Only a list and a list, or a tuple and a tuple, add up.
                match (
                    self.kind(&bin.left, depth + 1),
                    self.kind(&bin.right, depth + 1),
                ) {
                    (Some(a), Some(b)) if a == b && matches!(a, Kind::List | Kind::Tuple) => {}
                    _ => return Err(Unknown),
                }
                let mut left = self.sequence_depth(&bin.left, depth + 1)?;
                left.extend(self.sequence_depth(&bin.right, depth + 1)?);
                Ok(left)
            }
            Expr::Name(name) => {
                let (value, scope) = self.constant(name.id.as_str()).ok_or(Unknown)?;
                if !is_sequence_literal_like(value) {
                    return Err(Unknown);
                }
                scope.sequence_depth(value, depth + 1)
            }
            Expr::Call(call) => self.range_call(call).map(|ints| {
                ints.into_iter()
                    .map(|i| Elem::Value(Value::Int(i.to_string())))
                    .collect()
            }),
            _ => Err(Unknown),
        }
    }

    /// What kind of sequence an expression aye reads as one makes.
    fn kind(&self, expr: &'a Expr, depth: u8) -> Option<Kind> {
        if depth > DEPTH {
            return None;
        }
        match expr {
            Expr::List(_) => Some(Kind::List),
            Expr::Tuple(_) => Some(Kind::Tuple),
            Expr::Name(name) => {
                let (value, scope) = self.constant(name.id.as_str())?;
                scope.kind(value, depth + 1)
            }
            Expr::BinOp(bin) if matches!(bin.op, Operator::Add) => {
                let kind = self.kind(&bin.left, depth + 1)?;
                (self.kind(&bin.right, depth + 1)? == kind).then_some(kind)
            }
            _ => None,
        }
    }

    fn range_call(&self, call: &'a ExprCall) -> Result<Vec<i64>, Unknown> {
        if self.trusted_path(&call.func).as_deref() != Some("builtins.range")
            || !call.arguments.keywords.is_empty()
        {
            return Err(Unknown);
        }
        let mut bounds = Vec::new();
        for arg in &call.arguments.args {
            match self.value(arg)? {
                Value::Int(digits) => bounds.push(digits.parse::<i64>().map_err(|_| Unknown)?),
                _ => return Err(Unknown),
            }
        }
        let (first, end, stride) = match bounds.as_slice() {
            [end] => (0, *end, 1),
            [first, end] => (*first, *end, 1),
            [first, end, stride] if *stride != 0 => (*first, *end, *stride),
            _ => return Err(Unknown),
        };
        let mut out = Vec::new();
        let mut i = first;
        while (stride > 0 && i < end) || (stride < 0 && i > end) {
            out.push(i);
            if out.len() > 100_000 {
                return Err(Unknown);
            }
            // Past the range of `i64`, the next value is past `end` too.
            match i.checked_add(stride) {
                Some(next) => i = next,
                None => break,
            }
        }
        Ok(out)
    }

    /// A string value, statically.
    pub(crate) fn string(&self, expr: &'a Expr) -> Option<String> {
        match self.value(expr) {
            Ok(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// A boolean value, statically (`True`, `False`, or a constant).
    pub(crate) fn boolean(&self, expr: &'a Expr) -> Option<bool> {
        match self.value(expr) {
            Ok(Value::Bool(b)) => Some(b),
            _ => None,
        }
    }
}

/// A value with no id of its own, when it is surely built.
fn opaque(built: bool) -> Result<Value, Unknown> {
    if built {
        Ok(Value::Opaque)
    } else {
        Err(Unknown)
    }
}

/// Calls whose results are hashable, once made.
const HASHABLE_CALLS: &[&str] = &[
    "builtins.float",
    "builtins.frozenset",
    "builtins.object",
    "builtins.range",
    "datetime.date",
    "datetime.datetime",
    "datetime.time",
    "datetime.timedelta",
    "decimal.Decimal",
    "fractions.Fraction",
    "pathlib.Path",
    "pathlib.PosixPath",
    "pathlib.PurePath",
    "pathlib.PurePosixPath",
    "pathlib.PureWindowsPath",
    "pathlib.WindowsPath",
    "re.compile",
    "uuid.UUID",
];

/// Whether a value aye knows is hashable: not a container (whose kind
/// [`Value::Opaque`] doesn't tell).
fn hashable_value(value: &Value) -> bool {
    !matches!(value, Value::Opaque | Value::Unknown)
}

/// Whether `Decimal(text)` surely parses `text`: a plain decimal number
/// in ASCII, or an infinity or a NaN, with ASCII spaces around (a subset
/// of what it accepts).
fn decimal_parses(text: &str) -> bool {
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let lower = unsigned.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity") {
        return true;
    }
    if let Some(digits) = lower
        .strip_prefix("nan")
        .or_else(|| lower.strip_prefix("snan"))
    {
        return digits.bytes().all(|b| b.is_ascii_digit());
    }
    let (mantissa, exponent) = match lower.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (lower.as_str(), None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => {
            (digits(int) || int.is_empty())
                && (digits(frac) || frac.is_empty())
                && !(int.is_empty() && frac.is_empty())
        }
        None => digits(mantissa),
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    mantissa_ok && exponent_ok
}

/// Whether `UUID(text)` surely parses `text`: 32 hex digits once
/// `urn:`, `uuid:`, braces and hyphens are taken out, as it takes them.
fn uuid_parses(text: &str) -> bool {
    let hex = text.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(['{', '}']).replace('-', "");
    hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `date(…)`, `datetime(…)` or `time(…)` accepts these ints.
fn date_time_valid(path: &str, ints: &[i128]) -> bool {
    let date_ok = |year: i128, month: i128, day: i128| {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return false,
        };
        (1..=9999).contains(&year) && (1..=days).contains(&day)
    };
    let time_ok = |parts: &[i128]| {
        let limits = [23, 59, 59, 999_999];
        parts.len() <= limits.len()
            && parts
                .iter()
                .zip(limits)
                .all(|(part, limit)| (0..=limit).contains(part))
    };
    match path {
        "datetime.date" => matches!(ints, [y, m, d] if date_ok(*y, *m, *d)),
        "datetime.datetime" => {
            ints.len() >= 3 && date_ok(ints[0], ints[1], ints[2]) && time_ok(&ints[3..])
        }
        "datetime.time" => time_ok(ints),
        _ => false,
    }
}

/// A sequence's type, for `+`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    List,
    Tuple,
}

fn is_sequence_literal_like(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::List(_) | Expr::Tuple(_) | Expr::Name(_) | Expr::BinOp(_) | Expr::Call(_)
    )
}

fn complex_parts(value: &Value) -> Option<(f64, f64)> {
    match value {
        Value::Int(digits) => Some((digits.parse().ok()?, 0.0)),
        Value::Float(f) => Some((*f, 0.0)),
        Value::Complex(re, im) => Some((*re, *im)),
        Value::Bool(b) => Some((f64::from(u8::from(*b)), 0.0)),
        _ => None,
    }
}

fn negate(digits: &str) -> String {
    match digits.strip_prefix('-') {
        Some(rest) => rest.to_string(),
        None if digits == "0" => digits.to_string(),
        None => format!("-{digits}"),
    }
}

/// The longest int whose `str()` (its id) surely works: Python's limit
/// (`sys.set_int_max_str_digits`) is 4300 digits by default and can be
/// lowered to 640.
const MAX_INT_DIGITS: usize = 640;

/// A string's value. A lone surrogate escape (`"\ud800"`) reads as U+FFFD,
/// so any U+FFFD is left to Python: its id may differ.
fn literal_text(text: String) -> Result<Value, Unknown> {
    if text.contains('\u{fffd}') {
        Err(Unknown)
    } else {
        Ok(Value::Str(text))
    }
}

/// A dict key as Python compares keys: numbers by value across `bool`,
/// `int`, `float` and `complex`; strings and bytes never equal.
#[derive(Debug, PartialEq, Eq, Hash)]
enum DictKey {
    None,
    Str(String),
    Bytes(Vec<u8>),
    /// Integral numbers that fit (`1`, `True`, `1.0`, `1+0j`).
    Int(i128),
    /// The decimal digits of a larger int.
    BigInt(String),
    /// The bits of a larger integral float.
    BigFloat(u64),
    /// The bits of any other float.
    Float(u64),
    /// The bits of a complex number's parts, when it is not real.
    Complex(u64, u64),
}

impl DictKey {
    /// `None` for values that can't be compared statically (a `nan` is
    /// equal only to itself, by identity).
    fn of(value: &Value) -> Option<Self> {
        Some(match value {
            Value::None => Self::None,
            Value::Str(s) => Self::Str(s.clone()),
            Value::Bytes(b) => Self::Bytes(b.clone()),
            Value::Bool(b) => Self::Int(i128::from(*b)),
            Value::Int(digits) => digits
                .parse()
                .map_or_else(|_| Self::BigInt(digits.clone()), Self::Int),
            Value::Float(f) => Self::float(*f)?,
            Value::Complex(re, im) if *im == 0.0 => Self::float(*re)?,
            Value::Complex(re, im) if re.is_nan() || im.is_nan() => return None,
            // `-0.0 == 0.0`.
            Value::Complex(re, im) => Self::Complex((re + 0.0).to_bits(), im.to_bits()),
            _ => return None,
        })
    }

    fn float(f: f64) -> Option<Self> {
        // 2^126: integral floats below convert to `i128` exactly.
        const LIMIT: f64 = 85_070_591_730_234_615_865_843_651_857_942_052_864.0;
        if f.is_nan() {
            None
        } else if f.is_infinite() || f.fract() != 0.0 {
            Some(Self::Float(f.to_bits()))
        } else if f.abs() < LIMIT {
            #[expect(clippy::cast_possible_truncation, reason = "integral and below 2^126")]
            Some(Self::Int(f as i128))
        } else {
            Some(Self::BigFloat(f.to_bits()))
        }
    }
}

/// `float(text)` for the spellings Python accepts, ASCII only (Python also
/// takes other Unicode digits and spaces, which are left to it):
/// surrounding whitespace, one sign, then `inf`, `infinity`, `nan`, or
/// digits with single underscores between them, an optional fraction and an
/// optional exponent.
fn parse_float(text: &str) -> Option<f64> {
    /// Digits from `*at`, with single underscores between digits: how many
    /// digits, or `None` for a misplaced underscore.
    fn digits(bytes: &[u8], at: &mut usize) -> Option<usize> {
        let mut count = 0;
        while let Some(&byte) = bytes.get(*at) {
            if byte.is_ascii_digit() {
                count += 1;
            } else if byte == b'_' {
                if count == 0 || !bytes.get(*at + 1).is_some_and(u8::is_ascii_digit) {
                    return None;
                }
            } else {
                break;
            }
            *at += 1;
        }
        Some(count)
    }

    let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c'));
    if !trimmed.is_ascii() {
        return None;
    }
    let (sign, rest) = match trimmed.as_bytes().first() {
        Some(b'-') => (-1.0, &trimmed[1..]),
        Some(b'+') => (1.0, &trimmed[1..]),
        _ => (1.0, trimmed),
    };
    match rest.to_ascii_lowercase().as_str() {
        "inf" | "infinity" => return Some(sign * f64::INFINITY),
        "nan" => return Some(f64::NAN),
        _ => {}
    }
    let bytes = rest.as_bytes();
    let mut at = 0;
    let whole = digits(bytes, &mut at)?;
    let mut fraction = 0;
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        fraction = digits(bytes, &mut at)?;
    }
    if whole == 0 && fraction == 0 {
        return None;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        if digits(bytes, &mut at)? == 0 {
            return None;
        }
    }
    if at != bytes.len() {
        return None;
    }
    let cleaned: String = rest.chars().filter(|c| *c != '_').collect();
    cleaned.parse::<f64>().ok().map(|f| sign * f)
}

/// Whether `re.compile` surely accepts a pattern, without warnings, on
/// every Python pytest 9.1.1 supports: a conservative subset of the
/// syntax (literals, escapes, classes, plain, non-capturing, named and
/// lookahead groups, alternation, quantifiers). Anything else is left to
/// Python.
fn regex_compiles(pattern: &Value) -> bool {
    let (chars, bytes): (Vec<char>, bool) = match pattern {
        Value::Str(s) => (s.chars().collect(), false),
        Value::Bytes(b) => (b.iter().map(|b| char::from(*b)).collect(), true),
        _ => return false,
    };
    let mut parser = Regex {
        chars: &chars,
        at: 0,
        bytes,
        names: Vec::new(),
        groups: 0,
    };
    parser.alternation(0).is_some() && parser.at == chars.len()
}

/// What the last piece of a branch was, for the quantifier after it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Piece {
    /// Nothing yet: nothing to repeat.
    Nothing,
    /// `^`, `$`, `\b`, a lookahead: not repeated here.
    Assertion,
    Atom,
    /// Already repeated: another quantifier is an error, or possessive.
    Repeated,
}

/// A recursive-descent reader of `re` patterns (`_parser.py`), refusing
/// whatever it is not sure about.
struct Regex<'p> {
    chars: &'p [char],
    at: usize,
    bytes: bool,
    names: Vec<String>,
    groups: usize,
}

impl Regex<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += 1;
        Some(c)
    }

    fn alternation(&mut self, depth: usize) -> Option<()> {
        if depth > 32 {
            return None;
        }
        loop {
            self.branch(depth)?;
            if self.peek() != Some('|') {
                return Some(());
            }
            self.at += 1;
        }
    }

    fn branch(&mut self, depth: usize) -> Option<()> {
        let mut last = Piece::Nothing;
        while let Some(c) = self.peek() {
            if last == Piece::Repeated && matches!(c, '*' | '+' | '?' | '{') {
                return None;
            }
            last = match c {
                '|' | ')' => return Some(()),
                '*' | '+' | '?' => {
                    self.at += 1;
                    self.repeated(last)?
                }
                '{' => {
                    if self.quantifier()? {
                        self.repeated(last)?
                    } else {
                        Piece::Atom
                    }
                }
                '(' => {
                    self.at += 1;
                    self.group(depth)?
                }
                '[' => {
                    self.at += 1;
                    self.class()?;
                    Piece::Atom
                }
                '\\' => {
                    self.at += 1;
                    self.escape()?
                }
                '^' | '$' => {
                    self.at += 1;
                    Piece::Assertion
                }
                _ => {
                    self.at += 1;
                    Piece::Atom
                }
            };
        }
        Some(())
    }

    /// After a quantifier: the piece it repeats must be an atom; a lazy `?`
    /// may follow.
    fn repeated(&mut self, last: Piece) -> Option<Piece> {
        if last != Piece::Atom {
            return None;
        }
        if self.peek() == Some('?') {
            self.at += 1;
        }
        Some(Piece::Repeated)
    }

    /// At `{`: whether it starts a quantifier (consumed), or is a literal
    /// brace (only it consumed); `None` for a bad one.
    fn quantifier(&mut self) -> Option<bool> {
        let start = self.at;
        self.at += 1;
        if self.peek() == Some('}') {
            return Some(false);
        }
        let lo = self.digits();
        let hi = if self.peek() == Some(',') {
            self.at += 1;
            self.digits()
        } else {
            lo.clone()
        };
        if self.peek() != Some('}') {
            self.at = start + 1;
            return Some(false);
        }
        self.at += 1;
        let bound = |digits: &str| -> Option<Option<u32>> {
            if digits.is_empty() {
                Some(None)
            } else {
                digits.parse::<u32>().ok().filter(|n| *n < 65_536).map(Some)
            }
        };
        let lo = bound(&lo)?.unwrap_or(0);
        match bound(&hi)? {
            Some(hi) if hi < lo => None,
            _ => Some(true),
        }
    }

    fn digits(&mut self) -> String {
        let mut out = String::new();
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            out.push(c);
            self.at += 1;
        }
        out
    }

    /// After `(`.
    fn group(&mut self, depth: usize) -> Option<Piece> {
        let mut piece = Piece::Atom;
        if self.peek() == Some('?') {
            self.at += 1;
            match self.next()? {
                ':' => {}
                '=' | '!' => piece = Piece::Assertion,
                'P' => {
                    if self.next()? != '<' {
                        return None;
                    }
                    let mut name = String::new();
                    loop {
                        match self.next()? {
                            '>' => break,
                            c => name.push(c),
                        }
                    }
                    let mut chars = name.chars();
                    let valid = chars
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if !valid || self.names.contains(&name) {
                        return None;
                    }
                    self.names.push(name);
                    self.groups += 1;
                }
                // Flags, lookbehinds, comments, backreferences, atomic
                // groups, conditionals.
                _ => return None,
            }
        } else {
            self.groups += 1;
        }
        if self.groups > 100 {
            return None;
        }
        self.alternation(depth + 1)?;
        (self.next()? == ')').then_some(piece)
    }

    /// After `\`, outside a class.
    fn escape(&mut self) -> Option<Piece> {
        match self.next()? {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => Some(Piece::Atom),
            'A' | 'Z' | 'b' | 'B' => Some(Piece::Assertion),
            c => self.literal_escape(c).map(|_| Piece::Atom),
        }
    }

    /// An escape standing for one character, after `\` and `c`: its code.
    fn literal_escape(&mut self, c: char) -> Option<u32> {
        match c {
            'a' => Some(7),
            'b' => Some(8),
            't' => Some(9),
            'n' => Some(10),
            'v' => Some(11),
            'f' => Some(12),
            'r' => Some(13),
            'x' => self.hex(2),
            'u' if !self.bytes => self.hex(4),
            'U' if !self.bytes => self.hex(8).filter(|c| *c <= 0x10_ffff),
            '0' => {
                let mut code = 0;
                for _ in 0..2 {
                    match self.peek().and_then(|d| d.to_digit(8)) {
                        Some(digit) => {
                            code = code * 8 + digit;
                            self.at += 1;
                        }
                        None => break,
                    }
                }
                Some(code)
            }
            // Other letters are errors, digits are group references.
            c if c.is_ascii_alphanumeric() => None,
            c => Some(u32::from(c)),
        }
    }

    fn hex(&mut self, count: usize) -> Option<u32> {
        let mut code = 0;
        for _ in 0..count {
            code = code * 16 + self.next()?.to_digit(16)?;
        }
        Some(code)
    }

    /// After `[`.
    fn class(&mut self) -> Option<()> {
        // `[[`: "possible nested set" (a `FutureWarning`).
        if self.peek() == Some('[') {
            return None;
        }
        if self.peek() == Some('^') {
            self.at += 1;
        }
        let mut first = true;
        loop {
            let c = self.next()?;
            if c == ']' && !first {
                return Some(());
            }
            first = false;
            let lo = self.class_item(c)?;
            if self.peek() == Some('-') {
                self.at += 1;
                let that = self.next()?;
                if that == ']' {
                    return Some(());
                }
                if that == '-' {
                    return None;
                }
                match (lo, self.class_item(that)?) {
                    (Member::Char(lo), Member::Char(hi)) if lo <= hi => {}
                    _ => return None,
                }
            }
        }
    }

    /// One member of a class, after `c`; `None` when Python may refuse or
    /// warn.
    fn class_item(&mut self, c: char) -> Option<Member> {
        match c {
            '\\' => match self.next()? {
                'd' | 'D' | 's' | 'S' | 'w' | 'W' => Some(Member::Category),
                e => self.literal_escape(e).map(Member::Char),
            },
            // Nested sets and set operations warn.
            '[' => None,
            '-' | '&' | '~' | '|' if self.peek() == Some(c) => None,
            c => Some(Member::Char(u32::from(c))),
        }
    }
}

/// A member of a regex class.
#[derive(Clone, Copy)]
enum Member {
    /// One character, by code.
    Char(u32),
    /// `\d` and the like.
    Category,
}

/// Whether a function body makes it a generator (a `yield` outside nested
/// functions, lambdas and classes).
pub(crate) fn is_generator(body: &[Stmt]) -> bool {
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
pub(crate) fn exclusive(a: &Bound<'_>, b: &Bound<'_>) -> bool {
    a.branches
        .iter()
        .zip(&b.branches)
        .take_while(|(x, y)| x.0 == y.0)
        .any(|(x, y)| x.1 != y.1)
}

/// A `pytest.skip` or `pytest.importorskip` call that runs while the module
/// is imported, in its body or a class body: pytest then skips the whole
/// module, depending on the environment.
pub(crate) fn import_time_skip<'a>(body: &'a [Stmt], lookup: Lookup<'_, 'a>) -> Option<String> {
    use aye_adapter::ast::visitor::{Visitor, walk_expr, walk_stmt};

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
pub(crate) fn import_time_raise<'a>(body: &'a [Stmt], lookup: Lookup<'_, 'a>) -> Option<String> {
    use aye_adapter::ast::visitor::{Visitor, walk_expr, walk_stmt};

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

#[cfg(test)]
mod tests {
    //! Expected ids are copied from `pytest --collect-only -q` on pytest
    //! 9.1.1 (Python 3.13); `static_values_match_pytest` re-checks every
    //! exact reading against a real pytest when one is available.

    use std::path::{Path, PathBuf};
    use std::process::Command;

    use aye_adapter::{LineIndex, Project, Recognition, Recognizer, SourceFile};
    use aye_events::Certainty;

    use super::*;

    fn recognize(source: &str) -> Recognition {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
        let project = Project {
            root: dir.path(),
            cwd: dir.path(),
            args: &[],
            config_file: None,
            framework_args: &[],
        };
        let recognizer = crate::PytestRecognizer::new(&project).unwrap();
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let lines = LineIndex::from_source_text(source);
        let abs = dir.path().join("test_x.py");
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("test_x.py"),
            abs: &abs,
            source,
            module: parsed.syntax(),
            lines: &lines,
        };
        recognizer.recognize(&file, &[])
    }

    /// The ids of a file read statically, or `None` when it is refined.
    fn exact(source: &str) -> Option<Vec<String>> {
        let recognition = recognize(source);
        if recognition.refine.is_some() {
            return None;
        }
        Some(
            recognition
                .cases
                .iter()
                .map(|case| {
                    assert_eq!(case.certainty, Certainty::Exact, "{source}");
                    case.id
                        .as_str()
                        .trim_start_matches("test_x.py::")
                        .to_string()
                })
                .collect(),
        )
    }

    /// A module binding `CASES`, then `setup`, then a test parametrized
    /// over `CASES`.
    fn over_cases(setup: &str) -> String {
        format!(
            "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n"
        )
    }

    /// Module constants changed where only an import shows it (pytest
    /// collects the extra or reordered parameters), each refined.
    const CHANGED: &[(&str, &str)] = &[
        (
            "helper",
            "CASES = [1, 2]\ndef _add():\n    CASES.append(3)\n_add()",
        ),
        ("alias", "CASES = [1, 2]\nALIAS = CASES\nALIAS.append(3)"),
        (
            "class_alias",
            "CASES = [1, 2]\nclass H:\n    x = CASES\nH.x.append(3)",
        ),
        (
            "class_dict",
            "CASES = [1, 2]\nclass H:\n    x = CASES\nH.__dict__[\"x\"].append(3)",
        ),
        (
            "class_getattr",
            "CASES = [1, 2]\nclass H:\n    x = CASES\ngetattr(H, \"x\").append(3)",
        ),
        (
            "class_handed",
            "from elsewhere import register\nCASES = [1, 2]\nclass H:\n    x = CASES\nregister(H)",
        ),
        (
            "class_base",
            "from elsewhere import Base\nCASES = [1, 2]\nclass H(Base):\n    x = CASES",
        ),
        (
            "class_metaclass",
            "from elsewhere import Meta\nCASES = [1, 2]\nclass H(metaclass=Meta):\n    x = CASES",
        ),
        (
            "instance_default",
            "from dataclasses import dataclass\nCASES = [1, 2]\nclass P:\n    items = CASES\nP().items.append(3)",
        ),
        (
            "modules_literal",
            "import sys\nCASES = [1, 2]\nsys.modules[\"test_x\"].CASES.append(3)",
        ),
        (
            "from_itself",
            "CASES = [1, 2]\nfrom test_x import CASES as SAME\nSAME.append(3)",
        ),
        (
            "frame_back",
            "import inspect\nCASES = [1, 2]\ndef f():\n    inspect.stack()[0].frame.f_back.f_globals[\"CASES\"].append(3)\nf()",
        ),
        (
            "passed",
            "CASES = [1, 2]\ndef mutate(l):\n    l.append(3)\nmutate(CASES)",
        ),
        ("list_sort", "CASES = [3, 1, 2]\nlist.sort(CASES)"),
        (
            "type_append",
            "CASES = [1, 2]\ntype(CASES).append(CASES, 3)",
        ),
        (
            "comprehension",
            "CASES = [1, 2]\n[CASES.append(i) for i in (7, 8)]",
        ),
        (
            "if_expression",
            "CASES = [1, 2]\nCASES.append(3) if True else None",
        ),
        ("printed", "CASES = [1, 2]\nprint(CASES.append(3))"),
        ("lambda", "CASES = [1, 2]\n(lambda: CASES.append(3))()"),
        (
            "walrus",
            "CASES = [1, 2]\nif (n := CASES.append(3)) is None:\n    pass",
        ),
        ("assert", "CASES = [1, 2]\nassert CASES.append(3) is None"),
        (
            "match",
            "CASES = [1, 2]\nmatch CASES.append(3):\n    case _:\n        pass",
        ),
        (
            "with",
            "import contextlib\nCASES = [1, 2]\nwith contextlib.nullcontext(CASES.append(3)):\n    pass",
        ),
        (
            "default",
            "CASES = [1, 2]\ndef f(x=CASES.append(3)):\n    pass",
        ),
        (
            "decorator",
            "CASES = [1, 2]\ndef register(x):\n    return lambda f: f\n@register(CASES.append(3))\ndef helper():\n    pass",
        ),
        (
            "sort_if",
            "CASES = [3, 1, 2]\nCASES.sort() if CASES else None",
        ),
        ("pop", "CASES = [1, 2, 3]\n_last = CASES.pop()"),
        ("dunder", "CASES = [1, 2]\nCASES.__iadd__([9])"),
        (
            "class_body",
            "CASES = [1, 2]\nclass Helper:\n    CASES.append(3)",
        ),
        (
            "augmented_alias",
            "CASES = [1, 2]\nOTHER = CASES\nOTHER += [3]",
        ),
        ("kept_method", "CASES = [1, 2]\nadd = CASES.append\nadd(3)"),
        (
            "loop_variable",
            "CASES = [[1], [2]]\nfor row in CASES:\n    row.append(0)",
        ),
        (
            "nested_call",
            "CASES = [1, 2]\ndef a():\n    b()\ndef b():\n    CASES.append(3)\na()",
        ),
        (
            "decorated",
            "CASES = [1, 2]\ndef run(f):\n    f()\n    return f\n@run\ndef add():\n    CASES.append(3)",
        ),
        ("getattr", "CASES = [1, 2]\ngetattr(CASES, \"append\")(3)"),
        (
            "used_class",
            "CASES = [1, 2]\nclass Adder:\n    def __init__(self):\n        CASES.append(3)\nAdder()",
        ),
        (
            "hostile_dunder",
            "CASES = [1, 2]\nclass Eater:\n    def __eq__(self, other):\n        other.append(3)\n        return False\nEater() == CASES",
        ),
        (
            "globals",
            "CASES = [1, 2]\ng = globals()\ng[\"CASES\"] = [9]",
        ),
        (
            "setattr",
            "import sys\nCASES = [1, 2]\nsetattr(sys.modules[__name__], \"CASES\", [9])",
        ),
        ("exec", "CASES = [1, 2]\nexec(\"CASES.append(3)\")"),
        ("dict_update", "CASES = {\"a\": 1}\nCASES.update(b=2)"),
        (
            "stored",
            "CASES = [1, 2]\nclass Box:\n    pass\nBox.items = CASES\nBox.items.append(3)",
        ),
        (
            "local_alias",
            "CASES = [1, 2]\ndef grow():\n    items = CASES\n    items.append(3)\ngrow()",
        ),
        (
            "closure",
            "CASES = [1, 2]\ndef outer():\n    items = CASES\n    def inner():\n        items.append(3)\n    inner()\nouter()",
        ),
        (
            "default_parameter",
            "CASES = [1, 2]\ndef grow(items=CASES):\n    items.append(3)\ngrow()",
        ),
        (
            "global_statement",
            "CASES = [1, 2]\ndef grow():\n    global CASES\n    CASES = CASES + [3]\ngrow()",
        ),
    ];

    #[test]
    fn constants_changed_by_code_aye_cannot_follow_are_refined() {
        for (name, setup) in CHANGED {
            assert_eq!(exact(&over_cases(setup)), None, "{name}");
        }
        // The same through an element: `INNER` shares `CASES`'s fate.
        let nested = "import pytest\nINNER = [1, 2]\nCASES = [INNER, 3]\nCASES[0].append(5)\n@pytest.mark.parametrize(\"v\", INNER)\ndef test_x(v):\n    pass\n";
        assert_eq!(exact(nested), None);
        // And in a class body, for the module's constant.
        let class = "import pytest\nCASES = [1, 2]\nclass TestK:\n    CASES.append(3)\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\n";
        assert_eq!(exact(class), None);
        // A class attribute changed through its class.
        let attribute = "import pytest\nclass TestK:\n    CASES = [1, 2]\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\nTestK.CASES.append(3)\n";
        assert_eq!(exact(attribute), None);
        let rebound = "import pytest\nclass TestK:\n    CASES = (1, 2)\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        pass\nTestK.CASES = (3,)\n";
        assert_eq!(exact(rebound), None);
    }

    /// Reads, and code that only runs after collection, keep constants
    /// static.
    const UNCHANGED: &[(&str, &str, &[&str])] = &[
        (
            "test_body",
            "import pytest\nCASES = [1, 2]\ndef helper(x):\n    return x\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    assert helper(CASES)\n    CASES.append(v)\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "fixture",
            "import pytest\nCASES = [1, 2]\n@pytest.fixture\ndef data():\n    return CASES\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v, data):\n    pass\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "immutable",
            "import pytest\nCASES = (1, 2)\nprint(CASES)\nsorted(CASES, key=str)\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "reads",
            "import pytest\nCASES = [1, 2]\nN = len(CASES)\nFIRST = CASES[0]\nCOPY = list(CASES)\nTEXT = \", \".join(str(c) for c in CASES)\nassert 1 in CASES\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "concatenation",
            "import pytest\nBASE = [1, 2]\nALL = BASE + [3]\n@pytest.mark.parametrize(\"v\", ALL)\ndef test_x(v):\n    pass\n@pytest.mark.parametrize(\"w\", BASE)\ndef test_y(w):\n    pass\n",
            &[
                "test_x[1]",
                "test_x[2]",
                "test_x[3]",
                "test_y[1]",
                "test_y[2]",
            ],
        ),
        (
            "unused_helper",
            "import pytest\nCASES = [1, 2]\ndef never_called():\n    CASES.append(3)\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "class_attributes",
            "import pytest\nclass TestK:\n    CASES = [1, 2]\n    pytestmark = []\n    pytestmark += [pytest.mark.slow]\n    @pytest.mark.parametrize(\"v\", CASES)\n    def test_k(self, v):\n        self.CASES.append(v)\n",
            &["TestK::test_k[1]", "TestK::test_k[2]"],
        ),
        (
            "shadowing_parameter",
            "import pytest\nE = [2, 8]\ndef wrap(f):\n    return f\n@pytest.mark.parametrize(\"E\", E)\n@wrap\ndef test_x(E):\n    print(E)\n    E = []\n    E.append(1)\n",
            &["test_x[2]", "test_x[8]"],
        ),
        (
            "class_sibling",
            "import pytest\nCASES = [1, 2]\nOTHER = [3]\nclass H:\n    a = CASES\n    b = OTHER\nH.b.append(5)\nH.c = 1\n@pytest.mark.parametrize(\"v\", CASES)\ndef test_x(v):\n    pass\n",
            &["test_x[1]", "test_x[2]"],
        ),
        (
            "pytest_arguments",
            "import pytest\nCASES = [1, 2]\nIDS = [\"one\", \"two\"]\n@pytest.mark.parametrize(\"v\", CASES, ids=IDS)\ndef test_x(v):\n    pass\n@pytest.mark.parametrize(\"w\", [pytest.param(CASES, id=\"all\")])\ndef test_y(w):\n    pass\n",
            &["test_x[one]", "test_x[two]", "test_y[all]"],
        ),
    ];

    #[test]
    fn reads_and_code_run_after_collection_keep_constants_exact() {
        for (name, source, expected) in UNCHANGED {
            let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();
            assert_eq!(exact(source), Some(expected), "{name}");
        }
    }

    #[test]
    fn walrus_match_and_except_bindings_and_del_are_followed() {
        for (name, setup, values) in [
            (
                "comprehension",
                "X = [\"a\"]\n[(X := [\"comp\"]) for _ in range(1)]",
                "X",
            ),
            (
                "match",
                "X = [\"a\"]\nmatch [\"m\"]:\n    case X:\n        pass",
                "X",
            ),
            ("argument", "X = [\"a\"]\nprint(X := [\"p\"])", "X"),
            ("assigned", "X = [\"a\"]\nY = (X := [\"w\"])", "X"),
            ("test", "N = 2\nif (N := 3):\n    pass", "range(N)"),
            (
                "default",
                "V = \"orig\"\ndef f(x=(V := \"default\")):\n    pass",
                "[V]",
            ),
            ("statement", "X = [\"a\"]\n(X := [\"walrus\"])", "X"),
            (
                "except",
                "X = [\"a\"]\ntry:\n    raise ValueError\nexcept ValueError as X:\n    pass",
                "X",
            ),
            (
                "maybe_deleted",
                "import sys\nX = [\"a\"]\nif sys.platform == \"nope\":\n    del X",
                "X",
            ),
        ] {
            let source = format!(
                "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", {values})\ndef test_x(v):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{name}");
        }
        // pytest: test_x[len] (the builtin), and TestA::test_a[mod].
        assert_eq!(
            exact(
                "import pytest\nlen = 5\ndel len\n@pytest.mark.parametrize(\"v\", [len])\ndef test_x(v):\n    pass\n"
            ),
            Some(vec!["test_x[len]".to_string()])
        );
        assert_eq!(
            exact(
                "import pytest\nX = [\"mod\"]\nclass TestA:\n    X = [\"cls\"]\n    del X\n    @pytest.mark.parametrize(\"v\", X)\n    def test_a(self, v):\n        pass\n"
            ),
            Some(vec!["TestA::test_a[mod]".to_string()])
        );
    }

    #[test]
    fn renamed_functions_and_classes_are_refined() {
        for source in [
            "import pytest\ndef renamed():\n    pass\nrenamed.__name__ = \"other_name\"\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
            "import pytest\nclass Klass:\n    pass\nKlass.__name__ = \"NewKlass\"\n@pytest.mark.parametrize(\"v\", [Klass])\ndef test_x(v):\n    pass\n",
            "import pytest\ndef renamed():\n    pass\ndef _rename():\n    renamed.__name__ = \"via_helper\"\n_rename()\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
            "import pytest\ndef renamed():\n    pass\ng = renamed\ng.__name__ = \"alias\"\n@pytest.mark.parametrize(\"v\", [renamed])\ndef test_x(v):\n    pass\n",
            "import pytest\nf = lambda: 0\nf.__name__ = \"lam\"\n@pytest.mark.parametrize(\"v\", [f])\ndef test_x(v):\n    pass\n",
            "import pytest\ndef renamed():\n    pass\nclass TestK:\n    renamed.__name__ = \"in_class\"\n    @pytest.mark.parametrize(\"v\", [renamed])\n    def test_k(self, v):\n        pass\n",
        ] {
            assert_eq!(exact(source), None, "{source}");
        }
        // Renamed only once the tests run.
        assert_eq!(
            exact(
                "import pytest\ndef plain():\n    pass\n@pytest.mark.parametrize(\"v\", [plain, len])\ndef test_x(v):\n    plain.__name__ = \"late\"\n"
            ),
            Some(vec!["test_x[plain]".to_string(), "test_x[len]".to_string()])
        );
    }

    #[test]
    fn generators_are_named_genexpr() {
        assert_eq!(
            exact("import pytest\n@pytest.mark.parametrize(\"v\", [range(2), (x for x in []), ..., {1}, {}, [], ()])\ndef test_opaque(v):\n    pass\n").unwrap(),
            [
                "test_opaque[v0]",
                "test_opaque[<genexpr>]",
                "test_opaque[v2]",
                "test_opaque[v3]",
                "test_opaque[v4]",
                "test_opaque[v5]",
                "test_opaque[v6]",
            ]
        );
    }

    #[test]
    fn complex_sums_whose_sign_depends_on_the_python_version_are_refined() {
        // Python 3.14 gives (1-0j), 3.13 (1+0j).
        for value in ["1 - 0j", "0.0 - 0j", "-0.0 - 0j", "1 + -0j", "True - 0j"] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"c\", [{value}])\ndef test_c(c):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{value}");
        }
        assert_eq!(
            exact("import pytest\n@pytest.mark.parametrize(\"c\", [1 + 2j, 1 - 2j, -0.0 + 0j, 1 + 0j, 0 - -0j, (1+1j) - 0j, -3j])\ndef test_c(c):\n    pass\n").unwrap(),
            [
                "test_c[(1+2j)]",
                "test_c[(1-2j)]",
                "test_c[0j0]",
                "test_c[(1+0j)]",
                "test_c[0j1]",
                "test_c[(1+1j)]",
                "test_c[(-0-3j)]",
            ]
        );
    }

    #[test]
    fn surrogates_in_f_strings_are_refined() {
        for value in [
            r#"f"\ud800""#,
            r#""x" f"\udc00""#,
            r#"f"\ufffd""#,
            r#"f"a{'b'}\udfff""#,
        ] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"s\", [{value}])\ndef test_s(s):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{value}");
        }
        assert_eq!(
            exact("import pytest\n@pytest.mark.parametrize(\"s\", [f\"\\u00e9\", \"x\" f\"y\"])\ndef test_s(s):\n    pass\n").unwrap(),
            ["test_s[\\xe9]", "test_s[xy]"]
        );
    }

    #[test]
    fn dict_keys_are_deduplicated_as_python_does() {
        for (keys, expected) in [
            (r#"{"a": 1, "a": 2, "b": 3}"#, &["a", "b"][..]),
            (r#"{1: "x", True: "y", 1.0: "z", 2: "w"}"#, &["1", "2"]),
            ("{0: 1, False: 2, 0j: 3, -0.0: 4}", &["0"]),
            ("{True: 1, 1: 2}", &["True"]),
            (
                r#"{3: 1, "3": 2, b"3": 3, 3.5: 4, (1+2j): 5, (1+2j): 6}"#,
                &["3_0", "3_1", "3_2", "3.5", "(1+2j)"],
            ),
        ] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"k\", {keys})\ndef test_k(k):\n    pass\n"
            );
            let expected: Vec<String> = expected.iter().map(|id| format!("test_k[{id}]")).collect();
            assert_eq!(exact(&source), Some(expected), "{keys}");
        }
        // Keys that can't be compared statically.
        for keys in [
            "{float(\"nan\"): 1}",
            "{(1, 2): 1}",
            "{len: 1}",
            "{10**20: 1}",
        ] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"k\", {keys})\ndef test_k(k):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{keys}");
        }
    }

    #[test]
    fn ranges_reaching_the_limits_of_i64() {
        for (range, expected) in [
            (
                "9223372036854775806, 9223372036854775807, 2",
                "9223372036854775806",
            ),
            (
                "4611686018427387903, 4611686018427387904, 4611686018427387905",
                "4611686018427387903",
            ),
            (
                "-9223372036854775807, -9223372036854775808, -5",
                "-9223372036854775807",
            ),
        ] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"v\", range({range}))\ndef test_r(v):\n    pass\n"
            );
            assert_eq!(
                exact(&source),
                Some(vec![format!("test_r[{expected}]")]),
                "{range}"
            );
        }
    }

    #[test]
    fn values_that_raise_at_import_are_refined() {
        let huge_hex = format!("0x{}", "f".repeat(4000));
        let long_decimal = "9".repeat(700);
        let huge_int = format!("1{}", "0".repeat(400));
        let float_of_huge = format!("float({huge_int})");
        let refined = [
            r#"float("1._5")"#,
            r#"float("1_e5")"#,
            r#"float("1e_5")"#,
            r#"float("1_.5")"#,
            r#"float("+-5")"#,
            r#"float("--5")"#,
            r#"float("1e")"#,
            r#"float("١")"#,
            &float_of_huge,
            r#"re.compile("(")"#,
            r#"re.compile(r"\p")"#,
            r#"re.compile(r"a**")"#,
            r#"re.compile(r"[z-a]")"#,
            r#"re.compile("a", re.X)"#,
            r#"re.compile(b"\u00e9")"#,
            "ExceptionGroup",
            "BaseExceptionGroup",
            "PythonFinalizationError",
            &huge_hex,
            &long_decimal,
        ];
        for value in refined {
            let source = format!(
                "import pytest\nimport re\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{value}");
        }
        assert_eq!(
            exact("import pytest\nimport re\n@pytest.mark.parametrize(\"v\", [float(\" 1_000.25e-1_0 \"), float(\"-inf\"), float(\".5\"), float(\"5.\"), re.compile(r\"^a[b-d]+\\d{2,3}(?:x|y)?(?P<n>z)$\"), re.compile(\"a\", re.I | re.S), EnvironmentError])\ndef test_v(v):\n    pass\n").unwrap(),
            [
                "test_v[1.00025e-07]",
                "test_v[-inf]",
                "test_v[0.5]",
                "test_v[5.0]",
                "test_v[^a[b-d]+\\\\d{2,3}(?:x|y)?(?P<n>z)$]",
                "test_v[a]",
                "test_v[OSError]",
            ]
        );
    }

    /// Values aye reads as having no id of their own (argname + index),
    /// but which pytest fails to build while it imports the module.
    const RAISING: &[&str] = &[
        "range(0, 3, 0)",
        "range(1.5)",
        "range(\"a\")",
        "range()",
        "range(1, 2, 3, 4)",
        "range(0, 3, False)",
        "tuple(5)",
        "list(5)",
        "set([[1]])",
        "frozenset([[1]])",
        "{[1]: 2}",
        "{{1}}",
        "{(1, [2])}",
        "decimal.Decimal(\"x\")",
        "decimal.Decimal(\"1.2.3\")",
        "pathlib.Path(1)",
        "uuid.UUID(\"xyz\")",
        "datetime.date(2023, 2, 29)",
        "fractions.Fraction(1, 0)",
        "[*1]",
        "{**1}",
        "{**[1]}",
        "dict(1)",
        "dict([1])",
        "dict([(1, 2, 3)])",
        "slice()",
        "bytearray(\"a\")",
        "bytearray([256])",
        "object(1)",
        "(x for x in 1)",
        "lambda x=1/0: x",
        "[x for x in 5]",
        "[1] + (2,)",
        "[[1/0]]",
    ];

    #[test]
    fn values_built_by_known_calls_are_checked() {
        for value in RAISING {
            let source = format!(
                "import datetime, decimal, fractions, pathlib, uuid\nimport pytest\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{value}");
        }
        // Argument values themselves: a list and a tuple don't add up, and
        // a dict's values are evaluated too.
        for values in ["[1] + (2,)", "{1: 1/0}", "range(2) + [1]"] {
            let source = format!(
                "import pytest\n@pytest.mark.parametrize(\"v\", {values})\ndef test_v(v):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{values}");
        }
        // A call aye evaluates that fails at import, outside parametrize.
        assert_eq!(
            exact(
                "import pytest\nX = range(0, 3, 0)\n@pytest.mark.parametrize(\"v\", [1])\ndef test_v(v):\n    pass\n"
            ),
            None
        );
        // What pytest accepts stays exact (`constructed` in the suites
        // checks the ids against pytest).
        assert_eq!(
            exact(
                "import pytest\nX = range(3)\nY = float(\"1.5\")\n@pytest.mark.parametrize(\"v\", [range(-1), dict(a=1), {1: 2, **{3: 4}}, [*range(2)]])\ndef test_v(v):\n    pass\n"
            ),
            Some(vec![
                "test_v[v0]".to_string(),
                "test_v[v1]".to_string(),
                "test_v[v2]".to_string(),
                "test_v[v3]".to_string(),
            ])
        );
    }

    #[test]
    fn values_of_changed_modules_are_refined() {
        // Each changes `re.compile`, which `[re.compile("ab")]` reads.
        for setup in [
            "import re as r\ndel r.compile",
            "import re\nre.compile, X = None, 1",
            "import re\n[re][0].compile = None",
            "import re\nimport helpers\nhelpers.re.compile = None",
            "import re\nimport importlib\nimportlib.import_module(\"re\").compile = None",
            "import re\n__import__(\"re\").compile = None",
            "import re\nimport sys\nsys.modules.get(\"re\").compile = None",
            "import re\nfrom mock import patch\n@patch.object(re, \"compile\")\ndef helper(m):\n    pass",
            "import re\nimport sys\nsys.modules[sys.argv[0]].compile = None",
            "import re\ndef f():\n    global re\nf()",
        ] {
            let source = format!(
                "import pytest\n{setup}\n@pytest.mark.parametrize(\"v\", [re.compile(\"ab\")])\ndef test_v(v):\n    pass\n"
            );
            assert_eq!(exact(&source), None, "{setup}");
        }
        // pytest's own API, which parametrize and marks are read through.
        assert_eq!(
            exact("import pytest\npytest.param = None\ndef test_v():\n    pass\n"),
            None
        );
        // Other attributes, other modules, and code that runs later don't
        // count.
        assert_eq!(
            exact(
                "import pytest\nimport re\nimport os\npytest.expected_config = 1\nre.extra = 2\nos.sep2 = 3\ndef later():\n    re.compile = None\n@pytest.mark.parametrize(\"v\", [re.compile(\"ab\"), *range(1)])\ndef test_v(v):\n    re.compile = None\n"
            ),
            Some(vec!["test_v[ab]".to_string(), "test_v[0]".to_string()])
        );
    }

    /// Every value of [`RAISING`] does make pytest's import fail.
    #[test]
    fn raising_values_raise_in_pytest() {
        let Some(python) = python() else {
            eprintln!("skipped: no Python with pytest");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
        for (i, value) in RAISING.iter().enumerate() {
            std::fs::write(
                dir.path().join(format!("test_r{i}.py")),
                format!(
                    "import datetime, decimal, fractions, pathlib, uuid\nimport pytest\n@pytest.mark.parametrize(\"v\", [{value}])\ndef test_v(v):\n    pass\n"
                ),
            )
            .unwrap();
        }
        let output = Command::new(python)
            .args([
                "-m",
                "pytest",
                "--collect-only",
                "-q",
                "-p",
                "no:cacheprovider",
            ])
            .current_dir(dir.path())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env_remove("PYTEST_ADDOPTS")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        for (i, value) in RAISING.iter().enumerate() {
            assert!(
                stdout.contains(&format!("ERROR test_r{i}.py")),
                "{value} doesn't raise:\n{stdout}"
            );
        }
    }

    #[test]
    fn floats_parse_as_python_parses_them() {
        for (text, expected) in [
            ("1.5", Some(1.5)),
            ("  -2e3\n", Some(-2000.0)),
            ("1_000.000_1", Some(1000.0001)),
            (".5", Some(0.5)),
            ("5.", Some(5.0)),
            ("+1E+05", Some(100_000.0)),
            ("007", Some(7.0)),
            ("Infinity", Some(f64::INFINITY)),
            ("-iNf", Some(f64::NEG_INFINITY)),
            ("", None),
            (".", None),
            ("e5", None),
            ("1e", None),
            ("1e+", None),
            ("_1", None),
            ("1_", None),
            ("1__0", None),
            ("1_.5", None),
            ("1._5", None),
            ("1_e5", None),
            ("1e_5", None),
            ("+-5", None),
            ("--5", None),
            ("0x10", None),
            ("1 5", None),
            ("infinit", None),
            ("nan(1)", None),
        ] {
            assert_eq!(parse_float(text), expected, "{text:?}");
        }
        assert!(parse_float("-nan").is_some_and(f64::is_nan));
    }

    /// Patterns `regex_compiles` accepts; each compiles, without warnings,
    /// on Python 3.10 to 3.14.
    const GOOD_PATTERNS: &[&str] = &[
        "",
        "abc",
        "a|b|",
        "^a.b$",
        r"\d+\s*\w?\D\S\W",
        r"\.\*\(\)\[\]\{\}\\\-\ ",
        r"a{2}b{1,}c{,3}d{2,5}?e*?f+?g??",
        "a{}b{x}c{1,2",
        r"[abc][^abc][a-z0-9_][\d\s][-a][a-][]a][^]a]",
        r"[\x00-\x7f\u00e9\n\t\\]",
        r"(a)(?:b)(?P<name>c)(?=d)(?!e)()",
        r"\x41\u00e9\U0001F600\0\012\a\f\v\b",
        "é+",
    ];

    /// Patterns it refuses: errors, warnings, or syntax it doesn't follow.
    const BAD_PATTERNS: &[&str] = &[
        "(",
        ")",
        "a)",
        "*a",
        "a**",
        "a*+",
        "a{2}{3}",
        "a???",
        "|*",
        "^*",
        r"\b+",
        "[",
        "[]",
        "[z-a]",
        r"[a-\d]",
        "[[a]",
        "[a--b]",
        "[a&&b]",
        "[a||b]",
        "[a~~b]",
        r"\p",
        r"\1",
        r"\z",
        r"\x4",
        r"\u12",
        r"\N{LATIN SMALL LETTER A}",
        "\\",
        "(?i)a",
        "(?<=a)b",
        "(?#c)",
        "(?P=n)",
        "(?P<1>a)",
        "(?P<n>a)(?P<n>b)",
        "(?>a)",
        "a{3,2}",
        "a{99999}",
        "(?=a)*",
    ];

    #[test]
    fn regex_patterns_compile_when_aye_says_so() {
        for pattern in GOOD_PATTERNS {
            assert!(
                regex_compiles(&Value::Str((*pattern).to_string())),
                "{pattern:?}"
            );
        }
        for pattern in BAD_PATTERNS {
            assert!(
                !regex_compiles(&Value::Str((*pattern).to_string())),
                "{pattern:?}"
            );
        }
        assert!(regex_compiles(&Value::Bytes(b"[\x00-\xff]+".to_vec())));
        assert!(!regex_compiles(&Value::Bytes(br"\u00e9".to_vec())));
    }

    /// The interpreter for differential tests: `AYE_TEST_PYTHON`, or the
    /// workspace's `.venv`, when it has pytest.
    fn python() -> Option<PathBuf> {
        let candidate = std::env::var_os("AYE_TEST_PYTHON")
            .map(PathBuf::from)
            .or_else(|| {
                let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv");
                [venv.join("bin/python"), venv.join("Scripts/python.exe")]
                    .into_iter()
                    .find(|p| p.is_file())
            })?;
        let ok = Command::new(&candidate)
            .args(["-c", "import pytest"])
            .output()
            .is_ok_and(|o| o.status.success());
        ok.then_some(candidate)
    }

    #[test]
    fn regex_patterns_compile_in_python() {
        let Some(python) = python() else {
            eprintln!("skipped: no Python with pytest");
            return;
        };
        let script = r#"
import json, re, sys, warnings
warnings.simplefilter("error")
for pattern in json.loads(sys.stdin.read()):
    try:
        re.compile(pattern)
    except Exception as error:
        print(f"{pattern!r}: {error}")
"#;
        let mut child = Command::new(python)
            .args(["-c", script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let input = serde_json::to_string(GOOD_PATTERNS).unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), input.as_bytes()).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    }

    #[test]
    fn static_values_match_pytest() {
        let Some(python) = python() else {
            eprintln!("skipped: no Python with pytest");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pytest.ini"), "[pytest]\n").unwrap();
        let mut expected = Vec::new();
        for (name, source, ids) in UNCHANGED {
            std::fs::write(dir.path().join(format!("test_{name}.py")), source).unwrap();
            expected.extend(ids.iter().map(|id| format!("test_{name}.py::{id}")));
        }
        let output = Command::new(python)
            .args([
                "-m",
                "pytest",
                "--collect-only",
                "-q",
                "-p",
                "no:cacheprovider",
            ])
            .current_dir(dir.path())
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env_remove("PYTEST_ADDOPTS")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut collected: Vec<&str> = stdout.lines().filter(|l| l.contains("::")).collect();
        collected.sort_unstable();
        expected.sort_unstable();
        assert_eq!(collected, expected, "{stdout}");
    }
}
