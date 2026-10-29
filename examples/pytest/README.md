# pytest example

A tiny shop (`shop/`: prices, coupons, stock and a cart) and its pytest
suite. Nothing to install: `pyproject.toml` puts this directory on the import
path, and aye finds a Python with pytest on its own.

## Run it

```console
$ aye                              # the whole suite (testpaths = ["tests"])
$ aye -n auto                      # one worker per CPU
$ aye -k coupon                    # tests whose names match
$ aye -k "checkout and not gift"
$ aye -m slow                      # only the tests marked slow
$ aye -m "not slow"                # all but those
$ aye tests/test_cart.py::TestCheckout
$ aye --co                         # list the test ids; run nothing
$ aye --output json -v             # JSON lines for coding agents
```

The same suite under pytest:

```console
$ python -m pytest
```

## A failure

`test_summary_after_coupon` is wrong on purpose. It's skipped unless
`AYE_EXAMPLE_FAIL=1` is set:

```console
$ AYE_EXAMPLE_FAIL=1 aye
aye · pytest · python 3.13.1 · 30 tests · 1 worker

✗ 42641  tests/test_cart.py::test_summary_after_coupon
         dict differs in 2 of 4 keys: ['discount'], ['total'] at tests/test_cart.py:88

skipped  gift cards aren't supported yet (1)
✗ 1 failed · ✓ 27 passed · 1 skipped · 1 expected failure   0.22 s
details  aye show 42641
```

The run says why in one line; the key beside it (`42641`, the start of a
hash of the test's id, the same from run to run) asks the last run for the
detail, without running anything:

```console
$ aye show 42641
✗ 42641  tests/test_cart.py::test_summary_after_coupon
  failed in the test · 0 ms · python 3.13.1 · run r1, just now · file unchanged

  tests/test_cart.py:88
     86 │ cart.add(catalog["coffee"])
     87 │ cart.apply_coupon("WELCOME10")
  →  88 │ assert cart.summary() == {
        │        ^^^^^^^^^^^^^^
  dict differs in 2 of 4 keys
    ['discount']   expected Decimal('1.04')   actual Decimal('1.05')
    ['total']      expected Decimal('9.45')   actual Decimal('9.44')

  setup    catalog (session) → monkeypatch (function) → no_tax (function) → inventory (function) → cart (function)
  rerun    aye tests/test_cart.py::test_summary_after_coupon
  more     aye show 42641 -vv   (every frame, output, locals)
```

Then rerun only what failed last time, see what a coding agent would, or
every failure in detail:

```console
$ AYE_EXAMPLE_FAIL=1 aye --lf
$ AYE_EXAMPLE_FAIL=1 aye --output json -v
$ aye show --failed
$ aye show 42641 --output json
```

## What's in it

| File | Shows |
|---|---|
| `tests/conftest.py` | Fixtures of each scope: `catalog` (session), `staff_coupon` (module, with teardown after `yield`), `inventory` and `cart` (function). An autouse fixture with `monkeypatch`. |
| `tests/test_cart.py` | A test class, `xfail`, `skip`, `skipif`, `capsys`, `monkeypatch.setenv`, `pytest.raises`, and the failing test's dict diff. |
| `tests/test_discounts.py` | `parametrize` with an id per case (`pytest.param(..., id=...)`). |
| `tests/test_inventory.py` | `parametrize` with `ids=`, `tmp_path`, and the `slow` mark. |
| `pyproject.toml` | `testpaths`, `pythonpath`, registered `markers` and `--strict-markers`. |
