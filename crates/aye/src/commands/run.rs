//! One run, from its settings ([`RunSettings`]): collection, selection, the
//! runner and the reporters, wired around the event stream.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use aye_adapter::Project;
use aye_collect::{Collector, Options};
use aye_config::OutputFormat;
use aye_events::{
    Case, Counts, Diagnostic, DiagnosticCode, DiagnosticPath, Event, RunId, SCHEMA_VERSION,
    Severity, Stamper, StopReason,
};
use aye_fs::{StateBucket, StateDir, root_relative};
use aye_history::{History, RunWriter};
use aye_protocol::Refiner;
use aye_provision::EnvironmentFingerprint;
use aye_runner::{Echo, Plan, Runner, RunnerOptions};
use aye_static::EnvVars;

use crate::error::AyeError;
use crate::positional::Positional;
use crate::printer::Printer;
use crate::registry;
use crate::settings::RunSettings;
use crate::stream::{Keep, Passes, Remember, RerunFromHere, Sink, Stream, reporters};

mod collect;
mod exit;
mod interrupt;
mod plan;
mod reach;
pub(crate) mod select;

use collect::Pace;
pub(crate) use exit::{Errors, RunEnd};
use exit::{RunStage, RunVerdict};
use interrupt::install_interrupt;
use plan::{requested_ids, session_files};
use reach::{Named, Reach, broken_conftest_dirs, dotted_reach, explained_by_a_conftest};
use select::not_found_message;

/// Run what `settings` ask for, and say how it ended ([`RunEnd`]), or why
/// it couldn't start: a JUnit report that can't be written, before any
/// event. What aye says beside the stream goes to `printer`.
pub(crate) fn run(mut settings: RunSettings, printer: Printer) -> Result<RunEnd, AyeError> {
    let started = Instant::now();
    // A report that can't be written fails before anything runs, not
    // after the whole suite.
    let junit = settings.junit.take();
    if let Some(junit) = &junit
        && let Err(err) = writable(&junit.path)
    {
        return Err(AyeError::usage(format!(
            "cannot write the JUnit XML report to {}: {err}",
            junit.path.display()
        )));
    }
    let state = StateDir::for_root(&settings.root);
    let mut history = History::load(state.path());
    let durations = history.durations();
    let last_failed = history.last_failed();
    let mut selection = std::mem::take(&mut settings.selection);
    if settings.last_failed {
        selection = selection.with_last_failed(last_failed.iter().cloned());
    }

    let run_id = RunId::random();
    let runner = Runner::new(RunnerOptions {
        python: settings.interpreter.value.executable.clone(),
        root: settings.root.clone(),
        cwd: settings.cwd.clone(),
        workers: settings.workers,
        prelude: registry::PRELUDE,
        runtimes: settings.adapters.iter().map(|a| a.runtime()).collect(),
        // pytest-xdist's `testrun_uid`, which its workers share: one per
        // run, the run's id.
        env: vec![(
            EnvVars::PYTEST_XDIST_TESTRUNUID.into(),
            run_id.as_str().into(),
        )],
        // `-s` output stays off a machine-readable stdout.
        echo: match settings.output.format.value {
            OutputFormat::Text => Echo::Stdout,
            OutputFormat::Json => Echo::Stderr,
        },
        collect_only: settings.collect_only,
    });
    // Ctrl-C stops collection as well as the run: the walk of a big tree
    // and refinement can take a while, and must not ignore it.
    let interrupted = Arc::new(AtomicBool::new(false));
    let running = Arc::new(AtomicBool::new(false));
    install_interrupt(
        runner.stop_handle(),
        Arc::clone(&interrupted),
        Arc::clone(&running),
        printer,
    );

    let file_failed = Arc::new(AtomicBool::new(false));
    // The run's number in the store (`r81`), which `run_started` says,
    // when the run will be stored.
    let mut writer = RunWriter::new(
        state.bucket(StateBucket::Runs),
        settings.tool_aye.keep_runs(),
    );
    let number = if settings.collect_only {
        None
    } else {
        writer.claim()
    };
    let sink = Sink::new(
        reporters(&settings, junit, &file_failed, printer),
        Arc::clone(&file_failed),
        Some(Remember {
            history: &mut history,
            dir: state.path().to_path_buf(),
        }),
        Keep::new(writer),
        printer,
    );
    let mut out = Stream::new(Stamper::with_run(run_id), sink, started);

    out.emit(Event::RunStarted {
        schema_version: SCHEMA_VERSION,
        aye_version: env!("CARGO_PKG_VERSION").to_string(),
        root: settings.root.display().to_string(),
        collect_only: settings.collect_only,
        workers: settings.workers,
        run: number,
    });

    // -- collect and select ---------------------------------------------
    out.emit(Event::CollectionStarted);
    let collect_started = Instant::now();
    let project = settings.project();
    // The refinement cache is keyed on what refining depends on: the
    // interpreter and its packages, the arguments after `--`, and the
    // environment workers inherit (aye's own, as it read it; the
    // `PYTEST_XDIST_TESTRUNUID` the runner adds changes every run and
    // refinement has no use for it).
    let collector = Collector::new(
        project,
        &settings.prepared,
        Options {
            strict: settings.strict,
            cache_dir: Some(state.bucket(StateBucket::Cache)),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    )
    .with_environment(EnvironmentFingerprint {
        interpreter: settings.interpreter.value.executable.clone(),
        framework_args: settings.framework_args.clone(),
        variables: settings.env.vars.clone(),
    })
    .with_stop(Arc::clone(&interrupted));
    let targets: Vec<PathBuf> = if settings.only_paths {
        settings.paths.clone()
    } else if settings.paths.is_empty() {
        Vec::new()
    } else {
        // Paths mixed with dotted names or keys: the paths, and wherever
        // the adapters look when no path is given, for the rest.
        let bare = Project {
            args: &[],
            ..project
        };
        let mut targets: Vec<PathBuf> = settings
            .prepared
            .iter()
            .flat_map(|(_, recognizer)| recognizer.roots(&bare))
            .filter(|root| root.exists())
            .collect();
        targets.extend(settings.paths.iter().cloned());
        targets
    };
    let refiner: Option<&(dyn Refiner + Sync)> = if settings.no_refine {
        None
    } else {
        Some(&runner)
    };
    let collection = {
        let mut pace = Pace::new(collect_started);
        collector.collect_watching(&targets, refiner, &mut |progress| {
            if pace.due() {
                out.emit(Event::CollectionProgress {
                    walked: progress.walked,
                    test_files: progress.test_files,
                    refining: progress.refining,
                });
            }
        })
    };
    if collection.interrupted {
        out.emit(Event::CollectionFinished {
            selected: 0,
            deselected: 0,
            refined_files: 0,
            duration: collect_started.elapsed(),
        });
        let verdict = RunVerdict {
            interrupted: true,
            ..RunVerdict::default()
        };
        return Ok(out.finish(
            verdict.exit_code(RunStage::Collection),
            verdict.counts,
            Some(StopReason::Interrupted),
        ));
    }
    // Dotted names and keys are resolved among what collection found,
    // over the whole project: what the run reaches decides which of
    // collection's diagnostics are the run's (see `Reach`).
    let reach = Reach::of(&settings.positionals, &settings.root, &collection.cases);
    let session_files = session_files(
        &collection.cases,
        reach
            .as_ref()
            .map(|reach| (reach.selected.as_slice(), settings.root.as_path())),
    );
    let selected = selection.apply(collection.cases);
    let kept = |diagnostic: &Diagnostic| {
        reach
            .as_ref()
            .is_none_or(|reach| reach.keeps(&settings.root, diagnostic))
    };
    // The modules and directories skipped while collecting that the run
    // reaches: a dotted name for one names no test, but pytest found it.
    let skips: Vec<&str> = collection
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::Skipped && d.severity == Severity::Info && kept(d))
        .map(|d| d.path.as_str())
        .collect();
    // Selectors that match nothing, as pytest's `not found:`: a missing
    // path, or an id, dotted name or key that names no test. An existing
    // file or directory that has no tests is not one (pytest collects it
    // and finds nothing).
    let unmatched: Vec<(&String, &Positional)> = selected
        .unmatched
        .iter()
        .filter_map(|selector| {
            let positional = settings
                .positionals
                .iter()
                .find(|p| p.selector == *selector)?;
            let found = (positional.exists() && !positional.id)
                || (positional.path.is_none()
                    && skips
                        .iter()
                        .any(|path| dotted_reach(selector, path) == Named::Whole));
            (!found).then_some((selector, positional))
        })
        .collect();
    let reach = reach.map(|reach| Reach {
        unmatched: !unmatched.is_empty(),
        ..reach
    });
    let mut passes = Passes::new(RerunFromHere {
        cwd: &settings.cwd,
        root: &settings.root,
        rerun_args: &settings.rerun_args,
    });
    let pytest = aye_adapter_pytest::NAME;
    let pytest_runs = settings.prepared.iter().any(|(name, _)| *name == pytest);
    let refined_by = pytest_runs.then_some(&pytest);
    let mut collection_errors = false;
    let broken = broken_conftest_dirs(&collection.diagnostics);
    for diagnostic in &collection.diagnostics {
        if reach
            .as_ref()
            .is_some_and(|reach| !reach.keeps(&settings.root, diagnostic))
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
        collection_errors |= diagnostic.severity == Severity::Error;
        out.emit(Event::Diagnostic(diagnostic));
    }

    // A missing path collection has reported already is reported once.
    let reported: HashSet<&str> = collection
        .diagnostics
        .iter()
        .filter(|d| d.code == DiagnosticCode::NotFound)
        .map(|d| d.path.as_str())
        .collect();
    let mut not_found = Vec::new();
    for (selector, positional) in unmatched {
        let path = positional
            .path
            .as_deref()
            .map(|path| root_relative(path, &settings.root))
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
    for diagnostic in not_found {
        out.emit(Event::Diagnostic(diagnostic));
    }
    // Errors so far belong to collection; only later ones are the run's.
    let collected_errors = out.sink.take_errors();
    // pytest refuses to run when an explicit argument names nothing.
    let usage_error = collected_errors.usage;
    // A hook that failed while pytest collected (aye's pytest front-end
    // runs the collection hooks in refinement) is pytest's
    // `INTERNALERROR`: the session ends there, status 3, `--co` too.
    let internal_error = collected_errors.internal;

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
    let collected = RunVerdict {
        internal: internal_error,
        usage: usage_error,
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
        selected: collected.selected,
        deselected: selected.deselected,
        refined_files: collection.refined_files,
        duration: collect_started.elapsed(),
    });
    if collected.ends_after_collection() {
        return Ok(out.finish(
            collected.exit_code(RunStage::Collection),
            collected.counts,
            None,
        ));
    }

    // -- run ----------------------------------------------------------------
    let plan = Plan {
        cases,
        policy: settings.policy.clone(),
        contexts: settings
            .prepared
            .iter()
            .map(|(name, r)| (name.clone(), r.context()))
            .collect(),
        requested_ids: requested_ids(&settings.positionals),
        rerun_args: settings.rerun_args.clone(),
        selected_paths: reach
            .map(|reach| reach.selected)
            .unwrap_or_default()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        durations,
        first: if settings.failed_first {
            last_failed
        } else {
            HashSet::new()
        },
        session_files,
    };
    running.store(true, Ordering::SeqCst);
    let summary = runner.run(&plan, &mut out.stamper, &mut |mut record| {
        passes.apply(&mut record);
        out.sink.deliver(&mut record);
    });
    let counts = Counts {
        deselected: selected.deselected,
        ..summary.counts
    };
    let verdict = collected.with_run(&summary, out.sink.take_errors(), counts);
    Ok(out.finish(
        verdict.exit_code(RunStage::Run),
        verdict.counts,
        summary.stopped,
    ))
}

/// Whether a report can be written to `path`: its directory is created, and
/// the file opened without truncating it.
fn writable(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .map(drop)
}

#[cfg(test)]
mod tests;
