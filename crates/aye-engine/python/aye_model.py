"""aye's test model: what a front-end produces when it reads a framework's
test code, and what aye's engine executes. It knows no framework: pytest,
unittest and doctest are front-ends that translate their declarations into
these objects. Values live in the worker; nothing here crosses the protocol.

Standard library only (Python 3.10+).

Prototype (October 2026): the pytest front-end produces it and the native
engine (``aye_native_engine``) runs it; ``docs/model.md`` will specify it.
"""

from __future__ import annotations

import enum
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import Any


class Scope(enum.IntEnum):
    """How long a resource's value lives, outermost first. A resource may
    depend only on resources of its own scope or wider ones."""

    SESSION = 0
    PACKAGE = 1
    MODULE = 2
    CLASS = 3
    FUNCTION = 4

    @classmethod
    def named(cls, name: str) -> Scope:
        """``"session"``, ``"package"``, ``"module"``, ``"class"`` or
        ``"function"``."""
        return cls[name.upper()]


@dataclass(frozen=True)
class Tag:
    """A label on a test or node, with arguments: pytest's mark, unittest's
    skip decorators read as data. ``name`` is the mark's name."""

    name: str
    args: tuple[Any, ...] = ()
    kwargs: dict[str, Any] = field(default_factory=dict, hash=False, compare=False)


@dataclass(eq=False)
class ResourceDef:
    """A named resource (pytest's fixture): a factory with a scope. ``func``
    is called with the values of ``argnames`` (other resources, and
    ``request``); a generator function's code after its first ``yield`` is
    its teardown. ``params`` makes it parametrized: each case is bound to one
    of them (see ``Case.resource_params``)."""

    name: str
    func: Callable[..., Any]
    scope: Scope = Scope.FUNCTION
    argnames: tuple[str, ...] = ()
    params: list[Any] | None = None
    # The ids of ``params``, as the framework computed them.
    param_ids: list[str] | None = None
    autouse: bool = False
    # Where it was defined: a node id prefix ("" for everything, e.g. a
    # builtin or a root conftest; "tests/sub/" for a conftest there;
    # "tests/test_a.py" or "tests/test_a.py::TestX" for a module or class).
    baseid: str = ""
    # For a method resource (defined in a class): bound per instance.
    is_method: bool = False
    # For messages: "tests/conftest.py:12".
    location: str = ""


@dataclass(eq=False)
class Node:
    """A container of tests whose lifetime the engine opens and closes: a
    package, a module, a class. ``obj`` is the imported object."""

    id: str
    kind: str  # "package", "module" or "class"
    obj: Any
    parent: Node | None = None
    # Every resource name visible from here, each with its override chain,
    # closest definition first (a definition that requests its own name gets
    # the next one). Filled by the front-end; shared by the node's cases.
    visible: dict[str, list[ResourceDef]] = field(default_factory=dict)
    tags: list[Tag] = field(default_factory=list)

    def chain(self) -> list[Node]:
        """This node and its parents, outermost first."""
        nodes: list[Node] = []
        node: Node | None = self
        while node is not None:
            nodes.append(node)
            node = node.parent
        return nodes[::-1]


@dataclass(eq=False)
class Case:
    """One test as the engine runs it."""

    # aye's id: "tests/test_a.py::TestX::test_y[1-a]".
    id: str
    node: Node
    # The test function (for a class's test, the function: the engine makes
    # a fresh instance of ``cls`` per case and binds it).
    func: Callable[..., Any]
    cls: type | None = None
    # The resources set up for this case, in setup order (pytest's
    # ``fixturenames``: autouse first, then the function's arguments, then
    # what those need), all resolvable through ``node.visible``.
    closure: tuple[str, ...] = ()
    # The names the test function takes, in order (passed by keyword).
    argnames: tuple[str, ...] = ()
    # Direct parameters (parametrize without ``indirect``): argname -> value.
    call_params: dict[str, Any] = field(default_factory=dict)
    # Parametrized resources: name -> (index, value, id). Also indirect
    # parameters, which become a resource's ``request.param``.
    resource_params: dict[str, tuple[int, Any, str]] = field(default_factory=dict)
    # Scope of each parameter (parametrize ``scope=``, or the resource's):
    # cases share a resource instance only when their params agree.
    param_scopes: dict[str, Scope] = field(default_factory=dict)
    # Every tag on the case, closest first (its own, its parametrize param's,
    # its class's, its module's), as pytest's ``iter_markers`` returns them.
    tags: list[Tag] = field(default_factory=list)
    # The names ``-k`` matches, when the front-end gives them (none: a
    # shared empty tuple, never changed).
    keywords: Sequence[str] = ()
    # (path relative to the root, 1-based line) of the definition.
    location: tuple[str, int] = ("", 0)
    # The framework's kind of test: "function" (the default), or another the
    # engine has a runner for ("unittest").
    kind: str = "function"
    # Free slot for the front-end (e.g. the framework's item object for hook
    # calls); the engine passes it back in hooks and never reads it.
    extra: dict[str, Any] = field(default_factory=dict)

    def tag(self, name: str) -> Tag | None:
        """The closest tag called ``name``."""
        return next((t for t in self.tags if t.name == name), None)

    def tags_named(self, name: str) -> list[Tag]:
        return [t for t in self.tags if t.name == name]


@dataclass
class Policy:
    """A framework's outcome rules, as data the engine applies."""

    # Default for ``xfail(strict=...)`` (pytest's ``xfail_strict`` ini).
    xfail_strict: bool = False
    # Evaluate string conditions of skipif/xfail in this namespace, plus the
    # module's globals (pytest adds os, sys, platform, config).
    condition_namespace: dict[str, Any] = field(default_factory=dict)
    # Tags that mean a case is skipped / expected to fail, by name.
    skip_tags: tuple[str, ...] = ("skip", "skipif")
    xfail_tags: tuple[str, ...] = ("xfail",)
    usefixtures_tag: str = "usefixtures"
    # Exception types that mean skipped, expected failure, failed (without
    # being an error) and exit, by the framework (set by the front-end).
    skip_exceptions: tuple[type[BaseException], ...] = ()
    xfail_exceptions: tuple[type[BaseException], ...] = ()
    fail_exceptions: tuple[type[BaseException], ...] = ()
    exit_exceptions: tuple[type[BaseException], ...] = ()


@dataclass
class Hooks:
    """Calls the engine makes back into the front-end around each case
    (pytest's ``pytest_runtest_setup``, ``pytest_runtest_call``,
    ``pytest_runtest_teardown`` from conftests). Each gets the case."""

    setup: list[Callable[[Case], Any]] = field(default_factory=list)
    call: list[Callable[[Case], Any]] = field(default_factory=list)
    teardown: list[Callable[[Case], Any]] = field(default_factory=list)
