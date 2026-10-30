//! `--pyargs`: where pytest finds a dotted name its arguments give.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **`--pyargs` testpaths are looked up on `sys.path`** as pytest's
//!   `search_pypath` does (`pythonpath` first, then the rootdir,
//!   `PYTHONPATH` and `src`): a module's file or a regular package's
//!   directory, else the entry as a path.

use std::path::{Path, PathBuf};

use aye_python_semantic::module_resolver::find_module;
use aye_python_stdlib::identifier::is_identifier;

use crate::PytestRecognizer;

impl PytestRecognizer {
    /// Where pytest's `search_pypath` finds the dotted name `name` with
    /// `--pyargs`: a module's file, or a regular package's directory,
    /// looked up on `sys.path` as pytest has it then: `pythonpath` (put
    /// first), the rootdir (the working directory under `python -m
    /// pytest`), `PYTHONPATH`, then the project's `src` directories (an
    /// editable install's). A namespace package is no match (pytest then
    /// takes the name as a path).
    pub(crate) fn pyarg(&self, name: &str) -> Option<PathBuf> {
        if !name.split('.').all(is_identifier) {
            return None;
        }
        let mut search: Vec<PathBuf> = self.config.pythonpath.clone();
        search.push(self.config.rootdir.clone());
        search.extend(self.env_pythonpath.iter().cloned());
        search.push(self.config.rootdir.join("src"));
        search.push(self.root.join("src"));
        let found = find_module(name, &search)?;
        let dir = found.parent().map(Path::to_path_buf);
        Some(if found.file_name().is_some_and(|f| f == "__init__.py") {
            dir.unwrap_or(found)
        } else {
            found
        })
    }
}
