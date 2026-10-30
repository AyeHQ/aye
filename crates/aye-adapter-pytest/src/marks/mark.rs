//! One mark, and what a decorator does to collection.

use aye_adapter::ast::{Expr, ExprCall};
use aye_python_semantic::decorator::returns_its_argument;
use aye_python_semantic::eval::Value;
use aye_python_semantic::namespace::Binding;
use aye_python_semantic::resolve::{Lookup, Resolved};

/// A mark, with the call that gave it arguments and where to evaluate them.
#[derive(Clone)]
pub(crate) struct Mark<'s, 'a> {
    pub name: String,
    pub call: Option<(&'a ExprCall, Lookup<'s, 'a>)>,
}

impl std::fmt::Debug for Mark<'_, '_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Mark({})", self.name)
    }
}

/// What a decorator does, as far as collection is concerned.
#[derive(Debug, Clone)]
pub(crate) enum Decorator<'s, 'a> {
    Mark(Mark<'s, 'a>),
    /// `@pytest.fixture`: a fixture, not a test.
    Fixture(Option<&'a ExprCall>),
    StaticMethod,
    ClassMethod,
    /// `property` and friends: the attribute is not callable.
    Property,
    /// `abc.abstractproperty`: not callable, and it makes the class
    /// abstract.
    AbstractProperty,
    /// `abc.abstractmethod` and friends.
    Abstract,
    /// `dataclass` and friends: the class gets an `__init__`.
    Init,
    /// Known to leave the name, marks and parameters alone.
    Transparent,
    /// From a well-known library that keeps the test and its ids, but
    /// leaves attributes `-k` matches (Hypothesis' `given`, `flaky`).
    Keeps,
    /// Defined outside the file and well-known libraries, or not
    /// readable: it may add marks (`parametrize` included), replace the
    /// object, or add, remove or rename tests (`ddt`,
    /// `parameterized.expand`, registries that return `None`).
    Unknown,
}

/// Top-level packages whose decorators keep the test and its ids, but may
/// leave attributes on it.
const KEEPS: &[&str] = &["flaky", "hypothesis"];

/// Top-level packages whose decorators are known not to add marks or
/// parameters.
const TRANSPARENT: &[&str] = &[
    "_pytest",
    "abc",
    "anyio",
    "asyncio",
    "attr",
    "attrs",
    "builtins",
    "contextlib",
    "dataclasses",
    "django",
    "enum",
    "freezegun",
    "functools",
    "mock",
    "pytest",
    "pytest_asyncio",
    "pytest_django",
    "pytest_mock",
    "responses",
    "respx",
    "time_machine",
    "trio",
    "typing",
    "typing_extensions",
    "unittest",
    "vcr",
    "warnings",
];

enum MarkRes<'s, 'a> {
    Mark(Mark<'s, 'a>),
    NotMark,
    Unknown,
}

fn resolve_mark<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr, depth: u8) -> MarkRes<'s, 'a> {
    if depth > 8 {
        return MarkRes::Unknown;
    }
    match expr {
        Expr::Call(call) => {
            if let Some(path) = lookup.path(&call.func)
                && let Some(name) = path
                    .strip_prefix("pytest.mark.")
                    .and_then(|rest| rest.strip_suffix(".with_args"))
                && !name.contains('.')
            {
                return MarkRes::Mark(Mark {
                    name: name.to_string(),
                    call: Some((call, lookup)),
                });
            }
            match resolve_mark(lookup, &call.func, depth + 1) {
                MarkRes::Mark(Mark { name, call: None }) => MarkRes::Mark(Mark {
                    name,
                    call: Some((call, lookup)),
                }),
                // Calling a mark that already has arguments combines them.
                MarkRes::Mark(_) => MarkRes::Unknown,
                other => other,
            }
        }
        Expr::Name(_) | Expr::Attribute(_) => match lookup.resolve(expr) {
            Resolved::Path(path) => match path.strip_prefix("pytest.mark.") {
                Some(name) if !name.contains('.') => MarkRes::Mark(Mark {
                    name: name.to_string(),
                    call: None,
                }),
                Some(_) => MarkRes::Unknown,
                None => MarkRes::NotMark,
            },
            Resolved::Local(bound) => match (&bound.binding, expr) {
                (Binding::Value(_), Expr::Name(name)) => match lookup.constant(name.id.as_str()) {
                    Some((value, scope)) => resolve_mark(scope, value, depth + 1),
                    None => MarkRes::Unknown,
                },
                (Binding::Function(_) | Binding::Class(_), _) => MarkRes::NotMark,
                _ => MarkRes::Unknown,
            },
            Resolved::Unknown => MarkRes::Unknown,
        },
        _ => MarkRes::Unknown,
    }
}

/// The mark an expression evaluates to: `Ok(None)` when it is something
/// else, `Err` when it cannot be told.
pub(crate) fn mark<'s, 'a>(
    lookup: Lookup<'s, 'a>,
    expr: &'a Expr,
) -> Result<Option<Mark<'s, 'a>>, ()> {
    match resolve_mark(lookup, expr, 0) {
        MarkRes::Mark(mark) => Ok(Some(mark)),
        MarkRes::NotMark => Ok(None),
        MarkRes::Unknown => Err(()),
    }
}

/// Classify a decorator.
pub(crate) fn decorator<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> Decorator<'s, 'a> {
    match resolve_mark(lookup, expr, 0) {
        MarkRes::Mark(mark) => return Decorator::Mark(mark),
        MarkRes::Unknown => return Decorator::Unknown,
        MarkRes::NotMark => {}
    }
    let (callee, call) = match expr {
        Expr::Call(call) => (&*call.func, Some(call)),
        other => (other, None),
    };
    match lookup.resolve(callee) {
        Resolved::Path(path) => {
            if path == "pytest.fixture"
                || path == "pytest.yield_fixture"
                || path == "fixture"
                || path.ends_with(".fixture")
            {
                return Decorator::Fixture(call);
            }
            match path.as_str() {
                "builtins.staticmethod" => Decorator::StaticMethod,
                "builtins.classmethod" => Decorator::ClassMethod,
                "builtins.property" | "functools.cached_property" => Decorator::Property,
                "abc.abstractproperty" => Decorator::AbstractProperty,
                "abc.abstractmethod" | "abc.abstractclassmethod" | "abc.abstractstaticmethod" => {
                    Decorator::Abstract
                }
                "dataclasses.dataclass"
                | "attr.s"
                | "attr.attrs"
                | "attr.define"
                | "attr.frozen"
                | "attr.mutable"
                | "attrs.define"
                | "attrs.frozen"
                | "attrs.mutable" => {
                    let init_off = call.is_some_and(|c| {
                        c.arguments.keywords.iter().any(|k| {
                            k.arg.as_ref().is_some_and(|a| a.id.as_str() == "init")
                                && lookup.boolean(&k.value) == Some(false)
                        })
                    });
                    if init_off {
                        Decorator::Transparent
                    } else {
                        Decorator::Init
                    }
                }
                _ => {
                    let top = path.split('.').next().unwrap_or_default();
                    if TRANSPARENT.contains(&top) {
                        Decorator::Transparent
                    } else if KEEPS.contains(&top) {
                        Decorator::Keeps
                    } else {
                        Decorator::Unknown
                    }
                }
            }
        }
        // A decorator of the module: transparent only when its body visibly
        // hands back what it was given, or a `functools.wraps` wrapper of
        // it, without touching it.
        Resolved::Local(bound) => match (bound.binding, call) {
            (Binding::Function(def), None) if returns_its_argument(def) => Decorator::Transparent,
            _ => Decorator::Unknown,
        },
        Resolved::Unknown => Decorator::Unknown,
    }
}

/// `@pytest.mark.timeout(N)` / `timeout=N`: seconds, when literal.
pub(crate) fn timeout(mark: &Mark<'_, '_>) -> Option<f64> {
    let (call, lookup) = mark.call?;
    let expr = call.arguments.args.first().or_else(|| {
        call.arguments
            .keywords
            .iter()
            .find(|k| k.arg.as_ref().is_some_and(|a| a.id.as_str() == "timeout"))
            .map(|k| &k.value)
    })?;
    match lookup.value(expr).ok()? {
        Value::Int(digits) => digits.parse().ok(),
        Value::Float(f) => Some(f),
        _ => None,
    }
}

/// `@pytest.mark.usefixtures("a", "b")`: the names, and whether some could
/// not be read.
pub(crate) fn usefixtures(mark: &Mark<'_, '_>) -> (Vec<String>, bool) {
    let Some((call, lookup)) = mark.call else {
        return (Vec::new(), false);
    };
    let mut names = Vec::new();
    let mut unknown = !call.arguments.keywords.is_empty();
    for arg in &call.arguments.args {
        match lookup.string(arg) {
            Some(name) => names.push(name),
            None => unknown = true,
        }
    }
    (names, unknown)
}
