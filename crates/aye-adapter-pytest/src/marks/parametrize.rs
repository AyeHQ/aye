//! `@pytest.mark.parametrize` and `pytest.param`, read as
//! `_pytest/mark/structures.py` reads them.

use aye_adapter::ast::{Expr, ExprCall};
use aye_python_semantic::eval::{Elem, Value, deref};
use aye_python_semantic::resolve::Lookup;

use super::ids::{IdItem, ParamSet};
use super::mark::{Mark, mark, timeout};
use super::pytestmark::{MarkList, maybe_mark};

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
