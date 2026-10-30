//! A test's fixture closure, and what it says about the test's ids.

use std::collections::HashSet;

use aye_adapter::Ranged;
use aye_python_semantic::class::Klass;

use super::ModuleRecognizer;
use super::class::Frame;
use crate::conftest::{BUILTIN_FIXTURES, FixtureDef};
use crate::marks::{self, Mark};

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    /// The fixtures visible to a test, in registration order: plugins,
    /// conftests, the module, then each class (bases first). The last
    /// definition of a name is the closest.
    pub(super) fn fixture_defs(&self, frames: &[Frame<'s, 'a>]) -> Vec<&FixtureDef> {
        let mut defs: Vec<&FixtureDef> = self.scope.fixtures.iter().collect();
        defs.extend(&self.module_fixtures);
        for frame in frames {
            for klass in frame.mro.iter().rev() {
                if let Klass::Local(def) = klass {
                    defs.extend(self.class_fixtures.get(&def.start()).into_iter().flatten());
                }
            }
        }
        defs
    }

    /// The names in a test's fixture closure that this file and its
    /// conftests define, and what that says about its ids.
    pub(super) fn closure(
        &self,
        args: &[String],
        chain: &[Mark<'s, 'a>],
        direct: &HashSet<&str>,
        covered: &HashSet<&str>,
        frames: &[Frame<'s, 'a>],
    ) -> (HashSet<String>, Closure) {
        let defs = self.fixture_defs(frames);
        let mut verdict = Closure::Fine;
        let mut roots: Vec<String> = self.settings.usefixtures.clone();
        roots.extend(defs.iter().filter(|d| d.autouse).map(|d| d.name.clone()));
        for mark in chain.iter().filter(|m| m.name == "usefixtures") {
            let (names, unknown) = marks::usefixtures(mark);
            roots.extend(names);
            if unknown {
                verdict = Closure::Unreadable;
            }
        }
        roots.extend(args.iter().cloned());

        // A fixture registered under a name aye can't read may be any of
        // them.
        let any_name = defs.iter().any(|d| d.any_name);
        let mut closure: HashSet<String> = HashSet::new();
        let mut queue: Vec<String> = roots;
        while let Some(name) = queue.pop() {
            if !closure.insert(name.clone()) || name == "request" {
                continue;
            }
            if direct.contains(name.as_str()) {
                continue;
            }
            if any_name {
                return (closure, Closure::Opaque(name));
            }
            let matching: Vec<&FixtureDef> =
                defs.iter().copied().filter(|d| d.name == name).collect();
            if matching.is_empty() {
                // Defined by a plugin a conftest in scope loads from a
                // directory it puts on `sys.path` aye can't tell: it may be
                // a parametrized fixture.
                if self.scope.hidden_fixtures && !BUILTIN_FIXTURES.contains(&name.as_str()) {
                    return (closure, Closure::Opaque(name));
                }
                // Not defined where aye can read it: imported by the
                // module or a conftest in scope from somewhere else, it may
                // be a parametrized fixture.
                let imports = &self.scope.module_imports;
                let imported = imports.maybe.contains(&name)
                    || imports.any
                    || self.scope.maybe_fixtures.contains(&name)
                    || self.scope.any_fixture;
                if imported && matches!(verdict, Closure::Fine) {
                    verdict = Closure::Imported;
                }
                continue;
            }
            // Closest first; a definition that does not request the name
            // it overrides hides the ones further out.
            for def in matching.into_iter().rev() {
                if def.opaque && !covered.contains(name.as_str()) {
                    return (closure, Closure::Opaque(name));
                }
                if def.params && !covered.contains(name.as_str()) {
                    return (closure, Closure::Parametrized(name));
                }
                queue.extend(def.args.iter().filter(|a| **a != name).cloned());
                // One that may not be defined at runtime doesn't hide the
                // ones further out.
                if !def.args.contains(&name) && !def.conditional {
                    break;
                }
            }
        }
        (closure, verdict)
    }
}

/// What a fixture closure says about a test's ids.
pub(super) enum Closure {
    Fine,
    /// It uses a name imported from another module, maybe a fixture.
    Imported,
    /// `usefixtures` names it with something other than literals.
    Unreadable,
    Parametrized(String),
    /// A fixture in the closure is defined in a way aye can't read.
    Opaque(String),
}
