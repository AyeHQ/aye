//! Recognition of one test module, the way pytest's `Module`, `Class` and
//! `UnitTestCase` collectors walk it (`_pytest/python.py`,
//! `_pytest/unittest.py`), without importing it.

mod case;
mod class;
mod closure;
mod function;
mod module;
mod reorder;
mod scope;
mod unittest;

use std::collections::{HashMap, HashSet};

use aye_adapter::ast::{Stmt, StmtClassDef};
use aye_adapter::{Ranged, Recognition, SourceFile, TextRange, TextSize};
use aye_events::{Case, Certainty, Diagnostic, DiagnosticCode, Severity};
use aye_python_semantic::flow::{BodyFacts, conditional_deletes, guarded_by_try};
use aye_python_semantic::namespace::Namespace;
use aye_python_semantic::resolve::Lookup;
use aye_python_stdlib::fnmatch::matches_prefix_or_glob;

pub(crate) use scope::{Imports, ModuleImports, Scope, names_module, reached};

use crate::conftest::{FixtureDef, MARKING_HOOKS, fixture_of};
use crate::marks::ids::IdOptions;
use crate::marks::{EmptyMark, Mark};
use crate::{PYTEST, SETS};
use reorder::{ParamKey, reorder_items};

/// The configuration recognition depends on.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub python_classes: Vec<String>,
    pub python_functions: Vec<String>,
    pub ids: IdOptions,
    pub empty: EmptyMark,
    pub collect_imported_tests: bool,
    /// The `usefixtures` ini option.
    pub usefixtures: Vec<String>,
    /// `-p no:unittest`: `TestCase` classes are plain classes to pytest.
    pub unittest_blocked: bool,
    /// `-p no:python`: nothing collects Python files.
    pub python_blocked: bool,
}

/// Recognize one test module. `module` is its namespace
/// ([`Namespace::build`] over its body).
pub(crate) fn recognize<'a>(
    settings: &Settings,
    scope: &Scope,
    imports: &dyn Imports,
    file: &SourceFile<'a>,
    module: &Namespace<'a>,
    path_keywords: &[String],
) -> Recognition {
    let mut classes = HashMap::new();
    class_namespaces(&file.module.body, None, &mut classes);
    let empty = Namespace::default();
    let mut recognizer = ModuleRecognizer {
        settings,
        scope,
        imports,
        file,
        module,
        classes: &classes,
        empty: &empty,
        path_keywords,
        cases: Vec::new(),
        keys: Vec::new(),
        facts: std::cell::RefCell::default(),
        diagnostics: Vec::new(),
        refine: Vec::new(),
        module_marks: Vec::new(),
        module_marks_unknown: false,
        module_fixtures: Vec::new(),
        class_fixtures: HashMap::new(),
        module_wide: false,
        partial_names: HashSet::new(),
        sets_names: classes
            .values()
            .any(|info| info.ns.get("__set_name__").is_some()),
    };
    recognizer.run();
    if recognizer.module_wide || !recognizer.partial_names.is_empty() {
        let prefix = format!("{}::", file.rel);
        for case in &mut recognizer.cases {
            let name = case.id.as_str().strip_prefix(&prefix).unwrap_or_default();
            let name = name.split("::").next().unwrap_or_default();
            let name = name.split('[').next().unwrap_or_default();
            if (recognizer.module_wide || recognizer.partial_names.contains(name))
                && case.certainty == Certainty::Exact
            {
                case.certainty = Certainty::Partial;
            }
        }
    }
    if recognizer.keys.iter().any(|k| !k.is_empty()) {
        let order = reorder_items(&recognizer.keys);
        let mut cases: Vec<Option<Case>> = recognizer.cases.drain(..).map(Some).collect();
        recognizer.cases = order.into_iter().filter_map(|i| cases[i].take()).collect();
    }
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for diagnostic in recognizer.diagnostics {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by_key(|d| d.span.as_ref().map_or(0, |s| s.start));
    let mut refine: Vec<String> = Vec::new();
    for reason in recognizer.refine {
        if !refine.contains(&reason) {
            refine.push(reason);
        }
    }
    Recognition {
        cases: recognizer.cases,
        refine: (!refine.is_empty()).then(|| refine.join("; ").into()),
        diagnostics,
    }
}

/// A class body's namespace, and the class body its statement sits in.
struct ClassInfo<'a> {
    def: &'a StmtClassDef,
    ns: Namespace<'a>,
    site: Option<&'a StmtClassDef>,
}

fn class_namespaces<'a>(
    body: &'a [Stmt],
    site: Option<&'a StmtClassDef>,
    out: &mut HashMap<TextSize, ClassInfo<'a>>,
) {
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class) => {
                out.insert(
                    class.start(),
                    ClassInfo {
                        def: class,
                        ns: Namespace::build(&class.body, PYTEST),
                        site,
                    },
                );
                class_namespaces(&class.body, Some(class), out);
            }
            Stmt::If(s) => {
                class_namespaces(&s.body, site, out);
                for clause in &s.elif_else_clauses {
                    class_namespaces(&clause.body, site, out);
                }
            }
            Stmt::Try(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
                class_namespaces(&s.finalbody, site, out);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        class_namespaces(&handler.body, site, out);
                    }
                }
            }
            Stmt::With(s) => class_namespaces(&s.body, site, out),
            Stmt::For(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::While(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::Match(s) => {
                for case in &s.cases {
                    class_namespaces(&case.body, site, out);
                }
            }
            _ => {}
        }
    }
}

struct ModuleRecognizer<'s, 'a> {
    settings: &'s Settings,
    scope: &'s Scope,
    imports: &'s dyn Imports,
    file: &'s SourceFile<'a>,
    module: &'s Namespace<'a>,
    classes: &'s HashMap<TextSize, ClassInfo<'a>>,
    /// Stands in for a class body that was not walked (never expected).
    empty: &'s Namespace<'a>,
    path_keywords: &'s [String],
    cases: Vec<Case>,
    /// Each case's class- and module-scoped parameters, for `reorder_items`.
    keys: Vec<Vec<ParamKey>>,
    /// [`ModuleRecognizer::body_facts`], by class (`None` for the module).
    facts: std::cell::RefCell<HashMap<Option<TextSize>, std::rc::Rc<BodyFacts<'a>>>>,
    diagnostics: Vec<Diagnostic>,
    refine: Vec<String>,
    module_marks: Vec<Mark<'s, 'a>>,
    module_marks_unknown: bool,
    /// Fixtures defined in the module, in order.
    module_fixtures: Vec<FixtureDef>,
    /// Fixtures defined in each class body, by class.
    class_fixtures: HashMap<TextSize, Vec<FixtureDef>>,
    /// A reason that holds for the whole module was found: none of its
    /// cases is exact.
    module_wide: bool,
    /// Module names whose tests may not be what the source shows (bound
    /// or deleted through `global`): their cases are partial.
    partial_names: HashSet<String>,
    /// Some class of the module defines `__set_name__`, which Python calls
    /// with the class that holds an instance as an attribute.
    sets_names: bool,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    fn lookup(&self, class: Option<&'a StmtClassDef>, at: TextSize) -> Lookup<'s, 'a> {
        Lookup {
            module: self.module,
            class: class.map(|c| self.class_ns(c)),
            source: self.file.source,
            at,
        }
    }

    fn class_ns(&self, class: &StmtClassDef) -> &'s Namespace<'a> {
        self.classes
            .get(&class.start())
            .map_or(self.empty, |info| &info.ns)
    }

    fn end(&self) -> TextSize {
        TextSize::try_from(self.file.source.len()).unwrap_or(TextSize::new(u32::MAX))
    }

    /// Whether some hook may add marks to this file's tests.
    fn marking_hooks(&self) -> bool {
        MARKING_HOOKS
            .iter()
            .any(|hook| self.scope.hooks.contains(*hook))
            || self
                .scope
                .global_hooks
                .contains("pytest_collection_modifyitems")
    }

    /// Whether a name bound to `path` may be a unittest case of the
    /// project, which pytest collects whatever its name: a capitalized
    /// name (as classes are) from a project file, unless unittest is off.
    fn may_be_project_unittest(&self, name: &str, path: &str) -> bool {
        !self.settings.unittest_blocked
            && name.chars().next().is_some_and(char::is_uppercase)
            && self.imports.project_unittest(path)
    }

    fn is_test_function_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_functions, name, SETS)
    }

    fn is_test_class_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_classes, name, SETS)
    }

    /// What a module (`None`) or class body may skip at runtime, worked out
    /// once per body.
    fn body_facts(&self, site: Option<&'a StmtClassDef>) -> std::rc::Rc<BodyFacts<'a>> {
        let key = site.map(Ranged::start);
        if let Some(found) = self.facts.borrow().get(&key) {
            return std::rc::Rc::clone(found);
        }
        let body = site.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let facts = std::rc::Rc::new(BodyFacts {
            deletes: conditional_deletes(body),
            guarded: guarded_by_try(body),
        });
        self.facts
            .borrow_mut()
            .insert(key, std::rc::Rc::clone(&facts));
        facts
    }

    fn doubt(&mut self, reason: impl Into<String>) {
        self.refine.push(reason.into());
    }

    fn diagnostic(
        &mut self,
        severity: Severity,
        code: DiagnosticCode,
        range: TextRange,
        message: String,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            code,
            path: self.file.rel.to_project_path().into(),
            span: Some(self.file.span(range)),
            message,
        });
    }

    fn nodeid(&self, classes: &[String]) -> String {
        let mut id = self.file.rel.to_string();
        for class in classes {
            id.push_str("::");
            id.push_str(class);
        }
        id
    }

    /// Fixtures defined directly in a namespace, as [`fixture_of`] reads
    /// them (alternative branches, call forms and aliases included).
    fn fixtures_in(&self, ns: &Namespace<'a>, class: Option<&'a StmtClassDef>) -> Vec<FixtureDef> {
        let lookup = self.lookup(class, self.end());
        let body = class.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let guarded = guarded_by_try(body);
        ns.iter()
            .filter_map(|(name, _)| fixture_of(lookup, ns, name, class.is_some(), &guarded))
            .collect()
    }
}
