//! pytest's own doctest plugin, in a run without the doctest adapter:
//! what it collects, and what the recognizer makes of it.

use aye_adapter::{Recognition, SourceFile};
use aye_pytest::doctest::{self, DoctestFile};

use super::ignore::Ignored;
use crate::{PytestRecognizer, SETS};

/// Why pytest's doctest plugin collects doctests in a run, for messages.
pub(crate) const PYTEST_DOCTESTS: &str = "the doctest adapter isn't among the run's adapters";

impl PytestRecognizer {
    /// This recognizer for a run without the doctest adapter: pytest's own
    /// doctest plugin would collect doctests, as without aye. Its options
    /// reach the front-end ([`crate::PytestConfig::keep_doctest_options`]), and
    /// the files it collects are claimed (and refused by the adapter's
    /// recognizer): with `--doctest-modules`
    /// every `.py` file but `__main__.py` and a `setup.py` mentioning
    /// setuptools or distutils, and the text files matching
    /// `--doctest-glob` (`test*.txt` without one) or named on the command
    /// line as `.txt` or `.rst`, unless ignored. Unless `-p no:doctest`
    /// blocks the plugin, which then collects nothing.
    #[must_use]
    pub fn with_pytest_doctests(mut self) -> Self {
        self.config = self.config.keep_doctest_options();
        let blocked = self
            .config
            .plugins
            .iter()
            .any(|p| p.strip_prefix("no:").is_some_and(|n| n.trim() == "doctest"));
        if !blocked {
            self.doctests = Some(self.config.doctest.clone());
        }
        self
    }

    /// What pytest's doctest plugin collects from `rel`, when it collects
    /// doctests in this run (`_pytest/doctest.py`'s `pytest_collect_file`).
    pub(crate) fn doctest_file(&self, rel: &str, explicit: bool) -> Option<DoctestFile> {
        let doctests = self.doctests.as_ref()?;
        let name = rel.rsplit('/').next().unwrap_or(rel);
        let abs = self.root.join(rel);
        // pytest's `--doctest-glob` without one.
        let default = ["test*.txt".to_string()];
        let globs = if doctests.globs.is_empty() {
            &default[..]
        } else {
            &doctests.globs[..]
        };
        doctest::collects(name, &abs, explicit, doctests.modules, globs, SETS, || {
            self.ignored(&abs) == Ignored::Yes
        })
    }
}

/// A text file pytest's doctest plugin collects: one test when it holds
/// examples, which only pytest's parser (with `doctest_encoding`) can tell
/// for sure; none when, read as UTF-8, it holds no `>>>` prompt.
pub(crate) fn doctest_text(file: &SourceFile<'_>) -> Recognition {
    if !file.source.contains(">>>") && !file.source.contains('\u{FFFD}') {
        return Recognition::default();
    }
    Recognition {
        refine: Some(
            format!("pytest's doctest plugin collects this text file ({PYTEST_DOCTESTS})").into(),
        ),
        ..Recognition::default()
    }
}

/// Whether a test module may hold doctests: a `>>>` prompt in its text, a
/// `__test__` table (which may hold strings from elsewhere) or a
/// `__doc__` set at runtime. pytest's doctest plugin collects them beside
/// its tests.
pub(crate) fn may_hold_doctests(source: &str) -> bool {
    [">>>", "__test__", "__doc__"]
        .iter()
        .any(|needle| source.contains(needle))
}
