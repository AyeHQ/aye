# Working on aye

aye is a universal python test runner: a Rust control plane plus a Python execution plane, aye's engine,
which runs every test itself: pytest suites through aye's own `pytest` module and its pytest front-end (pytest is never imported),
unittest and doctest suites natively. Read `docs/architecture.md` before changing anything structural. Its "Core decisions" section is binding.

## Releasing

Releases are automated with release-please, so commit messages must follow [Conventional Commits](https://www.conventionalcommits.org): `feat: …`, `fix: …`, `feat!: …` for breaking changes. While aye is below 1.0, `feat` and `fix` bump the patch version and breaking changes bump the minor.

On every push to `main`, release-please updates a release PR. The PR bumps the version (the lines marked `x-release-please` in `Cargo.toml`, plus `Cargo.lock`) and `CHANGELOG.md`. Merging the PR tags `vX.Y.Z`, and `.github/workflows/release.yml` builds the wheels and runs `make publish-crates` and `make publish-pypi`. Every crate and the wheel share one version, so never bump versions by hand.

Credentials come only from the `CARGO_REGISTRY_TOKEN` and `UV_PUBLISH_TOKEN` environment variables, set from repository secrets in CI. To publish by hand: `make dist`, then `CARGO_REGISTRY_TOKEN=… UV_PUBLISH_TOKEN=… make publish`.
