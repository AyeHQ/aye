//! Which of a stored run's results `aye show` shows: those its arguments
//! name ([`choose`]: by key or its start, id, dotted name or path, with the
//! closest names for one that names nothing), and of those, what its
//! filters keep ([`Filter`]: `--failed`, `--outcome`, `-k`, `-m`).

use std::collections::HashSet;
use std::path::Path;

use aye_events::{Case, KeyPrefix, Outcome, TestId};
use aye_history::{RunMeta, StoredRunId};
use aye_select::Selection;
use aye_stream::{ItemIndex, Keys, RunIndex, RunItem, ShortKey};

use crate::cli::ShowArgs;
use crate::error::AyeError;
use crate::positional::Positional;

/// How many of the closest names a selector naming nothing lists.
pub(super) const CLOSEST: usize = 5;

/// A positional argument, as `aye show` reads it.
#[derive(Debug, Clone)]
pub(super) enum Wanted {
    /// A file or directory, relative to the root: alone, it filters the
    /// list; beside the others, it selects its results.
    Path(String),
    /// A key or its start, an id or a dotted name.
    Test(String),
}

/// How `aye show` reads a positional argument: a path (read as a run reads
/// one), or a test's key or its start, id or dotted name.
pub(super) fn read(cwd: &Path, root: &Path, arg: &str) -> Wanted {
    match Positional::resolve(cwd, root, arg) {
        Positional::Path { selector, .. } => Wanted::Path(selector),
        Positional::Id { selector, .. } | Positional::Word(selector) => Wanted::Test(selector),
    }
}

/// The results the arguments select, by their place in
/// [`RunIndex::items`]: every one without any; for a key, or the start of
/// one test's or lifetime's key ([`Keys::resolve`]), that test's every
/// result or that
/// lifetime; for another selector, every result it names (a test, a
/// lifetime, a file's diagnostics, as an id names them). One that names
/// nothing of the run is a usage error, listing the closest names; the
/// start of several keys too, listing them.
pub(super) fn choose(
    index: &RunIndex<'_>,
    keys: &Keys,
    wanted: &[(&str, Wanted)],
    meta: &RunMeta,
) -> Result<HashSet<ItemIndex>, AyeError> {
    if wanted.is_empty() {
        return Ok(index.items.indices().collect());
    }
    let mut chosen = HashSet::new();
    let mut problems = Vec::new();
    let names = index.names();
    let with_results: HashSet<&str> = names.iter().map(TestId::as_str).collect();
    let mut selectors: Vec<(&str, &str)> = Vec::new();
    for (arg, wanted) in wanted {
        let selector = match wanted {
            Wanted::Path(selector) => selector,
            Wanted::Test(selector) => {
                let found = arg
                    .parse::<KeyPrefix>()
                    .map_or_else(|_| Vec::new(), |prefix| keys.resolve(&prefix));
                if found.is_empty() {
                    selector
                } else {
                    match by_key(arg, &found, &with_results, &meta.run) {
                        Ok(id) => chosen.extend(
                            index
                                .items
                                .iter_enumerated()
                                .filter(|(_, item)| item.name.as_str() == id)
                                .map(|(at, _)| at),
                        ),
                        Err(problem) => problems.push(problem),
                    }
                    continue;
                }
            }
        };
        selectors.push((arg, selector.as_str()));
    }
    if !selectors.is_empty() {
        let selection = Selection::default().with_ids(selectors.iter().map(|(_, s)| *s));
        let selected = selection.apply(names.iter().map(|name| index.case(name)).collect());
        let kept: HashSet<TestId> = selected.cases.into_iter().map(|case| case.id).collect();
        chosen.extend(
            index
                .items
                .iter_enumerated()
                .filter(|(_, item)| kept.contains(&item.name))
                .map(|(at, _)| at),
        );
        for unmatched in &selected.unmatched {
            let arg = selectors
                .iter()
                .find(|(_, selector)| selector == unmatched)
                .map_or(unmatched.as_str(), |(arg, _)| *arg);
            problems.push(nothing_named(arg, &meta.run, &closest(arg, &names)));
        }
    }
    if problems.is_empty() {
        Ok(chosen)
    } else {
        Err(AyeError::usage(problems.join("\naye: error: ")))
    }
}

/// The one id with a result in run `run` that `arg` names as a key, of the
/// ids `found`, with their keys, whose key it is or starts (those in
/// `with_results` have one); else why not: the start of several keys, or
/// of keys none of whose tests has a result.
fn by_key<'a>(
    arg: &str,
    found: &[(&'a str, ShortKey)],
    with_results: &HashSet<&str>,
    run: &StoredRunId,
) -> Result<&'a str, String> {
    let shown: Vec<(&str, ShortKey)> = found
        .iter()
        .copied()
        .filter(|(id, _)| with_results.contains(id))
        .collect();
    match shown[..] {
        [(id, _)] => Ok(id),
        [] => Err(match found {
            [(id, _)] => format!("{arg}: the key of {id}, which has no result in run {run}"),
            _ => format!(
                "{arg}: the start of {} tests' keys, none of which has a result in run {run}",
                found.len()
            ),
        }),
        _ => {
            let width = shown.iter().map(|(_, key)| key.digits()).max().unwrap_or(0);
            let candidates: Vec<String> = shown
                .iter()
                .map(|(id, key)| format!("\n  {key:<width$}  {id}"))
                .collect();
            Err(format!(
                "{arg}: the start of several keys; give more of the one you want:{}",
                candidates.concat()
            ))
        }
    }
}

fn nothing_named(arg: &str, run: &StoredRunId, closest: &[&str]) -> String {
    let mut message = format!("{arg}: no test of run {run} has that name");
    if !closest.is_empty() {
        message.push_str("; the closest:");
        for id in closest {
            message.push_str("\n  ");
            message.push_str(id);
        }
    }
    message
}

/// The names closest to `arg`, without their parameters (each selects all
/// of them), each once: by edit distance to the whole name, or between
/// the last names (a test with that name elsewhere: an argument's last
/// `::` part, else its last dotted part, else itself, against a name's
/// last `::` part or its file's name), whichever is closest.
pub(super) fn closest<'a>(arg: &str, names: &'a [TestId]) -> Vec<&'a str> {
    let whole: Vec<char> = arg.chars().collect();
    // What the argument's last name may be: `tests/t.py::T::test_x` ends
    // in `test_x`, a dotted `tests.t.test_x` too, while a file's name
    // (`usage.rst`, `test_t.py`) is its own.
    let mut lasts: Vec<Vec<char>> = vec![whole.clone()];
    match arg.rsplit_once("::") {
        Some((_, name)) => lasts.push(name.chars().collect()),
        None if !arg.contains('/') => {
            if let Some((_, name)) = arg.rsplit_once('.') {
                lasts.push(name.chars().collect());
            }
        }
        None => {
            if let Some((_, name)) = arg.rsplit_once('/') {
                lasts.push(name.chars().collect());
            }
        }
    }
    let mut seen = HashSet::new();
    let mut scored: Vec<(usize, &str)> = names
        .iter()
        .filter(|id| seen.insert(id.without_params()))
        .map(|id| {
            let name = id.without_params();
            let file = id.path().file_name();
            let last = if id.is_file() {
                file
            } else {
                id.parts().names.rsplit("::").next().unwrap_or(file)
            };
            let score = lasts
                .iter()
                .flat_map(|arg| [distance(arg, last), distance(arg, file)])
                .fold(distance(&whole, name), usize::min);
            (score, name)
        })
        .collect();
    scored.sort_unstable();
    scored.into_iter().take(CLOSEST).map(|(_, id)| id).collect()
}

/// Levenshtein's distance between `a` and `b`, in characters.
pub(super) fn distance(a: &[char], b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// `--failed`, `--outcome`, `-k` and `-m`.
pub(super) struct Filter {
    /// The outcomes kept; empty keeps any.
    outcomes: HashSet<Outcome>,
    selection: Selection,
}

impl Filter {
    pub(super) fn new(args: &ShowArgs) -> Result<Option<Self>, AyeError> {
        let mut outcomes: HashSet<Outcome> = args.outcome.iter().copied().collect();
        if args.failed {
            outcomes.extend([Outcome::Failed, Outcome::Error]);
        }
        let mut selection = Selection::default();
        if let Some(expr) = &args.keyword {
            selection = selection
                .with_keyword(expr)
                .map_err(|err| AyeError::usage(err.to_string()))?;
        }
        if let Some(expr) = &args.tags {
            selection = selection
                .with_tags(expr)
                .map_err(|err| AyeError::usage(err.to_string()))?;
        }
        Ok(
            (!outcomes.is_empty() || !selection.is_empty()).then_some(Self {
                outcomes,
                selection,
            }),
        )
    }

    /// Whether a result is kept: by its outcome, and by `-k` and `-m` on
    /// its case (what can't be decided is kept, as a run keeps it). One
    /// without a name (a failed worker, a diagnostic about no file) has
    /// nothing for `-k` or `-m` to match.
    pub(super) fn keeps(&self, item: &RunItem<'_>, case: &Case) -> bool {
        (self.outcomes.is_empty() || self.outcomes.contains(&item.outcome))
            && (self.selection.is_empty()
                || (!item.name.as_str().is_empty() && !self.selection.truth(case).is_false()))
    }
}
