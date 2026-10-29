import pytest

pytestmark = pytest.mark.parametrize("flavour", ["vanilla", "chocolate"])


def test_flavours(flavour):
    pass
