//! The names the finder knows: the decorators that keep what they
//! decorate, the metaclasses that add nothing, the builtins that change a
//! namespace or make plain values, pytest's functions that end an import
//! or skip a conftest's directory, and the names whose binding changes
//! what the finder sees.

/// Decorators that return the function they are given, with its docstring:
/// anywhere, and on methods only.
pub(super) const SAME_FUNCTION: [&str; 8] = [
    "abc.abstractmethod",
    "typing.final",
    "typing.override",
    "typing_extensions.final",
    "typing_extensions.override",
    "typing.no_type_check",
    "builtins.staticmethod",
    "builtins.classmethod",
];
pub(super) const METHOD_ONLY: [&str; 3] = [
    "builtins.staticmethod",
    "builtins.classmethod",
    "builtins.property",
];

/// Class decorators that keep the class's members and docstrings (a
/// dataclass with `slots=True` is a new class with the same `__dict__`).
pub(super) const SAME_CLASS: [&str; 8] = [
    "dataclasses.dataclass",
    "functools.total_ordering",
    "typing.final",
    "typing_extensions.final",
    "enum.unique",
    "typing.runtime_checkable",
    "typing_extensions.runtime_checkable",
    "typing.type_check_only",
];

/// Metaclasses that add nothing doctest finds.
pub(super) const PLAIN_METACLASSES: [&str; 4] = [
    "builtins.type",
    "abc.ABCMeta",
    "enum.EnumMeta",
    "enum.EnumType",
];

/// Builtins that change a namespace by name, or hand it out.
pub(super) const NAMESPACE_CHANGERS: [&str; 7] = [
    "setattr", "delattr", "globals", "locals", "vars", "exec", "eval",
];

/// pytest's functions that end a module's import: a call to one of these
/// (or to a method of that name, as `pytest.skip`) at import may skip the
/// module or fail its collection, wherever it is.
pub(super) const IMPORT_ENDERS: [&str; 5] = ["skip", "importorskip", "xfail", "fail", "exit"];

/// Names that, bound in a namespace, change what the finder sees.
pub(super) const SPECIAL_MODULE_NAMES: [&str; 3] = ["__doc__", "__name__", "__test__"];
pub(super) const SPECIAL_CLASS_NAMES: [&str; 2] = ["__doc__", "__module__"];

/// Builtins whose result is never a routine, a class or a property,
/// whatever they are given.
pub(super) const INERT_BUILTINS: [&str; 37] = [
    "builtins.abs",
    "builtins.all",
    "builtins.any",
    "builtins.ascii",
    "builtins.bin",
    "builtins.bool",
    "builtins.bytearray",
    "builtins.bytes",
    "builtins.callable",
    "builtins.chr",
    "builtins.complex",
    "builtins.dict",
    "builtins.dir",
    "builtins.divmod",
    "builtins.enumerate",
    "builtins.filter",
    "builtins.float",
    "builtins.format",
    "builtins.frozenset",
    "builtins.hasattr",
    "builtins.hash",
    "builtins.hex",
    "builtins.id",
    "builtins.int",
    "builtins.isinstance",
    "builtins.issubclass",
    "builtins.iter",
    "builtins.len",
    "builtins.list",
    "builtins.map",
    "builtins.print",
    "builtins.range",
    "builtins.repr",
    "builtins.set",
    "builtins.sorted",
    "builtins.str",
    "builtins.tuple",
];

/// pytest's functions that skip what it is collecting when a conftest calls
/// them while loading: `skip` and `importorskip` (pytest's, or a method of
/// that name).
pub(super) const SKIPPERS: [&str; 2] = ["skip", "importorskip"];
