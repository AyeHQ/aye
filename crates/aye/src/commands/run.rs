//! One run, from its settings ([`RunSettings`]), in the steps of
//! architecture.md's "A run" ([`run`]): what resolution leaves to the run
//! (the JUnit report's check, the runner, the stream), collect
//! (`collect.rs`), select (`select.rs`, with what the arguments reach,
//! `reach.rs`), run the plan (`plan.rs`), and finish, with the exit code
//! decided in one place (`exit.rs`). Stop signals are handled from the
//! moment the runner exists (`interrupt.rs`), and every record goes to the
//! stream (`crate::stream`), the runner's after the passes that rewrite
//! them.
//!
//! Decided (implementation, October 2026):
//! - **Never silent, always stoppable.** While the run's settings resolve
//!   (preparing the pytest adapter may walk a big tree), a `collecting…`
//!   line shows on a terminal after half a second (the printer's `Busy`).
//!   Collection then sends `collection_progress` (after half a second, at
//!   most four a second).
//!   The first Ctrl-C before the run stops collection, which ends as
//!   interrupted (exit 2); if what is running then isn't watching, aye
//!   exits 2 after a second and a half anyway, its workers killed.
//! - **Workers and refinement.** Every worker gets the run's id as
//!   `PYTEST_XDIST_TESTRUNUID`, so pytest-xdist's `testrun_uid` is shared by
//!   a run's workers and new each run. The refinement cache is on, keyed on
//!   the interpreter workers run, the arguments after `--` and aye's own
//!   environment (which workers inherit; the run's id is left out). The
//!   plan's session is, by adapter, every file collection found tests in,
//!   selected or not, in collection order (with dotted names or keys, only
//!   those in the paths the run starts from): what each worker of an
//!   executor that lists the session (the pytest adapter's) lists before
//!   its first batch.

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use aye_config::OutputFormat;
use aye_events::{Counts, Event, RunId, SCHEMA_VERSION, Stamper, StopReason};
use aye_fs::{StateBucket, StateDir};
use aye_history::{History, RunWriter};
use aye_runner::{Echo, Runner, RunnerOptions};
use aye_select::LastFailed;
use aye_static::EnvVars;

use crate::error::AyeError;
use crate::printer::Printer;
use crate::registry;
use crate::settings::{JunitTarget, RunSettings};
use crate::stream::{Passes, Remember, RerunFromHere, Sink, Stream, reporters};

mod collect;
mod exit;
mod interrupt;
mod plan;
mod reach;
pub(crate) mod select;

pub(crate) use exit::{Errors, RunEnd};
use exit::{RunStage, RunVerdict};
use interrupt::Signals;

/// Run what `settings` ask for, and say how it ended ([`RunEnd`]), or why
/// it couldn't start: a JUnit report that can't be written, before any
/// event. What aye says beside the stream goes to `printer`.
pub(crate) fn run(mut settings: RunSettings, printer: Printer) -> Result<RunEnd, AyeError> {
    let started = Instant::now();
    // Resolve: what the settings leave to the run. A report that can't be
    // written fails before anything runs, not after the whole suite.
    let junit = settings.junit.take();
    if let Some(junit) = &junit {
        check_writable(junit)?;
    }
    let selection = std::mem::take(&mut settings.selection);
    let state = StateDir::for_root(&settings.root);
    let mut history = History::load(state.path());
    let durations = history.durations();
    let last_failed: LastFailed = history.last_failed().into_iter().collect();
    let run_id = RunId::random();
    let runner = Runner::new(runner_options(&settings, &run_id));
    let signals = Signals::install(&runner, printer);
    let mut out = start(
        &settings,
        &state,
        junit,
        &mut history,
        Stamper::with_run(run_id),
        started,
        printer,
    );

    // Collect.
    let Some(collected) = collect::collect(&settings, &state, &runner, &signals.stop, &mut out)
    else {
        let verdict = RunVerdict {
            interrupted: true,
            ..RunVerdict::default()
        };
        return Ok(out.finish(
            verdict.exit_code(RunStage::Collection),
            verdict.counts,
            Some(StopReason::Interrupted),
        ));
    };

    // Select.
    let mut passes = Passes::new(RerunFromHere {
        cwd: &settings.cwd,
        root: &settings.root,
        rerun_args: &settings.rerun_args,
    });
    let (selected, collection) = select::select(
        &settings,
        collected,
        selection,
        &last_failed,
        &mut passes,
        &mut out,
    );
    if collection.ends_after_collection() {
        return Ok(out.finish(
            collection.exit_code(RunStage::Collection),
            collection.counts,
            None,
        ));
    }

    // Run.
    let plan = plan::plan(&settings, selected, durations);
    signals.running();
    let summary = runner.run(&plan, &mut out.stamper, &mut |mut record| {
        passes.apply(&mut record);
        out.sink.deliver(&mut record);
    });

    // Finish.
    let counts = Counts {
        deselected: collection.counts.deselected,
        ..summary.counts
    };
    let verdict = collection.with_run(&summary, out.sink.take_errors(), counts);
    Ok(out.finish(
        verdict.exit_code(RunStage::Run),
        verdict.counts,
        summary.stopped,
    ))
}

/// What the runner is set up with: the interpreter, where it runs, the
/// workers, what every worker installs (the prelude, each adapter's
/// runtime), the variables workers get beside aye's environment, where
/// `-s` output goes, and whether the run only lists.
fn runner_options(settings: &RunSettings, run_id: &RunId) -> RunnerOptions {
    RunnerOptions {
        python: settings.interpreter.value.executable.clone(),
        root: settings.root.clone(),
        cwd: settings.cwd.clone(),
        workers: settings.workers,
        prelude: registry::PRELUDE,
        runtimes: settings.adapters.runtimes(),
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
    }
}

/// The run's stream, started with `run_started`: the reporters (the JUnit
/// report to `junit`), `history`, saved in the state directory, and the
/// run store, which claims the run's number unless `--co`.
fn start<'a>(
    settings: &RunSettings,
    state: &StateDir,
    junit: Option<JunitTarget>,
    history: &'a mut History,
    stamper: Stamper,
    started: Instant,
    printer: Printer,
) -> Stream<'a> {
    let report_failed = Arc::new(AtomicBool::new(false));
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
        reporters(settings, junit, &report_failed, printer),
        report_failed,
        Some(Remember {
            history,
            dir: state.path().to_path_buf(),
        }),
        writer,
        printer,
    );
    let mut out = Stream::new(stamper, sink, started);
    out.emit(Event::RunStarted {
        schema_version: SCHEMA_VERSION,
        aye_version: env!("CARGO_PKG_VERSION").to_string(),
        root: settings.root.display().to_string(),
        collect_only: settings.collect_only,
        workers: settings.workers,
        run: number,
    });
    out
}

/// Whether the JUnit report can be written where `junit` says, as a usage
/// error when it can't.
fn check_writable(junit: &JunitTarget) -> Result<(), AyeError> {
    writable(&junit.path).map_err(|err| {
        AyeError::usage(format!(
            "cannot write the JUnit XML report to {}: {err}",
            junit.path.display()
        ))
    })
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
