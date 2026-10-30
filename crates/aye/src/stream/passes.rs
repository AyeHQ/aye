//! The passes that rewrite the runner's records before anything consumes
//! them: a failed test's rerun spelled from where aye runs ([`Here`]), and
//! one error for what pytest reports once, a file's collection errors from
//! the adapters pytest collects in one module ([`OneModuleErrors`]) and a
//! conftest that fails to load ([`OneConftestError`]).

use std::collections::HashMap;
use std::path::Path;

use aye_events::{
    AdapterName, Diagnostic, DiagnosticCode, Event, Record, RerunCommand, Severity, TestId,
};
use aye_fs::{normalize_absolute, relative_below_root};

use crate::registry;

/// Where aye runs, for the reruns it prints.
pub(crate) struct Here<'a> {
    pub(crate) cwd: &'a Path,
    pub(crate) root: &'a Path,
    /// aye's options that shape the run, which a rerun repeats before the
    /// id ([`aye_protocol::BatchContext::rerun_args`]).
    pub(crate) rerun_args: &'a [String],
}

impl Here<'_> {
    /// Make a failed test's rerun (`aye <id>`) rerun it from the current
    /// directory. Ids are relative to the root, and a path is taken from
    /// the current directory first, then from the root: from a
    /// subdirectory holding a path of the same name (`tests/tests/…`), the
    /// id would name another file. There the rerun names the test's file
    /// relative to the current directory; elsewhere it stays the id.
    pub(crate) fn rerun_from_here(&self, record: &mut Record) {
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
    /// it would name another file from here; see [`Self::rerun_from_here`].
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
/// in one module ([`registry::collects_in_one_module`]: the unittest and
/// pytest adapters, which may each have tests in the file), counted as
/// pytest, the referee, counts them: its `Module` fails once, however many
/// of these adapters load the file. So a file shows as many errors as the
/// adapter reporting the most for it (pytest's batch may report its doctest
/// collector's too); another adapter's beyond those becomes an info saying
/// so. It isn't dropped, so the log's `seq` keeps counting every record.
/// Collection's errors count too ([`Self::refined`]): aye's pytest
/// front-end lists every pytest file in refinement, so a file that fails
/// to import there has its error from collection, not from a batch.
#[derive(Debug, Default)]
pub(crate) struct OneModuleErrors {
    /// Each batch's adapter, from its `batch_started`.
    adapters: HashMap<aye_events::BatchIndex, AdapterName>,
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
            || registry::is_conftest(diagnostic.path.as_str())
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

    pub(crate) fn see(&mut self, record: &mut Record) {
        let batch = record.envelope.batch;
        match &mut record.event {
            Event::BatchStarted { adapter, .. } => {
                if let Some(batch) = batch {
                    self.adapters.insert(batch, adapter.clone());
                }
            }
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    && diagnostic.code == DiagnosticCode::CollectionError
                    && !diagnostic.path.is_nowhere() =>
            {
                let Some(adapter) = batch.and_then(|batch| self.adapters.get(&batch)) else {
                    return;
                };
                if !registry::collects_in_one_module(adapter) {
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
/// (`registry::is_conftest`) that another adapter already reported with
/// that code, in refinement (where who reported it isn't known, and each
/// adapter reports one once) or in a batch, becomes an info saying so,
/// with its own words when they differ, kept in the stream as
/// [`OneModuleErrors`] keeps its. One adapter's own repeats (every pytest
/// batch reports an initial conftest's failure) stay errors, which
/// renderers show once.
#[derive(Debug, Default)]
pub(crate) struct OneConftestError {
    /// Each batch's adapter, from its `batch_started`.
    adapters: HashMap<aye_events::BatchIndex, AdapterName>,
    /// By code and path, who reported the error first (an adapter, or
    /// `None` for refinement), and its message.
    first: HashMap<(DiagnosticCode, String), (Option<AdapterName>, String)>,
}

impl OneConftestError {
    /// A record from a batch.
    pub(crate) fn see_record(&mut self, record: &mut Record) {
        let batch = record.envelope.batch;
        match &mut record.event {
            Event::BatchStarted { adapter, .. } => {
                if let Some(batch) = batch {
                    self.adapters.insert(batch, adapter.clone());
                }
            }
            Event::Diagnostic(diagnostic) => {
                let adapter = batch.and_then(|batch| self.adapters.get(&batch)).cloned();
                if adapter.is_some() {
                    self.see(adapter.as_ref(), diagnostic);
                }
            }
            _ => {}
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
            || !registry::is_conftest(diagnostic.path.as_str())
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

#[cfg(test)]
mod tests;
