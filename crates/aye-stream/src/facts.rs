//! What a renderer knows of the run from its records rather than from any
//! one result ([`RunFacts`]): the project, the workers, the run's number,
//! how many tests were selected, and each worker's Python.

use std::collections::HashMap;

use aye_events::{Event, Record, RunNumber, WorkerIndex};

/// The run, as its records say: `run_started`'s project root, whether it
/// only collects, its workers and its number in the run store (the last
/// `run_started`'s), `collection_finished`'s selected count, and each
/// worker's Python version, from its `worker_started`.
#[derive(Debug, Default)]
pub struct RunFacts {
    /// The project root, absolute.
    pub root: Option<String>,
    pub collect_only: bool,
    pub workers: u32,
    /// The run's number (`r81`).
    pub number: Option<RunNumber>,
    pub selected: Option<u32>,
    pythons: HashMap<Option<WorkerIndex>, String>,
}

impl RunFacts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note(&mut self, record: &Record) {
        match &record.event {
            Event::RunStarted {
                root,
                collect_only,
                workers,
                run,
                ..
            } => {
                self.root = Some(root.clone());
                self.collect_only = *collect_only;
                self.workers = *workers;
                self.number = *run;
            }
            Event::CollectionFinished { selected, .. } => self.selected = Some(*selected),
            Event::WorkerStarted {
                python, version, ..
            } => {
                if let Some(python) = python_version(python, version.as_deref()) {
                    self.pythons.insert(record.envelope.worker, python);
                }
            }
            _ => {}
        }
    }

    /// The Python version `worker` runs (`3.13.1`), once it started.
    pub fn python(&self, worker: Option<WorkerIndex>) -> Option<&str> {
        self.pythons.get(&worker).map(String::as_str)
    }
}

/// The Python version a `worker_started` names: its `version`'s first
/// word (`3.13.1`), else its `python` when that is a version itself.
fn python_version(python: &str, version: Option<&str>) -> Option<String> {
    match version.and_then(|version| version.split_whitespace().next()) {
        Some(version) => Some(version.to_owned()),
        None => python
            .starts_with(|c: char| c.is_ascii_digit())
            .then(|| python.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workers_python_is_its_versions_first_word() {
        assert_eq!(
            python_version("/usr/bin/python3", Some("3.13.1 (main, …) [Clang]")).as_deref(),
            Some("3.13.1")
        );
        assert_eq!(python_version("3.12", None).as_deref(), Some("3.12"));
        assert_eq!(python_version("/usr/bin/python3", None), None);
    }
}
