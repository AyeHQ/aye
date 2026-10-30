//! History as another consumer of the stream. Per-test duration statistics
//! (ewma, max, count, last run), outcomes and last-failed, flake rates, the
//! per-test coverage map, run metadata, and the queries built on them: run
//! diffs, tests-for, parallel floor. Same schema locally and in the cloud;
//! durations are advisory and separate from result caching, which has stricter
//! invalidation.
//!
//! Depends on: aye-events, aye-fs for the state directory, and aye-stream
//! for what the run store tells of a file that changed since a run
//! (`FileChange`).
//!
//! Native features (October 2026): the fingerprint that decides whether a
//! cached result holds carries its own version, so changing its inputs
//! invalidates caches instead of misreading them. Its inputs grow to include
//! the snapshot values a test compares against and the test's hermetic
//! verdict; a test that was not hermetic is never served from cache.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The directory is marked as a cache** when aye creates it, whichever
//!   writer comes first: history and the run store create theirs, as the
//!   refinement cache does, through `aye_fs::prepare_dir`, which says how.
//! - **Every run's log is kept** for `aye show` ([`store`]): the run store,
//!   one more consumer of the stream, writes each run's records (all of
//!   them, as `aye show --log` prints them) gzip-compressed to
//!   `<dir>/runs/r<N>.jsonl.gz` as they arrive (serialized where they
//!   arrive, compressed on a thread of its own), then a summary beside it
//!   (`r<N>.json`: when it ran, its exit code and counts, what its summary
//!   counts beside them, and the digest of each file holding a test it
//!   collected) and `<dir>/runs/last`, each written to a temporary file and
//!   renamed over. Runs are numbered, `r1`, `r2`, …, each claiming its
//!   number before its `run_started`, which says it. It keeps the runs that
//!   finished last (`[tool.aye] keep-runs`, 10 by default; 0 keeps none),
//!   never stores a collect-only run, nor one with nothing to show (no test
//!   ran, nothing failed but the command line's names), so `last` stays
//!   the run before it, and its number is free again.

pub mod history;
pub mod store;

pub use history::{FILE, History, TestStats, VERSION};
pub use store::{RunMeta, RunSelector, RunStore, RunWriter, SelectRunError, StoredRunId};
