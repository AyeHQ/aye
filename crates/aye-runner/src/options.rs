//! What a [`Runner`](crate::Runner) is given: how to start workers
//! ([`RunnerOptions`], with each adapter's runtime found by its name in
//! [`Runtimes`]) and what to run ([`Plan`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

use aye_events::{AdapterName, Case, ProjectPath, TestId};
use aye_protocol::{AdapterContext, BatchContext, ModuleSet, Policy, Runtime};

use crate::worker::Echo;

/// What a [`Runner`](crate::Runner) needs to start workers.
#[derive(Debug, Clone, Default)]
pub struct RunnerOptions {
    /// The interpreter workers run.
    pub python: PathBuf,
    /// The project root, absolute.
    pub root: PathBuf,
    /// The directory workers start in: where aye was started.
    pub cwd: PathBuf,
    /// Workers to run at most (0 counts as 1); never more than there are
    /// batches.
    pub workers: u32,
    /// What every worker installs after aye's engine, whatever its
    /// runtimes ([`WorkerSpec::prelude`](crate::WorkerSpec::prelude)).
    pub prelude: ModuleSet,
    /// Every adapter's runtime face, one each (of several, the first
    /// counts). Workers get only those the cases at hand need.
    pub runtimes: Vec<Runtime>,
    /// Environment variables for every worker.
    pub env: Vec<(OsString, OsString)>,
    /// Where workers' output is echoed when the plan doesn't capture it
    /// (`-s`): stdout beside a text report, stderr beside a machine-readable
    /// one.
    pub echo: Echo,
    /// No run follows refinement (`--co`): no worker starts for the other
    /// lanes while one refines for an executor that lists the session.
    pub collect_only: bool,
}

/// The runtimes of [`RunnerOptions::runtimes`], by adapter: what a
/// worker of an adapter's batches installs and builds, and whether its
/// executor lists the session.
#[derive(Debug, Clone, Default)]
pub(crate) struct Runtimes(BTreeMap<AdapterName, Runtime>);

impl Runtimes {
    /// `runtimes` by adapter, an adapter's first if it has several.
    pub(crate) fn new(runtimes: &[Runtime]) -> Self {
        let mut by_adapter = BTreeMap::new();
        for runtime in runtimes {
            by_adapter
                .entry(runtime.adapter.clone())
                .or_insert_with(|| runtime.clone());
        }
        Self(by_adapter)
    }

    pub(crate) fn get(&self, adapter: &AdapterName) -> Option<&Runtime> {
        self.0.get(adapter)
    }

    /// Whether `adapter`'s executor lists the session.
    pub(crate) fn lists_session(&self, adapter: &AdapterName) -> bool {
        self.get(adapter)
            .is_some_and(|runtime| runtime.session.lists_session())
    }

    /// The adapters whose executors list the session.
    pub(crate) fn listing_session(&self) -> impl Iterator<Item = &AdapterName> {
        self.0
            .values()
            .filter(|runtime| runtime.session.lists_session())
            .map(|runtime| &runtime.adapter)
    }
}

/// What to run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// In the order they should run within a batch.
    pub cases: Vec<Case>,
    /// For every batch. `capture: false` also echoes workers' output live,
    /// where [`RunnerOptions::echo`] says; `maxfail` 0 is no limit.
    pub policy: Policy,
    /// Sent with each batch, per adapter (the recognizer's `context()`),
    /// with the keys below added ([`Plan::context`]).
    pub contexts: BTreeMap<AdapterName, AdapterContext>,
    /// Added to every batch's context: [`BatchContext::requested_ids`].
    pub requested_ids: Vec<TestId>,
    /// Added to every batch's context: [`BatchContext::rerun_args`].
    pub rerun_args: Vec<String>,
    /// Added to every batch's context: [`BatchContext::selected_paths`].
    pub selected_paths: Vec<String>,
    /// Historical durations, for balancing.
    pub durations: HashMap<TestId, Duration>,
    /// Tests to run first (`--ff`: last run's failures): the batches holding
    /// them are handed out first, and they run first within their batch.
    pub first: HashSet<TestId>,
    /// By adapter, every file collection claimed for its tests (selected or
    /// not), in collection order: what each worker of an adapter whose
    /// executor lists the session lists before its first batch.
    pub session_files: BTreeMap<AdapterName, Vec<ProjectPath>>,
}

impl Default for Plan {
    /// Nothing to run, capturing output.
    fn default() -> Self {
        Self {
            cases: Vec::new(),
            policy: Policy {
                capture: true,
                ..Policy::default()
            },
            contexts: BTreeMap::new(),
            requested_ids: Vec::new(),
            rerun_args: Vec::new(),
            selected_paths: Vec::new(),
            durations: HashMap::new(),
            first: HashSet::new(),
            session_files: BTreeMap::new(),
        }
    }
}

impl Plan {
    /// The context of `adapter`'s batches: its recognizer's, with the keys
    /// the binary adds (the runner adds [`BatchContext::ran_ids`] to a
    /// batch it re-queues), as a lane's listing before its first batch
    /// gets it too.
    pub fn context(&self, adapter: &AdapterName) -> BatchContext {
        BatchContext {
            adapter: self.contexts.get(adapter).cloned().unwrap_or_default(),
            requested_ids: self.requested_ids.clone(),
            rerun_args: self.rerun_args.clone(),
            selected_paths: self.selected_paths.clone(),
            ran_ids: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use aye_protocol::{EntryPoint, SessionMode};

    use super::*;

    fn runtime(adapter: &'static str, class: &'static str, session: SessionMode) -> Runtime {
        Runtime {
            adapter: AdapterName::new_static(adapter),
            modules: ModuleSet::default(),
            entry: EntryPoint::new("m", class),
            session,
        }
    }

    #[test]
    fn a_runtime_is_found_by_its_adapter() {
        let runtimes = Runtimes::new(&[
            runtime("a", "A", SessionMode::PerBatch),
            runtime("b", "B", SessionMode::ListsSession),
            runtime("a", "Again", SessionMode::ListsSession),
        ]);
        let a = AdapterName::new_static("a");
        let b = AdapterName::new_static("b");
        let entry = runtimes.get(&a).map(|runtime| runtime.entry.to_string());
        assert_eq!(entry.as_deref(), Some("m:A"), "an adapter's first");
        assert!(!runtimes.lists_session(&a) && runtimes.lists_session(&b));
        let none = AdapterName::new_static("c");
        assert!(runtimes.get(&none).is_none() && !runtimes.lists_session(&none));
        assert_eq!(runtimes.listing_session().collect::<Vec<_>>(), [&b]);
    }
}
