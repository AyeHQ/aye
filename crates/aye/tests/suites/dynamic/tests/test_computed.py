import pytest

CASES = [n * n for n in range(3)]


@pytest.mark.parametrize("square", CASES)
def test_squares(square):
    assert square >= 0


@pytest.mark.parametrize("n", range(2), ids=lambda n: f"n{n}")
def test_callable_ids(n):
    pass


for _name in ("alpha", "beta"):
    exec(f"def test_{_name}():\n    pass\n")
