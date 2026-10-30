"""What the conftest's hooks are handed, for ``tests/contracts.rs``: each value
is recorded in ``observed.json`` under its name (``record``). The tests record
theirs through the ``record`` and ``hold`` fixtures."""

import json
import os
import re
import types

import pytest

OBSERVED = os.path.join(os.path.dirname(os.path.abspath(__file__)), "observed.json")


def record(name, value):
    try:
        with open(OBSERVED, encoding="utf-8") as file:
            data = json.load(file)
    except FileNotFoundError:
        data = {}
    data[name] = value
    with open(OBSERVED, "w", encoding="utf-8") as file:
        json.dump(data, file, indent=1, sort_keys=True)
        file.write("\n")


def kind(obj):
    """The class of ``obj`` as its default repr names it."""
    cls = type(obj)
    return f"{cls.__module__}.{cls.__qualname__}"


def shown(obj):
    """``repr(obj)``, its address blanked out."""
    return re.sub(r"0x[0-9a-fA-F]+", "0x…", repr(obj))


def hold(name, obj):
    """What user code holding ``obj`` sees of its class: the class's
    ``__module__`` and ``__qualname__``, and a default repr naming them; of
    a function, its own."""
    if isinstance(obj, types.FunctionType):
        record(f"function: {name}", f"{obj.__module__}.{obj.__qualname__}")
        return
    record(f"class: {name}", kind(obj))
    text = shown(obj)
    if " object at 0x…>" in text:
        record(f"repr: {name}", text)


@pytest.fixture(name="record")
def record_fixture():
    return record


@pytest.fixture(name="hold")
def hold_fixture():
    return hold


def pytest_addoption(parser, pluginmanager):
    hold("pytest_addoption parser", parser)
    hold("pytest_addoption pluginmanager", pluginmanager)
    group = parser.getgroup("contracts")
    hold("parser.getgroup()", group)
    group.addoption("--contracts-flag", action="store_true")


def pytest_configure(config):
    hold("pytest_configure config", config)
    hold("config.pluginmanager", config.pluginmanager)
    hold("config.hook", config.hook)
    hold("config.invocation_params", config.invocation_params)
    hold("config.stash", config.stash)
    hold("config.cache", config.cache)


def pytest_sessionstart(session):
    hold("pytest_sessionstart session", session)


def pytest_collection_modifyitems(session, config, items):
    for item in items:
        hold(f"item {item.nodeid.split('::', 1)[-1]}", item)
        parent = item.parent
        while parent is not None:
            hold(f"parent {parent.nodeid or '(session)'}", parent)
            parent = parent.parent


def pytest_generate_tests(metafunc):
    if "generated" in metafunc.fixturenames:
        hold("pytest_generate_tests metafunc", metafunc)
        metafunc.parametrize("generated", [1, 2])
