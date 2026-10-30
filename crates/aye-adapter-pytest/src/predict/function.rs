//! pytest's `istestfunction`, and `Metafunc.parametrize` for each test
//! function: the items it makes.

use std::collections::HashSet;

use aye_adapter::ast::{Expr, ExprLambda, StmtClassDef, StmtFunctionDef};
use aye_adapter::{Ranged, TextRange};
use aye_events::Certainty;
use aye_python_semantic::flow::is_generator;
use aye_python_semantic::namespace::{Binding, Bound, Flag};
use aye_python_semantic::resolve::Lookup;

use super::ModuleRecognizer;
use super::case::Item;
use super::class::Frame;
use super::closure::Closure;
use super::reorder::ParamKey;
use crate::conftest::default_argnames;
use crate::marks::ids::parameterset_ids;
use crate::marks::{self, Decorator, Mark, Scoped, SetTimeout, decorator};

/// Something callable bound to a test name.
#[derive(Clone, Copy)]
pub(super) enum Callable<'a> {
    Def(&'a StmtFunctionDef),
    Lambda(&'a ExprLambda, TextRange),
}

impl Callable<'_> {
    pub(super) fn span(&self) -> TextRange {
        match self {
            Self::Def(def) => def.name.range(),
            Self::Lambda(_, name) => *name,
        }
    }
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    /// The names in a function's `__dict__` that `-k` matches.
    pub(super) fn attributes(
        &self,
        name: &str,
        callable: Callable<'a>,
        site: Option<&'a StmtClassDef>,
        lookup: Lookup<'s, 'a>,
    ) -> Vec<String> {
        let mut out: Vec<String> = match callable {
            Callable::Def(def) => decorator_attributes(def, lookup)
                .into_iter()
                .map(str::to_string)
                .collect(),
            Callable::Lambda(..) => Vec::new(),
        };
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        for attribute in ns.attributes.get(name).into_iter().flatten() {
            if !out.iter().any(|a| a == attribute) {
                out.push((*attribute).to_string());
            }
        }
        out
    }

    /// `istestfunction` and `_genfunctions` for one callable.
    pub(super) fn function(
        &mut self,
        name: &str,
        callable: Callable<'a>,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
        test_attr: Flag,
    ) {
        if !self.is_test_function_name(name) && test_attr != Flag::Set(true) {
            if test_attr == Flag::Computed {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
            }
            return;
        }
        match test_attr {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let is_method = !frames.is_empty();
        let mut fn_marks = Vec::new();
        let mut certainty = Certainty::Exact;
        // Handed to code aye can't follow (`mark(test)`, `disable(test)`,
        // `vars(test)[…] = …`): its marks or `__test__` may change.
        let own_name = match callable {
            Callable::Def(def) => def.name.id.as_str(),
            Callable::Lambda(..) => name,
        };
        let handed = if site.is_some() {
            self.module.attribute_handed(name) || self.module.attribute_handed(own_name)
        } else {
            self.module.handed(name) || self.module.handed(own_name)
        };
        if handed {
            self.doubt(format!(
                "{name} is handed to code aye can't follow, which may change it"
            ));
            certainty = Certainty::Partial;
        }
        let mut staticmethod = false;
        // Whether something other than marks and `staticmethod` or
        // `classmethod` wraps the function: pytest then sees the wrapper.
        let mut wrapped = false;
        if let Callable::Def(def) = callable {
            let lookup = self.lookup(site, def.start());
            let mut opaque = false;
            for d in def.decorator_list.iter().rev() {
                match decorator(lookup, &d.expression) {
                    Decorator::Mark(mark) => fn_marks.push(mark),
                    Decorator::Fixture(_) | Decorator::Property | Decorator::AbstractProperty => {
                        return;
                    }
                    Decorator::StaticMethod => staticmethod = true,
                    Decorator::ClassMethod => {}
                    Decorator::Unknown => {
                        opaque = true;
                        wrapped = true;
                    }
                    Decorator::Keeps => {
                        certainty = Certainty::Partial;
                        wrapped = true;
                    }
                    _ => wrapped = true,
                }
            }
            if opaque {
                // It may parametrize, mark, rename, replace or multiply
                // the test.
                certainty = Certainty::Partial;
                self.doubt(format!("{name} has a decorator aye can't see through"));
            }
            if !def.is_async && is_generator(&def.body) {
                if wrapped {
                    // pytest checks the decorated object, which a wrapper
                    // (`functools.wraps`, `inlineCallbacks`, `gen_test`)
                    // makes a plain function.
                    self.doubt(format!(
                        "{name} is a generator wrapped by a decorator; pytest decides whether it is a test"
                    ));
                    certainty = Certainty::Partial;
                } else {
                    // pytest fails the module's collection ("'yield'
                    // keyword is allowed in fixtures, but not in tests");
                    // refinement reports its error, once.
                    self.doubt(format!(
                        "{name} is a generator, which pytest reports as a collection error"
                    ));
                    return;
                }
            }
        }
        // `test.pytestmark = …` replaces the marks the decorators left.
        let ns = site.map_or(self.module, |c| self.class_ns(c));
        if ns
            .attributes
            .get(name)
            .is_some_and(|names| names.contains(&"pytestmark"))
        {
            certainty = Certainty::Partial;
            self.doubt(format!("{name}.pytestmark is assigned"));
        }
        // Marks that could not be read may parametrize.
        if self.module_marks_unknown {
            self.doubt("the module's pytestmark can't be read");
        }
        if let Some(frame) = frames.iter().rev().find(|f| f.marks_unknown) {
            self.doubt(format!("{}'s marks can't be read", frame.name));
        }
        let own = fn_marks.len();
        let attributes = self.attributes(
            name,
            callable,
            site,
            self.lookup(site, callable.span().start()),
        );
        let mut chain = fn_marks.clone();
        for frame in frames.iter().rev() {
            chain.extend(frame.marks.iter().cloned());
        }
        chain.extend(self.module_marks.iter().cloned());
        if self.module_marks_unknown {
            certainty = Certainty::Partial;
        }
        if self.marking_hooks() {
            certainty = Certainty::Partial;
        }

        let classes: Vec<String> = frames.iter().map(|f| f.name.clone()).collect();
        let base = Item {
            classes: classes.clone(),
            name: name.to_string(),
            originalname: name.to_string(),
            span: callable.span(),
            marks: chain.clone(),
            fn_marks: own,
            param_marks: Vec::new(),
            param_timeout: SetTimeout::Unmarked,
            attributes: attributes.clone(),
            certainty,
        };

        // Reasons ids can only be known at runtime.
        let mut runtime: Option<String> = None;
        if self.scope.hooks.contains("pytest_generate_tests") {
            runtime = Some(format!(
                "{} implements pytest_generate_tests",
                self.scope.hook_source("pytest_generate_tests")
            ));
        }
        if let Some(Bound {
            binding: Binding::Function(_) | Binding::Value(_) | Binding::Import(_),
            ..
        }) = self.module.get("pytest_generate_tests")
        {
            runtime = Some("the module defines pytest_generate_tests".into());
        }
        if let Some(frame) = frames.last()
            && self
                .lookup_mro(&frame.mro, "pytest_generate_tests")
                .is_some()
        {
            runtime = Some(format!("{} defines pytest_generate_tests", frame.name));
        }

        let parametrize_marks: Vec<&Mark<'s, 'a>> =
            chain.iter().filter(|m| m.name == "parametrize").collect();
        if !parametrize_marks.is_empty()
            && self
                .scope
                .global_hooks
                .contains("pytest_make_parametrize_id")
        {
            runtime = Some(format!(
                "{} implements pytest_make_parametrize_id",
                self.scope.global_hook_source("pytest_make_parametrize_id")
            ));
        }

        let mut parsed = Vec::new();
        if runtime.is_none() {
            for mark in &parametrize_marks {
                match marks::parametrize(mark, &self.settings.empty) {
                    Ok(p) => parsed.push(p),
                    Err(reason) => {
                        runtime = Some(format!("{name}: {reason}"));
                        break;
                    }
                }
            }
        }

        // The fixture closure: parametrized fixtures make ids pytest's
        // business; arguments parametrize does not cover are errors.
        if runtime.is_none() {
            let args = match callable {
                Callable::Def(def) => {
                    crate::conftest::funcargnames(def, is_method && !staticmethod)
                }
                Callable::Lambda(lambda, _) => lambda_args(lambda, is_method),
            };
            let defaults = match callable {
                Callable::Def(def) => default_argnames(def),
                Callable::Lambda(..) => Vec::new(),
            };
            let covered: HashSet<&str> = parsed
                .iter()
                .flat_map(|p| p.argnames.iter().map(String::as_str))
                .collect();
            let direct: HashSet<&str> = parsed
                .iter()
                .flat_map(|p| p.direct().map(String::as_str))
                .collect();
            let (closure, verdict) = self.closure(&args, &chain, &direct, &covered, frames);
            match verdict {
                Closure::Fine => {}
                // A fixture imported from elsewhere may be parametrized.
                Closure::Imported => certainty = Certainty::Partial,
                Closure::Unreadable => {
                    runtime = Some(format!("{name}: usefixtures arguments are not literal"));
                }
                Closure::Parametrized(fixture) => {
                    runtime = Some(format!("{name} uses the parametrized fixture {fixture}"));
                }
                Closure::Opaque(fixture) => {
                    runtime = Some(format!(
                        "{name} uses {fixture}, a fixture defined in a way aye can't read"
                    ));
                }
            }
            if runtime.is_none() {
                let mut seen_args: HashSet<&str> = HashSet::new();
                'check: for p in &parsed {
                    for argname in &p.argnames {
                        if !seen_args.insert(argname.as_str()) {
                            runtime =
                                Some(format!("{name}: duplicate parametrization of {argname:?}"));
                            break 'check;
                        }
                        if !closure.contains(argname.as_str()) && !args.contains(argname) {
                            runtime = Some(if defaults.contains(argname) {
                                format!(
                                    "{name} already takes an argument {argname:?} with a default value"
                                )
                            } else {
                                format!("{name} uses no argument {argname:?}")
                            });
                            break 'check;
                        }
                    }
                }
            }
        }

        // Each parametrization's scope (`_find_parametrized_scope`):
        // explicit, else for an indirect-only one the narrowest of its
        // fixtures'. Class and module scopes make pytest reorder the file's
        // tests (`reorder_items`), which `reorder` models; package and
        // session scopes reorder across files.
        let mut scopes: Vec<Scoped> = Vec::new();
        if runtime.is_none() {
            let defs = self.fixture_defs(frames);
            for p in &parsed {
                let indirect_all =
                    !p.indirect.is_empty() && p.argnames.iter().all(|a| p.indirect.contains(a));
                let scope = match p.scope {
                    Some(scope) => scope,
                    None if indirect_all => p
                        .argnames
                        .iter()
                        .map(|a| {
                            defs.iter()
                                .rev()
                                .find(|d| d.name == *a)
                                .map_or(Scoped::Unknown, |d| d.scope)
                        })
                        .min()
                        .unwrap_or(Scoped::Function),
                    None => Scoped::Function,
                };
                if matches!(scope, Scoped::Package | Scoped::Session | Scoped::Unknown) {
                    runtime = Some(format!(
                        "{name}: parameters of a session-wide or unknown scope reorder tests at runtime"
                    ));
                    break;
                }
                scopes.push(scope);
            }
        }

        if let Some(reason) = runtime {
            self.doubt(reason);
            let mut item = base;
            item.certainty = Certainty::Partial;
            self.push(&item);
            return;
        }

        // `Metafunc.parametrize`, once per mark, each multiplying the calls.
        let mut calls: Vec<Call> = vec![(Vec::new(), Vec::new(), SetTimeout::Unmarked, Vec::new())];
        let mut any = false;
        let mut unknown_marks = false;
        let class_key = (!classes.is_empty()).then(|| self.nodeid(&classes));
        for (p, scope) in parsed.iter().zip(&scopes) {
            any = true;
            unknown_marks |= p.unknown_marks;
            let ids =
                match parameterset_ids(&p.argnames, &p.sets, p.ids.as_deref(), self.settings.ids) {
                    Ok(ids) => ids,
                    Err(error) => {
                        self.doubt(format!("{name}: {error}"));
                        let mut item = base;
                        item.certainty = Certainty::Partial;
                        self.push(&item);
                        return;
                    }
                };
            let mut next = Vec::with_capacity(calls.len() * p.sets.len());
            for (idlist, marks, timeout, keys) in &calls {
                for (index, ((id, set), set_timeout)) in
                    ids.iter().zip(&p.sets).zip(&p.timeouts).enumerate()
                {
                    let mut idlist = idlist.clone();
                    if let Some(id) = id {
                        idlist.push(id.clone());
                    }
                    let mut marks = marks.clone();
                    marks.extend(set.marks.iter().cloned());
                    let timeout = if *timeout == SetTimeout::Unmarked {
                        *set_timeout
                    } else {
                        *timeout
                    };
                    let mut keys = keys.clone();
                    if matches!(scope, Scoped::Class | Scoped::Module) {
                        for argname in &p.argnames {
                            let key = ParamKey {
                                scope: *scope,
                                argname: argname.clone(),
                                index,
                                class: (*scope == Scoped::Class)
                                    .then(|| class_key.clone())
                                    .flatten(),
                            };
                            keys.push((key, !p.indirect.contains(argname)));
                        }
                    }
                    next.push((idlist, marks, timeout, keys));
                }
            }
            calls = next;
        }
        // `_recompute_direct_params_indices`: a direct parameter's index is
        // its call's position, so only tests at the same position share it.
        for (position, call) in calls.iter_mut().enumerate() {
            for (key, direct) in &mut call.3 {
                if *direct {
                    key.index = position;
                }
            }
        }
        if !any {
            let mut item = base;
            item.certainty = certainty;
            self.push(&item);
            return;
        }
        for (idlist, param_marks, param_timeout, keys) in calls {
            let full = if idlist.is_empty() {
                name.to_string()
            } else {
                format!("{name}[{}]", idlist.join("-"))
            };
            let item = Item {
                classes: classes.clone(),
                name: full,
                originalname: name.to_string(),
                span: callable.span(),
                marks: chain.clone(),
                fn_marks: own,
                param_marks,
                param_timeout,
                attributes: attributes.clone(),
                certainty: if unknown_marks {
                    Certainty::Partial
                } else {
                    certainty
                },
            };
            self.push(&item);
            if let Some(last) = self.keys.last_mut() {
                *last = keys.into_iter().map(|(key, _)| key).collect();
            }
        }
    }
}

/// What decorators other than marks leave in a function's `__dict__`
/// (which `-k` matches): `__wrapped__` from `functools.wraps`, which
/// nearly all of them use, and `patchings` from `mock.patch`.
fn decorator_attributes<'a>(
    func: &'a StmtFunctionDef,
    lookup: Lookup<'_, 'a>,
) -> Vec<&'static str> {
    let mut out = Vec::new();
    for d in &func.decorator_list {
        match decorator(lookup, &d.expression) {
            Decorator::Mark(_) | Decorator::StaticMethod | Decorator::ClassMethod => {}
            _ => {
                if !out.contains(&"__wrapped__") {
                    out.push("__wrapped__");
                }
                let callee = match &d.expression {
                    Expr::Call(call) => &*call.func,
                    other => other,
                };
                let path = lookup.path(callee).unwrap_or_default();
                let is_patch = ["mock.patch", "unittest.mock.patch"].iter().any(|p| {
                    path == *p || path == format!("{p}.object") || path == format!("{p}.multiple")
                });
                if is_patch && !out.contains(&"patchings") {
                    out.push("patchings");
                }
            }
        }
    }
    out
}

/// A call `Metafunc.parametrize` builds: the ids, the parameter sets'
/// marks, their timeout, and its parameters of class or module scope, each
/// with whether it is direct (not through a fixture).
type Call = (Vec<String>, Vec<String>, SetTimeout, Vec<(ParamKey, bool)>);

fn lambda_args(lambda: &ExprLambda, is_method: bool) -> Vec<String> {
    let Some(params) = &lambda.parameters else {
        return Vec::new();
    };
    let mut names: Vec<String> = params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_none())
        .map(|p| p.parameter.name.id.to_string())
        .collect();
    if is_method && !names.is_empty() {
        names.remove(0);
    }
    names
}
