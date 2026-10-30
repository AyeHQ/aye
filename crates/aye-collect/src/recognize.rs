//! Each test file read, parsed and recognized by the recognizers that
//! claim it, with what the refinement cache needs to know of it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use aye_adapter::{Recognition, RefineReason, SourceFile};
use aye_events::{ContentDigest, Diagnostic};

use crate::Collector;
use crate::refine::standalone;
use crate::source::{self, Source};
use crate::walk::Tree;

/// A test file and, for each recognizer claiming it, that recognizer's
/// support files.
pub(crate) struct Job {
    pub(crate) at: usize,
    pub(crate) claimants: Vec<(usize, Vec<usize>)>,
}

impl Job {
    pub(crate) fn chain(&self, recognizer: usize) -> &[usize] {
        self.claimants
            .iter()
            .find(|(by, _)| *by == recognizer)
            .map_or(&[], |(_, chain)| chain)
    }
}

/// What the recognizers said about one test file.
pub(crate) struct Recognized {
    /// Why the file couldn't be read or parsed.
    pub(crate) diagnostic: Option<Diagnostic>,
    pub(crate) parsed: bool,
    /// By recognizer, in priority order.
    pub(crate) by: Vec<(usize, Recognition)>,
    /// The content digest, when the cache needs it.
    pub(crate) digest: Option<ContentDigest>,
    /// By recognizer asking for refinement, why the file's refinement may
    /// depend on something the cache's key leaves out (`None`: it can't).
    /// Empty when the cache is off.
    pub(crate) dependence: Vec<(usize, Option<String>)>,
}

impl Recognized {
    /// For a file collection stopped before reading.
    pub(crate) const fn nothing() -> Self {
        Self {
            diagnostic: None,
            parsed: false,
            by: Vec::new(),
            digest: None,
            dependence: Vec::new(),
        }
    }
}

impl Collector<'_> {
    pub(crate) fn recognize(
        &self,
        tree: &Tree<'_>,
        support: &HashMap<usize, Source>,
        job: &Job,
        seen: &Mutex<HashMap<PathBuf, bool>>,
    ) -> Recognized {
        let entry = &tree.entries[job.at];
        // A support file (one claimed as tests and support) was loaded with
        // the support files, and any problem reading it reported then.
        let (owned, diagnostic) = if entry.support != 0 {
            (None, None)
        } else {
            source::load(&entry.abs, &entry.rel)
        };
        let mut recognized = Recognized {
            diagnostic,
            parsed: false,
            by: Vec::new(),
            digest: None,
            dependence: Vec::new(),
        };
        let Some(source) = owned.as_ref().or_else(|| support.get(&job.at)) else {
            return recognized;
        };
        let Some(file) = source.file(&entry.rel, &entry.abs) else {
            return recognized;
        };
        recognized.parsed = true;
        for (i, chain) in &job.claimants {
            let support: Vec<SourceFile<'_>> = chain
                .iter()
                .filter_map(|at| {
                    let entry = &tree.entries[*at];
                    support.get(at)?.file(&entry.rel, &entry.abs)
                })
                .collect();
            let mut recognition =
                self.recognizers[*i]
                    .1
                    .recognize(&file, entry.requested(), &support);
            if source.is_lossy() && recognition.refine.is_none() {
                recognition.refine = Some(RefineReason::NotUtf8);
            }
            recognized.by.push((*i, recognition));
        }
        let refine = recognized.by.iter().any(|(_, r)| r.refine.is_some());
        if refine && self.caching().is_some() {
            recognized.digest = Some(ContentDigest::of(source.bytes()));
            for (i, recognition) in &recognized.by {
                if recognition.refine.is_none() {
                    continue;
                }
                let why = if source.is_lossy() {
                    Some("it is not valid UTF-8".to_string())
                } else {
                    let scope = self.scope(self.recognizers[*i].0.as_str(), seen);
                    standalone::dependence(file.module, &entry.abs, &scope)
                };
                recognized.dependence.push((*i, why));
            }
        }
        recognized
    }
}
