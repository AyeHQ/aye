//! A selection ([`Selection`]) and what it keeps of the cases it is
//! applied to ([`Selected`]): each case's verdict, from the positional
//! arguments, `--deselect` and both expressions, then last failed, in the
//! arguments' order.

use std::collections::{BTreeMap, HashSet};

use aye_events::{AdapterName, Case, Certainty, IdPrefix, TestId};

use crate::keyword::{keyword_truth, parse_keyword};
use crate::last_failed::{Failed, Seen, Visible};
use crate::marker::{parse_tags, tags_truth};
use crate::selector::Parts;
use crate::{Expression, SelectError, Selector, Truth};

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
pub(crate) fn miss(case: &Case) -> Truth {
    if case.certainty == Certainty::Partial {
        Truth::Unknown
    } else {
        Truth::False
    }
}
