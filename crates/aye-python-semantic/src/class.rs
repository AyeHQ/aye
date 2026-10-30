//! Classes as Python builds them: the bases a class names, the kinds of
//! known classes from elsewhere, and names as a class body stores them.

use aye_python_stdlib::unittest::{KNOWN_BASES, RUNTEST_BASES, TESTCASE_ROOTS};
use ruff_python_ast::StmtClassDef;
use ruff_text_size::Ranged;

/// Why a class's MRO is not known, and whether only classes that hold no
/// tests to inherit, and are no unittest cases, are missing from it (as
/// the caller judges them).
#[derive(Debug, Clone)]
pub struct Unresolved {
    pub reason: String,
    pub harmless: bool,
}

/// A class in a method resolution order.
#[derive(Debug, Clone)]
pub enum Klass<'a> {
    Local(&'a StmtClassDef),
    /// A known class from elsewhere, by dotted path.
    External(String),
}

impl Klass<'_> {
    pub fn key(&self) -> String {
        match self {
            Self::Local(def) => format!("local:{}", def.start().to_u32()),
            Self::External(path) => path.clone(),
        }
    }

    pub fn object() -> Self {
        Self::External("builtins.object".to_string())
    }
}

/// Two classes are the same when their keys are: a class of the module by
/// where its statement starts, one from elsewhere by its path.
impl PartialEq for Klass<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

/// What an external base class brings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum External {
    Object,
    Abc,
    Plain,
    /// `typing.Protocol`: its metaclass derives from `ABCMeta`, and
    /// `Protocol.__init_subclass__` gives protocol classes an `__init__`
    /// that their subclasses inherit.
    Protocol,
    UnitTest,
    /// Defines `__init__` and `__new__` (exceptions).
    InitNew,
    /// Defines `__new__` (enums, named tuples).
    New,
}

pub fn external_kind(path: &str) -> Option<External> {
    const PLAIN: &[&str] = &["typing.Generic", "typing_extensions.Generic"];
    const PROTOCOL: &[&str] = &["typing.Protocol", "typing_extensions.Protocol"];
    const INIT_NEW: &[&str] = &[
        "enum.Enum",
        "enum.IntEnum",
        "enum.StrEnum",
        "enum.Flag",
        "enum.IntFlag",
        "builtins.list",
        "builtins.dict",
        "builtins.set",
        "builtins.frozenset",
        "builtins.bytearray",
    ];
    const NEW: &[&str] = &[
        "typing.NamedTuple",
        "builtins.int",
        "builtins.str",
        "builtins.tuple",
        "builtins.float",
        "builtins.bytes",
    ];
    const EXCEPTIONS: &[&str] = &[
        "BaseException",
        "BaseExceptionGroup",
        "ExceptionGroup",
        "KeyboardInterrupt",
        "SystemExit",
        "GeneratorExit",
        "StopIteration",
        "StopAsyncIteration",
    ];
    let builtin = path.strip_prefix("builtins.");
    Some(match path {
        "builtins.object" => External::Object,
        "abc.ABC" => External::Abc,
        _ if PLAIN.contains(&path) => External::Plain,
        _ if PROTOCOL.contains(&path) => External::Protocol,
        _ if is_unittest_base(path) => External::UnitTest,
        _ if INIT_NEW.contains(&path) => External::InitNew,
        _ if NEW.contains(&path) => External::New,
        _ if builtin.is_some_and(|name| {
            name.ends_with("Error")
                || name.ends_with("Exception")
                || name.ends_with("Warning")
                || EXCEPTIONS.contains(&name)
        }) =>
        {
            External::InitNew
        }
        _ => return None,
    })
}

/// Whether `path` names one of the unittest bases aye knows without
/// reading them: `TestCase` and its standard library subclass,
/// `FunctionTestCase`, and the [`KNOWN_BASES`] of installed packages.
pub fn is_unittest_base(path: &str) -> bool {
    TESTCASE_ROOTS.contains(&path) || KNOWN_BASES.contains(&path) || RUNTEST_BASES.contains(&path)
}

/// A name bound in the body of the class `class` as Python stores it:
/// `__x` (not a dunder) becomes `_K__x`, `K` being the class's name
/// without its leading underscores (unless that leaves nothing).
pub fn mangle(name: &str, class: &str) -> String {
    let stripped = class.trim_start_matches('_');
    if name.starts_with("__") && !name.ends_with("__") && !stripped.is_empty() {
        format!("_{stripped}{name}")
    } else {
        name.to_string()
    }
}
