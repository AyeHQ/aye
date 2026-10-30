//! What a run selects of what collection found ([`select`]): the cases
//! static selection keeps, those of collection's diagnostics the run
//! reaches, a `not-found` for each argument that names nothing, and each
//! case it runs; and which `-k` and `-m` count for each adapter's cases
//! ([`Pick`]), which static selection's resolution applies.
//!
//! Decided (implementation, October 2026):
//! - **The framework's own `-k` and `-m`** (pytest's from `addopts` and
//!   `PYTEST_ADDOPTS`, in its adapter's `FrameworkDefaults`) apply
//!   statically to that adapter's cases when the command line gives none,
//!   so `--co` and `deselected` agree with pytest; other adapters' cases
//!   are left alone unless their frameworks name their own. One after `--` wins over both,
//!   for every case, since those arguments come last in pytest's. Where
//!   they can't be read (an `@file`, an unknown short option that might hold
//!   the letter), every adapter's cases get the command line's, else their
//!   own: no adapter applies an expression at runtime, so static selection
//!   is the last word on every case. The command line's must parse.
//! - **What names nothing** (a missing path; an id, dotted name or key that
//!   matches no test) is a `not-found` error diagnostic in the stream, and
//!   the run exits 4 without running, as pytest's `not found:` does. An
//!   existing file or directory without tests names nothing to run but is
//!   no error: alone, the run exits 5.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use aye_adapter::FrameworkDefaults;
use aye_events::{
    AdapterName, Case, Counts, Diagnostic, DiagnosticCode, DiagnosticPath, Event, ProjectPath,
    Severity, TestId,
};
use aye_fs::root_relative;
use aye_select::{Named, Selection, dotted_reach};

use super::collect::Collected;
use super::exit::RunVerdict;
use super::reach::{Reach, broken_conftest_dirs, explained_by_a_conftest};
use crate::framework_args::Passed;
use crate::positional::{Positional, Positionals};
use crate::settings::RunSettings;
use crate::stream::{Passes, Stream};

/// What a run selected for the runner, once collection is over: the
/// cases it runs, what its arguments reach when some aren't paths, and the
/// session's files by adapter.
pub(super) struct Selected {
    pub(super) cases: Vec<Case>,
    pub(super) reach: Option<Reach>,
    pub(super) session_files: BTreeMap<AdapterName, Vec<ProjectPath>>,
}

/// Select what the run runs of what collection found, through to
/// `collection_finished`, on `out`. Static selection (`selection`, with
/// what failed last under `--lf`: `last_failed`) keeps its cases, in the
/// order of the arguments. Of collection's diagnostics, those the run
/// reaches are emitted, after `passes` (a conftest's or a file's errors
/// counted as pytest counts them), then a `not-found` for each argument
/// that names nothing. Then each case the run runs (`case_collected`),
/// narrowed to the parameter ids asked for and, under `--ff`, what failed
/// last first. The errors the stream has carried so far are collection's,
/// and so is the verdict that comes with what is selected, which says
/// whether the run goes on (its counts hold the deselected).
pub(super) fn select(
    settings: &RunSettings,
    collected: Collected,
    selection: Selection,
    last_failed: &HashSet<TestId>,
    passes: &mut Passes<'_>,
    out: &mut Stream<'_>,
) -> (Selected, RunVerdict) {
    let Collected {
        collection,
        started,
    } = collected;
    let selection = if settings.last_failed {
        selection.with_last_failed(last_failed.iter().cloned())
    } else {
        selection
    };
    // Dotted names and keys are resolved among what collection found,
    // over the whole project: what the run reaches decides which of
    // collection's diagnostics are the run's (see `Reach`).
    let reach = Reach::of(&settings.positionals, &settings.root, &collection.cases);
    let session_files = collection.session_files(
        reach
            .as_ref()
            .map(|reach| (reach.selected.as_slice(), settings.root.as_path())),
    );
    let selected = selection.apply(collection.cases);
    let skips = skips(&collection.diagnostics, reach.as_ref(), &settings.root);
    let unmatched = unmatched(&selected.unmatched, &settings.positionals, &skips);
    // Whether an argument named nothing is known only now; the skips were
    // read without it.
    let reach = reach.map(|reach| Reach {
        unmatched: !unmatched.is_empty(),
        ..reach
    });
    let collection_errors = emit_diagnostics(
        settings,
        &collection.diagnostics,
        reach.as_ref(),
        passes,
        out,
    );
    for diagnostic in not_found(unmatched, &collection.diagnostics, &settings.root) {
        out.emit(Event::Diagnostic(diagnostic));
    }
    // Errors so far belong to collection; only later ones are the run's.
    let errors = out.sink.take_errors();

    let mut cases: Vec<Case> = Vec::with_capacity(selected.cases.len());
    for case in &selected.cases {
        for id in selected.ids_for(case) {
            let mut case = case.clone();
            case.id = id;
            cases.push(case);
        }
    }
    if settings.failed_first && !last_failed.is_empty() {
        cases.sort_by_key(|case| !last_failed.contains(&case.id));
    }
    for case in &cases {
        out.emit(Event::CaseCollected(Box::new(case.clone())));
    }
    let verdict = RunVerdict {
        // A hook that failed while pytest collected (aye's pytest
        // front-end runs the collection hooks in refinement) is pytest's
        // `INTERNALERROR`: the session ends there, status 3, `--co` too.
        internal: errors.internal,
        // pytest refuses to run when an explicit argument names nothing.
        usage: errors.usage,
        errors: collection_errors,
        selected: u32::try_from(cases.len()).unwrap_or(u32::MAX),
        counts: Counts {
            deselected: selected.deselected,
            ..Counts::default()
        },
        collect_only: settings.collect_only,
        continue_on_collection_errors: settings.continue_on_collection_errors,
        ..RunVerdict::default()
    };
    out.emit(Event::CollectionFinished {
        selected: verdict.selected,
        deselected: selected.deselected,
        refined_files: collection.refined_files,
        duration: started.elapsed(),
    });
    let selected = Selected {
        cases,
        reach,
        session_files,
    };
    (selected, verdict)
}

/// The modules and directories skipped while collecting that the run
/// reaches (all of them when `reach` is `None`): a dotted name for one
/// names no test, but pytest found it.
fn skips<'d>(diagnostics: &'d [Diagnostic], reach: Option<&Reach>, root: &Path) -> Vec<&'d str> {
    diagnostics
        .iter()
        .filter(|d| {
            d.code == DiagnosticCode::Skipped
                && d.severity == Severity::Info
                && reach.is_none_or(|reach| reach.keeps(root, d))
        })
        .map(|d| d.path.as_str())
        .collect()
}

/// Selectors that match nothing (`selected`'s unmatched, each with its
/// positional argument), as pytest's `not found:`: a missing path, or an
/// id, dotted name or key that names no test. An existing file or
/// directory that has no tests is not one (pytest collects it and finds
/// nothing), and neither is a dotted name for a module or package that was
/// skipped (in `skips`).
fn unmatched<'a>(
    selected: &'a [String],
    positionals: &'a Positionals,
    skips: &[&str],
) -> Vec<(&'a String, &'a Positional)> {
    selected
        .iter()
        .filter_map(|selector| {
            let positional = positionals.find(selector)?;
            let found = match positional {
                Positional::Path { absolute, .. } => absolute.exists(),
                Positional::Id { .. } => false,
                Positional::Word(_) => skips
                    .iter()
                    .any(|path| dotted_reach(selector, path) == Named::Whole),
            };
            (!found).then_some((selector, positional))
        })
        .collect()
}

/// Emit the diagnostics collection reported that the run reaches, but a
/// file's `no-tests` note below a conftest that failed to load: each
/// through `passes`, a `refine-failed` warning as an error. Whether any is
/// an error.
fn emit_diagnostics(
    settings: &RunSettings,
    diagnostics: &[Diagnostic],
    reach: Option<&Reach>,
    passes: &mut Passes<'_>,
    out: &mut Stream<'_>,
) -> bool {
    let refined_by = settings.adapters.refined_by();
    let mut errors = false;
    let broken = broken_conftest_dirs(diagnostics);
    for diagnostic in diagnostics {
        if reach.is_some_and(|reach| !reach.keeps(&settings.root, diagnostic))
            || explained_by_a_conftest(&broken, diagnostic)
        {
            continue;
        }
        let mut diagnostic = diagnostic.clone();
        if diagnostic.code == DiagnosticCode::RefineFailed {
            // aye's pytest front-end is the only source of pytest's
            // tests: without it there are none, which is no result.
            diagnostic.severity = Severity::Error;
        }
        passes.collected(&mut diagnostic, refined_by);
        errors |= diagnostic.severity == Severity::Error;
        out.emit(Event::Diagnostic(diagnostic));
    }
    errors
}

/// A `not-found` error for each selector that names nothing
/// (`unmatched`), but a missing path collection has reported already
/// (among `diagnostics`), which is reported once.
fn not_found(
    unmatched: Vec<(&String, &Positional)>,
    diagnostics: &[Diagnostic],
    root: &Path,
) -> Vec<Diagnostic> {
    let reported: HashSet<&str> = diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::NotFound)
        .map(|d| d.path.as_str())
        .collect();
    let mut not_found = Vec::new();
    for (selector, positional) in unmatched {
        let path = positional
            .path()
            .map(|path| root_relative(path, root))
            .unwrap_or_default();
        if !positional.exists() && reported.contains(path.as_str()) {
            continue;
        }
        not_found.push(Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::NotFound,
            path: DiagnosticPath::as_written(path),
            span: None,
            message: not_found_message(selector),
        });
    }
    not_found
}

/// pytest's `not found:`, with a word for those who expected a command.
fn not_found_message(selector: &str) -> String {
    if matches!(selector, "run" | "test" | "tests") && !selector.contains('/') {
        format!(
            "not found: {selector} (aye has no `{selector}` command: `aye` alone runs the tests)"
        )
    } else if matches!(selector, "show" | "config") {
        format!(
            "not found: {selector} (for `aye {selector}`, put `{selector}` first, before any \
             option: `aye {selector} …`)"
        )
    } else {
        format!("not found: {selector}")
    }
}

/// The `-k` or `-m` expressions (each adapter's `own`) static selection
/// applies, by adapter, when the arguments after `--` may hold one aye can't
/// read ([`Pick::Unknown`]): the command line's (`given`), else the
/// adapter's own. No adapter applies an expression at runtime (aye's
/// pytest front-end lists every test, and nothing re-reads those
/// arguments' `-k` or `-m` in a batch), so static selection is the last
/// word on every case.
pub(crate) fn unread<'d>(
    given: Option<&str>,
    defaults: &'d [(AdapterName, FrameworkDefaults)],
    own: impl Fn(&FrameworkDefaults) -> Option<String>,
) -> Vec<(&'d AdapterName, String)> {
    defaults
        .iter()
        .filter_map(|(name, defaults)| {
            let expr = given.map(str::to_string).or_else(|| own(defaults))?;
            Some((name, expr))
        })
        .collect()
}

/// Where a `-k` or `-m` expression for static selection comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pick {
    /// One for every case: the arguments after `--` give it (the framework
    /// sees them last), else the command line (an empty one turns
    /// selection off, the framework's own included, as in pytest).
    Given(String),
    /// The arguments after `--` may hold one aye can't read: every
    /// adapter's cases get the command line's, else their own ([`unread`]).
    Unknown,
    /// Neither gives one: each adapter's own (pytest's from `addopts` or
    /// `PYTEST_ADDOPTS`, in its context) for its own cases.
    Own,
}

/// See [`Pick`].
pub(crate) fn pick(given: Option<&str>, framework: Passed) -> Pick {
    match framework {
        Passed::Set(expr) => Pick::Given(expr),
        Passed::Unknown => Pick::Unknown,
        Passed::Unset => given.map_or(Pick::Own, |expr| Pick::Given(expr.to_string())),
    }
}
