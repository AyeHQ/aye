import logging
import sys

import pytest


def add(a, b):
    return a + b


def test_passes(module_value):
    assert module_value == 42


def test_fails_with_values():
    print("computing")
    assert add(2, 2) == 5


def test_fails_on_dicts():
    assert {"name": "aye", "speed": "fast"} == {"name": "aye", "speed": "slow"}


def test_raises_unexpectedly():
    raise ValueError("boom")


def test_logs_and_fails():
    logging.getLogger("app").warning("about to fail")
    sys.stderr.write("to stderr\n")
    assert False, "explicit message"


@pytest.mark.skip(reason="not today")
def test_skipped():
    pass


@pytest.mark.skipif(sys.version_info > (3,), reason="python 3")
def test_skipped_if():
    pass


def test_skips_inside():
    pytest.skip("decided at runtime")


@pytest.mark.xfail(reason="known bug")
def test_expected_failure():
    assert False


@pytest.mark.xfail(reason="fixed already")
def test_unexpected_pass():
    pass


@pytest.mark.xfail(reason="strict", strict=True)
def test_strict_unexpected_pass():
    pass


def test_setup_error(db):
    pass


def test_teardown_error_after_pass(broken_teardown):
    assert broken_teardown == "ok"


@pytest.mark.slow
@pytest.mark.db
def test_marked():
    pass


class TestGrouped:
    def test_method_passes(self):
        assert True

    def test_method_fails(self):
        assert [1, 2, 3] == [1, 2, 4]
