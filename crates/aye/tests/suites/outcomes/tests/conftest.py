import pytest


@pytest.fixture(scope="session")
def session_value():
    return 41


@pytest.fixture(scope="module")
def module_value(session_value):
    return session_value + 1


@pytest.fixture
def engine():
    raise RuntimeError("database is down")


@pytest.fixture
def db(engine):
    return engine


@pytest.fixture
def broken_teardown():
    yield "ok"
    raise OSError("disk full during teardown")
