//! Environments: how the runner obtains an interpreter with the project's
//! dependencies, for one Python version or for the whole matrix.
//!
//! aye never creates virtualenvs or installs packages itself. It asks a
//! `Provisioner` and keeps the result:
//!
//! `provision(python: VersionSpec, project, lockfile) -> Environment`
//!
//! where `Environment` is an interpreter path plus the facts needed to key
//! caches: interpreter build (version, implementation, free-threaded), the
//! lockfile hash, the project metadata hash, and the provisioner that made it.
//! The runner owns the request, the cache key and the lifecycle; the
//! provisioner owns venvs and installs.
//!
//! Decisions (October 2026):
//! - **uv, as a subprocess, is the first provisioner.** The user installs uv;
//!   aye invokes it and reads its output. One environment per Python version
//!   via `UV_PROJECT_ENVIRONMENT=<dir> uv sync --frozen --python <v>`, and
//!   `uv python find` / `uv python install` for interpreters, including
//!   free-threaded builds. Semantics come from uv itself, not reimplemented.
//! - **Bring-your-own exists from day one.** `[tool.aye] python = [...]` or
//!   explicit paths: the user has made the environments, aye only runs. This
//!   is also what conda, pixi, Poetry, Bazel and remote images are.
//! - **No uv installed is a clear error**, not a silent fallback: "install uv
//!   or set python paths". A pip + pylock floor is deliberately absent until a
//!   real user needs the matrix on a machine without uv.
//! - **uv's crates are not linked.** They are published (`uv-python`,
//!   `uv-installer`, `uv-resolver`, `uv-lock`, …, at 0.0.x) but the `uv sync`
//!   orchestration is not a library, the API moves every release, and the
//!   build cost is large. Revisit behind the same trait if hermetic builds or
//!   uv's ownership become a concern; nothing above the trait would change.
//! - **Environment is a dimension of a run, not of a test id.** Events carry
//!   an environment id; history keys on (test id, environment); the matrix is
//!   rows of one run with one report and one exit code.
//! - **Remote backends call the same contract inside the pod**: a
//!   user-supplied image is "bring your own"; sync-at-start runs the
//!   provisioner in the pod once per lockfile change; building and pushing
//!   images is not a local-tool concern.
//!
//! Open: where local environments live (`.aye/envs/<key>` in the project, or a
//! user cache); what "default" means for the matrix (on demand, CI only, or
//! always with warm environments); whether `requires-python` expands to a
//! matrix on its own.
//!
//! Owns: the `Provisioner` trait, `Environment`, the cache key, the uv
//! subprocess implementation, the bring-your-own implementation, and a fake
//! for tests. Does not own: scheduling, routing or backends (`aye-runner`),
//! nor the environment id type on the wire (`aye-events`).
//!
//! Depends on: aye-events. Must not depend on the runner or any engine.
