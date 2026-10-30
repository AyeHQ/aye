//! Which files pytest collects: test modules, by `python_files` and the
//! command line ([`PytestRecognizer::python_module`]), what its ignore
//! rules spare ([`ignore`]), and what its doctest plugin collects
//! ([`doctest_plugin`]).
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **A `conftest.py` is a test module too** ([`Claim::TestsAndSupport`])
//!   when it matches `python_files` (and isn't ignored) or is named on the
//!   command line, as pytest's `pytest_collect_file` doesn't spare it; it
//!   stays support when ignored. With `--noconftest` it is an ordinary file.
//!
//! [`Claim::TestsAndSupport`]: aye_adapter::Claim::TestsAndSupport

pub(crate) mod doctest_plugin;
pub(crate) mod ignore;

use std::path::Path;

use aye_python_stdlib::fnmatch;

use crate::conftest::Summary;
use crate::{PytestRecognizer, SETS};
use ignore::Ignored;

impl PytestRecognizer {
    /// pytest's `pytest_collect_file` for Python modules: a `.py` file
    /// named on the command line, or matching `python_files` (or a pattern
    /// a conftest adds) and not ignored, is a test module. With `-p
    /// no:python` none is: pytest's python plugin collects them.
    pub(crate) fn python_module(&self, rel: &str, explicit: bool) -> bool {
        if self.settings.python_blocked {
            return false;
        }
        let name = rel.rsplit('/').next().unwrap_or(rel);
        // pathlib's `suffix`: a dotfile like `.py` has none.
        if Path::new(name).extension().is_none_or(|e| e != "py") {
            return false;
        }
        if explicit {
            return true;
        }
        let abs = self.root.join(rel);
        self.ignored(&abs) != Ignored::Yes
            && (self
                .config
                .python_files
                .value
                .iter()
                .any(|pattern| fnmatch::fnmatch_ex(pattern, &abs, SETS))
                || self
                    .added_python_files(abs.parent().unwrap_or(&abs))
                    .iter()
                    .any(|pattern| fnmatch::fnmatch_ex(pattern, &abs, SETS)))
    }

    /// `python_files` patterns the support modules pytest has registered
    /// before collecting the files in `dir` add to the ini value
    /// (`config.addinivalue_line("python_files", …)` in a
    /// `pytest_configure`). A conftest outside `dir`'s ancestors that
    /// pytest happens to load earlier is not seen.
    fn added_python_files(&self, dir: &Path) -> Vec<String> {
        // Most projects add none: look without collecting first.
        let adds = |s: &Summary| !s.ini.python_files.is_empty();
        let any = self.preparse.iter().any(|plugin| adds(&plugin.summary))
            || self.outer.iter().any(|s| adds(s))
            || dir
                .ancestors()
                .take_while(|d| d.starts_with(&self.root))
                .filter_map(|d| self.conftest_in(d))
                .any(|s| adds(&s));
        if !any {
            return Vec::new();
        }
        self.configuring(dir)
            .iter()
            .flat_map(|s| s.ini.python_files.iter().cloned())
            .collect()
    }
}
