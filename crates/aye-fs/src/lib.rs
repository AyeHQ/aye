//! Paths, handled lexically: made absolute and normalized as Python's
//! `os.path` does, made relative, shown with `/`, and expanded.
//!
//! Decided (implementation, October 2026):
//! - **Paths are made absolute lexically**, as Python's `os.path.abspath`
//!   does and pytest relies on: joined to the current directory, with `.`
//!   and `..` folded away ([`absolute`]), never resolved through symlinks, so
//!   a symlinked test directory keeps its name. The filesystem is consulted
//!   only to reconcile two spellings of one directory (macOS's `/tmp` is
//!   `/private/tmp`, which is what the current directory reads as): when the
//!   given directories share nothing lexically but do once resolved, their
//!   common ancestor is the resolved one, and [`relative_to`] falls back to
//!   resolved paths the same way.
//! - **Two normalizers, each named for its rule.** [`normpath`] is Python's
//!   `os.path.normpath`; [`normalize_absolute`] lets every `..` remove the
//!   component before it. They agree on absolute paths, the only ones aye
//!   gives them, and each caller keeps the one it had.
//! - **Two relativizers**, which disagree on a path that shares only the
//!   filesystem's root with the base: collection's ids climb out through
//!   it ([`relative_through_root`]), the binary's selectors keep such a
//!   path absolute ([`relative_below_root`]).
//!
//! Owns: path algebra over `std::path`, paths shown with `/`, the expansion
//! of variables and `~` over a lookup it is given. Does not own: the
//! project root (aye-config), nor the environment (aye-static).
//!
//! Depends on: the standard library only. Must not depend on any aye crate.

mod path;

pub use path::{
    absolute, common_ancestor, expand_path, normalize_absolute, normpath, relative_below_root,
    relative_through_root, relative_to, root_relative, slashed,
};
