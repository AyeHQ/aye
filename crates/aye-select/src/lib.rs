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

mod expression;
mod selector;

use std::collections::{BTreeMap, HashMap, HashSet};

use aye_events::{AdapterName, Case, Certainty, IdPrefix, TestId};

pub use expression::{Expression, Kwarg, ParseError, Truth, Value};
pub use selector::Selector;

use selector::Parts;

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

/// What to run, out of what was collected. The default selects everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Positional arguments; a case is kept when any of them selects it.
    /// Empty selects every case.
    pub ids: Vec<Selector>,
    /// `-k`.
    pub keyword: Option<Expression>,
    /// `-m`.
    pub tags: Option<Expression>,
    /// The ids that failed last time, when running last-failed (`--lf`).
    pub last_failed: Option<HashSet<TestId>>,
    /// Id prefixes each adapter drops (pytest's `--deselect`), by adapter.
    pub deselect: BTreeMap<AdapterName, Vec<IdPrefix>>,
    /// Each adapter's own `-k` (or one the caller chose for its cases),
    /// for its cases when `keyword` is unset.
    pub own_keyword: BTreeMap<AdapterName, Expression>,
    /// Each adapter's own `-m` (or one the caller chose for its cases), for
    /// its cases when `tags` is unset.
    pub own_tags: BTreeMap<AdapterName, Expression>,
}

/// The outcome of [`Selection::apply`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selected {
    /// The kept cases: in the order of the positional arguments that select
    /// them, and in the order they came for one argument (or when there are
    /// none).
    pub cases: Vec<Case>,
    /// How many cases the positional arguments select (all of them, when
    /// there are none) that the expressions, `--deselect` or last failed
    /// then dropped, as pytest counts "deselected". Cases no positional
    /// argument selects are not counted: pytest never collects them; nor,
    /// under last failed, those its last-failed plugin drops while
    /// collecting (see the crate docs).
    pub deselected: u32,
    /// Kept `Partial` cases wanted only for some of their parameter ids (by
    /// `path::test[x]`, or because only those failed last time): the ids to
    /// hand the engine in place of the case's own, in the order of the
    /// arguments that asked for them.
    pub narrowed: BTreeMap<TestId, Vec<TestId>>,
    /// Positional arguments that matched no case, as given. An argument
    /// another one subsumes (`tests/t.py` beside `tests`, or a repeat) is
    /// never listed, as pytest drops it before collecting.
    pub unmatched: Vec<String>,
}

impl Selected {
    /// The ids to hand the engine for a kept case.
    pub fn ids_for(&self, case: &Case) -> Vec<TestId> {
        self.narrowed
            .get(&case.id)
            .cloned()
            .unwrap_or_else(|| vec![case.id.clone()])
    }
}

impl Selection {
    /// Add positional arguments, each read with [`Selector::parse`].
    #[must_use]
    pub fn with_ids<I>(mut self, ids: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        self.ids
            .extend(ids.into_iter().map(|id| Selector::parse(id.as_ref())));
        self
    }

    /// Set the keyword expression (`-k`).
    pub fn with_keyword(mut self, expr: &str) -> Result<Self, SelectError> {
        self.keyword = parse_keyword(expr)?;
        Ok(self)
    }

    /// Set the tag expression (`-m`).
    pub fn with_tags(mut self, expr: &str) -> Result<Self, SelectError> {
        self.tags = parse_tags(expr)?;
        Ok(self)
    }

    /// Set `adapter`'s own keyword expression (pytest's `-k` in its
    /// `addopts`), for its cases when [`Selection::keyword`] is unset.
    pub fn with_own_keyword(
        mut self,
        adapter: impl Into<AdapterName>,
        expr: &str,
    ) -> Result<Self, SelectError> {
        if let Some(parsed) = parse_keyword(expr)? {
            self.own_keyword.insert(adapter.into(), parsed);
        }
        Ok(self)
    }

    /// Set `adapter`'s own tag expression (pytest's `-m` in its `addopts`),
    /// for its cases when [`Selection::tags`] is unset.
    pub fn with_own_tags(
        mut self,
        adapter: impl Into<AdapterName>,
        expr: &str,
    ) -> Result<Self, SelectError> {
        if let Some(parsed) = parse_tags(expr)? {
            self.own_tags.insert(adapter.into(), parsed);
        }
        Ok(self)
    }

    /// Drop the cases of `adapter` whose ids start with one of `prefixes`
    /// (pytest's `--deselect`, from its options), counting them as
    /// deselected.
    #[must_use]
    pub fn with_deselect<I>(mut self, adapter: impl Into<AdapterName>, prefixes: I) -> Self
    where
        I: IntoIterator,
        I::Item: Into<IdPrefix>,
    {
        let prefixes: Vec<IdPrefix> = prefixes
            .into_iter()
            .map(Into::into)
            .filter(|prefix: &IdPrefix| !prefix.is_empty())
            .collect();
        if !prefixes.is_empty() {
            self.deselect
                .entry(adapter.into())
                .or_default()
                .extend(prefixes);
        }
        self
    }

    /// Run last-failed with the ids that failed last time.
    #[must_use]
    pub fn with_last_failed(mut self, failed: impl IntoIterator<Item = TestId>) -> Self {
        self.last_failed = Some(failed.into_iter().collect());
        self
    }

    /// Whether this selects every case, whatever was collected.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
            && self.keyword.is_none()
            && self.tags.is_none()
            && self.last_failed.is_none()
            && self.deselect.is_empty()
            && self.own_keyword.is_empty()
            && self.own_tags.is_empty()
    }

    /// The verdict on one case from the selectors and both expressions.
    /// Last failed is left out: it depends on every other case.
    pub fn truth(&self, case: &Case) -> Truth {
        let mut matched = vec![false; self.ids.len()];
        self.judge(&Parts::new(case), &mut matched).truth
    }

    /// Keep the cases whose verdict is not `False`.
    pub fn apply(&self, cases: Vec<Case>) -> Selected {
        let mut matched = vec![false; self.ids.len()];
        let failed = self
            .last_failed
            .as_ref()
            .filter(|f| !f.is_empty())
            .map(Failed::new);
        // Every case the positional arguments select, in the order they
        // came; the kept ones carry their place in it.
        let mut within: Vec<Seen> = Vec::new();
        // Files with a module-level test that failed last time, which turns
        // on pytest's last-failed filtering of collection (see `Visible`).
        let mut direct: HashSet<String> = HashSet::new();
        let mut kept = Vec::with_capacity(cases.len());
        for case in cases {
            let parts = Parts::new(&case);
            let verdict = self.judge(&parts, &mut matched);
            let top = parts.names.len() <= 1;
            let has_failed = failed.as_ref().is_some_and(|f| f.names(&case));
            if top && has_failed {
                direct.insert(parts.path.to_string());
            }
            if verdict.ids.is_false() {
                continue;
            }
            let at = within.len();
            within.push(Seen {
                path: parts.path.to_string(),
                top,
                failed: has_failed,
                kept: false,
            });
            if !verdict.truth.is_false() {
                kept.push((verdict.owner, at, case, verdict.narrow));
            }
        }
        // Argument order, then the order they came: stable.
        kept.sort_by_key(|(owner, _, _, _)| *owner);
        let mut kept: Vec<(usize, Case, Vec<TestId>)> = kept
            .into_iter()
            .map(|(_, at, case, narrow)| (at, case, narrow))
            .collect();

        if let Some(failed) = &failed {
            let narrows: Vec<_> = kept
                .iter()
                .map(|(_, case, narrow)| failed.judge(case, narrow))
                .collect();
            if narrows.iter().any(Option::is_some) {
                kept = kept
                    .into_iter()
                    .zip(narrows)
                    .filter_map(|((at, case, _), narrow)| Some((at, case, narrow?)))
                    .collect();
            }
        }
        for (at, _, _) in &kept {
            within[*at].kept = true;
        }
        let visible = failed.as_ref().map(|failed| {
            Visible::new(failed, &within, &direct, |path| {
                self.ids.iter().any(|selector| selector.names_file(path))
            })
        });
        let deselected = within
            .iter()
            .filter(|seen| !seen.kept)
            .filter(|seen| visible.as_ref().is_none_or(|v| v.sees(seen)))
            .count();

        let mut selected = Selected {
            deselected: u32::try_from(deselected).unwrap_or(u32::MAX),
            unmatched: self
                .ids
                .iter()
                .enumerate()
                .filter(|&(at, selector)| !matched[at] && !self.subsumed(at, selector))
                .map(|(_, selector)| selector.as_str().to_string())
                .collect(),
            ..Selected::default()
        };
        for (_, case, narrow) in kept {
            if !narrow.is_empty() {
                selected.narrowed.insert(case.id.clone(), narrow);
            }
            selected.cases.push(case);
        }
        selected
    }

    /// The verdict on one case. Marks the selectors that matched.
    fn judge(&self, case: &Parts<'_>, matched: &mut [bool]) -> Verdict {
        let mut narrow = Vec::new();
        let mut owner = (usize::MAX, 0);
        let ids = if self.ids.is_empty() {
            Truth::True
        } else {
            let mut truth = Truth::False;
            for (at, (selector, matched)) in self.ids.iter().zip(matched).enumerate() {
                let (found, rank) = selector.select_ranked(case, &mut narrow);
                if !found.is_false() {
                    *matched = true;
                    owner = owner.min((rank, at));
                }
                truth = truth | found;
            }
            truth
        };
        if ids == Truth::True {
            narrow.clear();
        }
        let mut truth = ids;
        if !truth.is_false() && self.deselects(case.case) {
            truth = Truth::False;
        }
        let adapter = &case.case.adapter;
        if let Some(expr) = self
            .keyword
            .as_ref()
            .or_else(|| self.own_keyword.get(adapter))
            && !truth.is_false()
        {
            truth = truth & keyword_truth(expr, case);
        }
        if let Some(expr) = self.tags.as_ref().or_else(|| self.own_tags.get(adapter))
            && !truth.is_false()
        {
            truth = truth & tags_truth(expr, case.case);
        }
        let mut seen = HashSet::new();
        narrow.retain(|id| seen.insert(id.clone()));
        Verdict {
            ids,
            truth,
            narrow,
            owner: owner.1,
        }
    }

    /// Whether a `--deselect` prefix of the case's adapter drops it: its id
    /// starts with one, as a string, as pytest's `--deselect` matches node
    /// ids. A `Partial` case's id may not be the framework's, so the
    /// framework deselects it itself.
    fn deselects(&self, case: &Case) -> bool {
        case.certainty != Certainty::Partial
            && self
                .deselect
                .get(&case.adapter)
                .is_some_and(|prefixes| prefixes.iter().any(|prefix| prefix.is_prefix_of(&case.id)))
    }

    /// Whether another positional argument subsumes the one at `at` (an
    /// equal one only when it comes first).
    fn subsumed(&self, at: usize, selector: &Selector) -> bool {
        self.ids.iter().enumerate().any(|(by, other)| {
            by != at && other.subsumes(selector) && (by < at || !selector.subsumes(other))
        })
    }
}

/// What [`Selection::judge`] says about one case.
struct Verdict {
    /// The positional arguments' verdict alone.
    ids: Truth,
    /// Theirs and both expressions'.
    truth: Truth,
    /// When the case is wanted only for some parameter ids, those ids, in
    /// the order of the arguments that ask for them.
    narrow: Vec<TestId>,
    /// The argument the case is collected for: the most general one that
    /// selects it, the first among equals (0 when there are none).
    owner: usize,
}

/// What a name the case lacks means: `False`, unless the case's parameter
/// ids or tags are not all known.
fn miss(case: &Case) -> Truth {
    if case.certainty == Certainty::Partial {
        Truth::Unknown
    } else {
        Truth::False
    }
}

fn keyword_truth(expr: &Expression, case: &Parts<'_>) -> Truth {
    let names = keyword_names(case);
    let miss = miss(case.case);
    expr.evaluate(|ident, _| {
        let ident = ident.to_lowercase();
        if names.iter().any(|name| name.contains(&ident)) {
            Truth::True
        } else {
            miss
        }
    })
}

/// The names a keyword expression matches, lowercase.
fn keyword_names(case: &Parts<'_>) -> Vec<String> {
    if !case.case.keywords.is_empty() {
        return case
            .case
            .keywords
            .iter()
            .map(|k| k.to_lowercase())
            .collect();
    }
    let mut names: Vec<String> = case.components.iter().map(|c| c.to_lowercase()).collect();
    if let Some((last, outer)) = case.names.split_last() {
        names.extend(outer.iter().map(|name| name.to_lowercase()));
        names.push(match case.params {
            Some(params) => format!("{last}[{params}]").to_lowercase(),
            None => last.to_lowercase(),
        });
    }
    names.extend(case.case.tags.iter().map(|tag| tag.to_lowercase()));
    names
}

fn tags_truth(expr: &Expression, case: &Case) -> Truth {
    let miss = miss(case);
    expr.evaluate(|ident, kwargs| {
        if !case.tags.iter().any(|tag| tag == ident) {
            miss
        } else if kwargs.is_empty() {
            Truth::True
        } else {
            Truth::Unknown
        }
    })
}

/// A keyword expression, `None` when it turns selection off (empty or
/// blank).
fn parse_keyword(expr: &str) -> Result<Option<Expression>, SelectError> {
    let expr = expr.trim_start();
    let error = |error| SelectError::Keyword {
        expr: expr.to_string(),
        error,
    };
    if expr.is_empty() {
        return Ok(None);
    }
    let parsed = Expression::parse(expr).map_err(error)?;
    if let Some(column) = parsed.first_call() {
        return Err(error(ParseError {
            column,
            message: "keyword expressions do not support call parameters".to_string(),
        }));
    }
    Ok(Some(parsed))
}

/// A tag expression, `None` when it turns selection off (empty).
fn parse_tags(expr: &str) -> Result<Option<Expression>, SelectError> {
    if expr.is_empty() {
        return Ok(None);
    }
    Expression::parse(expr)
        .map(Some)
        .map_err(|error| SelectError::Tags {
            expr: expr.to_string(),
            error,
        })
}

/// What [`Selection::apply`] remembers of a case the positional arguments
/// select.
struct Seen {
    /// Its file.
    path: String,
    /// Whether it is a module-level test (a single name after the path),
    /// rather than one inside a class.
    top: bool,
    /// Whether it failed last time.
    failed: bool,
    /// Whether it is kept in the end.
    kept: bool,
}

/// Which cases pytest's summary can count as deselected under `--lf`:
/// pytest's last-failed plugin drops most of what it doesn't rerun while
/// collecting, before anything is deselected (`_pytest/cacheprovider.py`).
/// Once collection reaches a file holding a module-level test that failed
/// (files on a failed path come first), files that held no failure are
/// skipped whole, and in each file that held one, module-level tests that
/// didn't fail are dropped unless the file was named on the command line;
/// tests inside classes stay. Only what is left reaches `-k`, `-m` and the
/// plugin's own deselection, and is counted.
struct Visible {
    /// Files on a failed path whose collection is filtered.
    filtered: HashSet<String>,
    /// Files on a failed path.
    failing: HashSet<String>,
    /// Whether any file was filtered, so files without failures are skipped
    /// (they come after the failing ones).
    skipping: bool,
    /// Files a positional argument names (pytest's "initial paths").
    named: HashSet<String>,
}

impl Visible {
    fn new(
        failed: &Failed<'_>,
        within: &[Seen],
        direct: &HashSet<String>,
        names_file: impl Fn(&str) -> bool,
    ) -> Self {
        let mut filtered = HashSet::new();
        let mut failing = HashSet::new();
        let mut named = HashSet::new();
        let mut skipping = false;
        for seen in within {
            if !failed.files.contains(seen.path.as_str()) || !failing.insert(seen.path.clone()) {
                continue;
            }
            if skipping || direct.contains(&seen.path) {
                skipping = true;
                filtered.insert(seen.path.clone());
            }
            if names_file(&seen.path) {
                named.insert(seen.path.clone());
            }
        }
        Self {
            filtered,
            failing,
            skipping,
            named,
        }
    }

    /// Whether pytest's collection keeps the case, so that dropping it
    /// counts as deselecting it.
    fn sees(&self, seen: &Seen) -> bool {
        if !self.failing.contains(&seen.path) {
            return !self.skipping;
        }
        !self.filtered.contains(&seen.path)
            || seen.failed
            || !seen.top
            || self.named.contains(&seen.path)
    }
}

/// The ids that failed last time, indexed for matching cases.
struct Failed<'a> {
    ids: &'a HashSet<TestId>,
    /// Ids naming a file or directory: a collection error.
    paths: Vec<Vec<&'a str>>,
    /// Parametrized ids, by the id without their parameter id.
    params: HashMap<&'a str, Vec<&'a TestId>>,
    /// The file (or directory) part of every id.
    files: HashSet<&'a str>,
}

impl<'a> Failed<'a> {
    fn new(ids: &'a HashSet<TestId>) -> Self {
        let mut paths = Vec::new();
        let mut params: HashMap<&str, Vec<&TestId>> = HashMap::new();
        let mut files = HashSet::new();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills lookups; what reaches output is sorted"
        )]
        for id in ids {
            let path = id.path();
            files.insert(path.as_str());
            if id.is_file() {
                let path: Vec<&str> = path.components().collect();
                if !path.is_empty() {
                    paths.push(path);
                }
            } else if id.parts().params.is_some() {
                params.entry(id.without_params()).or_default().push(id);
            }
        }
        Self {
            ids,
            paths,
            params,
            files,
        }
    }

    /// Whether the case's own id failed last time, or, for a `Partial`
    /// case, some of its parameter ids (pytest's `nodeid in lastfailed`).
    fn names(&self, case: &Case) -> bool {
        self.ids.contains(&case.id)
            || (case.certainty == Certainty::Partial && self.params.contains_key(case.id.as_str()))
    }

    /// `None` when the case did not fail last time; otherwise the parameter
    /// ids to narrow it to, empty for the whole case.
    fn judge(&self, case: &Case, narrow: &[TestId]) -> Option<Vec<TestId>> {
        let whole = self.ids.contains(&case.id)
            || (!self.paths.is_empty() && {
                let components: Vec<&str> = case.id.path().as_str().split('/').collect();
                self.paths.iter().any(|path| components.starts_with(path))
            });
        if !narrow.is_empty() {
            if whole {
                return Some(narrow.to_vec());
            }
            let failed: Vec<TestId> = narrow
                .iter()
                .filter(|id| self.ids.contains(*id))
                .cloned()
                .collect();
            return (!failed.is_empty()).then_some(failed);
        }
        if whole {
            return Some(Vec::new());
        }
        if case.certainty != Certainty::Partial {
            return None;
        }
        let mut failed: Vec<TestId> = self
            .params
            .get(case.id.as_str())?
            .iter()
            .map(|id| (*id).clone())
            .collect();
        failed.sort();
        Some(failed)
    }
}

#[cfg(test)]
mod tests;
