"""aye_rewrite: rewritten asserts explain themselves as pytest's do, and
behave as Python's when they pass.

pytest's own rewriter (``_pytest.assertion.rewrite``) is the referee for
messages: the same source goes through both and the messages must match,
object addresses aside. These tests run under pytest; ``aye_rewrite`` itself
never imports it (``test_imports_no_pytest``)."""

import ast
import asyncio
import gc
import importlib
import inspect
import itertools
import os
import re
import shutil
import subprocess
import sys
import textwrap
import traceback
import warnings
import weakref

import aye_rewrite
import pytest
from _pytest.assertion import rewrite as pytest_rewrite
from _pytest.assertion import util as pytest_util
from aye_rewrite import format_explanation, rewrite_source, saferepr

HERE = os.path.dirname(os.path.abspath(__file__))
PYTHON_DIR = os.path.dirname(HERE)
FILENAME = "/project/tests/test_sample.py"


def namespace(source, filename=FILENAME, **rewrite):
    """Execute ``source`` rewritten; its namespace."""
    code = rewrite_source(textwrap.dedent(source), filename, **rewrite)
    ns = {"__name__": "test_sample"}
    exec(code, ns)
    return ns


def failure(source, entry="test"):
    """The AssertionError calling ``entry`` in the rewritten ``source`` raises."""
    ns = namespace(source)
    with pytest.raises(AssertionError) as info:
        ns[entry]()
    return info.value


def referee(source, monkeypatch, entry="test"):
    """pytest's message for the same source, rewritten by pytest's own
    rewriter with no comparison plugin (the plain rewriting)."""
    monkeypatch.setattr(pytest_util, "_reprcompare", None)
    monkeypatch.setattr(pytest_util, "_config", None)
    text = textwrap.dedent(source)
    tree = ast.parse(text)
    pytest_rewrite.rewrite_asserts(tree, text.encode(), FILENAME, None)
    ns = {"__name__": "test_sample"}
    exec(compile(tree, FILENAME, "exec", dont_inherit=True), ns)
    with pytest.raises(AssertionError) as info:
        ns[entry]()
    return str(info.value)


def addresses(text):
    return re.sub(r"0x[0-9a-fA-F]+", "0x?", text)


def operands(exc):
    """What a failure's ``_aye_assert`` says of the comparison itself."""
    return {key: exc._aye_assert[key] for key in ("op", "left", "right", "expr")}


def plain(source):
    """Execute ``source`` as Python does; its namespace."""
    ns = {"__name__": "test_sample"}
    exec(compile(textwrap.dedent(source), FILENAME, "exec", dont_inherit=True), ns)
    return ns


# ------------------------------------------------------------ pytest parity

PARITY = {
    "eq": "def test():\n    x = 1\n    assert x == 2",
    "ne": "def test():\n    x = 1\n    assert x != 1",
    "lt": "def test():\n    x = 1\n    assert x < 1",
    "le": "def test():\n    x = 1\n    assert x <= 0",
    "gt": "def test():\n    x = 1\n    assert x > 1",
    "ge": "def test():\n    x = 1\n    assert x >= 2",
    "in": "def test():\n    x = 1\n    assert x in [2, 3]",
    "not-in": "def test():\n    x = 1\n    assert x not in [1]",
    "is": "def test():\n    x = 1\n    assert x is None",
    "is-not": "def test():\n    y = None\n    assert y is not None",
    "truthy-name": "def test():\n    x = None\n    assert x",
    "constant": "def test():\n    assert []",
    "not": "def test():\n    x = 1\n    assert not x",
    "not-compare": "def test():\n    assert not (1 == 1)",
    "invert-neg": "def test():\n    assert -1 == ~0 + 5",
    "binops": "def test():\n    x = 3\n    assert x + 1 == 5 - 2 * 1",
    "modulo": "def test():\n    x = 5\n    assert x % 2 == 0",
    "chain-last": "def test():\n    assert 1 < 2 < 1",
    "chain-first": "def test():\n    x = 2\n    assert 3 < x < 5",
    "chain-middle": "def test():\n    x = 2\n    assert 0 < x < 1 < x",
    "chain-equal": "def test():\n    assert 1 == 2 == 2",
    "chain-calls": (
        "def f(v):\n    return v\ndef test():\n    a = 1\n    assert f(a) < f(0) < f(5)"
    ),
    "compare-of-compares": "def test():\n    assert (1 == 2) == (3 == 3)",
    "compare-paren": "def test():\n    assert (1 < 2) == 3",
    "and": "def test():\n    a, b = 1, 0\n    assert a and b",
    "and-first": "def test():\n    a, b = 0, 1\n    assert a and b",
    "and-three": "def test():\n    assert 1 == 1 and 2 == 3 and 4 == 4",
    "or": "def test():\n    x = 2; y = 3\n    assert x == 1 or y == 2",
    "nested-boolops": "def test():\n    a, b, c = 0, 0, 1\n    assert (a or b) and c",
    "not-and": "def test():\n    a, b = 1, 2\n    assert not (a and b)",
    "boolop-in-compare": "def test():\n    x = [1, 2]\n    assert (x == [1] and x) == 5",
    "call": "def f():\n    return 1\ndef test():\n    assert f() == 2",
    "local-function": "def test():\n    def f():\n        return 1\n    assert f() == 2",
    "builtin-call": "def test():\n    assert len([1, 2]) == 3",
    "call-args": (
        "def f(*a, **k):\n    return 1\n"
        "def test():\n    x = 3\n    assert f(x, *[1, 2], a=x, **{'b': 2}) == 2"
    ),
    "attribute": ("class A:\n    y = 3\ndef test():\n    a = A()\n    assert a.y == 2"),
    "method": (
        "class A:\n    def m(self):\n        return 3\n"
        "def test():\n    a = A()\n    assert a.m() == 2"
    ),
    "method-chain": "def test():\n    s = 'abc'\n    assert s.upper().lower() == 'x'",
    "not-call": "def test():\n    s = 'abc'\n    assert not s.startswith('a')",
    "module-attribute": "import math\ndef test():\n    assert math.pi == 3",
    "module-function": "import os\ndef test():\n    assert os.path.exists('/nonexistent/aye')",
    "self-attribute": (
        "class C:\n    v = 2\n    def check(self):\n        assert self.v == 1\n"
        "def test():\n    C().check()"
    ),
    "global-value": "x = 5\ndef test():\n    assert x == 4",
    "global-class": "class K:\n    pass\ndef test():\n    assert K == 1",
    "subscript": "def test():\n    d = {'a': 1}\n    assert d['a'] == 2",
    "fstring": "def test():\n    assert f'{1}' == 'x'",
    "ifexp": "def test():\n    x = 3\n    assert x if x > 5 else None",
    "lambda-call": "def test():\n    assert (lambda: 1)() == 2",
    "comprehension": "def test():\n    assert [i for i in range(3)] == [1]",
    "generator-all": "def test():\n    xs = [1, -1]\n    assert all(x > 0 for x in xs)",
    "dict": "def test():\n    d = {'a': 1}\n    assert d == {'a': 2}",
    "walrus": "def test():\n    assert (y := 3) == 4",
    "walrus-rebinds": "def test():\n    x = 1\n    assert x == (x := 2)",
    "msg": "def test():\n    assert 1 == 2, 'hello'",
    "msg-lines": "def test():\n    assert 1 == 2, 'hello\\nworld'",
    "msg-percent": "def test():\n    assert 1 == 2, '100% %s %(x)s'",
    "msg-int": "def test():\n    assert 1 == 2, 42",
    "msg-none": "def test():\n    assert False, None",
    "msg-repr-lines": "def test():\n    assert 1 == 2, ['multi\\nline']",
    "msg-fstring": "def test():\n    x = 1\n    assert x == 2, f'x is {x}'",
    "msg-tuple": "def test():\n    assert 1 == 2, ('a', 'b\\nc')",
    "msg-with-where": "def f():\n    return 1\ndef test():\n    assert f() == 2, 'context'",
    "unicode-text": "def test():\n    assert 'é' == 'e'",
    "unicode-name": "def test():\n    café = 'ü'\n    assert café == 'u'",
    "bytes": "def test():\n    assert b'\\xff' == b''",
    "long-string": "def test():\n    x = 'a' * 300\n    assert x == 'b'",
    "long-list": "def test():\n    x = list(range(10))\n    assert x == 1",
    "long-int": "def test():\n    x = 10 ** 60\n    assert x == 1",
    "deep-nesting": "def test():\n    x = [[[[[[[1]]]]]]]\n    assert x == 1",
    "big-dict": "def test():\n    x = {i: i for i in range(10)}\n    assert x == 1",
    "repr-newline": (
        "class R:\n    def __repr__(self):\n        return 'a\\nb'\n"
        "def test():\n    assert R() == 1"
    ),
    "repr-raises": (
        "class R:\n    __hash__ = None\n    def __repr__(self):\n        raise ValueError('boom')\n"
        "def test():\n    r = R()\n    assert r == 1"
    ),
    "repr-long": (
        "class R:\n    def __repr__(self):\n        return 'r' * 1000\n"
        "def test():\n    assert R() == 1"
    ),
    "multiline": "def test():\n    x = 1\n    assert (\n        x\n        == 2\n    )",
    "in-class-method": (
        "class TestX:\n    def test_m(self):\n        assert [1] == [2]\n"
        "def test():\n    TestX().test_m()"
    ),
    "staticmethod": (
        "class TestX:\n    @staticmethod\n    def check():\n        assert 1 is None\n"
        "def test():\n    TestX.check()"
    ),
    "nested-function": (
        "def test():\n    def inner(v):\n        assert v == 'x', 'inner'\n    inner('y')"
    ),
    "closure": ("def test():\n    v = 2\n    def inner():\n        assert v == 1\n    inner()"),
    "in-loop": (
        "def test():\n    for i in range(3):\n        assert i < 2\n    else:\n        pass"
    ),
    "in-except": (
        "def test():\n    try:\n        1 / 0\n    except ZeroDivisionError as e:\n"
        "        assert e is None"
    ),
}


@pytest.mark.parametrize("source", PARITY.values(), ids=PARITY.keys())
def test_messages_match_pytests(source, monkeypatch):
    expected = addresses(referee(source, monkeypatch))
    assert addresses(str(failure(source))) == expected


def test_every_comparison_operator_carries_its_operands():
    cases = {
        "==": ("x == 2", 1, 2),
        "!=": ("x != 1", 1, 1),
        "<": ("x < 1", 1, 1),
        "<=": ("x <= 0", 1, 0),
        ">": ("x > 1", 1, 1),
        ">=": ("x >= 2", 1, 2),
        "in": ("x in [2, 3]", 1, [2, 3]),
        "not in": ("x not in [1]", 1, [1]),
        "is": ("x is None", 1, None),
        "is not": ("y is not None", None, None),
    }
    for op, (expr, left, right) in cases.items():
        exc = failure(f"def test():\n    x = 1\n    y = None\n    assert {expr}")
        assert operands(exc) == {"op": op, "left": left, "right": right, "expr": expr}
        assert str(exc) == f"assert {saferepr(left)} {op} {saferepr(right)}"


def test_operands_are_the_values_not_their_reprs():
    exc = failure("def test():\n    x = {'a': [1, 2]}\n    assert x == {'a': [1, 3]}, 'msg'")
    assert exc._aye_assert["left"] == {"a": [1, 2]}
    assert exc._aye_assert["right"] == {"a": [1, 3]}
    big = "x" * 10_000
    exc = failure(f"def test():\n    assert {big!r} == 'y'")
    assert exc._aye_assert["left"] == big


# ------------------------------------------------------------------ chains


def test_a_chain_explains_only_the_failing_link():
    source = "def test():\n    x = 3\n    assert 1 < x < 2 < 5"
    exc = failure(source)
    assert str(exc) == "assert 3 < 2"
    assert operands(exc) == {"op": "<", "left": 3, "right": 2, "expr": "1 < x < 2 < 5"}
    exc = failure("def test():\n    x = 0\n    assert 1 < x < 2")
    assert operands(exc) == {"op": "<", "left": 1, "right": 0, "expr": "1 < x < 2"}
    exc = failure("def test():\n    assert 1 < 2 < 3 == 4")
    assert operands(exc) == {"op": "==", "left": 3, "right": 4, "expr": "1 < 2 < 3 == 4"}


def test_a_chain_short_circuits_as_python_does():
    source = """
        calls = []
        def f(v):
            calls.append(v)
            return v
        def test():
            assert f(1) < f(0) < f(5)
    """
    ns = namespace(source)
    with pytest.raises(AssertionError) as info:
        ns["test"]()
    # pytest's rewriting calls f(5) too; Python doesn't.
    assert ns["calls"] == [1, 0]
    assert str(info.value) == "assert 1 < 0\n +  where 1 = f(1)\n +  and   0 = f(0)"


def test_a_passing_chain_inside_not_explains_its_last_link():
    exc = failure("def test():\n    assert not (1 < 2 < 3)")
    assert str(exc) == "assert not 2 < 3"
    assert not hasattr(exc, "_aye_assert")


# ------------------------------------------------------------ boolean ops


def test_boolean_operators_short_circuit_and_explain_what_ran():
    source = """
        calls = []
        def f(v):
            calls.append(v)
            return v
        def test_and():
            assert f(0) and f(1)
        def test_or():
            assert f(0) or f('') or f(None)
        def test_or_passes():
            assert f(0) or f(2) or f(3)
    """
    ns = namespace(source)
    with pytest.raises(AssertionError) as info:
        ns["test_and"]()
    assert str(info.value) == "assert (0)\n +  where 0 = f(0)"
    assert ns["calls"] == [0]
    del ns["calls"][:]
    with pytest.raises(AssertionError) as info:
        ns["test_or"]()
    assert ns["calls"] == [0, "", None]
    assert str(info.value).splitlines()[0] == "assert (0 or '' or None)"
    del ns["calls"][:]
    ns["test_or_passes"]()
    assert ns["calls"] == [0, 2]


def test_an_and_carries_the_comparison_that_decided_it():
    exc = failure("def test():\n    x, y = 1, 3\n    assert x == 1 and y == 2")
    assert operands(exc) == {
        "op": "==",
        "left": 3,
        "right": 2,
        "expr": "x == 1 and y == 2",
    }
    exc = failure("def test():\n    x, y = 0, 3\n    assert x == 1 and y == 2")
    assert exc._aye_assert["left"] == 0
    # Decided by an operand that is no comparison: nothing to compare.
    exc = failure("def test():\n    x, y = 1, 0\n    assert x == 1 and y")
    assert not hasattr(exc, "_aye_assert")
    # ``or``: every operand failed, none decided.
    exc = failure("def test():\n    x = 3\n    assert x == 1 or x == 2")
    assert not hasattr(exc, "_aye_assert")
    for test in ("not x == 3", "x - 3", "f(x == 1)"):
        exc = failure(f"def f(v):\n    return v\ndef test():\n    x = 3\n    assert {test}")
        assert not hasattr(exc, "_aye_assert"), test


# ------------------------------------------------------- calls, attributes


def test_calls_and_attributes_show_intermediate_values():
    source = """
        class Box:
            def __init__(self, items):
                self.items = items
            def count(self):
                return len(self.items)
        def make(*items, **kw):
            return Box(list(items))
        def test():
            assert make(1, 2, three=3).count() == 3
    """
    assert addresses(str(failure(source))) == (
        "assert 2 == 3\n"
        " +  where 2 = count()\n"
        " +    where count = <test_sample.Box object at 0x?>.count\n"
        " +      where <test_sample.Box object at 0x?> = make(1, 2, three=3)"
    )


def test_a_name_shows_its_value_unless_it_names_a_global_callable():
    source = """
        import os
        LIMIT = 3
        def helper():
            return 5
        def test():
            assert helper() < LIMIT
    """
    assert str(failure(source)) == "assert 5 < 3\n +  where 5 = helper()"


# ---------------------------------------------------------------- messages


def test_the_users_message_comes_first():
    exc = failure("def test():\n    x = 1\n    assert x == 2, 'x must be 2'")
    assert str(exc) == "x must be 2\nassert 1 == 2"
    assert exc._aye_assert["op"] == "=="
    exc = failure("def test():\n    assert False, 'one\\ntwo'")
    assert str(exc) == "one\n  two\nassert False"


def test_the_message_is_evaluated_only_on_failure():
    source = """
        calls = []
        def message():
            calls.append(1)
            return 'why'
        def test_passes():
            assert True, message()
        def test_fails():
            assert False, message()
    """
    ns = namespace(source)
    ns["test_passes"]()
    assert ns["calls"] == []
    with pytest.raises(AssertionError, match="why"):
        ns["test_fails"]()
    assert ns["calls"] == [1]


def test_a_failure_to_explain_still_fails_the_assert(monkeypatch):
    def broken(value):
        raise ValueError("boom")

    monkeypatch.setattr(aye_rewrite, "_repr", broken)
    exc = failure("def test():\n    x = 1\n    assert x == 2, 'why'")
    assert str(exc) == "why\nassert x == 2 (aye could not explain it: ValueError: boom)"
    assert operands(exc) == {"op": "==", "left": 1, "right": 2, "expr": "x == 2"}


def test_an_exception_in_the_message_propagates():
    ns = namespace("def test():\n    assert False, 1 / 0")
    with pytest.raises(ZeroDivisionError):
        ns["test"]()


# ------------------------------------------------- evaluated once, in order


def test_walrus_and_side_effects_are_evaluated_once():
    source = """
        calls = []
        def f(v):
            calls.append(v)
            return v
        def test_walrus():
            assert (x := f(1)) == 2
        def test_walrus_passes():
            assert (x := f(2)) == 2
            return x
        def test_args():
            assert f(f(1) + f(2)) == f(4)
        def test_rebind():
            x = 1
            assert x == (x := 2)
    """
    ns = namespace(source)
    with pytest.raises(AssertionError, match=r"^assert 1 == 2$"):
        ns["test_walrus"]()
    # pytest's rewriting calls f(1) three times here.
    assert ns["calls"] == [1]
    del ns["calls"][:]
    assert ns["test_walrus_passes"]() == 2
    assert ns["calls"] == [2]
    del ns["calls"][:]
    with pytest.raises(AssertionError) as info:
        ns["test_args"]()
    assert ns["calls"] == [1, 2, 3, 4]
    assert str(info.value).splitlines()[:2] == ["assert 3 == 4", " +  where 3 = f((1 + 2))"]
    with pytest.raises(AssertionError, match=r"^assert 1 == 2$"):
        ns["test_rebind"]()


def test_properties_and_dunders_run_once():
    source = """
        class Counted:
            def __init__(self):
                self.reads = 0
                self.compares = 0
                self.bools = 0
            @property
            def value(self):
                self.reads += 1
                return self
            def __eq__(self, other):
                self.compares += 1
                return self.result
            def __bool__(self):
                self.bools += 1
                return self.truth
            __hash__ = None
        class Result:
            def __init__(self, truth):
                self.truth, self.bools = truth, 0
            def __bool__(self):
                self.bools += 1
                return self.truth
        def check(c):
            assert c.value == 1
    """
    for ns in (namespace(source), plain(source)):
        counted = ns["Counted"]()
        counted.result = ns["Result"](True)
        ns["check"](counted)
        assert (counted.reads, counted.compares, counted.result.bools) == (1, 1, 1)


TRUTH_PRELUDE = """
class Truth:
    def __init__(self, name, value, log):
        self.name, self.value, self.log = name, value, log
    def __bool__(self):
        self.log.append(self.name)
        return self.value
    def __repr__(self):
        return self.name
class Link:
    def __init__(self, result):
        self.result = result
    def __lt__(self, other):
        return self.result
def f(v):
    return v
"""

TRUTH_SHAPES = [
    "a",
    "not a",
    "not not a",
    "a and b",
    "a or b",
    "a and b and c",
    "a or b or c",
    "(a or b) and c",
    "a and (b or c)",
    "not (a and b)",
    "not (a or b)",
    "not (a and (b or not c))",
    "l1 < l2",
    "l1 < l2 < l3",
    "not (l1 < l2 < l3)",
    "l1 < l2 < l3 and a",
    "a or l1 < l2 < l3",
    "a and l1 < l2",
    "f(a and b)",
    "f(a or b) and c",
    "(a and b) is not None",
    "a if b else c",
]


@pytest.mark.parametrize("shape", TRUTH_SHAPES)
def test_truth_is_tested_as_often_as_python_tests_it(shape):
    """``__bool__`` runs exactly as many times, on the same objects, as in
    Python's own assert (which tests each operand of an ``and``/``or`` or a
    chain once), passing or failing."""
    source = f"{TRUTH_PRELUDE}def check(a, b, c, l1, l2, l3):\n    assert {shape}\n"
    rewritten, python = namespace(source), plain(source)

    def run(ns, truths):
        log = []
        a, b, c, first, second = (
            ns["Truth"](name, truth, log) for name, truth in zip("abcAB", truths, strict=True)
        )
        try:
            ns["check"](a, b, c, ns["Link"](first), ns["Link"](second), ns["Link"](None))
        except AssertionError:
            return "failed", log
        return "passed", log

    for truths in itertools.product([True, False], repeat=5):
        assert run(rewritten, truths) == run(python, truths), truths


# ----------------------------------------------- passing asserts unchanged

PASSING = [
    "x = 1\nassert x == 1",
    "def f(a, b=2):\n    assert a < b, 'msg'\n    assert a and b\n    return locals()\nr = f(1)",
    "def f():\n    v = [1, 2]\n    assert (n := len(v)) == 2 and n\n    return n, sorted(locals())\nr = f()",
    "class C:\n    a = 1\n    assert a == 1\n    b = [a]\n    assert b\nr = sorted(vars(C))",
    (
        "def f():\n    out = []\n    for i in range(5):\n        assert i < 5 or out.append(i)\n"
        "        out.append(i)\n    return out\nr = f()"
    ),
    "def g():\n    x = yield 1\n    assert x == 2\n    yield x\ngen = g()\nr = [next(gen), gen.send(2)]",
    (
        "def f():\n    assert not [] and {} == {} and 1 in {1} and None is None\n"
        "    assert 0 < 1 <= 1 < 2 != 3\n    return True\nr = f()"
    ),
    "import math\nassert math.isclose(0.1 + 0.2, 0.3)\nr = math.pi",
    "def f(*a, **k):\n    assert a == (1,) and k == {'b': 2}\n    return a, k\nr = f(1, b=2)",
    (
        "async def f():\n    return 1\nasync def g():\n    assert await f() == 1\n    return 2\n"
        "import asyncio\nr = asyncio.run(g())"
    ),
    "r = []\nassert (lambda: r.append(1) or True)()\nassert r == [1]",
]


@pytest.mark.parametrize("source", PASSING)
def test_passing_asserts_behave_as_pythons(source):
    rewritten = namespace(source)
    expected = plain(source)
    helpers = {"@aye_rewrite", "@aye_builtins"}
    assert set(rewritten) - helpers == set(expected)
    assert rewritten.get("r") == expected.get("r")
    assert repr(rewritten.get("r")) == repr(expected.get("r"))


def test_no_temporaries_are_left_behind():
    ns = namespace("""
        def f():
            a = [1]
            assert a == [1] and len(a) == 1 or None
            return sorted(locals())
        class C:
            x = 1
            assert x == 1
        assert C.x == 1
    """)
    assert ns["f"]() == ["a"]
    assert not [name for name in vars(ns["C"]) if name.startswith("@")]
    assert sorted(name for name in ns if name.startswith("@")) == ["@aye_builtins", "@aye_rewrite"]


def test_a_module_without_asserts_is_left_alone():
    ns = namespace("x = 1\n")
    assert not [name for name in ns if name.startswith("@")]


# ---------------------------------------------- nested functions and classes


def test_asserts_are_rewritten_wherever_they_are():
    source = """
        import contextlib
        def outer():
            def inner(v):
                assert v == 1
            return inner
        class Outer:
            class Inner:
                def method(self):
                    assert self == 0
            @classmethod
            def cls_method(cls):
                assert cls is None
            @staticmethod
            def static():
                return (lambda v: [v][0])
        def in_with():
            with contextlib.nullcontext(2) as v:
                assert v == 1
        def in_finally():
            try:
                pass
            finally:
                assert 'finally' == 1
        def in_match(v):
            match v:
                case [x]:
                    assert x == 1
        def in_while():
            n = 0
            while True:
                n += 1
                if n > 2:
                    assert n == 1
    """
    ns = namespace(source)
    checks = [
        (lambda: ns["outer"]()(2), "assert 2 == 1"),
        (lambda: ns["Outer"].Inner().method(), "assert <test_sample.Outer.Inner object"),
        (ns["Outer"].cls_method, "assert <class 'test_sample.Outer'> is None"),
        (ns["in_with"], "assert 2 == 1"),
        (ns["in_finally"], "assert 'finally' == 1"),
        (lambda: ns["in_match"]([5]), "assert 5 == 1"),
        (ns["in_while"], "assert 3 == 1"),
    ]
    for call, start in checks:
        with pytest.raises(AssertionError) as info:
            call()
        assert str(info.value).startswith(start)


def test_a_failing_class_body_or_module_assert_raises_at_definition():
    with pytest.raises(AssertionError, match=r"^assert 1 == 2$"):
        namespace("class C:\n    x = 1\n    assert x == 2")
    with pytest.raises(AssertionError, match=r"^assert 1 == 2$"):
        namespace("x = 1\nassert x == 2")


def test_super_works_in_an_assert():
    source = """
        class A:
            def m(self):
                return 1
        class B(A):
            def m(self):
                assert super().m() == 1
                assert super().m() == 2
    """
    ns = namespace(source)
    with pytest.raises(AssertionError, match=r"^assert 1 == 2\n \+  where 1 = m\(\)"):
        ns["B"]().m()


def test_future_imports_and_docstrings_stay_in_place():
    ns = namespace('''
        """The docstring."""
        from __future__ import annotations
        def f(x: Undefined) -> AlsoUndefined:
            assert x == 1
    ''')
    assert ns["__doc__"] == "The docstring."
    assert ns["f"].__annotations__ == {"x": "Undefined", "return": "AlsoUndefined"}
    with pytest.raises(AssertionError, match=r"^assert 2 == 1$"):
        ns["f"](2)


def test_dont_rewrite_marker_leaves_the_module_alone():
    ns = namespace('"""PYTEST_DONT_REWRITE"""\ndef test():\n    assert 1 == 2')
    with pytest.raises(AssertionError) as info:
        ns["test"]()
    assert str(info.value) == ""
    assert "@aye_rewrite" not in ns


# ------------------------------------------------- generators and async


def test_asserts_in_generators_and_coroutines():
    source = """
        async def value():
            return 2
        async def test_await():
            assert await value() == 3
        async def agen():
            yield 1
            assert await value() == 4
        async def test_async_generator():
            async for _ in agen():
                pass
        def gen():
            received = yield
            assert received == 'expected'
        def test_yield_in_assert():
            def g():
                assert (yield 1) == 2
            it = g()
            next(it)
            it.send(3)
    """
    ns = namespace(source)
    with pytest.raises(AssertionError, match=r"^assert 2 == 3$"):
        asyncio.run(ns["test_await"]())
    with pytest.raises(AssertionError, match=r"^assert 2 == 4$"):
        asyncio.run(ns["test_async_generator"]())
    it = ns["gen"]()
    next(it)
    with pytest.raises(AssertionError, match=r"^assert 'got' == 'expected'$"):
        it.send("got")
    with pytest.raises(AssertionError, match=r"^assert 3 == 2$"):
        ns["test_yield_in_assert"]()


# ------------------------------------------------------- tuple warning


def test_an_assert_on_a_tuple_warns_and_always_passes():
    source = "def test():\n    x = 0\n    assert (x, 'never fails')\n"
    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        ns = namespace(source)
    assert [(w.category, str(w.message), w.filename, w.lineno) for w in caught] == [
        (SyntaxWarning, "assertion is always true, perhaps remove parentheses?", FILENAME, 3)
    ]
    ns["test"]()


def test_the_tuple_warning_takes_the_front_ends_category():
    class RewriteWarning(UserWarning):
        pass

    with warnings.catch_warnings(record=True) as caught:
        warnings.simplefilter("always")
        namespace("assert (1, 2)", warning=RewriteWarning)
    assert [w.category for w in caught] == [RewriteWarning]
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        with pytest.raises(RewriteWarning):
            namespace("assert (1, 2)", warning=RewriteWarning)


def test_an_empty_tuple_fails_without_warning():
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        exc = failure("def test():\n    assert ()")
    assert str(exc) == "assert ()"


# ------------------------------------------------------------------ unicode


def test_unicode_sources_and_values():
    source = "# -*- coding: latin-1 -*-\ndef test():\n    é = 'ü'\n    assert é == 'u'\n"
    code = rewrite_source(source.encode("latin-1"), FILENAME)
    ns = {}
    exec(code, ns)
    with pytest.raises(AssertionError) as info:
        ns["test"]()
    assert str(info.value) == "assert 'ü' == 'u'"
    exc = failure("def test():\n    s = 'naïve ✓'\n    assert s + '✓' == '日本'")
    assert operands(exc) == {
        "op": "==",
        "left": "naïve ✓✓",
        "right": "日本",
        "expr": "s + '✓' == '日本'",
    }
    assert str(exc) == "assert ('naïve ✓' + '✓') == '日本'"


def test_a_multiline_test_is_carried_as_written():
    exc = failure("def test():\n    x = 1\n    assert (\n        x\n        == 2\n    ), 'm'")
    assert exc._aye_assert["expr"] == "x\n        == 2"


# ------------------------------------------------------- the expected side

LITERALS = [
    "1",
    "-1.5",
    "'text'",
    "b'bytes'",
    "None",
    "True",
    "...",
    "[1, (2, 3), {4}]",
    "{'a': [1, -2], 'b': None}",
    "()",
    "MAX_SIZE",
    "config.DEFAULT_TIMEOUT",
    "Color.RED",
    "Decimal('9.45')",
    "Point(1, y=2)",
    "shop.Point(1, 2)",
    "Cart()",
    "dict(a=1)",
    "frozenset([1, 2])",
    "datetime(2026, 10, 11, 9, 30)",
    "dt.date(2026, 10, 11)",
    "[Point(1, 2), LIMIT]",
]
NOT_LITERALS = [
    "x",
    "cart.summary()",
    "N",
    "_",
    "config.timeout",
    "f(1)",
    "[x]",
    "{'a': x}",
    "{**base}",
    "[*items]",
    "Point(x, 2)",
    "Point(*args)",
    "dict(**kw)",
    "f'{x}'",
    "1 + 2",
    "not True",
    "x[0]",
    "(lambda: 1)()",
    "make()(1)",
]


@pytest.mark.parametrize("source", LITERALS)
def test_literals_are_what_a_test_expects(source):
    assert aye_rewrite.is_literal(ast.parse(source, mode="eval").body)


@pytest.mark.parametrize("source", NOT_LITERALS)
def test_anything_else_is_what_a_test_got(source):
    assert not aye_rewrite.is_literal(ast.parse(source, mode="eval").body)


def test_too_deep_a_literal_is_none():
    deep = ast.Constant(1)
    for _ in range(sys.getrecursionlimit() * 2):
        deep = ast.List([deep], ast.Load())
    assert not aye_rewrite.is_literal(deep)


def test_a_failure_says_which_operand_was_expected():
    sides = {
        "x == 2": "right",
        "2 == x": "left",
        "x == MAX": "right",
        "x == y": None,
        "1 == 2": "right",
        "x < 0 or x > 5": None,
    }
    for test, expected in sides.items():
        exc = failure(f"MAX = 3\ndef test():\n    x, y = 1, 2\n    assert {test}")
        found = getattr(exc, "_aye_assert", {}).get("expected")
        assert found == expected, test


def test_a_failure_carries_each_operands_source_and_span():
    source = 'def test():\n    cart = {"pear": 2}\n    assert cart["pear"] == {"pear": 3}["pear"]'
    exc = failure(source)
    line = source.splitlines()[-1]
    data = exc._aye_assert
    assert (data["left_src"], data["right_src"]) == ('cart["pear"]', '{"pear": 3}["pear"]')
    assert line[slice(*data["left_cols"])] == 'cart["pear"]'
    assert line[slice(*data["right_cols"])] == '{"pear": 3}["pear"]'
    assert data["expected"] is None
    assert line[slice(*exc._aye_span)] == 'cart["pear"] == {"pear": 3}["pear"]'


def test_spans_are_in_characters_on_the_asserts_first_line():
    exc = failure("def test():\n    café = 'é'\n    assert café == 'e'")
    assert (exc._aye_assert["left_cols"], exc._aye_assert["right_cols"]) == ((11, 15), (19, 22))
    assert exc._aye_span == (11, 22)
    # An operand that goes on past the first line ends at its end; one that
    # starts later has no span.
    exc = failure("def test():\n    x = [1]\n    assert x == [\n        2,\n    ]  # end")
    assert exc._aye_assert["right_cols"] == (16, 17)
    assert exc._aye_span == (11, 17)
    exc = failure("def test():\n    x = 1\n    assert x == (\n        2\n    )")
    assert exc._aye_assert["left_cols"] == (11, 12)
    assert exc._aye_assert["right_cols"] is None


def test_a_chain_and_an_and_carry_the_failing_links_sides():
    exc = failure("def test():\n    x = 3\n    assert 1 < x < 2")
    data = exc._aye_assert
    assert (data["left_src"], data["right_src"], data["expected"]) == ("x", "2", "right")
    exc = failure("def test():\n    a, b = 1, 3\n    assert a == 1 and b == 2")
    data = exc._aye_assert
    assert (data["left_src"], data["right_src"]) == ("b", "2")
    assert data["left_cols"] == (22, 23)


def test_an_assert_that_compares_nothing_still_has_its_span():
    exc = failure("def test():\n    ok = lambda: False\n    assert ok(), 'not ok'")
    assert not hasattr(exc, "_aye_assert")
    assert exc._aye_span == (11, 15)


def test_long_operand_sources_are_cut():
    big = "[" + ", ".join(["1"] * 600) + "]"
    exc = failure(f"def test():\n    assert {big} == 2")
    source = exc._aye_assert["left_src"]
    assert len(source) == aye_rewrite.MAX_SOURCE and source.endswith("…")


# ------------------------------------------------------------- long values


def test_long_reprs_are_bounded():
    exc = failure(
        "def test():\n    x = 'a' * 100_000\n    y = list(range(100_000))\n    assert x == y"
    )
    message = str(exc)
    assert len(message) < 2 * aye_rewrite.MAX_REPR + 20
    assert "..." in message
    exc = failure(
        "class R:\n    def __repr__(self):\n        return 'r' * 100_000\n"
        "def test():\n    assert R() == 1"
    )
    assert all(len(line) < 2 * aye_rewrite.MAX_REPR + 20 for line in str(exc).splitlines())


def test_saferepr():
    assert saferepr("x" * 1000).startswith("'xxx")
    assert len(saferepr("x" * 1000)) == aye_rewrite.MAX_REPR
    assert len(saferepr("x" * 1000, maxsize=None)) == 1002
    assert saferepr({"b": 1, "a": 2}) == "{'b': 1, 'a': 2}"

    class Broken:
        def __repr__(self):
            raise RuntimeError("no")

    assert saferepr(Broken()).startswith(
        "<[RuntimeError('no') raised in repr()] Broken object at 0x"
    )


def test_format_explanation():
    assert format_explanation("assert 1\n{1 = f()\n} == 2") == "assert 1 == 2\n +  where 1 = f()"
    assert format_explanation("a\n~b\n>assert c") == "a\n  b\nassert c"
    assert format_explanation("a\nb") == "a\\nb"
    assert (
        format_explanation("x\n{y\n{z\n}\n}\n{w\n}") == "x\n +  where y\n +    where z\n +  and   w"
    )
    assert format_explanation("unbalanced\n}") == "unbalanced"


# ------------------------------------------------------------- tracebacks


def test_a_failure_is_raised_in_the_users_frame_at_the_assert():
    source = "def test():\n    x = 1\n\n    assert (\n        x == 2\n    )\n"
    ns = namespace(source)
    with pytest.raises(AssertionError) as info:
        ns["test"]()
    last = traceback.extract_tb(info.value.__traceback__)[-1]
    assert (last.filename, last.name, last.lineno) == (FILENAME, "test", 4)


def test_an_operands_exception_propagates_from_its_line():
    source = "def f():\n    raise KeyError('k')\ndef test():\n    assert (\n        1 ==\n        f()\n    )\n"
    for ns in (namespace(source), plain(source)):
        with pytest.raises(KeyError) as info:
            ns["test"]()
        frames = traceback.extract_tb(info.value.__traceback__)
        assert [(f.name, f.lineno) for f in frames[-2:]] == [("test", 6), ("f", 2)]


def test_too_deep_to_explain_falls_back_to_pythons_assert():
    # Deep enough for the rewriter (two frames a level), not for Python.
    depth = sys.getrecursionlimit() * 2 // 3
    source = "def test():\n    assert " + " + ".join(["1"] * depth) + " == 0\n"
    plain(source)
    ns = namespace(source)
    with pytest.raises(AssertionError) as info:
        ns["test"]()
    assert str(info.value) == ""


# ------------------------------------------------------ explain_comparison


def test_explain_comparison_replaces_the_comparison(monkeypatch):
    seen = []

    def explain(op, left, right):
        seen.append((op, left, right))
        return [f"{left!r} {op} {right!r}", "", "100% different", "multi\nline"]

    monkeypatch.setattr(aye_rewrite, "explain_comparison", explain)
    exc = failure("def test():\n    x = 3\n    assert 1 < x < 2")
    assert seen == [("<", 3, 2)]
    assert str(exc) == "assert 3 < 2\n  \n  100% different\n  multi\\nline"
    monkeypatch.setattr(aye_rewrite, "explain_comparison", lambda op, left, right: None)
    assert str(failure("def test():\n    assert 1 == 2")) == "assert 1 == 2"

    def broken(op, left, right):
        raise RuntimeError

    monkeypatch.setattr(aye_rewrite, "explain_comparison", broken)
    assert str(failure("def test():\n    assert 1 == 2")) == "assert 1 == 2"


# ---------------------------------------------------------------- install


@pytest.fixture
def project(tmp_path, monkeypatch):
    """A directory on ``sys.path`` whose modules are forgotten afterwards,
    imported without pytest's own rewriting hook (that runs these tests)."""
    monkeypatch.syspath_prepend(str(tmp_path))
    hooks = [f for f in sys.meta_path if not isinstance(f, pytest_rewrite.AssertionRewritingHook)]
    monkeypatch.setattr(sys, "meta_path", hooks)
    before = set(sys.modules)
    yield tmp_path
    aye_rewrite.uninstall()
    for name in set(sys.modules) - before:
        del sys.modules[name]
    importlib.invalidate_caches()


FAILING = "def check():\n    x = 1\n    assert x == 2\n"


def message(module):
    with pytest.raises(AssertionError) as info:
        module.check()
    return str(info.value)


def test_install_rewrites_the_modules_the_predicate_accepts(project):
    (project / "test_one.py").write_text(FAILING)
    (project / "helper_one.py").write_text(FAILING)
    seen = []

    def should_rewrite(path):
        seen.append(path)
        return os.path.basename(path).startswith("test_")

    aye_rewrite.install(should_rewrite)
    assert sys.meta_path[0] is aye_rewrite._finder
    rewritten = importlib.import_module("test_one")
    untouched = importlib.import_module("helper_one")
    assert message(rewritten) == "assert 1 == 2"
    assert message(untouched) == ""
    assert str(project / "test_one.py") in seen
    assert all(os.path.isabs(path) for path in seen)
    assert isinstance(rewritten.__loader__, aye_rewrite.RewritingLoader)
    assert rewritten.__file__ == str(project / "test_one.py")
    # The source stays readable, for tracebacks and ``inspect``.
    assert inspect.getsource(rewritten) == FAILING
    # Python's own bytecode is never cached for a rewritten module: only
    # aye's rewritten code, apart.
    cache = project / "__pycache__"
    assert sorted(p.name for p in cache.iterdir() if "test_one" in p.name) == [
        f"test_one.{sys.implementation.cache_tag}-aye.pyc"
    ]
    assert not os.path.exists(importlib.util.cache_from_source(str(project / "test_one.py")))


def test_install_is_idempotent_and_the_last_settings_win(project):
    (project / "test_two.py").write_text(FAILING)
    aye_rewrite.install(lambda path: False)
    sys.meta_path.insert(0, object())
    try:
        aye_rewrite.install(lambda path: True)
        assert sys.meta_path[0] is aye_rewrite._finder
        assert sum(f is aye_rewrite._finder for f in sys.meta_path) == 1
    finally:
        sys.meta_path[:] = [f for f in sys.meta_path if type(f) is not object]
    assert message(importlib.import_module("test_two")) == "assert 1 == 2"
    aye_rewrite.uninstall()
    assert aye_rewrite._finder not in sys.meta_path


def test_uninstall_forgets_the_settings(project):
    """What the finder held (a front-end's predicate, its session) goes with
    it: later imports are Python's own, and nothing is kept alive."""

    class Predicate:
        def __call__(self, path):
            return True

    predicate = Predicate()
    held = weakref.ref(predicate)
    aye_rewrite.install(predicate, modules=["test_after"])
    aye_rewrite.uninstall()
    del predicate
    gc.collect()
    assert held() is None
    assert aye_rewrite._finder not in sys.meta_path
    (project / "test_after.py").write_text(FAILING)
    assert message(importlib.import_module("test_after")) == ""
    # Installing again sets them again.
    aye_rewrite.install(lambda path: True)
    assert message(forget("test_after")) == "assert 1 == 2"


def test_install_rewrites_named_modules_and_packages(project):
    package = project / "helpers_pkg"
    package.mkdir()
    (package / "__init__.py").write_text(FAILING)
    (package / "sub.py").write_text(FAILING)
    (project / "loose.py").write_text(FAILING)
    names = []
    aye_rewrite.install(lambda path: False, modules=names)
    names.append("helpers_pkg")  # after install: read at each import
    assert message(importlib.import_module("helpers_pkg")) == "assert 1 == 2"
    assert message(importlib.import_module("helpers_pkg.sub")) == "assert 1 == 2"
    assert message(importlib.import_module("loose")) == ""


def test_imports_made_while_deciding_are_not_claimed(project):
    (project / "test_three.py").write_text(FAILING)
    (project / "test_dep.py").write_text(FAILING)

    def should_rewrite(path):
        importlib.import_module("test_dep")
        return True

    aye_rewrite.install(should_rewrite)
    assert message(importlib.import_module("test_three")) == "assert 1 == 2"
    assert message(sys.modules["test_dep"]) == ""


def test_reload_rewrites_again(project):
    path = project / "test_reload.py"
    path.write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    module = importlib.import_module("test_reload")
    path.write_text(FAILING.replace("x == 2", "x == 3"))
    importlib.invalidate_caches()
    module = importlib.reload(module)
    assert message(module) == "assert 1 == 3"


def forget(name):
    """Import ``name`` afresh, as another worker would."""
    sys.modules.pop(name, None)
    importlib.invalidate_caches()
    return importlib.import_module(name)


@pytest.fixture
def rewrites(monkeypatch):
    """The paths rewritten from source (not loaded from aye's cache)."""
    seen = []
    real = aye_rewrite._rewrite

    def counting(source, filename, warning):
        seen.append(filename)
        return real(source, filename, warning)

    monkeypatch.setattr(aye_rewrite, "_rewrite", counting)
    return seen


def cached_files(project, stem):
    cache = project / "__pycache__"
    return (
        sorted(p.name for p in cache.iterdir() if p.name.startswith(f"{stem}."))
        if cache.exists()
        else []
    )


def test_rewritten_code_is_cached_and_reused(project, rewrites):
    path = project / "test_cached.py"
    path.write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    assert message(importlib.import_module("test_cached")) == "assert 1 == 2"
    assert rewrites == [str(path)]
    assert cached_files(project, "test_cached") == [
        f"test_cached.{sys.implementation.cache_tag}-aye.pyc"
    ]
    module = forget("test_cached")
    assert rewrites == [str(path)]  # from the cache
    assert message(module) == "assert 1 == 2"
    assert module.check.__code__.co_filename == str(path)


def test_the_cache_answers_only_for_the_same_bytes(project, rewrites):
    path = project / "test_edited.py"
    path.write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    importlib.import_module("test_edited")
    stat = path.stat()
    # Same size, same modification time, other bytes: rewritten again.
    path.write_text(FAILING.replace("x == 2", "x == 3"))
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    assert message(forget("test_edited")) == "assert 1 == 3"
    assert len(rewrites) == 2


def test_the_cache_answers_only_for_this_rewriter_and_path(project, rewrites, monkeypatch):
    path = project / "test_other.py"
    path.write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    importlib.import_module("test_other")
    monkeypatch.setattr(aye_rewrite, "_REWRITER", b"another rewriter")
    assert message(forget("test_other")) == "assert 1 == 2"
    assert len(rewrites) == 2
    # The same bytes at another path (a moved project): its frames name it.
    moved = project / "moved"
    moved.mkdir()
    shutil.copytree(project / "__pycache__", moved / "__pycache__")
    shutil.copy(path, moved / "test_other.py")
    monkeypatch.syspath_prepend(str(moved))
    module = forget("test_other")
    assert module.check.__code__.co_filename == str(moved / "test_other.py")
    assert len(rewrites) == 3


def test_a_damaged_cache_is_rewritten(project, rewrites):
    path = project / "test_damaged.py"
    path.write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    importlib.import_module("test_damaged")
    (cached,) = (project / "__pycache__").glob("test_damaged.*-aye.pyc")
    cached.write_bytes(cached.read_bytes()[:-7])
    assert message(forget("test_damaged")) == "assert 1 == 2"
    assert len(rewrites) == 2
    # Mended: the next import reads it.
    assert message(forget("test_damaged")) == "assert 1 == 2"
    assert len(rewrites) == 2


def test_a_module_whose_rewrite_warns_is_never_cached(project, rewrites):
    """Its warnings meet the filters of each import, as on the first: an
    always-true assert, and Python's compiler's own ``SyntaxWarning``."""
    (project / "test_tuple.py").write_text("def check():\n    assert (1, 'never fails')\n")
    (project / "test_literal.py").write_text("def check(x=1):\n    return x is 1\n")
    aye_rewrite.install(lambda p: True)
    for name, text in (
        ("test_tuple", "assertion is always true, perhaps remove parentheses?"),
        ("test_literal", "literal"),
    ):
        for _ in range(2):
            with warnings.catch_warnings(record=True) as caught:
                warnings.simplefilter("always")
                forget(name)
            (issued,) = [str(w.message) for w in caught if w.category is SyntaxWarning]
            assert text in issued
            with warnings.catch_warnings():
                warnings.simplefilter("error")
                with pytest.raises((SyntaxWarning, SyntaxError)):
                    forget(name)
        assert cached_files(project, name) == []


def test_nothing_is_cached_without_bytecode(project, rewrites, monkeypatch):
    monkeypatch.setattr(sys, "dont_write_bytecode", True)
    (project / "test_nowrite.py").write_text(FAILING)
    aye_rewrite.install(lambda p: True)
    assert message(importlib.import_module("test_nowrite")) == "assert 1 == 2"
    assert cached_files(project, "test_nowrite") == []


@pytest.mark.skipif(
    sys.platform == "win32" or getattr(os, "geteuid", lambda: 1)() == 0,
    reason="permissions",
)
def test_a_tree_that_cant_be_written_is_rewritten(project, rewrites):
    package = project / "readonly"
    package.mkdir()
    (package / "test_ro.py").write_text(FAILING)
    package.chmod(0o555)
    try:
        aye_rewrite.install(lambda p: True)
        assert message(importlib.import_module("readonly.test_ro")) == "assert 1 == 2"
        assert message(forget("readonly.test_ro")) == "assert 1 == 2"
    finally:
        package.chmod(0o755)
    assert len(rewrites) == 2
    assert not (package / "__pycache__").exists()


def test_rewriting_loader_imports_by_spec(tmp_path):
    path = tmp_path / "by_spec.py"
    path.write_text(FAILING)
    loader = aye_rewrite.RewritingLoader("by_spec", str(path))
    spec = importlib.util.spec_from_file_location("by_spec", str(path), loader=loader)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    assert message(module) == "assert 1 == 2"


# ------------------------------------------------------------- processes


def run_python(code, *flags):
    return subprocess.run(
        [sys.executable, *flags, "-c", code],
        capture_output=True,
        text=True,
        check=False,
        cwd=PYTHON_DIR,
        env={**os.environ, "PYTHONPATH": PYTHON_DIR},
    )


def test_imports_no_pytest():
    code = textwrap.dedent("""
        import sys
        sys.modules["pytest"] = None
        sys.modules["_pytest"] = None
        import aye_rewrite
        ns = {}
        exec(aye_rewrite.rewrite_source("def t():\\n    assert [1] == [2]\\n", "t.py"), ns)
        try:
            ns["t"]()
        except AssertionError as e:
            print(e)
        print(sorted(m for m in sys.modules if "pytest" in m))
    """)
    result = run_python(code)
    assert result.returncode == 0, result.stderr
    assert result.stdout.splitlines() == ["assert [1] == [2]", "['_pytest', 'pytest']"]


def test_rewritten_asserts_run_under_optimize():
    code = textwrap.dedent("""
        import aye_rewrite
        ns = {}
        exec(aye_rewrite.rewrite_source("def t():\\n    assert 1 == 2\\n", "t.py"), ns)
        try:
            ns["t"]()
        except AssertionError as e:
            print("raised:", e)
        else:
            print("passed")
    """)
    result = run_python(code, "-O")
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "raised: assert 1 == 2"
