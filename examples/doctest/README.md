# doctest example

A tiny shop whose examples are its tests. The docstrings of the package, its
modules, classes, functions and methods carry doctests, and
`docs/test_usage.txt` is a walkthrough that runs too.

`pyproject.toml` tells aye to use its doctest adapter, which runs the
examples natively, as doctest does, without pytest; and where to look: in
the modules' docstrings, and in text files named `test*.txt`, as
`pytest --doctest-modules` does:

```toml
[tool.aye]
adapters = ["doctest"]

[tool.aye.doctest]
modules = true
glob = ["test*.txt"]
```

## Run it

```console
$ aye                     # every docstring and text file
$ aye shop/cart.py        # one module's docstrings
$ aye --co                # list the doctests; run nothing
$ aye --output json -v    # JSON lines for coding agents
```

The same doctests under the standard library's runner, and under pytest:

```console
$ python -m doctest -v shop/*.py docs/*.txt
$ python -m pytest --doctest-modules
```

`python -m doctest` imports each file under its own name (`cart`, not
`shop.cart`), which is why the package's modules import each other by their
full names.

## A failure

doctest can't skip an example at run time, so `docs/test_failure.txt` passes
by default. With `AYE_EXAMPLE_FAIL=1` set, its last example gets a different
total than the one written under it:

```console
$ AYE_EXAMPLE_FAIL=1 aye
aye · doctest · python 3.13.1 · 15 tests · 1 worker

✗ fe2bc  docs/test_failure.txt::test_failure.txt › line 15: >>> cart.total()
         cart.total(): expected "Decimal('1.50')", actual "Decimal('2.00')" at docs/test_failure.txt:15

✗ 1 failed · ✓ 14 passed   0.13 s
details  aye show fe2bc
$ aye show fe2bc
✗ fe2bc  docs/test_failure.txt::test_failure.txt › line 15: >>> cart.total()
  failed in the test · 0 ms · python 3.13.1 · run r1, just now · file unchanged

  docs/test_failure.txt:15
  →  15 │ >>> cart.total()
  strings differ at line 1, column 10
    expected  "Decimal('1.50')"
    actual    "Decimal('2.00')"

  rerun    aye docs/test_failure.txt::test_failure.txt
  more     aye show fe2bc -vv   (every frame, output, locals)
```

Each example is reported on its own, by its line, with what was expected
and what came out. doctest's own runner says the same:

```console
$ AYE_EXAMPLE_FAIL=1 python -m doctest docs/test_failure.txt
**********************************************************************
File "docs/test_failure.txt", line 15, in test_failure.txt
Failed example:
    cart.total()
Expected:
    Decimal('1.50')
Got:
    Decimal('2.00')
**********************************************************************
1 item had failures:
   1 of   7 in test_failure.txt
***Test Failed*** 1 failure.
```

## What's in it

| File | Shows |
|---|---|
| `shop/__init__.py` | A package docstring that imports what it uses. |
| `shop/pricing.py` | A module docstring; a function's, with an expected exception; classes' and methods'. |
| `shop/cart.py` | Docstrings that use the module's globals; printed output; `+ELLIPSIS`. |
| `docs/test_usage.txt` | A text file: prose, with examples that build on each other. |
| `docs/test_failure.txt` | The example that fails when `AYE_EXAMPLE_FAIL=1` is set. |
