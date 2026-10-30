//! pytest's `Class` collector, and what the recognizer reads of a class:
//! its bases and MRO, its marks, `__test__`, and whether it may hold other
//! tests than its body says.

use std::collections::{BTreeSet, HashSet};

use aye_adapter::ast::{Expr, Stmt, StmtClassDef};
use aye_adapter::{Ranged, TextSize};
use aye_events::{Certainty, DiagnosticCode, Severity};
use aye_python_semantic::c3::{InconsistentMro, linearize};
use aye_python_semantic::class::{
    External, Klass, Unresolved, external_kind, is_unittest_base, mangle,
};
use aye_python_semantic::eval::deref;
use aye_python_semantic::flow::maybe_deleted;
use aye_python_semantic::namespace::{Binding, Bound, Flag, Namespace, OpKind};
use aye_python_semantic::resolve::{Lookup, Resolved};

use super::ModuleRecognizer;
use super::module::{IGNORED_ATTRIBUTES, is_literal};
use crate::conftest::{ClassMaker, makes_classes};
use crate::marks::{self, Decorator, Mark, MarkList, decorator};

/// A class being collected, as a node in pytest's tree.
#[derive(Clone)]
pub(super) struct Frame<'s, 'a> {
    /// The node name (the name it is bound to).
    pub(super) name: String,
    pub(super) mro: Vec<Klass<'a>>,
    /// The class's marks, from its MRO, base first.
    pub(super) marks: Vec<Mark<'s, 'a>>,
    /// Some of its marks could not be read.
    pub(super) marks_unknown: bool,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    /// The bases of a class, resolved where the class statement runs.
    /// `lenient` leaves out bases from elsewhere that are no unittest case
    /// and set no `__test__` (as a guess of what pytest collects).
    fn bases(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        lenient: bool,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        let lookup = self.lookup(site, class.start());
        let mut out = Vec::new();
        let mut error: Option<Unresolved> = None;
        let mut fail = |reason: String, harmless: bool| match &mut error {
            Some(found) => found.harmless &= harmless,
            None => error = Some(Unresolved { reason, harmless }),
        };
        let Some(arguments) = &class.arguments else {
            return Ok(out);
        };
        'bases: for base in &*arguments.args {
            let target = match base {
                Expr::Subscript(sub) => &*sub.value,
                other => other,
            };
            if base.is_starred_expr() {
                fail(
                    format!("{} has bases computed at runtime", class.name),
                    false,
                );
                continue;
            }
            if let Expr::Name(name) = target {
                // The binding the name has when the class statement runs.
                let ns = site.map_or(self.module, |c| self.class_ns(c));
                let scopes = [Some(ns), (site.is_some()).then_some(self.module)];
                let mut found = None;
                for ns in scopes.into_iter().flatten() {
                    let before: Vec<_> = ns
                        .bindings(name.id.as_str())
                        .iter()
                        .filter(|b| b.range.end() <= class.start())
                        .collect();
                    if let Some(last) = before.last() {
                        found = Some(((*last).clone(), before.iter().any(|b| b.conditional)));
                        break;
                    }
                }
                if let Some((bound, conditional)) = found {
                    match bound.binding {
                        Binding::Class(def) if !conditional => {
                            out.push(Klass::Local(def));
                            continue;
                        }
                        Binding::Class(_) => {
                            fail(
                                format!(
                                    "{} inherits from {}, defined conditionally",
                                    class.name, name.id
                                ),
                                false,
                            );
                            continue 'bases;
                        }
                        Binding::Import(_) => {}
                        _ => {
                            fail(
                                format!(
                                    "{} inherits from {}, bound at runtime",
                                    class.name, name.id
                                ),
                                false,
                            );
                            continue 'bases;
                        }
                    }
                }
            }
            match lookup.path(target) {
                Some(path) if external_kind(&path).is_some() => out.push(Klass::External(path)),
                Some(path) => {
                    let harmless =
                        !self.imports.may_be_unittest(&path) && !self.imports.project_test(&path);
                    if lenient && harmless {
                        continue;
                    }
                    fail(
                        format!(
                            "{} inherits from {path}, defined outside the module",
                            class.name
                        ),
                        harmless,
                    );
                }
                None => fail(
                    format!("{} has bases computed at runtime", class.name),
                    false,
                ),
            }
        }
        match error {
            Some(error) => Err(error),
            None => Ok(out),
        }
    }

    /// The class's C3 linearization, through classes of this module and
    /// known external ones.
    fn mro(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        depth: u8,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        self.mro_of(class, site, depth, false)
    }

    /// [`ModuleRecognizer::mro`], `lenient` as in
    /// [`ModuleRecognizer::bases`].
    fn mro_of(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        depth: u8,
        lenient: bool,
    ) -> Result<Vec<Klass<'a>>, Unresolved> {
        if depth > 32 {
            return Err(Unresolved {
                reason: format!("{} has too deep a hierarchy", class.name),
                harmless: false,
            });
        }
        let bases = self.bases(class, site, lenient)?;
        let mut sequences: Vec<Vec<Klass<'a>>> = Vec::new();
        for base in &bases {
            sequences.push(match base {
                Klass::Local(def) => {
                    let site = self.site_of(def);
                    self.mro_of(def, site, depth + 1, lenient)?
                }
                Klass::External(path) if path == "builtins.object" => vec![base.clone()],
                Klass::External(_) => vec![base.clone(), Klass::object()],
            });
        }
        sequences.push(bases.clone());
        let mut out =
            linearize(Klass::Local(class), sequences).map_err(|InconsistentMro| Unresolved {
                reason: format!("{} has an inconsistent MRO", class.name),
                harmless: false,
            })?;
        if !out.iter().any(|k| k.key() == "builtins.object") {
            out.push(Klass::object());
        }
        Ok(out)
    }

    /// The class statement starting at `start`.
    pub(super) fn class_def(&self, start: TextSize) -> Option<&'a StmtClassDef> {
        self.classes.get(&start).map(|info| info.def)
    }

    /// The class body a class statement sits in, if any.
    fn site_of(&self, class: &'a StmtClassDef) -> Option<&'a StmtClassDef> {
        self.classes.get(&class.start()).and_then(|info| info.site)
    }

    /// The first binding of `name` along the MRO, with the class whose body
    /// holds it.
    pub(super) fn lookup_mro(
        &self,
        mro: &[Klass<'a>],
        name: &str,
    ) -> Option<(Bound<'a>, &'a StmtClassDef)> {
        for klass in mro {
            if let Klass::Local(def) = klass
                && let Some(bound) = self.class_ns(def).get(name)
            {
                return Some((bound.clone(), def));
            }
        }
        None
    }

    fn decorators(
        &self,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
    ) -> Vec<Decorator<'s, 'a>> {
        let lookup = self.lookup(site, class.start());
        class
            .decorator_list
            .iter()
            .rev()
            .map(|d| decorator(lookup, &d.expression))
            .collect()
    }

    /// A namespace's `pytestmark` once its body has run: the value bound
    /// last, then what `.append`, `.extend` and `+=` add to it. `Err` when
    /// pytest would fail on it.
    pub(super) fn pytestmark(
        &self,
        ns: &'s Namespace<'a>,
        class: Option<&'a StmtClassDef>,
    ) -> Result<MarkList<'s, 'a>, String> {
        let ops: Vec<_> = ns.ops.iter().filter(|op| op.name == "pytestmark").collect();
        let Some(bound) = ns.get("pytestmark") else {
            return Ok(MarkList {
                marks: Vec::new(),
                unknown: ns.dynamic || !ops.is_empty(),
            });
        };
        let mut list = match bound.binding {
            Binding::Value(expr) => {
                let lookup = self.lookup(class, bound.range.end());
                let list = marks::pytestmark(lookup, expr)?;
                let (value, _) = deref(lookup, expr);
                // Only a list grows in place.
                let grows = ops.iter().any(|op| op.at > bound.range.start());
                if grows && !matches!(value, Expr::List(_)) {
                    return Ok(MarkList {
                        marks: list.marks,
                        unknown: true,
                    });
                }
                list
            }
            _ => MarkList {
                marks: Vec::new(),
                unknown: true,
            },
        };
        if ns.dynamic || ns.globals.contains("pytestmark") {
            list.unknown = true;
        }
        for op in ops.into_iter().filter(|op| op.at > bound.range.start()) {
            let lookup = self.lookup(class, op.at);
            if op.conditional {
                list.unknown = true;
                continue;
            }
            match op.kind {
                OpKind::Assign(_) => {}
                OpKind::Append(expr) => match marks::mark(lookup, expr) {
                    Ok(Some(mark)) => list.marks.push(mark),
                    _ => list.unknown = true,
                },
                OpKind::Extend(expr) | OpKind::AugAdd(expr) => {
                    let more = marks::mark_collection(lookup, expr);
                    list.marks.extend(more.marks);
                    list.unknown |= more.unknown;
                }
                OpKind::Mutate => list.unknown = true,
            }
        }
        Ok(list)
    }

    /// A class's marks, considering its MRO (`get_unpacked_marks`): for
    /// each class from the most basic, its `pytestmark`, then its mark
    /// decorators, innermost first; whether some could not be read; and
    /// whether a decorator aye can't see through applies to it, which may
    /// add, remove or rename its tests. `Err` when pytest would fail on
    /// them.
    pub(super) fn class_marks(
        &self,
        mro: &[Klass<'a>],
    ) -> Result<(Vec<Mark<'s, 'a>>, bool, bool), String> {
        let mut marks = Vec::new();
        let mut unknown = false;
        let mut opaque = false;
        for klass in mro.iter().rev() {
            let Klass::Local(def) = klass else {
                continue;
            };
            let ns = self.class_ns(def);
            let list = self
                .pytestmark(ns, Some(def))
                .map_err(|reason| format!("{} {reason}", def.name))?;
            marks.extend(list.marks);
            unknown |= list.unknown;
            let site = self.site_of(def);
            for d in self.decorators(def, site) {
                match d {
                    Decorator::Mark(mark) => marks.push(mark),
                    Decorator::Unknown => {
                        unknown = true;
                        opaque = true;
                    }
                    Decorator::Keeps => unknown = true,
                    _ => {}
                }
            }
        }
        Ok((marks, unknown, opaque))
    }

    /// Why the classes of `mro` may hold other tests than their bodies
    /// say: a decorator aye can't see through, code that binds names in a
    /// class body (`exec`, `locals()`), a metaclass, a base's
    /// `__init_subclass__`, or attributes the module sets on them.
    pub(super) fn dynamic_members(&self, mro: &[Klass<'a>], opaque: bool) -> Option<String> {
        if opaque {
            return Some("a class decorator aye can't see through applies".into());
        }
        for (i, klass) in mro.iter().enumerate() {
            let Klass::Local(def) = klass else {
                continue;
            };
            let name = def.name.id.as_str();
            let ns = self.class_ns(def);
            if ns.dynamic {
                return Some(format!("{name}'s body binds names at runtime"));
            }
            let site = self.site_of(def);
            let metaclass = def.arguments.as_ref().and_then(|args| {
                args.keywords
                    .iter()
                    .find(|k| k.arg.as_ref().is_some_and(|a| a.id.as_str() == "metaclass"))
            });
            if let Some(keyword) = metaclass
                && self
                    .lookup(site, def.start())
                    .path(&keyword.value)
                    .as_deref()
                    != Some("abc.ABCMeta")
            {
                return Some(format!("{name} has a metaclass"));
            }
            if i > 0 && ns.get("__init_subclass__").is_some() {
                return Some(format!("{name} defines __init_subclass__"));
            }
            if self.module.class_handed(def) {
                return Some(format!(
                    "{name} is handed to code aye can't follow, which may add or remove its tests"
                ));
            }
            // `__set_name__` gets the class whose body holds the value.
            if self.sets_names
                && ns.iter().any(
                    |(_, bound)| matches!(bound.binding, Binding::Value(expr) if !is_literal(expr)),
                )
            {
                return Some(format!(
                    "{name} holds values whose __set_name__ may change it"
                ));
            }
            let set = self.module.attributes.get(name).into_iter().flatten();
            for attribute in set {
                // `__bases__`, `__init__`, `__getattr__` and the like change
                // what pytest finds; `__test__` is read.
                let dunder = attribute.len() > 4
                    && attribute.starts_with("__")
                    && attribute.ends_with("__")
                    && !matches!(
                        *attribute,
                        "__test__" | "__doc__" | "__name__" | "__qualname__" | "__module__"
                    );
                if *attribute == "pytestmark"
                    || *attribute == "runTest"
                    || dunder
                    || self.is_test_function_name(attribute)
                    || self.is_test_class_name(attribute)
                {
                    return Some(format!("the module sets {name}.{attribute}"));
                }
            }
        }
        None
    }

    /// Whether a call surely makes something pytest doesn't collect under
    /// a test function's name: no function, nor a callable that wraps one
    /// (`functools.partial`, `update_wrapper`). Builtins that return data,
    /// and classes of the module that are not callable and make their own
    /// instances (no `__new__`, `__call__` or metaclass along a known MRO).
    pub(super) fn makes_instance(&self, lookup: Lookup<'s, 'a>, expr: &'a Expr) -> bool {
        const DATA: &[&str] = &[
            "builtins.dict",
            "builtins.list",
            "builtins.set",
            "builtins.frozenset",
            "builtins.tuple",
            "builtins.str",
            "builtins.bytes",
            "builtins.int",
            "builtins.float",
            "builtins.bool",
            "builtins.range",
            "builtins.len",
            "builtins.sorted",
            "builtins.open",
        ];
        let Expr::Call(call) = expr else {
            return false;
        };
        if lookup
            .path(&call.func)
            .is_some_and(|path| DATA.contains(&path.as_str()))
        {
            return true;
        }
        let Resolved::Local(Bound {
            binding: Binding::Class(class),
            conditional: false,
            ..
        }) = lookup.resolve(&call.func)
        else {
            return false;
        };
        let Ok(mro) = self.mro(class, self.site_of(class), 0) else {
            return false;
        };
        mro.iter().all(|klass| match klass {
            Klass::Local(def) => {
                let ns = self.class_ns(def);
                def.arguments
                    .as_ref()
                    .is_none_or(|args| args.keywords.is_empty())
                    && ns.get("__new__").is_none()
                    && ns.get("__call__").is_none()
                    && !ns.dynamic
                    && !self.module.class_handed(def)
            }
            Klass::External(path) => !matches!(
                external_kind(path),
                Some(External::UnitTest | External::Protocol) | None
            ),
        })
    }

    /// The callee, when a call may make a class pytest collects whatever
    /// the name it is bound to: a unittest case or a class whose
    /// `__test__` is true. `type(…)` with three arguments,
    /// `types.new_class`, and functions of the module or of project files
    /// (methods of the module's classes included) that make classes.
    pub(super) fn may_make_class(&self, lookup: Lookup<'s, 'a>, expr: &'a Expr) -> Option<String> {
        let Expr::Call(call) = expr else {
            return None;
        };
        let path = lookup.path(&call.func);
        let arguments = call.arguments.args.len() + call.arguments.keywords.len();
        match path.as_deref() {
            Some("builtins.type") if arguments >= 3 => return Some("type".into()),
            Some("types.new_class") => return Some("types.new_class".into()),
            _ => {}
        }
        if let Some(path) = path {
            return self.imports.project_factory(&path).then_some(path);
        }
        let function = match lookup.resolve(&call.func) {
            Resolved::Local(Bound {
                binding: Binding::Function(def),
                ..
            }) => Some(def),
            Resolved::Local(Bound {
                binding: Binding::Value(Expr::Lambda(lambda)),
                name_range,
                ..
            }) => {
                return ClassMaker::expr(&lambda.body)
                    .then(|| self.file.source[name_range].to_string());
            }
            // `Base.make(…)`: a method of one of the module's classes.
            _ => match &*call.func {
                Expr::Attribute(attr) => match lookup.resolve(&attr.value) {
                    Resolved::Local(Bound {
                        binding: Binding::Class(class),
                        ..
                    }) => self
                        .mro(class, self.site_of(class), 0)
                        .ok()
                        .and_then(|mro| self.lookup_mro(&mro, attr.attr.id.as_str()))
                        .and_then(|(bound, _)| match bound.binding {
                            Binding::Function(def) => Some(def),
                            _ => None,
                        }),
                    _ => None,
                },
                _ => None,
            },
        };
        function
            .filter(|def| makes_classes(&def.body))
            .map(|def| def.name.to_string())
    }

    fn is_unittest(mro: &[Klass<'a>]) -> bool {
        mro.iter()
            .any(|k| matches!(k, Klass::External(path) if is_unittest_base(path)))
    }

    /// `__test__` along the MRO: set in a class body, or on a class of
    /// the module after its body (`K.__test__ = …`), which wins over the
    /// body's.
    pub(super) fn dunder_test(&self, mro: &[Klass<'a>]) -> Flag {
        for klass in mro {
            let Klass::Local(def) = klass else {
                continue;
            };
            if self.site_of(def).is_none() {
                match self.module.test_attr(def.name.id.as_str()) {
                    Flag::Unset => {}
                    flag => return flag,
                }
            }
            if let Some(bound) = self.class_ns(def).get("__test__") {
                let value = match bound.binding {
                    Binding::Value(expr) => self.lookup(Some(def), bound.range.end()).boolean(expr),
                    _ => None,
                };
                return value.map_or(Flag::Computed, Flag::Set);
            }
        }
        Flag::Unset
    }

    pub(super) fn class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        site: Option<&'a StmtClassDef>,
        frames: &[Frame<'s, 'a>],
    ) {
        let mro = match self.mro(class, site, 0) {
            Ok(mro) => mro,
            Err(Unresolved { reason, harmless }) => {
                // Unknown bases: maybe unittest (whatever the class's name,
                // and with tests it only inherits), maybe more tests. Guess
                // from the class's own body.
                if self.is_test_class_name(name) {
                    self.doubt(reason);
                    let guess = vec![Klass::Local(class), Klass::object()];
                    let before = self.cases.len();
                    self.pytest_class(name, class, frames, &guess);
                    for case in &mut self.cases[before..] {
                        case.certainty = Certainty::Partial;
                    }
                } else if class_mentions_tests(class) || !harmless {
                    self.doubt(reason);
                } else if let Ok(guess) = self.mro_of(class, site, 0, true) {
                    // Only classes that hold no unittest case nor
                    // `__test__` are unknown: the bases aye knows decide
                    // whether pytest collects it, not which tests it has.
                    let unittest = Self::is_unittest(&guess) && !self.settings.unittest_blocked;
                    let dunder_test = self.dunder_test(&guess);
                    if unittest || dunder_test != Flag::Unset {
                        self.doubt(reason);
                        let before = self.cases.len();
                        if unittest {
                            self.unittest_class(name, class, frames, &guess);
                        } else if dunder_test == Flag::Set(true) {
                            self.pytest_class(name, class, frames, &guess);
                        }
                        for case in &mut self.cases[before..] {
                            case.certainty = Certainty::Partial;
                        }
                    }
                }
                return;
            }
        };
        if Self::is_unittest(&mro) {
            if self.settings.unittest_blocked {
                // With `-p no:unittest`, pytest's `Class` collector takes
                // the class, and `TestCase.__init__` stops it.
                if self.is_test_class_name(name) || self.dunder_test(&mro) == Flag::Set(true) {
                    let parent =
                        self.nodeid(&frames.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
                    self.diagnostic(
                        Severity::Warning,
                        DiagnosticCode::ClassHasInit,
                        class.name.range(),
                        format!(
                            "cannot collect test class '{}' because it has a __init__ constructor (from: {parent})",
                            class.name
                        ),
                    );
                }
                return;
            }
            self.unittest_class(name, class, frames, &mro);
            return;
        }
        let dunder_test = self.dunder_test(&mro);
        if !self.is_test_class_name(name) && dunder_test != Flag::Set(true) {
            if dunder_test == Flag::Computed {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
            } else if class_mentions_tests(class)
                && mro
                    .iter()
                    .any(|k| matches!(k, Klass::Local(def) if self.module.class_handed(def)))
            {
                // Code aye can't follow may set its `__test__`.
                self.doubt(format!(
                    "{name} is handed to code aye can't follow, which may make it a test class"
                ));
            }
            return;
        }
        self.pytest_class(name, class, frames, &mro);
    }

    fn is_abstract(&self, mro: &[Klass<'a>]) -> bool {
        let abcmeta = mro.iter().any(|k| match k {
            Klass::External(path) => {
                matches!(
                    external_kind(path),
                    Some(External::Abc | External::Protocol)
                )
            }
            Klass::Local(def) => def.arguments.as_ref().is_some_and(|args| {
                args.keywords.iter().any(|k| {
                    k.arg.as_ref().is_some_and(|a| a.id.as_str() == "metaclass")
                        && self
                            .lookup(self.site_of(def), def.start())
                            .path(&k.value)
                            .as_deref()
                            == Some("abc.ABCMeta")
                })
            }),
        });
        if !abcmeta {
            return false;
        }
        let mut names = BTreeSet::new();
        for klass in mro {
            if let Klass::Local(def) = klass {
                for (name, _) in self.class_ns(def).iter() {
                    names.insert(name);
                }
            }
        }
        names.into_iter().any(|name| {
            let Some((bound, def)) = self.lookup_mro(mro, name) else {
                return false;
            };
            let Binding::Function(func) = bound.binding else {
                return false;
            };
            let lookup = self.lookup(Some(def), func.start());
            func.decorator_list.iter().any(|d| {
                matches!(
                    decorator(lookup, &d.expression),
                    Decorator::Abstract | Decorator::AbstractProperty
                )
            })
        })
    }

    /// pytest's `Class` collector.
    fn pytest_class(
        &mut self,
        name: &str,
        class: &'a StmtClassDef,
        frames: &[Frame<'s, 'a>],
        mro: &[Klass<'a>],
    ) {
        if self.is_abstract(mro) {
            return;
        }
        match self.dunder_test(mro) {
            Flag::Set(false) => return,
            Flag::Computed => {
                self.doubt(format!("{name}.__test__ is computed at runtime"));
                return;
            }
            _ => {}
        }
        let parent = self.nodeid(&frames.iter().map(|f| f.name.clone()).collect::<Vec<_>>());
        for dunder in ["__init__", "__new__"] {
            let mut has = self.lookup_mro(mro, dunder).is_some();
            for klass in mro {
                match klass {
                    Klass::External(path) => {
                        let kind = external_kind(path);
                        has |= kind == Some(External::InitNew)
                            || (dunder == "__init__" && kind == Some(External::Protocol))
                            || (dunder == "__new__" && kind == Some(External::New));
                    }
                    Klass::Local(def) if dunder == "__init__" => {
                        let site = self.site_of(def);
                        has |= self
                            .decorators(def, site)
                            .iter()
                            .any(|d| matches!(d, Decorator::Init));
                    }
                    Klass::Local(_) => {}
                }
            }
            if has {
                self.diagnostic(
                    Severity::Warning,
                    DiagnosticCode::ClassHasInit,
                    class.name.range(),
                    format!(
                        "cannot collect test class '{}' because it has a {dunder} constructor (from: {parent})",
                        class.name
                    ),
                );
                return;
            }
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
        let mut frames = frames.to_vec();
        frames.push(Frame {
            name: name.to_string(),
            mro: mro.to_vec(),
            marks,
            marks_unknown: unknown,
        });
        let before = self.cases.len();

        // `PyCollector.collect`: each class's dict along the MRO, a name
        // only once (the most derived wins), base classes' items first.
        // The dict holds private names mangled (`__x` is `_K__x`).
        let mut seen: HashSet<String> = HashSet::new();
        let mut groups: Vec<Vec<(String, Bound<'a>, &'a StmtClassDef)>> = Vec::new();
        for klass in std::iter::once(&Klass::Local(class)).chain(mro.iter()) {
            let Klass::Local(def) = klass else {
                continue;
            };
            let ns = self.class_ns(def);
            let mut group = Vec::new();
            let entries = ns
                .iter()
                .map(|(n, b)| (n, b.clone()))
                .chain(maybe_deleted(ns, &def.body));
            for (member, bound) in entries {
                let key = mangle(member, def.name.id.as_str());
                if IGNORED_ATTRIBUTES.contains(&member) || !seen.insert(key.clone()) {
                    continue;
                }
                group.push((key, bound, *def));
            }
            groups.push(group);
        }
        self.duplicates(self.class_ns(class));
        for group in groups.into_iter().rev() {
            for (member, bound, def) in group {
                self.member(&member, &bound, Some(def), &frames, false);
            }
        }
        if unknown || members_unknown {
            for case in &mut self.cases[before..] {
                case.certainty = Certainty::Partial;
            }
        }
    }
}

/// Whether a class body defines anything that looks like a test, so that
/// not knowing its bases matters (it may be a unittest class).
fn class_mentions_tests(class: &StmtClassDef) -> bool {
    class.body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(def) => {
            def.name.id.as_str().starts_with("test") || def.name.id.as_str() == "runTest"
        }
        _ => false,
    })
}
