//! The merge: each file's cases by adapter priority, what refinement
//! replaced, the cases it found outside the requested files, `no-tests`
//! notes, and ids collected twice.

use std::collections::{BTreeMap, HashMap, HashSet};

use aye_events::{
    Case, Diagnostic, DiagnosticCode, DiagnosticPath, ProjectPathRef, Severity, TestId,
};

use crate::recognize::TestFile;
use crate::refine::Refinement;
use crate::walk::{AdapterId, FileId, Tree, WalkKey};
use crate::{Collection, Collector, Prepared};

impl Collector<'_> {
    pub(crate) fn assemble(
        &self,
        tree: &Tree<'_>,
        test_files: Vec<TestFile>,
        mut refinement: Refinement,
        mut diagnostics: Vec<Diagnostic>,
    ) -> Collection {
        /// One file's diagnostics, and its final cases by recognizer.
        struct Merged {
            /// The file's path, empty for what belongs to no file.
            rel: String,
            notes: Vec<Diagnostic>,
            groups: Vec<(AdapterId, Vec<Case>)>,
        }

        // Cases a reply put in files it wasn't asked about: those in the
        // package of a requested `__init__.py` are anchored there, the rest
        // go after every file.
        let mut anchored: BTreeMap<FileId, Vec<(AdapterId, Case)>> = BTreeMap::new();
        let mut unanchored = Vec::new();
        for stray in std::mem::take(&mut refinement.stray) {
            let mut rest = Vec::new();
            for case in stray.refined.cases {
                match package_anchor(tree, &stray.requested, case.id.path().as_str()) {
                    Some(at) => anchored
                        .entry(at)
                        .or_default()
                        .push((stray.recognizer, case)),
                    None => rest.push(case),
                }
            }
            unanchored.push((stray.recognizer, rest, stray.refined.diagnostics));
        }

        // Paths a reply said something about outside the requested files
        // (a directory a conftest skipped): that explains every file below.
        let explained: Vec<&str> = unanchored
            .iter()
            .flat_map(|(_, _, diagnostics)| diagnostics)
            .map(|d| d.path.as_str())
            .filter(|path| !path.is_empty())
            .collect();
        let mut files = Vec::new();
        let mut refined_files = 0;
        let mut probes = HashMap::new();
        for TestFile { job, found } in test_files {
            let mut notes: Vec<Diagnostic> = found.diagnostic.into_iter().collect();
            let mut groups = Vec::new();
            let (mut refined, mut fell_back) = (false, false);
            let rel = &tree.entries[job.file].rel;
            for (i, recognition) in found.by {
                let cases = if let Some(result) = refinement.refined.remove(&(job.file, i)) {
                    refined = true;
                    // The framework's import has the final word on whether
                    // the file collects, and reports its own error: a static
                    // error for the file would count it twice, or fail a
                    // file the framework collects.
                    notes.extend(
                        recognition
                            .diagnostics
                            .into_iter()
                            .filter(|d| d.severity != Severity::Error || d.path != rel.as_str()),
                    );
                    notes.extend(result.diagnostics);
                    result.cases
                } else {
                    notes.extend(recognition.diagnostics);
                    if let Some(diagnostic) = refinement.fallbacks.remove(&(job.file, i)) {
                        fell_back = true;
                        notes.push(diagnostic);
                    }
                    recognition.cases
                };
                groups.push((i, cases));
            }
            refined_files += u32::from(refined);
            // A file the adapters already said something about (the
            // framework skipped or ignored it, a class it won't collect),
            // or a directory it is in (a conftest skipped it), has had its
            // explanation.
            let quiet = notes.iter().all(|d| d.path != rel.as_str())
                && !explained.iter().any(|dir| {
                    rel.as_path()
                        .starts_with_dir(ProjectPathRef::as_written(dir))
                });
            if found.parsed
                && quiet
                && !fell_back
                && !anchored.contains_key(&job.file)
                && groups.iter().all(|(_, c)| c.is_empty())
                && groups.iter().any(|(i, _)| {
                    !claims_any_name(&mut probes, &self.recognizers[*i], *i, rel.as_str())
                })
            {
                let names: Vec<&str> = groups
                    .iter()
                    .map(|(i, _)| self.recognizers[*i].0.as_str())
                    .collect();
                notes.push(Diagnostic {
                    severity: Severity::Info,
                    code: DiagnosticCode::NoTests,
                    path: rel.clone().into(),
                    span: None,
                    message: format!("claimed for tests by {}, but has none", names.join(", ")),
                });
            }
            files.push(Merged {
                rel: rel.as_str().to_owned(),
                notes,
                groups,
            });
        }
        for (anchor, cases) in anchored {
            let package = tree.entries[anchor].rel.as_str();
            let package = &package[..package.len() - INIT.len()];
            // By file, in walk order; by recognizer within each.
            let mut by_file: BTreeMap<WalkKey, Vec<(AdapterId, Vec<Case>)>> = BTreeMap::new();
            for (i, case) in cases {
                let groups = by_file
                    .entry(WalkKey::new(case.id.path().as_str()))
                    .or_default();
                match groups.iter_mut().find(|(by, _)| *by == i) {
                    Some((_, group)) => group.push(case),
                    None => groups.push((i, vec![case])),
                }
            }
            let Some(start) = files
                .iter()
                .position(|f| f.rel == tree.entries[anchor].rel.as_str())
            else {
                continue;
            };
            for (key, groups) in by_file {
                // Past the package's files that come before it in the walk.
                let mut at = start + 1;
                while at < files.len()
                    && files[at].rel.starts_with(package)
                    && WalkKey::new(&files[at].rel) < key
                {
                    at += 1;
                }
                if files.get(at).is_some_and(|f| f.rel == key.0) {
                    files[at].groups.extend(groups);
                    files[at].groups.sort_by_key(|(i, _)| *i);
                } else {
                    files.insert(
                        at,
                        Merged {
                            rel: key.0,
                            notes: Vec::new(),
                            groups,
                        },
                    );
                }
            }
        }
        for (i, cases, diagnostics) in unanchored {
            files.push(Merged {
                rel: String::new(),
                notes: diagnostics,
                groups: vec![(i, cases)],
            });
        }

        let mut best: HashMap<TestId, AdapterId> = HashMap::new();
        for (i, case) in files
            .iter()
            .flat_map(|file| &file.groups)
            .flat_map(|(i, cases)| cases.iter().map(move |case| (*i, case)))
        {
            best.entry(case.id.clone())
                .and_modify(|best| *best = (*best).min(i))
                .or_insert(i);
        }
        let mut emitted = HashSet::new();
        let mut cases = Vec::new();
        for file in files {
            diagnostics.extend(file.notes);
            for (i, group) in file.groups {
                for case in group {
                    let winner = best[&case.id];
                    if winner == i && emitted.insert(case.id.clone()) {
                        cases.push(case);
                    } else if winner == i {
                        diagnostics.push(self.duplicate(&case, i));
                    }
                    // Another adapter's case for an id a higher-priority
                    // one collects is dropped quietly: listing unittest
                    // before pytest is how a project runs its `TestCase`
                    // classes natively and the rest with pytest, which
                    // collects both.
                }
            }
        }
        Collection {
            cases,
            diagnostics,
            refined_files,
            interrupted: false,
        }
    }

    /// A case the adapter `by` collects again under an id it already gave.
    fn duplicate(&self, case: &Case, by: AdapterId) -> Diagnostic {
        // The framework selects by id, so it runs them all (pytest does).
        Diagnostic {
            severity: Severity::Warning,
            code: DiagnosticCode::DuplicateId,
            path: DiagnosticPath::as_written(case.id.path().as_str()),
            span: case.span.clone(),
            message: format!(
                "{} collects `{}` more than once; aye lists it once, \
                 but running it runs every test with that id",
                self.recognizers[by].0, case.id
            ),
        }
    }
}

/// A package's `__init__.py`, which importing the package runs.
const INIT: &str = "__init__.py";

/// The requested package `__init__.py` whose package holds `path` (the
/// innermost, when several do), among the files a refinement request named:
/// importing a package can make tests in files of it the walk didn't claim
/// (unittest's `load_tests` discovering another pattern), which belong at
/// the package's place.
fn package_anchor(tree: &Tree<'_>, requested: &[FileId], path: &str) -> Option<FileId> {
    requested
        .iter()
        .filter_map(|&at| {
            let package = tree.entries[at].rel.as_str().strip_suffix(INIT)?;
            let holds = (package.is_empty() || package.ends_with('/')) && path.starts_with(package);
            holds.then_some((package.len(), at))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, at)| at)
}

/// Whether `recognizer` would claim for tests any file of `rel`'s kind in
/// its directory, whatever its name
/// ([`Recognizer::claims_any_name`](aye_adapter::Recognizer::claims_any_name)):
/// then a file of that kind without tests is no surprise, and gets no
/// `no-tests` note. Asked once per directory, extension and recognizer.
fn claims_any_name(
    probes: &mut HashMap<(String, String, AdapterId), bool>,
    recognizer: &Prepared,
    index: AdapterId,
    rel: &str,
) -> bool {
    let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    let extension = name.rsplit_once('.').map_or("", |(_, ext)| ext);
    *probes
        .entry((dir.to_string(), extension.to_string(), index))
        .or_insert_with(|| recognizer.1.claims_any_name(dir, extension))
}
