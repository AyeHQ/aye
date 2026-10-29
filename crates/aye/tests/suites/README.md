# Example suites

Each directory is a small project. The comparison harness
(`crates/aye/tests/compare.rs`) runs aye on a fresh copy of each and diffs it
with what the project's referee, the framework's own runner, recorded in the
suite's `expected.json`: the test ids and their order, the outcome per id and
the exit status, plus, from static collection alone (`--no-refine`), every
id aye predicts exactly. Several tests fail on purpose.

## Referees

`../harness/record.py` records every suite (`make expected` re-records them
all) and picks the referee from the suite's `[tool.aye] adapters`:

- **plain unittest** (`harness/expected_unittest.py`, discovery as the
  suite's `[tool.aye.unittest]` sets it) when unittest is the only adapter;
- **plain pytest** (`harness/expected.py`, with the suite's pytest
  configuration) otherwise. pytest runs its own tests, unittest's
  `TestCase` classes and, through its doctest plugin, doctests;
  `[tool.aye.doctest]` reaches it as the options it stands for (`modules =
  true` as `--doctest-modules`, `glob` as `--doctest-glob`).

Where unittest is one of several adapters, pytest referees the ids and, only
where they agree with plain unittest's, the outcomes: recording runs unittest
too and fails on any test whose outcome the two report differently.

The projects in the repository's `examples/` go through the same harness,
recorded in `../examples/<name>.json`: from the example's directory with no
paths, as its README runs it, once by default and once with
`AYE_EXAMPLE_FAIL=1`, which turns its deliberate failure on.

## Suites

- `outcomes`: every outcome, failures in setup and teardown, fixtures of
  every scope, captured output, structured assertions, pytest's subtests.
- `params`: parametrize ids pytest computes from values (escaping, floats,
  bytes, duplicates, `ids=`, `pytest.param`, stacked and class-level marks).
- `structure`: classes, nested classes, inheritance, unittest classes,
  duplicate definitions, conditional definitions, packages.
- `dynamic`: tests that only exist at runtime, which aye refines by importing,
  and a directory whose conftest calls `pytest.importorskip` for a module
  that isn't installed: pytest meets it while collecting, so it skips that
  directory alone.
- `rootdir`: pytest's rootdir differs from aye's project root.
- `unittest`: a unittest project for the unittest adapter (`[tool.aye]
  adapters = ["unittest"]`), refereed by plain unittest: every outcome, class
  and module fixtures (a failing `setUpClass` and `setUpModule`), `subTest`,
  skips, expected failures, an unexpected success, `load_tests`,
  inheritance across files and nested packages.
- `doctest`: the doctest adapter alone (`[tool.aye] adapters = ["doctest"]`),
  refereed by pytest with the `--doctest-modules` and `--doctest-glob`
  options of its `addopts`: docstrings of a package, modules, classes,
  methods, properties, static and class methods and nested classes, text,
  reStructuredText and Markdown files, option flags, failing examples, an
  unexpected exception, skipped examples, and a module only an import can
  settle.
- `pytest-doctest`: a pytest project with no `[tool.aye] adapters`, whose
  `addopts` turns `--doctest-modules` on and which has `test*.txt` files
  (pytest's default glob), so aye's default choice of adapters, doctest then
  pytest, runs it. It must give exactly pytest's ids, order and outcomes:
  docstrings of a package, a module, a class and its methods, a conftest and
  a test module (whose doctests pytest lists before its tests); passing,
  failing, skipped (`+SKIP`) and raising examples, and pytest's default
  `ELLIPSIS`; text files at the root and in a directory, and one the glob
  doesn't match; and pytest tests beside them (conftest fixtures,
  parametrize, skip, a strict xfail, a class); and `extras/`, whose
  conftest calls `pytest.importorskip` for a module that isn't installed,
  so pytest skips its doctests, text file and tests as one directory. The
  root's is the one conftest pytest collects: it imports every conftest
  outside a package as `conftest`, so with `--doctest-modules` a second one
  it collects is pytest's "import file mismatch" (the one in `extras/`
  skips before that).
- `unittest-pytest`: `[tool.aye] adapters = ["unittest", "pytest"]`, with
  `TestCase` classes and pytest functions in one file, a file of each kind,
  and a pytest fixture in a conftest. unittest's adapter runs the classes
  and pytest's the rest; both collect the classes, with the same ids, and
  the first listed wins. pytest is the referee, so the suite keeps to cases
  where pytest's outcomes are unittest's: passing, failing assertions (in a
  test and in `setUp`), `skipTest`, skip decorators on methods and classes,
  expected failures and an unexpected success (`failed` in both), a failing
  `setUpClass` (`error` in both), `setUpModule`, inheritance and `subTest`.
  It leaves out what they disagree on: any other exception in a test,
  `setUp` or `tearDown` (unittest's `error`, pytest's `failed`), and
  pytest's marks and autouse fixtures on `TestCase` methods (which only
  pytest applies). A mixed file defines its classes first, in name order:
  aye lists a file's tests adapter by adapter, and unittest orders classes
  by name, where pytest keeps the file's order.
