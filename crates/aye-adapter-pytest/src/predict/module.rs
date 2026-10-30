//! pytest's `Module` collector: the module-wide reasons to refine, and
//! `pytest_pycollect_makeitem` for each name a namespace binds.

use std::collections::HashMap;

use aye_adapter::TextRange;
use aye_adapter::ast::{Expr, StmtClassDef};
use aye_events::{Certainty, DiagnosticCode, Severity};
use aye_python_semantic::flow::{exclusive, import_time_raise, import_time_skip, maybe_deleted};
use aye_python_semantic::namespace::{Binding, Bound, Flag, Namespace};
use aye_python_semantic::resolve::Resolved;
use aye_python_stdlib::unittest::RUNTEST_BASES;

use super::ModuleRecognizer;
use super::class::Frame;
use super::function::Callable;
use super::scope::reached;
use crate::conftest::{REFINING_HOOKS, no_tests};

/// Attributes every module, class and instance has; pytest never collects
/// them.
pub(super) const IGNORED_ATTRIBUTES: &[&str] = &[
    "__builtins__",
    "__cached__",
    "__class__",
    "__delattr__",
    "__dict__",
    "__dir__",
    "__doc__",
    "__eq__",
    "__file__",
    "__firstlineno__",
    "__format__",
    "__ge__",
    "__getattribute__",
    "__getstate__",
    "__gt__",
    "__hash__",
    "__init__",
    "__init_subclass__",
    "__le__",
    "__loader__",
    "__lt__",
    "__module__",
    "__name__",
    "__ne__",
    "__new__",
    "__package__",
    "__qualname__",
    "__reduce__",
    "__reduce_ex__",
    "__repr__",
    "__setattr__",
    "__sizeof__",
    "__spec__",
    "__static_attributes__",
    "__str__",
    "__subclasshook__",
    "__weakref__",
];

/// Whether changing the module `path` names may change what aye reads of
/// pytest: its API (`pytest.param`, `pytest.mark`), or the internals that
/// collect and make ids. Other internals (`_pytest.doctest`, reporting)
/// don't.
fn changes_collection(path: &str) -> bool {
    let mut parts = path.split('.');
    match (parts.next(), parts.next()) {
        (Some("pytest" | "_pytest"), None) => true,
        (Some("pytest"), Some(name)) => matches!(
            name,
            "mark" | "param" | "HIDDEN_PARAM" | "fixture" | "skip" | "importorskip"
        ),
        (Some("_pytest"), Some(module)) => matches!(
            module,
            "python"
                | "python_api"
                | "mark"
                | "fixtures"
                | "compat"
                | "main"
                | "nodes"
                | "unittest"
        ),
        _ => false,
    }
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    pub(super) fn run(&mut self) {
        let module = self.module;
        let end = self.end();
        let lookup = self.lookup(None, end);
        self.module_fixtures = self.fixtures_in(module, None);
        self.module_fixtures
            .extend(self.scope.module_imports.fixtures.iter().cloned());
        let classes = self.classes;
        let mut class_fixtures = HashMap::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills a map: the order shows nowhere"
        )]
        for (start, info) in classes {
            let class = self.class_def(*start);
            class_fixtures.insert(*start, self.fixtures_in(&info.ns, class));
        }
        self.class_fixtures = class_fixtures;

        if let Some(bound) = module.get("__test__") {
            let value = match bound.binding {
                Binding::Value(expr) => lookup.at(bound.range.end()).boolean(expr),
                _ => None,
            };
            match value {
                Some(true) => {}
                Some(false) => return,
                None => {
                    self.doubt("the module's __test__ is computed at runtime");
                    return;
                }
            }
        }
        let reached = reached(module, self.scope, self.file.rel.as_str());
        if !reached.is_empty() {
            for reason in reached {
                self.doubt(reason);
            }
        } else if module.reflective {
            self.doubt(
                "the module reaches module namespaces at runtime (sys.modules, frames or gc)",
            );
        } else if module.dynamic {
            self.doubt("the module binds names at runtime (globals(), setattr or exec)");
        }
        if let Some(patched) = module
            .patched_modules()
            .chain(&self.scope.patched)
            .find(|m| changes_collection(m))
        {
            self.doubt(format!(
                "{patched} is changed at runtime, which may change how pytest collects"
            ));
        }
        if let Some(call) = import_time_skip(&self.file.module.body, lookup) {
            self.doubt(format!("the module may skip itself at import ({call})"));
        }
        if let Some(what) = import_time_raise(&self.file.module.body, lookup) {
            self.doubt(format!("{what} may raise while the module is imported"));
        }
        if self.settings.collect_imported_tests {
            for source in &module.star_imports {
                if !no_tests(source) {
                    self.doubt(format!("`from {source} import *` may bring in tests"));
                } else if !self.settings.unittest_blocked
                    && matches!(source.as_str(), "unittest" | "unittest.case")
                {
                    // `FunctionTestCase` brings a `runTest`, so pytest
                    // collects it wherever it is bound.
                    self.doubt(format!(
                        "`from {source} import *` binds FunctionTestCase, which pytest collects"
                    ));
                }
            }
        }
        match self.pytestmark(module, None) {
            Ok(list) => {
                self.module_marks = list.marks;
                self.module_marks_unknown = list.unknown;
            }
            Err(reason) => self.doubt(format!("module {reason}")),
        }
        for hook in REFINING_HOOKS {
            if self.scope.hooks.contains(hook) {
                self.doubt(format!(
                    "{} implements {hook}",
                    self.scope.hook_source(hook)
                ));
            }
        }
        if self.scope.global_hooks.contains("pytest_collection") {
            self.doubt(format!(
                "{} implements pytest_collection",
                self.scope.global_hook_source("pytest_collection")
            ));
        }
        if self.scope.skips_at_import {
            self.doubt("a conftest.py in scope may skip at import");
        }
        if self.scope.collect_file_touches_py {
            self.doubt("a conftest.py in scope implements pytest_collect_file for Python files");
        }
        if self.scope.hooks_unknown {
            self.doubt("a conftest.py in scope loads plugins aye can't read");
        }
        if self.scope.module_plugins {
            self.doubt("the module loads plugins through pytest_plugins");
        }
        // Each reason so far may change any of the module's tests.
        self.module_wide = !self.refine.is_empty();
        // A function that runs at import and binds or deletes a module
        // name through `global`: the name may be a test, or no longer one.
        for (name, value) in module.global_writes() {
            let unknown = self.is_test_function_name(name)
                || self.is_test_class_name(name)
                || value.is_none_or(|value| !is_literal(value));
            if unknown {
                self.doubt(format!(
                    "{name} is bound or deleted through `global` by a function that runs at import"
                ));
                self.partial_names.insert((*name).to_string());
            }
        }

        self.duplicates(module);
        let mut entries: Vec<(&'a str, Bound<'a>)> =
            module.iter().map(|(n, b)| (n, b.clone())).collect();
        entries.extend(maybe_deleted(module, &self.file.module.body));
        for (name, bound) in entries {
            if IGNORED_ATTRIBUTES.contains(&name) {
                continue;
            }
            self.member(name, &bound, None, &[], true);
        }
    }

    /// `pytest_pycollect_makeitem` for one name of a module or class
    /// namespace. `site` is the class whose body bound the name.
    pub(super) fn member(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) {
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        let bindings = ns.bindings(name);
        let mut alternatives: Vec<&Bound<'a>> = bindings
            .iter()
            .filter(|earlier| {
                earlier.range.start() < bound.range.start() && exclusive(earlier, bound)
            })
            .collect();
        if bound.conditional
            && let Some(previous) = bindings
                .iter()
                .rev()
                .find(|b| b.range.start() < bound.range.start() && !exclusive(b, bound))
        {
            alternatives.push(previous);
        }
        let before = self.cases.len();
        let facts = self.body_facts(site);
        // A `del` that may not run leaves the name bound or not.
        let deleted = facts
            .deletes
            .iter()
            .any(|(n, at)| *n == name && *at > bound.range.start());
        let guarded = facts.guarded.contains(&bound.range.start());
        if deleted {
            self.member_binding(name, bound, site, frames, in_module);
            if self.cases.len() > before {
                self.doubt(format!("{name} may be deleted at runtime"));
                for case in &mut self.cases[before..] {
                    case.certainty = Certainty::Partial;
                }
            }
            return;
        }
        if alternatives.is_empty() {
            self.member_binding(name, bound, site, frames, in_module);
            // Bound in a branch that may not run (`if`, `for`, `match`,
            // `except`, or a `try` body after a statement that may raise):
            // whether the test exists is known at runtime.
            if (bound.conditional || guarded) && self.cases.len() > before {
                self.doubt(format!(
                    "{name} is defined conditionally; whether it exists is known at runtime"
                ));
                for case in &mut self.cases[before..] {
                    case.certainty = Certainty::Partial;
                }
            }
            return;
        }
        // Which binding runs is known at runtime: fine when they all make
        // the same tests.
        let outcomes: Vec<Option<Vec<String>>> = alternatives
            .iter()
            .map(|alternative| self.trial(name, alternative, site, frames, in_module))
            .collect();
        let chosen = self.trial(name, bound, site, frames, in_module);
        if chosen.is_none() || outcomes.iter().any(|o| *o != chosen) {
            self.doubt(format!(
                "{name} is defined in alternative branches; which one runs is known at runtime"
            ));
        }
        self.member_binding(name, bound, site, frames, in_module);
    }

    /// The ids a binding would make, without keeping them; `None` when it
    /// raises doubts of its own.
    fn trial(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) -> Option<Vec<String>> {
        let (cases, refine, diagnostics) =
            (self.cases.len(), self.refine.len(), self.diagnostics.len());
        self.member_binding(name, bound, site, frames, in_module);
        let ids: Vec<String> = self.cases[cases..]
            .iter()
            .map(|c| c.id.as_str().to_owned())
            .collect();
        let clean = self.refine.len() == refine;
        self.cases.truncate(cases);
        self.keys.truncate(cases);
        self.refine.truncate(refine);
        self.diagnostics.truncate(diagnostics);
        clean.then_some(ids)
    }

    fn member_binding(
        &mut self,
        name: &str,
        bound: &Bound<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        in_module: bool,
    ) {
        let lookup = self.lookup(site, bound.range.start());
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        let test_attr = ns.test_attr(name);
        match &bound.binding {
            Binding::Function(def) => {
                self.function(name, Callable::Def(def), site, frames, test_attr);
            }
            Binding::Class(class) => self.class(name, class, site, frames),
            Binding::Value(expr) => match expr {
                Expr::Lambda(lambda) => {
                    self.function(
                        name,
                        Callable::Lambda(lambda, bound.name_range),
                        site,
                        frames,
                        test_attr,
                    );
                }
                Expr::Name(_) | Expr::Attribute(_) => {
                    let wanted = self.is_test_function_name(name)
                        || self.is_test_class_name(name)
                        || test_attr == Flag::Set(true);
                    match lookup.resolve(expr) {
                        Resolved::Local(target) if !target.conditional => match target.binding {
                            Binding::Function(def) => {
                                self.function(name, Callable::Def(def), site, frames, test_attr);
                            }
                            Binding::Class(class) => self.class(name, class, site, frames),
                            Binding::Value(Expr::Lambda(lambda)) => self.function(
                                name,
                                Callable::Lambda(lambda, target.name_range),
                                site,
                                frames,
                                test_attr,
                            ),
                            _ if wanted && !is_literal(expr) => {
                                self.doubt(format!("{name} is bound at runtime"));
                            }
                            _ => {}
                        },
                        Resolved::Path(path) if wanted => {
                            if (!in_module || self.settings.collect_imported_tests)
                                && !no_tests(&path)
                            {
                                self.doubt(format!(
                                    "{name} is bound to {path}, which may be a test"
                                ));
                            }
                        }
                        Resolved::Path(path) => {
                            if !in_module || self.settings.collect_imported_tests {
                                if self.may_be_project_unittest(name, &path) {
                                    self.doubt(format!(
                                        "{name} is bound to {path}, which may be a unittest case"
                                    ));
                                } else if self.imports.project_test(&path) {
                                    self.doubt(format!(
                                        "{name} is bound to {path}, whose __test__ may be true"
                                    ));
                                }
                            }
                        }
                        _ if wanted => self.doubt(format!("{name} is bound at runtime")),
                        _ => {}
                    }
                }
                other => {
                    let class_name = self.is_test_class_name(name);
                    let wanted = class_name
                        || self.is_test_function_name(name)
                        || matches!(test_attr, Flag::Set(true) | Flag::Computed);
                    // pytest collects a bound object only if it is a
                    // function (or a class, for a class name).
                    let instance = !class_name && self.makes_instance(lookup, other);
                    if wanted && !is_literal(other) && !instance {
                        self.doubt(format!("{name} is bound at runtime"));
                    } else if !wanted && let Some(callee) = self.may_make_class(lookup, other) {
                        // A unittest case, or a class whose `__test__` is
                        // true: pytest collects it whatever its name.
                        self.doubt(format!(
                            "{name} is bound to what {callee}(…) returns, which may be a test class"
                        ));
                    }
                }
            },
            Binding::Import(path) => {
                if in_module && !self.settings.collect_imported_tests {
                    return;
                }
                let wanted = self.is_test_function_name(name)
                    || self.is_test_class_name(name)
                    || name.ends_with("TestCase");
                // `FunctionTestCase` brings a `runTest`, so pytest collects
                // it wherever it is bound.
                if (wanted && !no_tests(path)) || RUNTEST_BASES.contains(&path.as_str()) {
                    self.doubt(format!("{name} is imported from {path} and may be a test"));
                } else if self.may_be_project_unittest(name, path) {
                    // pytest collects a unittest case whatever its name.
                    self.doubt(format!(
                        "{name} is imported from {path}, which may be a unittest case"
                    ));
                } else if !wanted && self.imports.project_test(path) {
                    // And anything whose `__test__` is true.
                    self.doubt(format!(
                        "{name} is imported from {path}, whose __test__ may be true"
                    ));
                }
            }
            Binding::Other => {
                if self.is_test_function_name(name) || self.is_test_class_name(name) {
                    self.doubt(format!("{name} is bound at runtime"));
                }
            }
        }
    }

    /// `duplicate-definition` warnings for test names bound twice.
    pub(super) fn duplicates(&mut self, ns: &'s Namespace<'a>) {
        let mut reported: Vec<(TextRange, String)> = Vec::new();
        for (name, _) in ns.iter() {
            let defs: Vec<&Bound<'a>> = ns
                .bindings(name)
                .iter()
                .filter(|b| matches!(b.binding, Binding::Function(_) | Binding::Class(_)))
                .collect();
            if defs.len() < 2 {
                continue;
            }
            let is_test = match defs.last().map(|b| &b.binding) {
                Some(Binding::Function(_)) => self.is_test_function_name(name),
                Some(Binding::Class(_)) => self.is_test_class_name(name),
                _ => false,
            };
            if !is_test {
                continue;
            }
            let last = defs[defs.len() - 1];
            let line = self.file.line(last.name_range.start());
            let deletions = ns.deletions.get(name).map_or(&[][..], Vec::as_slice);
            for earlier in &defs[..defs.len() - 1] {
                let deleted = deletions
                    .iter()
                    .any(|at| *at > earlier.range.start() && *at < last.range.start());
                if deleted || exclusive(earlier, last) {
                    continue;
                }
                reported.push((
                    earlier.name_range,
                    format!("{name} is defined again on line {line}; pytest only runs the last definition"),
                ));
            }
        }
        for (range, message) in reported {
            self.diagnostic(
                Severity::Warning,
                DiagnosticCode::DuplicateDefinition,
                range,
                message,
            );
        }
    }
}

/// Whether an expression is a literal that is not callable.
pub(super) fn is_literal(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::FString(_)
            | Expr::List(_)
            | Expr::Tuple(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::ListComp(_)
            | Expr::SetComp(_)
            | Expr::DictComp(_)
            | Expr::Generator(_)
            | Expr::Compare(_)
    )
}
