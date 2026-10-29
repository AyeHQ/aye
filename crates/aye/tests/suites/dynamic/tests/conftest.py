import pytest


def pytest_generate_tests(metafunc):
    if "generated" in metafunc.fixturenames:
        metafunc.parametrize("generated", ["g1", "g2"])


@pytest.fixture(params=["sqlite", "postgres"])
def backend(request):
    return request.param
