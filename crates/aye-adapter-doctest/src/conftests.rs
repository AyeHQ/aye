//! The project's conftests as the recognizer reads them: which may mark
//! doctest items, and which may skip the doctests below them while pytest
//! loads them.

use std::path::{Path, PathBuf};

use aye_adapter::Recognizer;
use aye_pytest::conftest;

use crate::{DoctestRecognizer, finder};

/// A `conftest.py` of the project, as the recognizer reads it.
#[derive(Debug)]
pub(crate) struct ProjectConftest {
    pub(crate) path: PathBuf,
    /// Which of [`conftest::MARKING_HOOKS`] it names.
    pub(crate) hooks: [bool; 2],
    /// It may raise pytest's `skip` while loaded.
    pub(crate) skips: bool,
}

impl DoctestRecognizer {
    /// The project's conftests (in the directories collection enters),
    /// which of [`conftest::MARKING_HOOKS`] each names, and whether each may
    /// skip while loaded; none with `--noconftest`.
    fn project_conftests(&self) -> &[ProjectConftest] {
        self.project_conftests.get_or_init(|| {
            let mut found = Vec::new();
            if self.conftests.disabled {
                return found;
            }
            // A skip in one pytest loads at start-up stops pytest (an
            // uncaught exception): it skips no directory.
            let initial = self.conftests.initial(&self.roots, &self.cwd);
            let mut stack = vec![self.root.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                    } else if entry.file_name() == "conftest.py"
                        && let Ok(source) = std::fs::read_to_string(&path)
                    {
                        let hooks =
                            conftest::MARKING_HOOKS.map(|h| conftest::names_hook(&source, h));
                        // One that doesn't parse fails its directory: no skip.
                        let skips = !initial.contains(&path)
                            && ruff_python_parser::parse_module(&source).is_ok_and(|parsed| {
                                finder::may_skip_while_imported(parsed.syntax())
                            });
                        found.push(ProjectConftest { path, hooks, skips });
                    }
                }
            }
            found
        })
    }

    /// Why the tags (marks) and keywords of a file's cases may be more than
    /// the source says: a conftest hook in scope that may mark doctest
    /// items (`pytest_collection_modifyitems` from any conftest,
    /// `pytest_itemcollected` from one in the file's directory or above),
    /// or plugins aye can't read (`-p NAME`, `PYTEST_PLUGINS`).
    pub(crate) fn may_be_marked(&self, file: &Path) -> Option<String> {
        if let Some(plugin) = self.config.plugins.first() {
            return Some(format!("the plugin {plugin} (-p) may mark doctest items"));
        }
        if self.env_plugins {
            return Some("PYTEST_PLUGINS may mark doctest items".into());
        }
        let dir = file.parent().unwrap_or(file);
        for conftest in self.project_conftests() {
            let [modifyitems, itemcollected] = conftest.hooks;
            if modifyitems || (itemcollected && self.loaded_for(&conftest.path, dir)) {
                return Some(format!(
                    "{} may mark doctest items",
                    conftest.path.display()
                ));
            }
        }
        None
    }

    /// Whether pytest loads `conftest` before collecting the files of `dir`
    /// (`_loadconftestmodules`): it is in `dir` or above, within the
    /// confcutdir.
    fn loaded_for(&self, conftest: &Path, dir: &Path) -> bool {
        conftest
            .parent()
            .is_some_and(|d| dir.starts_with(d) && self.conftests.in_confcutdir(d))
    }

    /// Why `file`'s doctests are refined when a conftest pytest loads
    /// before collecting it (not at start-up) may skip while loaded
    /// (`pytest.importorskip`): pytest then skips that directory, and
    /// collects no doctest below it.
    pub(crate) fn skipping_conftest(&self, file: &Path) -> Option<String> {
        let dir = file.parent().unwrap_or(file);
        let conftest = self
            .project_conftests()
            .iter()
            .find(|c| c.skips && self.loaded_for(&c.path, dir))?;
        let shown = conftest.path.strip_prefix(&self.root).map_or_else(
            |_| conftest.path.display().to_string(),
            |rel| rel.to_string_lossy().replace('\\', "/"),
        );
        Some(format!(
            "{shown} may skip while pytest loads it, which skips the doctests below it"
        ))
    }
}
