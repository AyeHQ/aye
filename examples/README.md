# Examples

Three small projects to try aye on, one per framework it runs. Each is the
same tiny shop (prices, coupons, stock and a cart), tested with:

| Example | Run with aye | Native equivalent |
|---|---|---|
| [`pytest/`](pytest) | `cd examples/pytest && aye` | `python -m pytest` |
| [`unittest/`](unittest) | `cd examples/unittest && aye` | `python -m unittest discover -s tests -t . -v` |
| [`doctest/`](doctest) | `cd examples/doctest && aye` | `python -m pytest --doctest-modules`, or `python -m doctest -v shop/*.py docs/*.txt` |

aye runs each through the adapter for its framework. The pytest example
needs no setting: without `[tool.aye] adapters`, a project that uses pytest
runs under pytest's adapter, with doctest's for the doctests pytest would
collect. The other two name theirs in `pyproject.toml`:
`adapters = ["unittest"]` and `adapters = ["doctest"]`. Those adapters are
native: they don't go through pytest, and their examples run on a Python
without it.

Every example passes under aye and under its native runner, with the same
tests and the same outcomes; aye's own test suite checks that on every
change (`crates/aye/tests/compare.rs`). Each README lists more commands to
try, and what the example's tests show.

Nothing needs installing beyond aye: run the commands from the example's
directory. `uv tool install aye` brings a Python with pytest, which the
pytest example needs; the unittest and doctest examples use only the
standard library.

## Seeing a failure

Each example has one test that's wrong on purpose, skipped unless
`AYE_EXAMPLE_FAIL=1` is set:

```console
$ cd examples/pytest
$ AYE_EXAMPLE_FAIL=1 aye
```

doctest can't skip an example at run time, so in the doctest example that
test passes until the variable is set.

## From a checkout of this repository

Build aye, set up the repository's Python, then point aye at it:

```console
$ uv sync && cargo build
$ cd examples/pytest
$ ../../target/debug/aye --python ../../.venv/bin/python
```
