<!--
The PR title becomes the squash-merge commit, which release-please reads, so it
must be a Conventional Commit: `feat: …`, `fix: …`, `perf: …`, `docs: …`,
`ci: …`, or `feat!: …` for a breaking change.
-->

## What and why

<!-- What does this change, and what problem does it solve? -->

Closes #

## How it was tested

<!-- Commands run, new tests, and output before and after if it's user-visible. -->

## Checklist

- [ ] `make lint` and `make test` pass
- [ ] Behaviour matches pytest, or the difference is intended and documented in `docs/compatibility.md`
- [ ] Event schema changes are reflected in `docs/events.md` (and `SCHEMA_VERSION` is bumped if a field was removed or renamed)
- [ ] Renderers only consume events; nothing reaches into runner internals
