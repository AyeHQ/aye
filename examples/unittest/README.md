# unittest example

The shop from [`../pytest`](../pytest), tested with the standard library's
unittest only: no pytest in the tests, and no dependencies at all.

`pyproject.toml` tells aye to use its unittest adapter, which runs the tests
natively, as unittest does, without pytest; and where discovery starts and
which directory the modules are imported from, as `-s` and `-t` tell
`python -m unittest discover`:

```toml
[tool.aye]
adapters = ["unittest"]

[tool.aye.unittest]
start-directory = "tests"
top-level-directory = "."
```

## Run it

```console
$ aye                                     # the whole suite
$ aye -n auto                             # one worker per CPU
$ aye -k checkout                         # tests whose names match
$ aye tests/test_cart.py::CheckoutTests   # one class
$ aye tests.test_cart.CheckoutTests       # the same, as unittest names it
$ aye --co                                # list the test ids; run nothing
$ aye --output json -v                    # JSON lines for coding agents
```

The same suite under unittest's own runner:

```console
$ python -m unittest discover -s tests -t . -v
```

`tests/` is a package and `-t .` makes this directory the top level, so
`shop` imports without being installed.

## A failure

`CartTests.test_summary_after_coupon` is wrong on purpose. It's skipped
unless `AYE_EXAMPLE_FAIL=1` is set:

```console
$ AYE_EXAMPLE_FAIL=1 aye
aye · unittest · python 3.13.1 · 24 tests · 1 worker

✗ 69f3e  tests/test_cart.py::CartTests::test_summary_after_coupon
         dict differs in 2 of 4 keys: ['discount'], ['total'] at tests/test_cart.py:105

skipped  gift cards aren't supported yet (1)
✗ 1 failed · ✓ 21 passed · 1 skipped · 1 expected failure   0.14 s
details  aye show 69f3e
$ aye show 69f3e
✗ 69f3e  tests/test_cart.py::CartTests::test_summary_after_coupon
  failed in the test · 0 ms · python 3.13.1 · run r1, just now · file unchanged

  tests/test_cart.py:105
     103 │ self.cart.add(CATALOG["coffee"])
     104 │ self.cart.apply_coupon("WELCOME10")
  →  105 │ self.assertEqual(
  dict differs in 2 of 4 keys
    ['discount']   left Decimal('1.05')   right Decimal('1.04')
    ['total']      left Decimal('9.44')   right Decimal('9.45')

  rerun    aye tests/test_cart.py::CartTests::test_summary_after_coupon
  more     aye show 69f3e -vv   (every frame, output, locals)
```

`aye show` gives the detail without running anything: which keys of the
dicts differ, and how. `assertEqual` says nothing of which argument is the
expected one, so the values are `left` and `right`. Then rerun only what
failed last time, or compare with unittest's runner:

```console
$ AYE_EXAMPLE_FAIL=1 aye --lf
$ AYE_EXAMPLE_FAIL=1 python -m unittest discover -s tests -t .
```

## What's in it

| File | Shows |
|---|---|
| `tests/test_cart.py` | `setUp` with `addCleanup`, `mock.patch.dict` (started and stopped, and as a class decorator), `expectedFailure`, `skip`, `skipIf`, `assertRaisesRegex`, `redirect_stdout`, and `IsolatedAsyncioTestCase` with `asyncSetUp`. |
| `tests/test_discounts.py` | `setUpModule` and `tearDownModule`, `subTest`. |
| `tests/test_inventory.py` | `setUp` and `tearDown`, `setUpClass` and `tearDownClass`, `assertRaises` as a context manager, `subTest`. |
