"""aye's assertion rewriting: ``assert`` statements that explain their
failures, as pytest's do, without pytest.

A service of aye's engine. It knows no framework: a front-end decides which
modules are rewritten (``install`` takes a predicate on their paths) and the
engine reads what a failure carries. It never imports the front-end, the
glue or ``pytest``.

- ``rewrite_source(source, filename)``: the module's code, its asserts
  rewritten.
- ``install(should_rewrite)``: a ``sys.meta_path`` finder, first, that
  imports the modules whose file ``should_rewrite(abs_path)`` accepts (or
  named in ``modules``) with their asserts rewritten. Idempotent: a second
  call replaces the predicate and moves the finder back to the front.
  ``RewritingLoader`` is the loader, for front-ends that import by spec.

A rewritten ``assert test, msg`` evaluates every subexpression once, in
Python's order (``and``/``or`` and chained comparisons short-circuit; a walrus
binds once), into temporaries. When the test is false it raises
``AssertionError(message)`` with pytest's message: ``assert 1 == 2``,
``where`` lines for calls and attributes (``+  where 3 = f(2)``), and the
user's message first when given (``msg\\nassert …``). Values appear through
``saferepr``, cut to ``MAX_REPR`` characters. When the test is a comparison
(``==, !=, <, <=, >, >=, in, not in, is, is not``; for a chain, the link that
failed), or an ``and`` decided by one, the exception also carries
``exc._aye_assert = {"op": "==", "left": <value>, "right": <value>, "expr":
"<source of the assert's test>", "left_src": …, "right_src": …,
"left_cols": …, "right_cols": …, "expected": …}``, from which the engine
builds its structured failure: each operand's source text (cut to
``MAX_SOURCE`` characters), its span on the assert's first line (``(start,
end)`` in characters, the end excluded, or ``None`` when it starts on a later
line; one that goes on past that line ends at the line's end), and which
operand is the expected value: ``"left"`` or ``"right"`` when exactly one is
written as a literal (``is_literal``), ``"right"`` when both are (``assert
actual == expected`` is the usual order), else ``None``. Every failure also
carries ``exc._aye_span``, the test's own span on the assert's first line (or
``None``), so the engine can point at it. Passing asserts behave as Python's: same values, same
calls (``__bool__`` too: each truth is tested as often as Python tests it),
same exceptions; the temporaries are deleted afterwards.

The message is built only on failure, by ``_fail``, from a plan of the
assert's test (a constant: which temporary holds what, and how to show it)
and the temporaries' values, so that a rewritten module's code stays small.

As pytest's rewriting, and unlike Python's ``assert``: rewritten asserts run
under ``python -O``, the message is pytest's text rather than ``msg``, a
module whose docstring says ``PYTEST_DONT_REWRITE`` is left alone, and
``assert (x, "msg")`` warns at rewrite time ("assertion is always true,
perhaps remove parentheses?", ``SyntaxWarning`` unless told otherwise: a
pytest front-end passes ``PytestAssertRewriteWarning``). Unlike pytest's, a
failing chain doesn't evaluate the operands after the failing link, a
walrus's value is never evaluated again for the message, and no truth is
tested twice.

``explain_comparison``, when set, is pytest's ``pytest_assertrepr_compare``:
called with the operator and operands of a comparison a message shows, the
lines it returns replace ``left op right``.

Python's own bytecode cache never holds a rewritten module (its ``.pyc`` is
the code as Python compiles it). The loader caches the rewritten code apart,
as pytest does (``cached_rewrite``): in ``__pycache__`` (or under
``sys.pycache_prefix``), as ``<name>.<cache tag>-aye.pyc``, keyed by a digest
of the source's bytes, its path, Python's bytecode version and optimization
level, and this module's own source, so that it answers only for the code
this rewriter would make of these bytes. A module whose rewriting warns
(``assert (x, "msg")``, a ``SyntaxWarning`` from Python's compiler) or gives
up on an assert too deep to explain is never cached: it is rewritten at each
import, so its warnings are issued, under the filters of the moment, every
time. Nothing is written under ``sys.dont_write_bytecode``; a cache that
can't be read or written is ignored.

Standard library only (Python 3.10+).
"""

from __future__ import annotations

import ast
import importlib.machinery
import importlib.util
import itertools
import linecache
import marshal
import os
import reprlib
import sys
import threading
import types
import warnings
from collections.abc import Callable, Iterable, Sequence
from contextlib import suppress
from hashlib import sha1
from typing import Any

# Bound at import, as the engine binds what tests may patch.
_abspath = os.path.abspath
_isfile = os.path.isfile
_find_spec = importlib.machinery.PathFinder.find_spec
_spec_from_file_location = importlib.util.spec_from_file_location
_decode_source = importlib.util.decode_source
_cache_from_source = importlib.util.cache_from_source
_open, _replace, _makedirs, _unlink, _getpid = open, os.replace, os.makedirs, os.unlink, os.getpid
_dirname, _splitext = os.path.dirname, os.path.splitext
_dumps, _loads = marshal.dumps, marshal.loads
_MAGIC_NUMBER = importlib.util.MAGIC_NUMBER

# The rewritten code's cache: the file's name ends with this before ``.pyc``
# and its content starts with ``_CACHE_HEADER`` and the key.
_CACHE_SUFFIX = "-aye"
_CACHE_HEADER = b"aye-rewrite\x01"

# A value in a message is cut to this many characters (pytest's
# ``DEFAULT_REPR_MAX_SIZE``).
MAX_REPR = 240

# What ``assert (x, "msg")`` warns, in Python's and pytest's words.
ALWAYS_TRUE = "assertion is always true, perhaps remove parentheses?"

# A module whose docstring holds this is not rewritten (pytest's marker).
DONT_REWRITE = "PYTEST_DONT_REWRITE"

# An operand's source text, as a failure carries it, is cut to this many
# characters (the rewritten code holds it).
MAX_SOURCE = 1000

# Lower-case callables whose call with literal arguments is a literal too, as
# a capitalised one's (a class: ``Decimal("9.45")``, ``Point(1, 2)``) is.
LITERAL_CALLS = frozenset(
    {"dict", "list", "set", "tuple", "frozenset", "datetime", "date", "time", "timedelta"}
)

# pytest's ``pytest_assertrepr_compare``: ``(op, left, right)`` of a
# comparison a message shows to the lines that replace ``left op right``, or
# ``None`` to keep it. An exception it raises keeps it too.
explain_comparison: Callable[[str, Any, Any], Sequence[str] | None] | None = None

# How rewritten code reaches this module and the builtins: names no source
# can spell, so they never clash with the user's.
_HELPERS = "@aye_rewrite"
_BUILTINS = "@aye_builtins"

_COMPARISONS: dict[type[ast.cmpop], str] = {
    ast.Eq: "==",
    ast.NotEq: "!=",
    ast.Lt: "<",
    ast.LtE: "<=",
    ast.Gt: ">",
    ast.GtE: ">=",
    ast.Is: "is",
    ast.IsNot: "is not",
    ast.In: "in",
    ast.NotIn: "not in",
}
_BINARY: dict[type[ast.operator], str] = {
    ast.BitOr: "|",
    ast.BitXor: "^",
    ast.BitAnd: "&",
    ast.LShift: "<<",
    ast.RShift: ">>",
    ast.Add: "+",
    ast.Sub: "-",
    ast.Mult: "*",
    ast.Div: "/",
    ast.FloorDiv: "//",
    ast.Mod: "%",
    ast.Pow: "**",
    ast.MatMult: "@",
}
_UNARY: dict[type[ast.unaryop], str] = {
    ast.Not: "not ",
    ast.Invert: "~",
    ast.USub: "-",
    ast.UAdd: "+",
}


# ------------------------------------------------------------------ install


class RewritingLoader(importlib.machinery.SourceFileLoader):
    """Loads a source file with its asserts rewritten. Never reads or writes
    Python's cached bytecode (a ``.pyc`` holds the code as Python compiled
    it): the rewritten code is cached apart (``cached_rewrite``)."""

    def __init__(self, fullname: str, path: str, *, warning: type[Warning] = SyntaxWarning):
        super().__init__(fullname, path)
        self.warning = warning

    def get_code(self, fullname: str) -> types.CodeType:
        path = self.get_filename(fullname)
        return cached_rewrite(self.get_data(path), path, warning=self.warning)


class _Finder:
    """The ``sys.meta_path`` finder ``install`` puts first. It finds a module
    as the path finder does, and claims it when its source file is to be
    rewritten; any other module is left to the finders after it."""

    def __init__(self) -> None:
        self.should_rewrite: Callable[[str], bool] = lambda path: False
        self.modules: Sequence[str] = ()
        self.warning: type[Warning] = SyntaxWarning
        # Imports made while deciding (by the predicate) are not claimed.
        self._deciding = threading.local()

    def find_spec(
        self, name: str, path: Sequence[str] | None = None, target: types.ModuleType | None = None
    ) -> importlib.machinery.ModuleSpec | None:
        if getattr(self._deciding, "on", False):
            return None
        self._deciding.on = True
        try:
            return self._claim(name, path)
        finally:
            self._deciding.on = False

    def _claim(
        self, name: str, path: Sequence[str] | None
    ) -> importlib.machinery.ModuleSpec | None:
        spec = _find_spec(name, path)
        if (
            spec is None
            or spec.origin is None
            or not isinstance(spec.loader, importlib.machinery.SourceFileLoader)
            or not _isfile(spec.origin)
        ):
            return None
        origin = spec.origin
        if not (_marked(name, self.modules) or self.should_rewrite(_abspath(origin))):
            return None
        loader = RewritingLoader(name, origin, warning=self.warning)
        return _spec_from_file_location(
            name,
            origin,
            loader=loader,
            submodule_search_locations=spec.submodule_search_locations,
        )

    def invalidate_caches(self) -> None:
        """Nothing cached here: the path finder's caches are its own."""


def _marked(name: str, modules: Iterable[str]) -> bool:
    return any(name == marked or name.startswith(marked + ".") for marked in modules)


_finder = _Finder()


def install(
    should_rewrite: Callable[[str], bool],
    *,
    modules: Sequence[str] = (),
    warning: type[Warning] = SyntaxWarning,
) -> None:
    """Rewrite the asserts of every module imported from now on whose source
    file's absolute path ``should_rewrite`` accepts, or whose dotted name is
    in ``modules`` or under a package there (pytest's
    ``register_assert_rewrite``: ``modules`` is read at each import, so a
    list the front-end appends to works). ``warning`` is the category of the
    always-true-assert warning. Idempotent: the finder is put first in
    ``sys.meta_path`` once, and a second call replaces its settings.
    Modules already imported stay as they are."""
    _remove_finder()
    _finder.should_rewrite = should_rewrite
    _finder.modules = modules
    _finder.warning = warning
    sys.meta_path.insert(0, _finder)


def uninstall() -> None:
    """Remove the finder: later imports are Python's own. Its settings are
    forgotten (they may hold a front-end's session)."""
    _remove_finder()
    _finder.should_rewrite = _never
    _finder.modules = ()


def _remove_finder() -> None:
    while _finder in sys.meta_path:
        sys.meta_path.remove(_finder)


def _never(path: str) -> bool:
    return False


# ------------------------------------------------------------------ rewriting


def rewrite_source(
    source: str | bytes, filename: str, *, warning: type[Warning] = SyntaxWarning
) -> types.CodeType:
    """The code of a module whose source is ``source`` (text, or bytes
    decoded as Python decodes a source file), its asserts rewritten.
    ``filename`` names it in tracebacks and warnings. Raises ``SyntaxError``
    as ``compile`` does, and the always-true-assert warning when filters make
    it an error."""
    return _rewrite(source, filename, warning)[0]


def _rewrite(
    source: str | bytes, filename: str, warning: type[Warning]
) -> tuple[types.CodeType, bool]:
    """``rewrite_source``'s code, and whether every assert was rewritten (an
    assert too deep to explain stays Python's)."""
    text = _decode_source(source) if isinstance(source, (bytes, bytearray)) else source
    tree = ast.parse(text, filename=filename)
    module = _Module(text, filename, warning)
    module.rewrite(tree)
    return compile(tree, filename, "exec", dont_inherit=True), module.complete


def cached_rewrite(
    source: bytes, filename: str, *, warning: type[Warning] = SyntaxWarning
) -> types.CodeType:
    """``rewrite_source(source, filename)`` for a module's source file
    (``source``: its bytes), from the cache when an earlier rewrite of the
    same bytes at the same path left it there, else rewritten and cached when
    the rewrite issued no warning (whatever the filters: it is tried with
    every warning recorded) and explained every assert. A module that warns
    is rewritten again as ``rewrite_source`` rewrites it, so its warnings
    meet the filters of the moment, and isn't cached."""
    cache = _cache_path(filename)
    key = _cache_key(source, filename) if cache is not None else None
    if cache is not None and key is not None:
        code = _read_cached(cache, key)
        if code is not None:
            return code
        code = _quiet_rewrite(source, filename, warning)
        if code is not None:
            if not sys.dont_write_bytecode:
                _write_cached(cache, key, code)
            return code
    return rewrite_source(source, filename, warning=warning)


def _quiet_rewrite(source: bytes, filename: str, warning: type[Warning]) -> types.CodeType | None:
    """The rewritten code when rewriting it issues no warning, raises
    nothing and explains every assert; else ``None``."""
    try:
        with warnings.catch_warnings(record=True) as issued:
            warnings.simplefilter("always")
            code, complete = _rewrite(source, filename, warning)
    except Exception:  # noqa: BLE001 - rewritten again, raising as it does
        return None
    return code if complete and not issued else None


def _rewriter_digest() -> bytes | None:
    """A digest of this module's source, which makes the rewritten code: a
    cache entry made by another rewriter is never used. ``None`` (no cache)
    when the source can't be read."""
    try:
        lines = linecache.getlines(_rewriter_digest.__code__.co_filename)
    except Exception:  # noqa: BLE001 - no cache, then
        return None
    if not lines:
        return None
    return sha1("".join(lines).encode("utf-8", "surrogatepass"), usedforsecurity=False).digest()


_REWRITER = _rewriter_digest()


def _cache_path(filename: str) -> str | None:
    """Where the rewritten code of ``filename`` is cached: beside Python's
    own bytecode for it, ``-aye`` before ``.pyc``; ``None`` where Python
    caches no bytecode."""
    if _REWRITER is None:
        return None
    try:
        standard = _cache_from_source(filename)
    except Exception:  # noqa: BLE001 - no cache tag, an odd path: no cache
        return None
    base, extension = _splitext(standard)
    return f"{base}{_CACHE_SUFFIX}{extension}"


def _cache_key(source: bytes, filename: str) -> bytes | None:
    """What the rewritten code depends on: this rewriter, Python's bytecode
    version and optimization level, the name the code imports the helpers
    by, the file's path (in the code's frames) and its source's bytes."""
    try:
        digest = sha1(_REWRITER or b"", usedforsecurity=False)
        for part in (
            _MAGIC_NUMBER,
            str(sys.flags.optimize).encode(),
            __name__.encode(),
            filename.encode("utf-8", "surrogatepass"),
        ):
            digest.update(len(part).to_bytes(8, "little"))
            digest.update(part)
        digest.update(source)
    except Exception:  # noqa: BLE001 - no cache, then
        return None
    return digest.digest()


def _read_cached(cache: str, key: bytes) -> types.CodeType | None:
    """The code cached at ``cache`` under ``key``, else ``None``."""
    header = _CACHE_HEADER + key
    try:
        with _open(cache, "rb") as file:
            data = file.read()
        if not data.startswith(header):
            return None
        code = _loads(memoryview(data)[len(header) :])
    except Exception:  # noqa: BLE001 - missing, unreadable or stale: rewritten again
        return None
    return code if isinstance(code, types.CodeType) else None


def _write_cached(cache: str, key: bytes, code: types.CodeType) -> None:
    """Cache ``code`` at ``cache`` under ``key``, atomically (written beside
    it, then moved in place), or not at all."""
    temporary = f"{cache}.{_getpid()}"
    try:
        data = _CACHE_HEADER + key + _dumps(code)
        _makedirs(_dirname(cache), exist_ok=True)
        with _open(temporary, "wb") as file:
            file.write(data)
        _replace(temporary, cache)
    except Exception:  # noqa: BLE001 - a read-only tree, a race: not cached
        with suppress(Exception):  # it may never have been made
            _unlink(temporary)


class _Module:
    """Rewrites the asserts of one module's tree in place. ``complete``
    turns false when an assert is too deep to explain."""

    def __init__(self, source: str, filename: str, warning: type[Warning]) -> None:
        self.source = source
        self.filename = filename
        self.warning = warning
        self.complete = True
        self._lines: list[str] | None = None

    def rewrite(self, module: ast.Module) -> None:
        if not module.body:
            return
        docstring = ast.get_docstring(module, clean=False)
        if docstring and DONT_REWRITE in docstring:
            return
        rewritten = False
        todo: list[ast.AST] = [module]
        while todo:
            node = todo.pop()
            for field, value in ast.iter_fields(node):
                if isinstance(value, list):
                    new: list[Any] = []
                    for item in value:
                        if isinstance(item, ast.Assert):
                            new.extend(self._assert(item))
                            rewritten = True
                        else:
                            new.append(item)
                            # Expressions hold no statements, so no asserts.
                            if isinstance(item, ast.AST) and not isinstance(item, ast.expr):
                                todo.append(item)
                    setattr(node, field, new)
                elif isinstance(value, ast.AST) and not isinstance(value, ast.expr):
                    todo.append(value)
        if rewritten:
            self._import_helpers(module)

    def _assert(self, node: ast.Assert) -> list[ast.stmt]:
        if isinstance(node.test, ast.Tuple) and node.test.elts:
            warnings.warn_explicit(
                self.warning(ALWAYS_TRUE),
                category=None,
                filename=self.filename,
                lineno=node.lineno,
            )
        try:
            return _Assert(node, self.segment(node.test), self).statements()
        except RecursionError:
            # Too deep to explain: Python's own assert, rather than none.
            self.complete = False
            return [node]

    def _import_helpers(self, module: ast.Module) -> None:
        """``import builtins as @aye_builtins`` and this module as
        ``@aye_rewrite``, after the docstring and ``__future__`` imports."""
        body = module.body
        position = 0
        for index, item in enumerate(body):
            if (
                index == 0
                and isinstance(item, ast.Expr)
                and isinstance(item.value, ast.Constant)
                and isinstance(item.value.value, str)
            ):
                position = 1
            elif (
                isinstance(item, ast.ImportFrom) and item.level == 0 and item.module == "__future__"
            ):
                position = index + 1
            else:
                break
        anchor = body[min(position, len(body) - 1)]
        decorators = getattr(anchor, "decorator_list", None)
        lineno = decorators[0].lineno if decorators else anchor.lineno
        place = {"lineno": lineno, "col_offset": 0, "end_lineno": lineno, "end_col_offset": 0}
        imports: list[ast.stmt] = [
            ast.Import(names=[ast.alias(name=name, asname=alias, **place)], **place)
            for name, alias in (("builtins", _BUILTINS), (__name__, _HELPERS))
        ]
        body[position:position] = imports

    def lines(self) -> list[str]:
        """The source's lines, without their ends."""
        if self._lines is None:
            self._lines = self.source.replace("\r\n", "\n").replace("\r", "\n").split("\n")
        return self._lines

    def columns(self, node: ast.expr, line: int) -> tuple[int, int] | None:
        """``node``'s span on line ``line`` (1-based), in characters, the end
        excluded: to the line's end (its trailing blanks left out) when
        ``node`` goes on past it; ``None`` when it starts on another line."""
        lines = self.lines()
        if node.lineno != line or not 0 < line <= len(lines):
            return None
        text = lines[line - 1]
        start = len(_cut(text, 0, node.col_offset))
        if node.end_lineno == line and node.end_col_offset is not None:
            end = len(_cut(text, 0, node.end_col_offset))
        else:
            end = len(text.rstrip())
        return (start, end) if start < end else None

    def operand(self, node: ast.expr, line: int) -> tuple[str | None, tuple[int, int] | None, bool]:
        """What a failure says of a compared operand: its source text (cut
        to ``MAX_SOURCE`` characters), its span on the assert's first line
        ``line``, and whether it is written as a literal."""
        source = self.segment(node)
        if source is not None and len(source) > MAX_SOURCE:
            source = source[: MAX_SOURCE - 1] + "…"
        return source, self.columns(node, line), is_literal(node)

    def segment(self, node: ast.expr) -> str | None:
        """The source text of ``node``."""
        lines = self.lines()
        end_lineno, end_col = node.end_lineno, node.end_col_offset
        if end_lineno is None or end_col is None:
            return None
        first, last = node.lineno - 1, end_lineno - 1
        if not 0 <= first <= last < len(lines):
            return None
        if first == last:
            return _cut(lines[first], node.col_offset, end_col)
        parts = [_cut(lines[first], node.col_offset, None), *lines[first + 1 : last]]
        parts.append(_cut(lines[last], 0, end_col))
        return "\n".join(parts)


def _cut(line: str, start: int, end: int | None) -> str:
    """``line[start:end]`` with offsets in UTF-8 bytes, as the AST's are."""
    return line.encode()[start:end].decode(errors="replace")


def is_literal(node: ast.expr) -> bool:
    """Whether ``node`` is written as a literal, the side of a comparison
    that holds the expected value: a constant, a negated one, a list, tuple,
    set or dict of literals, a named constant (a name or attribute whose
    last part is UPPER_CASE), or a call of a class (a capitalised name) or of
    one of ``LITERAL_CALLS`` with literal arguments."""
    try:
        return _literal(node)
    except RecursionError:
        return False


def _literal(node: ast.expr) -> bool:  # noqa: PLR0911 - one return per form
    if isinstance(node, ast.Constant):
        return True
    if isinstance(node, ast.UnaryOp):
        return isinstance(node.op, (ast.USub, ast.UAdd)) and isinstance(node.operand, ast.Constant)
    if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
        return all(_literal(item) for item in node.elts)
    if isinstance(node, ast.Dict):
        return all(key is not None and _literal(key) for key in node.keys) and all(
            _literal(value) for value in node.values
        )
    if isinstance(node, ast.Name):
        return _named_constant(node.id)
    if isinstance(node, ast.Attribute):
        return _named_constant(node.attr)
    if isinstance(node, ast.Call):
        func = node.func
        if isinstance(func, ast.Name):
            name = func.id
        elif isinstance(func, ast.Attribute):
            name = func.attr
        else:
            return False
        if not (name[:1].isupper() or name in LITERAL_CALLS):
            return False
        return all(_literal(arg) for arg in node.args) and all(
            keyword.arg is not None and _literal(keyword.value) for keyword in node.keywords
        )
    return False


def _named_constant(name: str) -> bool:
    """``MAX_SIZE``, ``RED``: UPPER_CASE, more than one character."""
    return len(name.strip("_")) > 1 and name.isupper()


_LOAD, _STORE, _DEL = ast.Load(), ast.Store(), ast.Del()


def _where(node: ast.AST) -> dict[str, Any]:
    """``node``'s location, for a node built in its place."""
    return {
        "lineno": node.lineno,  # type: ignore[attr-defined]
        "col_offset": node.col_offset,  # type: ignore[attr-defined]
        "end_lineno": node.end_lineno,  # type: ignore[attr-defined]
        "end_col_offset": node.end_col_offset,  # type: ignore[attr-defined]
    }


# A plan says how to explain an expression from the temporaries' values: a
# tuple ``(kind, index, ...)`` where ``index`` is the temporary holding the
# expression's value (-1: none). The kinds (``_Explainer``'s methods):
#   ("value", i)                               shown by its repr
#   ("name", i, name)                          a name (or a walrus's target)
#   ("unary", i, prefix, operand)              ``not x``, ``-x``, ``~x``, ``+x``
#   ("binary", i, symbol, left, right)         ``(left + right)``
#   ("call", i, func, args, keywords)          keywords: ((name|None, plan), ...)
#   ("starred", -1, value)                     ``*value`` in a call
#   ("attribute", i, owner, attr)              ``owner.attr``
#   ("boolop", i, is_or, operands, reached)    ``and``/``or``; ``reached``: the
#                                              temporary holding the index of
#                                              the last operand evaluated
#   ("compare", i, operands, symbols, reached, sides)
#                                              ``reached`` as for ``and``,
#                                              over the links (-1: one link);
#                                              ``sides``: per operand, what
#                                              ``_Module.operand`` says of it


class _Assert:
    """Rewrites one ``assert`` into statements that evaluate its test into
    temporaries and, when it is false, raise ``_fail``'s exception, built
    from the plan of the test and the temporaries' values (all bound first,
    so that what short-circuiting skipped is ``None`` and never read).

    Every node built is located at the assert (tracebacks and coverage point
    at it), or at the subexpression it stands for; the original
    subexpressions keep their own locations."""

    def __init__(self, node: ast.Assert, expr: str | None, module: _Module) -> None:
        self.node = node
        self.expr = expr
        self.module = module
        self.at = _where(node)
        self.temps: list[str] = []
        self.stmts: list[ast.stmt] = []
        # For an ``and``/``or`` or a chain whose truth is asked for: the
        # temporary that stands for it, by the temporary of its value.
        self.truths: dict[int, int] = {}

    # ------------------------------------------------------------- building

    def load(self, name: str) -> ast.Name:
        return ast.Name(name, _LOAD, **self.at)

    def const(self, value: Any) -> ast.Constant:
        return ast.Constant(value, **self.at)

    def temp(self) -> int:
        index = len(self.temps)
        self.temps.append(f"@aye_t{index}")
        return index

    def value(self, index: int) -> ast.Name:
        return self.load(self.temps[index])

    def assignment(self, index: int, value: ast.expr) -> ast.Assign:
        target = ast.Name(self.temps[index], _STORE, **self.at)
        return ast.Assign([target], value, **self.at)

    def set(self, index: int, value: ast.expr) -> None:
        self.stmts.append(self.assignment(index, value))

    def assign(self, value: ast.expr) -> int:
        index = self.temp()
        self.set(index, value)
        return index

    def go_on(self, test: ast.expr, when: bool, otherwise: list[ast.stmt]) -> None:
        """Evaluate what follows only when ``test`` is ``when``, and
        ``otherwise`` when it isn't."""
        body: list[ast.stmt] = []
        if not when and not otherwise:
            test, when = self.negate(test), True
        branches = (body, otherwise) if when else (otherwise, body)
        self.stmts.append(ast.If(test, *branches, **self.at))
        self.stmts = body

    def truth(self, plan: tuple[Any, ...]) -> ast.Name:
        """What stands for an expression's truth, not yet tested: its value,
        or what its ``and``/``or`` or chain left when it decided."""
        return self.value(self.truths.get(plan[1], plan[1]))

    def negate(self, value: ast.expr) -> ast.UnaryOp:
        return ast.UnaryOp(ast.Not(), value, **self.at)

    def helper(self, module: str, name: str, *args: ast.expr) -> ast.Call:
        func = ast.Attribute(self.load(module), name, _LOAD, **self.at)
        return ast.Call(func, list(args), [], **self.at)

    # ------------------------------------------------------------ the assert

    def statements(self) -> list[ast.stmt]:
        node = self.node
        plan = self.visit(node.test, jump=True)
        values = ast.Tuple([self.value(i) for i in range(len(self.temps))], _LOAD, **self.at)
        local_names = self.helper(_BUILTINS, "locals")
        span = self.module.columns(node.test, node.lineno)
        args = [self.const(plan), values, local_names, self.const(self.expr), self.const(span)]
        if node.msg is not None:
            args.append(node.msg)
        failed = ast.Raise(self.helper(_HELPERS, "_fail", *args), None, **self.at)
        stores = [ast.Name(name, _STORE, **self.at) for name in self.temps]
        deletes = [ast.Name(name, _DEL, **self.at) for name in self.temps]
        return [
            # Bound first, so that one ``del`` removes them whichever were
            # evaluated: no trace in ``locals()``, a module or a class body.
            ast.Assign(stores, self.const(None), **self.at),
            *self.stmts,
            ast.If(self.negate(self.truth(plan)), [failed], [], **self.at),
            ast.Delete(deletes, **self.at),
        ]

    # -------------------------------------------------------------- visitors

    def visit(self, node: ast.expr, jump: bool = False) -> tuple[Any, ...]:
        """The plan of ``node``, its evaluation added. ``jump``: only its
        truth matters (the assert's test, ``not``'s operand, an operand of
        such an ``and``/``or``), where Python tests each operand's truth
        once, as ``if`` does."""
        if isinstance(node, (ast.BoolOp, ast.Compare, ast.UnaryOp)):
            return getattr(self, f"visit_{type(node).__name__}")(node, jump)
        visitor = getattr(self, f"visit_{type(node).__name__}", None)
        if visitor is None:
            return self.generic(node)
        return visitor(node)

    def generic(self, node: ast.expr) -> tuple[Any, ...]:
        """Anything else (constants, subscripts, comprehensions, lambdas,
        ``await``, …): evaluated as written, shown by its repr."""
        return ("value", self.assign(node))

    def visit_Name(self, node: ast.Name) -> tuple[Any, ...]:
        return ("name", self.assign(node), node.id)

    def visit_NamedExpr(self, node: ast.NamedExpr) -> tuple[Any, ...]:
        # Bound once, as written; shown as its target would be.
        target = node.target
        return ("name", self.assign(node), target.id if isinstance(target, ast.Name) else "")

    def visit_UnaryOp(self, node: ast.UnaryOp, jump: bool) -> tuple[Any, ...]:
        jump = jump and isinstance(node.op, ast.Not)
        operand = self.visit(node.operand, jump)
        tested = self.truth(operand) if jump else self.value(operand[1])
        index = self.assign(ast.UnaryOp(node.op, tested, **_where(node)))
        return ("unary", index, _UNARY[type(node.op)], operand)

    def visit_BinOp(self, node: ast.BinOp) -> tuple[Any, ...]:
        left = self.visit(node.left)
        right = self.visit(node.right)
        value = ast.BinOp(self.value(left[1]), node.op, self.value(right[1]), **_where(node))
        return ("binary", self.assign(value), _BINARY[type(node.op)], left, right)

    def visit_Call(self, node: ast.Call) -> tuple[Any, ...]:
        func = self.visit(node.func)
        args: list[ast.expr] = []
        arg_plans: list[tuple[Any, ...]] = []
        for arg in node.args:
            if isinstance(arg, ast.Starred):
                inner = self.visit(arg.value)
                args.append(ast.Starred(self.value(inner[1]), _LOAD, **_where(arg)))
                arg_plans.append(("starred", -1, inner))
            else:
                plan = self.visit(arg)
                args.append(self.value(plan[1]))
                arg_plans.append(plan)
        keywords: list[ast.keyword] = []
        keyword_plans: list[tuple[str | None, tuple[Any, ...]]] = []
        for keyword in node.keywords:
            plan = self.visit(keyword.value)
            keywords.append(ast.keyword(keyword.arg, self.value(plan[1]), **_where(keyword)))
            keyword_plans.append((keyword.arg, plan))
        value = ast.Call(self.value(func[1]), args, keywords, **_where(node))
        return ("call", self.assign(value), func, tuple(arg_plans), tuple(keyword_plans))

    def visit_Attribute(self, node: ast.Attribute) -> tuple[Any, ...]:
        if not isinstance(node.ctx, ast.Load):
            return self.generic(node)
        owner = self.visit(node.value)
        value = ast.Attribute(self.value(owner[1]), node.attr, _LOAD, **_where(node))
        return ("attribute", self.assign(value), owner, node.attr)

    def visit_BoolOp(self, node: ast.BoolOp, jump: bool) -> tuple[Any, ...]:
        """``a and b``: ``b`` evaluated only when ``a`` is true."""
        result = self.temp()
        reached = self.temp()
        decided = self.temp() if jump else -1
        is_or = isinstance(node.op, ast.Or)
        outer = self.stmts
        plans: list[tuple[Any, ...]] = []
        last = len(node.values) - 1
        for index, operand in enumerate(node.values):
            plan = self.visit(operand, jump)
            self.set(result, self.value(plan[1]))
            self.set(reached, self.const(index))
            plans.append(plan)
            if index < last:
                # When decided, its truth is known: tested no more.
                otherwise = [self.assignment(decided, self.const(is_or))] if jump else []
                tested = self.truth(plan) if jump else self.value(result)
                self.go_on(tested, not is_or, otherwise)
            elif jump:
                self.set(decided, self.truth(plan))
        self.stmts = outer
        if jump:
            self.truths[result] = decided
        return ("boolop", result, is_or, tuple(plans), reached)

    def visit_Compare(self, node: ast.Compare, jump: bool) -> tuple[Any, ...]:
        """``a < b < c``: ``c`` evaluated only when ``a < b``."""
        operands = [node.left, *node.comparators]
        last = len(node.ops) - 1
        result = self.temp()
        reached = self.temp() if last else -1
        decided = self.temp() if jump and last else -1
        outer = self.stmts
        plans = [self.visit(operands[0])]
        for index, op in enumerate(node.ops):
            plans.append(self.visit(operands[index + 1]))
            left, right = self.value(plans[index][1]), self.value(plans[index + 1][1])
            self.set(result, ast.Compare(left, [op], [right], **_where(node)))
            if not last:
                break
            self.set(reached, self.const(index))
            if index < last:
                failed = [self.assignment(decided, self.const(False))] if decided >= 0 else []
                self.go_on(self.value(result), True, failed)
            elif decided >= 0:
                self.set(decided, self.value(result))
        self.stmts = outer
        if decided >= 0:
            self.truths[result] = decided
        symbols = tuple(_COMPARISONS[type(op)] for op in node.ops)
        line = self.node.lineno
        sides = tuple(self.module.operand(operand, line) for operand in operands)
        return ("compare", result, tuple(plans), symbols, reached, sides)


# ------------------------------------------------- helpers of rewritten code

_NO_MESSAGE = object()


def _fail(
    plan: tuple[Any, ...],
    values: tuple[Any, ...],
    local_names: Any,
    expr: str | None,
    span: tuple[int, int] | None,
    message: Any = _NO_MESSAGE,
) -> AssertionError:
    """The exception a failed assert raises (the rewritten code raises it,
    so that the traceback ends in the user's frame): pytest's message, the
    test's span on the assert's first line, and the comparison that decided,
    if one did."""
    try:
        explanation = _Explainer(values, local_names)(plan)
    except Exception as exc:  # noqa: BLE001 - a failure all the same
        explanation = f"{expr} (aye could not explain it: {type(exc).__name__}: {exc})"
    if message is _NO_MESSAGE:
        text = "assert " + explanation
    else:
        text = _format_message(message) + "\n>assert " + explanation
    error = AssertionError(format_explanation(text))
    error._aye_span = span  # type: ignore[attr-defined]
    decided = _decided(plan, values)
    if decided is not None:
        op, left, right, sides = decided
        data = {"op": op, "left": left, "right": right, "expr": expr}
        data.update(_sides(*sides))
        error._aye_assert = data  # type: ignore[attr-defined]
    return error


def _decided(
    plan: tuple[Any, ...], values: tuple[Any, ...]
) -> tuple[str, Any, Any, tuple[Any, Any]] | None:
    """The link that failed when the test is a comparison, or an ``and``
    whose last operand evaluated is one: (op, left, right, and what the plan
    says of each operand)."""
    if plan[0] == "boolop" and not plan[2]:
        plan = plan[3][values[plan[4]]]
    if plan[0] != "compare":
        return None
    operands, symbols, reached, sides = plan[2], plan[3], plan[4], plan[5]
    link = values[reached] if reached >= 0 else 0
    left, right = values[operands[link][1]], values[operands[link + 1][1]]
    return symbols[link], left, right, (sides[link], sides[link + 1])


def _sides(left: tuple[Any, ...], right: tuple[Any, ...]) -> dict[str, Any]:
    """``_aye_assert``'s keys about the operands: each one's source and span,
    and which one is the expected value: the literal, when only one is; the
    right one, when both are (``assert actual == expected``)."""
    expected = None
    if left[2] and right[2]:
        expected = "right"
    elif left[2] != right[2]:
        expected = "left" if left[2] else "right"
    return {
        "left_src": left[0],
        "right_src": right[0],
        "left_cols": left[1],
        "right_cols": right[1],
        "expected": expected,
    }


class _Explainer:
    """Explains an expression from its plan and the temporaries' values, in
    pytest's language (see ``format_explanation``): one method per kind of
    plan."""

    def __init__(self, values: tuple[Any, ...], local_names: Any) -> None:
        self.values = values
        self.local_names = local_names

    def __call__(self, plan: tuple[Any, ...]) -> str:
        return getattr(self, plan[0])(plan)

    def value(self, plan: tuple[Any, ...]) -> str:
        return _repr(self.values[plan[1]])

    def name(self, plan: tuple[Any, ...]) -> str:
        return _name(plan[2], self.values[plan[1]], self.local_names)

    def unary(self, plan: tuple[Any, ...]) -> str:
        return plan[2] + self(plan[3])

    def binary(self, plan: tuple[Any, ...]) -> str:
        return f"({self(plan[3])} {plan[2]} {self(plan[4])})"

    def call(self, plan: tuple[Any, ...]) -> str:
        shown = _repr(self.values[plan[1]])
        parts = [self(arg) for arg in plan[3]]
        for keyword, value in plan[4]:
            parts.append(f"{keyword}={self(value)}" if keyword else f"**{self(value)}")
        return f"{shown}\n{{{shown} = {self(plan[2])}({', '.join(parts)})\n}}"

    def starred(self, plan: tuple[Any, ...]) -> str:
        return "*" + self(plan[2])

    def attribute(self, plan: tuple[Any, ...]) -> str:
        shown = _repr(self.values[plan[1]])
        return f"{shown}\n{{{shown} = {self(plan[2])}.{plan[3]}\n}}"

    def boolop(self, plan: tuple[Any, ...]) -> str:
        evaluated = plan[3][: self.values[plan[4]] + 1]
        return "(" + (" or " if plan[2] else " and ").join(map(self, evaluated)) + ")"

    def compare(self, plan: tuple[Any, ...]) -> str:
        """The last link evaluated: the failing one when the chain is false."""
        operands, symbols, reached = plan[2], plan[3], plan[4]
        link = self.values[reached] if reached >= 0 else 0
        left, right, symbol = operands[link], operands[link + 1], symbols[link]
        custom = _custom(symbol, self.values[left[1]], self.values[right[1]])
        if custom is not None:
            return custom
        shown = [
            f"({self(operand)})" if operand[0] in ("compare", "boolop") else self(operand)
            for operand in (left, right)
        ]
        return f"{shown[0]} {symbol} {shown[1]}"


def _custom(op: str, left: Any, right: Any) -> str | None:
    """``explain_comparison``'s lines for a comparison, as one explanation."""
    explain = explain_comparison
    if explain is None:
        return None
    try:
        lines = explain(op, left, right)
        if not lines:
            return None
        return "\n~".join(str(line).replace("\n", "\\n") for line in lines)
    except Exception:  # noqa: BLE001 - the plain explanation then
        return None


def _format_message(message: Any) -> str:
    """The user's message: a string as it is, anything else by its repr;
    its lines kept as lines of the explanation."""
    if isinstance(message, str):
        return message.replace("\n", "\n~")
    return saferepr(message).replace("\n", "\n~").replace("\\n", "\n~")


def _repr(value: Any) -> str:
    """A value in an explanation: bound methods by their name, anything
    else by its safe repr, on one line."""
    if isinstance(value, types.MethodType):
        try:
            return str(value.__name__)
        except Exception:  # noqa: BLE001 - a callable without a name: its repr
            return saferepr(value).replace("\n", "\\n")
    return saferepr(value).replace("\n", "\\n")


def _name(identifier: str, value: Any, local_names: Any) -> str:
    """A name in an explanation: its value when it is a local, or a global
    neither callable nor named (pytest's rule), else the name."""
    if identifier in local_names or _should_repr(value):
        return _repr(value)
    return identifier


def _should_repr(value: Any) -> bool:
    """Whether a global name is shown by its value: not for functions,
    classes and modules, which are better known by their names."""
    if callable(value):
        return False
    try:
        return not hasattr(value, "__name__")
    except Exception:  # noqa: BLE001 - a broken ``__getattr__``
        return True


# ------------------------------------------------------------- explanations


def format_explanation(explanation: str) -> str:
    """pytest's explanation language: lines starting ``{`` open a ``where``
    (or ``and``) clause, ``}`` closes it, ``~`` continues the current
    clause on a new line, ``>`` starts a line at its level; any other
    newline is shown escaped."""
    raw = (explanation or "").split("\n")
    lines = [raw[0]]
    for line in raw[1:]:
        if line and line[0] in "{}~>":
            lines.append(line)
        else:
            lines[-1] += "\\n" + line
    result = lines[:1]
    stack = [0]
    counts = [0]
    for line in lines[1:]:
        if line.startswith("{"):
            word = "and   " if counts[-1] else "where "
            stack.append(len(result))
            counts[-1] += 1
            counts.append(0)
            result.append(" +" + "  " * (len(stack) - 1) + word + line[1:])
        elif line.startswith("}"):
            if len(stack) > 1:
                stack.pop()
                counts.pop()
            result[stack[-1]] += line[1:]
        else:
            stack[-1] += 1
            indent = len(stack) if line.startswith("~") else len(stack) - 1
            result.append("  " * indent + line[1:])
    return "\n".join(result)


def saferepr(value: Any, maxsize: int | None = MAX_REPR) -> str:
    """``repr(value)`` as pytest shows it: containers cut by ``reprlib``'s
    limits, the whole cut to ``maxsize`` characters (``None``: no limit)
    keeping its start and end, and a broken ``__repr__`` described rather
    than raised."""
    return _SafeRepr(maxsize).repr(value)


class _SafeRepr(reprlib.Repr):
    def __init__(self, maxsize: int | None) -> None:
        super().__init__()
        # Strings are cut by ``reprlib`` too; it needs an int.
        self.maxstring = maxsize if maxsize is not None else 1_000_000_000
        self.maxsize = maxsize

    def repr(self, x: Any) -> str:
        try:
            text = super().repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # noqa: BLE001 - shown, not raised
            text = _repr_failed(exc, x)
        return self._cut(text)

    def repr_instance(self, x: Any, level: int) -> str:
        try:
            text = repr(x)
        except (KeyboardInterrupt, SystemExit):
            raise
        except BaseException as exc:  # noqa: BLE001 - shown, not raised
            text = _repr_failed(exc, x)
        return self._cut(text)

    def repr_dict(self, x: dict[Any, Any], level: int) -> str:
        """In insertion order, where ``reprlib`` sorts."""
        if not x:
            return "{}"
        if level <= 0:
            return "{...}"
        pieces = [
            f"{self.repr1(key, level - 1)}: {self.repr1(x[key], level - 1)}"
            for key in itertools.islice(x, self.maxdict)
        ]
        if len(x) > self.maxdict:
            pieces.append("...")
        return "{" + ", ".join(pieces) + "}"

    def _cut(self, text: str) -> str:
        size = self.maxsize
        if size is None or len(text) <= size:
            return text
        head = max(0, (size - 3) // 2)
        tail = max(0, size - 3 - head)
        return text[:head] + "..." + text[len(text) - tail :]


def _repr_failed(exc: BaseException, value: Any) -> str:
    try:
        raised = _repr_or_str(exc)
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException as inner:  # noqa: BLE001 - shown, not raised
        raised = f"unpresentable exception ({_repr_or_str(inner)})"
    return f"<[{raised} raised in repr()] {type(value).__name__} object at 0x{id(value):x}>"


def _repr_or_str(value: Any) -> str:
    try:
        return repr(value)
    except (KeyboardInterrupt, SystemExit):
        raise
    except BaseException:  # noqa: BLE001 - its str, then
        return f'{type(value).__name__}("{value}")'
