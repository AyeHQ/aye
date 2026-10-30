//! The hooks the recognizer reasons about: those that decide which tests
//! a file below their conftest has, those pytest calls for the whole
//! session, those that only mark; and whether a module's
//! `pytest_collection_modifyitems` makes items.

use std::collections::HashSet;

use aye_python_semantic::namespace::{Binding, Bound, Namespace};

/// Hooks pytest calls for the files below their conftest only, that decide
/// which tests exist or what they are called: a file in their scope can
/// only be known by importing it.
pub(crate) const REFINING_HOOKS: [&str; 4] = [
    "pytest_pycollect_makeitem",
    "pytest_pycollect_makemodule",
    "pytest_ignore_collect",
    "pytest_collect_directory",
];

/// Whether a module's `pytest_collection_modifyitems` (`hook`) may make
/// items, and so add tests: `from_parent` in its body, or in a function or
/// class of the module it names, followed further; anywhere in the module
/// when the hook isn't a plain function. Other ways (`copy.copy(item)`, a
/// function of another module) are not seen.
pub(super) fn makes_items(source: &str, ns: &Namespace<'_>, hook: &Bound<'_>) -> bool {
    const MAKER: &str = "from_parent";
    if !source.contains(MAKER) {
        return false;
    }
    if !matches!(hook.binding, Binding::Function(_)) {
        return true;
    }
    let names_in = |text: &str, name: &str| {
        text.match_indices(name).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + name.len()..].chars().next();
            let part = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            !part(before) && !part(after)
        })
    };
    let mut seen: HashSet<&str> = HashSet::new();
    let mut pending = vec![hook.range];
    while let Some(range) = pending.pop() {
        let text = &source[range];
        if text.contains(MAKER) {
            return true;
        }
        for (name, bound) in ns.iter() {
            if matches!(bound.binding, Binding::Function(_) | Binding::Class(_))
                && !seen.contains(name)
                && names_in(text, name)
            {
                seen.insert(name);
                pending.push(bound.range);
            }
        }
    }
    false
}

/// Hooks pytest calls for every test of the session, wherever their
/// conftest is: `pytest_collection` replaces collection,
/// `pytest_make_parametrize_id` makes ids, `pytest_collection_modifyitems`
/// may add marks, deselect or (when its module makes items) add tests.
pub(crate) const GLOBAL_HOOKS: [&str; 3] = [
    "pytest_collection",
    "pytest_make_parametrize_id",
    "pytest_collection_modifyitems",
];

/// Hooks that may add marks to collected tests: ids stay, tags become
/// uncertain. `pytest_itemcollected` is called below its conftest only.
pub(crate) const MARKING_HOOKS: [&str; 2] =
    ["pytest_collection_modifyitems", "pytest_itemcollected"];
