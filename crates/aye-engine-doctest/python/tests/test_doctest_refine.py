"""Refinement: importing files and listing their doctests, as pytest does."""

import pytest

MODULE = '''
"""The module.

>>> 1 + 1
2
"""
import functools


class Shape:
    """
    >>> Shape().sides
    4
    """

    @property
    def sides(self):
        """
        >>> Shape().sides
        4
        """
        return 4

    @functools.cached_property
    def cached(self):
        """
        >>> Shape().cached
        1
        """
        return 1

    class Inner:
        def method(self):
            """
            >>> 6
            6
            """


def make():
    def inner():
        """
        >>> 1
        1
        """

    return inner


made = make()
alias = make


@functools.cache
def cached():
    """
    >>> cached()
    """


__test__ = {"extra": ">>> 2\\n2\\n"}
'''


def ids(reply):
    return [case["id"] for case in reply["cases"]]


def test_modules_give_what_pytest_collects_in_its_order(suite):
    suite.write(
        {
            "pytest.ini": "[pytest]\naddopts = --doctest-modules\n",
            "pkg/__init__.py": '"""\n>>> 1\n1\n"""\n',
            "pkg/shapes.py": MODULE,
            "loose/conftest.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "docs/test_guide.txt": "Guide.\n\n>>> 1 + 2\n3\n",
        }
    )
    files = ["pkg/__init__.py", "pkg/shapes.py", "loose/conftest.py", "docs/test_guide.txt"]
    reply = suite.refine(files)
    assert reply["diagnostics"] == []
    expected = suite.pytest_ids("-p", "no:python", *files)
    assert ids(reply) == expected
    assert "pkg/shapes.py::pkg.shapes.made" in expected
    assert "pkg/shapes.py::pkg.shapes.__test__.extra" in expected
    case = reply["cases"][1]
    assert case["adapter"] == "doctest"
    assert case["certainty"] == "refined"
    assert case["affinity"] == ["import:pkg.shapes", "pkg/shapes.py"]
    assert case["keywords"] == ["pkg", "shapes.py", case["id"].split("::")[1]]
    text = reply["cases"][-1]
    assert text["id"] == "docs/test_guide.txt::test_guide.txt"
    assert text["affinity"] == ["docs/test_guide.txt"]
    assert text["span"]["line"] == 1


def test_refinement_needs_no_pytest(suite):
    suite.write({"pkg/__init__.py": "", "pkg/shapes.py": MODULE})
    with_pytest = suite.refine(["pkg/shapes.py"])
    without = suite.refine(["pkg/shapes.py"], no_pytest=True)
    assert without["pytest_imported"] is False
    assert ids(without) == ids(with_pytest)


def test_what_pytest_fails_to_collect_is_a_collection_error(suite):
    suite.write(
        {
            "broken.py": "import nonexistent_module_xyz\n",
            "badparse.py": 'def f():\n    """\n    >>>1\n    """\n',
            "raises.py": "raise RuntimeError('at import')\n",
            "syntax.py": "def f(:\n",
            "fine.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "a/utils.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "b/utils.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "test_bad.txt": ">>>1\n",
        }
    )
    files = [
        "broken.py",
        "badparse.py",
        "raises.py",
        "syntax.py",
        "fine.py",
        "a/utils.py",
        "b/utils.py",
        "test_bad.txt",
        "missing.py",
    ]
    reply = suite.refine(files)
    assert ids(reply) == ["fine.py::fine.f", "a/utils.py::utils.f"]
    by_path = {d["path"]: d for d in reply["diagnostics"]}
    assert set(by_path) == {
        "broken.py",
        "badparse.py",
        "raises.py",
        "syntax.py",
        "b/utils.py",
        "test_bad.txt",
        "missing.py",
    }
    assert by_path["missing.py"]["code"] == "not-found"
    for path in set(by_path) - {"missing.py"}:
        assert by_path[path]["code"] == "collection-error"
        assert by_path[path]["severity"] == "error"
    assert "ImportError while importing test module" in by_path["broken.py"]["message"]
    assert "nonexistent_module_xyz" in by_path["broken.py"]["message"]
    assert "lacks blank after >>>" in by_path["badparse.py"]["message"]
    assert "RuntimeError: at import" in by_path["raises.py"]["message"]
    assert "import file mismatch" in by_path["b/utils.py"]["message"]
    # pytest agrees on which files fail.
    done = suite.pytest("-p", "no:python", "--doctest-modules", "--co", "-q", *files[:-1])
    for path in set(by_path) - {"missing.py"}:
        assert f"ERROR {path}" in done.stdout, path


def test_conftests_outside_packages_are_each_imported(suite):
    # They are all ``conftest``; pytest loads each afresh, so none is
    # another's import file mismatch.
    doc = 'def f():\n    """\n    >>> 1\n    1\n    """\n'
    suite.write(
        {
            "pytest.ini": "[pytest]\naddopts = --doctest-modules\n",
            "a/conftest.py": doc,
            "b/conftest.py": "X = 1\n",
            "c/conftest.py": doc,
        }
    )
    files = ["a/conftest.py", "b/conftest.py", "c/conftest.py"]
    reply = suite.refine(files)
    assert reply["diagnostics"] == []
    assert ids(reply) == ["a/conftest.py::conftest.f", "c/conftest.py::conftest.f"]
    assert ids(reply) == suite.pytest_ids("-p", "no:python")
    run = suite.run(ids(reply))
    assert run.outcomes == {
        "a/conftest.py::conftest.f": "passed",
        "c/conftest.py::conftest.f": "passed",
    }


def test_skipped_modules_and_ignored_import_errors(suite):
    suite.write(
        {
            "skipped.py": (
                "import unittest\nraise unittest.SkipTest('not here')\n"
                'def f():\n    """\n    >>> 1\n    1\n    """\n'
            ),
            "broken.py": "import nonexistent_module_xyz\n",
        }
    )
    reply = suite.refine(["skipped.py", "broken.py"], suite.context(ignore_import_errors=True))
    assert ids(reply) == []
    assert [(d["path"], d["severity"], d["code"]) for d in reply["diagnostics"]] == [
        ("skipped.py", "info", "skipped"),
        ("broken.py", "info", "skipped"),
    ]
    assert reply["diagnostics"][0]["message"] == "not here"
    assert "unable to import module" in reply["diagnostics"][1]["message"]


def test_pytest_skip_at_module_level(suite):
    suite.write(
        {
            "allowed.py": "import pytest\npytest.skip('later', allow_module_level=True)\n",
            "refused.py": "import pytest\npytest.skip('later')\n",
        }
    )
    reply = suite.refine(["allowed.py", "refused.py"])
    by_path = {d["path"]: d for d in reply["diagnostics"]}
    assert by_path["allowed.py"]["code"] == "skipped"
    assert by_path["refused.py"]["code"] == "collection-error"
    assert "allow_module_level=True" in by_path["refused.py"]["message"]


def test_text_files_use_the_configured_encoding(suite):
    path = suite.root / "test_latin.txt"
    path.write_bytes(">>> print('caf\xe9')\ncaf\xe9\n".encode("latin-1"))
    assert ids(suite.refine(["test_latin.txt"], suite.context(encoding="latin-1"))) == [
        "test_latin.txt::test_latin.txt"
    ]
    reply = suite.refine(["test_latin.txt"])
    assert reply["diagnostics"][0]["code"] == "collection-error"
    assert "UnicodeDecodeError" in reply["diagnostics"][0]["message"]


def test_the_import_mode(suite):
    suite.write(
        {
            "tests/helpers.py": 'def f():\n    """\n    >>> 1\n    1\n    """\n',
            "pytest.ini": "[pytest]\n",
        }
    )
    context = suite.context(import_mode="importlib")
    # pytest's own import names it from the rootdir.
    assert ids(suite.refine(["tests/helpers.py"], context)) == ["tests/helpers.py::tests.helpers.f"]
    assert suite.pytest_ids(
        "-p", "no:python", "--doctest-modules", "--import-mode=importlib", "tests/helpers.py"
    ) == ["tests/helpers.py::tests.helpers.f"]
    # Without pytest, the default mode.
    reply = suite.refine(["tests/helpers.py"], context, no_pytest=True)
    assert ids(reply) == ["tests/helpers.py::helpers.f"]
    assert ids(suite.refine(["tests/helpers.py"], suite.context(import_mode="append"))) == [
        "tests/helpers.py::helpers.f"
    ]


def test_pythonpath_is_put_first(suite):
    suite.write(
        {
            "lib/helper_lib.py": "VALUE = 3\n",
            "pkg/mod.py": (
                'import helper_lib\ndef f():\n    """\n    >>> helper_lib.VALUE\n    3\n    """\n'
            ),
        }
    )
    reply = suite.refine(["pkg/mod.py"], suite.context(pythonpath=[str(suite.root / "lib")]))
    assert reply["diagnostics"] == []
    assert ids(reply) == ["pkg/mod.py::mod.f"]


@pytest.mark.parametrize("no_pytest", [False, True])
def test_registered_flags_parse(suite, no_pytest):
    suite.write({"m.py": 'def f():\n    """\n    >>> 1.0  # doctest: +NUMBER\n    1\n    """\n'})
    assert ids(suite.refine(["m.py"], no_pytest=no_pytest)) == ["m.py::m.f"]
