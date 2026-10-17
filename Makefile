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

test: sync ## Run all tests, including the comparison with each framework's runner
	cargo test --workspace
	uv run pytest -q tests

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
