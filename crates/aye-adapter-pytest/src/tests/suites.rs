//! Scratch suites checked against pytest 9.1.1. The expected ids are
//! `pytest --collect-only -q` output, copied verbatim.

pub(super) struct Suite {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    /// What pytest 9.1.1 collects, in order.
    pub pytest: &'static [&'static str],
    /// Files recognition sends for refinement.
    pub refined: &'static [&'static str],
}

pub(super) const SUITES: &[Suite] = &[
    // stacked parametrize, inheritance order, duplicate definitions
    Suite {
        name: "basics",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_basic.py",
                r#"import pytest

@pytest.mark.parametrize("x", [0, 1])
@pytest.mark.parametrize("y", [2, 3])
def test_stack(x, y):
    pass

class TestB:
    def test_b2(self): pass
    def test_b1(self): pass

class TestC(TestB):
    def test_c(self): pass
    def test_b1(self): pass

def test_dup(): pass
def test_other(): pass
def test_dup(): pass

"#,
            ),
        ],
        pytest: &[
            "test_basic.py::test_stack[2-0]",
            "test_basic.py::test_stack[2-1]",
            "test_basic.py::test_stack[3-0]",
            "test_basic.py::test_stack[3-1]",
            "test_basic.py::TestB::test_b2",
            "test_basic.py::TestB::test_b1",
            "test_basic.py::TestC::test_b2",
            "test_basic.py::TestC::test_c",
            "test_basic.py::TestC::test_b1",
            "test_basic.py::test_dup",
            "test_basic.py::test_other",
        ],
        refined: &[],
    },
    // values pytest turns into ids: floats, strings, bytes, duplicates, ids=, empty sets
    Suite {
        name: "ids",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_ids.py",
                r#"import pytest

@pytest.mark.parametrize("v", [1.0, 1e16, 1e-5, 0.1, 1e22, 123456789.0, 1.5e-7, float("inf"), -0.0, 2.5, 1e15, 1e+16, 0.0001, 0.00001, 3.14159265358979])
def test_floats(v): pass

@pytest.mark.parametrize("v", ["é", "日本", "\t\n\r", "a\\b", "\x00\x7f", "😀", "", " ", "a b", "x::y", "[x]", "\u0080", "￿", "ß"])
def test_strs(v): pass

@pytest.mark.parametrize("v", [b"abc", b"\xc3\xa9", b"\t\n", b"\\", b"\x00\x7f", b""])
def test_bytes(v): pass

@pytest.mark.parametrize("v", [None, True, False, 0, -1, 1j, 1+2j, -1.5, (1, 2), [1], {"a": 1}, int, len])
def test_misc(v): pass

@pytest.mark.parametrize("a,b", [(1, 2), (3, 4)])
def test_two(a, b): pass

@pytest.mark.parametrize("a, b", [(1, "x"), pytest.param(3, "y", id="custom"), pytest.param(5, "z", marks=pytest.mark.slow)])
def test_param(a, b): pass

@pytest.mark.parametrize(("a", "b"), [[1, 2], (3, 4)])
def test_tuplenames(a, b): pass

@pytest.mark.parametrize(["a"], [(1,), (2,)])
def test_listnames(a): pass

@pytest.mark.parametrize("a,", [(1,), (2,)])
def test_trailing(a): pass

@pytest.mark.parametrize("x", [1, 1, 2, 1])
def test_dupint(x): pass

@pytest.mark.parametrize("x", ["a", "a", "a1", "a1", "b"])
def test_dupstr(x): pass

@pytest.mark.parametrize("x", ["a0", "a", "a"])
def test_dupclash(x): pass

@pytest.mark.parametrize("x", [1, 2, 3], ids=["one", None, "three"])
def test_ids_none(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=["same", "same"])
def test_ids_same(x): pass

@pytest.mark.parametrize("x", [])
def test_empty(x): pass

@pytest.mark.parametrize("x,y", [])
def test_empty2(x, y): pass

@pytest.mark.parametrize("x", [1, 2], ids=[1.5, True])
def test_ids_types(x): pass

@pytest.mark.parametrize("x", [pytest.param(1, id="é"), pytest.param(2, id="a\nb")])
def test_param_unicode(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=["é", b"\xc3"])
def test_ids_unicode(x): pass
"#,
            ),
        ],
        pytest: &[
            "test_ids.py::test_floats[1.0]",
            "test_ids.py::test_floats[1e+16_0]",
            "test_ids.py::test_floats[1e-05_0]",
            "test_ids.py::test_floats[0.1]",
            "test_ids.py::test_floats[1e+22]",
            "test_ids.py::test_floats[123456789.0]",
            "test_ids.py::test_floats[1.5e-07]",
            "test_ids.py::test_floats[inf]",
            "test_ids.py::test_floats[-0.0]",
            "test_ids.py::test_floats[2.5]",
            "test_ids.py::test_floats[1000000000000000.0]",
            "test_ids.py::test_floats[1e+16_1]",
            "test_ids.py::test_floats[0.0001]",
            "test_ids.py::test_floats[1e-05_1]",
            "test_ids.py::test_floats[3.14159265358979]",
            r"test_ids.py::test_strs[\xe9]",
            r"test_ids.py::test_strs[\u65e5\u672c]",
            r"test_ids.py::test_strs[\t\n\r]",
            r"test_ids.py::test_strs[a\\b]",
            r"test_ids.py::test_strs[\x00\x7f]",
            r"test_ids.py::test_strs[\U0001f600]",
            "test_ids.py::test_strs[]",
            "test_ids.py::test_strs[ ]",
            "test_ids.py::test_strs[a b]",
            "test_ids.py::test_strs[x::y]",
            "test_ids.py::test_strs[[x]]",
            r"test_ids.py::test_strs[\x80]",
            r"test_ids.py::test_strs[\uffff]",
            r"test_ids.py::test_strs[\xdf]",
            "test_ids.py::test_bytes[abc]",
            r"test_ids.py::test_bytes[\xc3\xa9]",
            r"test_ids.py::test_bytes[\t\n]",
            r"test_ids.py::test_bytes[\]",
            r"test_ids.py::test_bytes[\x00\x7f]",
            "test_ids.py::test_bytes[]",
            "test_ids.py::test_misc[None]",
            "test_ids.py::test_misc[True]",
            "test_ids.py::test_misc[False]",
            "test_ids.py::test_misc[0]",
            "test_ids.py::test_misc[-1]",
            "test_ids.py::test_misc[1j]",
            "test_ids.py::test_misc[(1+2j)]",
            "test_ids.py::test_misc[-1.5]",
            "test_ids.py::test_misc[v8]",
            "test_ids.py::test_misc[v9]",
            "test_ids.py::test_misc[v10]",
            "test_ids.py::test_misc[int]",
            "test_ids.py::test_misc[len]",
            "test_ids.py::test_two[1-2]",
            "test_ids.py::test_two[3-4]",
            "test_ids.py::test_param[1-x]",
            "test_ids.py::test_param[custom]",
            "test_ids.py::test_param[5-z]",
            "test_ids.py::test_tuplenames[1-2]",
            "test_ids.py::test_tuplenames[3-4]",
            "test_ids.py::test_listnames[1]",
            "test_ids.py::test_listnames[2]",
            "test_ids.py::test_trailing[1]",
            "test_ids.py::test_trailing[2]",
            "test_ids.py::test_dupint[1_0]",
            "test_ids.py::test_dupint[1_1]",
            "test_ids.py::test_dupint[2]",
            "test_ids.py::test_dupint[1_2]",
            "test_ids.py::test_dupstr[a0]",
            "test_ids.py::test_dupstr[a2]",
            "test_ids.py::test_dupstr[a1_0]",
            "test_ids.py::test_dupstr[a1_1]",
            "test_ids.py::test_dupstr[b]",
            "test_ids.py::test_dupclash[a0]",
            "test_ids.py::test_dupclash[a1]",
            "test_ids.py::test_dupclash[a2]",
            "test_ids.py::test_ids_none[one]",
            "test_ids.py::test_ids_none[2]",
            "test_ids.py::test_ids_none[three]",
            "test_ids.py::test_ids_same[same0]",
            "test_ids.py::test_ids_same[same1]",
            "test_ids.py::test_empty[NOTSET]",
            "test_ids.py::test_empty2[NOTSET]",
            "test_ids.py::test_ids_types[1.5]",
            "test_ids.py::test_ids_types[True]",
            r"test_ids.py::test_param_unicode[\xe9]",
            r"test_ids.py::test_param_unicode[a\nb]",
            r"test_ids.py::test_ids_unicode[\xe9]",
            r"test_ids.py::test_ids_unicode[\xc3]",
        ],
        refined: &[],
    },
    // class, module and nested parametrize; HIDDEN_PARAM; unittest; abstract and init classes
    Suite {
        name: "scopes",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_scopes.py",
                r#"import pytest
import unittest
from abc import ABC, abstractmethod
from dataclasses import dataclass

pytestmark = pytest.mark.parametrize("m", [1, 2])

@pytest.mark.parametrize("f", ["a", "b"])
def test_mod(m, f): pass

@pytest.mark.parametrize("c", [10, 20])
class TestCls:
    @pytest.mark.parametrize("f", ["x"])
    def test_meth(self, m, c, f): pass

    class TestNested:
        def test_n(self, m, c): pass

class TestHidden:
    @pytest.mark.parametrize("h", [pytest.param(1, id=pytest.HIDDEN_PARAM)])
    def test_h(self, m, h): pass

class Plain(unittest.TestCase):
    def test_z(self): pass
    def test_a(self): pass
    def helper(self): pass

class TestAbstract(ABC):
    @abstractmethod
    def foo(self): ...
    def test_abs(self, m): pass

@dataclass
class TestData:
    x: int = 1
    def test_dc(self, m): pass

class TestInit:
    def __init__(self): pass
    def test_i(self, m): pass

class TestOff:
    __test__ = False
    def test_off(self, m): pass

def test_lambda_alias(m): pass
test_lam = lambda m: None
"#,
            ),
        ],
        pytest: &[
            "test_scopes.py::test_mod[a-1]",
            "test_scopes.py::test_mod[a-2]",
            "test_scopes.py::test_mod[b-1]",
            "test_scopes.py::test_mod[b-2]",
            "test_scopes.py::TestCls::test_meth[x-10-1]",
            "test_scopes.py::TestCls::test_meth[x-10-2]",
            "test_scopes.py::TestCls::test_meth[x-20-1]",
            "test_scopes.py::TestCls::test_meth[x-20-2]",
            "test_scopes.py::TestCls::TestNested::test_n[10-1]",
            "test_scopes.py::TestCls::TestNested::test_n[10-2]",
            "test_scopes.py::TestCls::TestNested::test_n[20-1]",
            "test_scopes.py::TestCls::TestNested::test_n[20-2]",
            "test_scopes.py::TestHidden::test_h[1]",
            "test_scopes.py::TestHidden::test_h[2]",
            "test_scopes.py::Plain::test_a",
            "test_scopes.py::Plain::test_z",
            "test_scopes.py::test_lambda_alias[1]",
            "test_scopes.py::test_lambda_alias[2]",
            "test_scopes.py::test_lam[1]",
            "test_scopes.py::test_lam[2]",
        ],
        refined: &[],
    },
    // marks through class inheritance, aliases and pytest.param
    Suite {
        name: "marks",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_marks.py",
                r#"import pytest
from pytest import mark

param_a = pytest.mark.parametrize("a", [1, 2])
slow = mark.slow

@pytest.mark.parametrize("b", ["b1", "b2"])
class TestBase:
    def test_x(self, a, b, c, d): pass

@pytest.mark.parametrize("c", ["c1"])
@pytest.mark.parametrize("d", ["d1", "d2"])
class TestDerived(TestBase):
    pytestmark = [pytest.mark.parametrize("a", [7, 8])]

@param_a
@slow
def test_alias(a): pass

@pytest.mark.parametrize("x", [pytest.param(1, marks=[pytest.mark.foo, pytest.mark.bar]), pytest.param(2, marks=pytest.mark.baz)])
def test_marks(x): pass
"#,
            ),
        ],
        pytest: &[
            "test_marks.py::TestBase::test_x[b1]",
            "test_marks.py::TestBase::test_x[b2]",
            "test_marks.py::TestDerived::test_x[b1-7-d1-c1]",
            "test_marks.py::TestDerived::test_x[b1-7-d2-c1]",
            "test_marks.py::TestDerived::test_x[b1-8-d1-c1]",
            "test_marks.py::TestDerived::test_x[b1-8-d2-c1]",
            "test_marks.py::TestDerived::test_x[b2-7-d1-c1]",
            "test_marks.py::TestDerived::test_x[b2-7-d2-c1]",
            "test_marks.py::TestDerived::test_x[b2-8-d1-c1]",
            "test_marks.py::TestDerived::test_x[b2-8-d2-c1]",
            "test_marks.py::test_alias[1]",
            "test_marks.py::test_alias[2]",
            "test_marks.py::test_marks[1]",
            "test_marks.py::test_marks[2]",
        ],
        refined: &[],
    },
    // unittest classes: sorting, inheritance, runTest, __test__
    Suite {
        name: "unittest",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_ut.py",
                r#"import pytest
import unittest
from unittest import TestCase

pytestmark = pytest.mark.slow

class Base(unittest.TestCase):
    def test_b(self): pass
    def test_a(self): pass

class Derived(Base):
    def test_c(self): pass
    test_d = 5

class RunOnly(TestCase):
    def runTest(self): pass

class Off(TestCase):
    __test__ = False
    def test_x(self): pass

@pytest.mark.parametrize("x", [1, 2])
class Param(TestCase):
    def test_p(self): pass

class TestOuter:
    class Inner(TestCase):
        def test_in(self): pass
    class TestInner2(TestCase):
        def test_in2(self): pass

class Empty(TestCase):
    pass

class WithInit(TestCase):
    def __init__(self, *a, **k):
        super().__init__(*a, **k)
    def test_init(self): pass

class Disabled(TestCase):
    def test_on(self): pass
    def test_off(self): pass
    test_off.__test__ = False

class Static(TestCase):
    @staticmethod
    def test_static(): pass
    @classmethod
    def test_cls(cls): pass
    @property
    def test_prop(self): return 1
"#,
            ),
        ],
        pytest: &[
            "test_ut.py::Base::test_a",
            "test_ut.py::Base::test_b",
            "test_ut.py::Derived::test_a",
            "test_ut.py::Derived::test_b",
            "test_ut.py::Derived::test_c",
            "test_ut.py::RunOnly::runTest",
            "test_ut.py::Param::test_p",
            "test_ut.py::TestOuter::Inner::test_in",
            "test_ut.py::TestOuter::TestInner2::test_in2",
            "test_ut.py::WithInit::test_init",
            "test_ut.py::Disabled::test_on",
            "test_ut.py::Static::test_cls",
            "test_ut.py::Static::test_static",
        ],
        refined: &[],
    },
    // fixtures, __test__, staticmethods, properties, del and rebinding
    Suite {
        name: "names",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_misc.py",
                r#"import pytest

@pytest.fixture
def test_fix(): return 1

@pytest.fixture(name="other")
def test_fix2(): return 1

def test_off(): pass
test_off.__test__ = False

class Helper:
    __test__ = True
    def test_h(self): pass

def check(): pass
check.__test__ = True

class TestStatic:
    @staticmethod
    def test_s(): pass
    @classmethod
    def test_c(cls): pass
    @property
    def test_p(self): return 1
    test_attr = 5
    test_lam = lambda self: None

def test_after_del(): pass
del test_after_del

def test_rebound(): pass
test_rebound = 3

async def test_async(): pass

def test_nested_yield():
    def inner():
        yield 1
    pass

class TestAbsBase:
    pass

class TestA(TestAbsBase):
    def __new__(cls): return super().__new__(cls)
    def test_new(self): pass

class TestInheritsInit(TestStatic):
    pass
"#,
            ),
        ],
        pytest: &[
            "test_misc.py::Helper::test_h",
            "test_misc.py::check",
            "test_misc.py::TestStatic::test_s",
            "test_misc.py::TestStatic::test_c",
            "test_misc.py::TestStatic::test_lam",
            "test_misc.py::test_async",
            "test_misc.py::test_nested_yield",
            "test_misc.py::TestInheritsInit::test_s",
            "test_misc.py::TestInheritsInit::test_c",
            "test_misc.py::TestInheritsInit::test_lam",
        ],
        refined: &[],
    },
    // parametrized fixtures from a conftest
    Suite {
        name: "fixtures",
        files: &[
            (
                "conftest.py",
                r#"import pytest

@pytest.fixture(params=[1, 2])
def pf(request): return request.param

@pytest.fixture
def uses_pf(pf): return pf

@pytest.fixture(params=["a", "b"], ids=["A", "B"])
def pf2(request): return request.param
"#,
            ),
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_fx.py",
                r#"import pytest

def test_direct(pf): pass
def test_trans(uses_pf): pass
def test_none(): pass

@pytest.mark.parametrize("x", [10, 20])
def test_mix(x, pf2): pass

@pytest.mark.parametrize("pf", [7, 8])
def test_override(pf): pass

@pytest.mark.usefixtures("pf2")
def test_usefix(): pass
"#,
            ),
        ],
        pytest: &[
            "test_fx.py::test_direct[1]",
            "test_fx.py::test_direct[2]",
            "test_fx.py::test_trans[1]",
            "test_fx.py::test_trans[2]",
            "test_fx.py::test_none",
            "test_fx.py::test_mix[A-10]",
            "test_fx.py::test_mix[A-20]",
            "test_fx.py::test_mix[B-10]",
            "test_fx.py::test_mix[B-20]",
            "test_fx.py::test_override[7]",
            "test_fx.py::test_override[8]",
            "test_fx.py::test_usefix[A]",
            "test_fx.py::test_usefix[B]",
        ],
        refined: &["test_fx.py"],
    },
    // what pytest refuses to collect
    Suite {
        name: "errors",
        files: &[
            (
                "helper_mod.py",
                "def test_imported(): pass
class TestImported:
    def test_m(self): pass
def test_star(): pass
",
            ),
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_e1.py",
                r#"import pytest
pytestmark = pytest.mark.parametrize("m", [1])
def test_uses(m): pass
def test_nouse(): pass
"#,
            ),
            (
                "test_e2.py",
                r#"import pytest
@pytest.mark.parametrize("x", [1])
class TestX:
    @pytest.mark.parametrize("x", [2])
    def test_x(self, x): pass
def test_ok(): pass
"#,
            ),
            (
                "test_e3.py",
                r#"import pytest
@pytest.mark.parametrize("x", [1, 2], ids=["a"])
def test_x(x): pass
def test_ok(): pass
"#,
            ),
            (
                "test_e4.py",
                "from helper_mod import test_imported, TestImported
from helper_mod import *
def test_local(): pass
",
            ),
            (
                "test_e5.py",
                "import pytest
pytestmark = (pytest.mark.a, pytest.mark.b)
def test_t(): pass
",
            ),
            (
                "test_e6.py",
                r#"import pytest
@pytest.mark.parametrize("x", [1])
def test_default(x=3): pass
def test_ok(): pass
"#,
            ),
        ],
        pytest: &[
            "test_e2.py::test_ok",
            "test_e4.py::test_imported",
            "test_e4.py::TestImported::test_m",
            "test_e4.py::test_star",
            "test_e4.py::test_local",
        ],
        refined: &[
            "test_e1.py",
            "test_e2.py",
            "test_e3.py",
            "test_e4.py",
            "test_e5.py",
            "test_e6.py",
        ],
    },
    // literals, constants, ranges, aliases and parametrize call forms
    Suite {
        name: "values",
        files: &[
            (
                "pytest.ini",
                "[pytest]
markers =
    slow
    foo
    bar
    baz
",
            ),
            (
                "test_dups.py",
                r#"import pytest

@pytest.mark.parametrize("x", ["a1", "a1", "a10", "a1_0"])
def test_digits(x): pass

@pytest.mark.parametrize("x", [1, 1, 11, 1])
def test_ints(x): pass

@pytest.mark.parametrize("x,y", [(1, 2), (1, 2), ("1-2", 3)])
def test_joined(x, y): pass

@pytest.mark.parametrize("x", [1, 2, 3], ids=["a", "a", None])
def test_ids_dup(x): pass

@pytest.mark.parametrize("x", [pytest.param(1, id="p"), pytest.param(2, id="p"), 3])
def test_param_dup(x): pass

@pytest.mark.parametrize("x", ["", "", "0"])
def test_empty_strings(x): pass

@pytest.mark.parametrize("x", [None, None])
def test_nones(x): pass

@pytest.mark.parametrize("x", [(1,), (2,)])
def test_opaque_dup(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=["x0", None])
def test_clash(x): pass

@pytest.mark.parametrize("x", [1, 2, 3], ids=[pytest.HIDDEN_PARAM, "b", "c"])
def test_hidden_ids(x): pass

@pytest.mark.parametrize("x", [pytest.param(1, id=pytest.HIDDEN_PARAM)])
@pytest.mark.parametrize("y", [5, 6])
def test_hidden_stacked(x, y): pass
"#,
            ),
            (
                "test_forms.py",
                r#"import pytest
from pytest import mark, param

CASES = [(1, 2), (3, 4)]
MORE = [(5, 6)]
IDS = ["first", "second"]
NAMES = "a, b"
EMPTY = []
pair = pytest.mark.parametrize("p", [7, 8])
slow = mark.slow

@pytest.mark.parametrize(NAMES, CASES)
def test_constants(a, b): pass

@pytest.mark.parametrize("a,b", CASES + MORE)
def test_concat(a, b): pass

@pytest.mark.parametrize("a,b", [*CASES, (9, 10)])
def test_star(a, b): pass

@pytest.mark.parametrize("a,b", CASES, ids=IDS)
def test_ids_constant(a, b): pass

@pytest.mark.parametrize(argnames="x", argvalues=[1, 2])
def test_keywords(x): pass

@pytest.mark.parametrize("x", [1, 2], False, ["one", "two"])
def test_positional_ids(x): pass

@pytest.mark.parametrize(("x",), [(1,), (2,)])
def test_tuple_one(x): pass

@pytest.mark.parametrize(["x", "y"], [[1, 2]])
def test_list_names(x, y): pass

@pytest.mark.parametrize("x", EMPTY)
def test_empty_constant(x): pass

@pytest.mark.parametrize("x", [], ids=[])
def test_empty_ids(x): pass

@pair
@slow
def test_alias(p): pass

@mark.parametrize("x", [param(1, marks=[mark.foo]), param(2, marks=(mark.bar, mark.baz), id="two")])
def test_from_imports(x): pass

@pytest.fixture
def fx(request):
    return getattr(request, "param", None)

@pytest.mark.parametrize("fx", [1, 2], indirect=True)
def test_indirect(fx): pass

@pytest.mark.parametrize("fx, y", [(1, 2)], indirect=["fx"])
def test_indirect_list(fx, y): pass

@pytest.mark.parametrize("x", [pytest.param("é", id="ü"), pytest.param(2, id="a\tb")])
def test_param_escape(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=["é", b"\xc3\xa9"])
def test_ids_bytes(x): pass

@pytest.mark.parametrize("x", [1, 2], ids=[True, 2.5])
def test_ids_types(x): pass

@pytest.mark.parametrize("x", [1], scope="module")
def test_scope(x): pass
"#,
            ),
            (
                "test_values.py",
                r#"import re
import pytest

@pytest.mark.parametrize("v", [1+2j, -1j, 1-2j, 2j, 0j, -1.5+0j, 1e16j])
def test_complex(v): pass

@pytest.mark.parametrize("v", [-0, -1, 0x1F, 0o17, 0b101, 1_000_000, 0xFFFFFFFFFFFFFFFFFFFF, -100000000000000000000, +5, -True])
def test_ints(v): pass

@pytest.mark.parametrize("v", [r"a\b", "a" "b", f"plain", "\N{GREEK SMALL LETTER ALPHA}", "tab\there", b"\x00\xff", rb"\x41", "é\U0001F600", "'quoted'", '"dq"', "-", "a-b"])
def test_strings(v): pass

@pytest.mark.parametrize("v", [float("inf"), float("-inf"), float("nan"), float("1.5"), float(" 2 "), float(3)])
def test_float_calls(v): pass

@pytest.mark.parametrize("v", [int, str, ValueError, IOError, len, lambda x: x, re.compile("a.é"), re.compile(b"\xff")])
def test_named(v): pass

def helper(): pass
class Thing: pass

@pytest.mark.parametrize("v", [helper, Thing, ..., (1,), [2], {3}, {"k": 4}, None])
def test_objects(v): pass

@pytest.mark.parametrize("v", range(3))
def test_range(v): pass

@pytest.mark.parametrize("v", range(10, 0, -4))
def test_range_step(v): pass

@pytest.mark.parametrize("v", "abc")
def test_string_values(v): pass

@pytest.mark.parametrize("v", {"x": 1, "y": 2})
def test_dict_values(v): pass

@pytest.mark.parametrize("a,b", ["xy", "zw"])
def test_string_pairs(a, b): pass

@pytest.mark.parametrize("a,b", [(1, (2, 3)), (None, True), (False, "s")])
def test_nested(a, b): pass
"#,
            ),
        ],
        pytest: &[
            "test_dups.py::test_digits[a1_1]",
            "test_dups.py::test_digits[a1_2]",
            "test_dups.py::test_digits[a10]",
            "test_dups.py::test_digits[a1_0]",
            "test_dups.py::test_ints[1_0]",
            "test_dups.py::test_ints[1_1]",
            "test_dups.py::test_ints[11]",
            "test_dups.py::test_ints[1_2]",
            "test_dups.py::test_joined[1-2_0]",
            "test_dups.py::test_joined[1-2_1]",
            "test_dups.py::test_joined[1-2-3]",
            "test_dups.py::test_ids_dup[a0]",
            "test_dups.py::test_ids_dup[a1]",
            "test_dups.py::test_ids_dup[3]",
            "test_dups.py::test_param_dup[p0]",
            "test_dups.py::test_param_dup[p1]",
            "test_dups.py::test_param_dup[3]",
            "test_dups.py::test_empty_strings[1]",
            "test_dups.py::test_empty_strings[2]",
            "test_dups.py::test_empty_strings[0]",
            "test_dups.py::test_nones[None0]",
            "test_dups.py::test_nones[None1]",
            "test_dups.py::test_opaque_dup[x0]",
            "test_dups.py::test_opaque_dup[x1]",
            "test_dups.py::test_clash[x0]",
            "test_dups.py::test_clash[2]",
            "test_dups.py::test_hidden_ids",
            "test_dups.py::test_hidden_ids[b]",
            "test_dups.py::test_hidden_ids[c]",
            "test_dups.py::test_hidden_stacked[5]",
            "test_dups.py::test_hidden_stacked[6]",
            "test_forms.py::test_constants[1-2]",
            "test_forms.py::test_constants[3-4]",
            "test_forms.py::test_concat[1-2]",
            "test_forms.py::test_concat[3-4]",
            "test_forms.py::test_concat[5-6]",
            "test_forms.py::test_star[1-2]",
            "test_forms.py::test_star[3-4]",
            "test_forms.py::test_star[9-10]",
            "test_forms.py::test_ids_constant[first]",
            "test_forms.py::test_ids_constant[second]",
            "test_forms.py::test_keywords[1]",
            "test_forms.py::test_keywords[2]",
            "test_forms.py::test_positional_ids[one]",
            "test_forms.py::test_positional_ids[two]",
            "test_forms.py::test_tuple_one[1]",
            "test_forms.py::test_tuple_one[2]",
            "test_forms.py::test_list_names[1-2]",
            "test_forms.py::test_empty_constant[NOTSET]",
            "test_forms.py::test_empty_ids[NOTSET]",
            "test_forms.py::test_alias[7]",
            "test_forms.py::test_alias[8]",
            "test_forms.py::test_from_imports[1]",
            "test_forms.py::test_from_imports[two]",
            "test_forms.py::test_indirect[1]",
            "test_forms.py::test_indirect[2]",
            "test_forms.py::test_indirect_list[1-2]",
            r"test_forms.py::test_param_escape[\xfc]",
            r"test_forms.py::test_param_escape[a\tb]",
            r"test_forms.py::test_ids_bytes[\xe9]",
            r"test_forms.py::test_ids_bytes[\xc3\xa9]",
            "test_forms.py::test_ids_types[True]",
            "test_forms.py::test_ids_types[2.5]",
            "test_forms.py::test_scope[1]",
            "test_values.py::test_complex[(1+2j)]",
            "test_values.py::test_complex[(-0-1j)]",
            "test_values.py::test_complex[(1-2j)]",
            "test_values.py::test_complex[2j]",
            "test_values.py::test_complex[0j]",
            "test_values.py::test_complex[(-1.5+0j)]",
            "test_values.py::test_complex[1e+16j]",
            "test_values.py::test_ints[0]",
            "test_values.py::test_ints[-1_0]",
            "test_values.py::test_ints[31]",
            "test_values.py::test_ints[15]",
            "test_values.py::test_ints[5_0]",
            "test_values.py::test_ints[1000000]",
            "test_values.py::test_ints[1208925819614629174706175]",
            "test_values.py::test_ints[-100000000000000000000]",
            "test_values.py::test_ints[5_1]",
            "test_values.py::test_ints[-1_1]",
            r"test_values.py::test_strings[a\\b]",
            "test_values.py::test_strings[ab]",
            "test_values.py::test_strings[plain]",
            r"test_values.py::test_strings[\u03b1]",
            r"test_values.py::test_strings[tab\there]",
            r"test_values.py::test_strings[\x00\xff]",
            r"test_values.py::test_strings[\x41]",
            r"test_values.py::test_strings[\xe9\U0001f600]",
            "test_values.py::test_strings['quoted']",
            r#"test_values.py::test_strings["dq"]"#,
            "test_values.py::test_strings[-]",
            "test_values.py::test_strings[a-b]",
            "test_values.py::test_float_calls[inf]",
            "test_values.py::test_float_calls[-inf]",
            "test_values.py::test_float_calls[nan]",
            "test_values.py::test_float_calls[1.5]",
            "test_values.py::test_float_calls[2.0]",
            "test_values.py::test_float_calls[3.0]",
            "test_values.py::test_named[int]",
            "test_values.py::test_named[str]",
            "test_values.py::test_named[ValueError]",
            "test_values.py::test_named[OSError]",
            "test_values.py::test_named[len]",
            "test_values.py::test_named[<lambda>]",
            r"test_values.py::test_named[a.\xe9]",
            r"test_values.py::test_named[\xff]",
            "test_values.py::test_objects[helper]",
            "test_values.py::test_objects[Thing]",
            "test_values.py::test_objects[v2]",
            "test_values.py::test_objects[v3]",
            "test_values.py::test_objects[v4]",
            "test_values.py::test_objects[v5]",
            "test_values.py::test_objects[v6]",
            "test_values.py::test_objects[None]",
            "test_values.py::test_range[0]",
            "test_values.py::test_range[1]",
            "test_values.py::test_range[2]",
            "test_values.py::test_range_step[10]",
            "test_values.py::test_range_step[6]",
            "test_values.py::test_range_step[2]",
            "test_values.py::test_string_values[a]",
            "test_values.py::test_string_values[b]",
            "test_values.py::test_string_values[c]",
            "test_values.py::test_dict_values[x]",
            "test_values.py::test_dict_values[y]",
            "test_values.py::test_string_pairs[x-y]",
            "test_values.py::test_string_pairs[z-w]",
            "test_values.py::test_nested[1-b0]",
            "test_values.py::test_nested[None-True]",
            "test_values.py::test_nested[False-s]",
        ],
        refined: &[],
    },
    // MRO, ABCs, dataclasses, enums, aliases, nested and unittest classes, namespace order
    Suite {
        name: "classes",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_classes.py",
                r#"import abc
import enum
import unittest
from abc import ABC, ABCMeta, abstractmethod
from dataclasses import dataclass
from typing import Generic, NamedTuple, TypeVar

import pytest

T = TypeVar("T")

class A:
    def test_a(self): pass
    def test_shared(self): pass

class B(A):
    def test_b(self): pass

class C(A):
    def test_c(self): pass
    def test_shared(self): pass

class TestDiamond(B, C):
    def test_d(self): pass

class TestOverrideNonTest(A):
    test_a = None

class TestGeneric(Generic[T]):
    def test_g(self): pass

class TestMeta(metaclass=ABCMeta):
    @abstractmethod
    def x(self): ...
    def test_m(self): pass

class TestConcrete(TestMeta):
    def x(self): return 1

class TestAbstractProp(abc.ABC):
    @property
    @abc.abstractmethod
    def p(self): ...
    def test_p(self): pass

@dataclass(init=False)
class TestDataNoInit:
    def test_dni(self): pass

class TestErr(Exception):
    def test_e(self): pass

class TestTuple(NamedTuple):
    x: int = 0
    def test_nt(self): pass

class TestColor(enum.Enum):
    RED = 1
    def test_enum(self): pass

class Base:
    __test__ = False
    def test_base(self): pass

class TestStillOff(Base):
    def test_still_off(self): pass

class TestOn(Base):
    __test__ = True
    def test_on(self): pass

class Collected:
    __test__ = True
    def test_collected(self): pass

TestAlias = TestDiamond

class TestOuter:
    def test_outer(self): pass
    class TestInnerBase:
        def test_inner_base(self): pass
    class TestInner(TestInnerBase):
        def test_inner(self): pass
    class NotATest:
        def test_never(self): pass

class TestOuterDerived(TestOuter):
    def test_derived(self): pass

class Mixin:
    def test_mixin(self): pass

class TestWithMixin(Mixin, unittest.TestCase):
    def test_own(self): pass

class TestUnittestDerived(TestWithMixin):
    def test_zzz(self): pass
    def test_aaa(self): pass

class PlainCase(unittest.TestCase):
    pass

class OnlyRunTest(unittest.TestCase):
    def runTest(self): pass

class BothRunTest(unittest.TestCase):
    def runTest(self): pass
    def test_x(self): pass

class TestDeco:
    @staticmethod
    def test_static(): pass
    @classmethod
    def test_class(cls): pass
    @pytest.mark.parametrize("x", [1, 2])
    @staticmethod
    def test_static_param(x): pass

class TestNested2:
    @pytest.mark.parametrize("y", ["a"])
    class TestDeep:
        @pytest.mark.parametrize("z", [0])
        def test_deep(self, y, z): pass
"#,
            ),
            (
                "test_order.py",
                "import pytest

def test_first(): pass

class TestMiddle:
    def test_z(self): pass
    def test_a(self): pass

def test_dup(): pass

def test_last(): pass

def test_dup(): pass

if True:
    def test_in_if(): pass
else:
    def test_in_if(): pass

try:
    def test_in_try(): pass
except Exception:
    pass

def test_deleted(): pass
del test_deleted

def test_readded(): pass
del test_readded
def test_readded(): pass

def test_shadowed(): pass
test_shadowed = 5

class TestRedef:
    def test_m(self): pass
    def test_n(self): pass
    def test_m(self): pass

with open(__file__):
    def test_in_with(): pass

def test_original(): pass
test_alias_fn = test_original
",
            ),
        ],
        pytest: &[
            "test_classes.py::TestDiamond::test_a",
            "test_classes.py::TestDiamond::test_c",
            "test_classes.py::TestDiamond::test_shared",
            "test_classes.py::TestDiamond::test_b",
            "test_classes.py::TestDiamond::test_d",
            "test_classes.py::TestOverrideNonTest::test_shared",
            "test_classes.py::TestGeneric::test_g",
            "test_classes.py::TestConcrete::test_m",
            "test_classes.py::TestDataNoInit::test_dni",
            "test_classes.py::TestOn::test_base",
            "test_classes.py::TestOn::test_on",
            "test_classes.py::Collected::test_collected",
            "test_classes.py::TestAlias::test_a",
            "test_classes.py::TestAlias::test_c",
            "test_classes.py::TestAlias::test_shared",
            "test_classes.py::TestAlias::test_b",
            "test_classes.py::TestAlias::test_d",
            "test_classes.py::TestOuter::test_outer",
            "test_classes.py::TestOuter::TestInnerBase::test_inner_base",
            "test_classes.py::TestOuter::TestInner::test_inner_base",
            "test_classes.py::TestOuter::TestInner::test_inner",
            "test_classes.py::TestOuterDerived::test_outer",
            "test_classes.py::TestOuterDerived::TestInnerBase::test_inner_base",
            "test_classes.py::TestOuterDerived::TestInner::test_inner_base",
            "test_classes.py::TestOuterDerived::TestInner::test_inner",
            "test_classes.py::TestOuterDerived::test_derived",
            "test_classes.py::TestWithMixin::test_mixin",
            "test_classes.py::TestWithMixin::test_own",
            "test_classes.py::TestUnittestDerived::test_aaa",
            "test_classes.py::TestUnittestDerived::test_mixin",
            "test_classes.py::TestUnittestDerived::test_own",
            "test_classes.py::TestUnittestDerived::test_zzz",
            "test_classes.py::OnlyRunTest::runTest",
            "test_classes.py::BothRunTest::test_x",
            "test_classes.py::TestDeco::test_static",
            "test_classes.py::TestDeco::test_class",
            "test_classes.py::TestDeco::test_static_param[1]",
            "test_classes.py::TestDeco::test_static_param[2]",
            "test_classes.py::TestNested2::TestDeep::test_deep[0-a]",
            "test_order.py::test_first",
            "test_order.py::TestMiddle::test_z",
            "test_order.py::TestMiddle::test_a",
            "test_order.py::test_dup",
            "test_order.py::test_last",
            "test_order.py::test_in_if",
            "test_order.py::test_in_try",
            "test_order.py::test_readded",
            "test_order.py::TestRedef::test_m",
            "test_order.py::TestRedef::test_n",
            "test_order.py::test_in_with",
            "test_order.py::test_original",
            "test_order.py::test_alias_fn",
        ],
        refined: &[],
    },
    // custom patterns, norecursedirs, venvs, packages, collect_ignore
    Suite {
        name: "discovery",
        files: &[
            (
                ".hidden/check_h.py",
                "def check_hidden(): pass
",
            ),
            (
                "__pycache__/check_cache.py",
                "def check_cached(): pass
",
            ),
            (
                "build/check_b.py",
                "def check_build(): pass
",
            ),
            (
                "check_names.py",
                "class CheckThing:
    def check_one(self): pass
    def it_should_work(self): pass
    def test_also(self): pass
    def helper(self): pass

class LoginSuite:
    def check_login(self): pass

class TestNotMatched:
    def check_x(self): pass

def check_module(): pass
def other_should_pass(): pass
def nothing(): pass
",
            ),
            (
                "conftest.py",
                r#"import sys
collect_ignore = ["ignored_dir", "legacy/check_l.py"]
collect_ignore_glob = ["*_py2.py"]
"#,
            ),
            (
                "ignored_dir/check_i.py",
                "def check_ignored(): pass
",
            ),
            (
                "legacy/check_l.py",
                "def check_legacy(): pass
",
            ),
            (
                "login_spec.py",
                "def check_spec(): pass
",
            ),
            ("pkg/__init__.py", ""),
            (
                "pkg/check_pkg.py",
                "def check_pkg(): pass
",
            ),
            ("pkg/sub/__init__.py", ""),
            (
                "pkg/sub/helper.py",
                "def check_not(): pass
",
            ),
            (
                "pkg/sub/test_sub.py",
                "def check_sub(): pass
",
            ),
            (
                "plain/check_old_py2.py",
                "def check_old(): pass
",
            ),
            (
                "plain/check_p.py",
                "def check_plain(): pass
",
            ),
            (
                "pyproject.toml",
                r#"[tool.pytest.ini_options]
python_files = ["check_*.py", "*_spec.py", "test_*.py"]
python_classes = ["Check", "*Suite"]
python_functions = ["check_", "*_should_*", "test"]
norecursedirs = ["skipme*", "build"]
"#,
            ),
            (
                "skipme_too/check_s.py",
                "def check_skip(): pass
",
            ),
            (
                "venv/check_v.py",
                "def check_venv(): pass
",
            ),
            ("venv/pyvenv.cfg", ""),
        ],
        pytest: &[
            ".hidden/check_h.py::check_hidden",
            "check_names.py::CheckThing::check_one",
            "check_names.py::CheckThing::it_should_work",
            "check_names.py::CheckThing::test_also",
            "check_names.py::LoginSuite::check_login",
            "check_names.py::check_module",
            "check_names.py::other_should_pass",
            "login_spec.py::check_spec",
            "pkg/check_pkg.py::check_pkg",
            "pkg/sub/test_sub.py::check_sub",
            "plain/check_p.py::check_plain",
        ],
        refined: &[],
    },
    // conftest hooks and fixtures, in scope and session-wide
    Suite {
        name: "hooks",
        files: &[
            (
                "conftest.py",
                r#"import pytest

@pytest.fixture(params=["p1", "p2"])
def param_fx(request):
    return request.param

@pytest.fixture
def plain_fx():
    return 1
"#,
            ),
            (
                "gen/conftest.py",
                r#"def pytest_generate_tests(metafunc):
    if "g" in metafunc.fixturenames:
        metafunc.parametrize("g", [10, 20])
"#,
            ),
            (
                "gen/test_g.py",
                "def test_g(g): pass
def test_plain(): pass
",
            ),
            (
                "makeid/conftest.py",
                r#"def pytest_make_parametrize_id(config, val, argname):
    return f"{argname}={val}"
"#,
            ),
            (
                "makeid/test_mi.py",
                r#"import pytest
@pytest.mark.parametrize("x", [1, 2])
def test_mi(x): pass
def test_plain(): pass
"#,
            ),
            (
                "marked/conftest.py",
                "def pytest_collection_modifyitems(items):
    for item in items:
        pass
",
            ),
            (
                "marked/test_m.py",
                r#"import pytest
@pytest.mark.parametrize("x", [1, 2])
def test_m(x, plain_fx): pass
"#,
            ),
            (
                "overrides/conftest.py",
                r#"import pytest

@pytest.fixture
def param_fx():
    return "plain now"
"#,
            ),
            (
                "overrides/test_o.py",
                "def test_o(param_fx): pass
",
            ),
            (
                "plain/test_fixtures.py",
                r#"import pytest

@pytest.fixture
def local_fx(plain_fx):
    return plain_fx

@pytest.fixture(params=[1, 2], ids=["one", "two"])
def local_param(request):
    return request.param

def test_plain(local_fx): pass
def test_none(): pass

class TestWithClassFixture:
    @pytest.fixture(params=["c"])
    def class_param(self, request):
        return request.param
    def test_uses(self, class_param): pass
    def test_not(self): pass

@pytest.mark.parametrize("param_fx", ["direct"])
def test_overridden_by_parametrize(param_fx): pass

def pytest_dummy(): pass
"#,
            ),
            ("plug/conftest.py", ""),
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_autouse.py",
                r#"import pytest

@pytest.fixture(autouse=True, params=["a1", "a2"])
def auto(request):
    return request.param

def test_auto(): pass
"#,
            ),
            (
                "test_usefixtures.py",
                r#"import pytest

pytestmark = pytest.mark.usefixtures("param_fx")

def test_marked(): pass
"#,
            ),
        ],
        pytest: &[
            "gen/test_g.py::test_g[10]",
            "gen/test_g.py::test_g[20]",
            "gen/test_g.py::test_plain",
            "makeid/test_mi.py::test_mi[x=1]",
            "makeid/test_mi.py::test_mi[x=2]",
            "makeid/test_mi.py::test_plain",
            "marked/test_m.py::test_m[x=1]",
            "marked/test_m.py::test_m[x=2]",
            "overrides/test_o.py::test_o",
            "plain/test_fixtures.py::test_plain",
            "plain/test_fixtures.py::test_none",
            "plain/test_fixtures.py::TestWithClassFixture::test_uses[class_param=c]",
            "plain/test_fixtures.py::TestWithClassFixture::test_not",
            "plain/test_fixtures.py::test_overridden_by_parametrize[param_fx=direct]",
            "test_autouse.py::test_auto[auto=a1]",
            "test_autouse.py::test_auto[auto=a2]",
            "test_usefixtures.py::test_marked[param_fx=p1]",
            "test_usefixtures.py::test_marked[param_fx=p2]",
        ],
        refined: &[
            "gen/test_g.py",
            "makeid/test_mi.py",
            "marked/test_m.py",
            "plain/test_fixtures.py",
            "test_autouse.py",
            "test_usefixtures.py",
        ],
    },
    // fixture overrides, nested marks, timeouts, branches, class-level pytest_generate_tests
    Suite {
        name: "mixed",
        files: &[
            (
                "conftest.py",
                "import pytest

@pytest.fixture
def resource():
    return 1
",
            ),
            (
                "pytest.ini",
                "[pytest]
markers =
    slow
    db
    timeout
    smoke
",
            ),
            (
                "sub/conftest.py",
                r#"import pytest

@pytest.fixture(params=["x", "y"])
def resource(request):
    return request.param
"#,
            ),
            (
                "sub/test_sub.py",
                "def test_resource(resource): pass
def test_free(): pass
",
            ),
            (
                "test_branch.py",
                r#"import sys
import pytest

if sys.version_info >= (3, 8):
    @pytest.mark.parametrize("v", [1, 2])
    def test_branch(v): pass
else:
    @pytest.mark.parametrize("v", [3])
    def test_branch(v): pass
"#,
            ),
            (
                "test_gen.py",
                "class TestGen:
    def pytest_generate_tests(self, metafunc):
        pass
    def test_g(self): pass

class TestGenChild(TestGen):
    def test_child(self): pass
",
            ),
            (
                "test_misc2.py",
                r#"import sys
import pytest
from unittest import mock

@pytest.mark.db
class TestOuter:
    pytestmark = [pytest.mark.slow]

    @pytest.mark.smoke
    class TestInner:
        @pytest.mark.timeout(5)
        def test_timeout(self): pass

    def test_attr_off(self): pass
    test_attr_off.__test__ = False

    test_lambda = lambda self: None

    @mock.patch("os.getcwd")
    def test_patched(self, getcwd): pass

@pytest.mark.timeout(2.5)
@pytest.mark.parametrize("t", [pytest.param(1, marks=pytest.mark.timeout(9))])
def test_param_timeout(t): pass
"#,
            ),
            (
                "test_off.py",
                "__test__ = False
def test_never(): pass
",
            ),
            (
                "test_resource.py",
                "def test_resource(resource): pass
",
            ),
        ],
        pytest: &[
            "sub/test_sub.py::test_resource[x]",
            "sub/test_sub.py::test_resource[y]",
            "sub/test_sub.py::test_free",
            "test_branch.py::test_branch[1]",
            "test_branch.py::test_branch[2]",
            "test_gen.py::TestGen::test_g",
            "test_gen.py::TestGenChild::test_g",
            "test_gen.py::TestGenChild::test_child",
            "test_misc2.py::TestOuter::TestInner::test_timeout",
            "test_misc2.py::TestOuter::test_lambda",
            "test_misc2.py::TestOuter::test_patched",
            "test_misc2.py::test_param_timeout[1]",
            "test_resource.py::test_resource",
        ],
        // `test_off.py` has no test aye can see: pytest still imports it.
        refined: &[
            "sub/test_sub.py",
            "test_branch.py",
            "test_gen.py",
            "test_off.py",
        ],
    },
    // objects bound by assignment: instances, mocks, partials
    Suite {
        name: "bound",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_bound.py",
                "from unittest import mock
import functools

class Data:
    pass

def helper(x): pass

test_data = Data()
test_list = list(range(3))
test_mock = mock.Mock()
test_partial = functools.partial(helper, 1)
TestLike = Data()
",
            ),
        ],
        pytest: &["test_bound.py::test_partial"],
        refined: &["test_bound.py"],
    },
    // unescaped ids, empty_parameter_set_mark = xfail, python_functions
    Suite {
        name: "config",
        files: &[
            (
                "pytest.ini",
                "[pytest]
disable_test_id_escaping_and_forfeit_all_rights_to_community_support = true
empty_parameter_set_mark = xfail
python_functions = check_ test_
",
            ),
            (
                "test_cfg.py",
                r#"import pytest

@pytest.mark.parametrize("x", ["é", "日本", "a\tb", "x1", "x1"])
def test_raw(x): pass

@pytest.mark.parametrize("x", [])
def check_empty(x): pass

@pytest.mark.parametrize("x", [1, 1], ids=["ü", "ü"])
def test_ids(x): pass
"#,
            ),
        ],
        pytest: &[
            "test_cfg.py::test_raw[é]",
            "test_cfg.py::test_raw[日本]",
            "test_cfg.py::test_raw[a	b]",
            "test_cfg.py::test_raw[x1_0]",
            "test_cfg.py::test_raw[x1_1]",
            "test_cfg.py::check_empty[NOTSET]",
            "test_cfg.py::test_ids[ü0]",
            "test_cfg.py::test_ids[ü1]",
        ],
        refined: &[],
    },
    // strict_parametrization_ids
    Suite {
        name: "strict",
        files: &[
            (
                "pytest.ini",
                "[pytest]
strict_parametrization_ids = true
",
            ),
            (
                "test_strict.py",
                r#"import pytest

@pytest.mark.parametrize("x", [1, 1])
def test_dups(x): pass
"#,
            ),
            (
                "test_unique.py",
                r#"import pytest

@pytest.mark.parametrize("x", [1, 2])
def test_unique(x): pass
"#,
            ),
        ],
        pytest: &[
            "test_unique.py::test_unique[1]",
            "test_unique.py::test_unique[2]",
        ],
        refined: &["test_strict.py"],
    },
    // objects whose ids fall back to argname + index
    Suite {
        name: "objects",
        files: &[
            (
                "pytest.ini",
                "[pytest]
",
            ),
            (
                "test_objs.py",
                r#"import datetime
from decimal import Decimal
from fractions import Fraction
from pathlib import Path
import collections
import pytest

@pytest.mark.parametrize("v", [Decimal("1.5"), Fraction(1, 2), Path("x"), datetime.date(2020, 1, 1), datetime.timedelta(1), collections.OrderedDict(), dict(a=1), object(), range(3), "s", Decimal("2")])
def test_objs(v): pass

@pytest.mark.parametrize("v", [Decimal("1"), Decimal("1")])
def test_dups(v): pass
"#,
            ),
        ],
        pytest: &[
            "test_objs.py::test_objs[v0]",
            "test_objs.py::test_objs[v1]",
            "test_objs.py::test_objs[v2]",
            "test_objs.py::test_objs[v3]",
            "test_objs.py::test_objs[v4]",
            "test_objs.py::test_objs[v5]",
            "test_objs.py::test_objs[v6]",
            "test_objs.py::test_objs[v7]",
            "test_objs.py::test_objs[v8]",
            "test_objs.py::test_objs[s]",
            "test_objs.py::test_objs[v10]",
            "test_objs.py::test_dups[v0]",
            "test_objs.py::test_dups[v1]",
        ],
        refined: &[],
    },
    // marks read through list operations, transparent local decorators, the order of higher-scoped parameters, unittest and Protocol classes, and what only pytest knows
    Suite {
        name: "structure",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
markers =
    slow
",
            ),
            (
                "helper_base.py",
                r"import unittest

class BaseAPICase(unittest.TestCase):
    endpoint = None
    def test_get(self):
        pass
",
            ),
            (
                "registry.py",
                r"SCENARIOS = []

def scenario(f):
    SCENARIOS.append(f)
",
            ),
            (
                "test_bases.py",
                r"import collections
import enum

class Helper(collections.UserDict):
    pass

class Color(enum.Enum):
    RED = 1

def test_h():
    pass
",
            ),
            (
                "test_class_dyn.py",
                r#"def add_tests(cls):
    cls.test_added = lambda self: None
    return cls

@add_tests
class TestD:
    def test_d(self): pass

class Meta(type):
    def __new__(mcs, name, bases, ns):
        ns["test_meta"] = lambda self: None
        return super().__new__(mcs, name, bases, ns)

class TestM(metaclass=Meta):
    def test_m(self): pass

class Base:
    def __init_subclass__(cls, **kw):
        cls.test_sub = lambda self: None

class TestS(Base):
    def test_s(self): pass

class TestL:
    for _i in range(2):
        exec(f"def test_l{_i}(self): pass")
    def test_plain(self): pass

class TestA:
    def test_a(self): pass

def _extra(self): pass

TestA.test_extra = _extra
"#,
            ),
            (
                "test_cond_def.py",
                r#"import sys

if sys.platform == "win32":
    def test_windows(): pass
else:
    def test_unix(): pass

class TestCond:
    if sys.platform == "win32":
        def test_win(self): pass
    def test_d(self): pass

match sys.platform:
    case "win32":
        def test_m(): pass

for _ in []:
    def test_f(): pass

def test_plain(): pass
"#,
            ),
            (
                "test_cond_del.py",
                r#"import os, sys

def test_a(): pass
def test_b(): pass

if sys.platform == "win32":
    del test_b

class TestK:
    def test_k(self): pass
    if sys.platform == "win32":
        del test_k

class TestPosixOnly:
    def test_fork(self): pass

if os.name != "posix":
    del TestPosixOnly
"#,
            ),
            (
                "test_gen_wrapped.py",
                r"import contextlib
import functools

def drive(gen_fn):
    @functools.wraps(gen_fn)
    def run(*a, **k):
        for _ in gen_fn(*a, **k):
            pass
    return run

@drive
def test_driven():
    yield 1

@contextlib.contextmanager
def test_cm():
    yield 1
",
            ),
            (
                "test_inherit.py",
                r#"import helper_base
from helper_base import BaseAPICase

class UsersAPICase(BaseAPICase):
    endpoint = "/users"

class Orders(helper_base.BaseAPICase):
    endpoint = "/orders"
"#,
            ),
            (
                "test_local_deco.py",
                r"import functools

def ident(f):
    return f

def wrap(f):
    @functools.wraps(f)
    def inner(*a, **k):
        return f(*a, **k)
    return inner

@ident
def test_ident(): pass

@wrap
@ident
def test_wrapped(): pass
",
            ),
            (
                "test_marks_ops.py",
                r#"import pytest

pytestmark = [pytest.mark.slow]
pytestmark.append(pytest.mark.parametrize("b", [3, 4]))

def test_t(b):
    pass

class TestK:
    def test_k(self, b):
        pass
"#,
            ),
            (
                "test_marks_ops_class.py",
                r#"import pytest

class TestK:
    pytestmark = []
    pytestmark += [pytest.mark.parametrize("c", [5])]
    pytestmark.extend([pytest.mark.xfail(run=False)])

    def test_k(self, c):
        pass
"#,
            ),
            (
                "test_marks_unread.py",
                r#"import pytest

def make():
    return [pytest.mark.parametrize("b", [3, 4])]

pytestmark = make()

def deco(f):
    return pytest.mark.parametrize("c", [5, 6])(f)

def cases(*v):
    return pytest.mark.parametrize("d", v)

@deco
def test_local(b, c):
    pass

@cases(7, 8)
def test_factory(b, d):
    pass

def test_attr(b, e):
    pass

test_attr.pytestmark = [pytest.mark.parametrize("e", [9])]
"#,
            ),
            (
                "test_opaque_deco.py",
                r"from registry import scenario

@scenario
def test_registered(): pass

def test_real(): pass
",
            ),
            (
                "test_proto.py",
                r"import abc
import typing
import unittest

class TestProto(typing.Protocol):
    def test_p(self): ...

class Base(typing.Protocol):
    def m(self): ...

class TestFromProto(Base):
    def test_fp(self): pass

class TestAbs(abc.ABC):
    @abc.abstractproperty
    def prop(self): pass
    def test_b(self): pass

class TestAbsClassMethod(abc.ABC):
    @abc.abstractclassmethod
    def c(cls): pass
    def test_c(self): pass

class Wrapped(unittest.FunctionTestCase):
    pass

def test_ok(): pass
",
            ),
            (
                "test_reorder.py",
                r#"import pytest

@pytest.mark.parametrize("a", [1, 2], scope="module")
def test_x(a):
    pass

@pytest.mark.parametrize("a", [1, 2], scope="module")
def test_y(a):
    pass

class TestC:
    @pytest.mark.parametrize("a", [1, 2], scope="class")
    @pytest.mark.parametrize("b", [3, 4])
    def test_x(self, a, b):
        pass

    @pytest.mark.parametrize("a", [1, 2], scope="class")
    def test_y(self, a):
        pass

class TestD(TestC):
    pass

@pytest.mark.parametrize("a", [1, 2], scope="module")
@pytest.mark.parametrize("c", [5, 6], scope="module")
def test_z(a, c):
    pass

@pytest.fixture(scope="module")
def fx(request):
    return request.param

@pytest.mark.parametrize("fx", [7, 8], indirect=True)
def test_i(fx):
    pass

@pytest.mark.parametrize("fx", [7, 8], indirect=True)
def test_j(fx):
    pass
"#,
            ),
            (
                "test_try_import.py",
                r"try:
    import nonexistent_mod_xyz
    def test_needs_mod(): pass
except ImportError:
    pass

def test_plain(): pass
",
            ),
        ],
        pytest: &[
            "test_bases.py::test_h",
            "test_class_dyn.py::TestD::test_d",
            "test_class_dyn.py::TestD::test_added",
            "test_class_dyn.py::TestM::test_m",
            "test_class_dyn.py::TestM::test_meta",
            "test_class_dyn.py::TestS::test_s",
            "test_class_dyn.py::TestS::test_sub",
            "test_class_dyn.py::TestL::test_l0",
            "test_class_dyn.py::TestL::test_l1",
            "test_class_dyn.py::TestL::test_plain",
            "test_class_dyn.py::TestA::test_a",
            "test_class_dyn.py::TestA::test_extra",
            "test_cond_def.py::test_unix",
            "test_cond_def.py::TestCond::test_d",
            "test_cond_def.py::test_plain",
            "test_cond_del.py::test_a",
            "test_cond_del.py::test_b",
            "test_cond_del.py::TestK::test_k",
            "test_cond_del.py::TestPosixOnly::test_fork",
            "test_gen_wrapped.py::test_driven",
            "test_gen_wrapped.py::test_cm",
            "test_inherit.py::BaseAPICase::test_get",
            "test_inherit.py::UsersAPICase::test_get",
            "test_inherit.py::Orders::test_get",
            "test_local_deco.py::test_ident",
            "test_local_deco.py::test_wrapped",
            "test_marks_ops.py::test_t[3]",
            "test_marks_ops.py::test_t[4]",
            "test_marks_ops.py::TestK::test_k[3]",
            "test_marks_ops.py::TestK::test_k[4]",
            "test_marks_ops_class.py::TestK::test_k[5]",
            "test_marks_unread.py::test_local[5-3]",
            "test_marks_unread.py::test_local[5-4]",
            "test_marks_unread.py::test_local[6-3]",
            "test_marks_unread.py::test_local[6-4]",
            "test_marks_unread.py::test_factory[7-3]",
            "test_marks_unread.py::test_factory[7-4]",
            "test_marks_unread.py::test_factory[8-3]",
            "test_marks_unread.py::test_factory[8-4]",
            "test_marks_unread.py::test_attr[9-3]",
            "test_marks_unread.py::test_attr[9-4]",
            "test_opaque_deco.py::test_real",
            "test_proto.py::Wrapped::runTest",
            "test_proto.py::test_ok",
            "test_reorder.py::test_x[1]",
            "test_reorder.py::test_y[1]",
            "test_reorder.py::test_z[5-1]",
            "test_reorder.py::test_x[2]",
            "test_reorder.py::test_y[2]",
            "test_reorder.py::test_z[5-2]",
            "test_reorder.py::TestC::test_x[3-1]",
            "test_reorder.py::TestC::test_y[1]",
            "test_reorder.py::TestC::test_x[3-2]",
            "test_reorder.py::TestC::test_y[2]",
            "test_reorder.py::TestC::test_x[4-1]",
            "test_reorder.py::TestC::test_x[4-2]",
            "test_reorder.py::TestD::test_x[3-1]",
            "test_reorder.py::TestD::test_y[1]",
            "test_reorder.py::TestD::test_x[3-2]",
            "test_reorder.py::TestD::test_y[2]",
            "test_reorder.py::TestD::test_x[4-1]",
            "test_reorder.py::TestD::test_x[4-2]",
            "test_reorder.py::test_z[6-1]",
            "test_reorder.py::test_z[6-2]",
            "test_reorder.py::test_i[7]",
            "test_reorder.py::test_j[7]",
            "test_reorder.py::test_i[8]",
            "test_reorder.py::test_j[8]",
            "test_try_import.py::test_plain",
        ],
        refined: &[
            "test_class_dyn.py",
            "test_cond_def.py",
            "test_cond_del.py",
            "test_gen_wrapped.py",
            "test_inherit.py",
            "test_marks_unread.py",
            "test_opaque_deco.py",
            "test_try_import.py",
        ],
    },
    // fixtures and hooks conftests and test modules import from project files, plugins they load or register, and `-p` in addopts
    Suite {
        name: "imports",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
addopts = -p preplug
pythonpath = .
",
            ),
            (
                "preplug.py",
                r#"def pytest_generate_tests(metafunc):
    if "pre" in metafunc.fixturenames:
        metafunc.parametrize("pre", [1, 2])
"#,
            ),
            (
                "fixmod.py",
                r#"import pytest

@pytest.fixture(params=["p1", "p2"])
def pf(request):
    return request.param

@pytest.fixture
def plain():
    return 1
"#,
            ),
            (
                "hkmod.py",
                r#"def pytest_make_parametrize_id(config, val, argname):
    return f"S{val}"
"#,
            ),
            (
                "idplugin.py",
                r#"def pytest_make_parametrize_id(config, val, argname):
    if argname == "q":
        return f"P{val}"
"#,
            ),
            (
                "fx/conftest.py",
                r"from fixmod import *
",
            ),
            (
                "fx/test_fx.py",
                r"def test_pf(pf):
    pass

def test_plain(plain):
    pass
",
            ),
            (
                "fx/test_fx_local.py",
                r"from fixmod import pf

def test_local(pf):
    pass
",
            ),
            (
                "hk/conftest.py",
                r"from hkmod import *
",
            ),
            (
                "hk/test_hk.py",
                r#"import pytest

@pytest.mark.parametrize("x", [3])
def test_hk(x):
    pass
"#,
            ),
            (
                "test_late.py",
                r#"import pytest

@pytest.mark.parametrize("q", [3])
def test_late(q):
    pass
"#,
            ),
            (
                "test_loads_plugin.py",
                r#"import pytest

pytest_plugins = ["idplugin"]

@pytest.mark.parametrize("q", [1])
def test_loads(q):
    pass
"#,
            ),
            (
                "test_plain.py",
                r"def test_pre(pre):
    pass
",
            ),
        ],
        pytest: &[
            "fx/test_fx.py::test_pf[p1]",
            "fx/test_fx.py::test_pf[p2]",
            "fx/test_fx.py::test_plain",
            "fx/test_fx_local.py::test_local[p1]",
            "fx/test_fx_local.py::test_local[p2]",
            "hk/test_hk.py::test_hk[S3]",
            "test_late.py::test_late[S3]",
            "test_loads_plugin.py::test_loads[P1]",
            "test_plain.py::test_pre[S1]",
            "test_plain.py::test_pre[S2]",
        ],
        refined: &[
            "fx/test_fx.py",
            "fx/test_fx_local.py",
            "hk/test_hk.py",
            "test_late.py",
            "test_loads_plugin.py",
            "test_plain.py",
        ],
    },
    // module constants changed through class attributes, the module object or frames (refined); controls stay exact
    Suite {
        name: "reached",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "helper.py",
                r"def deco(cls):
    cls.x.append(3)
    return cls
",
            ),
            (
                "test_alias.py",
                r#"import pytest
CASES = [1, 2]
class H:
    x = CASES
H.x.append(3)
@pytest.mark.parametrize("v", CASES)
def test_alias(v):
    pass
"#,
            ),
            (
                "test_classmethod.py",
                r#"import pytest
CASES = [1, 2]
class H:
    x = CASES
    @classmethod
    def add(cls):
        cls.x.append(3)
H.add()
@pytest.mark.parametrize("v", CASES)
def test_classmethod(v):
    pass
"#,
            ),
            (
                "test_namedtuple.py",
                r#"import pytest
from typing import NamedTuple
CASES = [1, 2]
class P(NamedTuple):
    l: list = CASES
P().l.append(3)
@pytest.mark.parametrize("v", CASES)
def test_namedtuple(v):
    pass
"#,
            ),
            (
                "test_handed.py",
                r#"import pytest
from helper import deco
CASES = [1, 2]
@deco
class H:
    x = CASES
@pytest.mark.parametrize("v", CASES)
def test_handed(v):
    pass
"#,
            ),
            (
                "test_vars.py",
                r#"import pytest
CASES = [1, 2]
class H:
    x = CASES
vars(H)["x"].append(3)
@pytest.mark.parametrize("v", CASES)
def test_vars(v):
    pass
"#,
            ),
            (
                "test_modules.py",
                r#"import pytest
import sys
CASES = [1, 2]
sys.modules[__name__].CASES.append(3)
@pytest.mark.parametrize("v", CASES)
def test_modules(v):
    pass
"#,
            ),
            (
                "test_itself.py",
                r#"import pytest
CASES = [1, 2]
import test_itself
test_itself.CASES.append(3)
@pytest.mark.parametrize("v", CASES)
def test_itself(v):
    pass
"#,
            ),
            (
                "test_dunder_import.py",
                r#"import pytest
CASES = [1, 2]
__import__(__name__).CASES.append(3)
@pytest.mark.parametrize("v", CASES)
def test_dunder_import(v):
    pass
"#,
            ),
            (
                "test_import_module.py",
                r#"import pytest
import importlib
CASES = [1, 2]
importlib.import_module(__name__).CASES.append(3)
@pytest.mark.parametrize("v", CASES)
def test_import_module(v):
    pass
"#,
            ),
            (
                "test_frame.py",
                r#"import pytest
import inspect
CASES = [1, 2]
inspect.currentframe().f_globals["CASES"].append(3)
@pytest.mark.parametrize("v", CASES)
def test_frame(v):
    pass
"#,
            ),
            (
                "test_getframe.py",
                r#"import pytest
import sys
CASES = [1, 2]
sys._getframe().f_globals["CASES"].append(3)
@pytest.mark.parametrize("v", CASES)
def test_getframe(v):
    pass
"#,
            ),
            (
                "test_globals_attr.py",
                r#"import pytest
CASES = [1, 2]
def f():
    pass
f.__globals__["CASES"].append(3)
@pytest.mark.parametrize("v", CASES)
def test_globals_attr(v):
    pass
"#,
            ),
            (
                "test_gc.py",
                r#"import pytest
import gc
CASES = [1, 2]
[o for o in gc.get_objects() if o is CASES][0].append(3)
@pytest.mark.parametrize("v", CASES)
def test_gc(v):
    pass
"#,
            ),
            (
                "test_getattr_module.py",
                r#"import pytest
import sys
CASES = [1, 2]
getattr(sys.modules[__name__], "CASES").append(3)
@pytest.mark.parametrize("v", CASES)
def test_getattr_module(v):
    pass
"#,
            ),
            (
                "test_ok.py",
                r#"import sys
import pytest
CASES = [1, 2]
class H:
    x = (5, 6)
H.y = 1
sys.modules["fake_module_for_aye"] = object()
HAS_NUMPY = "numpy" in sys.modules
@pytest.mark.parametrize("v", CASES)
def test_ok(v):
    sys.modules[__name__].CASES.append(v)
"#,
            ),
            (
                "zsub/conftest.py",
                r#"def pytest_collectstart(collector):
    if type(collector).__name__ == "Module" and hasattr(collector.obj, "CASES"):
        collector.obj.CASES.append(3)
"#,
            ),
            (
                "zsub/test_hook.py",
                r#"import pytest
CASES = [1, 2]
@pytest.mark.parametrize("v", CASES)
def test_hook(v):
    pass
"#,
            ),
        ],
        pytest: &[
            "test_alias.py::test_alias[1]",
            "test_alias.py::test_alias[2]",
            "test_alias.py::test_alias[3]",
            "test_classmethod.py::test_classmethod[1]",
            "test_classmethod.py::test_classmethod[2]",
            "test_classmethod.py::test_classmethod[3]",
            "test_dunder_import.py::test_dunder_import[1]",
            "test_dunder_import.py::test_dunder_import[2]",
            "test_dunder_import.py::test_dunder_import[3]",
            "test_frame.py::test_frame[1]",
            "test_frame.py::test_frame[2]",
            "test_frame.py::test_frame[3]",
            "test_gc.py::test_gc[1]",
            "test_gc.py::test_gc[2]",
            "test_gc.py::test_gc[3]",
            "test_getattr_module.py::test_getattr_module[1]",
            "test_getattr_module.py::test_getattr_module[2]",
            "test_getattr_module.py::test_getattr_module[3]",
            "test_getframe.py::test_getframe[1]",
            "test_getframe.py::test_getframe[2]",
            "test_getframe.py::test_getframe[3]",
            "test_globals_attr.py::test_globals_attr[1]",
            "test_globals_attr.py::test_globals_attr[2]",
            "test_globals_attr.py::test_globals_attr[3]",
            "test_handed.py::test_handed[1]",
            "test_handed.py::test_handed[2]",
            "test_handed.py::test_handed[3]",
            "test_import_module.py::test_import_module[1]",
            "test_import_module.py::test_import_module[2]",
            "test_import_module.py::test_import_module[3]",
            "test_itself.py::test_itself[1]",
            "test_itself.py::test_itself[2]",
            "test_itself.py::test_itself[3]",
            "test_modules.py::test_modules[1]",
            "test_modules.py::test_modules[2]",
            "test_modules.py::test_modules[3]",
            "test_namedtuple.py::test_namedtuple[1]",
            "test_namedtuple.py::test_namedtuple[2]",
            "test_namedtuple.py::test_namedtuple[3]",
            "test_ok.py::test_ok[1]",
            "test_ok.py::test_ok[2]",
            "test_vars.py::test_vars[1]",
            "test_vars.py::test_vars[2]",
            "test_vars.py::test_vars[3]",
            "zsub/test_hook.py::test_hook[1]",
            "zsub/test_hook.py::test_hook[2]",
            "zsub/test_hook.py::test_hook[3]",
        ],
        refined: &[
            "test_alias.py",
            "test_classmethod.py",
            "test_dunder_import.py",
            "test_frame.py",
            "test_gc.py",
            "test_getattr_module.py",
            "test_getframe.py",
            "test_globals_attr.py",
            "test_handed.py",
            "test_import_module.py",
            "test_itself.py",
            "test_modules.py",
            "test_namedtuple.py",
            "test_vars.py",
            "zsub/test_hook.py",
        ],
    },
    // re, builtins and pytest changed while modules are imported (refined); controls stay exact
    Suite {
        name: "patched",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_re_store.py",
                r#"import pytest
import re
_o = re.compile
re.compile = lambda p, *a: "x" if p == "ab" else _o(p, *a)
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_store(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_global.py",
                r#"import pytest
import re
def f():
    global re
    import types
    re = types.SimpleNamespace(compile=lambda p: "y")
f()
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_global(v):
    pass
"#,
            ),
            (
                "test_range_builtins.py",
                r#"import builtins
import pytest
_r = builtins.range
builtins.range = lambda *a: [9] if a == (2,) else _r(*a)
@pytest.mark.parametrize("v", range(2))
def test_range_builtins(v):
    pass
builtins.range = _r
"#,
            ),
            (
                "test_range_dunder.py",
                r#"import pytest
_r = range
__builtins__["range"] = lambda *a: [8] if a == (2,) else _r(*a)
@pytest.mark.parametrize("v", range(2))
def test_range_dunder(v):
    pass
__builtins__["range"] = _r
"#,
            ),
            (
                "test_range_global.py",
                r#"import pytest
def f():
    global range
    range = lambda *a: [7]
f()
@pytest.mark.parametrize("v", range(2))
def test_range_global(v):
    pass
"#,
            ),
            (
                "test_re_setattr.py",
                r#"import pytest
import re
import re as r
_o = re.compile
setattr(r, "compile", lambda p, *a: "s" if p == "ab" else _o(p, *a))
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_setattr(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_vars.py",
                r#"import pytest
import re
_o = re.compile
vars(re)["compile"] = lambda p, *a: "w" if p == "ab" else _o(p, *a)
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_vars(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_modules.py",
                r#"import pytest
import re
import sys
_o = re.compile
sys.modules["re"].compile = lambda p, *a: "m" if p == "ab" else _o(p, *a)
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_modules(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_alias.py",
                r#"import pytest
import re
_o = re.compile
m = re
m.compile = lambda p, *a: "a" if p == "ab" else _o(p, *a)
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_alias(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_helper.py",
                r#"import pytest
import re
_o = re.compile
def f():
    re.compile = lambda p, *a: "h" if p == "ab" else _o(p, *a)
f()
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_helper(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_local.py",
                r#"import pytest
import re
_o = re.compile
def f():
    import re as r
    r.compile = lambda p, *a: "l" if p == "ab" else _o(p, *a)
f()
@pytest.mark.parametrize("v", [re.compile("ab")])
def test_re_local(v):
    pass
re.compile = _o
"#,
            ),
            (
                "test_re_flags.py",
                r#"import pytest
import re
_i = re.I
re.__dict__["I"] = re.X
@pytest.mark.parametrize("v", [re.compile("a b", re.I)])
def test_re_flags(v):
    pass
re.I = _i
"#,
            ),
            (
                "test_ok.py",
                r#"import os
import re
import sys
import pytest
sys.path.insert(0, os.path.dirname(__file__))
os.environ["AYE_PATCHED_SUITE"] = "1"
re.unused_attribute_for_aye = 1
@pytest.mark.parametrize("v", [re.compile("cd", re.I), *range(2), float("1.5")])
def test_ok(v, monkeypatch):
    monkeypatch.setattr(re, "compile", None)
"#,
            ),
            (
                "zsub/conftest.py",
                r#"import re
_o = re.compile
re.compile = lambda p, *a: "cf" if p == "zz" else _o(p, *a)
"#,
            ),
            (
                "zsub/test_cf.py",
                r#"import pytest
import re
@pytest.mark.parametrize("v", [re.compile("zz")])
def test_cf(v):
    pass
"#,
            ),
        ],
        pytest: &[
            "test_ok.py::test_ok[cd]",
            "test_ok.py::test_ok[0]",
            "test_ok.py::test_ok[1]",
            "test_ok.py::test_ok[1.5]",
            "test_range_builtins.py::test_range_builtins[9]",
            "test_range_dunder.py::test_range_dunder[8]",
            "test_range_global.py::test_range_global[7]",
            "test_re_alias.py::test_re_alias[a]",
            "test_re_flags.py::test_re_flags[a b]",
            "test_re_global.py::test_re_global[y]",
            "test_re_helper.py::test_re_helper[h]",
            "test_re_local.py::test_re_local[l]",
            "test_re_modules.py::test_re_modules[m]",
            "test_re_setattr.py::test_re_setattr[s]",
            "test_re_store.py::test_re_store[x]",
            "test_re_vars.py::test_re_vars[w]",
            "zsub/test_cf.py::test_cf[cf]",
        ],
        refined: &[
            "test_range_builtins.py",
            "test_range_dunder.py",
            "test_range_global.py",
            "test_re_alias.py",
            "test_re_flags.py",
            "test_re_global.py",
            "test_re_helper.py",
            "test_re_local.py",
            "test_re_modules.py",
            "test_re_setattr.py",
            "test_re_store.py",
            "test_re_vars.py",
            "zsub/test_cf.py",
        ],
    },
    // values built by calls and displays: valid ones stay exact (invalid ones: `values_that_raise_at_import_are_refined`)
    Suite {
        name: "constructed",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_built.py",
                r#"import collections
import datetime
import decimal
import fractions
import os
import pathlib
import uuid
import pytest
CASES = [1, 2]
@pytest.mark.parametrize("v", [
    range(3), range(0, 10, -1), tuple([1, 2]), list("ab"), set(), frozenset({1, 2}), frozenset([(1, 2)]),
    {(1, 2): 3}, {frozenset([1]): 2}, {1: 2, **{3: 4}}, [*range(3)], [*CASES, 3], dict(a=1), dict([(1, 2)]), dict({1: 2}),
    slice(1), slice(1, None, 2), bytearray(b"a"), bytearray(3), bytearray([1, 255]), object(),
    decimal.Decimal("1.5"), decimal.Decimal(" -Inf "), decimal.Decimal("1e5"), decimal.Decimal(2), fractions.Fraction(1, 2),
    pathlib.Path("a", "b"), pathlib.PurePosixPath(), uuid.UUID("12345678-1234-5678-1234-567812345678"),
    datetime.date(2024, 2, 29), datetime.datetime(2020, 1, 1, 23, 59), datetime.time(12), datetime.timedelta(days=1, hours=2),
    collections.OrderedDict(), collections.deque([1]), collections.Counter("aab"), collections.defaultdict(),
    (x for x in [1]), [x for x in range(3)], {x for x in (1, 2)}, lambda x=1: x, [os.sep, os.path], ...,
])
def test_built(v):
    pass
"#,
            ),
        ],
        pytest: &[
            "test_built.py::test_built[v0]",
            "test_built.py::test_built[v1]",
            "test_built.py::test_built[v2]",
            "test_built.py::test_built[v3]",
            "test_built.py::test_built[v4]",
            "test_built.py::test_built[v5]",
            "test_built.py::test_built[v6]",
            "test_built.py::test_built[v7]",
            "test_built.py::test_built[v8]",
            "test_built.py::test_built[v9]",
            "test_built.py::test_built[v10]",
            "test_built.py::test_built[v11]",
            "test_built.py::test_built[v12]",
            "test_built.py::test_built[v13]",
            "test_built.py::test_built[v14]",
            "test_built.py::test_built[v15]",
            "test_built.py::test_built[v16]",
            "test_built.py::test_built[v17]",
            "test_built.py::test_built[v18]",
            "test_built.py::test_built[v19]",
            "test_built.py::test_built[v20]",
            "test_built.py::test_built[v21]",
            "test_built.py::test_built[v22]",
            "test_built.py::test_built[v23]",
            "test_built.py::test_built[v24]",
            "test_built.py::test_built[v25]",
            "test_built.py::test_built[v26]",
            "test_built.py::test_built[v27]",
            "test_built.py::test_built[v28]",
            "test_built.py::test_built[v29]",
            "test_built.py::test_built[v30]",
            "test_built.py::test_built[v31]",
            "test_built.py::test_built[v32]",
            "test_built.py::test_built[v33]",
            "test_built.py::test_built[v34]",
            "test_built.py::test_built[v35]",
            "test_built.py::test_built[v36]",
            "test_built.py::test_built[<genexpr>]",
            "test_built.py::test_built[v38]",
            "test_built.py::test_built[v39]",
            "test_built.py::test_built[<lambda>]",
            "test_built.py::test_built[v41]",
            "test_built.py::test_built[v42]",
        ],
        refined: &[],
    },
    // unittest cases of the project imported under any name are collected by pytest (refined)
    Suite {
        name: "unittest_imports",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "base.py",
                r#"import unittest
from unittest.mock import Mock
CONSTANT = [1, 2]
LABEL = ", ".join(["a", "b"])
def helper():
    pass
class Model:
    pass
class BaseAPICase(unittest.TestCase):
    def test_get(self):
        pass
class Derived(BaseAPICase):
    pass
"#,
            ),
            (
                "test_reexp.py",
                r"from base import BaseAPICase
",
            ),
            (
                "test_alias.py",
                r"from base import BaseAPICase as B
",
            ),
            (
                "test_derived.py",
                r"from base import Derived
",
            ),
            (
                "test_assign.py",
                r"import base
B = base.BaseAPICase
",
            ),
            (
                "test_ok.py",
                r"from base import CONSTANT, LABEL, Mock, Model, helper
from unittest import TestCase
def test_ok():
    pass
",
            ),
        ],
        pytest: &[
            "test_alias.py::B::test_get",
            "test_assign.py::B::test_get",
            "test_derived.py::Derived::test_get",
            "test_ok.py::test_ok",
            "test_reexp.py::BaseAPICase::test_get",
        ],
        refined: &[
            "test_alias.py",
            "test_assign.py",
            "test_derived.py",
            "test_reexp.py",
        ],
    },
    // test files with no test aye can see are imported all the same (refined)
    Suite {
        name: "no_static_tests",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_broken.py",
                r"import nonexistent_dependency_for_aye

# def test_x():
#     pass
",
            ),
            (
                "test_empty.py",
                r"# nothing yet
",
            ),
            (
                "test_ok.py",
                r"def test_ok():
    pass
",
            ),
        ],
        pytest: &["test_ok.py::test_ok"],
        refined: &["test_broken.py", "test_empty.py"],
    },
];
