//! What a module's names and a class's attributes are bound to, as far as
//! the source tells: a [`Binding`], a [`Member`], a class's [`Base`], what a
//! function does when it runs ([`FunctionInfo`]), and what importing the
//! module may change elsewhere ([`Touch`]).

use aye_adapter::TextRange;

/// What a name is bound to when the module has run, as far as the source
/// tells.
#[derive(Debug, Clone)]
pub(crate) enum Binding {
    /// A class of this module.
    Class(usize),
    Function(FunctionInfo),
    /// A module (`import a.b` binds `a`): never a class.
    Module(String),
    /// A name imported from a module: `m.n`, or `.m.n` relative to the
    /// module's package.
    Import(String),
    /// What a call to an imported callable returned.
    Returned(String),
    /// A value that is no class and not callable (a literal, a container).
    Constant,
    /// A value that is no class.
    Plain,
    /// A value that may be a class.
    Unknown(String),
    /// Bound in a branch that may not run: one of these.
    Maybe(Vec<Binding>),
    /// Deleted (inside [`Binding::Maybe`]).
    Unbound,
}

/// What running a module-level function does, as far as its body tells.
#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionInfo {
    /// It may return a class: it defines one, calls `type`, or returns
    /// something that isn't plainly a value.
    pub may_return_class: bool,
    /// What in its body may change names of the module or attributes of
    /// classes when it runs.
    pub side_effects: Option<String>,
    /// It sets attributes or items on its arguments: as a class decorator,
    /// it may add or remove tests.
    pub changes_argument: bool,
    /// It returns its first argument, or a `functools.wraps` wrapper of it,
    /// and has no side effects: as a decorator it keeps the object (and its
    /// name).
    pub keeps: bool,
    /// It may change objects it isn't given and doesn't make (another
    /// module's classes): it sets their attributes, or calls `setattr`,
    /// `delattr`, `exec` or the like.
    pub foreign: bool,
}

/// What importing a module may change in other modules, or hand to their
/// code, by dotted path from the module (an imported name's, joined with
/// the rest of the path).
#[derive(Debug, Clone)]
pub(crate) enum Touch {
    /// An object whose attributes may change: assigned or deleted through,
    /// passed to a call, or a method called on it.
    Object(String),
    /// A name of another module assigned or deleted (`base.Base = …`).
    Bound(String),
    /// A callable of another module, called: what it does.
    Call(String),
    /// Something aye can't follow (`exec`, a function of the module that
    /// changes objects it doesn't make).
    Unknown(String),
}

/// A class base, resolved where the class statement runs.
#[derive(Debug, Clone)]
pub(crate) enum Base {
    Local(usize),
    /// Imported: resolved through [`Modules`](crate::project::Modules).
    Path(String),
    TestCase,
    Object,
    /// A class of the standard library or a builtin: no `TestCase` and no
    /// test attributes.
    Plain(String),
}

/// A class attribute, as `getattr(cls, name)` would find it.
#[derive(Debug, Clone)]
pub(crate) enum Member {
    Callable {
        range: TextRange,
        line: u32,
    },
    NotCallable,
    /// Bound to an imported name: callable or not, by what it is.
    Import(String),
    Unknown(String),
}
