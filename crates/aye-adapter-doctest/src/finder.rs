//! What `doctest.DocTestFinder().find(module, module.__name__)` returns for
//! a module, read from its AST: one test per docstring with examples, on the
//! module, its functions and classes, and recursively their methods,
//! properties and nested classes, named by the keys of the namespaces that
//! hold them (`pkg.mod.Class.method`).
//!
//! The finder walks `module.__dict__` and each class's `__dict__`, so what
//! counts is what those namespaces hold once the module is imported. Wherever
//! the source doesn't say that for sure, the file is refined: definitions
//! that may not run, names bound twice, decorators that may replace an
//! object, aliases and wrappers of objects with doctests (the finder visits
//! an object once, under the first name its namespace lists, and a wrapper
//! is a new object), code that changes a namespace at runtime (`setattr`,
//! `globals()`, `exec`, `__doc__` and `__module__` assignments, `type()`
//! with three arguments, functions doing so that run at import), `__test__`,
//! metaclasses and `__init_subclass__`, and docstrings `doctest` can't
//! parse.

mod alias;
mod checks;
mod namespace;
mod tables;

use std::collections::{HashMap, HashSet};

use aye_adapter::ast::{self, Expr, Stmt};
use aye_adapter::{Ranged, SourceFile, TextRange};

use crate::parser;

use namespace::{
    Flow, Imports, Kind, Namespace, collect_from_imports, defaults, docstring_of, names_skip,
    record_imports,
};
use tables::{SKIPPERS, SPECIAL_CLASS_NAMES, SPECIAL_MODULE_NAMES};

/// One test the finder makes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    /// `pkg.mod.Class.method`.
    pub name: String,
    /// The docstring.
    pub range: TextRange,
}

/// What the finder makes of a module.
#[derive(Debug, Clone, Default)]
pub(crate) struct Finding {
    /// Sorted by name, as `DocTestFinder.find` sorts them.
    pub tests: Vec<Found>,
    /// Why the tests can only be known by importing the module.
    pub refine: Option<String>,
    /// Namespace-level `from … import …` statements: their level and module.
    pub from_imports: Vec<(u32, Option<String>)>,
}

/// Reads a module's docstrings.
pub(crate) struct Finder<'f, 'a> {
    file: &'f SourceFile<'a>,
    /// The file has carriage returns: line endings to fold.
    has_cr: bool,
    reasons: Vec<String>,
    imports: Imports,
    /// Module functions and classes (the last definition of each name,
    /// wherever it is), for calls made at import time.
    module_defs: HashMap<&'a str, Kind<'a>>,
}

impl<'f, 'a> Finder<'f, 'a> {
    pub(crate) fn new(file: &'f SourceFile<'a>) -> Self {
        Self {
            file,
            has_cr: file.source.contains('\r'),
            reasons: Vec::new(),
            imports: Imports::default(),
            module_defs: HashMap::new(),
        }
    }

    fn refine(&mut self, reason: String) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }

    /// A docstring's value as Python sees it, line endings folded.
    fn doc_value(&mut self, literal: &ast::ExprStringLiteral, owner: &str) -> Option<String> {
        let value = literal.value.to_str();
        if !value.contains(">>>") {
            return None;
        }
        if !(value.contains('\r') && self.has_cr) {
            return Some(value.to_string());
        }
        // A carriage return may come from a line ending, which Python folds,
        // or from an escape, which it keeps.
        let escapes = literal
            .value
            .iter()
            .any(|part| !part.flags.prefix().is_raw() && self.file.text(part.range).contains('\\'));
        if escapes {
            self.refine(format!(
                "the docstring of `{owner}` has escapes and the file has carriage returns"
            ));
        }
        Some(parser::universal_newlines(value))
    }

    /// Whether `body`'s docstring has examples (refining when unsure).
    fn has_examples(&mut self, body: &[Stmt], owner: &str) -> Option<TextRange> {
        let literal = docstring_of(body)?;
        let value = self.doc_value(literal, owner)?;
        let doc = parser::docstring(&value);
        if let Some(why) = doc.unsure {
            self.refine(format!("the docstring of `{owner}`: {why}"));
        }
        doc.examples.then(|| literal.range())
    }

    /// The module's tests, named from `module` (its `__name__`).
    pub(crate) fn find(mut self, module: &str) -> Finding {
        let body = &self.file.module.body;
        self.read_imports(body);
        let mut tests = Vec::new();
        if let Some(range) = self.has_examples(body, module) {
            tests.push(Found {
                name: module.to_string(),
                range,
            });
        }
        let mut from_imports = Vec::new();
        collect_from_imports(body, &mut from_imports);
        let outer = HashSet::new();
        let (found, bearing) = self.namespace(module, body, false, &outer);
        tests.extend(found);
        self.whole_file(&bearing);
        tests.sort_by(|a, b| a.name.cmp(&b.name));
        Finding {
            tests,
            refine: (!self.reasons.is_empty()).then(|| self.reasons.join("; ")),
            from_imports,
        }
    }

    /// Every import in the module, wherever it is, for resolving
    /// decorators: a name some import binds and nothing else does.
    fn read_imports(&mut self, body: &'a [Stmt]) {
        let mut ns = Namespace::default();
        ns.gather(body, Flow::Always, false, &|_| false);
        let mut imported: HashSet<&str> = HashSet::new();
        for stmt in body {
            record_imports(stmt, &mut self.imports, &mut imported);
        }
        for binding in &ns.bindings {
            if !imported.contains(binding.name) {
                self.imports.other.insert(binding.name.to_string());
            }
            if let Kind::Def(_) | Kind::Class(_) = binding.kind {
                self.module_defs.insert(binding.name, binding.kind);
            }
        }
    }

    /// One namespace (the module's or a class's): its tests, and the names
    /// in it whose objects have doctests.
    fn namespace(
        &mut self,
        prefix: &str,
        body: &'a [Stmt],
        class: bool,
        outer: &HashSet<&'a str>,
    ) -> (Vec<Found>, HashSet<&'a str>) {
        let mut ns = Namespace::default();
        let imports = &self.imports;
        let overload =
            |e: &Expr| imports.is_one_of(e, &["typing.overload", "typing_extensions.overload"]);
        ns.gather(body, Flow::Always, class, &overload);
        // Each definition's tests, wherever it is.
        let mut tests_of: Vec<Vec<Found>> = Vec::with_capacity(ns.bindings.len());
        let mut bearing: HashSet<&'a str> = HashSet::new();
        for binding in &ns.bindings {
            let name = format!("{prefix}.{}", binding.name);
            let found = match binding.kind {
                Kind::Def(def) => self
                    .has_examples(&def.body, &name)
                    .map(|range| vec![Found { name, range }])
                    .unwrap_or_default(),
                Kind::Class(class_def) => {
                    let mut found: Vec<Found> = self
                        .has_examples(&class_def.body, &name)
                        .map(|range| Found {
                            name: name.clone(),
                            range,
                        })
                        .into_iter()
                        .collect();
                    let mut visible: HashSet<&'a str> = outer.clone();
                    visible.extend(bearing.iter().copied());
                    let (members, _) = self.namespace(&name, &class_def.body, true, &visible);
                    found.extend(members);
                    found
                }
                Kind::Accessor | Kind::Overload | Kind::Other => Vec::new(),
            };
            if !found.is_empty() {
                bearing.insert(binding.name);
            }
            tests_of.push(found);
        }
        let special: &[&str] = if class {
            &SPECIAL_CLASS_NAMES
        } else {
            &SPECIAL_MODULE_NAMES
        };
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for binding in &ns.bindings {
            if !matches!(binding.kind, Kind::Accessor | Kind::Overload) {
                *counts.entry(binding.name).or_default() += 1;
            }
            if special.contains(&binding.name) {
                self.refine(format!("`{prefix}` binds `{}`", binding.name));
            }
        }
        let mut tests = Vec::new();
        for (binding, found) in ns.bindings.iter().zip(&mut tests_of) {
            if found.is_empty() {
                continue;
            }
            let qualified = format!("{prefix}.{}", binding.name);
            if binding.flow == Flow::Maybe {
                self.refine(format!("`{qualified}` is defined conditionally"));
            }
            if counts.get(binding.name).copied().unwrap_or(0) > 1 {
                self.refine(format!("`{qualified}` is bound more than once"));
            }
            if ns.stars.iter().any(|star| *star > binding.at) {
                self.refine(format!("a star import may rebind `{qualified}`"));
            }
            self.check_decorators(binding, class, &qualified);
            if binding.flow == Flow::Always {
                tests.append(found);
            }
        }
        for binding in &ns.bindings {
            if let Kind::Class(class_def) = binding.kind {
                self.check_class(class_def, prefix);
            }
        }
        let mut visible: HashSet<&'a str> = outer.clone();
        visible.extend(bearing.iter().copied());
        self.check_expressions(&ns, prefix, &visible);
        if ns.raises {
            self.refine(format!("`{prefix}` raises while it is created"));
        }
        if ns.skips {
            self.refine(format!("`{prefix}` may raise a skip while it is created"));
        }
        (tests, bearing)
    }
}

/// Whether importing `module` may raise pytest's `skip`, as a conftest that
/// makes pytest skip the directory it is loaded for does: a call to one of
/// [`SKIPPERS`], or a `raise` of `Skipped` or `pytest.skip.Exception`, in
/// what runs at import (the module's statements in any branch, class
/// bodies, decorators, default values) or in a module function called by
/// name from there. `unittest.SkipTest` is no skip there: pytest fails the
/// directory.
pub(crate) fn may_skip_while_imported(module: &ast::ModModule) -> bool {
    struct AtImport<'a> {
        skips: bool,
        callees: Vec<&'a str>,
    }
    impl<'a> ast::visitor::Visitor<'a> for AtImport<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                // Only what makes the function runs now.
                Stmt::FunctionDef(def) => {
                    for decorator in &def.decorator_list {
                        self.visit_expr(&decorator.expression);
                    }
                    for default in defaults(&def.parameters) {
                        self.visit_expr(default);
                    }
                }
                Stmt::Raise(raise) => {
                    if raise
                        .exc
                        .as_deref()
                        .is_some_and(|exc| names_skip(exc) && !names_skip_test(exc))
                    {
                        self.skips = true;
                    }
                    ast::visitor::walk_stmt(self, stmt);
                }
                _ => ast::visitor::walk_stmt(self, stmt),
            }
        }

        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr {
                match call.func.as_ref() {
                    Expr::Name(name) => {
                        self.skips |= SKIPPERS.contains(&name.id.as_str());
                        self.callees.push(name.id.as_str());
                    }
                    Expr::Attribute(attr) => {
                        self.skips |= SKIPPERS.contains(&attr.attr.as_str());
                    }
                    _ => {}
                }
            }
            ast::visitor::walk_expr(self, expr);
        }
    }

    let mut functions: HashMap<&str, &[Stmt]> = HashMap::new();
    let mut ns = Namespace::default();
    ns.gather(&module.body, Flow::Always, false, &|_| false);
    for binding in &ns.bindings {
        if let Kind::Def(def) = binding.kind {
            functions.insert(binding.name, &def.body);
        }
    }
    let mut scan = AtImport {
        skips: false,
        callees: Vec::new(),
    };
    ast::visitor::Visitor::visit_body(&mut scan, &module.body);
    let mut scanned: HashSet<&str> = HashSet::new();
    while !scan.skips {
        let Some(callee) = scan.callees.pop() else {
            break;
        };
        if scanned.insert(callee)
            && let Some(body) = functions.get(callee)
        {
            ast::visitor::Visitor::visit_body(&mut scan, body);
        }
    }
    scan.skips
}

/// Whether a raised expression is unittest's `SkipTest`.
fn names_skip_test(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => names_skip_test(&call.func),
        Expr::Name(name) => name.id.as_str() == "SkipTest",
        Expr::Attribute(attr) => attr.attr.as_str() == "SkipTest",
        _ => false,
    }
}
