def test_generated(generated):
    assert generated.startswith("g")


def test_backend(backend):
    assert backend in ("sqlite", "postgres")
