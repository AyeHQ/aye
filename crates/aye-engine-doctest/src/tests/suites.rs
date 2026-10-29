//! Scratch suites checked against pytest 9.1.1 (`--doctest-modules`,
//! `--doctest-glob`). The expected ids are `pytest -p no:python
//! --collect-only -q` output, copied verbatim: its doctest items only, in
//! the order of aye's walk (without the python plugin, pytest doesn't put a
//! package's `__init__.py` first).

pub struct Suite {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
    /// What pytest 9.1.1 collects, by file in aye's walk order, in pytest's
    /// order within each file.
    pub pytest: &'static [&'static str],
    /// Files recognition sends for refinement.
    pub refined: &'static [&'static str],
}

pub const SUITES: &[Suite] = &[
    // Every kind of object the finder visits, names and order.
    Suite {
        name: "objects",
        files: &[
            (
                "pyproject.toml",
                "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n",
            ),
            (
                "pkg/__init__.py",
                r#""""The package.

>>> 1 + 1
2
"""
"#,
            ),
            (
                "pkg/Alpha.py",
                r#"def f():
    """
    >>> f()
    1
    """
    return 1
"#,
            ),
            (
                "pkg/shapes.py",
                r#""""Shapes.

>>> 2
2
"""
import abc
import enum
import dataclasses
from typing import final


class Color(enum.Enum):
    RED = 1

    def describe(self):
        """
        >>> Color.RED.describe()
        'red'
        """
        return "red"


class Shape(abc.ABC):
    """A shape.

    >>> Square(2).area()
    4
    """

    @property
    def sides(self):
        """The number of sides.

        >>> Square(1).sides
        4
        """
        return 4

    @sides.setter
    def sides(self, value):
        """
        >>> 'a setter docstring is never used'
        """

    @staticmethod
    def unit():
        """>>> Shape.unit()
        1
        """
        return 1

    @classmethod
    def make(cls):
        """
        >>> Shape.make() is Shape
        True
        """
        return cls

    @abc.abstractmethod
    def area(self):
        """
        >>> Square(3).area()
        9
        """

    class Inner:
        def method(self):
            """
            >>> 6
            6
            """

    def _private(self):
        """
        >>> 7
        7
        """

    def __init__(self):
        """
        >>> 8
        8
        """

    def no_examples(self):
        """Just prose, with >>> in the middle of a line."""


@final
class Square(Shape):
    def __init__(self, side=1):
        self.side = side

    def area(self):
        """
        >>> Square(2).area()
        4
        """
        return self.side**2


@dataclasses.dataclass
class Point:
    """
    >>> Point(1, 2)
    Point(x=1, y=2)
    """

    x: int = 0
    y: int = 0

    def norm(self):
        """
        >>> Point(3, 4).norm()
        5.0
        """
        return (self.x**2 + self.y**2) ** 0.5


async def fetch():
    """
    >>> import asyncio
    >>> asyncio.run(fetch())
    'data'
    """
    return "data"


def Zed():
    """
    >>> 1
    1
    """


def _under():
    """
    >>> 1
    1
    """


def comment_only():
    """
    >>> # just a comment
    """


if __name__ == "__main__":
    def never():
        """
        >>> 'never collected'
        """
"#,
            ),
            ("pkg/sub/__init__.py", ""),
            (
                "pkg/sub/test_both.py",
                r#"def test_a():
    pass


def helper():
    """
    >>> helper()
    9
    """
    return 9
"#,
            ),
            (
                "other/conftest.py",
                r#"def cf():
    """
    >>> cf()
    1
    """
    return 1
"#,
            ),
            ("other/test_doc.txt", "Text doctest:\n\n>>> 1 + 2\n3\n"),
            ("other/notes.txt", ">>> 'not a test file name'\n"),
            ("other/test_empty.txt", "No examples here.\n"),
            (
                "setup.py",
                "\"\"\"\n>>> 1\n1\n\"\"\"\nfrom setuptools import setup\n",
            ),
            ("__main__.py", "\"\"\"\n>>> 1\n1\n\"\"\"\n"),
        ],
        pytest: &[
            "other/conftest.py::conftest.cf",
            "other/test_doc.txt::test_doc.txt",
            "pkg/Alpha.py::pkg.Alpha.f",
            "pkg/__init__.py::pkg",
            "pkg/shapes.py::pkg.shapes",
            "pkg/shapes.py::pkg.shapes.Color.describe",
            "pkg/shapes.py::pkg.shapes.Point",
            "pkg/shapes.py::pkg.shapes.Point.norm",
            "pkg/shapes.py::pkg.shapes.Shape",
            "pkg/shapes.py::pkg.shapes.Shape.Inner.method",
            "pkg/shapes.py::pkg.shapes.Shape.__init__",
            "pkg/shapes.py::pkg.shapes.Shape._private",
            "pkg/shapes.py::pkg.shapes.Shape.area",
            "pkg/shapes.py::pkg.shapes.Shape.make",
            "pkg/shapes.py::pkg.shapes.Shape.sides",
            "pkg/shapes.py::pkg.shapes.Shape.unit",
            "pkg/shapes.py::pkg.shapes.Square.area",
            "pkg/shapes.py::pkg.shapes.Zed",
            "pkg/shapes.py::pkg.shapes._under",
            "pkg/shapes.py::pkg.shapes.fetch",
            "pkg/sub/test_both.py::pkg.sub.test_both.helper",
        ],
        refined: &[],
    },
    // Where files are, what is walked, how modules are named, globs with
    // and without separators, a custom `norecursedirs` replacing the
    // default, `--ignore`, virtualenvs, `setup.py` and `__main__.py`.
    Suite {
        name: "layout",
        files: &[
            (
                ".hidden/h.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("__main__.py", "\"\"\"\n>>> 1\n1\n\"\"\"\n"),
            (
                "build/b.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "custom_skip/c.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "dist/d.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("docs/guide.rst", "Docs\n\n>>> 1 + 1\n2\n"),
            ("docs/sub/deep.rst", ">>> 3\n3\n"),
            ("docs/sub/nested.md", ">>> 5\n5\n"),
            ("docs/top.md", ">>> 4\n4\n"),
            (
                "env/e.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("env/pyvenv.cfg", ""),
            (
                "ignored/i.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("my-dir/__init__.py", ""),
            (
                "my-dir/inner.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "node_modules/n.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("pkg_with.dot/__init__.py", ""),
            (
                "pkg_with.dot/m.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "pytest.ini",
                "[pytest]\naddopts = --doctest-modules --doctest-glob=*.rst --doctest-glob=docs/*.md --ignore=ignored\nnorecursedirs = .* build custom_skip\n",
            ),
            (
                "scripts/run.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("setup.py", "import setuptools\n\"\"\"\n>>> 1\n1\n\"\"\"\n"),
            (
                "src/app/__init__.py",
                "\"\"\"\n>>> 2\n2\n\"\"\"\nfrom .core.models import f\n__all__ = [\"f\"]\n",
            ),
            (
                "src/app/core/__init__.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "src/app/core/models.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "src/app/plugins/extra.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("test_default.txt", ">>> 6\n6\n"),
            (
                "tests/conftest.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "tests/unit/test_mod.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "venv/v.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "ünï/mod.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
        ],
        pytest: &[
            "dist/d.py::d.f",
            "docs/guide.rst::guide.rst",
            "docs/sub/deep.rst::deep.rst",
            "docs/sub/nested.md::nested.md",
            "docs/top.md::top.md",
            "my-dir/inner.py::inner.f",
            "node_modules/n.py::n.f",
            "pkg_with.dot/m.py::m.f",
            "scripts/run.py::run.f",
            "src/app/__init__.py::app",
            "src/app/core/__init__.py::app.core.f",
            "src/app/core/models.py::app.core.models.f",
            "src/app/plugins/extra.py::extra.f",
            "tests/conftest.py::conftest.f",
            "tests/unit/test_mod.py::test_mod.f",
            "venv/v.py::v.f",
            "ünï/mod.py::mod.f",
        ],
        refined: &[],
    },
    // Files only an import can settle: a wrapper, `__test__`, a decorator.
    Suite {
        name: "refined",
        files: &[
            (
                "m/alias.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n\n\ng = staticmethod(f)\n",
            ),
            (
                "m/cached.py",
                "import functools\n\n\n@functools.lru_cache(maxsize=None)\ndef f():\n    \"\"\"\n    >>> f()\n    \"\"\"\n",
            ),
            (
                "m/plain.py",
                "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            (
                "m/testdict.py",
                "__test__ = {\"extra\": \"\"\"\n>>> 2\n2\n\"\"\"}\n\n\ndef f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
            ),
            ("pytest.ini", "[pytest]\naddopts = --doctest-modules\n"),
        ],
        pytest: &[
            "m/alias.py::alias.f",
            "m/alias.py::alias.g",
            "m/cached.py::cached.f",
            "m/plain.py::plain.f",
            "m/testdict.py::testdict.__test__.extra",
            "m/testdict.py::testdict.f",
        ],
        refined: &["m/alias.py", "m/cached.py", "m/testdict.py"],
    },
];
