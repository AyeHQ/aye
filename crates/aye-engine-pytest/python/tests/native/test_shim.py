"""aye's own ``pytest`` module (``native/aye_pytest_shim.py``), compared with
real pytest wherever the behaviour is observable.

The shim is loaded from its file under another name (``aye_pytest_shim``), so
that the real pytest running these tests stays ``pytest``. What only happens
when it is installed as ``pytest`` (the ``_pytest`` aliases, shadowing an
installed pytest) runs in a subprocess, installed as the worker installs it.
"""

import builtins
import contextlib
import datetime
import importlib.util
import json
import os
import re
import subprocess
import sys
import warnings
from decimal import Decimal
from fractions import Fraction
from pathlib import Path

import _pytest.mark.structures
import pytest
from packaging.version import Version  # pytest's own dependency

HERE = Path(__file__).resolve().parent
SHIM_PATH = HERE.parents[1] / "native" / "aye_pytest_shim.py"
ENGINE = HERE.parents[3] / "aye-engine" / "python"
if str(ENGINE) not in sys.path:
    sys.path.insert(0, str(ENGINE))

aye_engine = importlib.import_module("aye_engine")

# Real pytest warns about the marks these tests make up; aye's module only
# does with a config.
pytestmark = pytest.mark.filterwarnings("ignore::pytest.PytestUnknownMarkWarning")


def load(name="aye_pytest_shim"):
    spec = importlib.util.spec_from_file_location(name, SHIM_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


shim = load()

PY311 = sys.version_info >= (3, 11)
# On Python 3.10, pytest's own backport (as the suites there use it).
ExceptionGroup = getattr(builtins, "ExceptionGroup", None) or getattr(
    importlib.util.find_spec("exceptiongroup") and importlib.import_module("exceptiongroup"),
    "ExceptionGroup",
    None,
)
needs_groups = pytest.mark.skipif(ExceptionGroup is None, reason="no exception groups")


def outcome(call):
    """What ``call()`` did: ``("ok", result)`` or ``("raised", type name,
    message)``, with addresses blanked out."""
    try:
        result = call()
    except BaseException as exc:  # noqa: BLE001 - what it raised is the point
        return ("raised", type(exc).__name__, blank(str(exc)))
    return ("ok", result)


def blank(text):
    return re.sub(r"0x[0-9a-fA-F]+", "0x…", text)


def same(call):
    """``call(module)`` does the same with the shim as with real pytest."""
    assert outcome(lambda: call(shim)) == outcome(lambda: call(pytest))


def marks_of(obj):
    return [(m.name, m.args, dict(m.kwargs)) for m in obj]


# ------------------------------------------------------------------ surface

# Every ``pytest.X`` that rich, click and packaging's tests, aye's examples
# and suites, and the corpus under /tmp/aye-native use (October 2026).
USED_NAMES = [
    "mark",
    "raises",
    "param",
    "fixture",
    "warns",
    "MonkeyPatch",
    "FixtureRequest",
    "skip",
    "CaptureFixture",
    "deprecated_call",
    "fail",
    "LogCaptureFixture",
    "approx",
    "importorskip",
    "TempPathFactory",
    "xfail",
    "Config",
    "Item",
    "Parser",
    "WarningsRecorder",
    "MarkDecorator",
    "hookimpl",
    "Function",
    "register_assert_rewrite",
    "Session",
    "Metafunc",
    "UsageError",
    "ExitCode",
    "ExceptionInfo",
    "PytestPluginManager",
    "PytestDeprecationWarning",
    "FixtureLookupError",
    "TestReport",
    "main",
    "StashKey",
    "Mark",
    "__file__",
]
# Used there, and not provided yet: annotations of pytester-based tests.
NOT_YET = ["Pytester", "RunResult"]
# Every mark name used there.
USED_MARKS = [
    "parametrize",
    "skipif",
    "usefixtures",
    "anyio",
    "benchmark",
    "xfail",
    "issue",
    "requires_internet",
    "c_extension",
    "filterwarnings",
    "thread_unsafe",
    "network",
    "skip",
    "timeout",
    "incompatible_with_mypyc",
    "slow",
    "dev_server",
    "property",
    "isolated",
    "expensive",
    "asyncio",
    "threaded",
    "webworkers",
    "gettz",
    "tzstr",
    "limit_memory",
    "with_jspi",
    "requires_git",
    "allow_backend_process",
    "syntax",
    "trio",
    "stress",
    "requires_unix",
    "hypothesis",
    "contextvars",
    "graalpy",
    "blackd",
    "smoke",
    "tzlocal",
    "integration",
    "flaky",
    "xdist_group",
    "capi",
    "db",
    "no_cover",
]


@pytest.mark.parametrize("name", USED_NAMES)
def test_every_name_suites_use_exists(name):
    assert hasattr(shim, name)


@pytest.mark.parametrize("name", NOT_YET)
def test_a_name_not_provided_says_so(name):
    assert not hasattr(shim, name)
    with pytest.raises(AttributeError) as info:
        getattr(shim, name)
    assert str(info.value) == (
        f"module 'pytest' has no attribute {name!r}: aye's pytest module doesn't provide "
        f"pytest.{name} yet (run the suite with pytest itself to use it)"
    )


def test_dunder_lookups_fail_plainly():
    with pytest.raises(AttributeError, match=r"^module 'pytest' has no attribute '__path__'$"):
        shim.__path__  # noqa: B018 - the lookup is the test


@pytest.mark.parametrize("name", USED_MARKS)
def test_every_mark_suites_use_is_a_mark(name):
    decorator = getattr(shim.mark, name)
    assert isinstance(decorator, shim.MarkDecorator)
    assert decorator.name == name


def test_pytest_x_y_names_suites_use():
    assert shim.ExitCode.USAGE_ERROR == 4
    with shim.MonkeyPatch.context() as patch:
        assert isinstance(patch, shim.MonkeyPatch)


def test_the_public_names_are_pytests():
    assert set(shim.__all__) <= set(pytest.__all__)
    for name in shim.__all__:
        assert hasattr(shim, name), name
    # What pytest exports and aye doesn't yet (each fails with the message).
    missing = sorted(set(pytest.__all__) - set(shim.__all__))
    assert missing == [
        "FixtureDef",
        "HookRecorder",
        "LineMatcher",
        "Pytester",
        "RaisesGroup",
        "RecordedHookCall",
        "RunResult",
        "SubtestReport",
        "Subtests",
        "TempdirFactory",
        "TerminalReporter",
        "TestShortLogReport",
        "Testdir",
        "freeze_includes",
        "register_fixture",
    ]
    for name in missing:
        assert not hasattr(shim, name)


def test_version():
    assert shim.__version__ == pytest.__version__ == "9.1.1"
    assert shim.version_tuple == (9, 1, 1)
    assert shim.__aye__ is True
    assert not hasattr(pytest, "__aye__")


def test_exit_codes_are_pytests():
    assert [(c.name, c.value) for c in shim.ExitCode] == [
        (c.name, c.value) for c in pytest.ExitCode
    ]
    assert shim.ExitCode.__module__ == "pytest"
    assert repr(shim.ExitCode(5)) == repr(pytest.ExitCode(5))


WARNINGS = [
    "PytestWarning",
    "PytestAssertRewriteWarning",
    "PytestCacheWarning",
    "PytestCollectionWarning",
    "PytestConfigWarning",
    "PytestDeprecationWarning",
    "PytestExperimentalApiWarning",
    "PytestFDWarning",
    "PytestRemovedIn10Warning",
    "PytestReturnNotNoneWarning",
    "PytestUnhandledThreadExceptionWarning",
    "PytestUnknownMarkWarning",
    "PytestUnraisableExceptionWarning",
]


@pytest.mark.parametrize("name", WARNINGS)
def test_warning_classes_are_pytests(name):
    ours, theirs = getattr(shim, name), getattr(pytest, name)
    assert [c.__name__ for c in ours.__mro__] == [c.__name__ for c in theirs.__mro__]
    assert ours.__module__ == "pytest"


def test_removed_in_9_warning_is_kept_for_suites_that_name_it():
    assert issubclass(shim.PytestRemovedIn9Warning, shim.PytestDeprecationWarning)


def test_experimental_api_warning():
    assert str(shim.PytestExperimentalApiWarning.simple("x")) == str(
        pytest.PytestExperimentalApiWarning.simple("x")
    )


def test_usage_error():
    assert shim.UsageError.__module__ == "pytest"
    assert issubclass(shim.UsageError, Exception)


@pytest.mark.parametrize(
    ("call", "name"),
    [
        (lambda: shim.main(["-x"]), "pytest.main()"),
        (shim.console_main, "pytest.console_main()"),
        (shim.cmdline.main, "pytest.main()"),
        (shim.set_trace, "pytest.set_trace()"),
    ],
)
def test_running_pytest_is_a_usage_error_naming_aye(call, name):
    with pytest.raises(shim.UsageError) as info:
        call()
    assert name in str(info.value)
    assert "aye" in str(info.value)


# ------------------------------------------------------------------ outcomes


MODULES = {"_pytest.outcomes": "pytest", "aye_pytest_shim": "pytest"}


@pytest.mark.parametrize(
    "call",
    [
        lambda p: p.skip("why"),
        lambda p: p.skip(),
        lambda p: p.skip("why", allow_module_level=True),
        lambda p: p.fail("bad"),
        lambda p: p.fail("bad", pytrace=False),
        lambda p: p.xfail("later"),
        lambda p: p.exit("bye", 3),
        lambda p: p.exit("bye"),
    ],
)
def test_outcome_helpers_raise_pytests_exceptions(call):
    def raised(module):
        try:
            call(module)
        except BaseException as exc:  # noqa: BLE001 - the outcome is the point
            return {
                "type": type(exc).__name__,
                "module": MODULES.get(type(exc).__module__, type(exc).__module__),
                "bases": [c.__name__ for c in type(exc).__mro__],
                "str": str(exc),
                "repr": repr(exc),
                **{
                    name: getattr(exc, name)
                    for name in ("msg", "pytrace", "allow_module_level", "returncode")
                    if hasattr(exc, name)
                },
            }
        return None

    assert raised(shim) == raised(pytest)


def test_outcome_exception_classes():
    assert shim.skip.Exception is shim.Skipped
    assert shim.fail.Exception is shim.Failed
    assert shim.xfail.Exception is shim.XFailed
    assert shim.exit.Exception is shim.Exit
    assert shim.raises.Exception is shim.fail.Exception
    assert issubclass(shim.XFailed, shim.Failed)
    assert not issubclass(shim.OutcomeException, Exception)
    assert issubclass(shim.Exit, Exception)
    assert shim.TEST_OUTCOME == (shim.OutcomeException, Exception)
    # pytest's own spelling, which aye_engine recognizes.
    assert shim.Skipped.__module__ == shim.Failed.__module__ == "builtins"


def test_outcome_reprs():
    same(
        lambda p: (repr(p.skip.Exception()), repr(p.fail.Exception("m")), str(p.xfail.Exception()))
    )


def test_a_message_that_is_no_string_is_a_type_error():
    same(lambda p: p.skip(3))
    same(lambda p: p.fail(["x"]))


def test_aye_engine_knows_the_skip():
    try:
        shim.skip("because")
    except BaseException as exc:  # noqa: BLE001 - the skip is the point
        assert aye_engine.is_skip(exc)
        assert aye_engine.skip_reason(exc) == "because"
    try:
        shim.fail("nope")
    except BaseException as exc:  # noqa: BLE001 - the failure is the point
        assert not aye_engine.is_skip(exc)


def test_importorskip(tmp_path, monkeypatch):
    (tmp_path / "shim_broken_mod.py").write_text("raise ImportError('inner')\n")
    (tmp_path / "shim_versioned_mod.py").write_text("__version__ = '1.2.post1'\n")
    (tmp_path / "shim_bad_version_mod.py").write_text("__version__ = 'not a version'\n")
    monkeypatch.syspath_prepend(str(tmp_path))
    same(lambda p: p.importorskip("shim_no_such_module_xyz"))
    same(lambda p: p.importorskip("shim_no_such_module_xyz", reason="no luck"))
    same(lambda p: p.importorskip("shim_broken_mod"))
    same(lambda p: p.importorskip("shim_broken_mod", exc_type=ImportError))
    same(lambda p: p.importorskip("shim_versioned_mod", minversion="1.3").__name__)
    same(lambda p: p.importorskip("shim_versioned_mod", minversion="1.2").__name__)
    same(lambda p: p.importorskip("shim_versioned_mod", minversion="1.2.post2"))
    same(lambda p: p.importorskip("json", minversion="1.0"))
    same(lambda p: p.importorskip("not a module"))
    # packaging's InvalidVersion is a ValueError; aye says the same.
    with pytest.raises(ValueError, match="Invalid version: 'not a version'"):
        shim.importorskip("shim_bad_version_mod", minversion="1.0")
    try:
        shim.importorskip("shim_no_such_module_xyz")
    except shim.Skipped as exc:
        assert exc.allow_module_level


def test_versions_order_as_packaging_orders_them():
    versions = [
        "1.0.dev0",
        "1.0a1.dev1",
        "1.0a1",
        "1.0alpha2",
        "1.0b1",
        "1.0rc1",
        "1.0c2",
        "1.0",
        "1.0.0",
        "1.0+local.7",
        "1.0+local.10",
        "1.0+abc",
        "1.0.post1.dev3",
        "1.0.post1",
        "1.0-2",
        "1.0r3",
        "1.0.1",
        "1.1.dev1",
        "2!0.1",
        "v1.2",
        "1.2_RC.3",
    ]
    ours = sorted(versions, key=shim._version_key)
    theirs = sorted(versions, key=Version)
    assert ours == theirs
    for a in versions:
        for b in versions:
            assert (shim._version_key(a) == shim._version_key(b)) == (Version(a) == Version(b))
            assert (shim._version_key(a) < shim._version_key(b)) == (Version(a) < Version(b))


# ------------------------------------------------------------------ marks


def decorate(p, kind):
    """The same marks, applied with the shim or with pytest."""

    @p.mark.slow
    @p.mark.parametrize("x", [1, 2], ids=["a", "b"])
    @p.mark.skipif(sys.platform == "nope", reason="never")
    def test_function(x):
        pass

    class Base:
        pass

    Base = p.mark.base(Base)

    class Sub(Base):
        def test_method(self):
            pass

    Sub = p.mark.sub(level=1)(Sub)
    Sub = p.mark.again(Sub)

    @p.mark.alone
    class Unrelated:
        pass

    return {"function": test_function, "base": Base, "sub": Sub, "unrelated": Unrelated}[kind]


def test_marks_on_a_function_are_pytests():
    ours, theirs = decorate(shim, "function"), decorate(pytest, "function")
    assert marks_of(ours.pytestmark) == marks_of(theirs.pytestmark)
    # The decorator nearest the def comes first.
    assert [m.name for m in ours.pytestmark] == ["skipif", "parametrize", "slow"]
    assert all(isinstance(m, shim.Mark) for m in ours.pytestmark)


def test_marks_on_classes_never_share_a_base_class_list():
    for p in (shim, pytest):
        base, sub = decorate(p, "base"), decorate(p, "sub")
        assert marks_of(base.pytestmark) == [("base", (), {})]
        assert marks_of(sub.__dict__["pytestmark"]) == [
            ("sub", (), {"level": 1}),
            ("again", (), {}),
        ]
        assert base.__dict__["pytestmark"] is not sub.__dict__["pytestmark"]
    for kind in ("base", "sub", "unrelated"):
        ours, theirs = decorate(shim, kind), decorate(pytest, kind)
        assert marks_of(ours.__dict__["pytestmark"]) == marks_of(theirs.__dict__["pytestmark"])
        assert marks_of(shim._get_unpacked_marks(ours)) == marks_of(
            _pytest.mark.structures.get_unpacked_marks(theirs)
        )
        assert marks_of(shim._get_unpacked_marks(ours, consider_mro=False)) == marks_of(
            _pytest.mark.structures.get_unpacked_marks(theirs, consider_mro=False)
        )


def test_reading_marks_takes_a_single_mark_or_a_list():
    module = type(sys)("m")
    module.pytestmark = shim.mark.one
    assert marks_of(shim._get_unpacked_marks(module)) == [("one", (), {})]
    module.pytestmark = [shim.mark.one, shim.mark.two(3).mark]
    assert marks_of(shim._get_unpacked_marks(module)) == [("one", (), {}), ("two", (3,), {})]
    module.pytestmark = ["nope"]
    with pytest.raises(TypeError, match="got 'nope' instead of Mark"):
        shim._get_unpacked_marks(module)
    with pytest.raises(TypeError, match="got 'nope' instead of Mark"):
        _pytest.mark.structures.get_unpacked_marks(module)


def test_marks_on_static_and_class_methods_go_on_the_function():
    for p in (shim, pytest):

        class Holder:
            @p.mark.a
            @staticmethod
            def static():
                pass

            @p.mark.b
            @classmethod
            def klass(cls):
                pass

        assert isinstance(Holder.__dict__["static"], staticmethod)
        assert marks_of(Holder.__dict__["static"].__func__.pytestmark) == [("a", (), {})]
        assert marks_of(Holder.__dict__["klass"].__func__.pytestmark) == [("b", (), {})]


def test_a_mark_called_with_anything_else_takes_it_as_arguments():
    def describe(p):
        lam = p.mark.x(lambda: 0)
        two = p.mark.x(len, len)
        kw = p.mark.x(len, reason="r")
        forced = p.mark.x.with_args(len)
        return [
            (type(d).__name__, d.name, len(d.args), dict(d.kwargs)) for d in (lam, two, kw, forced)
        ] + [d.args[0] is len for d in (two, kw, forced)]

    same(describe)


def test_mark_reprs_and_combining():
    same(lambda p: repr(p.mark.foo))
    same(lambda p: repr(p.mark.foo(1, x=2).mark))
    same(lambda p: repr(p.mark.foo(1)(2, y=3)))
    same(lambda p: p.mark.foo(1, x=1)(2, x=2) == p.mark.foo(1, 2, x=2))
    same(lambda p: (p.mark.foo.markname, p.mark.foo(1).args, p.mark.foo(a=1).kwargs))
    same(lambda p: p.mark._private)


def test_parametrize_remembers_which_side_carried_ids():
    for p in (shim, pytest):
        first = p.mark.parametrize("x", [1], ids=["one"]).mark
        second = p.mark.parametrize("y", [2]).mark
        assert first.combined_with(second)._param_ids_from is first
        assert second.combined_with(first)._param_ids_from is first
        assert second.combined_with(second)._param_ids_from is None


def test_a_mark_is_a_frozen_dataclass():
    mark = shim.Mark("x", (1,), {"a": 2})
    assert (mark.name, mark.args, mark.kwargs) == ("x", (1,), {"a": 2})
    with pytest.raises(AttributeError):
        mark.name = "y"
    assert mark == shim.Mark("x", (1,), {"a": 2})
    assert shim.MarkDecorator(mark).mark is mark


class FakeConfig:
    def __init__(self, markers=(), strict=False):
        self.ini = {"markers": list(markers), "strict_markers": strict, "strict": False}

    def getini(self, name):
        return self.ini[name]


def generator(p, config):
    if p is shim:
        gen = shim.MarkGenerator()
    else:
        gen = _pytest.mark.structures.MarkGenerator(_ispytest=True)
        # Builtin markers are registered lines in pytest; the shim knows them.
        config = FakeConfig(
            [*config.ini["markers"], "parametrize: x", "skip: x", "xfail(c): x"],
            config.ini["strict_markers"],
        )
    gen._config = config
    return gen


def test_registered_marks_pass_with_a_config():
    config = FakeConfig(["slow: slow tests", "db(name): needs a database"])
    for p in (shim, pytest):
        gen = generator(p, config)
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            assert gen.slow.name == "slow"
            assert gen.db.name == "db"
            assert gen.parametrize.name == "parametrize"
            assert gen.skip.name == "skip"


def test_a_new_config_forgets_the_markers_of_the_last():
    gen = shim.MarkGenerator()
    gen._config = FakeConfig(["slow: x"], strict=True)
    assert gen.slow.name == "slow"
    gen._config = FakeConfig([], strict=True)
    with pytest.raises(shim.fail.Exception, match="'slow' not found in `markers`"):
        gen.slow  # noqa: B018 - the lookup is the test
    gen._config = None
    assert gen.slow.name == "slow"


def test_unknown_marks_warn_or_fail_as_pytests():
    def unknown(p, strict):
        gen = generator(p, FakeConfig(["slow: x"], strict))
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            gen.typo  # noqa: B018 - the lookup is the test
        return [(w.category.__name__, str(w.message)) for w in caught]

    same(lambda p: unknown(p, False))
    same(lambda p: unknown(p, True))
    same(lambda p: generator(p, FakeConfig()).parameterize)
    assert outcome(lambda: unknown(shim, False))[1][0][0] == "PytestUnknownMarkWarning"


def test_marks_cannot_be_applied_to_fixtures():
    def mark_a_fixture(p):
        def fixture_function():
            pass

        return p.mark.x(p.fixture(fixture_function))

    def fixture_a_marked(p):
        @p.mark.x
        def fixture_function():
            pass

        return p.fixture(fixture_function)

    same(mark_a_fixture)
    same(fixture_a_marked)


# ------------------------------------------------------------------ param


def test_param_is_pytests_with_marks_as_marks():
    for kwargs in (
        {},
        {"id": "named"},
        {"marks": shim.mark.xfail},
        {"marks": [shim.mark.xfail(reason="r"), shim.mark.slow.mark]},
        {"id": shim.HIDDEN_PARAM},
    ):
        ours = shim.param(1, "two", **kwargs)
        theirs_kwargs = dict(kwargs)
        if "marks" in kwargs:
            marks = kwargs["marks"]
            marks = marks if isinstance(marks, list) else [marks]
            theirs_kwargs["marks"] = [
                getattr(pytest.mark, m.name)(*m.args, **m.kwargs) for m in marks
            ]
        if kwargs.get("id") is shim.HIDDEN_PARAM:
            theirs_kwargs["id"] = pytest.HIDDEN_PARAM
        theirs = pytest.param(1, "two", **theirs_kwargs)
        assert ours.values == theirs.values == (1, "two")
        assert repr(ours.id) == repr(theirs.id)
        assert marks_of(ours.marks) == marks_of(m.mark for m in theirs.marks)
        assert all(isinstance(m, shim.Mark) for m in ours.marks)
        assert isinstance(ours, tuple)
        assert ours._fields == ("values", "marks", "id")


def test_param_errors_are_pytests():
    same(lambda p: p.param(1, marks=p.mark.usefixtures("a")))
    same(lambda p: p.param(1, id=3))
    same(lambda p: p.param(1, marks=[p.mark.ok, p.mark.usefixtures("a")]))
    with pytest.raises(TypeError, match="got 'nope' instead of Mark"):
        shim.param(1, marks=["nope"])
    with pytest.raises(TypeError, match="takes a mark or a collection of marks"):
        shim.param(1, marks=3)


def test_hidden_param():
    assert repr(shim.HIDDEN_PARAM) == repr(pytest.HIDDEN_PARAM)


def test_parameter_set_helpers_are_pytests():
    ours, theirs = shim.ParameterSet, _pytest.mark.structures.ParameterSet
    for args in (("x", [1]), ("x,", [(1,)]), (" a , b ", [(1, 2)]), (["a", "b"], [(1, 2)])):
        assert ours._parse_parametrize_args(*args) == theirs._parse_parametrize_args(*args)
    for value, force in (((1, 2), False), (3, True), ((3,), True)):
        a, b = ours.extract_from(value, force), theirs.extract_from(value, force)
        assert (a.values, a.id, list(a.marks)) == (b.values, b.id, list(b.marks))
    p = shim.param(5)
    assert ours.extract_from(p) is p


# ------------------------------------------------------------------ fixtures


def test_fixture_bare_returns_the_function_with_its_spec():
    def thing():
        return 1

    assert shim.fixture(thing) is thing
    assert thing._aye_fixture == shim.FixtureSpec("function", None, False, None, None)


def test_fixture_called():
    def ids(value):
        return str(value)

    @shim.fixture(scope="module", params=range(3), autouse=True, ids=["a", "b", "c"], name="n")
    def first():
        pass

    @shim.fixture(params=[{"k": 1}], ids=ids)
    def second(request):
        pass

    assert first._aye_fixture == shim.FixtureSpec(
        scope="module", params=[0, 1, 2], autouse=True, ids=("a", "b", "c"), name="n"
    )
    assert second._aye_fixture.params == [{"k": 1}]
    assert second._aye_fixture.ids is ids
    spec = shim.fixture(scope="session")
    assert isinstance(spec, shim.FixtureSpec)
    assert spec.scope == "session"


def test_dynamic_scope_is_kept_for_the_front_end():
    def scope(fixture_name, config):
        return "module"

    @shim.fixture(scope=scope)
    def dynamic():
        pass

    assert dynamic._aye_fixture.scope is scope


def test_fixture_methods_stay_methods():
    class Holder:
        @shim.fixture
        def value(self):
            return self

    holder = Holder()
    assert holder.value() is holder
    assert Holder.__dict__["value"]._aye_fixture.scope == "function"


def test_fixture_errors_are_pytests():
    same(lambda p: p.fixture(type("C", (), {})))

    def twice(p):
        def thing():
            pass

        return p.fixture(p.fixture(thing))

    same(twice)

    def request():
        pass

    def reserved(p):
        return p.fixture(request)

    same(reserved)

    def renamed(p):
        def other():
            pass

        return p.fixture(name="request")(other)

    same(renamed)


def test_yield_fixture_is_a_deprecated_alias():
    def warned(p):
        def thing():
            pass

        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            p.yield_fixture(thing)
        return [(w.category.__name__, str(w.message)) for w in caught]

    same(warned)

    def thing():
        pass

    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        assert shim.yield_fixture(scope="module")(thing) is thing
    assert thing._aye_fixture.scope == "module"


def test_fixture_lookup_error_is_the_engines_once_it_is_loaded(monkeypatch):
    class EngineError(LookupError):
        pass

    engine = type(sys)("aye_native_engine")
    engine.FixtureLookupError = EngineError
    monkeypatch.setitem(sys.modules, "aye_native_engine", engine)
    assert shim.FixtureLookupError is EngineError
    with pytest.raises(shim.FixtureLookupError):
        raise EngineError("fixture 'x' not found")
    engine.FixtureLookupError = "not a class"
    assert shim.FixtureLookupError.__name__ == "FixtureLookupError"
    assert shim.FixtureLookupError is not EngineError


def test_fixture_lookup_error(monkeypatch):
    # Without the engine loaded (other tests in the same session load it).
    monkeypatch.delitem(sys.modules, "aye_native_engine", raising=False)
    error = shim.FixtureLookupError("db", None)
    assert isinstance(error, LookupError)
    assert str(error) == "fixture 'db' not found"
    assert (error.argname, error.request, error.msg) == ("db", None, None)
    assert str(shim.FixtureLookupError("db", None, "recursive")) == "recursive"


# ------------------------------------------------------------------ raises


def never(exc):
    return False


@pytest.mark.parametrize(
    "block",
    [
        ("raises", (ValueError,), {}, None),
        ("raises", ((ValueError, KeyError),), {}, None),
        ("raises", (), {"match": "x"}, None),
        ("raises", (ValueError,), {"match": "abc"}, ValueError("xyz")),
        ("raises", (ValueError,), {"match": "xyz"}, ValueError("the xyz")),
        ("raises", (ValueError,), {"match": "a.c"}, ValueError("a.c")),
        ("raises", (ValueError,), {"match": r"a\.c?"}, ValueError("a.c?")),
        ("raises", (ValueError,), {"match": "^hello world$"}, ValueError("hello there")),
        ("raises", (ValueError,), {"match": r"^x\(y\)$"}, ValueError("x(z)")),
        ("raises", (ValueError,), {"match": re.compile("abc", re.IGNORECASE)}, ValueError("xyz")),
        ("raises", (ValueError,), {"match": "ABC"}, ValueError("abc")),
        ("raises", (ValueError,), {"check": never}, ValueError("x")),
        ("raises", (ValueError,), {"check": lambda e: True}, ValueError("x")),
        ("raises", (ValueError,), {}, KeyError("k")),
        ("raises", (KeyboardInterrupt,), {}, KeyboardInterrupt()),
        ("raises", (Exception,), {}, ZeroDivisionError("z")),
        ("raises", (ValueError,), {"match": "note"}, "noted"),
    ],
)
def test_raises_as_a_context_manager_is_pytests(block):
    _, args, kwargs, exc = block

    def run(p):
        with p.raises(*args, **kwargs) as info:
            if exc == "noted":
                error = ValueError("x")
                if PY311:
                    error.add_note("a note")
                else:
                    error.__notes__ = ["a note"]
                raise error
            if exc is not None:
                raise exc
        return (info.typename, str(info.value))

    same(run)


@pytest.mark.parametrize(
    "call",
    [
        lambda p: p.raises(ZeroDivisionError, lambda: 1 / 0).typename,
        lambda p: p.raises(ValueError, int, "x").typename,
        lambda p: p.raises(KeyError, {}.__getitem__, "k").value.args,
        lambda p: p.raises(ValueError, lambda: None),
        lambda p: p.raises(ValueError, foo=1),
        lambda p: p.raises((), lambda: 1),
        lambda p: p.raises(ValueError, 3),
        lambda p: p.raises(ValueError("x")),
        lambda p: p.raises(int),
        lambda p: p.raises("ValueError"),
        lambda p: p.raises(),
        lambda p: p.raises(ValueError, match="("),
        lambda p: repr(p.raises(ValueError, match="a", check=len)),
        lambda p: repr(p.raises((ValueError, KeyError), match=re.compile("a", re.IGNORECASE))),
        lambda p: p.raises(ValueError).matches(None),
        lambda p: (p.raises(ValueError).matches(KeyError()), p.raises(ValueError).fail_reason),
    ],
)
def test_raises_calls_and_errors_are_pytests(call):
    same(call)


def test_an_empty_match_warns():
    def warned(p):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            p.raises(ValueError, match="")
        return [(w.category.__name__, str(w.message)) for w in caught]

    same(warned)


def test_raises_returns_the_exception_info():
    with shim.raises(ValueError) as info:
        raise ValueError("boom")
    assert info.type is ValueError
    assert str(info.value) == "boom"
    assert info.tb is info.value.__traceback__
    assert info.errisinstance(ValueError)
    assert info.errisinstance((KeyError, ValueError))
    assert not info.errisinstance(KeyError)
    assert info.match("bo+m") is True
    assert isinstance(info.getrepr(), object)
    assert "ValueError: boom" in str(info.getrepr())


def exception_info(p, raiser):
    with p.raises(BaseException) as info:
        raiser()
    return info


def boom():
    raise ValueError("boom")


def assertion():
    raise AssertionError("assert 1 == 2")


def assertion_message():
    raise AssertionError("custom")


@pytest.mark.parametrize("raiser", [boom, assertion, assertion_message])
def test_exception_info_reads_as_pytests(raiser):
    def read(p):
        info = exception_info(p, raiser)
        entries = info.traceback
        return {
            "repr": repr(info),
            "typename": info.typename,
            "exconly": info.exconly(),
            "short": info.exconly(tryshort=True),
            "linenos": [entry.lineno for entry in entries],
            "names": [entry.name for entry in entries],
            "paths": [str(entry.path) for entry in entries],
            "relline": [entry.relline for entry in entries],
            "firstline": [entry.getfirstlinesource() for entry in entries],
            "str": [str(entry) for entry in entries],
            "repr_entries": [repr(entry) for entry in entries],
            "hidden": [entry.ishidden(info) for entry in entries],
            "slice": type(entries[1:]).__name__,
            "from_exception": ExceptionInfoView(p.ExceptionInfo.from_exception(info.value)),
        }

    assert read(shim) == read(pytest)


class ExceptionInfoView:
    """What an ExceptionInfo says, for comparing two."""

    def __init__(self, info):
        self.data = (info.typename, info.exconly(), info.exconly(tryshort=True), str(info.value))

    def __eq__(self, other):
        return self.data == other.data

    __hash__ = None


def test_exception_info_before_the_block_exits_is_pytests():
    same(lambda p: repr(p.raises(ValueError).__enter__()))
    for attribute in ("type", "value", "tb", "typename"):
        same(lambda p, attribute=attribute: getattr(p.raises(ValueError).__enter__(), attribute))


def test_exception_info_match_failures_are_pytests():
    def match(p, pattern, message):
        with p.raises(ValueError) as info:
            raise ValueError(message)
        return info.match(pattern)

    same(lambda p: match(p, "abc", "xyz"))
    same(lambda p: match(p, "a.c", "a.c"))
    same(lambda p: match(p, "a.c?", "a.c?"))
    same(lambda p: match(p, "a(b)", "a(b)"))


def test_exception_info_constructors():
    try:
        boom()
    except ValueError:
        current = shim.ExceptionInfo.from_current()
    assert current.typename == "ValueError"
    later = shim.ExceptionInfo.for_later()
    assert repr(later) == "<ExceptionInfo for raises contextmanager>"
    later.fill_unfilled((type(current.value), current.value, current.tb))
    assert later.value is current.value
    with pytest.raises(AssertionError, match="already filled"):
        later.fill_unfilled((type(current.value), current.value, current.tb))
    same(lambda p: p.ExceptionInfo.from_exception(ValueError("never raised")))
    assert shim.ExceptionInfo[ValueError] is not None


def test_traceback_helpers():
    def outer():
        inner()

    def inner():
        boom()

    with shim.raises(ValueError) as info:
        outer()
    entries = info.traceback
    names = [entry.name for entry in entries]
    assert names[-3:] == ["outer", "inner", "boom"]
    assert isinstance(entries[1:], shim.Traceback)
    cut = entries.cut(path=__file__, firstlineno=inner.__code__.co_firstlineno - 1)
    assert [entry.name for entry in cut] == ["inner", "boom"]
    assert [e.name for e in entries.filter(lambda e: e.name != "inner")][-2:] == ["outer", "boom"]
    assert entries.recursionindex() is None
    assert entries[-1].locals == {}
    assert entries[-1].frame.code.name == "boom"


def test_traceback_entries_read_as_werkzeug_expects():
    # werkzeug's tests read "file':line" and the source line from str(entry).
    with shim.raises(ValueError) as info:
        boom()
    text = "".join(str(line) for line in info.traceback)
    assert f"test_shim.py':{boom.__code__.co_firstlineno + 1}" in text
    assert 'raise ValueError("boom")' in text


@needs_groups
def test_exception_groups_are_pytests():
    group = ExceptionGroup("many", [ValueError("v"), ExceptionGroup("inner", [KeyError("k")])])

    def raise_group():
        raise group

    def contains(p, *args, **kwargs):
        info = exception_info(p, raise_group)
        return info.group_contains(*args, **kwargs)

    same(lambda p: contains(p, ValueError))
    same(lambda p: contains(p, KeyError))
    same(lambda p: contains(p, KeyError, depth=1))
    same(lambda p: contains(p, KeyError, depth=2))
    same(lambda p: contains(p, ValueError, match="^v$"))
    same(lambda p: contains(p, ValueError, match="^w$"))
    same(lambda p: contains(p, ValueError, depth=0))
    same(lambda p: exception_info(p, boom).group_contains(ValueError))
    same(lambda p: exception_info(p, raise_group).exconly(tryshort=True))
    same(lambda p: p.raises(ExceptionGroup[Exception], raise_group).typename)
    same(lambda p: p.raises(ExceptionGroup[ValueError]))

    def group_match(p, pattern):
        with p.raises(ExceptionGroup, match=pattern):
            raise group

    same(lambda p: group_match(p, "^many$"))
    same(lambda p: group_match(p, "inner"))

    def nested(p):
        with p.raises(ValueError):
            raise group

    same(nested)


# ------------------------------------------------------------------ warns


def emit(*specs):
    for message, category in specs:
        warnings.warn(message, category, stacklevel=1)


@pytest.mark.parametrize(
    ("expected", "kwargs", "emitted"),
    [
        (UserWarning, {}, []),
        (UserWarning, {}, [("a", UserWarning)]),
        (UserWarning, {}, [("d", DeprecationWarning)]),
        (UserWarning, {"match": "abc"}, [("xyz", UserWarning)]),
        (UserWarning, {"match": "a.c"}, [("a.c", UserWarning)]),
        (UserWarning, {"match": "x.z"}, [("x.z?", UserWarning)]),
        ((UserWarning, DeprecationWarning), {}, [("d", DeprecationWarning)]),
        (Warning, {}, [("r", RuntimeWarning)]),
    ],
)
def test_warns_is_pytests(expected, kwargs, emitted):
    def run(p):
        with warnings.catch_warnings(record=True) as outer:
            warnings.simplefilter("always")
            try:
                with p.warns(expected, **kwargs) as record:
                    emit(*emitted)
            finally:
                reemitted = [(w.category.__name__, str(w.message)) for w in outer]
        return ([str(w.message) for w in record], reemitted)

    ours = outcome(lambda: run(shim))
    theirs = outcome(lambda: run(pytest))
    assert ours == theirs


def test_warns_lets_unmatched_warnings_reach_outer_filters():
    def run(p):
        with warnings.catch_warnings(record=True) as outer:
            warnings.simplefilter("always")
            with p.warns(UserWarning, match="keep"):
                emit(("keep me", UserWarning), ("drop me", UserWarning), ("d", DeprecationWarning))
        return [(w.category.__name__, str(w.message)) for w in outer]

    same(run)
    assert outcome(lambda: run(shim))[1] == [
        ("UserWarning", "drop me"),
        ("DeprecationWarning", "d"),
    ]


@pytest.mark.parametrize(
    "call",
    [
        lambda p: p.warns(ValueError),
        lambda p: p.warns((UserWarning, ValueError)),
        lambda p: p.warns(UserWarning, foo=1),
        lambda p: p.warns(UserWarning, 3),
        lambda p: p.warns(UserWarning, lambda: emit(("x", UserWarning)) or 7),
        lambda p: p.warns(UserWarning, lambda x: x * 2, 21),
        lambda p: p.deprecated_call(lambda: emit(("x", DeprecationWarning)) or "done"),
        lambda p: p.deprecated_call(lambda: emit(("x", FutureWarning)) or "done"),
        lambda p: p.deprecated_call(lambda: "nothing"),
    ],
)
def test_warns_calls_and_errors_are_pytests(call):
    def run(p):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            return call(p)

    same(run)


def test_deprecated_call_as_a_context_manager():
    def run(p, category, match=None):
        with warnings.catch_warnings():
            # An unmatched warning is re-emitted to the outer filters.
            warnings.simplefilter("ignore")
            with p.deprecated_call(match=match) as record:
                emit(("old api", category))
        return [w.category.__name__ for w in record]

    same(lambda p: run(p, PendingDeprecationWarning))
    same(lambda p: run(p, DeprecationWarning, match="^old"))
    same(lambda p: run(p, DeprecationWarning, match="new"))


def test_warns_lets_control_flow_through():
    def skipping(p):
        with p.warns(UserWarning):
            p.skip("skipped inside")

    def exiting(p):
        with p.warns(UserWarning):
            p.exit("bye")

    def failing(p):
        with p.warns(UserWarning):
            raise ValueError("an error, no warning")

    same(skipping)
    same(exiting)
    same(failing)


def test_warns_guards_non_string_user_warnings():
    def run(p):
        with p.warns(UserWarning):
            warnings.warn(UserWarning(42), stacklevel=1)

    same(run)


def test_warnings_recorder_is_pytests():
    def record(p):
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            recorder = p.WarningsRecorder(_ispytest=True)
            with recorder:
                emit(("u", UserWarning), ("d", DeprecationWarning), ("r", RuntimeWarning))
                emit(("p", p.PytestWarning))
        popped = [
            str(recorder.pop(DeprecationWarning).message),
            str(recorder.pop(UserWarning).message),
        ]
        rest = [str(w.message) for w in recorder]
        first = str(recorder[0].message)
        length = len(recorder)
        listed = len(recorder.list)
        recorder.clear()
        return popped, rest, first, length, listed, len(recorder)

    same(record)
    same(lambda p: p.WarningsRecorder(_ispytest=True).pop(UserWarning))

    def twice(p):
        recorder = p.WarningsRecorder(_ispytest=True)
        with recorder, recorder:
            pass

    assert outcome(lambda: twice(shim))[:2] == outcome(lambda: twice(pytest))[:2]

    def reused(p):
        recorder = p.WarningsRecorder(_ispytest=True)
        with recorder:
            emit(("one", UserWarning))
        with recorder:
            emit(("two", UserWarning))
        return [str(w.message) for w in recorder]

    same(reused)
    with pytest.raises(RuntimeError, match="without entering first"):
        shim.WarningsRecorder().__exit__(None, None, None)


# ------------------------------------------------------------------ approx

APPROX_EXPECTED = [
    (1.0, {}),
    (0.0, {}),
    (1.5, {"rel": 1e-3}),
    (1.5, {"abs": 0.1}),
    (1.5, {"rel": 0.5, "abs": 0.1}),
    (123456.0, {}),
    (1e9, {}),
    (1e-300, {}),
    (1e-5, {}),
    (-2.5, {}),
    (float("inf"), {}),
    (float("-inf"), {}),
    (float("nan"), {}),
    (float("nan"), {"nan_ok": True}),
    (True, {}),
    (False, {}),
    (None, {}),
    ("text", {}),
    (b"bytes", {}),
    (3, {}),
    (1 + 2j, {}),
    (2j, {}),
    (Fraction(1, 3), {}),
    (Decimal("1.5"), {}),
    (Decimal("1.5"), {"rel": Decimal("1e-2")}),
    (Decimal("1.5"), {"abs": 0.001}),
    ([1, 2.0, None, "x"], {}),
    ((0.1, 0.2), {"rel": 1e-2}),
    ({"a": 1.0, "b": None}, {}),
    ({"a": 1.0, "b": [1]}, {}),
    (range(3), {}),
    (datetime.timedelta(seconds=10), {"abs": datetime.timedelta(seconds=1)}),
    (datetime.timedelta(seconds=10), {"rel": 0.1}),
    (datetime.timedelta(seconds=10), {"rel": 0.1, "abs": datetime.timedelta(seconds=2)}),
    (
        datetime.datetime(2024, 1, 1, 12, tzinfo=datetime.timezone.utc),
        {"abs": datetime.timedelta(seconds=1)},
    ),
]

APPROX_ACTUAL = [
    1.0,
    1.0000001,
    1.001,
    0.0,
    1e-13,
    1.5,
    1.4,
    1.6,
    1.50001,
    123456.1,
    1e9 + 1,
    -2.5,
    float("inf"),
    float("-inf"),
    float("nan"),
    True,
    False,
    1,
    0,
    None,
    "text",
    b"bytes",
    3,
    1 + 2j,
    1 + 2.000001j,
    Fraction(1, 3),
    0.3333333,
    Decimal("1.5"),
    Decimal("1.5001"),
    [1, 2.0, None, "x"],
    [1, 2.0000001, None, "x"],
    [1, 2.0],
    (0.1, 0.2),
    (0.1001, 0.2),
    [0.1, 0.2],
    {"a": 1.0, "b": None},
    {"a": 1.0000001, "b": None},
    {"a": 1.0},
    {"b": None, "a": 1.0},
    [0, 1, 2],
    datetime.timedelta(seconds=10.5),
    datetime.timedelta(seconds=12),
    datetime.datetime(2024, 1, 1, 12, 0, 0, 500000, tzinfo=datetime.timezone.utc),
    datetime.datetime(2024, 1, 1, 12, 0, 2, tzinfo=datetime.timezone.utc),
    object(),
]


@pytest.mark.parametrize(("expected", "kwargs"), APPROX_EXPECTED, ids=repr)
def test_approx_reprs_are_pytests(expected, kwargs):
    same(lambda p: repr(p.approx(expected, **kwargs)))


@pytest.mark.parametrize(("expected", "kwargs"), APPROX_EXPECTED, ids=repr)
def test_approx_comparisons_are_pytests(expected, kwargs):
    def compare(p):
        approx = p.approx(expected, **kwargs)
        results = []
        for actual in APPROX_ACTUAL:
            results.append(outcome(lambda actual=actual: (actual == approx, actual != approx)))
            results.append(outcome(lambda actual=actual: approx == actual))
        return results

    same(compare)


@pytest.mark.parametrize(
    "call",
    [
        lambda p: bool(p.approx(1)),
        lambda p: p.approx({1, 2}),
        lambda p: p.approx(frozenset([1])),
        lambda p: p.approx([[1]]),
        lambda p: p.approx({"a": {"b": 1}}),
        lambda p: p.approx(datetime.timedelta(1)),
        lambda p: p.approx(datetime.datetime(2024, 1, 1, tzinfo=datetime.timezone.utc), rel=0.1),
        lambda p: p.approx(datetime.timedelta(1), abs=1),
        lambda p: p.approx(datetime.timedelta(1), abs=-datetime.timedelta(1)),
        lambda p: p.approx(datetime.timedelta(1), rel=-1),
        lambda p: p.approx(datetime.timedelta(1), rel="x"),
        lambda p: p.approx(datetime.timedelta(1), rel=float("nan")),
        lambda p: p.approx(datetime.timedelta(1), abs=datetime.timedelta(1), nan_ok=True),
        lambda p: 1.1 == p.approx(1, abs=-1),
        lambda p: 1.1 == p.approx(1, rel=-1),
        lambda p: 1.1 == p.approx(1, abs=float("nan")),
        lambda p: 1.1 == p.approx(1, rel=float("nan")),
        lambda p: repr(p.approx(1, rel=-1)),
        lambda p: hash(p.approx(1)),
        lambda p: p.approx(1) < 2,
        lambda p: p.approx(1.0, abs=1e-3).tolerance,
        lambda p: p.approx(1.0).tolerance,
        lambda p: p.approx(1.0, rel=1e-3, abs=1e-12).tolerance,
    ],
)
def test_approx_errors_and_corners_are_pytests(call):
    same(call)


@pytest.mark.parametrize(
    ("expected", "actual"),
    [
        ([1.0, 2.0, 3.0], [1.0, 2.5, 4.0]),
        ([1.0, 2.0], [1.0]),
        ([1.0, "a"], [2.0, "b"]),
        ({"a": 1.0, "b": 2.0}, {"a": 1.5, "b": 2.0}),
        ({"a": 0.0}, {"a": 1.0}),
        ({"a": 1.0}, {"b": 1.0}),
        ({"a": 1.0}, {"a": 1.0, "b": 2.0}),
        (1.0, 2.0),
        (datetime.timedelta(seconds=1), datetime.timedelta(seconds=3)),
    ],
)
def test_approx_explanations_are_pytests(expected, actual):
    def explain(p):
        kwargs = {"abs": datetime.timedelta(0)} if isinstance(expected, datetime.timedelta) else {}
        return p.approx(expected, **kwargs)._repr_compare(actual)

    same(explain)


class ArrayLike:
    """Duck-types an array; numpy isn't loaded, so it is a sequence."""

    def __init__(self, values):
        self.values = values

    def __array__(self):
        raise AssertionError("never called without numpy")

    def __getitem__(self, i):
        return self.values[i]

    def __len__(self):
        return len(self.values)

    def __iter__(self):
        return iter(self.values)


def test_approx_without_numpy_treats_array_likes_as_sequences():
    assert "numpy" not in sys.modules
    same(lambda p: [1.0, 2.0] == p.approx(ArrayLike([1.0, 2.0000001])))
    same(lambda p: repr(p.approx(ArrayLike([1.0, 2.0]))))


NUMPY = r"""
import importlib.util, json, sys
import numpy as np
import pytest

spec = importlib.util.spec_from_file_location("aye_pytest_shim", sys.argv[1])
shim = importlib.util.module_from_spec(spec)
sys.modules["aye_pytest_shim"] = shim
spec.loader.exec_module(shim)


def outcome(call):
    try:
        return ["ok", call()]
    except BaseException as exc:
        return ["raised", type(exc).__name__, str(exc)]


cases = [
    lambda p: repr(p.approx(np.array([1.0, 2.0]))),
    lambda p: repr(p.approx(np.array([[1.0, 2.0], [3.0, 4.0]]))),
    lambda p: bool(np.array([1.0, 2.0000001]) == p.approx(np.array([1.0, 2.0]))),
    lambda p: bool(np.array([1.0, 2.1]) == p.approx(np.array([1.0, 2.0]))),
    lambda p: bool(np.array([1.0]) == p.approx(np.array([1.0, 2.0]))),
    lambda p: bool(np.array([0.3, 0.3]) == p.approx(0.3)),
    lambda p: bool(np.float64(0.3) == p.approx(0.3)),
    lambda p: bool([1.0, 2.0] == p.approx(np.array([1.0, 2.0]))),
    lambda p: bool(1.0 == p.approx(np.array([1.0, 1.0]))),
    lambda p: bool(np.bool_(True) == p.approx(True)),
    lambda p: bool(1 == p.approx(np.bool_(True))),
    lambda p: p.approx(np.array([1.0, 2.0, 3.0]))._repr_compare(np.array([1.0, 2.5, 3.0])),
    lambda p: p.approx(np.array([1.0, 2.0]))._repr_compare(np.array([1.0])),
    lambda p: bool(np.array([1.0, 2.0]) == p.approx([1.0, 2.0])),
]
print(json.dumps([[outcome(lambda: c(shim)), outcome(lambda: c(pytest))] for c in cases]))
"""


def test_approx_with_numpy_is_pytests():
    if importlib.util.find_spec("numpy") is None:
        pytest.skip("numpy isn't installed")
    done = subprocess.run(
        [sys.executable, "-c", NUMPY, str(SHIM_PATH)],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    for ours, theirs in json.loads(done.stdout):
        assert ours == theirs


# ------------------------------------------------------------------ monkeypatch


class Target:
    @staticmethod
    def static():
        return "static"


def patch_everything(p, tmp_path, monkeypatch_of_the_runner):
    """A sequence of patches on fresh objects, then undo: the states
    observed along the way."""
    module = type(sys)("shim_target_module")
    module.attr = "original"
    monkeypatch_of_the_runner.setitem(sys.modules, "shim_target_module", module)
    target = type("Target", (), {"value": "class value", "static": staticmethod(Target.static)})
    instance = target()
    mapping = {"keep": 1, "gone": 2}
    env = "AYE_SHIM_TEST_VAR"
    os.environ.pop(env, None)
    os.environ[env + "_SET"] = "old"
    seen = []
    cwd = os.getcwd()
    path = list(sys.path)
    patch = p.MonkeyPatch()
    patch.setattr(target, "static", lambda: "patched")
    patch.setattr(target, "static", lambda: "patched again")
    patch.setattr(instance, "value", "instance value")
    patch.setattr(target, "new_attribute", 1, raising=False)
    patch.setattr("shim_target_module.attr", "via string")
    patch.delattr(target, "value")
    patch.delattr(target, "missing", raising=False)
    patch.delattr("shim_target_module.attr")
    patch.setitem(mapping, "keep", 10)
    patch.setitem(mapping, "keep", 100)
    patch.setitem(mapping, "new", 3)
    patch.delitem(mapping, "gone")
    patch.delitem(mapping, "missing", raising=False)
    patch.setenv(env, "1")
    patch.setenv(env + "_SET", "new", prepend=os.pathsep)
    patch.delenv(env + "_MISSING", raising=False)
    patch.syspath_prepend(str(tmp_path))
    patch.syspath_prepend(tmp_path / "second")
    patch.chdir(tmp_path)
    patch.chdir("/")
    seen.append(
        (
            target.static(),
            instance.value,
            target.new_attribute,
            hasattr(target, "value"),
            hasattr(module, "attr"),
            dict(mapping),
            os.environ[env],
            os.environ[env + "_SET"],
            sys.path[:2] == [str(tmp_path / "second"), str(tmp_path)],
            os.getcwd() == os.path.realpath("/"),
        )
    )
    patch.undo()
    seen.append(
        (
            target.static(),
            type(target.__dict__.get("static")).__name__,
            "static" in target.__dict__,
            "value" in vars(instance),
            hasattr(target, "new_attribute"),
            target.value,
            module.attr,
            dict(mapping),
            os.environ.get(env),
            os.environ[env + "_SET"],
            sys.path == path,
            os.getcwd() == cwd,
        )
    )
    patch.undo()  # a second undo changes nothing
    seen.append((module.attr, dict(mapping), os.getcwd() == cwd))
    del os.environ[env + "_SET"]
    return seen


def test_monkeypatch_undo_is_pytests(tmp_path, monkeypatch):
    (tmp_path / "second").mkdir()
    ours = patch_everything(shim, tmp_path, monkeypatch)
    theirs = patch_everything(pytest, tmp_path, monkeypatch)
    assert ours == theirs
    # Undo restored the class's own staticmethod, not the function behind it.
    assert ours[1][:3] == ("static", "staticmethod", True)


def test_monkeypatch_context():
    module = json
    original = module.dumps
    with shim.MonkeyPatch.context() as patch:
        patch.setattr(module, "dumps", len)
        assert module.dumps is len
    assert module.dumps is original


@pytest.mark.parametrize(
    "call",
    [
        lambda m: m.setattr("os.shim_nonexistent_attr", 1),
        lambda m: m.setattr(Target(), "nope", 1),
        lambda m: m.setattr("shim_nonexistent_mod_xyz.attr", 1),
        lambda m: m.setattr("os.path.shim_missing", 1),
        lambda m: m.setattr("nodots", 1),
        lambda m: m.setattr(object(), 1),
        lambda m: m.setattr(object(), 3, 1),
        lambda m: m.delattr(object(), "nope"),
        lambda m: m.delattr(object()),
        lambda m: m.delattr("os.shim_missing_attr"),
        lambda m: m.delitem({}, "nope"),
        lambda m: m.delenv("AYE_SHIM_SURELY_UNSET"),
        lambda m: m.setattr("json.shim_missing.deeper", 1),
    ],
)
def test_monkeypatch_errors_are_pytests(call):
    def run(p):
        patch = p.MonkeyPatch()
        try:
            return call(patch)
        finally:
            patch.undo()

    same(run)


def test_monkeypatch_setenv_converts_with_pytests_warning(monkeypatch):
    def run(p):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            patch = p.MonkeyPatch()
            patch.setenv("AYE_SHIM_INT", 3)
            value = os.environ["AYE_SHIM_INT"]
            patch.undo()
        return value, [(w.category.__name__, str(w.message)) for w in caught]

    same(run)
    assert "AYE_SHIM_INT" not in os.environ


def test_monkeypatch_importing_a_broken_submodule_is_pytests(tmp_path, monkeypatch):
    package = tmp_path / "shim_pkg"
    package.mkdir()
    (package / "__init__.py").write_text("")
    (package / "broken.py").write_text("import shim_not_there_either\n")
    monkeypatch.syspath_prepend(str(tmp_path))

    def run(p):
        sys.modules.pop("shim_pkg.broken", None)
        patch = p.MonkeyPatch()
        try:
            patch.setattr("shim_pkg.broken.attr", 1)
        finally:
            patch.undo()

    same(run)


# ------------------------------------------------------------------ types


def test_types_are_abstract_bases_the_engine_registers_with():
    class Request:
        pass

    shim.FixtureRequest.register(Request)
    assert isinstance(Request(), shim.FixtureRequest)
    assert not isinstance(Request(), shim.SubRequest)

    class Capture:
        pass

    shim.CaptureFixture.register(Capture)
    assert isinstance(Capture(), shim.CaptureFixture)
    for name in ("LogCaptureFixture", "TempPathFactory", "Config", "Metafunc", "Parser"):
        assert hasattr(getattr(shim, name), "register"), name


def test_types_take_subscripts_used_in_annotations():
    assert shim.CaptureFixture[str] is not None
    assert shim.CaptureFixture[bytes] is not None
    assert shim.ExceptionInfo[ValueError] is not None
    assert shim.CallInfo[None] is not None
    assert shim.RaisesExc[ValueError] is not None


def test_node_types_are_pytests_hierarchy():
    pairs = [
        ("Function", "Item"),
        ("Item", "Node"),
        ("Module", "File"),
        ("File", "Collector"),
        ("Class", "Collector"),
        ("Package", "Directory"),
        ("Dir", "Directory"),
        ("Directory", "Collector"),
        ("Session", "Collector"),
        ("DoctestItem", "Item"),
    ]
    real = {"Node": _pytest.nodes.Node}
    for sub, base in pairs:
        assert issubclass(getattr(shim, sub), getattr(shim, base)), (sub, base)
        assert issubclass(getattr(pytest, sub), getattr(pytest, base, real.get(base)))
    assert issubclass(shim.Session.Interrupted, KeyboardInterrupt)
    assert issubclass(shim.Collector.CollectError, Exception)


def test_the_front_end_can_subclass_the_types():
    class Item(shim.Function):
        def __init__(self, name):
            self.name = name

    item = Item("test_x")
    assert isinstance(item, shim.Item)
    assert isinstance(item, shim.Node)
    assert not isinstance(item, shim.DoctestItem)
    assert item.nextitem is None

    class Config(shim.Config):
        def getoption(self, name):
            return name

    assert Config().getoption("x") == "x"


def test_stash_is_pytests():
    def use(p):
        key = p.StashKey[int]()
        other = p.StashKey[str]()
        stash = p.Stash()
        stash[key] = 1
        out = [stash[key], stash.get(other, "d"), key in stash, other in stash, len(stash)]
        out.append(stash.setdefault(other, "s"))
        out.append(stash.setdefault(other, "t"))
        del stash[key]
        out.append(len(stash))
        return out

    same(use)
    with pytest.raises(KeyError):
        shim.Stash()[shim.StashKey()]


# ------------------------------------------------------------------ hooks


def test_hookimpl_marks_the_function():
    def bare():
        pass

    assert shim.hookimpl(bare) is bare
    assert bare._aye_hookimpl == {}

    @shim.hookimpl(tryfirst=True, wrapper=True)
    def called():
        pass

    assert called._aye_hookimpl == {"tryfirst": True, "wrapper": True}
    with pytest.raises(TypeError, match="unexpected keyword arguments: first"):
        shim.hookimpl(first=True)

    @shim.hookspec(firstresult=True)
    def spec():
        pass

    assert spec._aye_hookspec == {"firstresult": True}
    assert not hasattr(spec, "_aye_hookimpl")


def test_register_assert_rewrite():
    module = load("aye_pytest_shim_rewrite")
    module.register_assert_rewrite("tests.helpers", "other")
    module.register_assert_rewrite()
    assert module._aye_rewrite_modules == ["tests.helpers", "other"]
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        same(lambda p: p.register_assert_rewrite("ok", 3))


# ------------------------------------------------------------------ installed as pytest

INSTALLED = r"""
import json, sys, types

def install(name, path):
    module = types.ModuleType(name)
    sys.modules[name] = module
    with open(path, encoding="utf-8") as file:
        exec(compile(file.read(), f"<aye:{name}>", "exec"), module.__dict__)
    return module

install("aye_engine", sys.argv[2])
engine = types.ModuleType("aye_native_engine")
exec("class FixtureLookupError(LookupError):\n    pass", engine.__dict__)
sys.modules["aye_native_engine"] = engine
install("pytest", sys.argv[1])
report = {}
# As ``import pytest`` loads it: what adapters tell pytest's outcomes by.
outcomes = sys.modules.get("_pytest.outcomes")
report["outcomes loaded"] = outcomes is not None and outcomes.Skipped is sys.modules["pytest"].skip.Exception
import pytest
import _pytest.fixtures
try:
    raise engine.FixtureLookupError("fixture 'x' not found")
except pytest.FixtureLookupError:
    report["lookup error caught"] = _pytest.fixtures.FixtureLookupError is engine.FixtureLookupError

report["is_aye"] = getattr(pytest, "__aye__", False)
report["file"] = pytest.__file__
import aye_engine
try:
    pytest.skip("s")
except BaseException as exc:
    report["is_skip"] = aye_engine.is_skip(exc)

from _pytest.mark.structures import ParameterSet, MarkDecorator, get_unpacked_marks
from _pytest.mark import Mark
from _pytest.nodes import Item, Node
from _pytest.fixtures import SubRequest, FixtureRequest, getfixturemarker
from _pytest.config import Config, ExitCode
from _pytest.config.argparsing import Parser
from _pytest.outcomes import Skipped, OutcomeException
import _pytest.warning_types
report["aliases"] = [
    ParameterSet is pytest.ParameterSet,
    MarkDecorator is pytest.MarkDecorator,
    Mark is pytest.Mark,
    Item is pytest.Item,
    Node is pytest.Node,
    FixtureRequest is pytest.FixtureRequest,
    issubclass(SubRequest, pytest.FixtureRequest),
    Config is pytest.Config,
    Parser is pytest.Parser,
    Skipped is pytest.skip.Exception,
    _pytest.warning_types.PytestWarning is pytest.PytestWarning,
]

@pytest.mark.slow
def marked():
    pass
report["unpacked"] = [m.name for m in get_unpacked_marks(marked)]

def fixture_function():
    pass
report["fixture_marker"] = getfixturemarker(pytest.fixture(fixture_function)).scope

for name in ("_pytest.pytester", "_pytest._py.path", "py"):
    try:
        __import__(name)
    except ImportError as exc:
        report[name] = str(exc)
    else:
        report[name] = "imported"
try:
    from _pytest.fixtures import FixtureDef
except ImportError as exc:
    report["FixtureDef"] = type(exc).__name__
try:
    import _pytest.fixtures
    _pytest.fixtures.FixtureDef
except AttributeError as exc:
    report["FixtureDef attribute"] = str(exc)
from _pytest.assertion.rewrite import AssertionRewritingHook
try:
    AssertionRewritingHook(config=None)
except pytest.UsageError as exc:
    report["rewriting hook"] = str(exc)
report["installed twice"] = pytest._aye_install_private_modules()
again = install("pytest", sys.argv[1])
import _pytest.mark
report["reinstalled"] = [
    _pytest.mark.Mark is again.Mark,
    sum(type(f).__name__ == "_PrivateModules" for f in sys.meta_path),
]
print(json.dumps(report))
"""


def test_installed_as_pytest_it_shadows_pytest_and_serves_the_private_modules(tmp_path):
    done = subprocess.run(
        [sys.executable, "-c", INSTALLED, str(SHIM_PATH), str(ENGINE / "aye_engine.py")],
        capture_output=True,
        text=True,
        cwd=tmp_path,
        timeout=60,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    report = json.loads(done.stdout)
    assert report["is_aye"] is True
    assert report["outcomes loaded"] is True
    assert report["lookup error caught"] is True
    assert report["file"] == "<aye:pytest>/__init__.py"
    assert report["is_skip"] is True
    assert all(report["aliases"]), report["aliases"]
    assert report["unpacked"] == ["slow"]
    assert report["fixture_marker"] == "function"
    assert report["_pytest.pytester"] == (
        "No module named '_pytest.pytester': aye's pytest module doesn't provide "
        "_pytest.pytester yet (run the suite with pytest itself to use it)"
    )
    assert "_pytest._py" in report["_pytest._py.path"]
    # pytest's py.py shim imports _pytest._py: it fails, naming it.
    assert report["py"] == "imported" or "_pytest._py" in report["py"]
    assert report["FixtureDef"] == "ImportError"
    assert "doesn't provide _pytest.fixtures.FixtureDef yet" in report["FixtureDef attribute"]
    assert "AssertionRewritingHook" in report["rewriting hook"]
    assert report["installed twice"] is False
    # A second install serves the new objects, with one finder.
    assert report["reinstalled"] == [True, 1]


AFTER_REAL_PYTEST = r"""
import json, sys, types

import _pytest.outcomes

real = sys.modules["_pytest"]
for name, path in (("aye_engine", sys.argv[2]), ("pytest", sys.argv[1])):
    module = types.ModuleType(name)
    sys.modules[name] = module
    with open(path, encoding="utf-8") as file:
        exec(compile(file.read(), f"<aye:{name}>", "exec"), module.__dict__)
import pytest
print(json.dumps({
    "is_aye": getattr(pytest, "__aye__", False),
    "_pytest kept": sys.modules["_pytest"] is real,
    "outcomes kept": sys.modules["_pytest.outcomes"] is _pytest.outcomes,
    "finders": sum(type(f).__name__ == "_PrivateModules" for f in sys.meta_path),
    "installed again": pytest._aye_install_private_modules(),
}))
"""


def test_installed_as_pytest_after_a_real_pytest_it_serves_no_private_module(tmp_path):
    # A real ``_pytest`` imported first stays: aye's module installs no
    # alias over it (docs/reorganisation.md §7.2).
    done = subprocess.run(
        [sys.executable, "-c", AFTER_REAL_PYTEST, str(SHIM_PATH), str(ENGINE / "aye_engine.py")],
        capture_output=True,
        text=True,
        cwd=tmp_path,
        timeout=60,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    assert json.loads(done.stdout) == {
        "is_aye": True,
        "_pytest kept": True,
        "outcomes kept": True,
        "finders": 0,
        "installed again": False,
    }


DOCTEST_ADAPTER = HERE.parents[3] / "aye-engine-doctest" / "python" / "aye_engine_doctest.py"

FOUND = r"""
import importlib.metadata, importlib.util, json, sys, types
from pathlib import PurePath

def install(name, path):
    module = types.ModuleType(name)
    sys.modules[name] = module
    with open(path, encoding="utf-8") as file:
        exec(compile(file.read(), f"<aye:{name}>", "exec"), module.__dict__)
    return module

install("aye_engine", sys.argv[2])
install("pytest", sys.argv[1])
import pytest
distribution = importlib.metadata.distribution("pytest")
report = {
    "spec found": importlib.util.find_spec("pytest") is sys.modules["pytest"].__spec__,
    "spec": [pytest.__spec__.name, pytest.__spec__.origin],
    "version": importlib.metadata.version("pytest"),
    "normalized": importlib.metadata.version("PyTest"),
    "module version": pytest.__version__,
    "aye's distribution": type(distribution).__name__,
    "located": isinstance(distribution.locate_file("x"), PurePath),
    "other files": [distribution.read_text(f) for f in ("RECORD", "entry_points.txt")],
    "installed twice": pytest._aye_install_distribution(),
}
try:
    importlib.metadata.version("aye-no-such-distribution")
except importlib.metadata.PackageNotFoundError:
    report["others not found"] = True
again = install("pytest", sys.argv[1])
report["reinstalled"] = [
    importlib.util.find_spec("pytest") is again.__spec__,
    sum(type(f).__name__ == "_Distributions" for f in sys.meta_path),
    sum(d.metadata["Name"] == "pytest" for d in importlib.metadata.distributions()),
]
install("aye_engine_doctest", sys.argv[3])
import aye_engine_doctest
report["doctest adapter"] = aye_engine_doctest.pytest_version()
print(json.dumps(report))
"""


@pytest.mark.parametrize("site", [False, True], ids=["no pytest installed", "pytest installed"])
def test_installed_as_pytest_it_is_found_as_installed(tmp_path, site):
    # Without ``site`` (``-S``), as in an environment without pytest: the only
    # ``pytest`` distribution is aye's. With it, aye's comes first.
    done = subprocess.run(
        [
            sys.executable,
            *([] if site else ["-S"]),
            "-c",
            FOUND,
            str(SHIM_PATH),
            str(ENGINE / "aye_engine.py"),
            str(DOCTEST_ADAPTER),
        ],
        capture_output=True,
        text=True,
        cwd=tmp_path,
        timeout=60,
        check=False,
    )
    assert done.returncode == 0, done.stderr
    report = json.loads(done.stdout)
    assert report["spec found"] is True
    assert report["spec"] == ["pytest", "<aye:pytest>/__init__.py"]
    assert report["version"] == report["normalized"] == report["module version"] == "9.1.1"
    assert report["aye's distribution"] == "PytestDistribution"
    assert report["located"] is True
    assert report["other files"] == [None, None]
    assert report["installed twice"] is False
    assert report["others not found"] is True
    # Installed twice, one finder and one distribution of aye's (plus the
    # installed pytest's).
    assert report["reinstalled"] == [True, 1, 2 if site else 1]
    # The doctest adapter loads conftests when pytest is installed.
    assert report["doctest adapter"] == "9.1.1"


def test_loaded_under_another_name_it_installs_nothing():
    assert not any(type(f).__name__ == "_PrivateModules" for f in sys.meta_path)
    assert not any(type(f).__name__ == "_Distributions" for f in sys.meta_path)
    # Its spec is the one it was loaded with.
    assert shim.__spec__.name == "aye_pytest_shim"


def test_its_frames_are_internal_to_aye_engine():
    paths = aye_engine.Paths("/somewhere")
    assert paths.is_internal("<aye:pytest>")


@contextlib.contextmanager
def no_warnings():
    with warnings.catch_warnings():
        warnings.simplefilter("error")
        yield


def test_importing_it_warns_nothing():
    with no_warnings():
        load("aye_pytest_shim_quiet")


def test_its_source_imports_only_the_standard_library():
    source = SHIM_PATH.read_text(encoding="utf-8")
    imported = set(re.findall(r"^(?:from|import) ([\w.]+)", source, re.MULTILINE))
    roots = {name.split(".")[0] for name in imported}
    assert roots <= set(sys.stdlib_module_names) | {"__future__"}, roots
    assert "pytest" not in roots
    assert "_pytest" not in roots
