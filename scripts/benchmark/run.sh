#!/usr/bin/env bash
# Times aye against each framework's own runner on a large real project's unit
# tests, then draws the README's chart (assets/benchmark/). `make bench-runners`.
#
# Each project is cloned at a pinned commit into target/bench-runners/ with
# its own .venv, which both runners use. Every pair runs the same tests with
# the same outcomes: each runner prints its summary first, to check before timing.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
WORK=${WORK:-$ROOT/target/bench-runners}
RESULTS=$WORK/results
AYE=$ROOT/target/release/aye
RUNS=${RUNS:-3}
OUT=${OUT:-$ROOT/assets/benchmark}

clone() { # name url commit
  if [ ! -d "$WORK/$1" ]; then
    git init -q "$WORK/$1"
    git -C "$WORK/$1" fetch -q --depth 1 "$2" "$3"
    git -C "$WORK/$1" checkout -q FETCH_HEAD
  fi
  [ -x "$WORK/$1/.venv/bin/python" ] || uv venv -q -p 3.13 "$WORK/$1/.venv"
}

summary() { "$@" 2>&1 | tail -1; }

hf() { # export-name, then hyperfine's -n NAME COMMAND pairs
  local name=$1
  shift
  hyperfine --warmup 1 --runs "$RUNS" --export-json "$RESULTS/$name.json" "$@"
}

cargo build --release -p aye --manifest-path "$ROOT/Cargo.toml"
mkdir -p "$RESULTS"

# pytest: networkx's whole suite, with pytest-xdist for pytest's -n auto.
clone networkx https://github.com/networkx/networkx 8ff5a974cd03813a676c7fa5fdf4e9b6ee710a7f
cd "$WORK/networkx"
PY=.venv/bin/python
uv pip install -q -p $PY -e . pytest pytest-xdist numpy scipy
echo "pytest: $(summary $PY -m pytest -q -p no:cacheprovider networkx)"
echo "aye:    $(summary "$AYE" --python $PY -q networkx)"
hf pytest \
  -n "pytest" "$PY -m pytest -q -p no:cacheprovider networkx" \
  -n "aye" "$AYE --python $PY -q networkx" \
  -n "pytest -n auto" "$PY -m pytest -q -p no:cacheprovider -n auto networkx" \
  -n "aye -n auto" "$AYE --python $PY -q -n auto networkx"

# unittest: sqlglot's unit tests, as its `make unit` runs them.
clone sqlglot https://github.com/tobymao/sqlglot 9ae1a6a7520a5171b612179509555d4a2977a434
cd "$WORK/sqlglot"
grep -q '^\[tool.aye\]' pyproject.toml || printf '\n[tool.aye]\nadapters = ["unittest"]\n' >> pyproject.toml
uv pip install -q -p $PY -e . duckdb numpy pandas python-dateutil pytz setuptools setuptools-scm
export SKIP_INTEGRATION=1
echo "unittest: $(summary $PY -m unittest)"
echo "aye:      $(summary "$AYE" --python $PY -q)"
hf unittest \
  -n "python -m unittest" "$PY -m unittest" \
  -n "aye" "$AYE --python $PY -q" \
  -n "aye -n auto" "$AYE --python $PY -q -n auto"

pytest_version=$("$WORK/networkx/$PY" -c 'import pytest; print(pytest.__version__)')
echo "hyperfine, mean wall time of $RUNS runs after 1 warmup · Python 3.13 · \
aye $(git -C "$ROOT" rev-parse --short HEAD) (release build) · pytest $pytest_version" \
  > "$RESULTS/footer.txt"
uv run -q --no-project --with matplotlib "$ROOT/scripts/benchmark/plot.py" "$RESULTS" "$OUT"
