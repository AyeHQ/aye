"""What user code sees of aye's ``pytest`` module and of the objects aye hands
it, recorded for ``tests/contracts.rs`` (``docs/reorganisation.md`` §7.2).
The last four tests skip, fail or are expected to fail, and ``contracts.rs``
reads them from the stored log; every other test passes."""

import importlib.metadata
import importlib.util
import sys
import warnings

import pytest


def _text(call):
    """What ``call`` raised: its class and message."""
    try:
        call()
    except BaseException as exc:  # noqa: BLE001 - what it raised is the point
        return f"{type(exc).__module__}.{type(exc).__qualname__}: {exc}"
    return "nothing raised"


def _private(name):
    return lambda: importlib.import_module(name)


# The ``_pytest`` modules suites import from, which aye's module provides as
# aliases of its own objects.
PRIVATE_MODULES = [
    "_pytest",
    "_pytest._code",
    "_pytest._code.code",
    "_pytest.assertion",
    "_pytest.assertion.rewrite",
    "_pytest.cacheprovider",
    "_pytest.capture",
    "_pytest.compat",
    "_pytest.config",
    "_pytest.config.argparsing",
    "_pytest.config.exceptions",
    "_pytest.doctest",
    "_pytest.fixtures",
    "_pytest.logging",
    "_pytest.main",
    "_pytest.mark",
    "_pytest.mark.structures",
    "_pytest.monkeypatch",
    "_pytest.nodes",
    "_pytest.outcomes",
    "_pytest.python",
    "_pytest.python_api",
    "_pytest.raises",
    "_pytest.recwarn",
    "_pytest.reports",
    "_pytest.runner",
    "_pytest.stash",
    "_pytest.tmpdir",
    "_pytest.warning_types",
]

# The modules whose classes are aye's: those of the other names ``pytest``
# holds (``Path``, ``Any``) are the standard library's.
AYE_MODULES = ("pytest", "builtins")


def test_identity(record):
    record("pytest.__version__", pytest.__version__)
    record("pytest.version_tuple", list(pytest.version_tuple))
    record("pytest.__aye__", pytest.__aye__)
    record("pytest.__file__", pytest.__file__)
    record("pytest.__name__", pytest.__name__)
    record("pytest.__spec__", [pytest.__spec__.name, pytest.__spec__.origin])
    record("find_spec('pytest').origin", importlib.util.find_spec("pytest").origin)
    record("importlib.metadata.version('pytest')", importlib.metadata.version("pytest"))
    record("pytest.__all__", sorted(pytest.__all__))
    record("pytest.no_such_name", _text(lambda: pytest.no_such_name))
    for name in PRIVATE_MODULES:
        module = importlib.import_module(name)
        record(f"import {name}", module.__name__)
    record("import _pytest.no_such_module", _text(_private("_pytest.no_such_module")))
    record(
        "_pytest aliases are pytest's objects",
        importlib.import_module("_pytest.fixtures").FixtureRequest is pytest.FixtureRequest,
    )


def test_pytest_classes(record):
    """``__module__`` and ``__qualname__`` of every class ``pytest`` names,
    and of the exceptions its functions raise."""
    classes = {
        name: value
        for name, value in vars(pytest).items()
        if isinstance(value, type)
        and not name.startswith("_")
        and (
            name in pytest.__all__
            or value.__module__ in AYE_MODULES
            or value.__module__.startswith("aye_")
        )
    }
    classes["skip.Exception"] = pytest.skip.Exception
    classes["fail.Exception"] = pytest.fail.Exception
    classes["xfail.Exception"] = pytest.xfail.Exception
    classes["exit.Exception"] = pytest.exit.Exception
    classes["Session.Interrupted"] = pytest.Session.Interrupted
    classes["Session.Failed"] = pytest.Session.Failed
    for name, cls in sorted(classes.items()):
        record(f"pytest.{name}", f"{cls.__module__}.{cls.__qualname__}")


@pytest.mark.parametrize("name", ["capsys", "capfd", "capsysbinary", "capfdbinary"])
def test_capture_fixtures(hold, request, name):
    """One at a time: a test can't have two of them."""
    fixture = request.getfixturevalue(name)
    hold(name, fixture)
    hold(f"{name}.readouterr()", fixture.readouterr())


def test_builtin_fixtures(  # noqa: PLR0913 - every builtin fixture
    hold,
    request,
    tmp_path_factory,
    tmpdir_factory,
    tmpdir,
    caplog,
    monkeypatch,
    recwarn,
    pytestconfig,
    cache,
    record_property,
    record_xml_attribute,
    record_testsuite_property,
    subtests,
):
    held = {
        "request": request,
        "tmp_path_factory": tmp_path_factory,
        "tmpdir_factory": tmpdir_factory,
        "caplog": caplog,
        "caplog.handler": caplog.handler,
        "monkeypatch": monkeypatch,
        "recwarn": recwarn,
        "pytestconfig": pytestconfig,
        "cache": cache,
        "record_property": record_property,
        "record_xml_attribute": record_xml_attribute,
        "record_testsuite_property": record_testsuite_property,
        "subtests": subtests,
        "request.node": request.node,
        "request.node.keywords": request.node.keywords,
        "request.node.stash": request.node.stash,
        "request.config": request.config,
        "request.session": request.session,
    }
    for name, obj in held.items():
        hold(name, obj)
    hold("tmpdir", tmpdir)


def test_held_objects(hold):
    with pytest.raises(ValueError) as excinfo:
        raise ValueError("x")
    with warnings.catch_warnings():
        warnings.simplefilter("always")
        with pytest.warns(UserWarning) as warned:
            warnings.warn("w", UserWarning, stacklevel=1)
    held = {
        "pytest.raises(ValueError)": pytest.raises(ValueError),
        "excinfo": excinfo,
        "pytest.warns(UserWarning)": warned,
        "pytest.approx(1.0)": pytest.approx(1.0),
        "pytest.approx([1.0])": pytest.approx([1.0]),
        "pytest.approx({'a': 1.0})": pytest.approx({"a": 1.0}),
        "pytest.mark": pytest.mark,
        "pytest.mark.slow": pytest.mark.slow,
        "pytest.mark.slow.mark": pytest.mark.slow.mark,
        "pytest.param(1)": pytest.param(1),
        "pytest.MonkeyPatch()": pytest.MonkeyPatch(),
        "pytest.Stash()": pytest.Stash(),
        "pytest.StashKey()": pytest.StashKey(),
        "pytest.fail": pytest.fail,
        "pytest.skip": pytest.skip,
        "pytest.xfail": pytest.xfail,
        "pytest.exit": pytest.exit,
        "pytest.HIDDEN_PARAM": pytest.HIDDEN_PARAM,
    }
    for name, obj in held.items():
        hold(name, obj)


def test_parametrized(hold, request, generated):
    hold("request.node.callspec", request.node.callspec)


@pytest.fixture
def needs_request(request):
    return request


def test_subrequest(hold, needs_request):
    hold("a fixture's request", needs_request)


def test_qualified_names(record):
    """The classes whose qualified names reach ``detail``, or by which the
    engine tells an exception apart."""
    engine = sys.modules["aye_engine"]
    native = sys.modules["aye_native_engine"]
    for cls in (engine.Skip, native.Failed, native.XFailed, native.FixtureLookupError):
        record(f"class {cls.__name__}", f"{cls.__module__}.{cls.__qualname__}")
    record("aye_engine.SKIP_EXCEPTIONS", sorted(engine.SKIP_EXCEPTIONS))
    record(
        "pytest.FixtureLookupError is the engine's",
        pytest.FixtureLookupError is native.FixtureLookupError,
    )


def test_aye_skip_is_a_skip():
    raise sys.modules["aye_engine"].Skip("skipped by aye's own class")


def test_missing_fixture_through_request(request):
    request.getfixturevalue("no_such_fixture")


def test_fails():
    pytest.fail("boom")


@pytest.mark.xfail(run=False, reason="not run")
def test_xfail_not_run():
    raise AssertionError
