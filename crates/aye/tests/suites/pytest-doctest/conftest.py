"""Fixtures for the tests. pytest collects a conftest's doctests too.

>>> sorted({"b": 1, "a": 2})
['a', 'b']
"""

import pytest


@pytest.fixture
def numbers():
    return [1, 2, 3]


@pytest.fixture
def pair(numbers):
    return numbers[:2]
