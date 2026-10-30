//! The refinement cache on disk ([`RefineCache`]): one entry per file and
//! adapter, named by its key, read when it holds a clean result and written
//! aside, then renamed; and when the cache may answer for a file at all
//! ([`Cacheability`]).

use std::path::{Path, PathBuf};

use aye_events::{AdapterName, ContentDigest, ProjectPath};
use aye_protocol::Refined;

use super::key::{BaseKey, FileKey, RefineKey};

/// Whether the cache may answer for a file, and keep what refining it
/// gives: never differently from a fresh import, since a stale answer
/// drops tests the framework would run. Its key holds the file, its
/// support files and its packages' `__init__.py` files, but the
/// environment's installed packages by name only, and nothing an import
/// reads beyond them, so the cache answers for a file only when the file,
/// each of its support files and each of those `__init__.py` files is
/// [`Cacheability::Standalone`] (and readable, and UTF-8), and when the
/// adapter's context and the framework's arguments load no plugin by name
/// (`standalone::loads_by_name`). Of what refining it gives, only a clean
/// result is written ([`RefineCache::write`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cacheability {
    /// It imports only pure standard-library modules and the adapter's
    /// framework, touches no file, process, clock or randomness, runs no
    /// code by name, and names no module of the project and no installed
    /// one in a string (`standalone`'s rules).
    Standalone,
    /// Why its refinement may depend on something the key leaves out
    /// (shown nowhere: it only tells the rules apart).
    Dependent(String),
}

impl Cacheability {
    pub(crate) const fn is_standalone(&self) -> bool {
        matches!(self, Self::Standalone)
    }
}

/// Refinement results on disk, one JSON file per file and adapter:
/// `<cache dir>/refine/<adapter>/<key>.json`.
pub(super) struct RefineCache {
    /// The cache directory, created through `aye_fs::prepare_dir` so
    /// that the state directory holding it is marked as a cache.
    top: PathBuf,
    dir: PathBuf,
    /// Everything in the adapter's keys that doesn't depend on the file.
    base: BaseKey,
}

impl RefineCache {
    /// `adapter`'s entries in `cache_dir`, their keys made from `base`.
    pub(super) fn new(cache_dir: &Path, adapter: &AdapterName, base: BaseKey) -> Self {
        Self {
            top: cache_dir.to_path_buf(),
            dir: cache_dir.join(aye_fs::REFINE_CACHE).join(adapter.as_str()),
            base,
        }
    }

    /// The key of the file at `path` whose content has the digest
    /// `content`, with what importing it runs first ([`FileKey::runs_first`]).
    pub(super) fn key(
        &self,
        path: &ProjectPath,
        content: ContentDigest,
        runs_first: &[(ProjectPath, ContentDigest)],
    ) -> RefineKey {
        FileKey {
            base: self.base,
            path,
            content,
            runs_first,
        }
        .key()
    }

    pub(super) fn path(&self, key: RefineKey) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }

    /// An entry, when it holds a clean result (see [`RefineCache::clean`]).
    pub(super) fn read(&self, key: RefineKey) -> Option<Refined> {
        let bytes = std::fs::read(self.path(key)).ok()?;
        serde_json::from_slice(&bytes).ok().filter(Self::clean)
    }

    /// Only a clean result is cached: one with cases and no diagnostic. An
    /// import error or a module-level skip may be the environment's, and
    /// fixed without touching the file; a file with no tests at all costs
    /// little to refine again, and a stale one would hide tests.
    fn clean(refined: &Refined) -> bool {
        !refined.cases.is_empty() && refined.diagnostics.is_empty()
    }

    /// Best effort: a cache that can't be written only costs time. Written
    /// aside and renamed, so a concurrent run never reads half an entry.
    pub(super) fn write(&self, key: RefineKey, refined: &Refined) {
        if !Self::clean(refined) {
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
