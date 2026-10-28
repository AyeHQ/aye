"""Refinement: importing files as discovery would and reporting their tests."""

import aye_engine_unittest as adapter


def case_ids(reply):
    return [case["id"] for case in reply["cases"]]


def test_cases_come_in_the_loaders_order(suite):
    suite.write(
        {
            "test_order.py": """
                import unittest

                class Zeta(unittest.TestCase):
                    def test_b(self):
                        pass

                    def test_a(self):
                        pass

                class Alpha(unittest.TestCase):
                    def test_z(self):
                        pass

                def load_tests(loader, tests, pattern):
                    return tests
            """
        }
    )
    reply = suite.refine(["test_order.py"])
    assert case_ids(reply) == [
        "test_order.py::Alpha::test_z",
        "test_order.py::Zeta::test_a",
        "test_order.py::Zeta::test_b",
    ]
    assert reply["diagnostics"] == []
    case = reply["cases"][1]
    assert case == {
        "id": "test_order.py::Zeta::test_a",
        "adapter": "unittest",
        "span": {"path": "test_order.py", "start": 0, "end": 0, "line": 7},
        "keywords": ["test_order.py", "Zeta", "test_a"],
        "affinity": ["test_order.py", "test_order.py::Zeta"],
        "certainty": "refined",
    }


def test_load_tests_makes_any_kind_of_test(suite):
    suite.write(
        {
            "pkg/__init__.py": "",
            "pkg/mathutil.py": '''
                def add(a, b):
                    """
                    >>> add(1, 2)
                    3
                    """
                    return a + b
            ''',
            "pkg/test_mixed.py": """
                import doctest
                import unittest

                from pkg import mathutil

                class Base(unittest.TestCase):
                    def test_one(self):
                        pass

                def check():
                    pass

                def load_tests(loader, tests, pattern):
                    tests.addTest(unittest.FunctionTestCase(check))
                    tests.addTests(doctest.DocTestSuite(mathutil))
                    tests.addTests(loader.loadTestsFromTestCase(Base))
                    return tests
            """,
        }
    )
    reply = suite.refine(["pkg/test_mixed.py"])
    assert case_ids(reply) == [
        "pkg/test_mixed.py::Base::test_one",
        "pkg/test_mixed.py::check",
        "pkg/test_mixed.py::pkg.mathutil.add",
        "pkg/test_mixed.py::Base::test_one",
    ]
    doctest_case = reply["cases"][2]
    assert doctest_case["span"] == {"path": "pkg/mathutil.py", "start": 0, "end": 0, "line": 2}
    assert doctest_case["affinity"] == ["pkg/test_mixed.py"]
    run = suite.run(case_ids(reply)[1:3])
    assert run.outcomes == {
        "pkg/test_mixed.py::check": "passed",
        "pkg/test_mixed.py::pkg.mathutil.add": "passed",
    }


def test_imported_classes_belong_to_the_module_that_loads_them(suite):
    suite.write(
        {
            "base.py": """
                import unittest

                class Shared(unittest.TestCase):
                    def test_shared(self):
                        pass
            """,
            "test_a.py": "from base import Shared\n",
            "test_b.py": "from base import Shared\n",
        }
    )
    reply = suite.refine(["test_a.py", "test_b.py"])
    assert case_ids(reply) == ["test_a.py::Shared::test_shared", "test_b.py::Shared::test_shared"]
    # Each runs where it was loaded, with the class's own module as its
    # module lifetime.
    run = suite.run(["test_b.py::Shared::test_shared"])
    assert run.outcomes == {"test_b.py::Shared::test_shared": "passed"}
    assert run.lifetimes() == ["+base.py", "+base.py::Shared", "-base.py::Shared", "-base.py"]


def test_a_package_that_may_define_load_tests_answers_for_its_files(suite):
    suite.write(
        {
            "pkg/__init__.py": """
                import os
                import unittest

                class InInit(unittest.TestCase):
                    def test_init(self):
                        pass

                def check():
                    pass

                def load_tests(loader, standard_tests, pattern):
                    here = os.path.dirname(__file__)
                    standard_tests.addTests(loader.discover(start_dir=here, pattern=pattern))
                    standard_tests.addTest(unittest.FunctionTestCase(check))
                    return standard_tests
            """,
            "pkg/test_a.py": """
                import unittest

                class A(unittest.TestCase):
                    def test_a(self):
                        pass
            """,
            "pkg/sub/__init__.py": "",
            "pkg/sub/test_b.py": """
                import unittest

                class B(unittest.TestCase):
                    def test_b(self):
                        pass
            """,
            "pkg/sub/test_empty.py": "x = 1\n",
        }
    )
    files = ["pkg/__init__.py", "pkg/test_a.py", "pkg/sub/test_b.py", "pkg/sub/test_empty.py"]
    reply = suite.refine(files)
    assert case_ids(reply) == [
        "pkg/__init__.py::InInit::test_init",
        "pkg/__init__.py::check",
        "pkg/test_a.py::A::test_a",
        "pkg/sub/test_b.py::B::test_b",
    ]
    assert all(case["affinity"][0] == "package:pkg" for case in reply["cases"])
    # A file alone gets only its own tests.
    assert case_ids(suite.refine(["pkg/sub/test_b.py"])) == ["pkg/sub/test_b.py::B::test_b"]
    run = suite.run(case_ids(reply))
    assert set(run.outcomes.values()) == {"passed"}
    assert len(run.outcomes) == 4


def test_the_load_tests_rule_matches_the_recognizers():
    for text in [
        b"def load_tests(l, t, p): pass",
        b"from .x import *",
        b"from .x import\\\n  *",
        b"def __getattr__(name): pass",
        b"globals()['x'] = 1",
        b"setattr(sys.modules[__name__], 'x', 1)",
        b"vars()",
        b"exec('x')",
    ]:
        assert adapter.may_define_load_tests(text), text
    for text in [b"", b"import os\n", b"executor = 1", b"my_load_tests = 1", b"envvars = 1"]:
        assert not adapter.may_define_load_tests(text), text


def test_what_cant_load_is_diagnosed(suite):
    suite.write(
        {
            "test_broken.py": "import nonexistent_module_xyz\n",
            "test_skipmod.py": """
                import unittest
                raise unittest.SkipTest("not here")
            """,
            ".hidden/__init__.py": "",
        }
    )
    reply = suite.refine(["test_broken.py", "test_skipmod.py", ".hidden/__init__.py", "nope.py"])
    assert reply["cases"] == []
    diagnostics = [(d["severity"], d["code"], d["path"]) for d in reply["diagnostics"]]
    assert diagnostics == [
        ("error", "collection-error", "test_broken.py"),
        ("info", "skipped", "test_skipmod.py"),
        ("error", "collection-error", ".hidden/__init__.py"),
        ("error", "not-found", "nope.py"),
    ]
    skipped = next(d for d in reply["diagnostics"] if d["code"] == "skipped")
    assert skipped["message"] == "not here"


def test_a_file_outside_the_start_imports_from_its_package_root(suite):
    suite.write(
        {
            "tests/test_in.py": """
                import unittest

                class In(unittest.TestCase):
                    def test_in(self):
                        pass
            """,
            "tools/pkg/__init__.py": "",
            "tools/pkg/check_tool.py": """
                import unittest

                class Tool(unittest.TestCase):
                    def test_tool(self):
                        pass
            """,
        }
    )
    context = suite.context(
        start_directory=str(suite.root / "tests"), top_level_directory=str(suite.root / "tests")
    )
    reply = suite.refine(["tools/pkg/check_tool.py"], context=context)
    assert case_ids(reply) == ["tools/pkg/check_tool.py::Tool::test_tool"]
    run = suite.run(["tools/pkg/check_tool.py::Tool::test_tool"], context=context)
    assert run.outcomes == {"tools/pkg/check_tool.py::Tool::test_tool": "passed"}


def test_named_directories_are_starts_of_their_own(suite):
    suite.write(
        {
            "tests/__init__.py": "",
            "examples/__init__.py": """
                def load_tests(loader, tests, pattern):
                    return tests
            """,
            "examples/test_ex.py": """
                import unittest

                class Ex(unittest.TestCase):
                    def test_ex(self):
                        pass
            """,
        }
    )
    context = suite.context(
        start_directory=str(suite.root / "tests"),
        top_level_directory=str(suite.root),
        starts=[str(suite.root / "examples")],
    )
    # From `examples`, discovery hands the package to its load_tests,
    # which keeps only the package's own tests: none.
    assert suite.refine(["examples/test_ex.py"], context=context)["cases"] == []
    plain = suite.context(
        start_directory=str(suite.root / "tests"), top_level_directory=str(suite.root)
    )
    assert case_ids(suite.refine(["examples/test_ex.py"], context=plain)) == [
        "examples/test_ex.py::Ex::test_ex"
    ]
