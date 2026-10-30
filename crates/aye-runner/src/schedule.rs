//! What runs together, and in what order.
//!
//! Cases are grouped by their outermost affinity key (the file, for the
//! unittest and doctest adapters; a costly module or class lifetime, for
//! the pytest adapter's front-end), so that a
//! lifetime is set up once per group rather than once per test. Groups are
//! weighted by historical durations and go into a queue longest-first; each
//! worker pulls the next batch when it finishes one, which balances like LPT
//! scheduling while adapting to durations that history got wrong.
//!
//! A case that declares no affinity is *free*: it shares no costly lifetime
//! (the native engine's tests without module or class setup). A file's free
//! cases go with its first lifetime group when it has one, so that they keep
//! their place among its other cases; otherwise they are grouped by file,
//! and with several workers a file heavier than an even share of the work is
//! split into chunks of about that weight, in order, so that a big file
//! spreads across workers.
//!
//! With several workers, groups lighter than an even share of the work
//! (a quarter of a worker's part) are merged, so that a batch's fixed cost
//! (a round trip, a module or class lifetime) is paid less often while
//! every worker still gets a few batches to balance with. With one worker
//! there is nothing to balance: each adapter's cases form one batch, in
//! plan order.
//!
//! The batches of an adapter whose executor lists the session (the native
//! engine's) keep plan order instead, as pytest-xdist's workers do: only
//! consecutive groups are merged, and the batches take their places in the
//! hand-out in plan order, so each worker runs its tests in plan order (a
//! test that changes what a later one reads, rich's tables changing a box
//! its box tests read, runs after it as under plain pytest).
//!
//! Tests to run first (`--ff`) run first within their batch, in plan order,
//! and the batches holding them are handed out before the others, as
//! pytest runs last run's failures before everything else.
//!
//! A worker runs batches of one adapter only (see `run.rs`), so with
//! several lanes a worker that asks for its next batch prefers one of its
//! own adapter's, after those holding tests to run first: switching
//! adapters costs a new worker. With one lane, batches go in plan order.
//! A batch handed out when nothing else of its adapter is left for its
//! lane, and nothing can be pinned to it, ends its worker's session
//! ([`Assigned::ends_session`]); [`Queue::closing`] makes one with no ids
//! for a worker whose last batch didn't.
//!
//! A batch may be pinned to the first lane ([`Queue::pin`]): the ids of the
//! files whose tests a lane lists otherwise than the first lane does (see
//! `run.rs`). While other lanes may still pin some, the first lane waits for
//! them rather than finishing.

//! The plan is in `plan.rs`, the weights it goes by in `weights.rs`, a
//! batch as planned and handed out in `batch.rs`, and the hand-out in
//! `queue.rs`.

mod batch;
mod plan;
mod queue;
mod weights;

pub(crate) use batch::Assigned;
pub(crate) use plan::plan;
pub(crate) use queue::Queue;
pub use weights::DEFAULT_WEIGHT;
