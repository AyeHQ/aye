import pytest


def test_only_pytest(ledger):
    assert ledger == {"opening": 10}


@pytest.mark.skip(reason="pytest's own skip")
def test_skipped():
    raise AssertionError


def test_raises():
    with pytest.raises(KeyError):
        {}["missing"]
