//! A module as unittest's loader sees it, read without importing it.
//!
//! `TestLoader.loadTestsFromModule` takes every `TestCase` subclass bound in
//! the module's namespace (`dir(module)`, sorted by name), however it got
//! there: defined, imported or aliased. For each, `getTestCaseNames` takes
//! the callable attributes whose names start with `test`, inherited ones
//! included, sorted by name; with none, a `runTest` attribute is the test.
//!
//! [`summarize`] reads a module's statements in order into a [`Summary`]: the
//! final namespace, its classes (bases resolved where the class statement
//! runs, members as the class body leaves them) and why, if at all, the
//! namespace can't be read statically. What a name is bound to is
//! [`binding`]'s; the walk of the module's statements is [`builder`]'s, and
//! of a class statement [`class`]'s; what runs while the module is imported
//! is [`scan`]'s; and what a dotted path names, by the standard library's
//! tables, [`tables`]'s.

mod binding;
mod builder;
mod class;
mod scan;
mod tables;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use aye_adapter::{LineIndex, TextRange, ast};
use aye_index::{IndexVec, newtype_index};

pub(crate) use binding::{Base, Binding, FunctionInfo, Member, Touch};
pub(crate) use tables::{is_dunder, is_stdlib};

newtype_index! {
    /// A class's place among its module's classes ([`Summary::classes`]),
    /// in the order their statements are read.
    pub(crate) struct ClassIndex;
}

#[derive(Debug, Clone)]
pub(crate) struct ClassSummary {
    /// The name in the `class` statement: `__qualname__` at module level.
    pub name: String,
    pub range: TextRange,
    pub line: u32,
    pub bases: Result<Vec<Base>, String>,
    /// The class body's own namespace, as the body leaves it.
    pub members: BTreeMap<String, Member>,
    /// Why the class's attributes can't be read statically (a decorator,
    /// a metaclass, a dynamic body).
    pub dynamic: Option<String>,
    /// Defines `__init_subclass__`, which may change its subclasses.
    pub init_subclass: bool,
    /// Derives from `type`: calling it makes classes.
    pub metaclass: bool,
}

/// One module, read.
#[derive(Debug)]
pub(crate) struct Summary {
    /// The file, absolute.
    pub path: PathBuf,
    /// The namespace when the module has run, sorted by name as `dir()`
    /// sorts it.
    pub names: BTreeMap<String, Binding>,
    pub classes: IndexVec<ClassIndex, ClassSummary>,
    /// Why the namespace can't be read statically.
    pub dynamic: Option<String>,
    /// What importing it may change in other modules.
    pub touches: Vec<Touch>,
    /// The modules it imports while imported (as written: `a.b`, `.m`),
    /// and the names it imports from them (`a.b.name`, which may be
    /// submodules).
    pub imports: Vec<String>,
}

/// Read a module's statements in order.
pub(crate) fn summarize(path: &Path, module: &ast::ModModule, lines: &LineIndex) -> Summary {
    let mut builder = Builder {
        lines,
        names: BTreeMap::new(),
        classes: IndexVec::new(),
        dynamic: None,
        touches: Vec::new(),
        imports: Vec::new(),
    };
    builder.walk(&module.body, false);
    Summary {
        path: path.to_path_buf(),
        names: builder.names,
        classes: builder.classes,
        dynamic: builder.dynamic,
        touches: builder.touches,
        imports: builder.imports,
    }
}

struct Builder<'a> {
    lines: &'a LineIndex,
    names: BTreeMap<String, Binding>,
    classes: IndexVec<ClassIndex, ClassSummary>,
    dynamic: Option<String>,
    touches: Vec<Touch>,
    imports: Vec<String>,
}
