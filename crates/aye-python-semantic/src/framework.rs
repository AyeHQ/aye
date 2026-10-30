//! What the framework that collects a module does with its code, which
//! Python alone doesn't: the calls whose arguments it keeps, the functions
//! it calls itself, the calls that skip the module. The analysis knows no
//! framework; each adapter names its own.

/// A framework's ways with a module, as the analysis reads them.
/// [`Framework::NONE`] is Python alone.
#[derive(Debug, Clone, Copy)]
pub struct Framework {
    /// Its packages (`pytest`, `_pytest`): the analysis resolves calls
    /// through them, for [`Framework::marks`] and
    /// [`Framework::inert_calls`].
    pub packages: &'static [&'static str],
    /// Where its marks are (`pytest.mark.`): a mark keeps its arguments,
    /// never changes them, never changes itself, and hands back the
    /// function it decorates, name and all.
    pub marks: Option<&'static str>,
    /// Other calls that keep their arguments and never change them
    /// (`pytest.param`, `pytest.fixture`).
    pub inert_calls: &'static [&'static str],
    /// Prefixes of the decorators it knows neither call the function they
    /// get nor rename it (`pytest.`, `_pytest.`), besides Python's own.
    pub harmless_decorators: &'static [&'static str],
    /// The prefix of the module-level functions it calls itself while it
    /// collects, its hooks (`pytest_`).
    pub hooks: Option<&'static str>,
    /// Calls that skip the whole module when they run while it is imported
    /// (`pytest.skip`, `pytest.importorskip`).
    pub skips: &'static [&'static str],
}

impl Framework {
    /// No framework: only Python calls into the module.
    pub const NONE: Self = Self {
        packages: &[],
        marks: None,
        inert_calls: &[],
        harmless_decorators: &[],
        hooks: None,
        skips: &[],
    };

    /// Whether `path` names one of its marks.
    pub(crate) fn is_mark(&self, path: &str) -> bool {
        self.marks.is_some_and(|marks| path.starts_with(marks))
    }

    /// Whether a call to `path` keeps its arguments and never changes them.
    pub(crate) fn is_inert(&self, path: &str) -> bool {
        self.is_mark(path) || self.inert_calls.contains(&path)
    }

    /// Whether it calls a module-level function of this name itself.
    pub(crate) fn is_hook(&self, name: &str) -> bool {
        self.hooks.is_some_and(|hooks| name.starts_with(hooks))
    }
}

impl Default for Framework {
    fn default() -> Self {
        Self::NONE
    }
}
