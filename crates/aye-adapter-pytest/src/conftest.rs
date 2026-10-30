//! What `conftest.py` files (and the plugin modules they name in
//! `pytest_plugins`, register or import from) do to collection: hooks,
//! fixtures, `collect_ignore`, the modules they change and whether they
//! reach module namespaces; and what project modules define, for telling
//! whether a name a test module imports may be a unittest case.

mod fixtures;
mod hooks;
mod index;
mod ini_changes;
mod summary;

pub(crate) use fixtures::{
    BUILTIN_FIXTURES, FixtureDef, default_argnames, fixture_of, funcargnames,
};
pub(crate) use hooks::{GLOBAL_HOOKS, MARKING_HOOKS, REFINING_HOOKS};
pub(crate) use index::Supports;
pub(crate) use summary::{IgnoreList, Summary, requested_names};
