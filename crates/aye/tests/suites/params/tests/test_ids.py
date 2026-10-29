import pytest


@pytest.mark.parametrize("value", [1, -2, 3.5, 1e16, 1e-05, 0.1, float("inf"), True, None, 2j])
def test_scalars(value):
    pass


@pytest.mark.parametrize("text", ["plain", "with space", "tab\there", "newline\n", "é", "ü日本", "", "a/b", "back\\slash", "[x]"])
def test_strings(text):
    pass


@pytest.mark.parametrize("data", [b"bytes", b"\x00\xff", b""])
def test_bytes(data):
    pass


@pytest.mark.parametrize("a,b", [(1, 2), (3, 4)])
def test_two_names(a, b):
    pass


@pytest.mark.parametrize(("a", "b"), [("x", None), ("y", 1.0)])
def test_tuple_names(a, b):
    pass


@pytest.mark.parametrize("x", [1, 1, 2, 2, 1])
def test_duplicates(x):
    pass


@pytest.mark.parametrize("x", ["a", "a", "a0"])
def test_duplicates_that_collide(x):
    pass


@pytest.mark.parametrize("x", [1, 2, 3], ids=["one", None, "three"])
def test_ids_list(x):
    pass


@pytest.mark.parametrize(
    "x",
    [pytest.param(1, id="first"), pytest.param(2, marks=pytest.mark.slow), 3],
)
def test_param_objects(x):
    pass


@pytest.mark.parametrize("x", [0, 1])
@pytest.mark.parametrize("y", [2, 3])
def test_stacked(x, y):
    pass


@pytest.mark.parametrize("x", [])
def test_empty(x):
    pass


@pytest.mark.parametrize("x", [[1, 2], {"k": 1}, (1,)])
def test_containers(x):
    pass


@pytest.mark.parametrize("x", ["a"])
class TestClassLevel:
    def test_one(self, x):
        pass

    @pytest.mark.parametrize("y", [1, 2])
    def test_two(self, x, y):
        pass
