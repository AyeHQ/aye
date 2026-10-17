# aye

An extremely fast, pytest-compatible test runner for Python, built for humans and agents. Written in Rust.

```console
$ uv add --dev aye        # or, globally: uv tool install aye
$ aye                     # instead of: pytest
```

aye is a drop-in replacement for `pytest`. Your tests, fixtures, `conftest.py`, markers and `parametrize` run unchanged, and aye reads your existing pytest config. The flags are the ones you already know (`-k -m -x -q -v -s --lf --ff --maxfail -n`), and the exit codes are pytest's. aye runs every test on its own engine: `import pytest` gets aye's own `pytest` module, so pytest itself is never imported, and need not be installed. It also runs unittest suites and doctests natively.

> **Status: pre-alpha.** The walking skeleton works end to end: Rust-side
> collection and selection, parallel workers running every test on aye's own
> engine (pytest suites included, without pytest), and text and JSON lines
> output. Third-party pytest plugins don't load yet: one a suite names is an
> error, and one pytest would load by itself is a warning naming it. Django
> suites, the warm daemon, live TUI, watch mode and the absorbed plugins are
> next. See [the roadmap](docs/architecture.md#roadmap) and
> [what aye's pytest module covers](docs/compatibility.md#ayes-pytest-module).

## Why

**Faster.** Test discovery and `-k`/`-m` selection happen in Rust by parsing test files with Ruff's parser, so nothing gets imported. Built-in parallelism (`-n auto`) balances files across workers using timings from earlier runs. Next up is a warm Python daemon, which lets repeat runs skip interpreter startup and re-imports.

**Clearer.** Each failure prints as soon as it happens, with the failing line, a structured `left`/`right` comparison, and that test's captured output underneath. When setup is what broke, aye shows the fixture chain (`fixtures: db → engine`).

**Token-efficient for agents.** `--output json -v` prints JSON lines: each failure once, with what it takes to fix it, between the number of tests collected and a summary. No banners, no colour, no passing tests, no pytest-internal frames. `-v` and `-q` work the same way for both formats: `--output json -q` is the summary alone, plain `--output json` a line per failure, and `-vvv` the full event stream.
