//! Static soundness of classes, bindings and functions: what pytest may
//! collect that the source alone doesn't show is refined (Core decision 5),
//! and what is read statically matches pytest. Expected ids are `pytest
//! --collect-only -q` on pytest 9.1.1; `p1_suites_match_live_pytest`
//! re-checks every file left unrefined against the pytest at hand.

use aye_engine::Recognition;
use aye_events::Certainty;

use super::suites::Suite;
use super::{collect, differential, differential_metadata, materialize, python, write};

pub const SUITES: &[Suite] = &[
    // C0: classes handed to code aye can't follow (a loop over them, a helper, a classmethod, `__set_name__`, `__bases__`, `del K.attr`, `setattr`), and uses that leave members alone
    Suite {
        name: "handed_classes",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "helpers.py",
                r"def add_smoke(cls):
    def test_smoke(self):
        pass
    cls.test_smoke = test_smoke
",
            ),
            (
                "test_loop.py",
                r"def _extra(self):
    pass


class TestA:
    def test_a(self):
        pass


class TestB:
    def test_b(self):
        pass


for _cls in (TestA, TestB):
    _cls.test_extra = _extra
",
            ),
            (
                "test_helper.py",
                r"from helpers import add_smoke


class TestA:
    def test_a(self):
        pass


add_smoke(TestA)
",
            ),
            (
                "test_classmethod.py",
                r"def _extra(self):
    pass


class _Base:
    @classmethod
    def enable(cls):
        cls.test_enabled = _extra


class TestC(_Base):
    def test_c(self):
        pass


TestC.enable()
",
            ),
            (
                "test_set_name.py",
                r"def _extra(self):
    pass


class Registers:
    def __set_name__(self, owner, name):
        owner.test_registered = _extra


class TestD:
    hook = Registers()

    def test_d(self):
        pass
",
            ),
            (
                "test_bases.py",
                r"class _Extra:
    def test_rebased(self):
        pass


class _Plain:
    pass


class TestE(_Plain):
    def test_e(self):
        pass


TestE.__bases__ = (_Extra,)
",
            ),
            (
                "test_del.py",
                r"class TestB:
    def test_b(self):
        pass

    def test_keep(self):
        pass


del TestB.test_b
",
            ),
            (
                "test_setattr.py",
                r#"def _extra(self):
    pass


class TestS:
    def test_s(self):
        pass


setattr(TestS, "test_set", _extra)
"#,
            ),
            (
                "test_unittest_handed.py",
                r"import unittest


def _extra(self):
    pass


class Case(unittest.TestCase):
    def test_a(self):
        pass


def install(cls):
    cls.test_more = _extra


install(Case)
",
            ),
            (
                "test_subclass.py",
                r"def _extra(self):
    pass


class TestBase:
    def test_base(self):
        pass


class TestChild(TestBase):
    def test_child(self):
        pass


def install(cls):
    cls.test_more = _extra


install(TestBase)
",
            ),
            (
                "test_nested.py",
                r"def _extra(self):
    pass


class TestOuter:
    class TestInner:
        def test_i(self):
            pass


def install(cls):
    cls.test_more = _extra


install(TestOuter.TestInner)
",
            ),
            (
                "test_benign.py",
                r"import unittest


class TestK:
    maxDiff = None

    def test_k(self):
        assert isinstance(self, TestK)


TestK.flag = True


class Base(unittest.TestCase):
    def setUp(self):
        super(Base, self).setUp()

    def test_base(self):
        pass


class Child(Base):
    def setUp(self):
        super(Child, self).setUp()

    def test_child(self):
        assert issubclass(Child, Base)


ALL = [TestK]
NAMES = [cls.__name__ for cls in ALL]
",
            ),
        ],
        pytest: &[
            "test_bases.py::TestE::test_rebased",
            "test_bases.py::TestE::test_e",
            "test_benign.py::TestK::test_k",
            "test_benign.py::Base::test_base",
            "test_benign.py::Child::test_base",
            "test_benign.py::Child::test_child",
            "test_classmethod.py::TestC::test_c",
            "test_classmethod.py::TestC::test_enabled",
            "test_del.py::TestB::test_keep",
            "test_helper.py::TestA::test_a",
            "test_helper.py::TestA::test_smoke",
            "test_loop.py::TestA::test_a",
            "test_loop.py::TestA::test_extra",
            "test_loop.py::TestB::test_b",
            "test_loop.py::TestB::test_extra",
            "test_nested.py::TestOuter::TestInner::test_i",
            "test_nested.py::TestOuter::TestInner::test_more",
            "test_set_name.py::TestD::test_d",
            "test_set_name.py::TestD::test_registered",
            "test_setattr.py::TestS::test_s",
            "test_setattr.py::TestS::test_set",
            "test_subclass.py::TestBase::test_base",
            "test_subclass.py::TestBase::test_more",
            "test_subclass.py::TestChild::test_base",
            "test_subclass.py::TestChild::test_more",
            "test_subclass.py::TestChild::test_child",
            "test_unittest_handed.py::Case::test_a",
            "test_unittest_handed.py::Case::test_more",
        ],
        refined: &[
            "test_bases.py",
            "test_classmethod.py",
            "test_del.py",
            "test_helper.py",
            "test_loop.py",
            "test_nested.py",
            "test_set_name.py",
            "test_setattr.py",
            "test_subclass.py",
            "test_unittest_handed.py",
        ],
    },
    // C1: unittest cases and `__test__` classes made by calls and bound to any name
    Suite {
        name: "class_factories",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "helpers.py",
                r"import unittest


def make_case(backend):
    class Case(unittest.TestCase):
        def test_backend(self):
            pass
    return Case
",
            ),
            (
                "test_factory.py",
                r#"import unittest


def make_case(backend):
    class Case(unittest.TestCase):
        def test_backend(self):
            pass
    return Case


SqliteCase = make_case("sqlite")
"#,
            ),
            (
                "test_type.py",
                r#"import unittest


class _Mixin:
    def test_mixin(self):
        pass


MysqlCase = type("MysqlCase", (_Mixin, unittest.TestCase), {})
"#,
            ),
            (
                "test_dunder_factory.py",
                r#"def make_suite(flavour):
    class Suite:
        __test__ = True

        def test_flavour(self):
            pass
    return Suite


BadSuite = make_suite("bad")
"#,
            ),
            (
                "test_imported_factory.py",
                r#"from helpers import make_case

PgCase = make_case("pg")


def test_visible():
    pass
"#,
            ),
            (
                "test_lambda_factory.py",
                r#"import unittest

make = lambda: type("Made", (unittest.TestCase,), {"test_m": lambda self: None})
Made = make()
"#,
            ),
            (
                "test_method_factory.py",
                r#"import unittest


class Factory:
    @classmethod
    def make(cls):
        return type("Made", (unittest.TestCase,), {"test_m": lambda self: None})


Made = Factory.make()
"#,
            ),
            (
                "test_plain_calls.py",
                r#"import logging
import os


def load():
    return {"a": 1}


class Helper:
    pass


logger = logging.getLogger(__name__)
DATA = load()
helper = Helper()
HERE = os.path.dirname(__file__)


def test_visible():
    pass
"#,
            ),
        ],
        pytest: &[
            "test_dunder_factory.py::BadSuite::test_flavour",
            "test_factory.py::SqliteCase::test_backend",
            "test_imported_factory.py::PgCase::test_backend",
            "test_imported_factory.py::test_visible",
            "test_lambda_factory.py::Made::test_m",
            "test_method_factory.py::Made::test_m",
            "test_plain_calls.py::test_visible",
            "test_type.py::MysqlCase::test_mixin",
        ],
        refined: &[
            "test_dunder_factory.py",
            "test_factory.py",
            "test_imported_factory.py",
            "test_lambda_factory.py",
            "test_method_factory.py",
            "test_type.py",
        ],
    },
    // C2: `__test__` from another module, or set after the class body
    Suite {
        name: "dunder_test",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "contract.py",
                r"class StorageContract:
    __test__ = True

    def test_roundtrip(self):
        pass


def check_invariants():
    pass


check_invariants.__test__ = True


class Plain:
    def test_plain(self):
        pass
",
            ),
            (
                "test_imported_class.py",
                r"from contract import StorageContract


def test_visible():
    pass
",
            ),
            (
                "test_imported_function.py",
                r"from contract import check_invariants


def test_visible():
    pass
",
            ),
            (
                "test_subclass.py",
                r"from contract import StorageContract


class DiskStorage(StorageContract):
    pass
",
            ),
            (
                "test_alias.py",
                r"import contract

Shared = contract.StorageContract
",
            ),
            (
                "test_after_body.py",
                r"class LocalContract:
    def test_local_contract(self):
        pass


LocalContract.__test__ = True


class Derived(LocalContract):
    def test_derived(self):
        pass


class TestOff:
    def test_off(self):
        pass


TestOff.__test__ = False


class TestOn:
    __test__ = False

    def test_on(self):
        pass


TestOn.__test__ = True


def test_visible():
    pass
",
            ),
            (
                "test_conditional.py",
                r#"import sys


class TestMaybe:
    def test_maybe(self):
        pass


if sys.platform == "nonexistent":
    TestMaybe.__test__ = False


def test_visible():
    pass
"#,
            ),
            (
                "test_plain_import.py",
                r"from contract import Plain


def test_visible():
    pass
",
            ),
        ],
        pytest: &[
            "test_after_body.py::LocalContract::test_local_contract",
            "test_after_body.py::Derived::test_local_contract",
            "test_after_body.py::Derived::test_derived",
            "test_after_body.py::TestOn::test_on",
            "test_after_body.py::test_visible",
            "test_alias.py::Shared::test_roundtrip",
            "test_conditional.py::TestMaybe::test_maybe",
            "test_conditional.py::test_visible",
            "test_imported_class.py::StorageContract::test_roundtrip",
            "test_imported_class.py::test_visible",
            "test_imported_function.py::check_invariants",
            "test_imported_function.py::test_visible",
            "test_plain_import.py::test_visible",
            "test_subclass.py::StorageContract::test_roundtrip",
            "test_subclass.py::DiskStorage::test_roundtrip",
        ],
        refined: &[
            "test_alias.py",
            "test_conditional.py",
            "test_imported_class.py",
            "test_imported_function.py",
            "test_subclass.py",
        ],
    },
    // C3: tests bound from calls, whatever the callee's case
    Suite {
        name: "bound_by_calls",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_capitalized.py",
                r"import functools


def Check(n):
    def t():
        pass
    return t


test_positive = Check(1)


class Retry:
    def __init__(self, func):
        functools.update_wrapper(self, func)

    def __call__(self, *args, **kwargs):
        return self.__wrapped__(*args, **kwargs)


def _body():
    pass


test_retried = Retry(_body)


class Case:
    def __new__(cls, expected):
        def t():
            pass
        return t


test_case = Case(3)
",
            ),
            (
                "test_partial.py",
                r"import functools


def _impl(n):
    pass


test_partial = functools.partial(_impl, 1)
",
            ),
            (
                "test_instances.py",
                r"class Data:
    def __init__(self, n):
        self.n = n


test_data = Data(1)
test_list = list((1, 2))


def test_visible():
    pass
",
            ),
        ],
        pytest: &[
            "test_capitalized.py::test_positive",
            "test_capitalized.py::test_retried",
            "test_capitalized.py::test_case",
            "test_instances.py::test_visible",
            "test_partial.py::test_partial",
        ],
        refined: &["test_capitalized.py", "test_partial.py"],
    },
    // C4: tests bound or deleted through `global` by functions that run at import
    Suite {
        name: "globals",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_install.py",
                r"def _install():
    global test_installed

    def test_installed():
        pass


_install()


def _install_class():
    global TestMade

    class TestMade:
        def test_m(self):
            pass


_install_class()


def test_visible():
    pass
",
            ),
            (
                "test_remove.py",
                r"def test_removed():
    pass


def _remove():
    global test_removed
    del test_removed


_remove()


def test_visible():
    pass
",
            ),
            (
                "test_fixture_global.py",
                r"import pytest

_client = None
COUNT = 0


@pytest.fixture
def client():
    global _client
    _client = object()
    return _client


def _count():
    global COUNT
    COUNT = 1


_count()


def test_uses(client):
    pass
",
            ),
        ],
        pytest: &[
            "test_fixture_global.py::test_uses",
            "test_install.py::test_installed",
            "test_install.py::TestMade::test_m",
            "test_install.py::test_visible",
            "test_remove.py::test_visible",
        ],
        refined: &["test_install.py", "test_remove.py"],
    },
    // C7: a project mixin with a local unittest base or `__test__` class
    Suite {
        name: "unittest_mixins",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "mixins.py",
                r#"class ApiMixin:
    base_url = "http://localhost"
"#,
            ),
            (
                "test_mix.py",
                r"import unittest
from mixins import ApiMixin


class _BaseCase(unittest.TestCase):
    def test_reachable(self):
        pass


class UserChecks(ApiMixin, _BaseCase):
    pass
",
            ),
            (
                "test_mix_dunder.py",
                r"from mixins import ApiMixin


class _Contract:
    __test__ = True

    def test_reachable(self):
        pass


class OrderContract(ApiMixin, _Contract):
    pass
",
            ),
            (
                "test_mix_plain.py",
                r"from mixins import ApiMixin


class Settings(ApiMixin):
    pass


def test_visible():
    pass
",
            ),
        ],
        pytest: &[
            "test_mix.py::_BaseCase::test_reachable",
            "test_mix.py::UserChecks::test_reachable",
            "test_mix_dunder.py::_Contract::test_reachable",
            "test_mix_dunder.py::OrderContract::test_reachable",
            "test_mix_plain.py::test_visible",
        ],
        refined: &["test_mix.py", "test_mix_dunder.py"],
    },
    // C10: tests and `del`s in `with` and `try` bodies after a statement that may raise
    Suite {
        name: "suppressed",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_optional.py",
                r"import contextlib

with contextlib.suppress(ImportError):
    import not_installed_numpy_xyz

    def test_needs_numpy():
        pass


class TestOptional:
    with contextlib.suppress(ImportError):
        import also_missing_xyz

        def test_optional(self):
            pass

    def test_always(self):
        pass


def test_visible():
    pass
",
            ),
            (
                "test_del_guarded.py",
                r"def test_kept():
    pass


try:
    import not_installed_xyz
    del test_kept
except ImportError:
    pass
",
            ),
            (
                "test_with_plain.py",
                r"import contextlib

with contextlib.suppress(ImportError):
    def test_inside():
        pass


def test_visible():
    pass
",
            ),
        ],
        pytest: &[
            "test_del_guarded.py::test_kept",
            "test_optional.py::TestOptional::test_always",
            "test_optional.py::test_visible",
            "test_with_plain.py::test_inside",
            "test_with_plain.py::test_visible",
        ],
        refined: &["test_del_guarded.py", "test_optional.py"],
    },
    // C11: tests changed by calls after their definition
    Suite {
        name: "changed_after_definition",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_mark_statement.py",
                r#"import pytest


def test_n(n):
    pass


pytest.mark.parametrize("n", [1, 2])(test_n)
"#,
            ),
            (
                "test_disable.py",
                r"def disable(func):
    func.__test__ = False


def test_helper_like():
    pass


disable(test_helper_like)


def test_visible():
    pass
",
            ),
            (
                "test_vars.py",
                r#"def test_vars_disabled():
    pass


vars(test_vars_disabled)["__test__"] = False


def test_visible():
    pass
"#,
            ),
            (
                "test_loop_mark.py",
                r#"import pytest


def test_m(m):
    pass


for mk in [pytest.mark.parametrize("m", ["a", "b"])]:
    mk(test_m)
"#,
            ),
            (
                "test_method_attr.py",
                r"class TestK:
    def test_a(self):
        pass

    def test_b(self):
        pass


def disable(func):
    func.__test__ = False


disable(TestK.test_a)
",
            ),
            (
                "test_direct.py",
                r"def test_direct():
    pass


test_direct.__test__ = False


def test_visible():
    pass
",
            ),
        ],
        pytest: &[
            "test_direct.py::test_visible",
            "test_disable.py::test_visible",
            "test_loop_mark.py::test_m[a]",
            "test_loop_mark.py::test_m[b]",
            "test_mark_statement.py::test_n[1]",
            "test_mark_statement.py::test_n[2]",
            "test_method_attr.py::TestK::test_b",
            "test_vars.py::test_visible",
        ],
        refined: &[
            "test_disable.py",
            "test_loop_mark.py",
            "test_mark_statement.py",
            "test_method_attr.py",
            "test_vars.py",
        ],
    },
    // C14: class-private names as the class dict holds them
    Suite {
        name: "mangled",
        files: &[
            (
                "pyproject.toml",
                r#"[tool.pytest.ini_options]
python_functions = ["test_*", "*_check"]
python_classes = ["Test*", "*Checks"]
"#,
            ),
            (
                "tests/test_mangle.py",
                r"class TestK:
    def __private_check(self):
        pass

    def test_a(self):
        pass

    class __InnerChecks:
        def test_inner(self):
            pass


class _TestUnder:
    pass


class TestSub(TestK):
    def __private_check(self):
        pass
",
            ),
        ],
        pytest: &[
            "tests/test_mangle.py::TestK::_TestK__private_check",
            "tests/test_mangle.py::TestK::test_a",
            "tests/test_mangle.py::TestK::_TestK__InnerChecks::test_inner",
            "tests/test_mangle.py::TestSub::_TestK__private_check",
            "tests/test_mangle.py::TestSub::test_a",
            "tests/test_mangle.py::TestSub::_TestK__InnerChecks::test_inner",
            "tests/test_mangle.py::TestSub::_TestSub__private_check",
        ],
        refined: &[],
    },
    // C15: `from unittest import *` binds FunctionTestCase
    Suite {
        name: "star_unittest",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_star.py",
                r"from unittest import *


def test_a():
    pass
",
            ),
            (
                "test_star_case.py",
                r"from unittest.case import *


def test_a():
    pass
",
            ),
        ],
        pytest: &[
            "test_star.py::FunctionTestCase::runTest",
            "test_star.py::test_a",
            "test_star_case.py::FunctionTestCase::runTest",
            "test_star_case.py::test_a",
        ],
        refined: &["test_star.py", "test_star_case.py"],
    },
    // module-level code that may raise while the module is imported
    Suite {
        name: "raise_at_import",
        files: &[
            (
                "pytest.ini",
                r"[pytest]
",
            ),
            (
                "test_div.py",
                r"X = 1 / 0


def test_v():
    pass
",
            ),
            (
                "test_raise.py",
                r#"import unittest

raise unittest.SkipTest("not here")


def test_v():
    pass
"#,
            ),
            (
                "test_assert.py",
                r#"assert False, "never"


def test_v():
    pass
"#,
            ),
            (
                "test_fine.py",
                r#"X = 1 / 2
Y = "%s" % 0


def test_v():
    pass
"#,
            ),
        ],
        pytest: &["test_fine.py::test_v"],
        refined: &["test_assert.py", "test_div.py", "test_raise.py"],
    },
];

#[test]
fn p1_suites_match_pytest() {
    for suite in SUITES {
        let dir = materialize(suite);
        let found = collect(dir.path());
        let refined: Vec<&str> = found
            .iter()
            .filter(|(_, r)| r.refine.is_some())
            .map(|(rel, _)| rel.as_str())
            .collect();
        assert_eq!(
            refined, suite.refined,
            "suite {}: refined files",
            suite.name
        );
        let ours: Vec<&str> = found
            .iter()
            .filter(|(_, r)| r.refine.is_none())
            .flat_map(|(_, r)| r.cases.iter().map(|c| c.id.as_str()))
            .collect();
        let theirs: Vec<&str> = suite
            .pytest
            .iter()
            .copied()
            .filter(|id| {
                !suite
                    .refined
                    .contains(&id.split("::").next().unwrap_or_default())
            })
            .collect();
        assert_eq!(ours, theirs, "suite {}", suite.name);
    }
}

#[test]
fn p1_suites_match_live_pytest() {
    let Some(python) = python() else {
        eprintln!("skipped: no Python with pytest (set AYE_TEST_PYTHON)");
        return;
    };
    for suite in SUITES {
        let dir = materialize(suite);
        let excluded = differential(&python, dir.path())
            .unwrap_or_else(|report| panic!("suite {}: {report}", suite.name));
        differential_metadata(&python, dir.path(), &excluded)
            .unwrap_or_else(|report| panic!("suite {}: {report}", suite.name));
    }
}

/// The static guess of a refined file (what `--no-refine` keeps) holds no
/// exact id pytest doesn't make.
#[test]
fn static_guesses_hold_no_wrong_exact_id() {
    for suite in SUITES {
        let dir = materialize(suite);
        for (rel, recognition) in collect(dir.path()) {
            for case in &recognition.cases {
                assert!(
                    case.certainty != Certainty::Exact || suite.pytest.contains(&case.id.as_str()),
                    "suite {}: {rel}: {} is exact, pytest doesn't collect it",
                    suite.name,
                    case.id
                );
            }
        }
    }
}

fn certainties(recognition: &Recognition) -> Vec<(&str, Certainty)> {
    recognition
        .cases
        .iter()
        .map(|c| (c.id.as_str(), c.certainty))
        .collect()
}

/// A reason that holds for the whole module (it may raise or skip itself
/// at import, star imports, hooks) leaves no exact case in the guess.
#[test]
fn module_wide_reasons_leave_partial_guesses() {
    for source in [
        "import pytest\nX = range(0, 3, 0)\n@pytest.mark.parametrize('v', [1])\ndef test_v(v): pass\n",
        "X = 1 / 0\ndef test_v(): pass\n",
        "X = 1 % 0.0\ndef test_v(): pass\n",
        "import unittest\nraise unittest.SkipTest('no')\ndef test_v(): pass\n",
        "assert (), 'never'\ndef test_v(): pass\n",
        "from unittest import *\ndef test_v(): pass\n",
    ] {
        let recognition = super::recognize(source);
        assert!(recognition.refine.is_some(), "{source}");
        assert!(
            recognition
                .cases
                .iter()
                .all(|c| c.certainty == Certainty::Partial),
            "{source}: {:?}",
            certainties(&recognition)
        );
    }
    // Reasons of one test leave the others exact.
    let recognition = super::recognize(
        "def test_a(): pass\ndef disable(f): f.__test__ = False\ndef test_b(): pass\ndisable(test_b)\n",
    );
    assert_eq!(
        certainties(&recognition),
        [
            ("test_x.py::test_a", Certainty::Exact),
            ("test_x.py::test_b", Certainty::Partial)
        ]
    );
    // Arithmetic that can't raise is no reason.
    let recognition = super::recognize("X = 1 / 2\nY = '%s' % 0\nZ = 0 / 1\ndef test_v(): pass\n");
    assert_eq!(recognition.refine, None);
}

/// Refinement reasons name what implements a hook: a `-p` plugin, a plugin
/// a conftest loads, or a conftest.
#[test]
fn reasons_name_what_implements_hooks() {
    let plugin = "def pytest_make_parametrize_id(config, val, argname):\n    return f'P{val}'\n";
    let test = "import pytest\n@pytest.mark.parametrize('x', [3])\ndef test_hk(x):\n    pass\n";
    let reason = |files: &[(&str, &str)]| {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            write(dir.path(), rel, body);
        }
        let found = collect(dir.path());
        let (_, recognition) = found
            .iter()
            .find(|(rel, _)| rel == "tests/test_hk.py")
            .unwrap();
        recognition.refine.clone().unwrap_or_default()
    };
    let ini = "[tool.pytest.ini_options]\npythonpath = [\".\"]\n";
    assert_eq!(
        reason(&[
            (
                "pyproject.toml",
                &format!("{ini}addopts = \"-p idplugin\"\n")
            ),
            ("idplugin.py", plugin),
            ("tests/test_hk.py", test),
        ]),
        "a -p plugin implements pytest_make_parametrize_id"
    );
    assert_eq!(
        reason(&[
            ("pyproject.toml", ini),
            (
                "conftest.py",
                "import idplugin\ndef pytest_configure(config):\n    config.pluginmanager.register(idplugin, 'idplugin')\n"
            ),
            ("idplugin.py", plugin),
            ("tests/test_hk.py", test),
        ]),
        "a plugin a conftest.py loads implements pytest_make_parametrize_id"
    );
    assert_eq!(
        reason(&[
            ("pyproject.toml", ini),
            ("tests/conftest.py", plugin),
            ("tests/test_hk.py", test),
        ]),
        "a conftest.py implements pytest_make_parametrize_id"
    );
    assert_eq!(
        reason(&[
            ("pyproject.toml", ini),
            (
                "tests/conftest.py",
                "def pytest_pycollect_makeitem(collector, name, obj):\n    return None\n"
            ),
            ("tests/test_hk.py", "def test_a():\n    pass\n"),
        ]),
        "a conftest.py in scope implements pytest_pycollect_makeitem"
    );
}

/// A conditional `collect_ignore` may exclude the file: none of its cases
/// is exact.
#[test]
fn maybe_ignored_files_leave_partial_guesses() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "pytest.ini", "[pytest]\n");
    write(
        dir.path(),
        "conftest.py",
        "import sys\ncollect_ignore = []\nif sys.platform == 'nonexistent':\n    collect_ignore.append('test_a.py')\n",
    );
    write(dir.path(), "test_a.py", "def test_a():\n    pass\n");
    let found = collect(dir.path());
    let (_, recognition) = found.iter().find(|(rel, _)| rel == "test_a.py").unwrap();
    assert!(
        recognition
            .refine
            .as_deref()
            .is_some_and(|r| r.contains("collect_ignore")),
        "{:?}",
        recognition.refine
    );
    assert_eq!(
        certainties(recognition),
        [("test_a.py::test_a", Certainty::Partial)]
    );
}
