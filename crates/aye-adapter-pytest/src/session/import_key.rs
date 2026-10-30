//! The name pytest imports a test file under, as its cases' first
//! affinity key.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The first affinity key is the module's import name** (`import:NAME`),
//!   as pytest computes it for the import mode in effect: the dotted name
//!   from the outermost package for `prepend` and `append` (the basename
//!   for a file outside packages), the rootdir-relative name for
//!   `importlib` outside packages, and the stem with
//!   `consider_namespace_packages`. The scheduler groups cases by their
//!   first key, so files pytest can't import in one session (its "import
//!   file mismatch") share a batch and the clash is reported as pytest
//!   reports it, whatever the worker count. The file and its classes follow.

use std::path::Path;

use aye_events::AffinityKey;
use aye_pytest::config;
use aye_pytest::pathlib::{compute_module_name, resolve_package_path};

use crate::PytestRecognizer;

impl PytestRecognizer {
    /// The name pytest imports a test file under, for the import mode in
    /// effect, as the first affinity key of its cases: `import:NAME`.
    /// Files that share it can't be imported in one session (pytest's
    /// "import file mismatch"), so the scheduler, which groups cases by
    /// their first key, keeps them in one batch and pytest reports the
    /// clash as it would on its own, whatever the worker count. With
    /// `consider_namespace_packages` the name depends on `sys.path` at
    /// runtime; the file's stem stands in, which groups every file that
    /// could clash (and some that can't).
    pub(crate) fn import_key(&self, file: &Path) -> AffinityKey {
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.config.consider_namespace_packages {
            return AffinityKey::import(&stem);
        }
        let packaged = resolve_package_path(file)
            .and_then(|package| compute_module_name(package.parent().unwrap_or(&package), file));
        let name = match (packaged, self.config.import_mode) {
            (Some(name), _) => name,
            (None, config::ImportMode::Importlib) => {
                // `module_name_from_path`: unique per file.
                let rel = file.with_extension("");
                let parts: Vec<String> = match rel.strip_prefix(&self.config.rootdir) {
                    Ok(rel) => rel
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                    Err(_) => rel
                        .components()
                        .skip(1)
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect(),
                };
                let mut parts = parts;
                if parts.len() >= 2 && parts.last().is_some_and(|p| p == "__init__") {
                    parts.pop();
                }
                parts
                    .iter()
                    .map(|p| p.replace('.', "_"))
                    .collect::<Vec<_>>()
                    .join(".")
            }
            (None, _) => stem,
        };
        AffinityKey::import(&name)
    }
}
