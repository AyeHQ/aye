//! What a support module's fixtures are, as far as parametrization is
//! concerned: each name's definitions, read as pytest's `parsefactories`
//! registers them, and what each requests.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **Fixtures are read from every binding that may be the final one.**
//!   For each name of a module, class or support namespace: the final
//!   binding, the bindings in exclusive branches before it (`if`/`else`,
//!   `try`/`except`, `match`), and the previous one when the final one may
//!   not run (`if` without `else`, a loop, a `try` body after a statement
//!   that may raise). Merged, the fixture is parametrized if any of them
//!   is, requests what any requests, and is `conditional`: it doesn't hide
//!   the definitions further out (a conftest's parametrized one stays in
//!   the closure). Besides `@pytest.fixture` functions, the call forms
//!   `fixture(…)(function)` and `fixture(function, …)` are read (name,
//!   parameters, scope, autouse and what the function requests) and an
//!   alias of a fixture (`alias = some_fixture`) is that fixture under its
//!   name, as pytest's `parsefactories` registers it. A name bound in a way
//!   aye can't read that may make a fixture (a decorator or a call it can't
//!   see through, `reg = register(reg)`, `held = _Holder.held`, unpacking)
//!   is an opaque fixture, and a computed `name=` (or a support module that
//!   binds names at runtime: `globals()`, `vars()`, `exec`, `setattr` on
//!   `sys.modules[…]`) makes one under any name. A test whose closure
//!   reaches an opaque fixture is refined, as one reaching a parametrized
//!   one is. A call to a class of the module that makes its own instances
//!   (unconditional and undecorated, with no metaclass, no `__new__`,
//!   nothing that changes it afterwards, and only such classes or `object`
//!   as bases), to a builtin, or to a function of a package that holds no
//!   tests makes no fixture: pytest registers only objects whose type is
//!   exactly its `FixtureFunctionDefinition`. A capitalized callee is not
//!   taken for a class (`backend = Param("a", "b")` may return a
//!   parametrized fixture), as for test names above; this was decided at
//!   integration, where the two rounds' rules differed, for Core decision 5.

use aye_adapter::ast::{Expr, ExprCall, Stmt, StmtFunctionDef};
use aye_adapter::{Ranged, TextSize};
use aye_python_semantic::flow::exclusive;
use aye_python_semantic::namespace::{Binding, Bound, Namespace};
use aye_python_semantic::resolve::{Lookup, Resolved};

use crate::marks::{Decorator, Scoped, decorator};

/// A fixture definition, as far as parametrization is concerned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FixtureDef {
    /// The name pytest registers it under.
    pub name: String,
    /// The attribute that holds it in its module: the function's name, or
    /// the name it was imported as.
    pub attribute: String,
    /// `name=` was given, so importing it under another name keeps it.
    pub explicit_name: bool,
    /// Defined with `params=`.
    pub params: bool,
    pub autouse: bool,
    pub scope: Scoped,
    /// The fixtures it requests.
    pub args: Vec<String>,
    /// Whether the name is a fixture at runtime depends on a branch that
    /// may not run, or on which of several definitions runs: it doesn't
    /// hide the definitions further out.
    pub conditional: bool,
    /// Bound in a way aye can't read (a decorator or a call it can't see
    /// through, an alias of something it can't follow): it may be a
    /// fixture, with any parameters, requesting anything.
    pub opaque: bool,
    /// `name=` is computed (or the module binds names at runtime): it may
    /// be registered under any name.
    pub any_name: bool,
}

impl FixtureDef {
    /// A plain fixture `name`, as pytest registers a function.
    pub(super) fn named(name: &str, args: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            attribute: name.to_string(),
            explicit_name: false,
            params: false,
            autouse: false,
            scope: Scoped::Function,
            args,
            conditional: false,
            opaque: false,
            any_name: false,
        }
    }

    /// Something bound to `name` that may be a fixture aye can't read.
    pub(super) fn opaque(name: &str) -> Self {
        Self {
            scope: Scoped::Unknown,
            opaque: true,
            ..Self::named(name, Vec::new())
        }
    }
}

/// pytest's own fixtures (and those of the plugins it ships), which are
/// never parametrized: a test may request them without any project file
/// defining them.
pub(crate) const BUILTIN_FIXTURES: &[&str] = &[
    "cache",
    "capfd",
    "capfdbinary",
    "caplog",
    "capsys",
    "capsysbinary",
    "capteesys",
    "doctest_namespace",
    "linecomp",
    "LineMatcher",
    "monkeypatch",
    "pytestconfig",
    "pytester",
    "record_property",
    "record_testsuite_property",
    "record_xml_attribute",
    "recwarn",
    "request",
    "subtests",
    "testdir",
    "testrun_uid",
    "tmp_path",
    "tmp_path_factory",
    "tmpdir",
    "tmpdir_factory",
    "worker_id",
];

/// `getfuncargnames`: positional-or-keyword and keyword-only parameters
/// without defaults, minus `self`/`cls` for methods.
pub(crate) fn funcargnames(def: &StmtFunctionDef, drop_first: bool) -> Vec<String> {
    let params = &def.parameters;
    let mut names: Vec<String> = params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_none())
        .map(|p| p.parameter.name.id.to_string())
        .collect();
    // pytest drops the first name it kept, whatever it is.
    if drop_first && !names.is_empty() {
        names.remove(0);
    }
    names
}

/// Parameter names that have defaults.
pub(crate) fn default_argnames(def: &StmtFunctionDef) -> Vec<String> {
    let params = &def.parameters;
    params
        .args
        .iter()
        .chain(params.kwonlyargs.iter())
        .filter(|p| p.default.is_some())
        .map(|p| p.parameter.name.id.to_string())
        .collect()
}

/// A fixture defined by `def`, if its decorators make it one.
pub(crate) fn fixture_def<'a>(
    lookup: Lookup<'_, 'a>,
    def: &'a StmtFunctionDef,
    is_method: bool,
) -> Option<FixtureDef> {
    let lookup = lookup.at(def.start());
    let mut fixture = None;
    let mut staticmethod = false;
    for d in &def.decorator_list {
        match decorator(lookup, &d.expression) {
            Decorator::Fixture(call) => fixture = Some(call),
            Decorator::StaticMethod => staticmethod = true,
            _ => {}
        }
    }
    let call = fixture?;
    let mut out = FixtureDef::named(
        def.name.id.as_str(),
        funcargnames(def, is_method && !staticmethod),
    );
    if let Some(call) = call {
        fixture_options(lookup, call, &mut out);
        // `pytest.fixture(scope, params, autouse, ids, name)` positionally
        // is gone in pytest 9; a bare positional is the function itself.
    }
    Some(out)
}

/// The keywords of a `pytest.fixture(…)` call, read into `out`.
fn fixture_options<'a>(lookup: Lookup<'_, 'a>, call: &'a ExprCall, out: &mut FixtureDef) {
    for keyword in &call.arguments.keywords {
        let Some(arg) = &keyword.arg else {
            out.params = true;
            out.autouse = true;
            out.any_name = true;
            continue;
        };
        match arg.id.as_str() {
            "name" => {
                if let Some(name) = lookup.string(&keyword.value) {
                    out.name = name;
                    out.explicit_name = true;
                } else if !matches!(keyword.value, Expr::NoneLiteral(_)) {
                    out.any_name = true;
                }
            }
            "params" => out.params = !matches!(keyword.value, Expr::NoneLiteral(_)),
            "scope" => out.scope = Scoped::of(lookup, &keyword.value),
            "autouse" => out.autouse = lookup.boolean(&keyword.value) != Some(false),
            _ => {}
        }
    }
}

/// Whether `func` names pytest's fixture decorator.
fn is_fixture_decorator(lookup: Lookup<'_, '_>, func: &Expr) -> bool {
    lookup.path(func).is_some_and(|path| {
        path == "pytest.fixture"
            || path == "pytest.yield_fixture"
            || path == "fixture"
            || path.ends_with(".fixture")
    })
}

/// The fixture `name` is once the namespace `ns` has run, if it may be one:
/// its final binding's, or, when which binding runs is known only at
/// runtime (alternative branches, `try`/`except ImportError`, a last
/// binding that may not run), any of theirs, merged: parametrized if any
/// is, requesting what any requests, and `conditional`. Besides `@fixture`
/// functions, it reads `fixture(…)(function)` and `fixture(function, …)`
/// calls and aliases of fixtures (`alias = fixture_function`); names bound
/// in ways aye can't read that may make a fixture (a decorator or a call
/// it can't see through, unpacking) are `opaque`. `guarded` holds where the
/// body's statements that may not run start
/// ([`guarded_by_try`](aye_python_semantic::flow::guarded_by_try)).
pub(crate) fn fixture_of<'a>(
    lookup: Lookup<'_, 'a>,
    ns: &Namespace<'a>,
    name: &str,
    is_method: bool,
    guarded: &[TextSize],
) -> Option<FixtureDef> {
    let last = ns.get(name)?;
    let bindings = ns.bindings(name);
    let start = last.range.start();
    let mut candidates: Vec<&Bound<'a>> = vec![last];
    candidates.extend(
        bindings
            .iter()
            .filter(|b| b.range.start() < start && exclusive(b, last)),
    );
    let may_not_run = last.conditional || guarded.contains(&start);
    if may_not_run
        && let Some(previous) = bindings
            .iter()
            .rev()
            .find(|b| b.range.start() < start && !exclusive(b, last))
    {
        candidates.push(previous);
    }
    let alternatives = candidates.len() > 1;
    let mut merged: Option<FixtureDef> = None;
    for bound in candidates {
        let Some(def) = binding_fixture(lookup, name, bound, is_method, 0) else {
            continue;
        };
        merged = Some(match merged {
            None => def,
            Some(mut found) => {
                found.params |= def.params;
                found.autouse |= def.autouse;
                found.opaque |= def.opaque;
                found.any_name |= def.any_name;
                if found.scope != def.scope {
                    found.scope = Scoped::Unknown;
                }
                for arg in def.args {
                    if !found.args.contains(&arg) {
                        found.args.push(arg);
                    }
                }
                found
            }
        });
    }
    let mut def = merged?;
    def.conditional |= may_not_run || alternatives;
    Some(def)
}

/// The fixture one binding of `name` makes, if it may make one (see
/// [`fixture_of`]).
fn binding_fixture<'a>(
    lookup: Lookup<'_, 'a>,
    name: &str,
    bound: &Bound<'a>,
    is_method: bool,
    depth: u8,
) -> Option<FixtureDef> {
    let lookup = lookup.at(bound.range.start());
    match &bound.binding {
        Binding::Function(def) => {
            if let Some(mut fixture) = fixture_def(lookup, def, is_method) {
                if fixture.attribute != name {
                    // Bound under another name by an alias.
                    fixture.attribute = name.to_string();
                    if !fixture.explicit_name {
                        fixture.name = name.to_string();
                    }
                }
                return Some(fixture);
            }
            // A decorator aye can't see through may make it a fixture.
            def.decorator_list
                .iter()
                .any(|d| matches!(decorator(lookup, &d.expression), Decorator::Unknown))
                .then(|| FixtureDef::opaque(name))
        }
        Binding::Value(expr) => value_fixture(lookup, name, expr, is_method, depth),
        Binding::Other => Some(FixtureDef::opaque(name)),
        Binding::Class(_) | Binding::Import(_) => None,
    }
}

/// The fixture `name = expr` makes, if it may make one: `fixture(…)(f)`,
/// `fixture(f, …)`, an alias of a fixture, or a call or attribute aye can't
/// read (opaque). Calls that surely make no fixture (a class of the module
/// that makes its own instances, a builtin, a function of a package that
/// holds none) don't.
fn value_fixture<'a>(
    lookup: Lookup<'_, 'a>,
    name: &str,
    expr: &'a Expr,
    is_method: bool,
    depth: u8,
) -> Option<FixtureDef> {
    if depth > 4 {
        return Some(FixtureDef::opaque(name));
    }
    match expr {
        Expr::Call(call) => {
            // `fixture(…)(function)`: the options are the inner call's.
            let (options, function) = match &*call.func {
                Expr::Call(inner) if is_fixture_decorator(lookup, &inner.func) => {
                    (Some(inner), call.arguments.args.first())
                }
                func if is_fixture_decorator(lookup, func) => {
                    (Some(call), call.arguments.args.first())
                }
                _ => (None, None),
            };
            if let Some(options) = options {
                let Some(function) = function else {
                    // `fixture(…)` alone makes a decorator, not a fixture.
                    return None;
                };
                let mut out = FixtureDef::named(name, Vec::new());
                match lookup.resolve(function) {
                    Resolved::Local(target) => match target.binding {
                        Binding::Function(def) => out.args = funcargnames(def, is_method),
                        Binding::Value(Expr::Lambda(lambda)) => {
                            out.args = lambda
                                .parameters
                                .iter()
                                .flat_map(|p| p.args.iter().chain(&p.kwonlyargs))
                                .filter(|p| p.default.is_none())
                                .map(|p| p.parameter.name.id.to_string())
                                .collect();
                        }
                        _ => out.opaque = true,
                    },
                    _ => out.opaque = true,
                }
                fixture_options(lookup, options, &mut out);
                return Some(out);
            }
            // A class of the module makes its own instance, which pytest
            // never registers. A capitalized callee may still be a factory
            // that returns a fixture (`backend = Param("a", "b")`).
            if makes_own_instance(lookup, &call.func, 0) {
                return None;
            }
            if let Some(path) = lookup.path(&call.func)
                && (path.starts_with("builtins.") || super::no_tests(&path))
            {
                return None;
            }
            Some(FixtureDef::opaque(name))
        }
        Expr::Name(_) | Expr::Attribute(_) => match lookup.resolve(expr) {
            Resolved::Local(target) => {
                if let Binding::Function(def) = target.binding
                    && fixture_def(lookup, def, is_method).is_none()
                    && !def
                        .decorator_list
                        .iter()
                        .any(|d| matches!(decorator(lookup, &d.expression), Decorator::Unknown))
                {
                    return None;
                }
                binding_fixture(lookup, name, &target, is_method, depth + 1)
            }
            Resolved::Path(path) if super::no_tests(&path) => None,
            Resolved::Path(path) if path.starts_with("builtins.") => None,
            _ => Some(FixtureDef::opaque(name)),
        },
        _ => None,
    }
}

/// Whether calling `func` surely makes an instance of a class of this
/// module, which pytest never registers as a fixture (it registers only
/// objects whose type is exactly its `FixtureFunctionDefinition`): an
/// unconditional, undecorated class with no class keywords (a metaclass),
/// no `__new__`, nothing in its body that may bind one, nothing that may
/// change it afterwards, and only such classes or `object` as bases.
fn makes_own_instance<'a>(lookup: Lookup<'_, 'a>, func: &'a Expr, depth: u8) -> bool {
    if depth > 8 || lookup.module.dynamic {
        return false;
    }
    let Resolved::Local(Bound {
        binding: Binding::Class(class),
        conditional: false,
        ..
    }) = lookup.resolve(func)
    else {
        return false;
    };
    let plain_body = class.body.iter().all(|stmt| match stmt {
        Stmt::FunctionDef(def) => def.name.id.as_str() != "__new__",
        Stmt::Assign(assign) => assign
            .targets
            .iter()
            .all(|target| !matches!(target, Expr::Name(n) if n.id.as_str() == "__new__")),
        Stmt::AnnAssign(assign) => {
            !matches!(&*assign.target, Expr::Name(n) if n.id.as_str() == "__new__")
        }
        Stmt::Pass(_) => true,
        Stmt::Expr(expr) => matches!(&*expr.value, Expr::StringLiteral(_)),
        _ => false,
    });
    let changed = lookup.module.class_handed(class)
        || lookup
            .module
            .attributes
            .get(class.name.id.as_str())
            .is_some_and(|attributes| attributes.iter().any(|a| a.starts_with("__")));
    if !plain_body || changed || !class.decorator_list.is_empty() {
        return false;
    }
    let Some(arguments) = &class.arguments else {
        return true;
    };
    let lookup = lookup.at(class.start());
    arguments.keywords.is_empty()
        && arguments.args.iter().all(|base| {
            lookup.path(base).as_deref() == Some("builtins.object")
                || makes_own_instance(lookup, base, depth + 1)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_drop_self() {
        let source =
            "class A:\n    def f(self, a, b=1, *, c, d=2): pass\n    def g(a, /, b): pass\n";
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let class = parsed.syntax().body[0].as_class_def_stmt().unwrap();
        let f = class.body[0].as_function_def_stmt().unwrap();
        assert_eq!(funcargnames(f, true), ["a", "c"]);
        assert_eq!(funcargnames(f, false), ["self", "a", "c"]);
        assert_eq!(default_argnames(f), ["b", "d"]);
        // A positional-only `self` is not kept, so pytest drops `b`.
        let g = class.body[1].as_function_def_stmt().unwrap();
        assert_eq!(funcargnames(g, true), Vec::<String>::new());
    }
}
