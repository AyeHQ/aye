//! Refinement: asking a worker for the tests only an import can tell, with
//! its clean answers cached when nothing outside the key can change them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aye_adapter::RefineReason;
use aye_events::{ContentDigest, ProjectPath};
use aye_protocol::{RefineRequest, Refined, Refiner};

use crate::Collector;
use crate::diagnostic::CollectDiagnostic;
use crate::recognize::{Job, Recognized, TestFile};
use crate::source::Source;
use crate::walk::{AdapterId, FileId, Tree};

mod cache;
mod key;
pub(crate) mod standalone;

pub(crate) use cache::Cacheability;
use cache::RefineCache;
use key::{AdapterKey, BaseKey, RunKey};
use standalone::Scope;

#[derive(Default)]
pub(crate) struct Refinement {
    /// What a worker or the cache reported, by file and recognizer.
    pub refined: HashMap<(FileId, AdapterId), Refined>,
    /// Why a file kept its static guess, by file and recognizer.
    pub fallbacks: HashMap<(FileId, AdapterId), CollectDiagnostic>,
    /// Reported cases and diagnostics outside the requested files.
    pub stray: Vec<Stray>,
}

/// What one request's reply held outside the files it named.
pub(crate) struct Stray {
    pub recognizer: AdapterId,
    /// The files the request named.
    pub requested: Vec<FileId>,
    pub refined: Refined,
}

impl Collector<'_> {
    /// Refine every file a recognizer flagged: from the cache when the file
    /// is standalone and nothing in its key changed (see the crate docs),
    /// otherwise in one request per adapter.
    pub(crate) fn refine(
        &self,
        tree: &Tree<'_>,
        support: &HashMap<FileId, Source>,
        test_files: &[TestFile],
        refiner: Option<&(dyn Refiner + Sync)>,
        seen: &Mutex<HashMap<PathBuf, bool>>,
    ) -> Refinement {
        let mut out = Refinement::default();
        let mut digests: HashMap<FileId, ContentDigest> = HashMap::new();
        // Computed once, when a file first needs it.
        let mut environment: Option<Option<ContentDigest>> = None;
        // By support file and recognizer: whether it is standalone.
        let mut supports_standalone: HashMap<(FileId, AdapterId), bool> = HashMap::new();
        // By package `__init__.py` and recognizer: its digest when standalone.
        let mut inits: HashMap<(PathBuf, AdapterId), Option<ContentDigest>> = HashMap::new();
        for (i, (adapter, recognizer)) in self.recognizers.iter_enumerated() {
            let pending: Vec<(&Job, &Recognized, &RefineReason)> = test_files
                .iter()
                .filter_map(|TestFile { job, found }| {
                    let (_, recognition) = found.by.iter().find(|(by, _)| *by == i)?;
                    Some((job, found, recognition.refine.as_ref()?))
                })
                .collect();
            if pending.is_empty() {
                continue;
            }
            let context = recognizer.context();
            let scope = self.scope(adapter.as_str(), seen);
            let environment = *environment.get_or_insert_with(|| {
                self.caching()
                    .and_then(|environment| environment.digest(self.project.cwd))
            });
            let cache = match (self.options.cache_dir.as_deref(), environment) {
                (Some(dir), Some(environment))
                    if !self.loads_by_name(context.as_value(), &scope) =>
                {
                    let run = RunKey {
                        version: &self.options.version,
                        root: self.project.root,
                        cwd: self.project.cwd,
                        environment,
                        framework_args: self.project.framework_args,
                    };
                    let settings = recognizer.settings();
                    let base = BaseKey::new(
                        &run,
                        &AdapterKey {
                            name: adapter,
                            context: context.as_value(),
                            settings: &settings,
                        },
                    );
                    Some(RefineCache::new(dir, adapter, base))
                }
                _ => None,
            };

            let mut misses = Vec::new();
            for (job, found, reason) in pending {
                let standalone = cache.is_some()
                    && found
                        .cacheability
                        .iter()
                        .any(|(by, cacheability)| *by == i && cacheability.is_standalone())
                    && job.chain(i).iter().all(|&at| {
                        *supports_standalone.entry((at, i)).or_insert_with(|| {
                            let entry = &tree.entries[at];
                            support
                                .get(&at)
                                .and_then(|source| source.file(&entry.rel, &entry.abs))
                                .is_some_and(|file| {
                                    standalone::cacheability(file.module, &entry.abs, &scope)
                                        .is_standalone()
                                })
                        })
                    });
                let packages = if standalone {
                    self.package_inits(&tree.entries[job.file].abs, i, &scope, &mut inits)
                } else {
                    None
                };
                let key = cache.as_ref().zip(found.digest).zip(packages).map(
                    |((cache, content), packages)| {
                        let runs_first: Vec<(ProjectPath, ContentDigest)> = job
                            .chain(i)
                            .iter()
                            .filter_map(|&at| {
                                let source = support.get(&at)?;
                                let digest = *digests
                                    .entry(at)
                                    .or_insert_with(|| ContentDigest::of(source.bytes()));
                                Some((tree.entries[at].rel.clone(), digest))
                            })
                            .chain(packages)
                            .collect();
                        cache.key(&tree.entries[job.file].rel, content, &runs_first)
                    },
                );
                if let Some(hit) = cache.as_ref().zip(key).and_then(|(c, k)| c.read(k)) {
                    out.refined.insert((job.file, i), hit);
                } else {
                    misses.push((job.file, reason, key));
                }
            }
            if misses.is_empty() {
                continue;
            }

            let Some(refiner) = refiner else {
                for (at, reason, _) in misses {
                    let diagnostic = CollectDiagnostic::NotRefined {
                        adapter: adapter.clone(),
                        path: tree.entries[at].rel.clone(),
                        reason: reason.clone(),
                    };
                    out.fallbacks.insert((at, i), diagnostic);
                }
                continue;
            };
            let request = RefineRequest {
                adapter: adapter.clone(),
                files: misses
                    .iter()
                    .map(|(at, _, _)| tree.entries[*at].rel.clone())
                    .collect(),
                context: context.into(),
                digests_only: false,
            };
            match refiner.refine(&request) {
                Ok(reply) => {
                    let mut parts: HashMap<&str, Refined> = request
                        .files
                        .iter()
                        .map(|rel| (rel.as_str(), Refined::default()))
                        .collect();
                    let mut stray = Refined::default();
                    for case in reply.cases {
                        match parts.get_mut(case.id.path().as_str()) {
                            Some(part) => part.cases.push(case),
                            None => stray.cases.push(case),
                        }
                    }
                    for diagnostic in reply.diagnostics {
                        match parts.get_mut(diagnostic.path.as_str()) {
                            Some(part) => part.diagnostics.push(diagnostic),
                            None => stray.diagnostics.push(diagnostic),
                        }
                    }
                    // Anything reported outside the requested files (a
                    // conftest that failed to import, a usage error, cases
                    // from elsewhere) may have shaped every part: cache none.
                    let clean = stray.cases.is_empty() && stray.diagnostics.is_empty();
                    let requested: Vec<FileId> = misses.iter().map(|(at, _, _)| *at).collect();
                    for (at, _, key) in misses {
                        let part = parts
                            .remove(tree.entries[at].rel.as_str())
                            .unwrap_or_default();
                        if let (Some(cache), Some(key), true) = (&cache, key, clean) {
                            cache.write(key, &part);
                        }
                        out.refined.insert((at, i), part);
                    }
                    if !clean {
                        out.stray.push(Stray {
                            recognizer: i,
                            requested,
                            refined: stray,
                        });
                    }
                }
                Err(err) => {
                    for (at, _, _) in misses {
                        let diagnostic = CollectDiagnostic::RefineFailed {
                            adapter: adapter.clone(),
                            path: tree.entries[at].rel.clone(),
                            error: err.0.clone(),
                        };
                        out.fallbacks.insert((at, i), diagnostic);
                    }
                }
            }
        }
        out
    }
}

impl Collector<'_> {
    /// The package `__init__.py` files that importing the file at `abs`
    /// may run (in its directory and each one above it up to the root,
    /// outermost first), with their digests; `None` when one of them isn't
    /// standalone or can't be read.
    fn package_inits(
        &self,
        abs: &Path,
        recognizer: AdapterId,
        scope: &Scope<'_>,
        memo: &mut HashMap<(PathBuf, AdapterId), Option<ContentDigest>>,
    ) -> Option<Vec<(ProjectPath, ContentDigest)>> {
        let root = self.project.root;
        let mut out = Vec::new();
        for dir in abs
            .parent()?
            .ancestors()
            .take_while(|d| d.starts_with(root))
        {
            let init = dir.join("__init__.py");
            if init == abs || !init.is_file() {
                continue;
            }
            let rel = ProjectPath::as_written(aye_fs::relative_through_root(&init, root));
            let digest = *memo.entry((init.clone(), recognizer)).or_insert_with(|| {
                let (source, _) = crate::source::load(&init, &rel);
                let source = source.filter(|s| !s.is_lossy())?;
                let file = source.file(&rel, &init)?;
                standalone::cacheability(file.module, &init, scope)
                    .is_standalone()
                    .then(|| ContentDigest::of(source.bytes()))
            });
            out.push((rel, digest?));
        }
        out.reverse();
        Some(out)
    }
}

/// Every string in a JSON value, depth first.
fn strings_in<'v>(value: &'v serde_json::Value, out: &mut Vec<&'v str>) {
    match value {
        serde_json::Value::String(text) => out.push(text),
        serde_json::Value::Array(items) => {
            for item in items {
                strings_in(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values() {
                strings_in(item, out);
            }
        }
        _ => {}
    }
}

impl Collector<'_> {
    /// Whether the recognizer's context or the framework's arguments (the
    /// project's or the environment's) may load a module whose content the
    /// key can't hold (`-p tests.plugin`, `-p installed_plugin`; see
    /// [`standalone::loads_by_name`]).
    fn loads_by_name(&self, context: &serde_json::Value, scope: &Scope<'_>) -> bool {
        let mut strings = Vec::new();
        strings_in(context, &mut strings);
        let texts = strings
            .into_iter()
            .chain(self.project.framework_args.iter().map(String::as_str))
            .chain(
                self.environment
                    .iter()
                    .flat_map(|environment| &environment.framework_args)
                    .map(String::as_str),
            );
        standalone::loads_by_name(texts, scope).is_some()
    }
}
