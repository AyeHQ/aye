//! Marks, decorators and `@pytest.mark.parametrize`, read from the source
//! (`_pytest/mark/structures.py`).

use aye_adapter::ast::{Expr, ExprCall};

use crate::ids::{IdItem, ParamSet, Value};
use crate::scope::{Binding, Elem, Lookup, Resolved};

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

/// Whether a one-argument function returns its argument, or a nested
/// function decorated only with `functools.wraps(argument)`, on every path,
/// and uses the argument for nothing but calling it: it then keeps the
/// decorated object's name, marks, parameters and tests.
fn returns_its_argument(def: &aye_adapter::ast::StmtFunctionDef) -> bool {
    use aye_adapter::ast::Stmt;

    let params = &def.parameters;
    let positional: Vec<_> = params.posonlyargs.iter().chain(&params.args).collect();
    if def.is_async
        || crate::scope::is_generator(&def.body)
        || positional.len() != 1
        || params.vararg.is_some()
        || params.kwarg.is_some()
        || !params.kwonlyargs.is_empty()
    {
        return false;
    }
    let arg = positional[0].parameter.name.id.as_str();
    // Nested functions that wrap the argument.
    let mut wrappers: Vec<&str> = Vec::new();
    for stmt in &def.body {
        if let Stmt::FunctionDef(inner) = stmt
            && !inner.decorator_list.is_empty()
            && inner
                .decorator_list
                .iter()
                .all(|d| is_wraps_of(&d.expression, arg))
        {
            wrappers.push(inner.name.id.as_str());
        }
    }
    if !matches!(def.body.last(), Some(Stmt::Return(_))) {
        return false;
    }
    let mut check = ArgUses {
        arg,
        wrappers: &wrappers,
        ok: true,
        depth: 0,
    };
    for stmt in &def.body {
        aye_adapter::ast::visitor::Visitor::visit_stmt(&mut check, stmt);
    }
    check.ok
}

/// `functools.wraps(arg)` or `wraps(arg)`.
fn is_wraps_of(expr: &Expr, arg: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let named = match &*call.func {
        Expr::Name(name) => name.id.as_str() == "wraps",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "wraps",
        _ => false,
    };
    named
        && call.arguments.keywords.is_empty()
        && matches!(&call.arguments.args[..], [Expr::Name(n)] if n.id.as_str() == arg)
}

/// Checks a decorator's body: its argument is only called, passed to
/// `wraps`, or returned; every `return` of the decorator itself hands back
/// the argument or a wrapper.
struct ArgUses<'w> {
    arg: &'w str,
    wrappers: &'w [&'w str],
    ok: bool,
    /// Nesting in functions and lambdas, whose returns are their own.
    depth: usize,
}

impl<'a> aye_adapter::ast::visitor::Visitor<'a> for ArgUses<'_> {
    fn visit_stmt(&mut self, stmt: &'a aye_adapter::ast::Stmt) {
        use aye_adapter::ast::Stmt;
        use aye_adapter::ast::visitor::walk_stmt;
        match stmt {
            Stmt::Return(ret) if self.depth == 0 => {
                let handed = match ret.value.as_deref() {
                    Some(Expr::Name(name)) => {
                        let name = name.id.as_str();
                        name == self.arg || self.wrappers.contains(&name)
                    }
                    _ => false,
                };
                if !handed {
                    self.ok = false;
                }
            }
            Stmt::FunctionDef(inner) => {
                // `wraps(arg)` decorators are fine; anything else is
                // checked like the rest of the body.
                for d in &inner.decorator_list {
                    if !is_wraps_of(&d.expression, self.arg) {
                        self.visit_expr(&d.expression);
                    }
                }
                self.depth += 1;
                for s in &inner.body {
                    self.visit_stmt(s);
                }
                self.depth -= 1;
            }
            Stmt::ClassDef(_) | Stmt::Global(_) | Stmt::Nonlocal(_) => self.ok = false,
            _ => walk_stmt(self, stmt),
        }
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        use aye_adapter::ast::visitor::walk_expr;
        match expr {
            Expr::Name(name) if name.id.as_str() == self.arg => {
                // Any use not handled below (passed along, stored,
                // rebound) may change the object.
                self.ok = false;
            }
            Expr::Call(call) if matches!(&*call.func, Expr::Name(n) if n.id.as_str() == self.arg) =>
            {
                for arg in &call.arguments.args {
                    self.visit_expr(arg);
                }
                for keyword in &call.arguments.keywords {
                    self.visit_expr(&keyword.value);
                }
            }
            Expr::Lambda(_) => {
                self.depth += 1;
                walk_expr(self, expr);
                self.depth -= 1;
            }
            _ => walk_expr(self, expr),
        }
    }
}

/// Marks read from a `pytestmark` value or a `marks=` argument.
#[derive(Debug, Default, Clone)]
pub(crate) struct MarkList<'s, 'a> {
    pub marks: Vec<Mark<'s, 'a>>,
    /// Some entry could not be read.
    pub unknown: bool,
}

/// Whether an expression that is not a mark may still evaluate to one
/// (a name imported from elsewhere, a call).
fn maybe_mark<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> bool {
    match expr {
        Expr::Call(_) => true,
        Expr::Name(_) | Expr::Attribute(_) => !matches!(
            lookup.resolve(expr),
            Resolved::Local(crate::scope::Bound {
                binding: Binding::Function(_) | Binding::Class(_),
                ..
            })
        ),
        _ => false,
    }
}

/// `pytestmark = …`: a mark or a list of marks. A tuple, or anything that
/// is not a mark, is a collection error in pytest.
pub(crate) fn pytestmark<'s, 'a>(
    lookup: Lookup<'s, 'a>,
    expr: &'a Expr,
) -> Result<MarkList<'s, 'a>, String> {
    let (expr, lookup) = deref(lookup, expr);
    match expr {
        Expr::List(list) => {
            let mut out = MarkList::default();
            for elt in &list.elts {
                if elt.is_starred_expr() {
                    out.unknown = true;
                    continue;
                }
                match mark(lookup, elt) {
                    Ok(Some(mark)) => out.marks.push(mark),
                    Ok(None) if maybe_mark(lookup, elt) => out.unknown = true,
                    Ok(None) => return Err("pytestmark holds something that is not a mark".into()),
                    Err(()) => out.unknown = true,
                }
            }
            Ok(out)
        }
        Expr::Tuple(_) => Err("pytestmark is a tuple; pytest wants a mark or a list".into()),
        other => match mark(lookup, other) {
            Ok(Some(mark)) => Ok(MarkList {
                marks: vec![mark],
                unknown: false,
            }),
            Ok(None) if !maybe_mark(lookup, other) => Err("pytestmark is not a mark".into()),
            Ok(None) | Err(()) => Ok(MarkList {
                marks: Vec::new(),
                unknown: true,
            }),
        },
    }
}

/// Follow names bound to constants.
pub(crate) fn deref<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> (&'a Expr, Lookup<'s, 'a>) {
    let (mut expr, mut lookup) = (expr, lookup);
    for _ in 0..8 {
        let Expr::Name(name) = expr else {
            break;
        };
        match lookup.constant(name.id.as_str()) {
            Some((value, scope)) => {
                expr = value;
                lookup = scope;
            }
            None => break,
        }
    }
    (expr, lookup)
}

/// What the empty parameter set becomes (`empty_parameter_set_mark`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmptyMark {
    Skip,
    Xfail,
    FailAtCollect,
    Invalid(String),
}

impl EmptyMark {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "" | "skip" => Self::Skip,
            "xfail" => Self::Xfail,
            "fail_at_collect" => Self::FailAtCollect,
            other => Self::Invalid(other.to_string()),
        }
    }
}

/// The timeout a parameter set's marks set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SetTimeout {
    /// No `timeout` mark.
    Unmarked,
    Seconds(f64),
    /// A `timeout` mark whose value is not literal.
    Unreadable,
}

impl SetTimeout {
    fn of(mark: &Mark<'_, '_>) -> Self {
        timeout(mark).map_or(Self::Unreadable, Self::Seconds)
    }
}

/// A `parametrize` mark, read.
#[derive(Debug, Clone)]
pub(crate) struct Parametrize {
    pub argnames: Vec<String>,
    pub sets: Vec<ParamSet>,
    pub ids: Option<Vec<Option<IdItem>>>,
    /// Argument names passed indirectly (to fixtures).
    pub indirect: Vec<String>,
    /// Some `pytest.param(marks=…)` could not be read.
    pub unknown_marks: bool,
    /// Per parameter set, what its first `timeout` mark sets.
    pub timeouts: Vec<SetTimeout>,
    /// `scope=`, when given (and not `None`).
    pub scope: Option<Scoped>,
}

/// A fixture's or a parametrization's scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Scoped {
    Function,
    Class,
    Module,
    Package,
    Session,
    /// Not a literal aye can read.
    Unknown,
}

impl Scoped {
    /// The scope an expression names.
    pub(crate) fn of<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> Self {
        match lookup.string(expr).as_deref() {
            Some("function") => Self::Function,
            Some("class") => Self::Class,
            Some("module") => Self::Module,
            Some("package") => Self::Package,
            Some("session") => Self::Session,
            _ => Self::Unknown,
        }
    }
}

impl Parametrize {
    /// Argument names parametrized directly (not through fixtures).
    pub(crate) fn direct(&self) -> impl Iterator<Item = &String> {
        self.argnames.iter().filter(|n| !self.indirect.contains(n))
    }
}

/// The arguments of a call, matched to a signature the way Python does.
struct Args<'a> {
    slots: Vec<Option<&'a Expr>>,
}

fn bind_args<'a>(call: &'a ExprCall, params: &[&str]) -> Result<Args<'a>, String> {
    let mut slots = vec![None; params.len()];
    let arguments = &call.arguments;
    if arguments.args.len() > params.len() {
        return Err("too many positional arguments".into());
    }
    for (slot, arg) in slots.iter_mut().zip(arguments.args.iter()) {
        if arg.is_starred_expr() {
            return Err("arguments passed with *".into());
        }
        *slot = Some(arg);
    }
    for keyword in &arguments.keywords {
        let Some(name) = &keyword.arg else {
            return Err("arguments passed with **".into());
        };
        let Some(at) = params.iter().position(|p| *p == name.id.as_str()) else {
            return Err(format!("unexpected keyword argument {}", name.id));
        };
        if slots[at].is_some() {
            return Err(format!("argument {} given twice", name.id));
        }
        slots[at] = Some(&keyword.value);
    }
    Ok(Args { slots })
}

/// Read a `parametrize` mark. `Err` holds why pytest's ids for it can only
/// be known at runtime (or why pytest would refuse it).
pub(crate) fn parametrize(mark: &Mark<'_, '_>, empty: &EmptyMark) -> Result<Parametrize, String> {
    let Some((call, lookup)) = mark.call else {
        return Err("parametrize without arguments".into());
    };
    let args = bind_args(call, &["argnames", "argvalues", "indirect", "ids", "scope"])?;
    let (Some(argnames), Some(argvalues)) = (args.slots[0], args.slots[1]) else {
        return Err("parametrize without argnames or argvalues".into());
    };

    let (argnames, force_tuple) = if let Some(text) = lookup.string(argnames) {
        let names: Vec<String> = text
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let force = names.len() == 1 && !text.trim_end().ends_with(',');
        (names, force)
    } else {
        let elems = lookup
            .sequence(argnames)
            .map_err(|_| "parametrize argnames are not literal".to_string())?;
        let mut names = Vec::new();
        for elem in elems {
            match elem_value(&elem) {
                Value::Str(name) => names.push(name),
                _ => return Err("parametrize argnames are not literal".into()),
            }
        }
        (names, false)
    };
    if argnames.iter().any(|n| n == "request") {
        return Err("'request' is a reserved name for parametrize".into());
    }

    let elems = lookup
        .sequence(argvalues)
        .map_err(|_| "parametrize argvalues are not literal".to_string())?;
    let mut sets = Vec::with_capacity(elems.len());
    let mut timeouts = Vec::with_capacity(elems.len());
    let mut unknown_marks = false;
    for elem in &elems {
        let set = match elem {
            Elem::Expr(expr, lk) => {
                let (expr, lk) = deref(*lk, expr);
                if let Some(param) = as_param(lk, expr) {
                    let (set, unknown, timeout) = param_set(lk, param)?;
                    unknown_marks |= unknown;
                    timeouts.push(timeout);
                    set
                } else if force_tuple {
                    timeouts.push(SetTimeout::Unmarked);
                    ParamSet {
                        values: vec![lk.value(expr).unwrap_or(Value::Unknown)],
                        id: None,
                        marks: Vec::new(),
                    }
                } else {
                    timeouts.push(SetTimeout::Unmarked);
                    let values = lk
                        .sequence(expr)
                        .map_err(|_| "parametrize argvalues are not literal".to_string())?;
                    ParamSet {
                        values: values.iter().map(elem_value).collect(),
                        id: None,
                        marks: Vec::new(),
                    }
                }
            }
            Elem::Value(value) => {
                timeouts.push(SetTimeout::Unmarked);
                let values = if force_tuple {
                    vec![value.clone()]
                } else {
                    match value {
                        Value::Str(text) => {
                            text.chars().map(|c| Value::Str(c.to_string())).collect()
                        }
                        _ => return Err("parametrize argvalues are not sequences".into()),
                    }
                };
                ParamSet {
                    values,
                    id: None,
                    marks: Vec::new(),
                }
            }
        };
        if set.values.len() != argnames.len() {
            return Err(format!(
                "parametrize has {} names but a parameter set with {} values",
                argnames.len(),
                set.values.len()
            ));
        }
        sets.push(set);
    }
    if sets.is_empty() {
        let mark = match empty {
            EmptyMark::Skip => "skip",
            EmptyMark::Xfail => "xfail",
            EmptyMark::FailAtCollect => {
                return Err(
                    "empty parameter set with empty_parameter_set_mark = fail_at_collect".into(),
                );
            }
            EmptyMark::Invalid(value) => {
                return Err(format!("unknown empty_parameter_set_mark {value:?}"));
            }
        };
        sets.push(ParamSet {
            values: vec![Value::Opaque; argnames.len()],
            id: Some(IdItem::Text("NOTSET".into())),
            marks: vec![mark.to_string()],
        });
        timeouts.push(SetTimeout::Unmarked);
    }

    let ids = match args.slots[3] {
        None | Some(Expr::NoneLiteral(_)) => None,
        Some(expr) => {
            let elems = lookup
                .sequence(expr)
                .map_err(|_| "parametrize ids are computed at runtime".to_string())?;
            let mut ids = Vec::with_capacity(elems.len());
            for elem in &elems {
                ids.push(match elem {
                    Elem::Value(value) => Some(IdItem::Value(value.clone())),
                    Elem::Expr(expr, lk) => {
                        let (expr, lk) = deref(*lk, expr);
                        if matches!(expr, Expr::NoneLiteral(_)) {
                            None
                        } else if is_hidden(lk, expr) {
                            Some(IdItem::Hidden)
                        } else {
                            Some(IdItem::Value(lk.value(expr).unwrap_or(Value::Unknown)))
                        }
                    }
                });
            }
            if !ids.is_empty() && ids.len() != sets.len() {
                return Err(format!(
                    "{} parameter sets but {} ids",
                    sets.len(),
                    ids.len()
                ));
            }
            Some(ids)
        }
    };

    let indirect = match args.slots[2] {
        None => Vec::new(),
        Some(expr) => indirect_names(lookup, expr, &argnames)?,
    };

    // `Scope.from_user`: a name other than pytest's five fails collection.
    let scope = match args.slots[4] {
        None | Some(Expr::NoneLiteral(_)) => None,
        Some(expr) => match Scoped::of(lookup, expr) {
            Scoped::Unknown => match lookup.string(expr) {
                Some(name) => {
                    return Err(format!(
                        "parametrize() got an unexpected scope value '{name}'"
                    ));
                }
                None => Some(Scoped::Unknown),
            },
            scope => Some(scope),
        },
    };

    Ok(Parametrize {
        argnames,
        sets,
        ids,
        indirect,
        unknown_marks,
        timeouts,
        scope,
    })
}

/// `indirect=`, as `_resolve_args_directness` reads it: a boolean, or a
/// sequence (a list, a tuple, a string) of names among `argnames`. Anything
/// else fails collection, and a value aye can't read may name any of them.
fn indirect_names<'a>(
    lookup: Lookup<'_, 'a>,
    expr: &'a Expr,
    argnames: &[String],
) -> Result<Vec<String>, String> {
    if let Some(flag) = lookup.boolean(expr) {
        return Ok(if flag { argnames.to_vec() } else { Vec::new() });
    }
    let (value, lk) = deref(lookup, expr);
    if matches!(
        value,
        Expr::NoneLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::Dict(_)
            | Expr::Set(_)
            | Expr::EllipsisLiteral(_)
    ) {
        return Err("parametrize indirect is neither a boolean nor a sequence".into());
    }
    let elems = lk
        .sequence(value)
        .map_err(|_| "parametrize indirect is not literal".to_string())?;
    let mut names = Vec::with_capacity(elems.len());
    for elem in &elems {
        match elem_value(elem) {
            Value::Str(name) if argnames.contains(&name) => names.push(name),
            Value::Str(name) => return Err(format!("indirect fixture '{name}' doesn't exist")),
            Value::Unknown => return Err("parametrize indirect is not literal".into()),
            _ => return Err("parametrize indirect names something that is not an argument".into()),
        }
    }
    Ok(names)
}

fn elem_value(elem: &Elem<'_, '_>) -> Value {
    match elem {
        Elem::Value(value) => value.clone(),
        Elem::Expr(expr, lk) => lk.value(expr).unwrap_or(Value::Unknown),
    }
}

fn as_param<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> Option<&'a ExprCall> {
    let Expr::Call(call) = expr else {
        return None;
    };
    (lookup.path(&call.func).as_deref() == Some("pytest.param")).then_some(call)
}

fn is_hidden<'a>(lookup: Lookup<'_, 'a>, expr: &'a Expr) -> bool {
    lookup.path(expr).as_deref() == Some("pytest.HIDDEN_PARAM")
}

/// `pytest.param(*values, marks=(), id=None)`: the set, whether some marks
/// could not be read, and the timeout its marks set.
fn param_set<'a>(
    lookup: Lookup<'_, 'a>,
    call: &'a ExprCall,
) -> Result<(ParamSet, bool, SetTimeout), String> {
    let mut values = Vec::new();
    let mut id = None;
    let mut marks = Vec::new();
    let mut unknown = false;
    let mut set_timeout = SetTimeout::Unmarked;
    for arg in &call.arguments.args {
        if arg.is_starred_expr() {
            return Err("pytest.param values passed with *".into());
        }
        values.push(lookup.value(arg).unwrap_or(Value::Unknown));
    }
    for keyword in &call.arguments.keywords {
        let Some(name) = &keyword.arg else {
            return Err("pytest.param arguments passed with **".into());
        };
        let (value, lk) = deref(lookup, &keyword.value);
        match name.id.as_str() {
            "id" => {
                id = if matches!(value, Expr::NoneLiteral(_)) {
                    None
                } else if is_hidden(lk, value) {
                    Some(IdItem::Hidden)
                } else if let Some(text) = lk.string(value) {
                    Some(IdItem::Text(text))
                } else {
                    return Err("pytest.param id is not a literal string".into());
                };
            }
            "marks" => {
                let list = param_marks(lk, value)?;
                if list.marks.iter().any(|m| m.name == "usefixtures") {
                    return Err("pytest.param cannot add usefixtures".into());
                }
                unknown |= list.unknown;
                if let Some(mark) = list.marks.iter().find(|m| m.name == "timeout") {
                    set_timeout = SetTimeout::of(mark);
                }
                marks.extend(list.marks.into_iter().map(|m| m.name));
            }
            other => return Err(format!("pytest.param got an unexpected argument {other}")),
        }
    }
    Ok((ParamSet { values, id, marks }, unknown, set_timeout))
}

/// `pytest.param(marks=…)`: a mark, or a collection of marks. `Err` when
/// pytest would fail while the module is imported: a set of marks (marks
/// aren't hashable), or something that is surely no mark (`param` reads
/// each one's `name`).
fn param_marks<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> Result<MarkList<'s, 'a>, String> {
    let (expr, lookup) = deref(lookup, expr);
    // One entry: a mark, something that may be one, or an error.
    let entry = |lookup: Lookup<'s, 'a>, expr: &'a Expr| -> Result<Option<Mark<'s, 'a>>, String> {
        match mark(lookup, expr) {
            Ok(Some(mark)) => Ok(Some(mark)),
            _ if matches!(expr, Expr::Call(call) if made_by_builtin(lookup, call)) => {
                Err("pytest.param marks are not marks, which raises at import".into())
            }
            _ if maybe_mark(lookup, expr) => Ok(None),
            _ => Err("pytest.param marks are not marks, which raises at import".into()),
        }
    };
    // `tuple(…)`, `list(…)`, `set()`: the collection their argument makes.
    if let Expr::Call(call) = expr
        && call.arguments.keywords.is_empty()
        && let Some(path) = lookup.path(&call.func)
        && matches!(
            path.as_str(),
            "builtins.tuple" | "builtins.list" | "builtins.set" | "builtins.frozenset"
        )
    {
        return match call.arguments.args.as_ref() {
            [] => Ok(MarkList::default()),
            [arg] if matches!(path.as_str(), "builtins.tuple" | "builtins.list") => {
                param_marks(lookup, arg)
            }
            _ => Err("pytest.param marks are a set, which raises TypeError".into()),
        };
    }
    let elts = match expr {
        Expr::List(list) => &list.elts[..],
        Expr::Tuple(tuple) => &tuple.elts[..],
        Expr::Set(set) if set.elts.is_empty() => &[],
        Expr::Set(_) => {
            return Err("pytest.param marks are a set, which raises TypeError".into());
        }
        other => {
            return Ok(match entry(lookup, other)? {
                Some(mark) => MarkList {
                    marks: vec![mark],
                    unknown: false,
                },
                None => MarkList {
                    marks: Vec::new(),
                    unknown: true,
                },
            });
        }
    };
    let mut out = MarkList::default();
    for elt in elts {
        if elt.is_starred_expr() {
            out.unknown = true;
            continue;
        }
        let (elt, lookup) = deref(lookup, elt);
        match entry(lookup, elt)? {
            Some(mark) => out.marks.push(mark),
            None => out.unknown = true,
        }
    }
    Ok(out)
}

/// Whether a call surely makes a builtin container or value (never a
/// mark): `set(…)`, `frozenset(…)`, `dict(…)`…
fn made_by_builtin<'a>(lookup: Lookup<'_, 'a>, call: &'a ExprCall) -> bool {
    lookup
        .path(&call.func)
        .is_some_and(|path| path.starts_with("builtins."))
}

/// `marks=`: a mark, or a collection of marks. A set of marks raises
/// `TypeError` (marks aren't hashable), so it is never read.
pub(crate) fn mark_collection<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> MarkList<'s, 'a> {
    let (expr, lookup) = deref(lookup, expr);
    let elts = match expr {
        Expr::List(list) => &list.elts[..],
        Expr::Tuple(tuple) => &tuple.elts[..],
        Expr::Set(_) => {
            return MarkList {
                marks: Vec::new(),
                unknown: true,
            };
        }
        other => {
            return match mark(lookup, other) {
                Ok(Some(mark)) => MarkList {
                    marks: vec![mark],
                    unknown: false,
                },
                _ => MarkList {
                    marks: Vec::new(),
                    unknown: true,
                },
            };
        }
    };
    let mut out = MarkList::default();
    for elt in elts {
        match mark(lookup, elt) {
            Ok(Some(mark)) => out.marks.push(mark),
            _ => out.unknown = true,
        }
    }
    out
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
