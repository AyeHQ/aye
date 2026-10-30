//! pytest's `UnitTestCase` collector: `TestLoader.getTestCaseNames`.

use std::collections::BTreeSet;

use aye_adapter::Ranged;
use aye_adapter::ast::{Expr, StmtClassDef};
use aye_events::Certainty;
use aye_python_semantic::class::Klass;
use aye_python_semantic::flow::maybe_deleted;
use aye_python_semantic::namespace::{Binding, Bound, Flag};
use aye_python_semantic::resolve::Resolved;
use aye_python_stdlib::unittest::RUNTEST_BASES;

use super::ModuleRecognizer;
use super::case::Item;
use super::class::Frame;
use super::function::Callable;
use super::module::is_literal;
use crate::marks::{Decorator, SetTimeout, decorator};

/// What an attribute of a unittest class is to `getTestCaseNames`.
enum Attr<'a> {
    Test(Callable<'a>),
    NotTest,
    Unknown,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    /// pytest's `UnitTestCase` collector: `TestLoader.getTestCaseNames`.
    pub(super) fn unittest_class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        frames: &[Frame<'s, 'a>],
        mro: &[Klass<'a>],
    ) {
        match self.dunder_test(mro) {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let (marks, unknown, opaque) = match self.class_marks(mro) {
            Ok(found) => found,
            Err(reason) => {
                self.doubt(reason);
                (Vec::new(), true, false)
            }
        };
        let members_unknown = match self.dynamic_members(mro, opaque) {
            Some(reason) => {
                self.doubt(reason);
                true
            }
            None => false,
        };
        let before = self.cases.len();
        self.duplicates(self.class_ns(class));
        let mut frames = frames.to_vec();
        frames.push(Frame {
            name: name.to_string(),
            mro: mro.to_vec(),
            marks,
            // parametrize does not apply to unittest methods: unknown marks
            // only make tags uncertain.
            marks_unknown: false,
        });
        let mut names: BTreeSet<&'a str> = BTreeSet::new();
        for klass in mro {
            if let Klass::Local(def) = klass {
                let ns = self.class_ns(def);
                for (member, _) in ns.iter() {
                    if member.starts_with("test") {
                        names.insert(member);
                    }
                }
                for (member, _) in maybe_deleted(ns, &def.body) {
                    if member.starts_with("test") && names.insert(member) {
                        self.doubt(format!("{name}.{member} may be deleted at runtime"));
                    }
                }
            }
        }
        let mut found = Vec::new();
        for member in names {
            let Some((bound, def)) = self.lookup_mro(mro, member) else {
                continue;
            };
            match self.unittest_callable(member, &bound, def) {
                Attr::Test(callable) => found.push((member, callable, def)),
                Attr::NotTest => {}
                Attr::Unknown => self.doubt(format!("{name}.{member} is bound at runtime")),
            }
        }
        if found.is_empty()
            && let Some((bound, def)) = self.lookup_mro(mro, "runTest")
            && let Attr::Test(callable) = self.unittest_callable("runTest", &bound, def)
        {
            found.push(("runTest", callable, def));
        }
        // `unittest.FunctionTestCase` brings a `runTest` of its own.
        if found.is_empty()
            && self.lookup_mro(mro, "runTest").is_none()
            && mro.iter().any(
                |k| matches!(k, Klass::External(path) if RUNTEST_BASES.contains(&path.as_str())),
            )
        {
            let certainty = if unknown || self.module_marks_unknown || self.marking_hooks() {
                Certainty::Partial
            } else {
                Certainty::Exact
            };
            let mut all = Vec::new();
            for frame in frames.iter().rev() {
                all.extend(frame.marks.iter().cloned());
            }
            all.extend(self.module_marks.iter().cloned());
            let item = Item {
                classes: frames.iter().map(|f| f.name.clone()).collect(),
                name: "runTest".to_string(),
                originalname: "runTest".to_string(),
                span: class.name.range(),
                marks: all,
                fn_marks: 0,
                param_marks: Vec::new(),
                param_timeout: SetTimeout::Unmarked,
                attributes: Vec::new(),
                certainty,
            };
            self.push(&item);
        }
        for (member, callable, def) in found {
            let lookup = self.lookup(Some(def), callable.span().start());
            let mut fn_marks = Vec::new();
            let mut certainty = if unknown {
                Certainty::Partial
            } else {
                Certainty::Exact
            };
            if let Callable::Def(func) = callable {
                let mut opaque = false;
                for d in func.decorator_list.iter().rev() {
                    match decorator(lookup, &d.expression) {
                        Decorator::Mark(mark) => fn_marks.push(mark),
                        Decorator::Unknown => opaque = true,
                        Decorator::Keeps => certainty = Certainty::Partial,
                        _ => {}
                    }
                }
                if opaque {
                    // `parameterized.expand` and the like replace the method
                    // with generated ones.
                    certainty = Certainty::Partial;
                    self.doubt(format!(
                        "{name}.{member} has a decorator aye can't see through"
                    ));
                }
            }
            if self
                .class_ns(def)
                .attributes
                .get(member)
                .is_some_and(|names| names.contains(&"pytestmark"))
            {
                certainty = Certainty::Partial;
            }
            let own = fn_marks.len();
            let mut all = fn_marks;
            for frame in frames.iter().rev() {
                all.extend(frame.marks.iter().cloned());
            }
            all.extend(self.module_marks.iter().cloned());
            if self.module_marks_unknown || self.marking_hooks() {
                certainty = Certainty::Partial;
            }
            let item = Item {
                classes: frames.iter().map(|f| f.name.clone()).collect(),
                name: member.to_string(),
                originalname: member.to_string(),
                span: callable.span(),
                marks: all,
                fn_marks: own,
                param_marks: Vec::new(),
                param_timeout: SetTimeout::Unmarked,
                attributes: self.attributes(member, callable, Some(def), lookup),
                certainty,
            };
            self.push(&item);
        }
        if members_unknown {
            for case in &mut self.cases[before..] {
                case.certainty = Certainty::Partial;
            }
        }
    }

    /// Whether a unittest attribute is a callable test.
    fn unittest_callable(&self, name: &str, bound: &Bound<'a>, def: &'a StmtClassDef) -> Attr<'a> {
        let ns = self.class_ns(def);
        match ns.test_attr(name) {
            Flag::Set(false) => return Attr::NotTest,
            Flag::Computed => return Attr::Unknown,
            _ => {}
        }
        let lookup = self.lookup(Some(def), bound.range.start());
        match &bound.binding {
            Binding::Function(func) => {
                let property = func.decorator_list.iter().any(|d| {
                    matches!(
                        decorator(lookup, &d.expression),
                        Decorator::Property | Decorator::AbstractProperty
                    )
                });
                if property {
                    Attr::NotTest
                } else {
                    Attr::Test(Callable::Def(func))
                }
            }
            Binding::Value(Expr::Lambda(lambda)) => {
                Attr::Test(Callable::Lambda(lambda, bound.name_range))
            }
            Binding::Value(expr) if is_literal(expr) => Attr::NotTest,
            Binding::Value(expr) => match lookup.resolve(expr) {
                Resolved::Local(Bound {
                    binding: Binding::Function(func),
                    conditional: false,
                    ..
                }) => Attr::Test(Callable::Def(func)),
                _ => Attr::Unknown,
            },
            // A nested class is callable too, and imports could be anything.
            _ => Attr::Unknown,
        }
    }
}
