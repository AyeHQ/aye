//! The passes that rewrite records before anything consumes them
//! ([`Passes`], over one fold of the batches' adapters, [`BatchAdapters`]):
//! a failed test's rerun spelled from where aye runs ([`RerunFromHere`]),
//! and one error for what pytest reports once, a file's collection errors
//! from the adapters pytest collects in one module ([`OneModuleErrors`])
//! and a conftest that fails to load ([`OneConftestError`]); and, for every
//! record, how a bad outcome's test ended before this run
//! ([`PreviousOutcome`]), which the sink applies first, as it holds the
//! history.
//!
//! Decided (implementation, October 2026):
//! - **A file's collection errors count as pytest counts them.** pytest
//!   collects a file's `TestCase` classes and its own tests in one
//!   `Module`, so the unittest and pytest adapters' batches that both load
//!   a file failing to import are one collection error: the file shows as
//!   many as the adapter reporting the most for it, and another adapter's
//!   beyond those become infos saying so, kept in the stream
//!   ([`registry::AdapterTraits::one_module`]). The doctest adapter's are
//!   pytest's `DoctestModule`, another collector, and count apart. A
//!   conftest pytest fails to load is one error too, however many adapters
//!   load it (the doctest adapter imports the conftests pytest would): a
//!   `usage-error` or `collection-error` on a `conftest.py` that another
//!   adapter (or refinement) already reported with that code becomes an
//!   info saying so, with its own words when they differ. Files below a
//!   conftest that failed to load have no tests because of it, which its
//!   error says: collection's `no-tests` notes on them are left out.
//! - **Reruns name the test from where aye ran.** Ids are relative to the
//!   root and a path is read from the current directory first, so from a
//!   subdirectory holding a path of the same name (`tests/tests/test_a.py`
//!   under `tests/`) an id would name the other file: there a failed
//!   test's `reproduce` command names its file relative to the current
//!   directory (one is added for an adapter that sets none), so what aye
//!   prints reruns it. Elsewhere the command is the id.

use std::collections::HashMap;
use std::path::Path;

use aye_events::{
    AdapterName, BatchIndex, Diagnostic, DiagnosticCode, Event, Record, RerunCommand, Severity,
    TestId,
};
use aye_fs::{normalize_absolute, relative_below_root};
use aye_history::History;

use crate::registry;

/// The passes the runner's records go through before the sink, in this
/// order: the batches' adapters are folded, a failed test's rerun is
/// spelled from where aye runs, then a file's collection errors and a
/// conftest's are counted as pytest counts them. The diagnostics
/// collection reported, which the binary emits itself, go through the last
/// two first ([`Self::collected`]).
pub(crate) struct Passes<'a> {
    batches: BatchAdapters,
    rerun: RerunFromHere<'a>,
    one_module: OneModuleErrors,
    one_conftest: OneConftestError,
}

impl<'a> Passes<'a> {
    pub(crate) fn new(rerun: RerunFromHere<'a>) -> Self {
        Self {
            batches: BatchAdapters::default(),
            rerun,
            one_module: OneModuleErrors::default(),
            one_conftest: OneConftestError::default(),
        }
    }

    /// A diagnostic collection reported, before the binary emits it: a
    /// conftest's error that refinement reported already becomes an info,
    /// and a file's collection error counts as `refined_by`'s, the adapter
    /// whose refinement lists every file it collects, when it runs.
    pub(crate) fn collected(
        &mut self,
        diagnostic: &mut Diagnostic,
        refined_by: Option<&AdapterName>,
    ) {
        self.one_conftest.see(None, diagnostic);
        if let Some(adapter) = refined_by {
            self.one_module.refined(adapter, diagnostic);
        }
    }

    /// A record from the runner.
    pub(crate) fn apply(&mut self, record: &mut Record) {
        self.batches.see(record);
        self.rerun.apply(record);
        self.one_module.see(record, &self.batches);
        self.one_conftest.see_record(record, &self.batches);
    }
}

/// Each batch's adapter, from its `batch_started`: what the passes that
/// count by adapter read a batch's records by.
#[derive(Debug, Default)]
pub(crate) struct BatchAdapters(HashMap<BatchIndex, AdapterName>);

impl BatchAdapters {
    pub(crate) fn see(&mut self, record: &Record) {
        if let (Event::BatchStarted { adapter, .. }, Some(batch)) =
            (&record.event, record.envelope.batch)
        {
            self.0.insert(batch, adapter.clone());
        }
    }

    /// The adapter of `batch`, the batch a record came from.
    pub(crate) fn of(&self, batch: Option<BatchIndex>) -> Option<&AdapterName> {
        batch.and_then(|batch| self.0.get(&batch))
    }
}

/// Where aye runs, for the reruns it prints.
pub(crate) struct RerunFromHere<'a> {
    pub(crate) cwd: &'a Path,
    pub(crate) root: &'a Path,
    /// aye's options that shape the run, which a rerun repeats before the
    /// id ([`aye_protocol::BatchContext::rerun_args`]).
    pub(crate) rerun_args: &'a [String],
}

impl RerunFromHere<'_> {
    /// Make a failed test's rerun (`aye <id>`) rerun it from the current
    /// directory. Ids are relative to the root, and a path is taken from
    /// the current directory first, then from the root: from a
    /// subdirectory holding a path of the same name (`tests/tests/…`), the
    /// id would name another file. There the rerun names the test's file
    /// relative to the current directory; elsewhere it stays the id.
    pub(crate) fn apply(&self, record: &mut Record) {
        let Event::TestFinished(result) = &mut record.event else {
            return;
        };
        if result.reproduce.is_none() && !result.outcome.is_bad() {
            return;
        }
        let Some(local) = self.local_id(&result.id) else {
            return;
        };
        match &mut result.reproduce {
            Some(reproduce) => {
                if let Some(mut rerun) = RerunCommand::parse(&reproduce.command, &result.id) {
                    rerun.target = local;
                    reproduce.command = rerun.to_command();
                }
            }
            None if result.outcome.is_bad() => {
                let rerun = RerunCommand {
                    options: self.rerun_args.to_vec(),
                    target: local,
                    framework_args: Vec::new(),
                };
                result.reproduce = Some(aye_events::Reproduce {
                    command: rerun.to_command(),
                    ..aye_events::Reproduce::default()
                });
            }
            None => {}
        }
    }

    /// `id` spelled from the current directory, when spelled from the root
    /// it would name another file from here; see [`Self::apply`].
    fn local_id(&self, id: &TestId) -> Option<String> {
        if self.cwd == self.root {
            return None;
        }
        let path = id.path().as_str();
        let from_cwd = normalize_absolute(&self.cwd.join(path));
        let from_root = normalize_absolute(&self.root.join(path));
        if from_cwd == from_root || !from_cwd.exists() {
            return None;
        }
        // The names, from their `::` on, follow the file as they are.
        let mut local = relative_below_root(&from_root, self.cwd);
        local.push_str(&id.as_str()[path.len()..]);
        Some(local)
    }
}

/// A file's collection errors from the batches of adapters pytest collects
/// in one module ([`registry::AdapterTraits::one_module`]: the unittest
/// and pytest adapters, which may each have tests in the file), counted as
/// pytest, the referee, counts them: its `Module` fails once, however many
/// of these adapters load the file. So a file shows as many errors as the
/// adapter reporting the most for it (pytest's batch may report its doctest
/// collector's too); another adapter's beyond those becomes an info saying
/// so. It isn't dropped, so the log's `seq` keeps counting every record.
/// Collection's errors count too ([`Self::refined`]): aye's pytest
/// front-end lists every pytest file in refinement
/// ([`registry::AdapterTraits::refines_every_file`]), so a file that fails
/// to import there has its error from collection, not from a batch.
#[derive(Debug, Default)]
pub(crate) struct OneModuleErrors {
    /// By file, how many collection errors each adapter reported for it,
    /// in the order the adapters first did.
    reported: HashMap<String, Vec<(AdapterName, usize)>>,
}

impl OneModuleErrors {
    /// A collection error collection reported (refinement's), counted as
    /// `adapter`'s: refinement doesn't say whose it is, and with the pytest
    /// adapter in the run, it lists every pytest file (the doctest
    /// adapter's errors, which pytest counts apart, then count as its too,
    /// so another adapter's batch error beyond them still becomes an info).
    /// A conftest's is [`OneConftestError`]'s.
    pub(crate) fn refined(&mut self, adapter: &AdapterName, diagnostic: &Diagnostic) {
        if diagnostic.severity != Severity::Error
            || diagnostic.code != DiagnosticCode::CollectionError
            || diagnostic.path.is_nowhere()
            || registry::support_file(diagnostic.path.as_str()).is_some()
        {
            return;
        }
        let counts = self
            .reported
            .entry(diagnostic.path.to_string())
            .or_default();
        match counts.iter_mut().find(|(by, _)| by == adapter) {
            Some((_, count)) => *count += 1,
            None => counts.push((adapter.clone(), 1)),
        }
    }

    /// A record from a batch of the adapter `batches` say.
    pub(crate) fn see(&mut self, record: &mut Record, batches: &BatchAdapters) {
        let batch = record.envelope.batch;
        match &mut record.event {
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    && diagnostic.code == DiagnosticCode::CollectionError
                    && !diagnostic.path.is_nowhere() =>
            {
                let Some(adapter) = batches.of(batch) else {
                    return;
                };
                if !registry::traits(adapter).one_module {
                    return;
                }
                let counts = self
                    .reported
                    .entry(diagnostic.path.to_string())
                    .or_default();
                let mine = if let Some((_, count)) = counts.iter_mut().find(|(by, _)| by == adapter)
                {
                    *count += 1;
                    *count
                } else {
                    counts.push((adapter.clone(), 1));
                    1
                };
                let mut others = counts.iter().filter(|(by, _)| by != adapter);
                let Some((first, _)) = others.clone().next() else {
                    return;
                };
                if others.all(|(_, count)| *count < mine) {
                    return;
                }
                diagnostic.severity = Severity::Info;
                diagnostic.message = format!(
                    "one collection error with the {first} adapter's for this file, as \
                     pytest collects the file once; the {adapter} adapter reported: {}",
                    diagnostic.message
                );
            }
            _ => {}
        }
    }
}

/// A conftest that fails to load is one error in pytest, which loads it
/// once per session, however many adapters load it: the doctest adapter
/// imports the conftests pytest would and words their failures as the
/// pytest adapter does, but a frame or a word apart would make two errors
/// of one. So a `usage-error` or `collection-error` on a `conftest.py`
/// ([`registry::AdapterTraits::support_file`]) that another adapter
/// already reported with that code, in refinement (where who reported it
/// isn't known, and each adapter reports one once) or in a batch, becomes
/// an info saying so, with its own words when they differ, kept in the
/// stream as [`OneModuleErrors`] keeps its. One adapter's own repeats
/// (every pytest batch reports an initial conftest's failure) stay errors,
/// which renderers show once.
#[derive(Debug, Default)]
pub(crate) struct OneConftestError {
    /// By code and path, who reported the error first (an adapter, or
    /// `None` for refinement), and its message.
    first: HashMap<(DiagnosticCode, String), (Option<AdapterName>, String)>,
}

impl OneConftestError {
    /// A record from a batch of the adapter `batches` say.
    pub(crate) fn see_record(&mut self, record: &mut Record, batches: &BatchAdapters) {
        let batch = record.envelope.batch;
        if let Event::Diagnostic(diagnostic) = &mut record.event
            && let Some(adapter) = batches.of(batch)
        {
            self.see(Some(adapter), diagnostic);
        }
    }

    /// A diagnostic `adapter` reported, or refinement (`None`).
    pub(crate) fn see(&mut self, adapter: Option<&AdapterName>, diagnostic: &mut Diagnostic) {
        let kind = match diagnostic.code {
            DiagnosticCode::UsageError => "usage error",
            DiagnosticCode::CollectionError => "collection error",
            _ => return,
        };
        if diagnostic.severity != Severity::Error
            || registry::support_file(diagnostic.path.as_str()).is_none()
        {
            return;
        }
        let key = (diagnostic.code.clone(), diagnostic.path.to_string());
        let Some((first, message)) = self.first.get(&key) else {
            let first = (adapter.cloned(), diagnostic.message.clone());
            self.first.insert(key, first);
            return;
        };
        if first.is_some() && first.as_ref() == adapter {
            return;
        }
        let by = adapter.map_or_else(
            || "another adapter".to_string(),
            |adapter| format!("the {adapter} adapter"),
        );
        let words = if *message == diagnostic.message {
            String::new()
        } else {
            format!(", in its words: {}", diagnostic.message)
        };
        diagnostic.severity = Severity::Info;
        diagnostic.message =
            format!("also reported by {by}, one {kind} as pytest loads a conftest once{words}");
    }
}

/// How a bad outcome's test ended before this run, from `history` as it
/// was then: its result's `history`, set before anything else sees the
/// record, when the test has some.
pub(crate) struct PreviousOutcome<'h>(pub(crate) &'h History);

impl PreviousOutcome<'_> {
    pub(crate) fn apply(&self, record: &mut Record) {
        if let Event::TestFinished(result) = &mut record.event
            && result.outcome.is_bad()
            && result.history.is_none()
        {
            result.history = self.0.previous(&result.id);
        }
    }
}

#[cfg(test)]
mod tests;
