# Cargo manages the Rust dependencies, uv the Python ones.
# Publishing reads credentials from the environment only:
#   CARGO_REGISTRY_TOKEN  crates.io
#   UV_PUBLISH_TOKEN      PyPI

.DEFAULT_GOAL := help
.PHONY: help sync hooks build test expected bench bench-suite lint dist publish publish-crates publish-pypi clean

help: ## Show this help
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*## "} {printf "  %-15s %s\n", $$1, $$2}'

sync: ## Create .venv with the Python dev dependencies (uv)
	uv sync

hooks: sync ## Install the git hooks (pre-commit and commit-msg)
	uv run pre-commit install

build: ## Build the aye binary
	cargo build

# Every crate's Python tests: the engine's, the runner's and each adapter's.
PYTHON_TESTS := $(wildcard crates/*/python/tests)

test: sync ## Run all tests, including the comparison with each framework's runner
	cargo test --workspace
	uv run pytest -q $(PYTHON_TESTS)

expected: sync ## Re-record what each suite's and example's own runner does on it
	set -e; for suite in crates/aye/tests/suites/*/; do \
		uv run python crates/aye/tests/harness/record.py "$$suite" "$${suite}expected.json"; \
	done
	set -e; for example in examples/*/; do \
		uv run python crates/aye/tests/harness/record.py --example "$$example" \
			"crates/aye/tests/examples/$$(basename "$$example").json"; \
	done

bench: ## Run the benchmarks
	cargo bench --workspace --benches

# A project's suite timed under pytest and under aye, with hyperfine
# (`brew install hyperfine` or `cargo install hyperfine`):
#   make bench-suite PROJECT=~/code/myproject [PY=python] [ARGS=tests/unit] [RUNS=5]
# PY defaults to the project's .venv. Both runners get the same interpreter
# and ARGS; pytest-xdist's `-n auto` is timed too when it is installed. The
# results are also written to target/bench-suite/.
PROJECT ?= examples/pytest
PROJECT_DIR = $(abspath $(patsubst ~/%,$(HOME)/%,$(PROJECT)))
PY ?= $(firstword $(wildcard $(PROJECT_DIR)/.venv/bin/python) python3)
RUNS ?= 5
HYPERFINE ?= hyperfine
AYE = $(CURDIR)/target/release/aye
PYTEST = $(PY) -m pytest -q -p no:cacheprovider
XDIST = $(shell "$(PY)" -c "import xdist" 2>/dev/null && echo yes)
BENCH_OUT = $(CURDIR)/target/bench-suite/$(notdir $(PROJECT_DIR))

bench-suite: ## Time a project's suite with pytest and with aye (PROJECT=dir)
	cargo build --release -p aye
	mkdir -p "$(BENCH_OUT)"
	@# Timings only compare when the outcomes do: each configuration's summary.
	@cd "$(PROJECT_DIR)" && \
		echo "pytest:         $$($(PYTEST) $(ARGS) 2>&1 | tail -1)" && \
		echo "aye:            $$($(AYE) --python $(PY) -q $(ARGS) 2>&1 | tail -1)" && \
		$(if $(XDIST),echo "pytest -n auto: $$($(PYTEST) -n auto $(ARGS) 2>&1 | tail -1)" &&) \
		echo "aye -n auto:    $$($(AYE) --python $(PY) -q -n auto $(ARGS) 2>&1 | tail -1)"
	cd "$(PROJECT_DIR)" && $(HYPERFINE) --warmup 1 --runs $(RUNS) --ignore-failure \
		--export-markdown "$(BENCH_OUT)/run.md" --export-json "$(BENCH_OUT)/run.json" \
		-n pytest "$(PYTEST) $(ARGS)" \
		-n aye "$(AYE) --python $(PY) -q $(ARGS)" \
		$(if $(XDIST),-n "pytest -n auto" "$(PYTEST) -n auto $(ARGS)") \
		-n "aye -n auto" "$(AYE) --python $(PY) -q -n auto $(ARGS)"
	cd "$(PROJECT_DIR)" && $(HYPERFINE) --warmup 1 --runs $(RUNS) \
		--export-markdown "$(BENCH_OUT)/collect.md" --export-json "$(BENCH_OUT)/collect.json" \
		--prepare true --prepare true --prepare "rm -rf .aye" \
		-n "pytest --co" "$(PYTEST) --co $(ARGS)" \
		-n "aye --co" "$(AYE) --python $(PY) --co $(ARGS)" \
		-n "aye --co, first run" "$(AYE) --python $(PY) --co $(ARGS)"

lint: sync ## Run every check and formatter in .pre-commit-config.yaml
	uv run pre-commit run --all-files

dist: ## Build the sdist and a wheel for this machine into dist/
	rm -rf dist
	uv build

publish: ## Publish the crates, then dist/ to PyPI
	$(MAKE) publish-crates
	$(MAKE) publish-pypi

publish-crates: ## Publish every crate to crates.io (needs CARGO_REGISTRY_TOKEN)
	@test -n "$$CARGO_REGISTRY_TOKEN" || { echo "CARGO_REGISTRY_TOKEN is not set" >&2; exit 1; }
	cargo publish --workspace --locked

publish-pypi: ## Upload dist/ to PyPI (needs UV_PUBLISH_TOKEN)
	@test -n "$$UV_PUBLISH_TOKEN" || { echo "UV_PUBLISH_TOKEN is not set" >&2; exit 1; }
	uv publish

clean: ## Remove build outputs
	cargo clean
	rm -rf dist
