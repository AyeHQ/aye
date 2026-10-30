//! Static values: what an expression surely evaluates to once the module
//! is imported, when nothing that runs can change it, and whether
//! building it surely succeeds.

use std::collections::HashSet;

use aye_python_stdlib::builtins::{builtin_name, parse_float};
use aye_python_stdlib::re;
use aye_python_stdlib::repr::int_literal_decimal;
use ruff_python_ast::{self as ast, Expr, ExprCall, Number, Operator, UnaryOp};
use ruff_text_size::Ranged;

use crate::namespace::{Binding, Bound, Namespace};
use crate::resolve::{DEPTH, Lookup, Resolved};

/// A statically known parameter value, as far as ids are concerned.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Str(String),
    Bytes(Vec<u8>),
    /// Decimal digits, with a leading `-` when negative.
    Int(String),
    Float(f64),
    Complex(f64, f64),
    Bool(bool),
    None,
    /// An object identified by its `__name__`: a function, a class,
    /// `<lambda>`.
    Named(String),
    /// A compiled regular expression; its id is its escaped pattern.
    Pattern(Box<Value>),
    /// A value with no id of its own (list, dict, tuple…): the id falls back
    /// to the argument name and the parameter set's index.
    Opaque,
    /// Not known statically. Harmless inside a parameter set whose id is
    /// explicit, but a lone unknown value may itself be a `pytest.param`
    /// (a helper's result) whose id and marks win over `ids=`.
    Unknown,
}

/// Callables whose results have no id of their own: not strings, bytes,
/// numbers, booleans, patterns or enums, and no `__name__`. pytest falls
/// back to `argname + index` for them.
pub(crate) const NO_ID: &[&str] = &[
    "builtins.object",
    "builtins.dict",
    "builtins.list",
    "builtins.set",
    "builtins.frozenset",
    "builtins.tuple",
    "builtins.bytearray",
    "builtins.slice",
    "builtins.range",
    "collections.Counter",
    "collections.OrderedDict",
    "collections.defaultdict",
    "collections.deque",
    "datetime.date",
    "datetime.datetime",
    "datetime.time",
    "datetime.timedelta",
    "datetime.timezone",
    "decimal.Decimal",
    "fractions.Fraction",
    "pathlib.Path",
    "pathlib.PosixPath",
    "pathlib.PurePath",
    "pathlib.PurePosixPath",
    "pathlib.PureWindowsPath",
    "pathlib.WindowsPath",
    "uuid.UUID",
    "numpy.arange",
    "numpy.array",
    "numpy.asarray",
    "numpy.empty",
    "numpy.full",
    "numpy.linspace",
    "numpy.ones",
    "numpy.zeros",
    "pandas.Categorical",
    "pandas.DataFrame",
    "pandas.DatetimeIndex",
    "pandas.Index",
    "pandas.Interval",
    "pandas.MultiIndex",
    "pandas.Period",
    "pandas.RangeIndex",
    "pandas.Series",
    "pandas.Timedelta",
    "pandas.Timestamp",
    "pandas.array",
];

/// Why a value could not be known statically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown;

/// One element of a statically known sequence: an expression and where to
/// evaluate it, or a value already known.
#[derive(Clone)]
pub enum Elem<'s, 'a> {
    Expr(&'a Expr, Lookup<'s, 'a>),
    Value(Value),
}

impl<'s, 'a> Lookup<'s, 'a> {
    /// The value bound to a name at `self.at`, when it is a plain constant:
    /// bound unconditionally before this point, last by an assignment, not
    /// deleted since, and never changed (the module's escape analysis finds
    /// nothing that may change it, unless the value can't change).
    pub fn constant(&self, name: &str) -> Option<(&'a Expr, Lookup<'s, 'a>)> {
        for (level, ns) in self.scopes().enumerate() {
            let in_class = level == 0 && self.class.is_some();
            let bindings = ns.bindings(name);
            if bindings.is_empty() {
                continue;
            }
            let before: Vec<&Bound<'a>> = bindings
                .iter()
                .filter(|b| b.range.end() <= self.at)
                .collect();
            let Some(last) = before.last() else {
                // Bound in this namespace, but only later: Python looks
                // further out in a class body, and fails in a module.
                if in_class {
                    continue;
                }
                return None;
            };
            match ns.deleted(name, last.range.end(), self.at) {
                None => {}
                // Unbound again: in a class body, the module's binding (or
                // the builtin) shows through.
                Some(false) if in_class => continue,
                Some(_) => return None,
            }
            if before.iter().any(|b| b.conditional) || ns.changed(name) {
                return None;
            }
            let Binding::Value(expr) = last.binding else {
                return None;
            };
            let scope = if in_class {
                *self
            } else {
                Lookup {
                    class: None,
                    ..*self
                }
            };
            let found = &self.module.escapes;
            let (changed, rebound) = if in_class {
                (
                    self.class_changed(ns, name) || found.attributes_changed.contains(name),
                    found.attributes.contains(name),
                )
            } else {
                (found.changed.contains(name), false)
            };
            if rebound
                || found.renamed.contains(name)
                || (found.renamed_unknown && matches!(expr, Expr::Lambda(_)))
                || (changed && !scope.immutable(expr))
            {
                return None;
            }
            return Some((expr, scope.at(last.range.start())));
        }
        None
    }

    /// Whether the escape analysis says a name of a class body may change:
    /// the module's, which walked the class body knowing the module's
    /// names, or else the class body's own.
    fn class_changed(&self, class: &Namespace<'a>, name: &str) -> bool {
        let found = &self.module.escapes;
        if class
            .origin
            .is_some_and(|at| found.classes_all.contains(&at))
        {
            return true;
        }
        match class.origin.and_then(|at| found.classes.get(&at)) {
            Some(names) => names.contains(name),
            None => class.escapes.changed.contains(name),
        }
    }

    /// Whether the value an expression makes can't change once made (its
    /// names aside, which are looked up again): strings, numbers, tuples of
    /// those, `range(…)`, marks.
    fn immutable(&self, expr: &'a Expr) -> bool {
        match expr {
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::FString(_) => true,
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .all(|elt| matches!(elt, Expr::Name(_)) || self.immutable(elt)),
            Expr::UnaryOp(op) => self.immutable(&op.operand),
            Expr::BinOp(op) => self.immutable(&op.left) && self.immutable(&op.right),
            Expr::Call(call) => matches!(
                self.trusted_path(&call.func).as_deref(),
                Some("builtins.range" | "builtins.float" | "re.compile")
            ),
            Expr::Attribute(_) => self
                .path(expr)
                .is_some_and(|p| p.starts_with("pytest.mark.")),
            _ => false,
        }
    }

    /// Whether the `__name__` of what `name` refers to may differ from its
    /// definition's: reassigned, or handed to code aye can't follow.
    fn renamed(&self, name: &str) -> bool {
        let found = &self.module.escapes;
        found.renamed_unknown
            || found.renamed.contains(name)
            || found.changed.contains(name)
            || self
                .class
                .is_some_and(|class| self.class_changed(class, name))
            || self.scopes().any(|ns| {
                ns.dynamic
                    || ns
                        .attributes
                        .get(name)
                        .is_some_and(|attrs| attrs.contains(&"__name__"))
            })
    }

    /// Evaluate an expression to a value pytest can make an id from.
    pub fn value(&self, expr: &'a Expr) -> Result<Value, Unknown> {
        self.value_depth(expr, 0)
    }

    fn value_depth(&self, expr: &'a Expr, depth: u8) -> Result<Value, Unknown> {
        if depth > DEPTH {
            return Err(Unknown);
        }
        match expr {
            Expr::StringLiteral(s) => literal_text(s.value.to_str().to_string()),
            Expr::BytesLiteral(b) => Ok(Value::Bytes(b.value.bytes().collect())),
            Expr::NumberLiteral(n) => Ok(match &n.value {
                Number::Int(int) => {
                    let digits = match int.as_u64() {
                        Some(small) => small.to_string(),
                        None => int_literal_decimal(&self.source[n.range()]).ok_or(Unknown)?,
                    };
                    // `str()` refuses longer ints when the limit is
                    // lowered (`sys.set_int_max_str_digits`, 4300 by
                    // default), and Python refuses decimal literals beyond
                    // it.
                    if digits.len() > MAX_INT_DIGITS {
                        return Err(Unknown);
                    }
                    Value::Int(digits)
                }
                Number::Float(f) => Value::Float(*f),
                Number::Complex { real, imag } => Value::Complex(*real, *imag),
            }),
            Expr::BooleanLiteral(b) => Ok(Value::Bool(b.value)),
            Expr::NoneLiteral(_) => Ok(Value::None),
            // Containers have no id of their own, but building one may
            // raise (`[*1]`, `{[1]: 2}`): only those surely built are known.
            Expr::EllipsisLiteral(_) => Ok(Value::Opaque),
            Expr::List(list) => opaque(self.elements(&list.elts, false, depth)),
            Expr::Tuple(tuple) => opaque(self.elements(&tuple.elts, false, depth)),
            Expr::Set(set) => opaque(self.elements(&set.elts, true, depth)),
            Expr::Dict(dict) => opaque(self.mapping(dict, depth)),
            Expr::ListComp(comp) => {
                opaque(self.comprehension(&comp.elt, &comp.generators, false, depth))
            }
            Expr::SetComp(comp) => {
                opaque(self.comprehension(&comp.elt, &comp.generators, true, depth))
            }
            // `Expr::DictComp` is never read: see the default below.
            Expr::Lambda(lambda) => {
                // Its defaults are evaluated when it is made.
                let defaults_safe = lambda.parameters.as_deref().is_none_or(|parameters| {
                    parameters
                        .iter_non_variadic_params()
                        .filter_map(|p| p.default.as_deref())
                        .all(|default| self.safe(default, depth + 1))
                });
                if defaults_safe {
                    Ok(Value::Named("<lambda>".to_string()))
                } else {
                    Err(Unknown)
                }
            }
            // A generator object's `__name__`. Its first iterable is
            // iterated at once; the rest runs later.
            Expr::Generator(generator) => match generator.generators.first() {
                Some(first)
                    if !first.is_async && self.iterable(&first.iter, depth + 1).is_some() =>
                {
                    Ok(Value::Named("<genexpr>".to_string()))
                }
                _ => Err(Unknown),
            },
            Expr::FString(f) => {
                let mut text = String::new();
                for part in &f.value {
                    match part {
                        ast::FStringPart::Literal(lit) => text.push_str(lit.as_str()),
                        ast::FStringPart::FString(fstring) => {
                            for element in &fstring.elements {
                                match element.as_literal() {
                                    Some(lit) => text.push_str(&lit.value),
                                    None => return Err(Unknown),
                                }
                            }
                        }
                    }
                }
                literal_text(text)
            }
            Expr::UnaryOp(op) => {
                let operand = self.value_depth(&op.operand, depth + 1)?;
                match (op.op, operand) {
                    (UnaryOp::UAdd, v @ (Value::Int(_) | Value::Float(_) | Value::Complex(..))) => {
                        Ok(v)
                    }
                    (UnaryOp::UAdd, Value::Bool(b)) => Ok(Value::Int(u8::from(b).to_string())),
                    (UnaryOp::USub, Value::Int(digits)) => Ok(Value::Int(negate(&digits))),
                    (UnaryOp::USub, Value::Bool(b)) => {
                        Ok(Value::Int(if b { "-1" } else { "0" }.to_string()))
                    }
                    (UnaryOp::USub, Value::Float(f)) => Ok(Value::Float(-f)),
                    (UnaryOp::USub, Value::Complex(re, im)) => Ok(Value::Complex(-re, -im)),
                    _ => Err(Unknown),
                }
            }
            Expr::BinOp(bin) => {
                let left = self.value_depth(&bin.left, depth + 1)?;
                let right = self.value_depth(&bin.right, depth + 1)?;
                match (bin.op, left, right) {
                    (Operator::Add, Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
                    (Operator::Add, Value::Bytes(mut a), Value::Bytes(b)) => {
                        a.extend(b);
                        Ok(Value::Bytes(a))
                    }
                    (op @ (Operator::Add | Operator::Sub), left, Value::Complex(re, im)) => {
                        // Python 3.14 adds a real number to a complex one
                        // part by part (C99 Annex G); earlier versions make
                        // it `complex(a, 0.0)` first. Only the sign of a
                        // zero imaginary part differs: `1 + -0j`, `1 - 0j`.
                        let add = matches!(op, Operator::Add);
                        if !matches!(left, Value::Complex(..))
                            && im == 0.0
                            && im.is_sign_negative() == add
                        {
                            return Err(Unknown);
                        }
                        let (a_re, a_im) = complex_parts(&left).ok_or(Unknown)?;
                        Ok(if matches!(op, Operator::Add) {
                            Value::Complex(a_re + re, a_im + im)
                        } else {
                            Value::Complex(a_re - re, a_im - im)
                        })
                    }
                    _ => Err(Unknown),
                }
            }
            Expr::Name(name) => {
                if let Some((value, scope)) = self.constant(name.id.as_str()) {
                    return scope.value_depth(value, depth + 1);
                }
                match self.resolve(expr) {
                    Resolved::Path(path) => match path.strip_prefix("builtins.") {
                        Some(builtin) if self.trusted_path(expr).is_some() => builtin_name(builtin)
                            .map(|n| Value::Named(n.to_string()))
                            .ok_or(Unknown),
                        _ => Err(Unknown),
                    },
                    Resolved::Local(bound) => self.named(&bound),
                    Resolved::Unknown => Err(Unknown),
                }
            }
            Expr::Call(call) => self.call_value(call, depth),
            // Module attributes (`math.pi`) are not read: any module of
            // the session may change them while it is imported (`math.pi
            // = 3` in another test module), which aye doesn't follow.
            _ => Err(Unknown),
        }
    }

    /// The `__name__` of a function or class defined in this file, when the
    /// name still refers to it.
    fn named(&self, bound: &Bound<'a>) -> Result<Value, Unknown> {
        if bound.range.end() > self.at || bound.conditional {
            return Err(Unknown);
        }
        let (name, decorators) = match bound.binding {
            Binding::Function(def) => (def.name.id.as_str(), &def.decorator_list),
            Binding::Class(class) => (class.name.id.as_str(), &class.decorator_list),
            _ => return Err(Unknown),
        };
        if self.renamed(name) {
            return Err(Unknown);
        }
        if decorators.iter().all(|d| {
            self.path(&d.expression)
                .is_some_and(|p| p.starts_with("pytest.mark."))
        }) {
            Ok(Value::Named(name.to_string()))
        } else {
            Err(Unknown)
        }
    }

    fn call_value(&self, call: &'a ExprCall, depth: u8) -> Result<Value, Unknown> {
        let path = self.trusted_path(&call.func).ok_or(Unknown)?;
        let args = &call.arguments;
        match path.as_str() {
            "re.compile" if !args.args.is_empty() && args.args.len() <= 2 => {
                if args.args.iter().any(Expr::is_starred_expr)
                    || args
                        .keywords
                        .iter()
                        .any(|k| k.arg.as_ref().is_none_or(|a| a.id.as_str() != "flags"))
                {
                    return Err(Unknown);
                }
                let flags: Vec<&Expr> = args.args[1..]
                    .iter()
                    .chain(args.keywords.iter().map(|k| &k.value))
                    .collect();
                if flags.len() > 1 || !flags.iter().all(|f| self.regex_flags(f)) {
                    return Err(Unknown);
                }
                match self.value_depth(&args.args[0], depth + 1)? {
                    pattern @ (Value::Str(_) | Value::Bytes(_)) if regex_compiles(&pattern) => {
                        Ok(Value::Pattern(Box::new(pattern)))
                    }
                    _ => Err(Unknown),
                }
            }
            "builtins.float" if args.args.len() == 1 && args.keywords.is_empty() => {
                match self.value_depth(&args.args[0], depth + 1)? {
                    Value::Str(text) => parse_float(&text).map(Value::Float).ok_or(Unknown),
                    Value::Float(f) => Ok(Value::Float(f)),
                    // An int too large for a float raises `OverflowError`.
                    Value::Int(digits) => match digits.parse::<f64>() {
                        Ok(f) if f.is_finite() => Ok(Value::Float(f)),
                        _ => Err(Unknown),
                    },
                    _ => Err(Unknown),
                }
            }
            _ if NO_ID.contains(&path.as_str()) => opaque(self.constructs(&path, call, depth)),
            _ => Err(Unknown),
        }
    }

    /// Whether evaluating `expr` here surely doesn't raise: aye knows its
    /// value, or it is a name surely bound by then, or an attribute of an
    /// imported module (which raises only if the module lacks it, as an
    /// import would).
    fn safe(&self, expr: &'a Expr, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        match expr {
            Expr::Starred(_) => false,
            Expr::Name(name) => {
                self.value_depth(expr, depth).is_ok() || self.bound_here(name.id.as_str())
            }
            Expr::Attribute(_) => {
                let mut root = expr;
                while let Expr::Attribute(attr) = root {
                    root = &attr.value;
                }
                matches!(root, Expr::Name(name)
                if self.bound_here(name.id.as_str())
                    && matches!(
                        self.binding(name.id.as_str()),
                        Some(Bound { binding: Binding::Import(_), .. })
                    ))
            }
            _ => self.value_depth(expr, depth).is_ok(),
        }
    }

    /// Whether `name` is surely bound where it is read (so reading it
    /// doesn't raise): bound unconditionally before, and never deleted
    /// since nor by a function; or a builtin.
    fn bound_here(&self, name: &str) -> bool {
        for (level, ns) in self.scopes().enumerate() {
            let in_class = level == 0 && self.class.is_some();
            let before: Vec<&Bound<'a>> = ns
                .bindings(name)
                .iter()
                .filter(|b| b.range.end() <= self.at)
                .collect();
            let Some(last) = before.last() else {
                if in_class || ns.bindings(name).is_empty() {
                    continue;
                }
                return false;
            };
            match ns.deleted(name, last.range.end(), self.at) {
                None => {}
                Some(false) if in_class => continue,
                Some(_) => return false,
            }
            if ns.dynamic || ns.globals.contains(name) {
                return false;
            }
            return before.iter().any(|b| !b.conditional);
        }
        builtin_name(name).is_some() && !self.module.globals.contains(name)
    }

    /// Whether a value is surely hashable (a set element, a dict key).
    fn hashable(&self, expr: &'a Expr, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        match expr {
            Expr::EllipsisLiteral(_) | Expr::Lambda(_) | Expr::Generator(_) => {
                self.value_depth(expr, depth).is_ok()
            }
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .all(|elt| !elt.is_starred_expr() && self.hashable(elt, depth + 1)),
            Expr::Name(name) => match self.constant(name.id.as_str()) {
                Some((value, scope)) => scope.hashable(value, depth + 1),
                None => self
                    .value_depth(expr, depth)
                    .is_ok_and(|v| hashable_value(&v)),
            },
            Expr::Call(call) => {
                self.value_depth(expr, depth).is_ok()
                    && self
                        .trusted_path(&call.func)
                        .is_some_and(|path| HASHABLE_CALLS.contains(&path.as_str()))
            }
            _ => self
                .value_depth(expr, depth)
                .is_ok_and(|v| hashable_value(&v)),
        }
    }

    /// The elements of a statically known iterable, when iterating it
    /// surely doesn't raise.
    fn iterable(&self, expr: &'a Expr, depth: u8) -> Option<Vec<Elem<'s, 'a>>> {
        if depth > DEPTH {
            return None;
        }
        let elems = self.sequence_depth(expr, depth).ok()?;
        elems
            .iter()
            .all(|elem| match elem {
                Elem::Value(_) => true,
                Elem::Expr(expr, lookup) => lookup.safe(expr, depth + 1),
            })
            .then_some(elems)
    }

    /// Whether the elements of a list, tuple or set display (with `*`
    /// unpacking) are surely built, and hashable when `hashed`.
    fn elements(&self, elts: &'a [Expr], hashed: bool, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        elts.iter().all(|elt| match elt {
            Expr::Starred(starred) => {
                self.iterable(&starred.value, depth + 1)
                    .is_some_and(|elems| {
                        !hashed
                            || elems.iter().all(|elem| match elem {
                                Elem::Value(value) => hashable_value(value),
                                Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                            })
                    })
            }
            elt => self.safe(elt, depth + 1) && (!hashed || self.hashable(elt, depth + 1)),
        })
    }

    /// Whether a dict display is surely built: hashable keys, and `**`
    /// of dict displays.
    fn mapping(&self, dict: &'a ast::ExprDict, depth: u8) -> bool {
        if depth > DEPTH {
            return false;
        }
        dict.items.iter().all(|item| {
            let Some(key) = &item.key else {
                let (value, lookup) = deref(*self, &item.value);
                return matches!(value, Expr::Dict(inner) if lookup.mapping(inner, depth + 1));
            };
            self.safe(key, depth + 1)
                && self.hashable(key, depth + 1)
                && self.safe(&item.value, depth + 1)
        })
    }

    /// Whether a list or set comprehension is surely built: one `for` over
    /// a known iterable, no condition, making its elements or a literal.
    fn comprehension(
        &self,
        elt: &'a Expr,
        generators: &'a [ast::Comprehension],
        hashed: bool,
        depth: u8,
    ) -> bool {
        let [generator] = generators else {
            return false;
        };
        let Expr::Name(target) = &generator.target else {
            return false;
        };
        if generator.is_async || !generator.ifs.is_empty() {
            return false;
        }
        let Some(elems) = self.iterable(&generator.iter, depth + 1) else {
            return false;
        };
        match elt {
            Expr::Name(name) if name.id == target.id => {
                !hashed
                    || elems.iter().all(|elem| match elem {
                        Elem::Value(value) => hashable_value(value),
                        Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                    })
            }
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_) => self.safe(elt, depth + 1),
            _ => false,
        }
    }

    /// Whether a call to one of [`NO_ID`]'s callables surely makes its
    /// object: arguments it accepts, of types it accepts, with values it
    /// accepts. Anything aye can't tell is no.
    fn constructs(&self, path: &str, call: &'a ExprCall, depth: u8) -> bool {
        let arguments = &call.arguments;
        if arguments.args.iter().any(Expr::is_starred_expr)
            || arguments.keywords.iter().any(|k| k.arg.is_none())
        {
            return false;
        }
        let args: Vec<&'a Expr> = arguments.args.iter().collect();
        let keywords: Vec<(&str, &'a Expr)> = arguments
            .keywords
            .iter()
            .filter_map(|k| Some((k.arg.as_ref()?.id.as_str(), &k.value)))
            .collect();
        let value = |expr: &'a Expr| self.value_depth(expr, depth + 1).ok();
        let int = |expr: &'a Expr| match value(expr) {
            Some(Value::Int(digits)) => digits.parse::<i128>().ok(),
            Some(Value::Bool(b)) => Some(i128::from(b)),
            _ => None,
        };
        match path {
            "builtins.object" | "collections.defaultdict" => args.is_empty() && keywords.is_empty(),
            "builtins.range" => {
                if !keywords.is_empty() || !(1..=3).contains(&args.len()) {
                    return false;
                }
                // Any size of int; a step of 0 raises.
                let ints: Vec<Option<bool>> = args
                    .iter()
                    .map(|arg| match value(arg) {
                        Some(Value::Int(digits)) => {
                            Some(digits.trim_start_matches('-').bytes().all(|b| b == b'0'))
                        }
                        Some(Value::Bool(b)) => Some(!b),
                        _ => None,
                    })
                    .collect();
                ints.iter().all(Option::is_some) && !(args.len() == 3 && ints[2] == Some(true))
            }
            "builtins.list" | "builtins.tuple" | "collections.deque" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => self.iterated(arg, false, depth + 1),
                        _ => false,
                    }
            }
            "builtins.set" | "builtins.frozenset" | "collections.Counter" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => self.iterated(arg, true, depth + 1),
                        _ => false,
                    }
            }
            "builtins.dict" | "collections.OrderedDict" => {
                keywords.iter().all(|(_, v)| self.safe(v, depth + 1))
                    && match args.as_slice() {
                        [] => true,
                        [arg] => {
                            let (arg, lookup) = deref(*self, arg);
                            match arg {
                                Expr::Dict(dict) => lookup.mapping(dict, depth + 1),
                                // Pairs, each a two-item display.
                                _ => lookup.iterable(arg, depth + 1).is_some_and(|elems| {
                                    elems.iter().all(|elem| match elem {
                                        Elem::Expr(Expr::Tuple(pair), lookup) => {
                                            pair.elts.len() == 2
                                                && lookup.elements(&pair.elts, false, depth + 1)
                                                && lookup.hashable(&pair.elts[0], depth + 1)
                                        }
                                        Elem::Expr(Expr::List(pair), lookup) => {
                                            pair.elts.len() == 2
                                                && lookup.elements(&pair.elts, false, depth + 1)
                                                && lookup.hashable(&pair.elts[0], depth + 1)
                                        }
                                        _ => false,
                                    })
                                }),
                            }
                        }
                        _ => false,
                    }
            }
            "builtins.bytearray" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => match value(arg) {
                            Some(Value::Bytes(_)) => true,
                            Some(Value::Int(_) | Value::Bool(_)) => {
                                int(arg).is_some_and(|n| (0..=1_000_000).contains(&n))
                            }
                            _ => self.iterable(arg, depth + 1).is_some_and(|elems| {
                                elems.iter().all(|elem| {
                                    let byte = match elem {
                                        Elem::Value(Value::Int(digits)) => {
                                            digits.parse::<i64>().ok()
                                        }
                                        Elem::Expr(expr, lookup) => {
                                            match lookup.value_depth(expr, depth + 1) {
                                                Ok(Value::Int(digits)) => digits.parse().ok(),
                                                Ok(Value::Bool(b)) => Some(i64::from(b)),
                                                _ => None,
                                            }
                                        }
                                        Elem::Value(_) => None,
                                    };
                                    byte.is_some_and(|b| (0..=255).contains(&b))
                                })
                            }),
                        },
                        _ => false,
                    }
            }
            "builtins.slice" => {
                keywords.is_empty()
                    && (1..=3).contains(&args.len())
                    && args.iter().all(|arg| self.safe(arg, depth + 1))
            }
            "decimal.Decimal" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [arg] => match value(arg) {
                            Some(Value::Int(_) | Value::Float(_) | Value::Bool(_)) => true,
                            Some(Value::Str(text)) => decimal_parses(&text),
                            _ => false,
                        },
                        _ => false,
                    }
            }
            "fractions.Fraction" => {
                keywords.is_empty()
                    && match args.as_slice() {
                        [] => true,
                        [n] => int(n).is_some(),
                        [n, d] => int(n).is_some() && int(d).is_some_and(|d| d != 0),
                        _ => false,
                    }
            }
            "pathlib.Path"
            | "pathlib.PurePath"
            | "pathlib.PurePosixPath"
            | "pathlib.PureWindowsPath"
            | "pathlib.PosixPath"
            | "pathlib.WindowsPath" => {
                // A concrete path of the other platform can't be made.
                let platform = match path {
                    "pathlib.PosixPath" => !cfg!(windows),
                    "pathlib.WindowsPath" => cfg!(windows),
                    _ => true,
                };
                platform
                    && keywords.is_empty()
                    && args
                        .iter()
                        .all(|arg| matches!(value(arg), Some(Value::Str(_))))
            }
            "uuid.UUID" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [arg] if matches!(value(arg), Some(Value::Str(text)) if uuid_parses(&text)))
            }
            "datetime.date" | "datetime.datetime" | "datetime.time" => {
                let ints: Option<Vec<i128>> = args.iter().map(|arg| int(arg)).collect();
                keywords.is_empty() && ints.is_some_and(|ints| date_time_valid(path, &ints))
            }
            "datetime.timedelta" => {
                const UNITS: [&str; 7] = [
                    "days",
                    "seconds",
                    "microseconds",
                    "milliseconds",
                    "minutes",
                    "hours",
                    "weeks",
                ];
                args.len() <= UNITS.len()
                    && keywords
                        .iter()
                        .all(|(name, _)| UNITS[args.len()..].contains(name))
                    && args
                        .iter()
                        .chain(keywords.iter().map(|(_, v)| v))
                        .all(|arg| match value(arg) {
                            Some(Value::Int(_) | Value::Bool(_)) => {
                                int(arg).is_some_and(|n| n.abs() <= 1_000_000)
                            }
                            Some(Value::Float(f)) => f.abs() <= 1e6,
                            _ => false,
                        })
            }
            "numpy.zeros" | "numpy.ones" | "numpy.empty" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [shape] if self.shape(shape, depth))
            }
            "numpy.full" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [shape, fill] if self.shape(shape, depth)
                        && matches!(value(fill), Some(Value::Int(_) | Value::Float(_) | Value::Bool(_))))
            }
            "numpy.arange" => {
                let numbers: Option<Vec<f64>> = args
                    .iter()
                    .map(|arg| match value(arg) {
                        Some(Value::Int(digits)) => digits.parse::<f64>().ok(),
                        Some(Value::Float(f)) => Some(f),
                        _ => None,
                    })
                    .collect();
                keywords.is_empty()
                    && numbers.is_some_and(|n| {
                        let (first, end, stride) = match n.as_slice() {
                            [end] => (0.0, *end, 1.0),
                            [first, end] => (*first, *end, 1.0),
                            [first, end, stride] => (*first, *end, *stride),
                            _ => return false,
                        };
                        stride != 0.0
                            && [first, end, stride].iter().all(|x| x.is_finite())
                            && ((end - first) / stride) < 1e7
                    })
            }
            "numpy.linspace" => {
                let number = |arg: &'a Expr| {
                    matches!(
                        value(arg),
                        Some(Value::Int(_) | Value::Float(_) | Value::Bool(_))
                    )
                };
                keywords.is_empty()
                    && match args.as_slice() {
                        [a, b] => number(a) && number(b),
                        [a, b, n] => {
                            number(a)
                                && number(b)
                                && int(n).is_some_and(|n| (0..10_000_000).contains(&n))
                        }
                        _ => false,
                    }
            }
            "numpy.array" | "numpy.asarray" => {
                keywords.is_empty()
                    && matches!(args.as_slice(), [arg] if self.rectangular(arg, depth + 1).is_some())
            }
            // pandas parses and checks its arguments in ways aye doesn't
            // follow.
            _ => false,
        }
    }

    /// Whether iterating `expr` surely works, and gives hashable elements
    /// when `hashed`: a known sequence, or a set or dict display.
    fn iterated(&self, expr: &'a Expr, hashed: bool, depth: u8) -> bool {
        let (expr, lookup) = deref(*self, expr);
        match expr {
            Expr::Set(set) => lookup.elements(&set.elts, true, depth),
            Expr::Dict(dict) => lookup.mapping(dict, depth),
            _ => lookup.iterable(expr, depth).is_some_and(|elems| {
                !hashed
                    || elems.iter().all(|elem| match elem {
                        Elem::Value(value) => hashable_value(value),
                        Elem::Expr(expr, lookup) => lookup.hashable(expr, depth + 1),
                    })
            }),
        }
    }

    /// A numpy shape: a non-negative int, or a tuple or list of them.
    fn shape(&self, expr: &'a Expr, depth: u8) -> bool {
        let dimension = |expr: &'a Expr| match self.value_depth(expr, depth + 1) {
            Ok(Value::Int(digits)) => digits.parse::<u32>().is_ok_and(|n| n <= 10_000_000),
            _ => false,
        };
        match expr {
            Expr::Tuple(tuple) => tuple.elts.iter().all(dimension),
            Expr::List(list) => list.elts.iter().all(dimension),
            other => dimension(other),
        }
    }

    /// The shape of nested lists and tuples of numbers, booleans or
    /// strings that numpy makes an array of (it refuses ragged ones).
    fn rectangular(&self, expr: &'a Expr, depth: u8) -> Option<Vec<usize>> {
        if depth > DEPTH {
            return None;
        }
        let elts = match expr {
            Expr::List(list) => &list.elts,
            Expr::Tuple(tuple) => &tuple.elts,
            other => {
                return matches!(
                    self.value_depth(other, depth),
                    Ok(Value::Int(_) | Value::Float(_) | Value::Bool(_) | Value::Str(_))
                )
                .then(Vec::new);
            }
        };
        let mut inner: Option<Vec<usize>> = None;
        for elt in elts {
            let shape = self.rectangular(elt, depth + 1)?;
            match &inner {
                Some(found) if *found != shape => return None,
                Some(_) => {}
                None => inner = Some(shape),
            }
        }
        let mut shape = vec![elts.len()];
        shape.extend(inner.unwrap_or_default());
        Some(shape)
    }

    /// Whether `flags` for `re.compile` are known to be accepted with any
    /// pattern: `0`, `re.I`, `re.M`, `re.S` and their unions.
    fn regex_flags(&self, flags: &'a Expr) -> bool {
        match flags {
            Expr::NumberLiteral(n) => matches!(&n.value, Number::Int(i) if i.as_u64() == Some(0)),
            Expr::BinOp(op) if matches!(op.op, Operator::BitOr) => {
                self.regex_flags(&op.left) && self.regex_flags(&op.right)
            }
            other => matches!(
                self.trusted_path(other).as_deref(),
                Some("re.I" | "re.IGNORECASE" | "re.M" | "re.MULTILINE" | "re.S" | "re.DOTALL")
            ),
        }
    }

    /// The elements of a statically known iterable: list, tuple and dict
    /// literals (keys), strings, `range(...)`, concatenations, and names
    /// bound to those.
    pub fn sequence(&self, expr: &'a Expr) -> Result<Vec<Elem<'s, 'a>>, Unknown> {
        self.sequence_depth(expr, 0)
    }

    fn sequence_depth(&self, expr: &'a Expr, depth: u8) -> Result<Vec<Elem<'s, 'a>>, Unknown> {
        if depth > DEPTH {
            return Err(Unknown);
        }
        let elements = |elts: &'a [Expr]| -> Result<Vec<Elem<'s, 'a>>, Unknown> {
            let mut out = Vec::with_capacity(elts.len());
            for elt in elts {
                if let Expr::Starred(starred) = elt {
                    out.extend(self.sequence_depth(&starred.value, depth + 1)?);
                } else {
                    out.push(Elem::Expr(elt, *self));
                }
            }
            Ok(out)
        };
        match expr {
            Expr::List(list) => elements(&list.elts),
            Expr::Tuple(tuple) => elements(&tuple.elts),
            // A dict keeps one entry per distinct key, the first key written
            // in the first key's place: `{1: …, True: …, 1.0: …}` has one.
            Expr::Dict(dict) => {
                let mut seen: HashSet<DictKey> = HashSet::new();
                let mut out = Vec::new();
                for item in &dict.items {
                    let key = item.key.as_ref().ok_or(Unknown)?;
                    let value = self.value_depth(key, depth + 1)?;
                    // Its values are evaluated too, and may raise.
                    if !self.safe(&item.value, depth + 1) {
                        return Err(Unknown);
                    }
                    if seen.insert(DictKey::of(&value).ok_or(Unknown)?) {
                        out.push(Elem::Expr(key, *self));
                    }
                }
                // Huge ints and huge floats may be equal; not compared here.
                if seen.iter().any(|k| matches!(k, DictKey::BigInt(_)))
                    && seen.iter().any(|k| matches!(k, DictKey::BigFloat(_)))
                {
                    return Err(Unknown);
                }
                Ok(out)
            }
            Expr::StringLiteral(_) => match self.value(expr)? {
                Value::Str(text) => Ok(text
                    .chars()
                    .map(|c| Elem::Value(Value::Str(c.to_string())))
                    .collect()),
                _ => Err(Unknown),
            },
            Expr::BytesLiteral(b) => Ok(b
                .value
                .bytes()
                .map(|byte| Elem::Value(Value::Int(byte.to_string())))
                .collect()),
            Expr::BinOp(bin) if matches!(bin.op, Operator::Add) => {
                if !is_sequence_literal_like(&bin.left) && !is_sequence_literal_like(&bin.right) {
                    return Err(Unknown);
                }
                // Only a list and a list, or a tuple and a tuple, add up.
                match (
                    self.kind(&bin.left, depth + 1),
                    self.kind(&bin.right, depth + 1),
                ) {
                    (Some(a), Some(b)) if a == b && matches!(a, Kind::List | Kind::Tuple) => {}
                    _ => return Err(Unknown),
                }
                let mut left = self.sequence_depth(&bin.left, depth + 1)?;
                left.extend(self.sequence_depth(&bin.right, depth + 1)?);
                Ok(left)
            }
            Expr::Name(name) => {
                let (value, scope) = self.constant(name.id.as_str()).ok_or(Unknown)?;
                if !is_sequence_literal_like(value) {
                    return Err(Unknown);
                }
                scope.sequence_depth(value, depth + 1)
            }
            Expr::Call(call) => self.range_call(call).map(|ints| {
                ints.into_iter()
                    .map(|i| Elem::Value(Value::Int(i.to_string())))
                    .collect()
            }),
            _ => Err(Unknown),
        }
    }

    /// What kind of sequence an expression aye reads as one makes.
    fn kind(&self, expr: &'a Expr, depth: u8) -> Option<Kind> {
        if depth > DEPTH {
            return None;
        }
        match expr {
            Expr::List(_) => Some(Kind::List),
            Expr::Tuple(_) => Some(Kind::Tuple),
            Expr::Name(name) => {
                let (value, scope) = self.constant(name.id.as_str())?;
                scope.kind(value, depth + 1)
            }
            Expr::BinOp(bin) if matches!(bin.op, Operator::Add) => {
                let kind = self.kind(&bin.left, depth + 1)?;
                (self.kind(&bin.right, depth + 1)? == kind).then_some(kind)
            }
            _ => None,
        }
    }

    fn range_call(&self, call: &'a ExprCall) -> Result<Vec<i64>, Unknown> {
        if self.trusted_path(&call.func).as_deref() != Some("builtins.range")
            || !call.arguments.keywords.is_empty()
        {
            return Err(Unknown);
        }
        let mut bounds = Vec::new();
        for arg in &call.arguments.args {
            match self.value(arg)? {
                Value::Int(digits) => bounds.push(digits.parse::<i64>().map_err(|_| Unknown)?),
                _ => return Err(Unknown),
            }
        }
        let (first, end, stride) = match bounds.as_slice() {
            [end] => (0, *end, 1),
            [first, end] => (*first, *end, 1),
            [first, end, stride] if *stride != 0 => (*first, *end, *stride),
            _ => return Err(Unknown),
        };
        let mut out = Vec::new();
        let mut i = first;
        while (stride > 0 && i < end) || (stride < 0 && i > end) {
            out.push(i);
            if out.len() > 100_000 {
                return Err(Unknown);
            }
            // Past the range of `i64`, the next value is past `end` too.
            match i.checked_add(stride) {
                Some(next) => i = next,
                None => break,
            }
        }
        Ok(out)
    }

    /// A string value, statically.
    pub fn string(&self, expr: &'a Expr) -> Option<String> {
        match self.value(expr) {
            Ok(Value::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// A boolean value, statically (`True`, `False`, or a constant).
    pub fn boolean(&self, expr: &'a Expr) -> Option<bool> {
        match self.value(expr) {
            Ok(Value::Bool(b)) => Some(b),
            _ => None,
        }
    }
}

/// A value with no id of its own, when it is surely built.
fn opaque(built: bool) -> Result<Value, Unknown> {
    if built {
        Ok(Value::Opaque)
    } else {
        Err(Unknown)
    }
}

/// Calls whose results are hashable, once made.
const HASHABLE_CALLS: &[&str] = &[
    "builtins.float",
    "builtins.frozenset",
    "builtins.object",
    "builtins.range",
    "datetime.date",
    "datetime.datetime",
    "datetime.time",
    "datetime.timedelta",
    "decimal.Decimal",
    "fractions.Fraction",
    "pathlib.Path",
    "pathlib.PosixPath",
    "pathlib.PurePath",
    "pathlib.PurePosixPath",
    "pathlib.PureWindowsPath",
    "pathlib.WindowsPath",
    "re.compile",
    "uuid.UUID",
];

/// Whether a value aye knows is hashable: not a container (whose kind
/// [`Value::Opaque`] doesn't tell).
fn hashable_value(value: &Value) -> bool {
    !matches!(value, Value::Opaque | Value::Unknown)
}

/// Whether `Decimal(text)` surely parses `text`: a plain decimal number
/// in ASCII, or an infinity or a NaN, with ASCII spaces around (a subset
/// of what it accepts).
fn decimal_parses(text: &str) -> bool {
    let text = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let lower = unsigned.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "infinity") {
        return true;
    }
    if let Some(digits) = lower
        .strip_prefix("nan")
        .or_else(|| lower.strip_prefix("snan"))
    {
        return digits.bytes().all(|b| b.is_ascii_digit());
    }
    let (mantissa, exponent) = match lower.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (lower.as_str(), None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => {
            (digits(int) || int.is_empty())
                && (digits(frac) || frac.is_empty())
                && !(int.is_empty() && frac.is_empty())
        }
        None => digits(mantissa),
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    mantissa_ok && exponent_ok
}

/// Whether `UUID(text)` surely parses `text`: 32 hex digits once
/// `urn:`, `uuid:`, braces and hyphens are taken out, as it takes them.
fn uuid_parses(text: &str) -> bool {
    let hex = text.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(['{', '}']).replace('-', "");
    hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether `date(…)`, `datetime(…)` or `time(…)` accepts these ints.
fn date_time_valid(path: &str, ints: &[i128]) -> bool {
    let date_ok = |year: i128, month: i128, day: i128| {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return false,
        };
        (1..=9999).contains(&year) && (1..=days).contains(&day)
    };
    let time_ok = |parts: &[i128]| {
        let limits = [23, 59, 59, 999_999];
        parts.len() <= limits.len()
            && parts
                .iter()
                .zip(limits)
                .all(|(part, limit)| (0..=limit).contains(part))
    };
    match path {
        "datetime.date" => matches!(ints, [y, m, d] if date_ok(*y, *m, *d)),
        "datetime.datetime" => {
            ints.len() >= 3 && date_ok(ints[0], ints[1], ints[2]) && time_ok(&ints[3..])
        }
        "datetime.time" => time_ok(ints),
        _ => false,
    }
}

/// A sequence's type, for `+`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    List,
    Tuple,
}

fn is_sequence_literal_like(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::List(_) | Expr::Tuple(_) | Expr::Name(_) | Expr::BinOp(_) | Expr::Call(_)
    )
}

fn complex_parts(value: &Value) -> Option<(f64, f64)> {
    match value {
        Value::Int(digits) => Some((digits.parse().ok()?, 0.0)),
        Value::Float(f) => Some((*f, 0.0)),
        Value::Complex(re, im) => Some((*re, *im)),
        Value::Bool(b) => Some((f64::from(u8::from(*b)), 0.0)),
        _ => None,
    }
}

fn negate(digits: &str) -> String {
    match digits.strip_prefix('-') {
        Some(rest) => rest.to_string(),
        None if digits == "0" => digits.to_string(),
        None => format!("-{digits}"),
    }
}

/// The longest int whose `str()` (its id) surely works: Python's limit
/// (`sys.set_int_max_str_digits`) is 4300 digits by default and can be
/// lowered to 640.
const MAX_INT_DIGITS: usize = 640;

/// A string's value. A lone surrogate escape (`"\ud800"`) reads as U+FFFD,
/// so any U+FFFD is left to Python: its id may differ.
fn literal_text(text: String) -> Result<Value, Unknown> {
    if text.contains('\u{fffd}') {
        Err(Unknown)
    } else {
        Ok(Value::Str(text))
    }
}

/// A dict key as Python compares keys: numbers by value across `bool`,
/// `int`, `float` and `complex`; strings and bytes never equal.
#[derive(Debug, PartialEq, Eq, Hash)]
enum DictKey {
    None,
    Str(String),
    Bytes(Vec<u8>),
    /// Integral numbers that fit (`1`, `True`, `1.0`, `1+0j`).
    Int(i128),
    /// The decimal digits of a larger int.
    BigInt(String),
    /// The bits of a larger integral float.
    BigFloat(u64),
    /// The bits of any other float.
    Float(u64),
    /// The bits of a complex number's parts, when it is not real.
    Complex(u64, u64),
}

impl DictKey {
    /// `None` for values that can't be compared statically (a `nan` is
    /// equal only to itself, by identity).
    fn of(value: &Value) -> Option<Self> {
        Some(match value {
            Value::None => Self::None,
            Value::Str(s) => Self::Str(s.clone()),
            Value::Bytes(b) => Self::Bytes(b.clone()),
            Value::Bool(b) => Self::Int(i128::from(*b)),
            Value::Int(digits) => digits
                .parse()
                .map_or_else(|_| Self::BigInt(digits.clone()), Self::Int),
            Value::Float(f) => Self::float(*f)?,
            Value::Complex(re, im) if *im == 0.0 => Self::float(*re)?,
            Value::Complex(re, im) if re.is_nan() || im.is_nan() => return None,
            // `-0.0 == 0.0`.
            Value::Complex(re, im) => Self::Complex((re + 0.0).to_bits(), im.to_bits()),
            _ => return None,
        })
    }

    fn float(f: f64) -> Option<Self> {
        // 2^126: integral floats below convert to `i128` exactly.
        const LIMIT: f64 = 85_070_591_730_234_615_865_843_651_857_942_052_864.0;
        if f.is_nan() {
            None
        } else if f.is_infinite() || f.fract() != 0.0 {
            Some(Self::Float(f.to_bits()))
        } else if f.abs() < LIMIT {
            #[expect(clippy::cast_possible_truncation, reason = "integral and below 2^126")]
            Some(Self::Int(f as i128))
        } else {
            Some(Self::BigFloat(f.to_bits()))
        }
    }
}

/// Whether `re.compile` surely accepts a pattern, `str` or `bytes`, on
/// every Python pytest 9.1.1 supports.
fn regex_compiles(pattern: &Value) -> bool {
    match pattern {
        Value::Str(s) => re::compiles(s),
        Value::Bytes(b) => re::compiles_bytes(b),
        _ => false,
    }
}

/// Follow names bound to constants.
pub fn deref<'s, 'a>(lookup: Lookup<'s, 'a>, expr: &'a Expr) -> (&'a Expr, Lookup<'s, 'a>) {
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
