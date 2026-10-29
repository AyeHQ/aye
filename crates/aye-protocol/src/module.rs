//! The Python modules a worker installs, and the order it installs them in.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

/// A Python module a worker installs into `sys.modules`: its import name
/// and its source. A module embedded in the binary borrows both
/// ([`ModuleSource::embedded`]), so building a handshake copies no source;
/// one read from the wire owns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleSource {
    /// Import name, e.g. `aye_engine`.
    pub name: Cow<'static, str>,
    pub source: Cow<'static, str>,
}

impl ModuleSource {
    /// The module `name`, whose source is embedded in the binary.
    pub const fn embedded(name: &'static str, source: &'static str) -> Self {
        Self {
            name: Cow::Borrowed(name),
            source: Cow::Borrowed(source),
        }
    }
}

/// Embedded modules in install order, owned by the crate that embeds them.
/// Each imports, when it is installed, only the standard library and the
/// modules installed before it: those before it in its set, and those of
/// the sets a worker installs first (aye's engine, then the prelude the
/// binary gives every worker, then each runtime's own set).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModuleSet(&'static [ModuleSource]);

impl ModuleSet {
    /// The set of `modules`, in install order. A constant writes the array
    /// in a `const` block, `ModuleSet::new(const { &[A, B] })`, since a
    /// constant can't drop the array of [`ModuleSource`]s it would
    /// otherwise build.
    pub const fn new(modules: &'static [ModuleSource]) -> Self {
        Self(modules)
    }

    pub const fn modules(self) -> &'static [ModuleSource] {
        self.0
    }

    pub fn iter(self) -> std::slice::Iter<'static, ModuleSource> {
        self.0.iter()
    }

    /// The modules' import names, in install order.
    pub fn names(self) -> impl Iterator<Item = &'static str> {
        self.0.iter().map(|module| &*module.name)
    }
}

impl IntoIterator for ModuleSet {
    type Item = &'static ModuleSource;
    type IntoIter = std::slice::Iter<'static, ModuleSource>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}
