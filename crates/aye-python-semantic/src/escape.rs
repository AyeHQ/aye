//! The escape analysis behind [`Namespace::build`]: which values of a
//! body code aye can't follow may change, which modules code that runs
//! may change, and whether it reaches module namespaces in ways aye
//! can't follow.

use aye_python_stdlib::builtins::builtin_name;
use ruff_python_ast::{
    self as ast, Expr, ExprAttribute, ExprCall, ExprContext, Parameters, Pattern, Stmt,
    StmtClassDef, StmtFunctionDef,
};
use ruff_text_size::{Ranged, TextSize};

use crate::hash::{FastMap, FastSet};
use crate::namespace::{
    Binding, Bound, MUTATORS, Namespace, children, evaluated, pattern_captures, walrus_targets,
};
use crate::resolve::{Lookup, Resolved};

/// The escape analysis's verdicts on one body (see [`Escapes`]).
#[derive(Debug, Default)]
pub(crate) struct Escaped<'a> {
    /// The body's names whose value code aye can't follow may change, with
    /// every name that may share that value.
    pub(crate) changed: FastSet<&'a str>,
    /// The same for the names of each class body walked, by where the
    /// body starts.
    pub(crate) classes: FastMap<TextSize, FastSet<&'a str>>,
    /// Attributes whose value may change, read through any object: class
    /// attributes of these names may change.
    pub(crate) attributes_changed: FastSet<&'a str>,
    /// Names whose `__name__` may be reassigned.
    pub(crate) renamed: FastSet<&'a str>,
    /// Some `__name__` is reassigned through something other than a name.
    pub(crate) renamed_unknown: bool,
    /// Attributes assigned or deleted on some object (`Klass.attr = …`):
    /// class attributes of these names may be rebound.
    pub(crate) attributes: FastSet<&'a str>,
    /// Class bodies (by where they start) whose class code aye can't follow
    /// gets (a decorator, a metaclass, a base from elsewhere, or handed
    /// on): any of their attributes may change.
    pub(crate) classes_all: FastSet<TextSize>,
    /// Modules (dotted paths) whose attributes code that runs may change:
    /// set or deleted through a name bound to them, or the module handed to
    /// code aye can't follow (`setattr`, `vars(m)[…]`, `m.__dict__`).
    pub(crate) patched: FastSet<String>,
    /// Any module's attributes may change (see [`Namespace::reflective`]).
    pub(crate) patches_all: bool,
    /// Modules that code that runs imports or looks up by a literal name
    /// (`sys.modules["x"]`, `importlib.import_module("x")`), as written.
    pub(crate) modules: Vec<String>,
    /// Names of the body whose value code aye can't follow may get hold
    /// of (handed to an unknown call, a loop over it, an alias whose
    /// attributes are set, `del NAME.attr`): a class may gain or lose
    /// members, a function its `__test__` or marks. A class's own
    /// decorators, keywords and bases don't count here, nor `NAME.attr =
    /// …` statements of the body, which recognition reads.
    pub(crate) handed: FastSet<&'a str>,
    /// The same for every class statement walked, by where its body
    /// starts.
    pub(crate) handed_classes: FastSet<TextSize>,
    /// The same for attributes read through any object (`TestK.test_a`):
    /// a method of that name may change.
    pub(crate) handed_attributes: FastSet<&'a str>,
    /// Names functions that may run during the import declare `global`
    /// and bind or delete, with the value assigned (`None` when not an
    /// assignment).
    pub(crate) global_writes: Vec<(&'a str, Option<&'a Expr>)>,
}

/// Methods of the builtin containers and strings that neither change their
/// receiver nor keep their arguments.
const PURE_METHODS: &[&str] = &[
    "capitalize",
    "casefold",
    "center",
    "copy",
    "count",
    "decode",
    "difference",
    "encode",
    "endswith",
    "expandtabs",
    "find",
    "format",
    "format_map",
    "get",
    "hex",
    "index",
    "intersection",
    "isalnum",
    "isalpha",
    "isascii",
    "isdecimal",
    "isdigit",
    "isdisjoint",
    "isidentifier",
    "islower",
    "isnumeric",
    "isprintable",
    "isspace",
    "issubset",
    "issuperset",
    "istitle",
    "isupper",
    "items",
    "join",
    "keys",
    "ljust",
    "lower",
    "lstrip",
    "partition",
    "removeprefix",
    "removesuffix",
    "replace",
    "rfind",
    "rindex",
    "rjust",
    "rpartition",
    "rsplit",
    "rstrip",
    "split",
    "splitlines",
    "startswith",
    "strip",
    "swapcase",
    "symmetric_difference",
    "title",
    "union",
    "upper",
    "values",
    "zfill",
];

/// Builtins that read their arguments without keeping or changing them
/// (they may call the arguments' dunder methods).
const READ_BUILTINS: &[&str] = &[
    "builtins.abs",
    "builtins.all",
    "builtins.any",
    "builtins.ascii",
    "builtins.bin",
    "builtins.bool",
    "builtins.callable",
    "builtins.chr",
    "builtins.complex",
    "builtins.divmod",
    "builtins.float",
    "builtins.format",
    "builtins.hasattr",
    "builtins.hash",
    "builtins.hex",
    "builtins.id",
    "builtins.int",
    "builtins.isinstance",
    "builtins.issubclass",
    "builtins.len",
    "builtins.oct",
    "builtins.ord",
    "builtins.pow",
    "builtins.print",
    "builtins.range",
    "builtins.repr",
    "builtins.round",
    "builtins.str",
    "builtins.sum",
    "builtins.type",
    "copy.deepcopy",
];

/// Builtins whose result holds their positional arguments or their
/// elements (when called without keywords, which may pass callables).
const FLOW_BUILTINS: &[&str] = &[
    "builtins.dict",
    "builtins.enumerate",
    "builtins.frozenset",
    "builtins.getattr",
    "builtins.iter",
    "builtins.list",
    "builtins.max",
    "builtins.min",
    "builtins.next",
    "builtins.reversed",
    "builtins.set",
    "builtins.sorted",
    "builtins.tuple",
    "builtins.vars",
    "builtins.zip",
    "copy.copy",
];

/// Dunder methods that run only once their class is used: they need an
/// explicit call, an instance, a subclass or a subscript.
const EXPLICIT_DUNDERS: &[&str] = &[
    "__call__",
    "__class_getitem__",
    "__init__",
    "__init_subclass__",
    "__new__",
    "__post_init__",
    "__set_name__",
];

/// Dunder methods whose arguments after the first never carry a value the
/// module's code holds: they get nothing, names, keys of their own, or
/// values already counted as escaping (`obj.x = v`, `obj[k] = v`).
const NO_OPERAND_DUNDERS: &[&str] = &[
    "__abs__",
    "__aenter__",
    "__aexit__",
    "__aiter__",
    "__anext__",
    "__await__",
    "__bool__",
    "__bytes__",
    "__ceil__",
    "__complex__",
    "__copy__",
    "__del__",
    "__delattr__",
    "__delete__",
    "__dir__",
    "__enter__",
    "__exit__",
    "__float__",
    "__floor__",
    "__fspath__",
    "__get__",
    "__getattr__",
    "__getattribute__",
    "__hash__",
    "__index__",
    "__int__",
    "__invert__",
    "__iter__",
    "__len__",
    "__length_hint__",
    "__neg__",
    "__next__",
    "__pos__",
    "__reduce__",
    "__repr__",
    "__reversed__",
    "__round__",
    "__set__",
    "__setattr__",
    "__sizeof__",
    "__str__",
    "__trunc__",
];

/// Attributes that reach a module's namespace from a frame or a function:
/// code that reads them may change any module's names.
fn is_frame_attribute(name: &str) -> bool {
    matches!(
        name,
        "__globals__"
            | "ag_frame"
            | "cr_frame"
            | "f_back"
            | "f_builtins"
            | "f_globals"
            | "f_locals"
            | "gi_frame"
            | "tb_frame"
    )
}

/// The last part of each of [`REFLECTIVE`]'s paths, to resolve only the
/// names that may be one.
fn may_be_reflective(name: &str) -> bool {
    matches!(
        name,
        "__import__"
            | "_current_frames"
            | "_getframe"
            | "currentframe"
            | "get_objects"
            | "get_referents"
            | "get_referrers"
            | "getinnerframes"
            | "getmodule"
            | "getouterframes"
            | "import_module"
            | "modules"
            | "reload"
            | "stack"
            | "trace"
    )
}

/// What reaches module namespaces (this module's, or any module's
/// attributes) in ways aye can't follow: modules by computed name, frames,
/// the garbage collector's view of every object.
const REFLECTIVE: &[&str] = &[
    "__import__",
    "builtins.__import__",
    "gc.get_objects",
    "gc.get_referents",
    "gc.get_referrers",
    "importlib.__import__",
    "importlib.import_module",
    "importlib.reload",
    "inspect.currentframe",
    "inspect.getinnerframes",
    "inspect.getmodule",
    "inspect.getouterframes",
    "inspect.stack",
    "inspect.trace",
    "sys._current_frames",
    "sys._getframe",
    "sys.modules",
];

fn is_reflective(path: &str) -> bool {
    REFLECTIVE.contains(&path)
}

/// Whether `expr` is `sys.modules`.
fn is_sys_modules<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> bool {
    let named = match expr {
        Expr::Attribute(attr) => attr.attr.id.as_str() == "modules",
        Expr::Name(name) => name.id.as_str() == "modules",
        _ => false,
    };
    named && scope.path(expr).as_deref() == Some("sys.modules")
}

/// Whether a class base is one whose effect on its subclasses aye follows:
/// a class of the module (its `__init_subclass__` is walked), or one of the
/// standard library's that leaves subclasses' attributes alone.
fn followed_base<'a>(base: &'a Expr, scope: Lookup<'_, 'a>) -> bool {
    let target = match base {
        Expr::Subscript(sub) => &*sub.value,
        other => other,
    };
    match scope.resolve(target) {
        Resolved::Local(Bound {
            binding: Binding::Class(_),
            ..
        }) => true,
        Resolved::Path(path) => [
            "builtins.",
            "unittest.",
            "abc.",
            "typing.",
            "typing_extensions.",
            "collections.abc.",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix)),
        _ => false,
    }
}

fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

/// Decorators known to neither call the function they get nor rename it.
fn harmless_decorator(path: Option<&str>) -> bool {
    let Some(path) = path else {
        return false;
    };
    [
        "pytest.",
        "_pytest.",
        "unittest.mock.patch",
        "mock.patch",
        "unittest.skip",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
        || matches!(
            path,
            "abc.abstractmethod"
                | "builtins.classmethod"
                | "builtins.staticmethod"
                | "dataclasses.dataclass"
                | "functools.total_ordering"
                | "functools.wraps"
                | "typing.final"
                | "typing.overload"
                | "typing.override"
                | "typing_extensions.final"
                | "typing_extensions.overload"
                | "typing_extensions.override"
                | "unittest.expectedFailure"
        )
}

/// The dotted path a callee names, when its first name is bound to an
/// import or is a builtin (empty for imports `callee` has nothing to say
/// about). Other names resolve to themselves, which for a function's
/// parameters and locals says nothing.
fn known_path<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> Option<String> {
    let mut root = expr;
    while let Expr::Attribute(attr) = root {
        root = &attr.value;
    }
    let Expr::Name(name) = root else {
        return None;
    };
    let name = name.id.as_str();
    match scope.binding(name) {
        None if builtin_name(name).is_none() => None,
        // Only paths under these matter to `callee`; skip building others.
        Some(Bound {
            binding: Binding::Import(path),
            ..
        }) if !["pytest", "_pytest", "copy"]
            .contains(&path.split('.').next().unwrap_or_default()) =>
        {
            Some(String::new())
        }
        _ => scope.path(expr),
    }
}

/// What a decorator expression resolves to (the callee, for a call).
fn decorator_path<'a>(expr: &'a Expr, scope: Lookup<'_, 'a>) -> Option<String> {
    match expr {
        Expr::Call(call) => scope.path(&call.func),
        other => scope.path(other),
    }
}

/// What a name in some code refers to, for the escape analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key<'a> {
    /// A name of the analyzed body.
    Global(&'a str),
    /// A name bound in the body of the class at that index.
    Class(usize, &'a str),
    /// A local of the function (or lambda) starting there.
    Local(TextSize, &'a str),
    /// An attribute read through some object (`TestK.CASES`): a class
    /// attribute of any class.
    Attr(&'a str),
}

/// Where a value goes once an expression has produced it.
#[derive(Clone, Default)]
struct Sink<'a> {
    /// Code aye can't follow may keep or change it.
    escape: bool,
    /// It goes to an operator or a builtin, which may call one of the
    /// module's dunder methods with it.
    operand: bool,
    /// Names that hold it from now on (bound to it, or to a container
    /// holding it).
    binds: Vec<Key<'a>>,
    /// Its attributes are set or deleted (it owns an attribute that is
    /// stored to): a module it is changes.
    patch: bool,
    /// Its attributes are set or deleted other than by a statement of the
    /// analyzed body naming it (`K.attr = …`, which the namespace records
    /// in [`Namespace::attributes`]): a class or function it is may gain
    /// or lose tests in ways aye doesn't read.
    touch: bool,
}

impl Sink<'_> {
    fn inert() -> Self {
        Self::default()
    }

    fn operand() -> Self {
        Self {
            operand: true,
            ..Self::default()
        }
    }

    fn escape() -> Self {
        Self {
            escape: true,
            ..Self::default()
        }
    }

    fn with_operand(mut self) -> Self {
        self.operand = true;
        self
    }

    /// The owner of an attribute stored to or deleted.
    fn patched() -> Self {
        Self {
            operand: true,
            patch: true,
            touch: true,
            ..Self::default()
        }
    }

    /// Whether whoever gets the value keeps it.
    fn held(&self) -> bool {
        self.escape || !self.binds.is_empty()
    }

    /// Where the parts of a value go (its elements, the operands it was
    /// computed from): wherever the value is held.
    fn part(&self) -> Self {
        Self {
            escape: self.escape,
            operand: false,
            binds: self.binds.clone(),
            patch: self.patch,
            touch: self.touch,
        }
    }
}

/// What one body of code does with names.
#[derive(Default)]
struct Facts<'a> {
    /// Names it reads (or calls), by name.
    loads: Vec<&'a str>,
    /// Values code aye can't follow may change or keep.
    escapes: Vec<Key<'a>>,
    /// Values changed in place by statements [`Namespace::statement`]
    /// records as ops.
    tracked: Vec<Key<'a>>,
    /// Values going to an operator or builtin.
    operands: Vec<Key<'a>>,
    /// Pairs of names that may hold the same value.
    unions: Vec<(Key<'a>, Key<'a>)>,
    /// Names whose `__name__` is reassigned.
    renamed: Vec<Key<'a>>,
    /// Attributes assigned or deleted.
    attributes: Vec<&'a str>,
    renamed_unknown: bool,
    /// The namespace may change in ways no static reading follows.
    dynamic: bool,
    /// Values whose attributes are set or deleted.
    patched: Vec<Key<'a>>,
    /// Values whose attributes are set or deleted other than by a
    /// statement of the analyzed body naming them (see [`Sink::touch`]).
    touched: Vec<Key<'a>>,
    /// Names whose attributes a statement of the analyzed body sets
    /// (`K.attr = …`).
    direct: Vec<Key<'a>>,
    /// Classes handed to code aye can't follow by their own statement: a
    /// decorator, a keyword, a base from elsewhere.
    defined: Vec<Key<'a>>,
    /// Names a function declares `global` and binds or deletes, with the
    /// value assigned (`None` for a `def`, `class`, `del`, import or any
    /// other binding).
    global_writes: Vec<(&'a str, Option<&'a Expr>)>,
    /// Modules changed through a literal name (`sys.modules["m"].x = …`,
    /// `__builtins__`).
    patched_paths: Vec<String>,
    /// Names bound by imports, and the dotted path each names.
    imports: Vec<(Key<'a>, String)>,
    /// Modules imported or looked up by a literal name.
    modules: Vec<String>,
    /// Module namespaces are reached in ways aye can't follow.
    reflective: bool,
}

/// When a body of code runs during the import.
#[derive(Clone, Copy)]
enum Trigger<'a> {
    /// The analyzed body itself.
    Root,
    /// Whenever the code defining it runs: lambdas, functions whose
    /// decorators may call them, dunder methods Python or pytest calls on
    /// their own, hooks.
    Always,
    /// A function: when code that runs uses its name.
    Named(&'a str),
    /// A method: when its class is used, or code that runs uses its name.
    Method(usize, &'a str),
}

struct Unit<'a> {
    parent: usize,
    /// Where the function starts (`None` for the analyzed body).
    id: Option<TextSize>,
    /// A function whose body is walked once it may run.
    def: Option<&'a StmtFunctionDef>,
    /// Walked: it may run during the import.
    runs: bool,
    /// The function's locals: its parameters and the names its body binds
    /// (not declared `global` or `nonlocal`).
    locals: FastSet<&'a str>,
    trigger: Trigger<'a>,
    facts: Facts<'a>,
    /// For a dunder method Python calls with operands (`__eq__`,
    /// `__add__`…): its parameters after the first.
    operand_params: Vec<&'a str>,
}

struct ClassUse<'a> {
    name: &'a str,
    /// The name the class statement binds.
    key: Key<'a>,
    /// Where its body starts: how its namespace is found again.
    origin: TextSize,
    /// The unit whose code runs the class statement.
    unit: usize,
    /// Used whatever its name's uses: decorated, with keywords, or with
    /// bases that may call into it.
    intrinsic: bool,
}

/// Names that may hold the same value, as a union-find.
#[derive(Default)]
struct Groups<'a> {
    index: FastMap<Key<'a>, usize>,
    keys: Vec<Key<'a>>,
    parent: Vec<usize>,
}

impl<'a> Groups<'a> {
    fn root(&mut self, key: Key<'a>) -> usize {
        let mut at = if let Some(at) = self.index.get(&key) {
            *at
        } else {
            let at = self.keys.len();
            self.index.insert(key, at);
            self.keys.push(key);
            self.parent.push(at);
            at
        };
        while self.parent[at] != at {
            self.parent[at] = self.parent[self.parent[at]];
            at = self.parent[at];
        }
        at
    }

    fn union(&mut self, a: Key<'a>, b: Key<'a>) {
        let (a, b) = (self.root(a), self.root(b));
        if a != b {
            self.parent[a] = b;
        }
    }
}

/// The escape analysis's result for one body.
pub(crate) struct Found<'a> {
    pub(crate) escaped: Escaped<'a>,
    /// Names of the body changed in ways its statements don't record as
    /// ops.
    pub(crate) untracked: FastSet<&'a str>,
    pub(crate) dynamic: bool,
    /// See [`Namespace::reflective`].
    pub(crate) reflective: bool,
}

/// How a call treats its arguments.
enum Callee {
    /// Keeps them, never changes them (`pytest.mark.*`, `pytest.param`).
    Inert,
    /// Reads them.
    Operand,
    /// Its result holds them.
    Flow,
    /// Anything may happen to them.
    Unknown,
}

/// The escape analysis: a walk over every expression of a body, class
/// bodies included, recording what each body of code does with names, and
/// over the bodies of the functions it defines once they may run during the
/// import; then which values code aye can't follow may change.
///
/// It is conservative: names that may hold the same value (an alias, an
/// element of a literal, a loop variable, a parameter and its default)
/// share their fate.
pub(crate) struct Escapes<'s, 'a> {
    /// The analyzed body's namespace, for resolving names.
    module: &'s Namespace<'a>,
    units: Vec<Unit<'a>>,
    classes: Vec<ClassUse<'a>>,
    /// The unit being walked.
    unit: usize,
    /// The class whose body is being walked (not one of its methods).
    class: Option<usize>,
    /// Where the statement being walked starts.
    at: TextSize,
}

impl<'s, 'a> Escapes<'s, 'a> {
    pub(crate) fn analyze(ns: &'s Namespace<'a>, body: &'a [Stmt]) -> Found<'a> {
        let mut this = Self {
            module: ns,
            units: vec![Unit {
                parent: 0,
                id: None,
                def: None,
                runs: true,
                locals: FastSet::default(),
                trigger: Trigger::Root,
                facts: Facts::default(),
                operand_params: Vec::new(),
            }],
            classes: Vec::new(),
            unit: 0,
            class: None,
            at: TextSize::default(),
        };
        let scope = Lookup {
            module: ns,
            class: None,
            source: "",
            at: TextSize::new(u32::MAX),
        };
        this.body(body, scope);
        this.run_functions();
        this.finish()
    }

    /// Walk the bodies of the functions that may run, until no more may.
    fn run_functions(&mut self) {
        let mut loaded: FastSet<&'a str> = self.units[0].facts.loads.iter().copied().collect();
        // Passes in definition order: a pass that walks nothing ends it.
        let mut more = true;
        while more {
            more = false;
            let mut index = 1;
            while index < self.units.len() {
                let unit = &self.units[index];
                let fires = !unit.runs
                    && self.units[unit.parent].runs
                    && match unit.trigger {
                        Trigger::Root | Trigger::Always => true,
                        Trigger::Named(name) => loaded.contains(name),
                        Trigger::Method(class, name) => {
                            let class = &self.classes[class];
                            loaded.contains(name)
                                || (self.units[class.unit].runs
                                    && (class.intrinsic || loaded.contains(class.name)))
                        }
                    };
                if fires {
                    self.run_function(index);
                    loaded.extend(self.units[index].facts.loads.iter().copied());
                    more = true;
                }
                index += 1;
            }
        }
    }

    fn run_function(&mut self, index: usize) {
        let def = self.units[index].def.expect("only functions wait");
        self.units[index].locals = function_locals(Some(&def.parameters), &def.body);
        self.units[index].runs = true;
        let (unit, class) = (self.unit, self.class.take());
        self.unit = index;
        let scope = Lookup {
            module: self.module,
            class: None,
            source: "",
            at: TextSize::new(u32::MAX),
        };
        self.body(&def.body, scope);
        self.unit = unit;
        self.class = class;
    }

    /// Statements of the analyzed body itself, outside class bodies: the
    /// ones [`Namespace::statement`] walks.
    fn top(&self) -> bool {
        self.unit == 0 && self.class.is_none()
    }

    fn facts(&mut self) -> &mut Facts<'a> {
        &mut self.units[self.unit].facts
    }

    /// What a name refers to outside class bodies: the local of the
    /// innermost function binding it, else the analyzed body's name.
    fn outer(&self, name: &'a str) -> Key<'a> {
        let mut unit = self.unit;
        loop {
            let found = &self.units[unit];
            if let Some(id) = found.id
                && found.locals.contains(name)
            {
                return Key::Local(id, name);
            }
            if unit == 0 {
                return Key::Global(name);
            }
            unit = found.parent;
        }
    }

    /// What a name read here refers to: in a class body, the class's own
    /// binding when it surely exists, the enclosing scope's when it surely
    /// doesn't, else both.
    fn keys(&self, name: &'a str, scope: Lookup<'_, 'a>) -> [Option<Key<'a>>; 2] {
        let (Some(class), Some(ns)) = (self.class, scope.class) else {
            return [Some(self.outer(name)), None];
        };
        let before: Vec<&Bound<'a>> = ns
            .bindings(name)
            .iter()
            .filter(|b| b.range.end() <= self.at)
            .collect();
        match before.first() {
            None => [Some(self.outer(name)), None],
            Some(first)
                if !before.iter().any(|b| b.conditional)
                    && ns.deleted(name, first.range.start(), self.at).is_none() =>
            {
                [Some(Key::Class(class, name)), None]
            }
            Some(_) => [Some(Key::Class(class, name)), Some(self.outer(name))],
        }
    }

    /// What a name bound here is.
    fn bound(&self, name: &'a str) -> Key<'a> {
        match self.class {
            Some(class) => Key::Class(class, name),
            None => self.outer(name),
        }
    }

    /// A name a statement binds or deletes, `value` when it assigns one:
    /// recorded when a function binds it after declaring it `global`.
    fn global_write(&mut self, name: &'a str, value: Option<&'a Expr>) {
        if self.unit != 0
            && self.class.is_none()
            && self.units[self.unit].def.is_some()
            && matches!(self.outer(name), Key::Global(_))
        {
            self.facts().global_writes.push((name, value));
        }
    }

    /// [`Escapes::global_write`] for the names an assignment target binds.
    fn global_targets(&mut self, target: &'a Expr, value: Option<&'a Expr>) {
        match target {
            Expr::Name(name) => self.global_write(name.id.as_str(), value),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| self.global_targets(e, None)),
            Expr::List(list) => list.elts.iter().for_each(|e| self.global_targets(e, None)),
            Expr::Starred(starred) => self.global_targets(&starred.value, None),
            _ => {}
        }
    }

    /// A name read, its value going to `sink`.
    fn load(&mut self, name: &'a str, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        self.facts().loads.push(name);
        for key in self.keys(name, scope).into_iter().flatten() {
            self.flow(key, sink);
        }
    }

    /// The value `key` holds goes to `sink`.
    fn flow(&mut self, key: Key<'a>, sink: &Sink<'a>) {
        let facts = self.facts();
        if sink.escape {
            facts.escapes.push(key);
        }
        if sink.patch {
            facts.patched.push(key);
        }
        if sink.touch {
            facts.touched.push(key);
        }
        if sink.operand {
            facts.operands.push(key);
        }
        for bound in &sink.binds {
            facts.unions.push((key, *bound));
        }
    }

    /// The value of `name` changed in place, by a statement of the analyzed
    /// body when `tracked`.
    fn changed(&mut self, name: &'a str, tracked: bool, scope: Lookup<'_, 'a>) {
        if name == "__builtins__" {
            // The builtins' namespace (a module's `__builtins__` is its
            // dict).
            self.facts().patched_paths.push("builtins".to_string());
        }
        let tracked = tracked && self.top();
        let keys = self.keys(name, scope).into_iter().flatten();
        let facts = self.facts();
        facts.loads.push(name);
        if tracked {
            facts.tracked.extend(keys);
        } else {
            facts.escapes.extend(keys);
        }
    }

    /// Where the value assigned to `targets` goes: the names they bind, and
    /// code aye can't follow when one is an attribute or a subscript.
    fn binding_sink(&self, targets: &'a [Expr]) -> Sink<'a> {
        fn collect<'a>(this: &Escapes<'_, 'a>, target: &'a Expr, sink: &mut Sink<'a>) {
            match target {
                Expr::Name(name) => sink.binds.push(this.bound(name.id.as_str())),
                Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| collect(this, e, sink)),
                Expr::List(list) => list.elts.iter().for_each(|e| collect(this, e, sink)),
                Expr::Starred(starred) => collect(this, &starred.value, sink),
                _ => sink.escape = true,
            }
        }
        let mut sink = Sink::default();
        for target in targets {
            collect(self, target, &mut sink);
        }
        sink
    }

    /// Walk the body of the lambda starting at `id`, which runs whenever the
    /// code defining it does.
    fn nested(&mut self, id: TextSize, locals: FastSet<&'a str>, walk: impl FnOnce(&mut Self)) {
        let index = self.units.len();
        self.units.push(Unit {
            parent: self.unit,
            id: Some(id),
            def: None,
            runs: true,
            locals,
            trigger: Trigger::Always,
            facts: Facts::default(),
            operand_params: Vec::new(),
        });
        let (unit, class) = (self.unit, self.class.take());
        self.unit = index;
        walk(self);
        self.unit = unit;
        self.class = class;
    }

    fn body(&mut self, body: &'a [Stmt], scope: Lookup<'_, 'a>) {
        for stmt in body {
            self.at = stmt.start();
            self.stmt(stmt, scope);
        }
    }

    fn stmt(&mut self, stmt: &'a Stmt, scope: Lookup<'_, 'a>) {
        match stmt {
            Stmt::FunctionDef(def) => self.function(def, scope),
            Stmt::ClassDef(class) => self.class(class, scope),
            Stmt::Return(s) => {
                if let Some(value) = &s.value {
                    self.expr(value, &Sink::escape(), scope);
                }
            }
            Stmt::Delete(s) => {
                for target in &s.targets {
                    self.global_targets(target, None);
                    self.store(target, scope, true);
                }
            }
            Stmt::Assign(s) => self.assign(&s.targets, &s.value, scope),
            Stmt::AnnAssign(s) => {
                self.expr(&s.annotation, &Sink::operand(), scope);
                if let Some(value) = &s.value {
                    self.assign(std::slice::from_ref(&*s.target), value, scope);
                }
            }
            Stmt::AugAssign(s) => {
                self.expr(&s.value, &Sink::escape(), scope);
                self.global_targets(&s.target, Some(&s.value));
                match &*s.target {
                    Expr::Name(name) => self.changed(name.id.as_str(), true, scope),
                    target => self.store(target, scope, true),
                }
            }
            Stmt::For(s) => {
                let sink = self
                    .binding_sink(std::slice::from_ref(&*s.target))
                    .with_operand();
                self.expr(&s.iter, &sink, scope);
                self.global_targets(&s.target, None);
                self.store(&s.target, scope, true);
                self.body(&s.body, scope);
                self.body(&s.orelse, scope);
            }
            Stmt::While(s) => {
                self.expr(&s.test, &Sink::operand(), scope);
                self.body(&s.body, scope);
                self.body(&s.orelse, scope);
            }
            Stmt::If(s) => {
                self.expr(&s.test, &Sink::operand(), scope);
                self.body(&s.body, scope);
                for clause in &s.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.expr(test, &Sink::operand(), scope);
                    }
                    self.body(&clause.body, scope);
                }
            }
            Stmt::With(s) => {
                for item in &s.items {
                    self.expr(&item.context_expr, &Sink::escape(), scope);
                    if let Some(vars) = &item.optional_vars {
                        self.global_targets(vars, None);
                        self.store(vars, scope, true);
                    }
                }
                self.body(&s.body, scope);
            }
            Stmt::Match(s) => {
                let mut captures = Vec::new();
                for case in &s.cases {
                    pattern_captures(&case.pattern, &mut captures);
                }
                let sink = Sink {
                    operand: true,
                    binds: captures
                        .iter()
                        .map(|name| self.bound(name.id.as_str()))
                        .collect(),
                    ..Sink::default()
                };
                self.expr(&s.subject, &sink, scope);
                for case in &s.cases {
                    self.pattern(&case.pattern, scope);
                    if let Some(guard) = &case.guard {
                        self.expr(guard, &Sink::operand(), scope);
                    }
                    self.body(&case.body, scope);
                }
            }
            Stmt::Raise(s) => {
                for expr in s.exc.iter().chain(&s.cause) {
                    self.expr(expr, &Sink::escape(), scope);
                }
            }
            Stmt::Try(s) => {
                self.body(&s.body, scope);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        if let Some(kind) = &handler.type_ {
                            self.expr(kind, &Sink::operand(), scope);
                        }
                        self.body(&handler.body, scope);
                    }
                }
                self.body(&s.orelse, scope);
                self.body(&s.finalbody, scope);
            }
            Stmt::Assert(s) => {
                for expr in std::iter::once(&*s.test).chain(s.msg.as_deref()) {
                    self.expr(expr, &Sink::operand(), scope);
                }
            }
            Stmt::Expr(s) => self.expression_statement(&s.value, scope),
            Stmt::Import(s) => {
                for alias in &s.names {
                    let dotted = alias.name.id.as_str();
                    // `import a.b` binds `a`.
                    let head = dotted.split('.').next().unwrap_or(dotted);
                    let (local, path) = alias
                        .asname
                        .as_ref()
                        .map_or((head, head), |asname| (asname.id.as_str(), dotted));
                    let key = self.bound(local);
                    self.global_write(local, None);
                    let facts = self.facts();
                    facts.modules.push(dotted.to_string());
                    facts.imports.push((key, path.to_string()));
                }
            }
            Stmt::ImportFrom(s) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(s.level as usize),
                    s.module.as_ref().map_or("", |m| m.id.as_str())
                );
                self.facts().modules.push(module.clone());
                for alias in &s.names {
                    let name = alias.name.id.as_str();
                    if name == "*" {
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    let path = if module.ends_with('.') {
                        format!("{module}{name}")
                    } else {
                        format!("{module}.{name}")
                    };
                    let key = self.bound(local);
                    self.global_write(local, None);
                    let facts = self.facts();
                    facts.modules.push(path.clone());
                    facts.imports.push((key, path));
                }
            }
            // A type alias's value is evaluated lazily.
            Stmt::TypeAlias(_)
            | Stmt::Global(_)
            | Stmt::Nonlocal(_)
            | Stmt::Pass(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::IpyEscapeCommand(_) => {}
        }
    }

    /// `NAME.mutator(…)` as a statement of the analyzed body is one of
    /// [`Namespace::statement`]'s ops.
    fn expression_statement(&mut self, expr: &'a Expr, scope: Lookup<'_, 'a>) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(attr) = &*call.func
            && let Expr::Name(owner) = &*attr.value
            && MUTATORS.contains(&attr.attr.id.as_str())
            && self.top()
        {
            self.changed(owner.id.as_str(), true, scope);
            self.arguments(&call.arguments, &Sink::escape(), scope);
            return;
        }
        self.expr(expr, &Sink::inert(), scope);
    }

    fn assign(&mut self, targets: &'a [Expr], value: &'a Expr, scope: Lookup<'_, 'a>) {
        let sink = self.binding_sink(targets);
        self.expr(value, &sink, scope);
        for target in targets {
            self.global_targets(target, Some(value));
            match target {
                // `K.attr = …` in the analyzed body: the namespace records
                // it, so what it does to a class or function is read.
                Expr::Attribute(attr) if self.top() && matches!(&*attr.value, Expr::Name(_)) => {
                    self.set_attribute(attr, scope, true);
                }
                _ => self.store(target, scope, true),
            }
        }
    }

    /// An assignment or deletion target; `tracked` when
    /// [`Namespace::statement`] records it (not in a comprehension).
    fn store(&mut self, target: &'a Expr, scope: Lookup<'_, 'a>, tracked: bool) {
        match target {
            Expr::Name(_) => {}
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.store(elt, scope, tracked);
                }
            }
            Expr::List(list) => {
                for elt in &list.elts {
                    self.store(elt, scope, tracked);
                }
            }
            Expr::Starred(starred) => self.store(&starred.value, scope, tracked),
            Expr::Attribute(attr) => self.set_attribute(attr, scope, false),
            Expr::Subscript(sub) if self.module_lookup(sub, scope) => {}
            Expr::Subscript(sub) => {
                match &*sub.value {
                    Expr::Name(owner) => self.changed(owner.id.as_str(), tracked, scope),
                    owner => self.expr(owner, &Sink::escape(), scope),
                }
                self.expr(&sub.slice, &Sink::operand(), scope);
            }
            other => self.expr(other, &Sink::escape(), scope),
        }
    }

    /// `owner.attr = …`, `owner.attr += …` or `del owner.attr`; `direct`
    /// for `NAME.attr = …` as a statement of the analyzed body.
    fn set_attribute(&mut self, attr: &'a ExprAttribute, scope: Lookup<'_, 'a>, direct: bool) {
        let name = attr.attr.id.as_str();
        self.facts().attributes.push(name);
        if name == "__name__" {
            match &*attr.value {
                Expr::Name(owner) => {
                    let keys = self.keys(owner.id.as_str(), scope);
                    self.facts().renamed.extend(keys.into_iter().flatten());
                }
                _ => self.facts().renamed_unknown = true,
            }
        }
        // `a.b.x = …` changes what `a.b` is: the module `a.b` names, or
        // (the way modules usually hold one) the module `b`.
        if matches!(&*attr.value, Expr::Attribute(_))
            && let Some(owner) = scope.path(&attr.value)
        {
            let last = owner.rsplit('.').next().unwrap_or(&owner).to_string();
            let facts = self.facts();
            facts.patched_paths.push(owner);
            facts.patched_paths.push(last);
        }
        // `m.x = …` on a name only ever bound to imports changes `m.x`.
        if let Expr::Name(owner) = &*attr.value
            && let [Some(Key::Global(global)), None] = self.keys(owner.id.as_str(), scope)
        {
            let bindings = self.module.bindings(global);
            let paths: Vec<String> = bindings
                .iter()
                .filter_map(|b| match &b.binding {
                    Binding::Import(path) => Some(format!("{path}.{name}")),
                    _ => None,
                })
                .collect();
            if !paths.is_empty() && paths.len() == bindings.len() {
                self.facts().patched_paths.extend(paths);
                self.load(owner.id.as_str(), &Sink::operand(), scope);
                return;
            }
        }
        if direct && let Expr::Name(owner) = &*attr.value {
            let key = self.outer(owner.id.as_str());
            self.facts().direct.push(key);
            let sink = Sink {
                touch: false,
                ..Sink::patched()
            };
            self.load(owner.id.as_str(), &sink, scope);
            return;
        }
        self.expr(&attr.value, &Sink::patched(), scope);
    }

    /// The defaults and annotations of the function starting at `id`.
    fn parameters(&mut self, parameters: &'a Parameters, id: TextSize, scope: Lookup<'_, 'a>) {
        for param in parameters.iter_non_variadic_params() {
            if let Some(default) = &param.default {
                let sink = Sink {
                    binds: vec![Key::Local(id, param.parameter.name.id.as_str())],
                    ..Sink::default()
                };
                self.expr(default, &sink, scope);
            }
        }
        for param in parameters {
            if let Some(annotation) = &param.as_parameter().annotation {
                self.expr(annotation, &Sink::operand(), scope);
            }
        }
    }

    fn function(&mut self, def: &'a StmtFunctionDef, scope: Lookup<'_, 'a>) {
        let mut harmless = true;
        for decorator in &def.decorator_list {
            self.expr(&decorator.expression, &Sink::inert(), scope);
            harmless &= harmless_decorator(decorator_path(&decorator.expression, scope).as_deref());
        }
        let id = def.start();
        self.parameters(&def.parameters, id, scope);
        if let Some(returns) = &def.returns {
            self.expr(returns, &Sink::operand(), scope);
        }
        let name = def.name.id.as_str();
        self.global_write(name, None);
        // Python calls dunder methods on its own (and pytest instantiates
        // test classes and reads their attributes while collecting); pytest
        // calls hooks.
        let implicit = is_dunder(name) && !EXPLICIT_DUNDERS.contains(&name);
        let trigger = if !harmless || implicit || (self.top() && name.starts_with("pytest_")) {
            Trigger::Always
        } else if let Some(class) = self.class {
            Trigger::Method(class, name)
        } else {
            Trigger::Named(name)
        };
        let operand_params = if implicit && !NO_OPERAND_DUNDERS.contains(&name) {
            def.parameters
                .iter()
                .skip(1)
                .map(|p| p.as_parameter().name.id.as_str())
                .collect()
        } else {
            Vec::new()
        };
        // Its body is walked if it may run (`run_functions`).
        self.units.push(Unit {
            parent: self.unit,
            id: Some(id),
            def: Some(def),
            runs: false,
            locals: FastSet::default(),
            trigger,
            facts: Facts::default(),
            operand_params,
        });
    }

    fn class(&mut self, class: &'a StmtClassDef, scope: Lookup<'_, 'a>) {
        let key = self.bound(class.name.id.as_str());
        let mut intrinsic = false;
        // The class goes to code aye can't follow, which may change any of
        // its attributes: a decorator, a metaclass, a base from elsewhere
        // (its `__init_subclass__`).
        let mut handed = false;
        for decorator in &class.decorator_list {
            self.expr(&decorator.expression, &Sink::inert(), scope);
            let harmless =
                harmless_decorator(decorator_path(&decorator.expression, scope).as_deref());
            intrinsic |= !harmless;
            handed |= !harmless;
        }
        if let Some(arguments) = &class.arguments {
            for base in &*arguments.args {
                self.expr(base, &Sink::operand(), scope);
                // A base may call into its subclasses (`__init_subclass__`,
                // a metaclass); builtin and unittest bases don't.
                intrinsic |= !scope
                    .path(base)
                    .is_some_and(|p| p.starts_with("builtins.") || p.starts_with("unittest."));
                handed |= !followed_base(base, scope);
            }
            for keyword in &arguments.keywords {
                self.expr(&keyword.value, &Sink::escape(), scope);
                intrinsic = true;
                handed = true;
            }
        }
        if handed {
            self.facts().defined.push(key);
        }
        self.global_write(class.name.id.as_str(), None);
        let index = self.classes.len();
        self.classes.push(ClassUse {
            key,
            name: class.name.id.as_str(),
            origin: class.body.first().map_or(class.start(), Ranged::start),
            unit: self.unit,
            intrinsic,
        });
        let ns = Namespace::without_escapes(&class.body);
        let inner = Lookup {
            class: Some(&ns),
            ..scope
        };
        let (outer, at) = (self.class.replace(index), self.at);
        self.body(&class.body, inner);
        self.class = outer;
        self.at = at;
    }

    fn arguments(&mut self, arguments: &'a ast::Arguments, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        for arg in &*arguments.args {
            self.expr(arg, sink, scope);
        }
        for keyword in &arguments.keywords {
            self.expr(&keyword.value, sink, scope);
        }
    }

    fn expr(&mut self, expr: &'a Expr, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        match expr {
            Expr::Name(name) => {
                if matches!(name.ctx, ExprContext::Load) {
                    let id = name.id.as_str();
                    if id == "__builtins__" {
                        self.facts().patched_paths.push("builtins".to_string());
                    } else if may_be_reflective(id)
                        && scope.path(expr).is_some_and(|p| is_reflective(&p))
                    {
                        self.facts().reflective = true;
                    }
                    self.load(id, sink, scope);
                }
            }
            Expr::Attribute(attr) => self.attribute(attr, sink, scope),
            Expr::Subscript(sub) if self.module_lookup(sub, scope) => {}
            Expr::Subscript(sub) => {
                self.expr(&sub.value, &sink.part().with_operand(), scope);
                self.expr(&sub.slice, &Sink::operand(), scope);
            }
            Expr::Starred(starred) => self.expr(&starred.value, sink, scope),
            Expr::List(list) => {
                for elt in &list.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Set(set) => {
                for elt in &set.elts {
                    self.expr(elt, sink, scope);
                }
            }
            Expr::Dict(dict) => {
                for item in &dict.items {
                    if let Some(key) = &item.key {
                        self.expr(key, sink, scope);
                    }
                    self.expr(&item.value, sink, scope);
                }
            }
            Expr::BoolOp(op) => {
                let sink = sink.clone().with_operand();
                for value in &op.values {
                    self.expr(value, &sink, scope);
                }
            }
            Expr::Named(named) => {
                let mut sink = sink.clone();
                if let Expr::Name(target) = &*named.target {
                    sink.binds.push(self.bound(target.id.as_str()));
                }
                self.expr(&named.value, &sink, scope);
            }
            Expr::BinOp(op) => {
                let sink = sink.part().with_operand();
                self.expr(&op.left, &sink, scope);
                self.expr(&op.right, &sink, scope);
            }
            Expr::UnaryOp(op) => self.expr(&op.operand, &Sink::operand(), scope),
            Expr::Lambda(lambda) => {
                let id = lambda.start();
                if let Some(parameters) = &lambda.parameters {
                    self.parameters(parameters, id, scope);
                }
                let inner = Lookup {
                    class: None,
                    ..scope
                };
                let locals = function_locals(lambda.parameters.as_deref(), &[]);
                self.nested(id, locals, |this| {
                    this.expr(&lambda.body, &Sink::escape(), inner);
                });
            }
            Expr::If(op) => {
                self.expr(&op.test, &Sink::operand(), scope);
                self.expr(&op.body, sink, scope);
                self.expr(&op.orelse, sink, scope);
            }
            Expr::ListComp(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::SetComp(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::Generator(comp) => {
                self.generators(&comp.generators, scope);
                self.expr(&comp.elt, sink, scope);
            }
            Expr::DictComp(comp) => {
                self.generators(&comp.generators, scope);
                if let Some(key) = &comp.key {
                    self.expr(key, sink, scope);
                }
                self.expr(&comp.value, sink, scope);
            }
            Expr::Await(op) => self.expr(&op.value, &Sink::escape(), scope),
            Expr::Yield(op) => {
                if let Some(value) = &op.value {
                    self.expr(value, &Sink::escape(), scope);
                }
            }
            Expr::YieldFrom(op) => self.expr(&op.value, &Sink::escape(), scope),
            Expr::Compare(op) => {
                self.expr(&op.left, &Sink::operand(), scope);
                for (cmp, comparator) in op.ops.iter().zip(&*op.comparators) {
                    // `"m" in sys.modules` only asks.
                    if matches!(cmp, ast::CmpOp::In | ast::CmpOp::NotIn)
                        && is_sys_modules(comparator, scope)
                    {
                        continue;
                    }
                    self.expr(comparator, &Sink::operand(), scope);
                }
            }
            Expr::Call(call) => self.call(call, sink, scope),
            Expr::FString(_) => {
                for part in children(expr) {
                    self.expr(part, &Sink::operand(), scope);
                }
            }
            // A template keeps the values it interpolates.
            Expr::TString(_) => {
                let sink = sink.part().with_operand();
                for part in children(expr) {
                    self.expr(part, &sink, scope);
                }
            }
            Expr::Slice(slice) => {
                for part in [&slice.lower, &slice.upper, &slice.step]
                    .into_iter()
                    .flatten()
                {
                    self.expr(part, &Sink::operand(), scope);
                }
            }
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::IpyEscapeCommand(_) => {}
        }
    }

    fn generators(&mut self, generators: &'a [ast::Comprehension], scope: Lookup<'_, 'a>) {
        for generator in generators {
            let sink = self
                .binding_sink(std::slice::from_ref(&generator.target))
                .with_operand();
            self.expr(&generator.iter, &sink, scope);
            self.store(&generator.target, scope, false);
            for condition in &generator.ifs {
                self.expr(condition, &Sink::operand(), scope);
            }
        }
    }

    /// `owner.attr`, read.
    fn attribute(&mut self, attr: &'a ExprAttribute, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        let name = attr.attr.id.as_str();
        if is_frame_attribute(name) {
            // A frame's or a function's globals: a module's namespace.
            self.facts().reflective = true;
        } else if name == "__builtins__" {
            self.facts().patched_paths.push("builtins".to_string());
        } else if may_be_reflective(name)
            && scope
                .path(&attr.value)
                .is_some_and(|owner| is_reflective(&format!("{owner}.{name}")))
        {
            self.facts().reflective = true;
        }
        // A class attribute read through its class (`TestK.CASES`).
        self.flow(Key::Attr(name), sink);
        // A method kept for later (`add = CASES.append`) may change its
        // owner when called; the names every class and function has are
        // strings.
        let string = matches!(name, "__name__" | "__qualname__" | "__module__" | "__doc__");
        let owner = if sink.held() && (MUTATORS.contains(&name) || (is_dunder(name) && !string)) {
            Sink::escape()
        } else {
            Sink::operand()
        };
        self.expr(&attr.value, &owner, scope);
    }

    /// `sys.modules[…]`, read or written: the module a literal names may
    /// change (and may be this one); a computed name may be any.
    fn module_lookup(&mut self, sub: &'a ast::ExprSubscript, scope: Lookup<'_, 'a>) -> bool {
        if !is_sys_modules(&sub.value, scope) {
            return false;
        }
        match &*sub.slice {
            Expr::StringLiteral(name) => self.module_named(name.value.to_str()),
            slice => {
                self.expr(slice, &Sink::operand(), scope);
                self.facts().reflective = true;
            }
        }
        true
    }

    /// A module looked up by a literal name: it may change, and it may be
    /// the analyzed module itself.
    fn module_named(&mut self, name: &str) {
        let facts = self.facts();
        facts.modules.push(name.to_string());
        facts.patched_paths.push(name.to_string());
        // `__import__("a.b")` returns `a`.
        if let Some((head, _)) = name.split_once('.') {
            facts.patched_paths.push(head.to_string());
        }
    }

    /// Calls that look modules up by name: `sys.modules.get("m")`,
    /// `importlib.import_module("m")`, `__import__("m")`, and
    /// `patch.dict(sys.modules, …)`, which only adds and removes entries.
    /// `true` when handled.
    fn module_call(&mut self, call: &'a ExprCall, scope: Lookup<'_, 'a>) -> bool {
        let (method, owner) = match &*call.func {
            Expr::Attribute(attr) => (attr.attr.id.as_str(), Some(&*attr.value)),
            Expr::Name(name) => (name.id.as_str(), None),
            _ => return false,
        };
        let args = &call.arguments;
        let literal = match args.args.first() {
            Some(Expr::StringLiteral(name)) => Some(name.value.to_str()),
            _ => None,
        };
        let rest = |this: &mut Self| {
            for arg in args.args.iter().skip(1) {
                this.expr(arg, &Sink::escape(), scope);
            }
            for keyword in &args.keywords {
                this.expr(&keyword.value, &Sink::escape(), scope);
            }
        };
        match method {
            "get" | "pop" | "setdefault" if owner.is_some_and(|o| is_sys_modules(o, scope)) => {
                match literal {
                    Some(name) => self.module_named(name),
                    None => self.facts().reflective = true,
                }
                rest(self);
                true
            }
            "import_module" | "__import__" => {
                let path = scope.path(&call.func);
                if !path.as_deref().is_some_and(|p| {
                    matches!(
                        p,
                        "importlib.import_module"
                            | "importlib.__import__"
                            | "builtins.__import__"
                            | "__import__"
                    )
                }) {
                    return false;
                }
                let Some(name) = literal else {
                    self.facts().reflective = true;
                    return false;
                };
                self.module_named(name);
                rest(self);
                true
            }
            "dict"
                if args.args.first().is_some_and(|a| is_sys_modules(a, scope))
                    && scope
                        .path(&call.func)
                        .is_some_and(|p| p.ends_with("patch.dict")) =>
            {
                rest(self);
                true
            }
            _ => false,
        }
    }

    fn call(&mut self, call: &'a ExprCall, sink: &Sink<'a>, scope: Lookup<'_, 'a>) {
        if self.module_call(call, scope) {
            return;
        }
        let path = known_path(&call.func, scope);
        let arguments = &call.arguments;
        match path.as_deref() {
            // The namespace itself: changing it binds names.
            Some("builtins.globals" | "builtins.locals" | "builtins.vars")
                if arguments.is_empty() =>
            {
                if sink.held() {
                    self.facts().dynamic = true;
                }
            }
            Some("builtins.exec" | "builtins.eval") => self.facts().dynamic = true,
            Some("builtins.setattr" | "builtins.delattr") => self.setattr(arguments, scope),
            _ => {}
        }
        let callee = callee(call, path.as_deref());
        match &*call.func {
            Expr::Name(name) => self.load(name.id.as_str(), &Sink::inert(), scope),
            // A method of a value.
            Expr::Attribute(attr) if path.is_none() => {
                let owner = if PURE_METHODS.contains(&attr.attr.id.as_str()) {
                    sink.part().with_operand()
                } else {
                    Sink::escape()
                };
                self.expr(&attr.value, &owner, scope);
            }
            func @ Expr::Attribute(_) => self.expr(func, &Sink::inert(), scope),
            // A computed callee (`getattr(x, "append")(…)`) may be a method
            // of anything it was computed from.
            func => self.expr(func, &Sink::escape(), scope),
        }
        let sink = match callee {
            Callee::Inert => Sink::inert(),
            Callee::Operand => Sink::operand(),
            Callee::Flow => sink.part().with_operand(),
            Callee::Unknown => Sink::escape(),
        };
        // `super(K, obj)` only reads `K`'s MRO; what it calls gets `obj`.
        if path.as_deref() == Some("builtins.super")
            && let [first, rest @ ..] = &*arguments.args
            && arguments.keywords.is_empty()
        {
            self.expr(first, &Sink::operand(), scope);
            for arg in rest {
                self.expr(arg, &sink, scope);
            }
            return;
        }
        self.arguments(arguments, &sink, scope);
    }

    /// `setattr(owner, "attr", value)` and `delattr(owner, "attr")`.
    fn setattr(&mut self, arguments: &'a ast::Arguments, scope: Lookup<'_, 'a>) {
        let attr = match arguments.args.get(1) {
            Some(Expr::StringLiteral(s)) => Some(s.value.to_str()),
            _ => None,
        };
        if let (Some(Expr::Name(owner)), Some(attr)) = (arguments.args.first(), attr) {
            self.facts().attributes.push(attr);
            if attr == "__name__" {
                let keys = self.keys(owner.id.as_str(), scope);
                self.facts().renamed.extend(keys.into_iter().flatten());
            }
        } else {
            let facts = self.facts();
            facts.dynamic = true;
            facts.renamed_unknown = true;
        }
    }

    fn pattern(&mut self, pattern: &'a Pattern, scope: Lookup<'_, 'a>) {
        match pattern {
            Pattern::MatchValue(p) => self.expr(&p.value, &Sink::operand(), scope),
            Pattern::MatchSingleton(_) | Pattern::MatchStar(_) => {}
            Pattern::MatchSequence(p) => {
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchMapping(p) => {
                for key in &p.keys {
                    self.expr(key, &Sink::operand(), scope);
                }
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchClass(p) => {
                self.expr(&p.cls, &Sink::operand(), scope);
                for inner in &p.arguments.patterns {
                    self.pattern(inner, scope);
                }
                for keyword in &p.arguments.keywords {
                    self.pattern(&keyword.pattern, scope);
                }
            }
            Pattern::MatchAs(p) => {
                if let Some(inner) = &p.pattern {
                    self.pattern(inner, scope);
                }
            }
            Pattern::MatchOr(p) => {
                for inner in &p.patterns {
                    self.pattern(inner, scope);
                }
            }
        }
    }

    /// What the bodies that may run do to names.
    fn finish(self) -> Found<'a> {
        let running: Vec<&Unit<'a>> = self.units.iter().filter(|unit| unit.runs).collect();

        let mut groups = Groups::default();
        for unit in &running {
            for (a, b) in &unit.facts.unions {
                groups.union(*a, *b);
            }
        }
        // Classes bound in class bodies are attributes too (`K.Inner`).
        for class in &self.classes {
            if matches!(class.key, Key::Class(..)) {
                groups.root(class.key);
            }
        }
        // A value bound in a class body is reachable as an attribute of
        // anything (`H.x`, `cls.x`, `self.x`, `P().x`).
        let members: Vec<Key<'a>> = groups
            .keys
            .iter()
            .copied()
            .filter(|key| matches!(key, Key::Class(..)))
            .collect();
        for key in &members {
            if let Key::Class(_, name) = key {
                groups.union(*key, Key::Attr(name));
            }
        }
        // Locals no union ties to another name (most of them) stay out of
        // the groups: what happens to them stays with them.
        let lonely = |groups: &Groups<'a>, key: &Key<'a>| {
            matches!(key, Key::Local(..)) && !groups.index.contains_key(key)
        };
        // Groups changed by code aye can't follow, and the names of the
        // analyzed body its statements change, by group.
        let mut changed: FastSet<usize> = FastSet::default();
        let mut changed_alone: FastSet<Key<'a>> = FastSet::default();
        // The same, without what classes' own statements hand on, and with
        // what has attributes set by code recognition doesn't read.
        let mut strict: FastSet<usize> = FastSet::default();
        let mut strict_alone: FastSet<Key<'a>> = FastSet::default();
        let mut tracked: FastMap<usize, FastSet<Key<'a>>> = FastMap::default();
        for unit in &running {
            for key in &unit.facts.escapes {
                if lonely(&groups, key) {
                    changed_alone.insert(*key);
                    strict_alone.insert(*key);
                } else {
                    changed.insert(groups.root(*key));
                    strict.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.defined {
                if lonely(&groups, key) {
                    changed_alone.insert(*key);
                } else {
                    changed.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.touched {
                if lonely(&groups, key) {
                    strict_alone.insert(*key);
                } else {
                    strict.insert(groups.root(*key));
                }
            }
            for key in &unit.facts.tracked {
                tracked.entry(groups.root(*key)).or_default().insert(*key);
            }
        }
        // `NAME.attr = …` statements, by group: through another name of
        // the group (an alias, a loop variable), the attribute is set on
        // whatever it holds.
        let mut direct: FastMap<usize, FastSet<Key<'a>>> = FastMap::default();
        for unit in &running {
            for key in &unit.facts.direct {
                direct.entry(groups.root(*key)).or_default().insert(*key);
            }
        }
        // A class handed to code aye can't follow (`vars(H)`, `getattr(H,
        // …)`, a decorator) takes what its body binds along.
        let handed = |groups: &mut Groups<'a>, changed: &FastSet<usize>, key: Key<'a>| {
            if lonely(groups, &key) {
                changed_alone.contains(&key)
            } else {
                changed.contains(&groups.root(key))
            }
        };
        let mut more = true;
        while more {
            more = false;
            for key in &members {
                let Key::Class(class, _) = key else {
                    continue;
                };
                let class = &self.classes[*class];
                if self.units[class.unit].runs && handed(&mut groups, &changed, class.key) {
                    more |= changed.insert(groups.root(*key));
                }
            }
        }
        // A dunder method Python calls with operands, which lets one of them
        // escape: every operand may.
        let hostile = running
            .iter()
            .flat_map(|unit| {
                let id = unit.id.unwrap_or_default();
                unit.operand_params.iter().map(move |p| Key::Local(id, p))
            })
            .any(|param| {
                if lonely(&groups, &param) {
                    return changed_alone.contains(&param);
                }
                let root = groups.root(param);
                changed.contains(&root) || tracked.contains_key(&root)
            });
        if hostile {
            for unit in &running {
                for key in &unit.facts.operands {
                    if !lonely(&groups, key) {
                        changed.insert(groups.root(*key));
                        strict.insert(groups.root(*key));
                    }
                }
            }
        }
        let handed_key = |groups: &mut Groups<'a>, key: Key<'a>| {
            if lonely(groups, &key) {
                return strict_alone.contains(&key);
            }
            let root = groups.root(key);
            strict.contains(&root)
                || direct
                    .get(&root)
                    .is_some_and(|keys| keys.iter().any(|k| *k != key))
        };
        let mut escaped = Escaped::default();
        let mut renamed: FastSet<usize> = FastSet::default();
        for unit in &running {
            for key in &unit.facts.renamed {
                match key {
                    Key::Local(_, name) if lonely(&groups, key) => {
                        escaped.renamed.insert(name);
                    }
                    _ => {
                        renamed.insert(groups.root(*key));
                    }
                }
            }
        }

        let origins: Vec<TextSize> = self.classes.iter().map(|c| c.origin).collect();
        let mut untracked = FastSet::default();
        for key in groups.keys.clone() {
            let root = groups.root(key);
            let may_change = changed.contains(&root) || tracked.contains_key(&root);
            let name = match key {
                Key::Global(name) => {
                    let by_others = tracked
                        .get(&root)
                        .is_some_and(|keys| keys.iter().any(|other| *other != key));
                    if changed.contains(&root) || by_others {
                        untracked.insert(name);
                    }
                    if may_change {
                        escaped.changed.insert(name);
                    }
                    name
                }
                Key::Class(class, name) => {
                    if may_change {
                        escaped
                            .classes
                            .entry(origins[class])
                            .or_default()
                            .insert(name);
                    }
                    name
                }
                Key::Attr(name) => {
                    if may_change {
                        escaped.attributes_changed.insert(name);
                    }
                    name
                }
                Key::Local(_, name) => name,
            };
            if renamed.contains(&root) {
                escaped.renamed.insert(name);
            }
        }
        // Every class walked gets an entry, so that its namespace knows the
        // module's analysis covered it.
        for origin in origins {
            escaped.classes.entry(origin).or_default();
        }
        // Modules whose attributes may change: a name bound to one is
        // handed to code aye can't follow, or has attributes stored to.
        let mut patched_roots: FastSet<usize> = FastSet::default();
        let mut patched_alone: FastSet<Key<'a>> = FastSet::default();
        for unit in &running {
            for key in &unit.facts.patched {
                if lonely(&groups, key) {
                    patched_alone.insert(*key);
                } else {
                    patched_roots.insert(groups.root(*key));
                }
            }
        }
        for unit in &running {
            for (key, path) in &unit.facts.imports {
                let hit = if lonely(&groups, key) {
                    changed_alone.contains(key) || patched_alone.contains(key)
                } else {
                    let root = groups.root(*key);
                    changed.contains(&root)
                        || patched_roots.contains(&root)
                        || tracked.contains_key(&root)
                };
                if hit {
                    escaped.patched.insert(path.clone());
                }
            }
            escaped
                .patched
                .extend(unit.facts.patched_paths.iter().cloned());
            escaped.modules.extend(unit.facts.modules.iter().cloned());
        }
        let reflective = running.iter().any(|u| u.facts.reflective);
        escaped.patches_all = reflective;
        // Classes handed to code aye can't follow: any attribute may change.
        for class in &self.classes {
            if self.units[class.unit].runs && handed(&mut groups, &changed, class.key) {
                escaped.classes_all.insert(class.origin);
            }
            if self.units[class.unit].runs && handed_key(&mut groups, class.key) {
                escaped.handed_classes.insert(class.origin);
            }
        }
        for key in groups.keys.clone() {
            match key {
                Key::Global(name) if handed_key(&mut groups, key) => {
                    escaped.handed.insert(name);
                }
                Key::Attr(name) if handed_key(&mut groups, key) => {
                    escaped.handed_attributes.insert(name);
                }
                _ => {}
            }
        }
        escaped.global_writes = running
            .iter()
            .flat_map(|u| u.facts.global_writes.iter().copied())
            .collect();
        escaped.renamed_unknown = running.iter().any(|u| u.facts.renamed_unknown);
        escaped.attributes = running
            .iter()
            .flat_map(|u| u.facts.attributes.iter().copied())
            .collect();
        Found {
            escaped,
            untracked,
            dynamic: running.iter().any(|u| u.facts.dynamic),
            reflective,
        }
    }
}

/// A function's locals: its parameters and the names its body binds, but
/// not those its own `global` and `nonlocal` statements declare.
fn function_locals<'a>(parameters: Option<&'a Parameters>, body: &'a [Stmt]) -> FastSet<&'a str> {
    fn targets<'a>(target: &'a Expr, out: &mut FastSet<&'a str>) {
        match target {
            Expr::Name(name) => {
                out.insert(name.id.as_str());
            }
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| targets(e, out)),
            Expr::List(list) => list.elts.iter().for_each(|e| targets(e, out)),
            Expr::Starred(starred) => targets(&starred.value, out),
            _ => {}
        }
    }
    fn walk<'a>(body: &'a [Stmt], out: &mut FastSet<&'a str>, declared: &mut FastSet<&'a str>) {
        for stmt in body {
            let mut walruses = Vec::new();
            for (expr, _) in evaluated(stmt) {
                walrus_targets(expr, false, &mut walruses);
            }
            for (target, _) in walruses {
                targets(target, out);
            }
            match stmt {
                Stmt::FunctionDef(def) => {
                    out.insert(def.name.id.as_str());
                }
                Stmt::ClassDef(class) => {
                    out.insert(class.name.id.as_str());
                }
                Stmt::Assign(s) => s.targets.iter().for_each(|t| targets(t, out)),
                Stmt::AnnAssign(s) => targets(&s.target, out),
                Stmt::AugAssign(s) => targets(&s.target, out),
                Stmt::Delete(s) => s.targets.iter().for_each(|t| targets(t, out)),
                Stmt::TypeAlias(s) => targets(&s.name, out),
                Stmt::Import(s) => {
                    for alias in &s.names {
                        let local = alias.asname.as_ref().unwrap_or(&alias.name).id.as_str();
                        out.insert(local.split('.').next().unwrap_or(local));
                    }
                }
                Stmt::ImportFrom(s) => {
                    for alias in &s.names {
                        out.insert(alias.asname.as_ref().unwrap_or(&alias.name).id.as_str());
                    }
                }
                Stmt::Global(s) => declared.extend(s.names.iter().map(|n| n.id.as_str())),
                Stmt::Nonlocal(s) => declared.extend(s.names.iter().map(|n| n.id.as_str())),
                Stmt::For(s) => {
                    targets(&s.target, out);
                    walk(&s.body, out, declared);
                    walk(&s.orelse, out, declared);
                }
                Stmt::While(s) => {
                    walk(&s.body, out, declared);
                    walk(&s.orelse, out, declared);
                }
                Stmt::If(s) => {
                    walk(&s.body, out, declared);
                    for clause in &s.elif_else_clauses {
                        walk(&clause.body, out, declared);
                    }
                }
                Stmt::With(s) => {
                    for item in &s.items {
                        if let Some(vars) = &item.optional_vars {
                            targets(vars, out);
                        }
                    }
                    walk(&s.body, out, declared);
                }
                Stmt::Try(s) => {
                    walk(&s.body, out, declared);
                    for handler in &s.handlers {
                        if let Some(handler) = handler.as_except_handler() {
                            if let Some(name) = &handler.name {
                                out.insert(name.id.as_str());
                            }
                            walk(&handler.body, out, declared);
                        }
                    }
                    walk(&s.orelse, out, declared);
                    walk(&s.finalbody, out, declared);
                }
                Stmt::Match(s) => {
                    for case in &s.cases {
                        let mut captures = Vec::new();
                        pattern_captures(&case.pattern, &mut captures);
                        out.extend(captures.iter().map(|n| n.id.as_str()));
                        walk(&case.body, out, declared);
                    }
                }
                _ => {}
            }
        }
    }

    let mut locals = FastSet::default();
    let mut declared = FastSet::default();
    walk(body, &mut locals, &mut declared);
    for param in parameters.into_iter().flatten() {
        locals.insert(param.as_parameter().name.id.as_str());
    }
    locals.retain(|name| !declared.contains(name));
    locals
}

/// How a call treats its arguments.
fn callee(call: &ExprCall, path: Option<&str>) -> Callee {
    if let Expr::Attribute(attr) = &*call.func {
        // `str` and `bytes` methods keep and change none of their arguments.
        if matches!(
            &*attr.value,
            Expr::StringLiteral(_) | Expr::BytesLiteral(_) | Expr::FString(_)
        ) {
            return Callee::Operand;
        }
        if path.is_none() {
            return if PURE_METHODS.contains(&attr.attr.id.as_str()) {
                Callee::Operand
            } else {
                Callee::Unknown
            };
        }
    }
    match path {
        Some(p)
            if p.starts_with("pytest.mark.") || matches!(p, "pytest.param" | "pytest.fixture") =>
        {
            Callee::Inert
        }
        Some(p) if READ_BUILTINS.contains(&p) => Callee::Operand,
        Some(p) if FLOW_BUILTINS.contains(&p) && call.arguments.keywords.is_empty() => Callee::Flow,
        _ => Callee::Unknown,
    }
}
