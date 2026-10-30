//! pytest's own doctest plugin, in a run without the doctest adapter:
//! what it collects, and what the recognizer makes of it.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **pytest's own doctests, without the doctest adapter** (finding F21).
//!   [`Pytest::prepare_among`] tells whether the run uses the doctest
//!   adapter; [`Pytest::prepare`] assumes it does. When it doesn't
//!   (`[tool.aye] adapters = ["pytest"]`), pytest's doctest plugin would
//!   collect doctests ([`PytestRecognizer::with_pytest_doctests`]), unless
//!   `-p no:doctest` blocks it: the front-end gets the doctest options as
//!   given, and the files the plugin collects (`_pytest/doctest.py`'s
//!   `pytest_collect_file`) are claimed as test files, which the pytest
//!   front-end refuses with a `usage-error` naming the fix (add the
//!   doctest adapter): aye's engine doesn't run pytest's own doctests
//!   ([`Pytest`]), and a test is never silently dropped. With
//!   `--doctest-modules`, every `.py` file but `__main__.py` and a
//!   `setup.py` mentioning setuptools or distutils; text files matching
//!   `--doctest-glob` (pytest appends them across addopts,
//!   `PYTEST_ADDOPTS` and the arguments after `--`), else `test*.txt`,
//!   which pytest collects even without any doctest option, and a `.txt`
//!   or `.rst` file named on the command line; `--ignore` and the other
//!   ignore rules hold, but not for named files. Their doctests are
//!   refined, never guessed (pytest's parser, `doctest_encoding` and the
//!   module's import decide): a module only the doctest plugin collects
//!   (`m.py`, not matching `python_files`) gives no static case, a test
//!   module whose text holds `>>>`, `__test__` or `__doc__` is refined
//!   with its static cases `Partial` (others keep theirs: a docstring with
//!   examples holds a prompt), a text file is refined unless, read as
//!   UTF-8, it holds no `>>>` (then it has no test). `aye config` shows a
//!   `doctests` setting saying who would collect them.
//!   Known gap: doctests of a test module whose text holds none of those
//!   (a docstring built from escapes).
//!
//! [`Pytest`]: crate::Pytest
//! [`Pytest::prepare`]: aye_adapter::Adapter::prepare
//! [`Pytest::prepare_among`]: aye_adapter::Adapter::prepare_among

use aye_adapter::{Recognition, SourceFile};
use aye_pytest::config::DoctestOptions;
use aye_pytest::doctest::{self, DoctestFile};

use super::ignore::Ignored;
use crate::{PytestRecognizer, SETS};

/// Why pytest's doctest plugin collects doctests in a run, for messages.
pub(crate) const PYTEST_DOCTESTS: &str = "the doctest adapter isn't among the run's adapters";

/// Who collects the doctests of a run the pytest adapter is in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum PytestDoctests {
    /// The doctest adapter, which runs beside this one.
    #[default]
    ByDoctestAdapter,
    /// pytest's own doctest plugin, as without aye, by these options: no
    /// doctest adapter runs.
    ByPytestPlugin(DoctestOptions),
    /// Nobody: no doctest adapter runs, and `-p no:doctest` blocks
    /// pytest's plugin.
    Blocked,
}

impl PytestDoctests {
    /// What pytest's doctest plugin collects by, when it collects the run's
    /// doctests.
    pub(crate) fn by_pytest(&self) -> Option<&DoctestOptions> {
        match self {
            Self::ByPytestPlugin(options) => Some(options),
            Self::ByDoctestAdapter | Self::Blocked => None,
        }
    }
}

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
        self.doctests = if blocked {
            PytestDoctests::Blocked
        } else {
            PytestDoctests::ByPytestPlugin(self.config.doctest.clone())
        };
        self
    }

    /// What pytest's doctest plugin collects from `rel`, when it collects
    /// doctests in this run (`_pytest/doctest.py`'s `pytest_collect_file`).
    pub(crate) fn doctest_file(&self, rel: &str, explicit: bool) -> Option<DoctestFile> {
        let doctests = self.doctests.by_pytest()?;
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
