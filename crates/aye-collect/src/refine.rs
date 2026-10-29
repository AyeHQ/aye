//! Refinement: asking a worker for the tests only an import can tell, with
//! its clean answers cached when nothing outside the key can change them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aye_engine::{RefineRequest, Refined, Refiner, Setting};
use aye_events::{Diagnostic, ProjectPath, Severity};

use crate::source::Source;
use crate::standalone::{self, Scope};
use crate::tree::Tree;
use crate::{Collector, Job, Recognized, codes};

/// The cache's own format, part of every key: entries written under other
/// rules are never read.
const FORMAT: &str = "refine-2";

#[derive(Default)]
pub(crate) struct Refinement {
    /// What a worker or the cache reported, by file and recognizer.
    pub refined: HashMap<(usize, usize), Refined>,
    /// Why a file kept its static guess, by file and recognizer.
    pub fallbacks: HashMap<(usize, usize), Diagnostic>,
    /// Reported cases and diagnostics outside the requested files.
    pub stray: Vec<Stray>,
}

/// What one request's reply held outside the files it named.
pub(crate) struct Stray {
    pub recognizer: usize,
    /// The files the request named.
    pub requested: Vec<usize>,
    pub refined: Refined,
}

impl Collector<'_> {
    /// Refine every file a recognizer flagged: from the cache when the file
    /// is standalone and nothing in its key changed (see the crate docs),
    /// otherwise in one request per adapter.
    pub(crate) fn refine(
        &self,
        tree: &Tree<'_>,
        support: &HashMap<usize, Source>,
        jobs: &[Job],
        recognized: &[Recognized],
        refiner: Option<&(dyn Refiner + Sync)>,
        seen: &Mutex<HashMap<PathBuf, bool>>,
    ) -> Refinement {
        let mut out = Refinement::default();
        let mut digests: HashMap<usize, u128> = HashMap::new();
        // Computed once, when a file first needs it.
        let mut environment: Option<Option<u128>> = None;
        // By support file and recognizer: whether it is standalone.
        let mut supports_standalone: HashMap<(usize, usize), bool> = HashMap::new();
        // By package `__init__.py` and recognizer: its digest when standalone.
        let mut inits: HashMap<(PathBuf, usize), Option<u128>> = HashMap::new();
        for (i, (adapter, recognizer)) in self.recognizers.iter().enumerate() {
            let pending: Vec<(&Job, &Recognized, &str)> = jobs
                .iter()
                .zip(recognized)
                .filter_map(|(job, found)| {
                    let (_, recognition) = found.by.iter().find(|(by, _)| *by == i)?;
                    Some((job, found, recognition.refine.as_deref()?))
                })
                .collect();
            if pending.is_empty() {
                continue;
            }
            let context = recognizer.context();
            let scope = self.scope(adapter, seen);
            let environment = *environment.get_or_insert_with(|| {
                self.caching()
                    .then(|| self.environment.digest(self.project.cwd))
                    .flatten()
            });
            let cache = match (self.options.cache_dir.as_deref(), environment) {
                (Some(dir), Some(environment)) if !self.loads_by_name(&context, &scope) => {
                    // The arguments after `--`, as adapters were given them.
                    let mut args = Fnv::new();
                    for arg in self.project.framework_args {
                        args.part(arg.as_bytes());
                    }
                    Some(Cache::new(
                        dir,
                        &[
                            FORMAT.as_bytes(),
                            self.options.version.as_bytes(),
                            self.project.root.as_os_str().as_encoded_bytes(),
                            self.project.cwd.as_os_str().as_encoded_bytes(),
                            &environment.to_le_bytes(),
                            &args.finish().to_le_bytes(),
                        ],
                        adapter,
                        &context,
                        &recognizer.settings(),
                    ))
                }
                _ => None,
            };

            let mut misses = Vec::new();
            for (job, found, reason) in pending {
                let standalone = cache.is_some()
                    && found
                        .dependence
                        .iter()
                        .any(|(by, why)| *by == i && why.is_none())
                    && job.chain(i).iter().all(|&at| {
                        *supports_standalone.entry((at, i)).or_insert_with(|| {
                            let entry = &tree.entries[at];
                            support
                                .get(&at)
                                .and_then(|source| source.file(&entry.rel, &entry.abs))
                                .is_some_and(|file| {
                                    standalone::dependence(file.module, &entry.abs, &scope)
                                        .is_none()
                                })
                        })
                    });
                let packages = if standalone {
                    self.package_inits(&tree.entries[job.at].abs, i, &scope, &mut inits)
                } else {
                    None
                };
                let key = cache.as_ref().zip(found.digest).zip(packages).map(
                    |((cache, content), packages)| {
                        let chain: Vec<(ProjectPath, u128)> = job
                            .chain(i)
                            .iter()
                            .filter_map(|&at| {
                                let source = support.get(&at)?;
                                let hash =
                                    *digests.entry(at).or_insert_with(|| digest(source.bytes()));
                                Some((tree.entries[at].rel.clone(), hash))
                            })
                            .chain(packages)
                            .collect();
                        cache.key(
                            tree.entries[job.at].rel.as_str(),
                            content,
                            chain.iter().map(|(rel, hash)| (rel.as_str(), *hash)),
                        )
                    },
                );
                if let Some(hit) = cache
                    .as_ref()
                    .zip(key.as_deref())
                    .and_then(|(c, k)| c.read(k))
                {
                    out.refined.insert((job.at, i), hit);
                } else {
                    misses.push((job.at, reason, key));
                }
            }
            if misses.is_empty() {
                continue;
            }

            let Some(refiner) = refiner else {
                for (at, reason, _) in misses {
                    let message = format!(
                        "{adapter} must import this file to know its tests ({reason}), \
                         but refinement is off; keeping the static guess"
                    );
                    let diagnostic =
                        fallback(Severity::Info, codes::NOT_REFINED, tree, at, message);
                    out.fallbacks.insert((at, i), diagnostic);
                }
                continue;
            };
            let request = RefineRequest {
                adapter: (*adapter).to_string(),
                files: misses
                    .iter()
                    .map(|(at, _, _)| tree.entries[*at].rel.as_str().to_owned())
                    .collect(),
                context,
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
                    let requested: Vec<usize> = misses.iter().map(|(at, _, _)| *at).collect();
                    for (at, _, key) in misses {
                        let part = parts
                            .remove(tree.entries[at].rel.as_str())
                            .unwrap_or_default();
                        if let (Some(cache), Some(key), true) = (&cache, &key, clean) {
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
                        let message = format!(
                            "{adapter} could not import this file to know its tests ({}); \
                             keeping the static guess",
                            err.0
                        );
                        let diagnostic =
                            fallback(Severity::Warning, codes::REFINE_FAILED, tree, at, message);
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
        recognizer: usize,
        scope: &Scope<'_>,
        memo: &mut HashMap<(PathBuf, usize), Option<u128>>,
    ) -> Option<Vec<(ProjectPath, u128)>> {
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
            let hash = *memo.entry((init.clone(), recognizer)).or_insert_with(|| {
                let (source, _) = crate::source::load(&init, rel.as_str());
                let source = source.filter(|s| !s.is_lossy())?;
                let file = source.file(&rel, &init)?;
                standalone::dependence(file.module, &init, scope)
                    .is_none()
                    .then(|| digest(source.bytes()))
            });
            out.push((rel, hash?));
        }
        out.reverse();
        Some(out)
    }
}

fn fallback(
    severity: Severity,
    code: &str,
    tree: &Tree<'_>,
    at: usize,
    message: String,
) -> Diagnostic {
    Diagnostic {
        severity,
        code: code.to_string(),
        path: tree.entries[at].rel.as_str().to_owned(),
        span: None,
        message,
    }
}

/// Refinement results on disk, one JSON file per file and adapter:
/// `<cache dir>/refine/<adapter>/<key>.json`.
struct Cache {
    /// The cache directory, created through `aye_fs::prepare_dir` so
    /// that the state directory holding it is marked as a cache.
    top: PathBuf,
    dir: PathBuf,
    /// Everything in the key that doesn't depend on the file.
    base: u128,
}

impl Cache {
    /// `parts` are the run's inputs (format, version, root, cwd,
    /// environment, the project's framework arguments), hashed before the
    /// adapter's own.
    fn new(
        cache_dir: &Path,
        parts: &[&[u8]],
        adapter: &str,
        context: &serde_json::Value,
        settings: &[Setting],
    ) -> Self {
        let mut hash = Fnv::new();
        for part in parts {
            hash.part(part);
        }
        hash.part(adapter.as_bytes());
        hash.part(context.to_string().as_bytes());
        // The content of the files the context names: pytest's inifile holds
        // options aye doesn't read itself.
        let mut strings = Vec::new();
        strings_in(context, &mut strings);
        for text in strings {
            let path = Path::new(text);
            if path.is_absolute()
                && let Ok(bytes) = std::fs::read(path)
            {
                hash.part(text.as_bytes());
                hash.part(&bytes);
            }
        }
        for setting in settings {
            hash.part(setting.name.as_bytes());
            hash.part(setting.value.as_bytes());
        }
        Self {
            top: cache_dir.to_path_buf(),
            dir: cache_dir.join(aye_fs::REFINE_CACHE).join(adapter),
            base: hash.finish(),
        }
    }

    fn key<'a>(
        &self,
        rel: &str,
        digest: u128,
        support: impl Iterator<Item = (&'a str, u128)>,
    ) -> String {
        let mut hash = Fnv::new();
        hash.part(&self.base.to_le_bytes());
        hash.part(rel.as_bytes());
        hash.part(&digest.to_le_bytes());
        for (rel, digest) in support {
            hash.part(rel.as_bytes());
            hash.part(&digest.to_le_bytes());
        }
        format!("{:032x}", hash.finish())
    }

    fn path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// An entry, when it holds a clean result (see [`Cache::cacheable`]).
    fn read(&self, key: &str) -> Option<Refined> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        serde_json::from_slice(&bytes).ok().filter(Self::cacheable)
    }

    /// Only a clean result is cached: one with cases and no diagnostic. An
    /// import error or a module-level skip may be the environment's, and
    /// fixed without touching the file; a file with no tests at all costs
    /// little to refine again, and a stale one would hide tests.
    fn cacheable(refined: &Refined) -> bool {
        !refined.cases.is_empty() && refined.diagnostics.is_empty()
    }

    /// Best effort: a cache that can't be written only costs time. Written
    /// aside and renamed, so a concurrent run never reads half an entry.
    fn write(&self, key: &str, refined: &Refined) {
        if !Self::cacheable(refined) {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(refined) else {
            return;
        };
        let aside = self.dir.join(format!("{key}.{}.tmp", std::process::id()));
        let written = aye_fs::prepare_dir(&self.top)
            .and_then(|()| std::fs::create_dir_all(&self.dir))
            .and_then(|()| std::fs::write(&aside, bytes))
            .and_then(|()| std::fs::rename(&aside, self.path(key)));
        if written.is_err() {
            let _ = std::fs::remove_file(&aside);
        }
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
            .chain(self.environment.framework_args.iter().map(String::as_str));
        standalone::loads_by_name(texts, scope).is_some()
    }
}

/// The content digest of a file.
pub(crate) fn digest(bytes: &[u8]) -> u128 {
    let mut hash = Fnv::new();
    hash.bytes(bytes);
    hash.finish()
}

/// FNV-1a, 128 bits: stable across builds and platforms, unlike `std`'s
/// hasher, which is all a cache key needs.
pub(crate) struct Fnv(u128);

impl Fnv {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

    pub(crate) fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u128::from(byte)).wrapping_mul(Self::PRIME);
        }
    }

    /// Length-prefixed, so that consecutive parts can't run into each other.
    pub(crate) fn part(&mut self, bytes: &[u8]) {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        self.bytes(&len.to_le_bytes());
        self.bytes(bytes);
    }

    pub(crate) fn finish(&self) -> u128 {
        self.0
    }
}
