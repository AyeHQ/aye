//! The plugins of pytest's session: pytest's own, and what implements a
//! hook, as the refinement reasons say it.

/// pytest's `builtin_plugins`, and the plugins merged into pytest that it
/// refuses to register (`DEPRECATED_EXTERNAL_PLUGINS`): `-p NAME` for these
/// loads nothing aye doesn't model.
pub(crate) const BUILTIN_PLUGINS: [&str; 37] = [
    "mark",
    "main",
    "runner",
    "fixtures",
    "helpconfig",
    "python",
    "terminal",
    "debugging",
    "unittest",
    "capture",
    "skipping",
    "legacypath",
    "tmpdir",
    "monkeypatch",
    "recwarn",
    "pastebin",
    "assertion",
    "junitxml",
    "doctest",
    "cacheprovider",
    "setuponly",
    "setupplan",
    "stepwise",
    "unraisableexception",
    "threadexception",
    "warnings",
    "logging",
    "reports",
    "faulthandler",
    "subtests",
    "pytester",
    "pytester_assertions",
    "terminalprogress",
    "pytest_catchlog",
    "pytest_capturelog",
    "pytest_faulthandler",
    "pytest_subtests",
];

/// What implements a hook, in refinement reasons.
pub(crate) const PLUGIN_P: &str = "a -p plugin";
pub(crate) const PLUGIN_ENV: &str = "a PYTEST_PLUGINS plugin";
pub(crate) const CONFTEST: &str = "a conftest.py";
pub(crate) const PLUGIN_OF_CONFTEST: &str = "a plugin a conftest.py loads";
pub(crate) const PLUGIN_OF_MODULE: &str = "a plugin a test module loads";
pub(crate) const SCOPED_CONFTEST: &str = "a conftest.py in scope";
pub(crate) const PLUGIN_OF_SCOPED_CONFTEST: &str = "a plugin a conftest.py in scope loads";
