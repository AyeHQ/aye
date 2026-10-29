//! Scratch suites checked against `python -m unittest discover` on Python
//! 3.13: `exact` lists the ids static recognition is sure of, in its order
//! (the files not refined), and `refined` the files it sends for refinement.

pub(super) struct Suite {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    /// The ids unittest loads from the files static recognition predicts,
    /// in aye's walk order.
    pub exact: &'static [&'static str],
    /// Files recognition sends for refinement.
    pub refined: &'static [&'static str],
}

pub(super) const SUITES: &[Suite] = &[
    // order: classes by bound name, methods by name, code point order
    Suite {
        name: "order",
        files: &[(
            "test_order.py",
            r#"import unittest
from unittest import TestCase


class Zeta(TestCase):
    def test_b(self): pass
    def test_a(self): pass
    def test_Upper(self): pass
    def test_9(self): pass
    def test_é(self): pass
    def runTest(self): pass


class alpha(unittest.TestCase):
    def test_x(self): pass


class Beta(unittest.case.TestCase):
    def testNoUnderscore(self): pass
    def tes_not(self): pass
    def _test_private(self): pass
    def test(self): pass


class OnlyRun(TestCase):
    def runTest(self): pass


class Empty(TestCase):
    def helper(self): pass


class NotATest:
    def test_never(self): pass


if __name__ == "__main__":
    unittest.main()
"#,
        )],
        exact: &[
            "test_order.py::Beta::test",
            "test_order.py::Beta::testNoUnderscore",
            "test_order.py::OnlyRun::runTest",
            "test_order.py::Zeta::test_9",
            "test_order.py::Zeta::test_Upper",
            "test_order.py::Zeta::test_a",
            "test_order.py::Zeta::test_b",
            "test_order.py::Zeta::test_é",
            "test_order.py::alpha::test_x",
        ],
        refined: &[],
    },
    // what counts as a test method
    Suite {
        name: "members",
        files: &[(
            "test_members.py",
            r#"import functools
import unittest
from unittest import mock


def helper(self):
    pass


def keep(func):
    return func


def wrapping(func):
    @functools.wraps(func)
    def inner(*args, **kwargs):
        return func(*args, **kwargs)
    return inner


CONSTANT = 3


class Members(unittest.TestCase):
    test_lambda = lambda self: None
    test_helper = helper
    test_constant = CONSTANT
    test_none = None
    test_list = [1, 2]
    test_string = "x"

    def test_method(self): pass
    test_alias = test_method

    @staticmethod
    def test_static(): pass

    @classmethod
    def test_class(cls): pass

    @property
    def test_property(self): return 1

    @functools.cached_property
    def test_cached(self): return 1

    class test_nested:
        pass

    @unittest.skip("not now")
    def test_skipped(self): pass

    @unittest.skipIf(True, "never")
    def test_skip_if(self): pass

    @unittest.expectedFailure
    def test_xfail(self): self.fail()

    @mock.patch("os.getcwd")
    def test_patched(self, getcwd): pass

    @keep
    def test_kept(self): pass

    @wrapping
    def test_wrapped(self): pass

    async def test_async(self): pass

    test_async_alias = staticmethod(helper)


@unittest.skip("whole class")
class Skipped(unittest.TestCase):
    def test_s(self): pass


@mock.patch.dict("os.environ", {"X": "1"})
class Patched(unittest.TestCase):
    def test_p(self): pass


class Async(unittest.IsolatedAsyncioTestCase):
    async def test_a(self): pass
"#,
        )],
        exact: &[
            "test_members.py::Async::test_a",
            "test_members.py::Members::test_alias",
            "test_members.py::Members::test_async",
            "test_members.py::Members::test_async_alias",
            "test_members.py::Members::test_class",
            "test_members.py::Members::test_helper",
            "test_members.py::Members::test_kept",
            "test_members.py::Members::test_lambda",
            "test_members.py::Members::test_method",
            "test_members.py::Members::test_nested",
            "test_members.py::Members::test_patched",
            "test_members.py::Members::test_skip_if",
            "test_members.py::Members::test_skipped",
            "test_members.py::Members::test_static",
            "test_members.py::Members::test_wrapped",
            "test_members.py::Members::test_xfail",
            "test_members.py::Patched::test_p",
            "test_members.py::Skipped::test_s",
        ],
        refined: &[],
    },
    // inheritance in the module: mixins, overrides, `del`, aliases
    Suite {
        name: "inheritance",
        files: &[(
            "test_inherit.py",
            r"import abc
import unittest


class Mixin:
    def test_from_mixin(self): pass
    def test_overridden(self): pass


class Base(unittest.TestCase):
    def test_base(self): pass
    def test_hidden(self): pass


class Child(Mixin, Base):
    test_hidden = None
    def test_overridden(self): pass
    def test_child(self): pass


class Abstract(unittest.TestCase, metaclass=abc.ABCMeta):
    def test_abstract(self): pass


class Removed(unittest.TestCase):
    def test_removed(self): pass


del Removed

Alias = Child
",
        )],
        exact: &[
            "test_inherit.py::Abstract::test_abstract",
            "test_inherit.py::Alias::test_base",
            "test_inherit.py::Alias::test_child",
            "test_inherit.py::Alias::test_from_mixin",
            "test_inherit.py::Alias::test_overridden",
            "test_inherit.py::Base::test_base",
            "test_inherit.py::Base::test_hidden",
            "test_inherit.py::Child::test_base",
            "test_inherit.py::Child::test_child",
            "test_inherit.py::Child::test_from_mixin",
            "test_inherit.py::Child::test_overridden",
        ],
        refined: &[],
    },
    // discovery: packages only, patterns, valid names, `__init__.py`
    Suite {
        name: "discovery",
        files: &[
            (
                "test_top.py",
                "import unittest\nclass Top(unittest.TestCase):\n    def test_top(self): pass\n",
            ),
            (
                "__init__.py",
                "import unittest\nclass NotLoaded(unittest.TestCase):\n    def test_never(self): pass\n",
            ),
            (
                "Test_upper.py",
                "import unittest\nclass U(unittest.TestCase):\n    def test_u(self): pass\n",
            ),
            (
                "test-dash.py",
                "import unittest\nclass D(unittest.TestCase):\n    def test_d(self): pass\n",
            ),
            (
                "helper.py",
                "import unittest\nclass H(unittest.TestCase):\n    def test_h(self): pass\n",
            ),
            (
                "plain/test_plain.py",
                "import unittest\nclass P(unittest.TestCase):\n    def test_p(self): pass\n",
            ),
            (
                "pkg/__init__.py",
                "import unittest\nclass InInit(unittest.TestCase):\n    def test_init(self): pass\n",
            ),
            (
                "pkg/test_pkg.py",
                "import unittest\nclass K(unittest.TestCase):\n    def test_k(self): pass\n",
            ),
            ("pkg/sub/__init__.py", ""),
            (
                "pkg/sub/test_sub.py",
                "import unittest\nclass S(unittest.TestCase):\n    def test_s(self): pass\n",
            ),
            (
                "pkg/nopkg/test_lost.py",
                "import unittest\nclass L(unittest.TestCase):\n    def test_l(self): pass\n",
            ),
            (".hidden/__init__.py", ""),
            (
                ".hidden/test_hidden.py",
                "import unittest\nclass Hid(unittest.TestCase):\n    def test_h(self): pass\n",
            ),
        ],
        exact: &[
            // `fnmatch` folds case on Windows: there `test*.py` takes it.
            #[cfg(windows)]
            "Test_upper.py::U::test_u",
            "pkg/__init__.py::InInit::test_init",
            "pkg/sub/test_sub.py::S::test_s",
            "pkg/test_pkg.py::K::test_k",
            "test_top.py::Top::test_top",
        ],
        // A package whose name has a dot can't be imported by its dotted
        // name: unittest reports one import error and loads nothing below.
        refined: &[".hidden/__init__.py"],
    },
    // imports: classes from the project's modules are read, others not
    Suite {
        name: "imports",
        files: &[
            (
                "helpers/__init__.py",
                "from .bases import BaseTestCase, SharedTests\n",
            ),
            (
                "helpers/bases.py",
                r"import unittest


class BaseTestCase(unittest.TestCase):
    def setUp(self):
        self.value = 1

    def check(self):
        pass


class SharedTests(BaseTestCase):
    def test_shared(self):
        pass
",
            ),
            (
                "helpers/util.py",
                "def make():\n    return 1\n\nCONSTANT = 2\n",
            ),
            (
                "test_uses.py",
                r"import os
import unittest
from unittest import TestCase, IsolatedAsyncioTestCase
from helpers import BaseTestCase
from helpers.util import make, CONSTANT
import helpers.util


class Uses(BaseTestCase):
    def test_uses(self):
        pass
",
            ),
            (
                "test_shares.py",
                r"from helpers.bases import SharedTests


class More(SharedTests):
    def test_more(self):
        pass
",
            ),
            ("test_relative/__init__.py", ""),
            (
                "test_relative/base.py",
                "import unittest\n\nclass RelBase(unittest.TestCase):\n    def test_rel(self): pass\n",
            ),
            (
                "test_relative/test_rel.py",
                "from .base import RelBase\n\nclass Rel(RelBase):\n    pass\n",
            ),
        ],
        exact: &[
            "helpers/__init__.py::SharedTests::test_shared",
            "test_relative/test_rel.py::Rel::test_rel",
            "test_relative/test_rel.py::RelBase::test_rel",
            "test_shares.py::More::test_more",
            "test_shares.py::More::test_shared",
            "test_shares.py::SharedTests::test_shared",
            "test_uses.py::Uses::test_uses",
        ],
        refined: &[],
    },
    // what real test modules hold besides tests: none of it needs an import
    Suite {
        name: "realistic",
        files: &[
            (
                "test_real.py",
                r#"# Tests of the cart.

from __future__ import annotations

import json
import logging
import os
import sys
import unittest
from typing import TYPE_CHECKING
from unittest import mock

try:
    import numpy
except ImportError:
    numpy = None

if TYPE_CHECKING:
    from helpers import Thing

try:
    from django.test import TestCase as DjangoTestCase
except ImportError:
    DjangoTestCase = None

__all__ = ["CartTests"]

logger = logging.getLogger(__name__)
HERE = os.path.dirname(os.path.abspath(__file__))
DATA = json.loads('{"a": 1}')
NAMES = ["a", "b"]
sys.path.insert(0, HERE)


def tagged(func):
    func.tag = "slow"
    return func


def register(cls):
    return cls


def build(value):
    global logger
    return value


@register
class CartTests(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls):
        cls.shared = 1

    def setUp(self):
        self.cart = []

    @tagged
    def test_add(self):
        self.cart.append(1)

    @mock.patch.object(os, "getcwd")
    def test_cwd(self, getcwd):
        pass

    @unittest.skipUnless(numpy, "needs numpy")
    def test_numpy(self):
        pass

    def helper(self):
        return build(1)


if __name__ == "__main__":
    unittest.main()
"#,
            ),
            ("tests/__init__.py", ""),
            (
                "tests/helpers.py",
                "import unittest\n\nclass HelperTests(unittest.TestCase):\n    def test_helper(self): pass\n",
            ),
            (
                "tests/test_hack.py",
                "import sys, os\nsys.path.insert(0, os.path.dirname(__file__))\nfrom helpers import HelperTests\n",
            ),
            (
                "tests/test_changes.py",
                "import unittest\n\ndef add_tests(cls):\n    cls.test_added = lambda self: None\n    return cls\n\n@add_tests\nclass Changed(unittest.TestCase):\n    pass\n",
            ),
        ],
        exact: &[
            "test_real.py::CartTests::test_add",
            "test_real.py::CartTests::test_cwd",
            "test_real.py::CartTests::test_numpy",
        ],
        refined: &["tests/test_changes.py", "tests/test_hack.py"],
    },
    // the MRO decides which binding of a name counts
    Suite {
        name: "mro",
        files: &[
            (
                "test_diamond.py",
                r"import unittest


class Base(unittest.TestCase):
    def test_who(self):
        pass

    def test_base(self):
        pass


class Left(Base):
    def test_left(self):
        pass


class Right(Base):
    test_who = None


class Diamond(Left, Right):
    pass
",
            ),
            (
                "test_init_subclass.py",
                r"import unittest


class Registers(unittest.TestCase):
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__(**kwargs)
        cls.test_added = lambda self: None


class Child(Registers):
    def test_own(self):
        pass
",
            ),
            (
                "test_conditional_member.py",
                r"import sys
import unittest


class C(unittest.TestCase):
    if sys.platform == 'win32':
        def test_windows(self):
            pass

    def test_always(self):
        pass
",
            ),
            (
                "test_bad_mro.py",
                r"import unittest


class A(unittest.TestCase):
    pass


class B(A):
    pass


class Bad(A, B):
    def test_never(self):
        pass
",
            ),
        ],
        exact: &[
            "test_diamond.py::Base::test_base",
            "test_diamond.py::Base::test_who",
            "test_diamond.py::Diamond::test_base",
            "test_diamond.py::Diamond::test_left",
            "test_diamond.py::Left::test_base",
            "test_diamond.py::Left::test_left",
            "test_diamond.py::Left::test_who",
            "test_diamond.py::Right::test_base",
        ],
        refined: &[
            "test_bad_mro.py",
            "test_conditional_member.py",
            "test_init_subclass.py",
        ],
    },
    // what only an import can tell
    Suite {
        name: "dynamic",
        files: &[
            (
                "test_load_tests.py",
                "import unittest\nclass A(unittest.TestCase):\n    def test_a(self): pass\n\
                 def load_tests(loader, tests, pattern):\n    return tests\n",
            ),
            (
                "test_conditional.py",
                "import sys, unittest\nif sys.version_info >= (3,):\n    class C(unittest.TestCase):\n        def test_c(self): pass\n",
            ),
            (
                "test_factory.py",
                "import unittest\ndef make():\n    class T(unittest.TestCase):\n        def test_t(self): pass\n    return T\nMade = make()\n",
            ),
            (
                "test_setattr.py",
                "import unittest\nclass S(unittest.TestCase):\n    pass\nfor i in range(2):\n    setattr(S, f'test_{i}', lambda self: None)\n",
            ),
            (
                "test_decorated.py",
                "import unittest\nfrom parameterized import parameterized\nclass P(unittest.TestCase):\n    @parameterized.expand([(1,), (2,)])\n    def test_p(self, x): pass\n",
            ),
            (
                "test_skipmod.py",
                "import unittest\nraise unittest.SkipTest('not here')\n",
            ),
            (
                "test_thirdparty.py",
                "from somelib.testing import LibTestCase\nclass T(LibTestCase):\n    def test_t(self): pass\n",
            ),
            (
                "test_meta.py",
                "import unittest\nclass Meta(type):\n    pass\nclass M(unittest.TestCase, metaclass=Meta):\n    def test_m(self): pass\n",
            ),
            ("test_star.py", "from helpers_star import *\n"),
            (
                "helpers_star.py",
                "import unittest\nclass Star(unittest.TestCase):\n    def test_star(self): pass\n",
            ),
            (
                "test_assigned.py",
                "import unittest\nclass A(unittest.TestCase):\n    pass\nA.test_late = lambda self: None\n",
            ),
            (
                "test_type.py",
                "import unittest\nT = type('T', (unittest.TestCase,), {'test_t': lambda self: None})\n",
            ),
            (
                "test_lib_tests.py",
                "from somelib.testing import SharedSerializerTests\nfrom somelib import helper\n",
            ),
            (
                "test_fine.py",
                "import unittest\nclass F(unittest.TestCase):\n    def test_f(self): pass\n",
            ),
        ],
        exact: &["test_fine.py::F::test_f"],
        refined: &[
            "test_assigned.py",
            "test_conditional.py",
            "test_decorated.py",
            "test_factory.py",
            "test_lib_tests.py",
            "test_load_tests.py",
            "test_meta.py",
            "test_setattr.py",
            "test_skipmod.py",
            "test_star.py",
            "test_thirdparty.py",
            "test_type.py",
        ],
    },
    // a package that may define load_tests decides its files
    Suite {
        name: "load_tests_package",
        files: &[
            (
                "lt/__init__.py",
                r"import os
import unittest


class InInit(unittest.TestCase):
    def test_init(self): pass


def load_tests(loader, standard_tests, pattern):
    this_dir = os.path.dirname(__file__)
    standard_tests.addTests(loader.discover(start_dir=this_dir, pattern=pattern))
    return standard_tests
",
            ),
            (
                "lt/test_l.py",
                "import unittest\nclass L(unittest.TestCase):\n    def test_l(self): pass\n",
            ),
            (
                "lazy/__init__.py",
                "def __getattr__(name):\n    raise AttributeError(name)\n",
            ),
            (
                "lazy/test_z.py",
                "import unittest\nclass Z(unittest.TestCase):\n    def test_z(self): pass\n",
            ),
            (
                "test_out.py",
                "import unittest\nclass O(unittest.TestCase):\n    def test_o(self): pass\n",
            ),
        ],
        exact: &["test_out.py::O::test_o"],
        refined: &[
            "lazy/__init__.py",
            "lazy/test_z.py",
            "lt/__init__.py",
            "lt/test_l.py",
        ],
    },
    // configuration: start and top-level directories, pattern
    Suite {
        name: "configured",
        files: &[
            (
                "pyproject.toml",
                "[tool.aye.unittest]\nstart-directory = \"tests\"\ntop-level-directory = \".\"\npattern = \"*_test.py\"\n",
            ),
            ("tests/__init__.py", ""),
            (
                "tests/cart_test.py",
                "import unittest\nclass Cart(unittest.TestCase):\n    def test_add(self): pass\n",
            ),
            (
                "tests/test_cart.py",
                "import unittest\nclass Other(unittest.TestCase):\n    def test_x(self): pass\n",
            ),
            ("tests/unit/__init__.py", ""),
            (
                "tests/unit/model_test.py",
                "from tests.cart_test import Cart\nclass Model(Cart):\n    def test_model(self): pass\n",
            ),
            (
                "other_test.py",
                "import unittest\nclass Outside(unittest.TestCase):\n    def test_o(self): pass\n",
            ),
        ],
        exact: &[
            "tests/cart_test.py::Cart::test_add",
            "tests/unit/model_test.py::Cart::test_add",
            "tests/unit/model_test.py::Model::test_add",
            "tests/unit/model_test.py::Model::test_model",
        ],
        refined: &[],
    },
    // what a test module's import does to the classes it loads, through
    // another module: assigned, deleted, given to a call, a classmethod
    // called on it; and what changes nothing
    Suite {
        name: "effects",
        files: &[
            (
                "helpers.py",
                r#"import unittest


class Base(unittest.TestCase):
    def test_base(self): pass


class DelBase(unittest.TestCase):
    def test_kept(self): pass
    def test_dropped(self): pass


class ScenarioBase(unittest.TestCase):
    @classmethod
    def add_scenario(cls, name, value):
        def test(self):
            self.assertTrue(value)
        setattr(cls, f"test_{name}", test)

    def test_static(self): pass


class CheckBase(unittest.TestCase):
    def test_static(self): pass


class LoopBase(unittest.TestCase):
    def test_static(self): pass


class Untouched(unittest.TestCase):
    def test_plain(self): pass


class Config:
    pass


def install(cls, names):
    for name in names:
        setattr(cls, f"test_{name}", lambda self: None)


def install_loop():
    LoopBase.test_loop = lambda self: None


def configure(cls):
    cls.flag = True
"#,
            ),
            (
                "test_attr.py",
                "import helpers\n\n\nclass Child(helpers.Base):\n    pass\n\n\ndef _extra(self):\n    pass\n\n\nhelpers.Base.test_extra = _extra\n",
            ),
            (
                "test_del.py",
                "import helpers\n\n\nclass Kid(helpers.DelBase):\n    pass\n\n\ndel helpers.DelBase.test_dropped\n",
            ),
            (
                "test_install.py",
                "from helpers import CheckBase, install\n\ninstall(CheckBase, [\"x\"])\n",
            ),
            (
                "test_loop.py",
                "from helpers import LoopBase, install_loop\n\ninstall_loop()\n",
            ),
            (
                "test_quiet.py",
                "import unittest\n\nimport helpers\nfrom helpers import Config, Untouched, configure\n\nconfigure(Config)\nhelpers.Config.flag = 1\nunittest.TestCase.maxDiff = None\n\n\nclass Q(Untouched):\n    pass\n",
            ),
            (
                "test_scen.py",
                "from helpers import ScenarioBase\n\nScenarioBase.add_scenario(\"good\", True)\nScenarioBase.add_scenario(\"bad\", False)\n",
            ),
        ],
        exact: &[
            "test_quiet.py::Q::test_plain",
            "test_quiet.py::Untouched::test_plain",
        ],
        refined: &[
            "test_attr.py",
            "test_del.py",
            "test_install.py",
            "test_loop.py",
            "test_scen.py",
        ],
    },
    // what the package's `__init__.py` (imported first) and a module the
    // test module imports do to its classes
    Suite {
        name: "package_effects",
        files: &[
            (
                "tests/__init__.py",
                "import tests.base\n\n\ndef _common(self):\n    pass\n\n\ntests.base.Base.test_common = _common\n",
            ),
            (
                "tests/base.py",
                "import unittest\n\n\nclass Base(unittest.TestCase):\n    def test_base(self): pass\n\n\nclass Other(unittest.TestCase):\n    def test_other(self): pass\n\n\nclass Third(unittest.TestCase):\n    def test_third(self): pass\n",
            ),
            (
                "tests/patches.py",
                "from tests import base\n\n\ndef _patched(self):\n    pass\n\n\nbase.Other.test_patched = _patched\n",
            ),
            (
                "tests/test_x.py",
                "from tests.base import Base\n\n\nclass X(Base):\n    def test_x(self): pass\n",
            ),
            (
                "tests/test_y.py",
                "import tests.patches\nfrom tests.base import Other\n\n\nclass Y(Other):\n    pass\n",
            ),
            (
                "tests/test_z.py",
                "from tests.base import Third\n\n\nclass Z(Third):\n    pass\n",
            ),
        ],
        exact: &[
            "tests/test_z.py::Third::test_third",
            "tests/test_z.py::Z::test_third",
        ],
        refined: &["tests/__init__.py", "tests/test_x.py", "tests/test_y.py"],
    },
    // a class is named by the name the loader finds it under: an alias, an
    // imported class's local name
    Suite {
        name: "bound_names",
        files: &[
            (
                "shared.py",
                "import unittest\n\n\nclass SharedTests(unittest.TestCase):\n    def test_shared(self): pass\n",
            ),
            (
                "test_api.py",
                "from shared import SharedTests as TestShared\n",
            ),
            (
                "test_alias.py",
                "import unittest\n\n\nclass Outer:\n    class Inner(unittest.TestCase):\n        def test_inner(self): pass\n\n\nInner = Outer.Inner\nTwice = Inner\n",
            ),
        ],
        exact: &["test_api.py::TestShared::test_shared"],
        // A nested class bound at module level is refined.
        refined: &["test_alias.py"],
    },
];
