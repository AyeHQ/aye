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
//! namespace can't be read statically. [`Modules`] follows imports into the
//! project's own files, so that a class imported from a helper module, or a
//! base defined there, is read too; classes from installed packages are not
//! read. A [`Verdict`] is the module's tests in loader order, or why only an
//! import can tell.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aye_engine::ast::visitor::{self, Visitor};
use aye_engine::ast::{self, CmpOp, Expr, ExprCall, Stmt, StmtClassDef, StmtFunctionDef};
use aye_engine::{LineIndex, Ranged, TextRange};

/// The test method prefix (`TestLoader.testMethodPrefix`).
const PREFIX: &str = "test";

/// `TestCase` itself and its subclass in the standard library: `TestCase`
/// subclasses with no tests of their own.
const TESTCASE_ROOTS: &[&str] = &[
    "unittest.TestCase",
    "unittest.case.TestCase",
    "unittest.IsolatedAsyncioTestCase",
    "unittest.async_case.IsolatedAsyncioTestCase",
];

/// Builtins that rebind names or reach the namespace by name.
const DYNAMIC_CALLS: &[&str] = &[
    "exec",
    "eval",
    "globals",
    "locals",
    "vars",
    "setattr",
    "delattr",
    "__import__",
];

/// Builtins that only read what they are given.
const READING_BUILTINS: &[&str] = &[
    "print",
    "isinstance",
    "issubclass",
    "len",
    "repr",
    "str",
    "id",
    "hash",
    "hasattr",
    "getattr",
    "callable",
    "type",
    "list",
    "tuple",
    "dict",
    "set",
    "frozenset",
    "sorted",
    "iter",
    "any",
    "all",
    "vars",
    "dir",
];

/// Decorators that keep a test method callable (and its name).
const KEEPING_DECORATORS: &[&str] = &[
    "staticmethod",
    "classmethod",
    "unittest.skip",
    "unittest.skipIf",
    "unittest.skipUnless",
    "unittest.expectedFailure",
    "unittest.case.skip",
    "unittest.case.skipIf",
    "unittest.case.skipUnless",
    "unittest.case.expectedFailure",
    "unittest.mock.patch",
    "unittest.mock.patch.object",
    "unittest.mock.patch.dict",
    "unittest.mock.patch.multiple",
    "mock.patch",
    "mock.patch.object",
    "mock.patch.dict",
    "mock.patch.multiple",
    "functools.wraps",
    "functools.lru_cache",
    "functools.cache",
    "abc.abstractmethod",
    "typing.override",
    "typing_extensions.override",
    "typing.no_type_check",
    "contextlib.contextmanager",
    "contextlib.asynccontextmanager",
];

/// Decorators that make a method a descriptor that isn't callable.
const PROPERTY_DECORATORS: &[&str] = &[
    "property",
    "functools.cached_property",
    "abc.abstractproperty",
    "types.DynamicClassAttribute",
];

/// Class decorators that return the class with its test names unchanged
/// (`mock.patch` wraps each `test*` method in place).
const CLASS_DECORATORS: &[&str] = &[
    "unittest.skip",
    "unittest.skipIf",
    "unittest.skipUnless",
    "unittest.expectedFailure",
    "unittest.case.skip",
    "unittest.case.skipIf",
    "unittest.case.skipUnless",
    "unittest.case.expectedFailure",
    "unittest.mock.patch",
    "unittest.mock.patch.object",
    "unittest.mock.patch.dict",
    "unittest.mock.patch.multiple",
    "mock.patch",
    "mock.patch.object",
    "mock.patch.dict",
    "mock.patch.multiple",
    "typing.final",
    "typing_extensions.final",
];

/// Class decorators of the standard library that return the class (or, for
/// `dataclass(slots=True)`, a copy with the same bases and names) without
/// adding `test*` names, but may change how it is instantiated: accepted
/// on classes whose bases can't make them a `TestCase` (a dataclass the
/// test module imports, say).
const PLAIN_CLASS_DECORATORS: &[&str] = &[
    "dataclasses.dataclass",
    "functools.total_ordering",
    "enum.unique",
    "enum.verify",
    "typing.runtime_checkable",
    "typing_extensions.runtime_checkable",
];

/// Top-level modules of the standard library (Python 3.10 to 3.13), but
/// `unittest`, `doctest` and `test`, which the module reads by name. Their
/// classes are plain classes to unittest and their functions return no
/// test classes.
const STDLIB: &[&str] = &[
    "__future__",
    "abc",
    "aifc",
    "argparse",
    "array",
    "ast",
    "asynchat",
    "asyncio",
    "asyncore",
    "atexit",
    "audioop",
    "base64",
    "bdb",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "cProfile",
    "calendar",
    "cgi",
    "cgitb",
    "chunk",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "collections",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "crypt",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "distutils",
    "email",
    "encodings",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "genericpath",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "grp",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "imghdr",
    "imp",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "mailcap",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "modulefinder",
    "msvcrt",
    "multiprocessing",
    "netrc",
    "nis",
    "nntplib",
    "ntpath",
    "nturl2path",
    "numbers",
    "opcode",
    "operator",
    "optparse",
    "os",
    "ossaudiodev",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pipes",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posix",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "pyexpat",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtpd",
    "smtplib",
    "sndhdr",
    "socket",
    "socketserver",
    "spwd",
    "sqlite3",
    "sre_compile",
    "sre_constants",
    "sre_parse",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "sunau",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tabnanny",
    "tarfile",
    "telnetlib",
    "tempfile",
    "termios",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "types",
    "typing",
    "typing_extensions",
    "unicodedata",
    "urllib",
    "uu",
    "uuid",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "wsgiref",
    "xdrlib",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
];

/// `TestCase` subclasses of well-known installed packages, with no tests of
/// their own: importing one into a test module adds no tests.
const KNOWN_BASES: &[&str] = &[
    "django.test.TestCase",
    "django.test.SimpleTestCase",
    "django.test.TransactionTestCase",
    "django.test.LiveServerTestCase",
    "django.test.testcases.TestCase",
    "django.test.testcases.SimpleTestCase",
    "django.test.testcases.TransactionTestCase",
    "django.test.testcases.LiveServerTestCase",
    "django.contrib.staticfiles.testing.StaticLiveServerTestCase",
    "rest_framework.test.APITestCase",
    "rest_framework.test.APISimpleTestCase",
    "rest_framework.test.APITransactionTestCase",
    "rest_framework.test.APILiveServerTestCase",
    "twisted.trial.unittest.TestCase",
    "twisted.trial.unittest.SynchronousTestCase",
    "asynctest.TestCase",
    "aiounittest.AsyncTestCase",
    "absl.testing.absltest.TestCase",
    "absl.testing.parameterized.TestCase",
    "tornado.testing.AsyncTestCase",
    "tornado.testing.AsyncHTTPTestCase",
    "aiohttp.test_utils.AioHTTPTestCase",
    "flask_testing.TestCase",
    "pyfakefs.fake_filesystem_unittest.TestCase",
    "testtools.TestCase",
];

/// Whether a name imported from an installed package aye doesn't read
/// looks like a class holding tests (`Test*`, `*Test`, `*Tests`,
/// `*TestCase`), other than the well-known bases that hold none. Installed
/// packages are otherwise taken to export no test classes.
fn looks_like_tests(name: &str, path: &str) -> bool {
    if KNOWN_BASES.contains(&path) {
        return false;
    }
    let own = path.rsplit('.').next().unwrap_or(path);
    [name, own].iter().any(|n| {
        n.starts_with("Test")
            || n.ends_with("Test")
            || n.ends_with("Tests")
            || n.ends_with("TestCase")
    })
}

/// Whether `path` (dotted) is in the standard library, `unittest` included.
pub fn is_stdlib(path: &str) -> bool {
    let top = path.split('.').next().unwrap_or(path);
    top == "unittest" || STDLIB.contains(&top)
}

fn is_dunder(name: &str) -> bool {
    name.len() > 4 && name.starts_with("__") && name.ends_with("__")
}

// ---------------------------------------------------------------- summaries

/// What a name is bound to when the module has run, as far as the source
/// tells.
#[derive(Debug, Clone)]
pub enum Binding {
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
pub struct FunctionInfo {
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
pub enum Touch {
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
pub enum Base {
    Local(usize),
    /// Imported: resolved through [`Modules`].
    Path(String),
    TestCase,
    Object,
    /// A class of the standard library or a builtin: no `TestCase` and no
    /// test attributes.
    Plain(String),
}

/// A class attribute, as `getattr(cls, name)` would find it.
#[derive(Debug, Clone)]
pub enum Member {
    Callable {
        range: TextRange,
        line: u32,
    },
    NotCallable,
    /// Bound to an imported name: callable or not, by what it is.
    Import(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct ClassSummary {
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
pub struct Summary {
    /// The file, absolute.
    pub path: PathBuf,
    /// The namespace when the module has run, sorted by name as `dir()`
    /// sorts it.
    pub names: BTreeMap<String, Binding>,
    pub classes: Vec<ClassSummary>,
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
pub fn summarize(path: &Path, module: &ast::ModModule, lines: &LineIndex) -> Summary {
    let mut builder = Builder {
        lines,
        names: BTreeMap::new(),
        classes: Vec::new(),
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
    classes: Vec<ClassSummary>,
    dynamic: Option<String>,
    touches: Vec<Touch>,
    imports: Vec<String>,
}

/// `expr` as a dotted path through an imported name (a module's, or one
/// imported from a module): `base.Base` for `base.Base` after `import base`.
fn imported_path(names: &BTreeMap<String, Binding>, expr: &Expr) -> Option<String> {
    let dotted = dotted(expr)?;
    let (head, rest) = dotted.split_once('.').unwrap_or((dotted.as_str(), ""));
    match names.get(head) {
        Some(Binding::Module(path) | Binding::Import(path)) => Some(if rest.is_empty() {
            path.clone()
        } else {
            format!("{path}.{rest}")
        }),
        _ => None,
    }
}

/// `name` as a dotted path through `names`, or `None`.
fn dotted(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.id.as_str().to_string()),
        Expr::Attribute(attr) => Some(format!("{}.{}", dotted(&attr.value)?, attr.attr.id)),
        _ => None,
    }
}

/// Literals, and operations and containers of them: values that are no
/// class and not callable.
fn literal_like(expr: &Expr) -> bool {
    match expr {
        Expr::StringLiteral(_)
        | Expr::BytesLiteral(_)
        | Expr::NumberLiteral(_)
        | Expr::BooleanLiteral(_)
        | Expr::NoneLiteral(_)
        | Expr::EllipsisLiteral(_)
        | Expr::FString(_)
        | Expr::TString(_) => true,
        Expr::List(list) => list.elts.iter().all(literal_like),
        Expr::Tuple(tuple) => tuple.elts.iter().all(literal_like),
        Expr::Set(set) => set.elts.iter().all(literal_like),
        Expr::Dict(dict) => dict
            .items
            .iter()
            .all(|item| item.key.as_ref().is_none_or(literal_like) && literal_like(&item.value)),
        Expr::BinOp(op) => literal_like(&op.left) && literal_like(&op.right),
        Expr::UnaryOp(op) => literal_like(&op.operand),
        Expr::BoolOp(op) => op.values.iter().all(literal_like),
        Expr::Compare(cmp) => literal_like(&cmp.left) && cmp.comparators.iter().all(literal_like),
        _ => false,
    }
}

/// Values that are no class and not callable whatever they hold:
/// containers, comprehensions, strings.
fn not_callable(expr: &Expr) -> bool {
    literal_like(expr)
        || matches!(
            expr,
            Expr::List(_)
                | Expr::Tuple(_)
                | Expr::Set(_)
                | Expr::Dict(_)
                | Expr::ListComp(_)
                | Expr::SetComp(_)
                | Expr::DictComp(_)
                | Expr::Generator(_)
        )
}

/// `__name__ == "__main__"`, either way round: false when imported.
fn is_main_guard(test: &Expr) -> bool {
    let Expr::Compare(cmp) = test else {
        return false;
    };
    if cmp.ops.as_ref() != [CmpOp::Eq] || cmp.comparators.len() != 1 {
        return false;
    }
    let is_name = |e: &Expr| matches!(e, Expr::Name(n) if n.id.as_str() == "__name__");
    let is_main = |e: &Expr| matches!(e, Expr::StringLiteral(s) if s.value.to_str() == "__main__");
    let right = &cmp.comparators[0];
    (is_name(&cmp.left) && is_main(right)) || (is_main(&cmp.left) && is_name(right))
}

/// `TYPE_CHECKING` or `typing.TYPE_CHECKING`: false at runtime.
fn is_type_checking(test: &Expr) -> bool {
    match test {
        Expr::Name(name) => name.id.as_str() == "TYPE_CHECKING",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "TYPE_CHECKING",
        _ => false,
    }
}

impl Builder<'_> {
    fn line(&self, range: TextRange) -> u32 {
        u32::try_from(self.lines.line_index(range.start()).get()).unwrap_or(u32::MAX)
    }

    fn dynamic(&mut self, reason: impl Into<String>) {
        if self.dynamic.is_none() {
            self.dynamic = Some(reason.into());
        }
    }

    fn bind(&mut self, name: &str, binding: Binding, conditional: bool) {
        let binding = if conditional {
            let old = self.names.get(name).cloned().unwrap_or(Binding::Unbound);
            let mut options = Vec::new();
            for option in [old, binding] {
                match option {
                    Binding::Maybe(inner) => options.extend(inner),
                    other => options.push(other),
                }
            }
            Binding::Maybe(options)
        } else {
            binding
        };
        if name == "load_tests" {
            self.dynamic("the module defines load_tests, which decides its tests");
        }
        if matches!(name, "__getattr__" | "__dir__") {
            self.dynamic(format!(
                "the module defines {name}, which changes what dir() lists"
            ));
        }
        self.names.insert(name.to_string(), binding);
    }

    fn unbind(&mut self, name: &str, conditional: bool) {
        if conditional {
            self.bind(name, Binding::Unbound, true);
        } else {
            self.names.remove(name);
        }
    }

    /// The binding of the head of a dotted path, joined with its rest.
    fn path_of(&self, expr: &Expr) -> Option<String> {
        let dotted = dotted(expr)?;
        let (head, rest) = dotted
            .split_once('.')
            .map_or((dotted.as_str(), ""), |(h, r)| (h, r));
        let join = |base: &str| {
            if rest.is_empty() {
                base.to_string()
            } else {
                format!("{base}.{rest}")
            }
        };
        match self.names.get(head) {
            Some(Binding::Module(path) | Binding::Import(path)) => Some(join(path)),
            Some(_) => None,
            // A builtin.
            None => Some(join(&format!("builtins.{head}"))),
        }
    }

    fn walk(&mut self, body: &[Stmt], conditional: bool) {
        for stmt in body {
            self.stmt(stmt, conditional);
        }
    }

    fn stmt(&mut self, stmt: &Stmt, conditional: bool) {
        match stmt {
            Stmt::FunctionDef(def) => {
                self.decorators(&def.decorator_list);
                for default in function_defaults(def) {
                    self.scan(default);
                }
                let info = function_info(def);
                self.bind(def.name.id.as_str(), Binding::Function(info), conditional);
            }
            Stmt::ClassDef(class) => {
                self.decorators(&class.decorator_list);
                if let Some(arguments) = &class.arguments {
                    for arg in &arguments.args {
                        self.scan(arg);
                    }
                }
                let index = self.class(class);
                self.bind(class.name.id.as_str(), Binding::Class(index), conditional);
            }
            Stmt::Assign(assign) => {
                self.scan(&assign.value);
                let value = self.value(&assign.value);
                for target in &assign.targets {
                    self.assign(target, &assign.value, &value, conditional);
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Some(value) = &assign.value {
                    self.scan(value);
                    let binding = self.value(value);
                    self.assign(&assign.target, value, &binding, conditional);
                }
            }
            Stmt::AugAssign(assign) => {
                self.scan(&assign.value);
                match &*assign.target {
                    Expr::Name(name) => {
                        let binding = match self.names.get(name.id.as_str()) {
                            Some(Binding::Constant | Binding::Plain) => Binding::Plain,
                            _ => Binding::Unknown(format!("{} is changed in place", name.id)),
                        };
                        self.bind(name.id.as_str(), binding, conditional);
                    }
                    target => self.mutation(target),
                }
            }
            Stmt::Delete(delete) => {
                for target in &delete.targets {
                    match target {
                        Expr::Name(name) => self.unbind(name.id.as_str(), conditional),
                        other => self.mutation(other),
                    }
                }
            }
            Stmt::Import(import) => {
                for alias in &import.names {
                    let dotted = alias.name.id.as_str();
                    self.imports.push(dotted.to_string());
                    // `import a.b` binds `a`; `import a.b as c` binds `c`.
                    let (local, path) = if let Some(asname) = &alias.asname {
                        (asname.id.as_str(), dotted.to_string())
                    } else {
                        let head = dotted.split('.').next().unwrap_or(dotted);
                        (head, head.to_string())
                    };
                    self.bind(local, Binding::Module(path), conditional);
                }
            }
            Stmt::ImportFrom(import) => {
                let module = format!(
                    "{}{}",
                    ".".repeat(import.level as usize),
                    import.module.as_ref().map_or("", |m| m.id.as_str())
                );
                self.imports.push(module.clone());
                for alias in &import.names {
                    let name = alias.name.id.as_str();
                    if name == "*" {
                        if import.level > 0 || !is_stdlib(&module) {
                            self.dynamic(format!("the module star-imports {module}"));
                        }
                        continue;
                    }
                    let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                    let path = if module.ends_with('.') {
                        format!("{module}{name}")
                    } else {
                        format!("{module}.{name}")
                    };
                    self.imports.push(path.clone());
                    self.bind(local, Binding::Import(path), conditional);
                }
            }
            Stmt::Expr(expr) => self.scan(&expr.value),
            Stmt::If(s) => {
                let skipped = is_main_guard(&s.test) || is_type_checking(&s.test);
                if !skipped {
                    self.scan(&s.test);
                    self.walk(&s.body, true);
                }
                let only_else =
                    s.elif_else_clauses.len() == 1 && s.elif_else_clauses[0].test.is_none();
                for clause in &s.elif_else_clauses {
                    if let Some(test) = &clause.test {
                        self.scan(test);
                    }
                    // `if __name__ == "__main__": … else: …` runs the else.
                    self.walk(&clause.body, conditional || !(skipped && only_else));
                }
            }
            Stmt::Try(s) => {
                self.walk(&s.body, true);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        if let Some(kind) = &handler.type_ {
                            self.scan(kind);
                        }
                        self.walk(&handler.body, true);
                    }
                }
                self.walk(&s.orelse, true);
                self.walk(&s.finalbody, conditional);
            }
            Stmt::With(s) => {
                for item in &s.items {
                    self.scan(&item.context_expr);
                    if let Some(vars) = &item.optional_vars {
                        self.bind_target(vars, &Binding::Unknown("bound by with".into()), true);
                    }
                }
                self.walk(&s.body, true);
            }
            Stmt::For(s) => {
                self.scan(&s.iter);
                self.bind_target(&s.target, &Binding::Unknown("bound by a loop".into()), true);
                self.walk(&s.body, true);
                self.walk(&s.orelse, true);
            }
            Stmt::While(s) => {
                self.scan(&s.test);
                self.walk(&s.body, true);
                self.walk(&s.orelse, true);
            }
            Stmt::Match(s) => {
                self.scan(&s.subject);
                for case in &s.cases {
                    let mut captures = Vec::new();
                    pattern_captures(&case.pattern, &mut captures);
                    for name in captures {
                        self.bind(&name, Binding::Unknown("bound by match".into()), true);
                    }
                    self.walk(&case.body, true);
                }
            }
            Stmt::Raise(_) => self.dynamic("the module may raise while it is imported (a skip?)"),
            Stmt::TypeAlias(alias) => {
                if let Expr::Name(name) = &*alias.name {
                    self.bind(name.id.as_str(), Binding::Plain, conditional);
                }
            }
            Stmt::Assert(s) => self.scan(&s.test),
            _ => {}
        }
    }

    fn assign(&mut self, target: &Expr, value: &Expr, binding: &Binding, conditional: bool) {
        match (target, value) {
            (Expr::Tuple(targets), Expr::Tuple(values))
                if targets.elts.len() == values.elts.len()
                    && !targets.elts.iter().any(|t| matches!(t, Expr::Starred(_))) =>
            {
                for (target, value) in targets.elts.iter().zip(&values.elts) {
                    let binding = self.value(value);
                    self.assign(target, value, &binding, conditional);
                }
            }
            _ => self.bind_target(target, binding, conditional),
        }
    }

    fn bind_target(&mut self, target: &Expr, binding: &Binding, conditional: bool) {
        match target {
            Expr::Name(name) => {
                let binding = if is_dunder(name.id.as_str())
                    && !matches!(name.id.as_str(), "__getattr__" | "__dir__")
                {
                    Binding::Constant
                } else {
                    binding.clone()
                };
                self.bind(name.id.as_str(), binding, conditional);
            }
            Expr::Tuple(tuple) => {
                for elt in &tuple.elts {
                    self.bind_target(
                        elt,
                        &Binding::Unknown("bound by unpacking".into()),
                        conditional,
                    );
                }
            }
            Expr::List(list) => {
                for elt in &list.elts {
                    self.bind_target(
                        elt,
                        &Binding::Unknown("bound by unpacking".into()),
                        conditional,
                    );
                }
            }
            Expr::Starred(starred) => {
                self.bind_target(&starred.value, &Binding::Constant, conditional);
            }
            other => self.mutation(other),
        }
    }

    /// An assignment to an attribute or an item: a class's attributes may
    /// change, and so may the module's (`sys.modules[…]`).
    fn mutation(&mut self, target: &Expr) {
        let mut root = target;
        loop {
            match root {
                Expr::Attribute(attr) => root = &attr.value,
                Expr::Subscript(sub) => root = &sub.value,
                _ => break,
            }
        }
        let Expr::Name(name) = root else {
            self.scan(target);
            return;
        };
        let shown = || format!("{} is changed while the module is imported", name.id);
        match self.names.get(name.id.as_str()) {
            Some(Binding::Module(path)) => {
                if path == "sys" {
                    self.dynamic(shown());
                }
            }
            Some(Binding::Constant | Binding::Plain | Binding::Function(_)) | None => {}
            Some(_) => self.dynamic(shown()),
        }
        // Through another module (`base.Base.test_x = f`, `del
        // base.Base.test_y`): its classes may change.
        if matches!(
            self.names.get(name.id.as_str()),
            Some(Binding::Module(_) | Binding::Import(_))
        ) {
            let owner = match target {
                Expr::Attribute(attr) => Some(&*attr.value),
                Expr::Subscript(sub) => Some(&*sub.value),
                _ => None,
            };
            match owner.and_then(|owner| imported_path(&self.names, owner)) {
                Some(path) => self.touches.push(Touch::Object(path)),
                None => self.touches.push(Touch::Unknown(shown())),
            }
            if let Some(path) = imported_path(&self.names, target) {
                self.touches.push(Touch::Bound(path));
            }
        }
        self.scan(target);
    }

    /// Expressions run while the module is imported: calls that rebind
    /// names or may change classes.
    fn scan(&mut self, expr: &Expr) {
        let mut found = Scan {
            names: &self.names,
            reason: None,
            walrus: Vec::new(),
            touches: Vec::new(),
        };
        found.visit_expr(expr);
        let Scan {
            reason,
            walrus,
            touches,
            ..
        } = found;
        self.touches.extend(touches);
        if let Some(reason) = reason {
            self.dynamic(reason);
        }
        for name in walrus {
            self.bind(&name, Binding::Unknown("bound by :=".into()), true);
        }
    }

    /// Module-level decorators run at import: a local decorator with side
    /// effects, or one reaching into a class, may change classes.
    fn decorators(&mut self, decorators: &[ast::Decorator]) {
        for decorator in decorators {
            self.scan(&decorator.expression);
            let target = match &decorator.expression {
                Expr::Call(call) => &*call.func,
                other => other,
            };
            if let Expr::Name(name) = target
                && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
                && let Some(effect) = &info.side_effects
            {
                let reason = format!("the decorator {} {effect}", name.id);
                if info.foreign {
                    self.touches.push(Touch::Unknown(reason.clone()));
                }
                self.dynamic(reason);
            }
        }
    }

    /// What a value is, as a binding.
    fn value(&self, expr: &Expr) -> Binding {
        if literal_like(expr) || not_callable(expr) {
            return Binding::Constant;
        }
        match expr {
            Expr::Lambda(_) => Binding::Plain,
            Expr::Name(name) => self
                .names
                .get(name.id.as_str())
                .cloned()
                .unwrap_or(Binding::Plain),
            Expr::Attribute(_) => {
                let Some(dotted) = dotted(expr) else {
                    return Binding::Unknown("an attribute of a computed value".into());
                };
                let head = dotted.split('.').next().unwrap_or_default();
                match self.names.get(head) {
                    Some(Binding::Module(_) | Binding::Import(_)) => {
                        Binding::Import(self.path_of(expr).unwrap_or(dotted))
                    }
                    Some(Binding::Constant | Binding::Function(_)) | None => Binding::Plain,
                    Some(_) => Binding::Unknown(format!("{dotted} may be a class")),
                }
            }
            Expr::Call(call) => self.call_value(call),
            Expr::Subscript(sub) => match self.value(&sub.value) {
                Binding::Constant => Binding::Constant,
                Binding::Module(_) => Binding::Plain,
                Binding::Import(path) if is_stdlib(&path) => Binding::Plain,
                _ => Binding::Unknown("an item of a computed value".into()),
            },
            Expr::If(ternary) => {
                Binding::Maybe(vec![self.value(&ternary.body), self.value(&ternary.orelse)])
            }
            Expr::Named(named) => self.value(&named.value),
            Expr::BinOp(_) | Expr::UnaryOp(_) | Expr::BoolOp(_) | Expr::Compare(_) => {
                Binding::Plain
            }
            _ => Binding::Unknown("a value computed at import".into()),
        }
    }

    fn call_value(&self, call: &ExprCall) -> Binding {
        match &*call.func {
            Expr::Name(name) => match self.names.get(name.id.as_str()) {
                None => match name.id.as_str() {
                    "type" => Binding::Unknown("type() may make a class".into()),
                    "getattr" => Binding::Unknown("getattr() may return a class".into()),
                    _ => Binding::Plain,
                },
                Some(Binding::Class(index)) => {
                    if self.classes[*index].metaclass {
                        Binding::Unknown(format!("{} makes classes", name.id))
                    } else {
                        Binding::Plain
                    }
                }
                Some(Binding::Function(info)) => {
                    if info.may_return_class {
                        Binding::Unknown(format!("{}() may return a class", name.id))
                    } else {
                        Binding::Plain
                    }
                }
                Some(Binding::Import(path) | Binding::Module(path)) => {
                    Binding::Returned(path.clone())
                }
                Some(Binding::Constant | Binding::Plain) => Binding::Plain,
                Some(_) => Binding::Unknown(format!("{}() may return a class", name.id)),
            },
            func @ Expr::Attribute(attr) => {
                let head = dotted(func).and_then(|d| d.split('.').next().map(str::to_string));
                match head.as_deref().and_then(|h| self.names.get(h)) {
                    Some(Binding::Module(_) | Binding::Import(_)) => self
                        .path_of(func)
                        .map_or_else(|| Binding::Unknown("a call".into()), Binding::Returned),
                    Some(Binding::Class(_) | Binding::Unknown(_) | Binding::Maybe(_)) => {
                        Binding::Unknown(format!("{}() may return a class", attr.attr.id))
                    }
                    _ => Binding::Plain,
                }
            }
            _ => Binding::Unknown("a call to a computed callable".into()),
        }
    }

    // ------------------------------------------------------------ classes

    fn class(&mut self, class: &StmtClassDef) -> usize {
        let mut summary = ClassSummary {
            name: class.name.id.as_str().to_string(),
            range: class.name.range(),
            line: self.line(class.name.range()),
            bases: Ok(Vec::new()),
            members: BTreeMap::new(),
            dynamic: None,
            init_subclass: false,
            metaclass: false,
        };
        let mut bases = Vec::new();
        let mut failed = None;
        if let Some(arguments) = &class.arguments {
            for base in &arguments.args {
                match self.base(base) {
                    Ok(base) => {
                        if matches!(&base, Base::Plain(p) if p == "builtins.type") {
                            summary.metaclass = true;
                        }
                        bases.push(base);
                    }
                    Err(reason) => {
                        failed.get_or_insert(format!("{} {reason}", summary.name));
                    }
                }
            }
            for keyword in &arguments.keywords {
                let reason = match keyword.arg.as_ref().map(|a| a.id.as_str()) {
                    Some("metaclass") => {
                        let path = self.path_of(&keyword.value);
                        if matches!(path.as_deref(), Some("abc.ABCMeta")) {
                            continue;
                        }
                        "has a metaclass, which may add attributes".to_string()
                    }
                    Some(other) => format!("passes {other}= to __init_subclass__"),
                    None => "passes keywords to __init_subclass__".to_string(),
                };
                summary
                    .dynamic
                    .get_or_insert(format!("{} {reason}", summary.name));
            }
        }
        let plain = failed.is_none()
            && bases
                .iter()
                .all(|base| matches!(base, Base::Object | Base::Plain(_)));
        summary.bases = failed.map_or(Ok(bases), Err);
        for decorator in &class.decorator_list {
            if let Err(reason) = self.class_decorator(&decorator.expression, plain) {
                summary
                    .dynamic
                    .get_or_insert(format!("{} {reason}", summary.name));
            }
        }
        let mut members = BTreeMap::new();
        let mut dynamic = None;
        self.class_body(
            &class.body,
            &summary.name,
            &mut members,
            &mut dynamic,
            &mut summary.init_subclass,
            false,
        );
        summary.members = members;
        if summary.dynamic.is_none() {
            summary.dynamic = dynamic;
        }
        self.classes.push(summary);
        self.classes.len() - 1
    }

    fn base(&self, expr: &Expr) -> Result<Base, String> {
        match expr {
            Expr::Name(name) => match self.names.get(name.id.as_str()) {
                Some(Binding::Class(index)) => Ok(Base::Local(*index)),
                Some(Binding::Import(path)) => classify(path),
                Some(_) => Err(format!("inherits from {}, bound at runtime", name.id)),
                None if name.id.as_str() == "object" => Ok(Base::Object),
                None => Ok(Base::Plain(format!("builtins.{}", name.id))),
            },
            Expr::Attribute(_) => match self.path_of(expr) {
                Some(path) if !path.starts_with("builtins.") => classify(&path),
                _ => Err("inherits from an attribute of a computed value".into()),
            },
            Expr::Subscript(sub) => self.base(&sub.value),
            _ => Err("has bases computed at runtime".into()),
        }
    }

    /// Whether a class decorator keeps the class's test names; `plain` says
    /// the class's bases can't make it a `TestCase`.
    fn class_decorator(&self, expr: &Expr, plain: bool) -> Result<(), String> {
        let callee = match expr {
            Expr::Call(call) => &*call.func,
            other => other,
        };
        if let Expr::Name(name) = callee
            && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
        {
            return if info.keeps && !info.changes_argument {
                Ok(())
            } else {
                Err(format!(
                    "is decorated with {}, which may change it",
                    name.id
                ))
            };
        }
        match self.path_of(callee) {
            Some(path)
                if CLASS_DECORATORS.contains(&path.as_str())
                    || path.starts_with("pytest.mark.")
                    || (plain && PLAIN_CLASS_DECORATORS.contains(&path.as_str())) =>
            {
                Ok(())
            }
            Some(path) => Err(format!(
                "is decorated with {}",
                path.trim_start_matches("builtins.")
            )),
            None => Err("is decorated with a computed decorator".into()),
        }
    }

    fn class_body(
        &self,
        body: &[Stmt],
        class: &str,
        members: &mut BTreeMap<String, Member>,
        dynamic: &mut Option<String>,
        init_subclass: &mut bool,
        conditional: bool,
    ) {
        let interesting = |name: &str| name.starts_with(PREFIX) || name == "runTest";
        let put = |members: &mut BTreeMap<String, Member>, name: &str, member: Member| {
            let member = if conditional && interesting(name) {
                Member::Unknown(format!("{class}.{name} is defined conditionally"))
            } else {
                member
            };
            members.insert(name.to_string(), member);
        };
        for stmt in body {
            if let Some(reason) = class_body_dynamics(stmt) {
                dynamic.get_or_insert(format!("{class} {reason}"));
            }
            match stmt {
                Stmt::FunctionDef(def) => {
                    if def.name.id.as_str() == "__init_subclass__" {
                        *init_subclass = true;
                    }
                    let member = self.method(def, class, members);
                    put(members, def.name.id.as_str(), member);
                }
                Stmt::ClassDef(nested) => {
                    let line = self.line(nested.name.range());
                    put(
                        members,
                        nested.name.id.as_str(),
                        Member::Callable {
                            range: nested.name.range(),
                            line,
                        },
                    );
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.class_assign(
                            target,
                            Some(&assign.value),
                            class,
                            members,
                            dynamic,
                            conditional,
                        );
                    }
                }
                Stmt::AnnAssign(assign) => {
                    if let Some(value) = &assign.value {
                        self.class_assign(
                            &assign.target,
                            Some(value),
                            class,
                            members,
                            dynamic,
                            conditional,
                        );
                    }
                }
                Stmt::AugAssign(assign) => {
                    self.class_assign(&assign.target, None, class, members, dynamic, conditional);
                }
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        if let Expr::Name(name) = target {
                            if conditional && interesting(name.id.as_str()) {
                                put(members, name.id.as_str(), Member::NotCallable);
                            } else {
                                members.remove(name.id.as_str());
                            }
                        }
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let local = alias.asname.as_ref().map_or_else(
                            || alias.name.id.as_str().split('.').next().unwrap_or_default(),
                            |a| a.id.as_str(),
                        );
                        put(members, local, Member::NotCallable);
                    }
                }
                Stmt::ImportFrom(import) => {
                    let module = format!(
                        "{}{}",
                        ".".repeat(import.level as usize),
                        import.module.as_ref().map_or("", |m| m.id.as_str())
                    );
                    for alias in &import.names {
                        let name = alias.name.id.as_str();
                        if name == "*" {
                            dynamic.get_or_insert(format!("{class} star-imports {module}"));
                            continue;
                        }
                        let local = alias.asname.as_ref().map_or(name, |a| a.id.as_str());
                        let path = if module.ends_with('.') {
                            format!("{module}{name}")
                        } else {
                            format!("{module}.{name}")
                        };
                        put(members, local, Member::Import(path));
                    }
                }
                Stmt::If(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    for clause in &s.elif_else_clauses {
                        self.class_body(&clause.body, class, members, dynamic, init_subclass, true);
                    }
                }
                Stmt::Try(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    for handler in &s.handlers {
                        if let Some(handler) = handler.as_except_handler() {
                            self.class_body(
                                &handler.body,
                                class,
                                members,
                                dynamic,
                                init_subclass,
                                true,
                            );
                        }
                    }
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                    self.class_body(
                        &s.finalbody,
                        class,
                        members,
                        dynamic,
                        init_subclass,
                        conditional,
                    );
                }
                Stmt::With(s) => {
                    for item in &s.items {
                        if let Some(vars) = &item.optional_vars {
                            class_targets(vars, class, members, "is bound by with");
                        }
                    }
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                }
                Stmt::For(s) => {
                    class_targets(&s.target, class, members, "is bound by a loop");
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                }
                Stmt::While(s) => {
                    self.class_body(&s.body, class, members, dynamic, init_subclass, true);
                    self.class_body(&s.orelse, class, members, dynamic, init_subclass, true);
                }
                Stmt::Match(s) => {
                    for case in &s.cases {
                        let mut captures = Vec::new();
                        pattern_captures(&case.pattern, &mut captures);
                        for name in captures {
                            if interesting(&name) {
                                members.insert(
                                    name.clone(),
                                    Member::Unknown(format!("{class}.{name} is bound by match")),
                                );
                            }
                        }
                        self.class_body(&case.body, class, members, dynamic, init_subclass, true);
                    }
                }
                _ => {}
            }
        }
    }

    fn class_assign(
        &self,
        target: &Expr,
        value: Option<&Expr>,
        class: &str,
        members: &mut BTreeMap<String, Member>,
        dynamic: &mut Option<String>,
        conditional: bool,
    ) {
        match target {
            Expr::Name(name) => {
                let id = name.id.as_str();
                if matches!(
                    id,
                    "__qualname__" | "__name__" | "__module__" | "__class__" | "__dict__"
                ) {
                    dynamic.get_or_insert(format!("{class} sets {id}"));
                }
                let member = match value {
                    Some(value) => self.member_value(value, name.range(), members),
                    None => Member::Unknown(format!("{class}.{id} is changed in place")),
                };
                let member = if conditional && (id.starts_with(PREFIX) || id == "runTest") {
                    Member::Unknown(format!("{class}.{id} is defined conditionally"))
                } else {
                    member
                };
                members.insert(id.to_string(), member);
            }
            Expr::Tuple(_) | Expr::List(_) | Expr::Starred(_) => {
                class_targets(target, class, members, "is bound by unpacking");
            }
            _ => {}
        }
    }

    /// What `name = value` in a class body makes the attribute: callable,
    /// not, or unknown.
    fn member_value(
        &self,
        value: &Expr,
        target: TextRange,
        members: &BTreeMap<String, Member>,
    ) -> Member {
        let callable = || Member::Callable {
            range: target,
            line: self.line(target),
        };
        if not_callable(value) {
            return Member::NotCallable;
        }
        match value {
            Expr::Lambda(_) => callable(),
            Expr::Name(name) => {
                let id = name.id.as_str();
                if let Some(member) = members.get(id) {
                    return match member {
                        Member::Callable { .. } => callable(),
                        other => other.clone(),
                    };
                }
                match self.names.get(id) {
                    Some(Binding::Function(_) | Binding::Class(_)) | None => callable(),
                    Some(Binding::Module(_) | Binding::Constant) => Member::NotCallable,
                    Some(Binding::Import(path)) => Member::Import(path.clone()),
                    Some(_) => Member::Unknown(format!("{id} may or may not be callable")),
                }
            }
            Expr::Call(call) => match self.path_of(&call.func).as_deref() {
                Some("builtins.staticmethod" | "builtins.classmethod" | "functools.partial") => {
                    callable()
                }
                Some(
                    "builtins.property"
                    | "builtins.list"
                    | "builtins.dict"
                    | "builtins.set"
                    | "builtins.tuple"
                    | "builtins.frozenset"
                    | "builtins.str"
                    | "builtins.int"
                    | "builtins.float"
                    | "builtins.bool"
                    | "builtins.bytes"
                    | "builtins.range"
                    | "functools.cached_property",
                ) => Member::NotCallable,
                _ => Member::Unknown("a value computed when the class is made".into()),
            },
            _ => Member::Unknown("a value computed when the class is made".into()),
        }
    }

    /// What a decorated method is: callable unless its outermost known
    /// decorator makes a descriptor that isn't (`property`); unknown when a
    /// decorator may replace it.
    fn method(
        &self,
        def: &StmtFunctionDef,
        class: &str,
        members: &BTreeMap<String, Member>,
    ) -> Member {
        let range = def.name.range();
        let mut member = Member::Callable {
            range,
            line: self.line(range),
        };
        for decorator in def.decorator_list.iter().rev() {
            let callee = match &decorator.expression {
                Expr::Call(call) => &*call.func,
                other => other,
            };
            if let Expr::Name(name) = callee
                && let Some(Binding::Function(info)) = self.names.get(name.id.as_str())
            {
                if info.keeps {
                    continue;
                }
                return Member::Unknown(format!(
                    "{class}.{} is decorated with {}",
                    def.name.id, name.id
                ));
            }
            if let Expr::Attribute(attr) = callee
                && let Expr::Name(owner) = &*attr.value
                && matches!(attr.attr.id.as_str(), "setter" | "getter" | "deleter")
                && members.contains_key(owner.id.as_str())
            {
                member = Member::NotCallable;
                continue;
            }
            match self.path_of(callee) {
                Some(path)
                    if KEEPING_DECORATORS.contains(&path.trim_start_matches("builtins."))
                        || path.starts_with("pytest.mark.") =>
                {
                    if !matches!(member, Member::NotCallable)
                        || path.contains("skip")
                        || path.contains("patch")
                    {
                        member = Member::Callable {
                            range,
                            line: self.line(range),
                        };
                    }
                }
                Some(path)
                    if PROPERTY_DECORATORS.contains(&path.trim_start_matches("builtins.")) =>
                {
                    member = Member::NotCallable;
                }
                Some(path) => {
                    return Member::Unknown(format!(
                        "{class}.{} is decorated with {}",
                        def.name.id,
                        path.trim_start_matches("builtins.")
                    ));
                }
                None => {
                    return Member::Unknown(format!(
                        "{class}.{} has a computed decorator",
                        def.name.id
                    ));
                }
            }
        }
        member
    }
}

/// A base's dotted path, classified: `TestCase` itself, a plain class of
/// the standard library, or one to resolve.
fn classify(path: &str) -> Result<Base, String> {
    if TESTCASE_ROOTS.contains(&path) {
        return Ok(Base::TestCase);
    }
    if path == "builtins.object" {
        return Ok(Base::Object);
    }
    let top = path.split('.').next().unwrap_or(path);
    if top == "doctest"
        || matches!(
            path,
            "unittest.FunctionTestCase" | "unittest.case.FunctionTestCase"
        )
    {
        return Err(format!(
            "inherits from {path}, which brings a test of its own"
        ));
    }
    if top == "unittest" || STDLIB.contains(&top) {
        return Ok(Base::Plain(path.to_string()));
    }
    Ok(Base::Path(path.to_string()))
}

/// Targets in a class body bound in ways the source doesn't value.
fn class_targets(target: &Expr, class: &str, members: &mut BTreeMap<String, Member>, how: &str) {
    match target {
        Expr::Name(name) => {
            members.insert(
                name.id.as_str().to_string(),
                Member::Unknown(format!("{class}.{} {how}", name.id)),
            );
        }
        Expr::Tuple(tuple) => {
            for elt in &tuple.elts {
                class_targets(elt, class, members, how);
            }
        }
        Expr::List(list) => {
            for elt in &list.elts {
                class_targets(elt, class, members, how);
            }
        }
        Expr::Starred(starred) => class_targets(&starred.value, class, members, how),
        _ => {}
    }
}

/// What in a class body's statement (not in its methods) may add attributes
/// the source doesn't spell: `exec`, `locals()`, `vars()`, `setattr`.
fn class_body_dynamics(stmt: &Stmt) -> Option<String> {
    struct Finder(Option<String>);
    impl<'a> Visitor<'a> for Finder {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            if matches!(stmt, Stmt::FunctionDef(_) | Stmt::ClassDef(_)) {
                return;
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            if let Expr::Call(call) = expr
                && let Expr::Name(name) = &*call.func
                && matches!(
                    name.id.as_str(),
                    "exec" | "eval" | "locals" | "vars" | "setattr" | "globals"
                )
            {
                self.0
                    .get_or_insert(format!("calls {}() in its body", name.id));
            }
            if matches!(expr, Expr::Lambda(_)) {
                return;
            }
            visitor::walk_expr(self, expr);
        }
    }
    match stmt {
        Stmt::FunctionDef(def) => {
            let mut finder = Finder(None);
            for decorator in &def.decorator_list {
                finder.visit_expr(&decorator.expression);
            }
            finder.0
        }
        Stmt::ClassDef(_) => None,
        other => {
            let mut finder = Finder(None);
            finder.visit_stmt(other);
            finder.0
        }
    }
}

/// Expressions run at import: dynamic calls, calls given a class of the
/// module (which may change it), calls of local functions with side
/// effects, and `:=` targets.
struct Scan<'n> {
    names: &'n BTreeMap<String, Binding>,
    reason: Option<String>,
    walrus: Vec<String>,
    /// What the calls may change in, or hand to, other modules.
    touches: Vec<Touch>,
}

impl<'a> Visitor<'a> for Scan<'_> {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => self.call(call),
            Expr::Named(named) => {
                if let Expr::Name(name) = &*named.target {
                    self.walrus.push(name.id.as_str().to_string());
                }
            }
            _ => {}
        }
        visitor::walk_expr(self, expr);
    }
}

impl Scan<'_> {
    fn note(&mut self, reason: String) {
        self.reason.get_or_insert(reason);
    }

    fn call(&mut self, call: &ExprCall) {
        let callee = &*call.func;
        let mut reading = false;
        match callee {
            Expr::Name(name) => {
                let id = name.id.as_str();
                match self.names.get(id) {
                    None if DYNAMIC_CALLS.contains(&id) => {
                        self.note(format!("the module calls {id}() while it is imported"));
                        match id {
                            "setattr" | "delattr" => {
                                if let Some(first) = call.arguments.args.first() {
                                    self.object(first, &format!("{id}()"));
                                }
                            }
                            "exec" | "eval" | "__import__" => {
                                self.touches.push(Touch::Unknown(format!(
                                    "the module calls {id}() while it is imported"
                                )));
                            }
                            _ => {}
                        }
                    }
                    None => reading = READING_BUILTINS.contains(&id),
                    Some(Binding::Function(info)) => {
                        if let Some(effect) = &info.side_effects {
                            let reason =
                                format!("{id}(), called while the module is imported, {effect}");
                            if info.foreign {
                                self.touches.push(Touch::Unknown(reason.clone()));
                            }
                            self.note(reason);
                        }
                    }
                    Some(Binding::Module(path) | Binding::Import(path)) => {
                        reading = path.starts_with("unittest.") || path.starts_with("typing.");
                        if !reading {
                            self.touches.push(Touch::Call(path.clone()));
                        }
                    }
                    _ => {}
                }
            }
            Expr::Attribute(attr) => {
                let head = dotted(callee).and_then(|d| d.split('.').next().map(str::to_string));
                match head.as_deref().and_then(|h| self.names.get(h)) {
                    Some(Binding::Class(_)) => {
                        self.note(format!(
                            "{} is called while the module is imported",
                            dotted(callee).unwrap_or_default()
                        ));
                    }
                    Some(Binding::Module(path) | Binding::Import(path)) => {
                        reading =
                            path == "unittest" || path.starts_with("unittest.") || path == "typing";
                        if !reading {
                            // A method of another module's object (a
                            // classmethod adding tests), or its function.
                            if let Some(path) = imported_path(self.names, callee) {
                                self.touches.push(Touch::Call(path));
                            }
                            self.object(&attr.value, "a method call");
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if reading {
            return;
        }
        let given = call
            .arguments
            .args
            .iter()
            .chain(call.arguments.keywords.iter().map(|k| &k.value));
        for arg in given {
            let arg = match arg {
                Expr::Starred(starred) => &*starred.value,
                other => other,
            };
            if let Expr::Name(name) = arg
                && matches!(self.names.get(name.id.as_str()), Some(Binding::Class(_)))
            {
                self.note(format!(
                    "the class {} is passed to a call while the module is imported",
                    name.id
                ));
            }
            // Another module's object given to a call may be changed by it.
            if let Some(path) = imported_path(self.names, arg) {
                self.touches.push(Touch::Object(path));
            }
        }
    }

    /// An object a call may change (`setattr`'s first argument, a method's
    /// receiver), when it is another module's.
    fn object(&mut self, expr: &Expr, how: &str) {
        if let Some(path) = imported_path(self.names, expr) {
            self.touches.push(Touch::Object(path));
        } else if dotted(expr).is_none() && how.ends_with("()") {
            self.touches.push(Touch::Unknown(format!(
                "{how} changes an object aye can't name while the module is imported"
            )));
        }
    }
}

fn function_defaults(def: &StmtFunctionDef) -> impl Iterator<Item = &Expr> {
    let params = &def.parameters;
    params
        .posonlyargs
        .iter()
        .chain(&params.args)
        .chain(&params.kwonlyargs)
        .filter_map(|p| p.default.as_deref())
}

/// Read a module-level function's body: what calling it may do and return.
fn function_info(def: &StmtFunctionDef) -> FunctionInfo {
    struct Reader<'a> {
        locals: HashSet<&'a str>,
        params: HashSet<&'a str>,
        may_return_class: bool,
        side_effects: Option<String>,
        changes_argument: bool,
        foreign: bool,
        returns: Vec<Option<&'a Expr>>,
        yields: bool,
    }
    impl<'a> Visitor<'a> for Reader<'a> {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                // Nested functions don't run unless called; a nested class
                // may be returned.
                Stmt::ClassDef(_) => {
                    self.may_return_class = true;
                    return;
                }
                Stmt::FunctionDef(_) => return,
                Stmt::Return(ret) => self.returns.push(ret.value.as_deref()),
                Stmt::Global(_) | Stmt::Nonlocal(_) => {
                    self.side_effects
                        .get_or_insert("rebinds names of the module".into());
                }
                Stmt::Assign(assign) => {
                    for target in &assign.targets {
                        self.target(target);
                    }
                }
                Stmt::AnnAssign(assign) => self.target(&assign.target),
                Stmt::AugAssign(assign) => self.target(&assign.target),
                Stmt::Delete(delete) => {
                    for target in &delete.targets {
                        self.target(target);
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }
        fn visit_expr(&mut self, expr: &'a Expr) {
            match expr {
                Expr::Lambda(_) => return,
                Expr::Yield(_) | Expr::YieldFrom(_) => self.yields = true,
                Expr::Call(call) => {
                    if let Expr::Name(name) = &*call.func {
                        let id = name.id.as_str();
                        if DYNAMIC_CALLS.contains(&id) && !self.locals.contains(id) {
                            self.side_effects.get_or_insert(format!("calls {id}()"));
                            if matches!(id, "setattr" | "delattr" | "exec" | "eval" | "__import__")
                            {
                                self.foreign = true;
                            }
                        }
                        if id == "type" && call.arguments.args.len() == 3 {
                            self.may_return_class = true;
                        }
                    }
                    if let Expr::Attribute(attr) = &*call.func
                        && attr.attr.id.as_str() == "new_class"
                    {
                        self.may_return_class = true;
                    }
                }
                Expr::Named(named) => {
                    if let Expr::Name(name) = &*named.target {
                        self.locals.insert(name.id.as_str());
                    }
                }
                _ => {}
            }
            visitor::walk_expr(self, expr);
        }
    }
    impl<'a> Reader<'a> {
        fn target(&mut self, target: &'a Expr) {
            let mut root = target;
            loop {
                match root {
                    Expr::Attribute(attr) => root = &attr.value,
                    Expr::Subscript(sub) => root = &sub.value,
                    _ => break,
                }
            }
            match (target, root) {
                (Expr::Name(name), _) => {
                    self.locals.insert(name.id.as_str());
                }
                (Expr::Tuple(tuple), _) => {
                    for elt in &tuple.elts {
                        self.target(elt);
                    }
                }
                (Expr::List(list), _) => {
                    for elt in &list.elts {
                        self.target(elt);
                    }
                }
                (Expr::Starred(starred), _) => self.target(&starred.value),
                (_, Expr::Name(name)) if self.locals.contains(name.id.as_str()) => {}
                (_, Expr::Name(name)) if self.params.contains(name.id.as_str()) => {
                    self.changes_argument = true;
                }
                _ => {
                    self.side_effects
                        .get_or_insert("changes attributes of objects it doesn't make".into());
                    self.foreign = true;
                }
            }
        }
    }

    let params: Vec<&str> = def
        .parameters
        .posonlyargs
        .iter()
        .chain(&def.parameters.args)
        .map(|p| p.parameter.name.id.as_str())
        .collect();
    let mut reader = Reader {
        locals: HashSet::new(),
        params: params.iter().copied().collect(),
        may_return_class: false,
        side_effects: None,
        changes_argument: false,
        foreign: false,
        returns: Vec::new(),
        yields: false,
    };
    // Assignments to locals made before an attribute assignment count:
    // read the body once to collect them, then again for effects.
    collect_locals(&def.body, &mut reader.locals);
    for stmt in &def.body {
        reader.visit_stmt(stmt);
    }
    let first = params.first().copied();
    // Nested functions wrapping the first argument with `functools.wraps`.
    let wrappers: HashSet<&str> = def
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::FunctionDef(inner)
                if first.is_some()
                    && inner
                        .decorator_list
                        .iter()
                        .any(|d| wraps(&d.expression, first.unwrap_or_default())) =>
            {
                Some(inner.name.id.as_str())
            }
            _ => None,
        })
        .collect();
    let returns_value = |value: Option<&Expr>| value.is_none_or(literal_like);
    let mut may_return_class = reader.may_return_class;
    if !reader.yields && reader.returns.iter().any(|value| !returns_value(*value)) {
        may_return_class = true;
    }
    let keeps = first.is_some()
        && !reader.returns.is_empty()
        && reader.side_effects.is_none()
        && reader.returns.iter().all(|value| {
            matches!(value, Some(Expr::Name(name))
                if Some(name.id.as_str()) == first || wrappers.contains(name.id.as_str()))
        });
    FunctionInfo {
        may_return_class,
        side_effects: reader.side_effects,
        changes_argument: reader.changes_argument,
        keeps,
        foreign: reader.foreign,
    }
}

/// `functools.wraps(name)` or `wraps(name)`.
fn wraps(expr: &Expr, name: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    let named = match &*call.func {
        Expr::Name(n) => n.id.as_str() == "wraps",
        Expr::Attribute(attr) => attr.attr.id.as_str() == "wraps",
        _ => false,
    };
    named && matches!(call.arguments.args.first(), Some(Expr::Name(arg)) if arg.id.as_str() == name)
}

fn collect_locals<'a>(body: &'a [Stmt], out: &mut HashSet<&'a str>) {
    for stmt in body {
        match stmt {
            Stmt::Assign(assign) => {
                for target in &assign.targets {
                    if let Expr::Name(name) = target {
                        out.insert(name.id.as_str());
                    }
                }
            }
            Stmt::AnnAssign(assign) => {
                if let Expr::Name(name) = &*assign.target {
                    out.insert(name.id.as_str());
                }
            }
            Stmt::For(s) => {
                if let Expr::Name(name) = &*s.target {
                    out.insert(name.id.as_str());
                }
                collect_locals(&s.body, out);
            }
            Stmt::With(s) => {
                for item in &s.items {
                    if let Some(Expr::Name(name)) = item.optional_vars.as_deref() {
                        out.insert(name.id.as_str());
                    }
                }
                collect_locals(&s.body, out);
            }
            Stmt::If(s) => {
                collect_locals(&s.body, out);
                for clause in &s.elif_else_clauses {
                    collect_locals(&clause.body, out);
                }
            }
            Stmt::Try(s) => {
                collect_locals(&s.body, out);
                collect_locals(&s.orelse, out);
                collect_locals(&s.finalbody, out);
            }
            Stmt::While(s) => collect_locals(&s.body, out),
            _ => {}
        }
    }
}

fn pattern_captures(pattern: &ast::Pattern, out: &mut Vec<String>) {
    use ast::Pattern;
    match pattern {
        Pattern::MatchAs(p) => {
            if let Some(name) = &p.name {
                out.push(name.id.as_str().to_string());
            }
            if let Some(inner) = &p.pattern {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchStar(p) => {
            if let Some(name) = &p.name {
                out.push(name.id.as_str().to_string());
            }
        }
        Pattern::MatchMapping(p) => {
            if let Some(rest) = &p.rest {
                out.push(rest.id.as_str().to_string());
            }
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchSequence(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchClass(p) => {
            for inner in &p.arguments.patterns {
                pattern_captures(inner, out);
            }
            for keyword in &p.arguments.keywords {
                pattern_captures(&keyword.pattern, out);
            }
        }
        Pattern::MatchOr(p) => {
            for inner in &p.patterns {
                pattern_captures(inner, out);
            }
        }
        Pattern::MatchValue(_) | Pattern::MatchSingleton(_) => {}
    }
}

// ---------------------------------------------------------------- the project

/// Where an imported dotted path leads.
#[derive(Debug, Clone)]
pub enum Target {
    Class(Arc<Summary>, usize),
    /// Another binding of a project module.
    Binding(Arc<Summary>, Binding),
    Module,
    /// The standard library.
    Stdlib(String),
    /// An installed package aye doesn't read.
    External(String),
    Unknown(String),
}

/// The project's own modules, read on demand: what imports lead to.
#[derive(Debug)]
pub struct Modules {
    root: PathBuf,
    /// Where absolute imports are looked up: the top-level directory, the
    /// project root and its `src`.
    search: Vec<PathBuf>,
    cache: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// By module file: what importing it may change elsewhere, resolved.
    effects: Mutex<HashMap<PathBuf, Arc<Vec<Effect>>>>,
    /// By module file: the project files importing it imports (its
    /// imports' modules and their packages' `__init__.py`).
    edges: Mutex<HashMap<PathBuf, Arc<Vec<PathBuf>>>>,
}

/// A [`Touch`], resolved where it was made.
#[derive(Debug, Clone)]
enum Effect {
    /// A project class may change.
    Class(PathBuf, usize, String),
    /// Something aye can't resolve may change: named so (its last name),
    /// or not at all.
    Unknown(Option<String>, String),
}

/// Modules followed from a test module before refinement takes over.
const MAX_MODULES: usize = 4000;

/// What [`Modules::module_file`] finds.
enum Found {
    File(PathBuf),
    Namespace,
}

/// Imports followed this deep are left to refinement.
const MAX_DEPTH: usize = 16;

impl Modules {
    pub fn new(top: &Path, root: &Path) -> Self {
        let mut search = Vec::new();
        for dir in [top.to_path_buf(), root.to_path_buf(), root.join("src")] {
            if !search.contains(&dir) {
                search.push(dir);
            }
        }
        Self {
            root: root.to_path_buf(),
            search,
            cache: Mutex::new(HashMap::new()),
            effects: Mutex::new(HashMap::new()),
            edges: Mutex::new(HashMap::new()),
        }
    }

    /// A project file, read and summarized once.
    pub fn summary(&self, path: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.cache.lock().expect("lock").get(path) {
            return found.clone();
        }
        let summary = std::fs::read_to_string(path).ok().and_then(|source| {
            let parsed = ruff_python_parser::parse_module(&source).ok()?;
            let lines = LineIndex::from_source_text(&source);
            Some(Arc::new(summarize(path, parsed.syntax(), &lines)))
        });
        self.cache
            .lock()
            .expect("lock")
            .insert(path.to_path_buf(), summary.clone());
        summary
    }

    /// The file of module `parts` under `base`: `a/b.py` or
    /// `a/b/__init__.py`, or a directory without one (a namespace package).
    fn module_file(base: &Path, parts: &[&str]) -> Option<Found> {
        let mut dir = base.to_path_buf();
        for part in parts {
            dir.push(part);
        }
        if parts.is_empty() {
            let init = dir.join("__init__.py");
            return init.is_file().then_some(Found::File(init));
        }
        let file = dir.with_extension("py");
        if file.is_file() {
            return Some(Found::File(file));
        }
        let init = dir.join("__init__.py");
        if init.is_file() {
            return Some(Found::File(init));
        }
        dir.is_dir().then_some(Found::Namespace)
    }

    /// Where `path` (as imported in `from`) leads.
    pub fn target(&self, path: &str, from: &Path, depth: usize) -> Target {
        if depth > MAX_DEPTH {
            return Target::Unknown(format!("{path} is imported through too many modules"));
        }
        let dots = path.chars().take_while(|c| *c == '.').count();
        let rest = &path[dots..];
        let parts: Vec<&str> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('.').collect()
        };
        let bases: Vec<PathBuf> = if dots == 0 {
            if is_stdlib(path) {
                return Target::Stdlib(path.to_string());
            }
            self.search.clone()
        } else {
            let mut base = from.parent().unwrap_or(from).to_path_buf();
            for _ in 1..dots {
                base = base.parent().unwrap_or(&base).to_path_buf();
            }
            vec![base]
        };
        let lowest = usize::from(dots == 0);
        for split in (lowest..=parts.len()).rev() {
            for base in &bases {
                let found = Self::module_file(base, &parts[..split]);
                if found.is_some() && split == parts.len() {
                    return Target::Module;
                }
                // A namespace package holds only submodules, and the longer
                // paths found none: a directory of the same name, not this
                // import.
                let Some(Found::File(file)) = found else {
                    continue;
                };
                let Some(summary) = self.summary(&file) else {
                    return Target::Unknown(format!("{} can't be read", file.display()));
                };
                return self.member(&summary, &parts[split..], depth);
            }
        }
        if dots > 0 {
            return Target::Unknown(format!("{path} is not a module aye can find"));
        }
        // Found beside the importing module (or above it), where only a
        // `sys.path` change makes Python look: maybe the project's.
        let beside = from
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.root))
            .filter(|dir| !self.search.iter().any(|searched| searched == dir))
            .find(|dir| matches!(Self::module_file(dir, &parts[..1]), Some(Found::File(_))));
        if let Some(dir) = beside {
            return Target::Unknown(format!(
                "{path} may be imported from {}, through a sys.path change",
                dir.display()
            ));
        }
        Target::External(path.to_string())
    }

    /// The project file of the module `path` (as imported in `from`):
    /// `a/b.py` or `a/b/__init__.py`; `None` for a name that is no module,
    /// the standard library and installed packages.
    fn module_path(&self, path: &str, from: &Path) -> Option<PathBuf> {
        let dots = path.chars().take_while(|c| *c == '.').count();
        let rest = &path[dots..];
        let parts: Vec<&str> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('.').collect()
        };
        let bases: Vec<PathBuf> = if dots == 0 {
            if is_stdlib(path) {
                return None;
            }
            self.search.clone()
        } else {
            let mut base = from.parent().unwrap_or(from).to_path_buf();
            for _ in 1..dots {
                base = base.parent().unwrap_or(&base).to_path_buf();
            }
            vec![base]
        };
        bases
            .iter()
            .find_map(|base| match Self::module_file(base, &parts) {
                Some(Found::File(file)) => Some(file),
                _ => None,
            })
    }

    /// The `__init__.py` of each package holding `file`, nearest first,
    /// within the project: importing the file runs them.
    fn packages_of(&self, file: &Path) -> Vec<PathBuf> {
        file.ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.root))
            .map(|dir| dir.join("__init__.py"))
            .take_while(|init| init.is_file())
            .filter(|init| init != file)
            .collect()
    }

    /// See [`Modules::edges`].
    fn edges_of(&self, summary: &Summary) -> Arc<Vec<PathBuf>> {
        if let Some(found) = self.edges.lock().expect("lock").get(&summary.path) {
            return Arc::clone(found);
        }
        let mut out: Vec<PathBuf> = Vec::new();
        for import in &summary.imports {
            if let Some(file) = self.module_path(import, &summary.path) {
                out.extend(self.packages_of(&file));
                out.push(file);
            }
        }
        out.dedup();
        let out = Arc::new(out);
        self.edges
            .lock()
            .expect("lock")
            .insert(summary.path.clone(), Arc::clone(&out));
        out
    }

    /// See [`Modules::effects`].
    fn effects_of(&self, summary: &Arc<Summary>) -> Arc<Vec<Effect>> {
        if let Some(found) = self.effects.lock().expect("lock").get(&summary.path) {
            return Arc::clone(found);
        }
        let mut out = Vec::new();
        let shown = |what: &str, path: &str| {
            format!(
                "{} {what} {path} while it is imported",
                summary.path.display()
            )
        };
        for touch in &summary.touches {
            let (path, how) = match touch {
                Touch::Object(path) => (path, "may change"),
                Touch::Bound(path) => (path, "rebinds"),
                Touch::Call(path) => (path, "calls"),
                Touch::Unknown(reason) => {
                    out.push(Effect::Unknown(None, reason.clone()));
                    continue;
                }
            };
            let reason = shown(how, path);
            // unittest itself: its loader decides what a test is
            // (`TestLoader.testMethodPrefix = …`); `TestCase.maxDiff` and
            // the like change none.
            if path == "unittest" || path.starts_with("unittest.") {
                let loader = path
                    .split('.')
                    .any(|part| part.ends_with("Loader") || part == "loader");
                if loader && !matches!(touch, Touch::Call(_)) {
                    out.push(Effect::Unknown(None, reason));
                }
                continue;
            }
            let last = path.rsplit('.').next().unwrap_or(path).to_string();
            match (touch, self.target(path, &summary.path, 0)) {
                (Touch::Call(_), Target::Binding(_, Binding::Function(info))) => {
                    if info.foreign {
                        out.push(Effect::Unknown(None, reason));
                    }
                }
                (Touch::Call(_), _) => {}
                (_, Target::Class(owner, index)) => {
                    out.push(Effect::Class(owner.path.clone(), index, reason));
                }
                (_, Target::Unknown(why)) => {
                    out.push(Effect::Unknown(Some(last), format!("{reason}: {why}")));
                }
                _ => {}
            }
        }
        let out = Arc::new(out);
        self.effects
            .lock()
            .expect("lock")
            .insert(summary.path.clone(), Arc::clone(&out));
        out
    }

    /// Why importing the test module `summary` may give the classes it
    /// loads (`loaded`) other tests than their source says: a module that
    /// runs before the loader reads them (the packages holding it, which
    /// discovery imports first; it; the project modules it imports, and
    /// theirs) changes one of those classes or its bases (`base.Base.test_x
    /// = f`, `setattr`, a classmethod called on it, the class passed to a
    /// call), or something aye can't follow. `None` when nothing may.
    pub fn effects(&self, summary: &Arc<Summary>, loaded: &[Loaded]) -> Option<String> {
        if loaded.is_empty() {
            return None;
        }
        let classes: HashSet<(&Path, usize)> = loaded
            .iter()
            .flat_map(|l| l.mro.iter().map(|(path, index)| (path.as_path(), *index)))
            .collect();
        // The names those classes go by, for what aye can't resolve.
        let mut names: HashSet<String> = loaded.iter().map(|l| l.name.clone()).collect();
        for (path, index) in &classes {
            if let Some(owner) = self.summary(path)
                && let Some(class) = owner.classes.get(*index)
            {
                names.insert(class.name.clone());
            }
        }
        let mut queue: Vec<PathBuf> = self.packages_of(&summary.path);
        queue.push(summary.path.clone());
        let mut seen: HashSet<PathBuf> = HashSet::new();
        while let Some(file) = queue.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            if seen.len() > MAX_MODULES {
                return Some(format!(
                    "{} imports more than {MAX_MODULES} project modules, which may change its \
                     classes",
                    summary.path.display()
                ));
            }
            let Some(module) = (if file == summary.path {
                Some(Arc::clone(summary))
            } else {
                self.summary(&file)
            }) else {
                continue;
            };
            for effect in self.effects_of(&module).iter() {
                match effect {
                    Effect::Class(path, index, reason)
                        if classes.contains(&(path.as_path(), *index)) =>
                    {
                        return Some(reason.clone());
                    }
                    Effect::Unknown(name, reason)
                        if name.as_ref().is_none_or(|name| names.contains(name)) =>
                    {
                        return Some(reason.clone());
                    }
                    _ => {}
                }
            }
            queue.extend(self.edges_of(&module).iter().cloned());
        }
        None
    }

    fn member(&self, summary: &Arc<Summary>, names: &[&str], depth: usize) -> Target {
        let shown = || format!("{} in {}", names.join("."), summary.path.display());
        if let Some(reason) = &summary.dynamic {
            return Target::Unknown(format!("{}: {reason}", shown()));
        }
        let Some(binding) = summary.names.get(names[0]) else {
            return Target::Unknown(format!("{} is not bound statically", shown()));
        };
        let rest = &names[1..];
        let join = |base: &str| {
            if rest.is_empty() {
                base.to_string()
            } else {
                format!("{base}.{}", rest.join("."))
            }
        };
        match binding {
            Binding::Import(path) | Binding::Module(path) => {
                self.target(&join(path), &summary.path, depth + 1)
            }
            Binding::Class(index) if rest.is_empty() => Target::Class(Arc::clone(summary), *index),
            other if rest.is_empty() => Target::Binding(Arc::clone(summary), other.clone()),
            _ => Target::Unknown(format!("{} is an attribute aye can't follow", shown())),
        }
    }
}

// ---------------------------------------------------------------- verdicts

/// A test of a class: its name and where its method is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub name: String,
    pub file: PathBuf,
    pub range: TextRange,
    pub line: u32,
}

/// One class's tests, as the loader takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    /// The class's `__qualname__`.
    pub class: String,
    /// The name the loader found it under in the module's namespace (an
    /// alias's, or an imported class's local name): the test ids' class.
    pub name: String,
    pub methods: Vec<Method>,
    /// The project classes of its MRO, by file and index.
    pub mro: Vec<(PathBuf, usize)>,
}

#[derive(Clone)]
enum Node {
    Class(Arc<Summary>, usize),
    TestCase,
    Object,
    Plain(String),
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Class(a, i), Self::Class(b, j)) => Arc::ptr_eq(a, b) && i == j,
            (Self::TestCase, Self::TestCase) | (Self::Object, Self::Object) => true,
            (Self::Plain(a), Self::Plain(b)) => a == b,
            _ => false,
        }
    }
}

impl Modules {
    /// The tests `loadTestsFromModule` takes from `summary` (the module's
    /// own file), in its order: by name in the namespace, then by method
    /// name; or why only an import can tell.
    pub fn loaded(&self, summary: &Arc<Summary>) -> Result<Vec<Loaded>, String> {
        if let Some(reason) = &summary.dynamic {
            return Err(reason.clone());
        }
        let mut out = Vec::new();
        for (name, binding) in &summary.names {
            if let Some(mut loaded) = self.binding_tests(summary, name, binding)? {
                loaded.name.clone_from(name);
                out.push(loaded);
            }
        }
        Ok(out)
    }

    fn binding_tests(
        &self,
        summary: &Arc<Summary>,
        name: &str,
        binding: &Binding,
    ) -> Result<Option<Loaded>, String> {
        match binding {
            Binding::Class(index) => self.class_tests(summary, *index, 0),
            Binding::Import(path) => match self.target(path, &summary.path, 0) {
                Target::Class(other, index) => self.class_tests(&other, index, 0),
                Target::Binding(other, binding) => self.binding_tests(&other, name, &binding),
                Target::External(path) if looks_like_tests(name, &path) => Err(format!(
                    "{name} is imported from {path}, which aye doesn't read, and may hold tests"
                )),
                Target::Module | Target::Stdlib(_) | Target::External(_) => Ok(None),
                Target::Unknown(reason) => Err(format!("{name} may be a test class: {reason}")),
            },
            Binding::Returned(path) => match self.target(path, &summary.path, 0) {
                Target::Binding(_, Binding::Function(info)) if !info.may_return_class => Ok(None),
                Target::Class(other, index) if !other.classes[index].metaclass => Ok(None),
                Target::Stdlib(p) if !p.ends_with("new_class") => Ok(None),
                Target::External(_) => Ok(None),
                _ => Err(format!(
                    "{name} is the result of {path}(), which may be a class"
                )),
            },
            Binding::Unknown(reason) => {
                if is_dunder(name) {
                    Ok(None)
                } else {
                    Err(format!("{name} may be a test class: {reason}"))
                }
            }
            Binding::Maybe(options) => {
                for option in options {
                    match self.binding_tests(summary, name, option) {
                        Ok(None) => {}
                        Ok(Some(_)) | Err(_) => {
                            return Err(format!("{name} is bound in a branch that may not run"));
                        }
                    }
                }
                Ok(None)
            }
            Binding::Function(_)
            | Binding::Module(_)
            | Binding::Constant
            | Binding::Plain
            | Binding::Unbound => Ok(None),
        }
    }

    /// A class's MRO, as C3 linearizes it; external classes count for
    /// themselves and `object`.
    fn mro(&self, summary: &Arc<Summary>, index: usize, depth: usize) -> Result<Vec<Node>, String> {
        if depth > MAX_DEPTH {
            return Err("classes nest too deep".into());
        }
        let class = &summary.classes[index];
        if let Some(reason) = &class.dynamic {
            return Err(reason.clone());
        }
        let bases = class.bases.clone()?;
        let mut sequences: Vec<Vec<Node>> = Vec::new();
        let mut direct = Vec::new();
        for base in &bases {
            let (head, sequence) = match base {
                Base::Local(j) => (
                    Node::Class(Arc::clone(summary), *j),
                    self.mro(summary, *j, depth + 1)?,
                ),
                Base::TestCase => (Node::TestCase, vec![Node::TestCase, Node::Object]),
                Base::Object => (Node::Object, vec![Node::Object]),
                Base::Plain(path) => (
                    Node::Plain(path.clone()),
                    vec![Node::Plain(path.clone()), Node::Object],
                ),
                Base::Path(path) => match self.target(path, &summary.path, depth + 1) {
                    Target::Class(other, j) => (
                        Node::Class(Arc::clone(&other), j),
                        self.mro(&other, j, depth + 1)?,
                    ),
                    Target::Stdlib(path) => (
                        Node::Plain(path.clone()),
                        vec![Node::Plain(path), Node::Object],
                    ),
                    Target::External(path) => {
                        return Err(format!(
                            "{} inherits from {path}, which aye doesn't read",
                            class.name
                        ));
                    }
                    Target::Module | Target::Binding(..) => {
                        return Err(format!(
                            "{} inherits from {path}, which is not a class",
                            class.name
                        ));
                    }
                    Target::Unknown(reason) => {
                        return Err(format!("{} inherits from {path}: {reason}", class.name));
                    }
                },
            };
            direct.push(head);
            sequences.push(sequence);
        }
        if direct.is_empty() {
            direct.push(Node::Object);
            sequences.push(vec![Node::Object]);
        }
        sequences.push(direct);
        let mut out = vec![Node::Class(Arc::clone(summary), index)];
        loop {
            sequences.retain(|s| !s.is_empty());
            if sequences.is_empty() {
                return Ok(out);
            }
            let candidate = sequences
                .iter()
                .map(|s| &s[0])
                .find(|head| !sequences.iter().any(|s| s[1..].contains(head)))
                .cloned();
            let Some(next) = candidate else {
                return Err(format!(
                    "{} has an inconsistent MRO; Python refuses to make it",
                    class.name
                ));
            };
            for sequence in &mut sequences {
                if sequence[0] == next {
                    sequence.remove(0);
                }
            }
            out.push(next);
        }
    }

    /// [`Modules::class_tests`] of a class of `summary`, for a static guess.
    pub fn class_tests_for_guess(
        &self,
        summary: &Arc<Summary>,
        index: usize,
    ) -> Result<Option<Loaded>, String> {
        self.class_tests(summary, index, 0)
    }

    /// The tests of a class, when it is a `TestCase`: callable `test*`
    /// attributes along its MRO (the first binding of a name wins), sorted;
    /// with none, `runTest` when it has one.
    fn class_tests(
        &self,
        summary: &Arc<Summary>,
        index: usize,
        depth: usize,
    ) -> Result<Option<Loaded>, String> {
        let mro = self.mro(summary, index, depth)?;
        if !mro.contains(&Node::TestCase) {
            return Ok(None);
        }
        let class = &summary.classes[index];
        let mut seen: HashSet<&str> = HashSet::new();
        let mut methods = Vec::new();
        let mut run_test = None;
        for (position, node) in mro.iter().enumerate() {
            let Node::Class(owner, j) = node else {
                continue;
            };
            let defining = &owner.classes[*j];
            if position > 0 && defining.init_subclass {
                return Err(format!(
                    "{} defines __init_subclass__, which may change {}",
                    defining.name, class.name
                ));
            }
            for (name, member) in &defining.members {
                if !seen.insert(name.as_str()) {
                    continue;
                }
                let is_test = name.starts_with(PREFIX);
                if !is_test && name != "runTest" {
                    continue;
                }
                let located = |range: TextRange, line: u32| Method {
                    name: name.clone(),
                    file: owner.path.clone(),
                    range,
                    line,
                };
                let found = match member {
                    Member::Callable { range, line } => Some(located(*range, *line)),
                    Member::NotCallable => None,
                    Member::Import(path) => match self.target(path, &owner.path, depth + 1) {
                        Target::Class(..) | Target::Binding(_, Binding::Function(_)) => {
                            Some(located(defining.range, defining.line))
                        }
                        Target::Binding(_, Binding::Constant) | Target::Module => None,
                        _ => {
                            return Err(format!(
                                "{}.{name} is imported, and may or may not be callable",
                                defining.name
                            ));
                        }
                    },
                    Member::Unknown(reason) => return Err(reason.clone()),
                };
                if name == "runTest" {
                    // `hasattr`: any binding counts.
                    run_test =
                        Some(found.unwrap_or_else(|| located(defining.range, defining.line)));
                } else if let Some(method) = found {
                    methods.push(method);
                }
            }
        }
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        if methods.is_empty()
            && let Some(run) = run_test
        {
            methods.push(run);
        }
        let mro = mro
            .iter()
            .filter_map(|node| match node {
                Node::Class(owner, j) => Some((owner.path.clone(), *j)),
                _ => None,
            })
            .collect();
        Ok(Some(Loaded {
            class: class.name.clone(),
            name: class.name.clone(),
            methods,
            mro,
        }))
    }
}

/// Whether a package's `__init__.py` may define `load_tests`, which then
/// decides every test below it: the word `load_tests`, or a way to bind
/// names the source doesn't spell (a star import, a module `__getattr__`,
/// `globals()`, `setattr`, …). The executor applies the same rule to the
/// same bytes (`LOAD_TESTS_MARKERS`), so both halves agree on which
/// packages take over their files.
pub fn may_define_load_tests(source: &[u8]) -> bool {
    const MARKERS: &[&[u8]] = &[
        b"load_tests",
        b"__getattr__",
        b"__dir__",
        b"__dict__",
        b"__class__",
        b"globals",
        b"locals",
        b"vars",
        b"exec",
        b"setattr",
    ];
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut at = 0;
    while at < source.len() {
        if !word(source[at]) {
            at += 1;
            continue;
        }
        let start = at;
        while at < source.len() && word(source[at]) {
            at += 1;
        }
        let identifier = &source[start..at];
        if MARKERS.contains(&identifier) {
            return true;
        }
        if identifier == b"import" {
            let mut next = at;
            while next < source.len()
                && (source[next].is_ascii_whitespace()
                    || source[next] == b'\\'
                    || source[next] == 0x0b)
            {
                next += 1;
            }
            if source.get(next) == Some(&b'*') {
                return true;
            }
        }
    }
    false
}
