//! The run store: each run's whole log, kept under `.aye/runs` for `aye
//! show`, another consumer of the stream.
//!
//! Runs are numbered: `r1`, `r2`, … A run takes 1 + the highest number
//! the directory's runs (stored, being written, or left over) and `last`
//! have, and claims it by creating its log with `create_new`, trying the
//! next number while another run has it ([`RunWriter::claim`]): two runs
//! at once never share one. The binary claims the number before the
//! run's `run_started`, which says it (`run_number` on the wire). A run
//! with nothing to show deletes its log, and its number is free again.
//! Runs stored under the hex ids of aye before October 2026 count for no
//! number, and are listed and pruned like any other.
//!
//! Layout, in the directory it is given (`.aye/runs`):
//! - `r<N>.jsonl.gz`: the run's log, every record as `aye show --log`
//!   prints it, one per line, gzip-compressed (`miniz_oxide`'s fastest
//!   level). It is written as the records arrive: each is serialized where
//!   it arrives, and the serialized log is handed in chunks to a thread of
//!   its own, which compresses it, so the run never waits on compression.
//! - `r<N>.json`: what the store knows beside the log ([`RunMeta`]):
//!   when the run started and finished (the store's clock, as it saw
//!   `run_started` and the end of the stream), its exit code, counts and
//!   duration from `run_finished`, what its summary counts beside those
//!   (`extra`: modules skipped while collecting, failed lifetimes that kept
//!   no test from running, error diagnostics; the binary hands them over,
//!   as `aye_stream::Tally` counts them), and the digest of each file
//!   holding a test it collected (or that collection reported an error on,
//!   or skipped), read when `collection_finished` arrives, so that `aye
//!   show` can tell a file that changed since. Written when
//!   the log is complete, by writing a temporary file and renaming it
//!   over: a run is in the store once this file exists, so a log being
//!   written, or left by an aye that was killed, is never shown.
//! - `last`: the id of the run that finished last, replaced the same way
//!   once its summary is written. Two runs at once each write their own
//!   files; the one that renames `last` last is `last`, and listed first
//!   ([`RunStore::runs`]). When the run it names is gone (another aye's
//!   retention, a hand), the newest stored run stands for it.
//!
//! A run is kept when it has something to show: a test's result, a failed
//! lifetime, a failed worker, or an error diagnostic other than the
//! command line's (what it names that doesn't exist: `not-found` from
//! outside any batch, `unclaimed`). One that ran nothing else (a typo in a
//! path, a `-k` that matches nothing, a collection interrupted) leaves no
//! trace, so `last` stays the run before it, and it takes no place in
//! retention. A collect-only run is not stored either. When a run is
//! kept, the store keeps the `keep` runs that finished last (and `last`)
//! and deletes the rest (`[tool.aye] keep-runs`, 10 by default), and logs
//! left without a summary for a day. `keep` 0 writes nothing and deletes
//! every stored run. Nothing here may fail a run: the binary reports
//! errors as warnings.

mod id;
mod layout;
mod meta;
mod reader;
mod retention;
mod writer;

pub use id::StoredRunId;
pub use layout::{DIR, LAST};
pub use meta::{RunMeta, VERSION};
pub use reader::RunStore;
pub use writer::RunWriter;

#[cfg(test)]
mod tests;
