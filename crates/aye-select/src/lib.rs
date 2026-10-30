//! Selection: filtering collected cases by explicit ids, a keyword
//! expression and a tag expression, with three-valued logic so that anything
//! static analysis cannot decide is kept and left to the engine to re-apply,
//! and putting what is kept in the order of the explicit ids.
//!
//! Owns: the selection language and its evaluation. Universal across engines.
//!
//! Depends on: aye-events.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **One verdict per case**, a [`Truth`]: the explicit selectors (any of
//!   them), the keyword expression and the tag expression, joined with
//!   Kleene's `and`; last failed then filters what is left. A case is
//!   dropped only on `False`. The engine re-applies both expressions at
//!   runtime ([`Expression::as_str`] is the text to hand it), so `Unknown`
//!   costs a little planning, never a test.
//! - **Positional arguments** ([`Selector`]) are relative to the project
//!   root, with `/` separators; the caller makes them so, since selection
//!   never touches the file system. One containing `::` is a canonical id
//!   (`tests/t.py::TestA::test_b[x]`), selecting every case it prefixes by
//!   whole names; an id without a parameter id selects all of them. One
//!   containing `/` is a file or directory, selecting by whole path
//!   components. Any other word is read every way it can be, and selects
//!   what any reading selects: a top-level file or directory (`tests`), a
//!   dotted name in unittest's style (`pkg.mod.Class.test` is
//!   `pkg/mod.py::Class::test`, `pkg` also `pkg.py`, `__init__` dropped),
//!   and, for 4 to 16 hex digits, a prefix of a [`TestKey`]. Dotted names are
//!   read from the root, except that a Python file's case whose first name
//!   is itself dotted (a doctest's, `pkg/mod.py::pkg.mod.func`, which names
//!   the module as it is imported, `src/` layouts included) is also
//!   selected by that name (`pkg.mod.func`, `pkg.mod`); otherwise a `src/`
//!   layout is selected by path. Callers that know an argument is a path
//!   on disk build it with [`Selector::path`].
//! - **Parameters known only at runtime.** `path::test[x]` keeps a
//!   `Partial` case `path::test` as `Unknown` and narrows it: the case's
//!   entry in [`Selected::narrowed`] lists the parameter ids to hand the
//!   engine in its place, so only `[x]` runs. A key selects the case whose
//!   own id has it; keys of parameter ids that collection cannot know are
//!   for the caller to resolve (from history) into ids.
//! - **Selectors that match no case** are listed in [`Selected::unmatched`],
//!   so the runner can fail like pytest's `not found:`. Like pytest's
//!   `normalize_collection_arguments`, an argument another one subsumes (a
//!   path below a directory argument, an id below a file or a shorter id, a
//!   repeat) is never listed.
//! - **Order follows the arguments**, as pytest collects them: each kept case
//!   is placed at the most general argument that selects it (the first among
//!   equals), and keeps the order it came in within one argument. A
//!   `Partial` case's narrowed parameter ids stay together, in the order the
//!   arguments name them.
//! - **Deselected** counts what the expressions, `--deselect` and last
//!   failed drop from what the positional arguments select, as pytest's
//!   summary does; cases no argument selects were never collected as far as
//!   pytest is concerned. Under last failed, only what pytest's last-failed
//!   plugin lets its collection keep is counted (`Visible`): once
//!   collection meets a file holding a module-level test that failed
//!   (files on a failed path come first), files that held no failure are
//!   skipped whole, and in a failing file the module-level tests that
//!   didn't fail are dropped unless a positional argument names the file;
//!   tests inside classes are kept, and counted. A file pytest's sort puts
//!   between failing files in other directories before that point is
//!   counted as skipped all the same (an approximation).
//! - **An adapter's own expressions and `--deselect`.** The command line's
//!   `-k` and `-m` apply to every case; when it gives none, each adapter's
//!   own (pytest's from its `addopts`) apply to that adapter's cases
//!   ([`Selection::own_keyword`], [`Selection::own_tags`]). The caller may
//!   also give some adapters an expression and others none (the binary,
//!   when the framework's arguments can't be read, leaves pytest's cases to
//!   pytest and gives the others the command line's). `--deselect`
//!   prefixes ([`Selection::deselect`]) drop the adapter's cases whose ids
//!   start with one, as strings, as pytest matches node ids (`test_x` also
//!   drops `test_x2`), but never a `Partial` case, whose id may not be the
//!   framework's: the framework deselects that one itself.
//! - **Expressions are pytest 9's**, errors and columns included, with
//!   strict `name=value` arguments on tag identifiers. The keyword
//!   expression ignores leading whitespace and turns off when empty; the tag
//!   expression turns off only when it is the empty string, and one of only
//!   spaces matches nothing. A keyword expression with call arguments is an
//!   error when it is built, where pytest raises it only if evaluation
//!   reaches the call.
//! - **Keywords** match case-insensitively as substrings of
//!   [`Case::keywords`] or, when the adapter gives none, of the case's path
//!   components, names (the last with its parameter id) and tags, the names
//!   pytest matches. **Tags** match exactly; a tag that is present but asked
//!   for with arguments (`device(serial="1")`) is `Unknown`, since cases do
//!   not carry tag arguments. A miss is `Unknown` instead of `False` for
//!   `Partial` cases, whose parameter ids and tags are not all known.
//! - **Last failed** filters after everything else, as pytest's does. A case
//!   is kept when its id failed, when a file or directory containing it
//!   failed (a collection error), or when it is `Partial` and some of its
//!   parameter ids failed, which narrows it to those. When no remaining
//!   case failed, or nothing failed at all, every remaining case is kept
//!   (pytest's default, `--lfnf=all`).
//!
//! [`TestKey`]: aye_events::TestKey
//! [`Case::keywords`]: aye_events::Case::keywords

mod expression;
mod keyword;
mod last_failed;
mod marker;
mod order;
mod selection;
mod selector;
mod truth;

pub use expression::{Expression, Kwarg, ParseError, Value};
pub use order::failed_first;
pub use selection::{Selected, Selection};
pub use selector::{Named, Selector, dotted_reach};
pub use truth::Truth;

/// A malformed `-k` or `-m` expression: a usage error. The parse error is
/// part of the message and not a [`std::error::Error::source`], so printing
/// the error with its chain shows it once, as pytest does.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectError {
    #[error("wrong expression passed to '-k': {expr}: {error}")]
    Keyword { expr: String, error: ParseError },
    #[error("wrong expression passed to '-m': {expr}: {error}")]
    Tags { expr: String, error: ParseError },
}

#[cfg(test)]
mod tests;
