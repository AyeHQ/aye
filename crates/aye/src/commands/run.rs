//! One run, from its settings ([`RunSettings`]): collection, selection, the
//! runner and the reporters, wired around the event stream.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aye_adapter::Project;
use aye_collect::{Collector, Options};
use aye_config::OutputFormat;
use aye_events::{
    AdapterName, Case, Counts, Diagnostic, DiagnosticCode, DiagnosticOrigin, DiagnosticPath, Event,
    ExitCode, ProjectPath, Record, RerunCommand, RunId, SCHEMA_VERSION, Severity, Stamper,
    StopReason, TestId,
};
use aye_fs::{StateBucket, StateDir, normalize_absolute, relative_below_root, root_relative};
use aye_history::{History, RunWriter, StoredRunId};
use aye_protocol::Refiner;
use aye_provision::EnvironmentFingerprint;
use aye_render::{GithubAnnotations, JsonLines, Junit, Reporter, Tally, Tee, Text, TextOptions};
use aye_runner::{Echo, Plan, Runner, RunnerOptions};
use aye_select::Selection;
use aye_static::EnvVars;

use crate::error::AyeError;
use crate::positional::Positional;
use crate::printer::Printer;
use crate::registry;
use crate::settings::{JunitTarget, RunSettings};

mod exit;

pub(crate) use exit::RunEnd;
use exit::{Errors, RunStage, RunVerdict};

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
    let mut out = Stream {
        reporters: reporters(&settings, junit, &file_failed, printer),
        history: Some(Remember {
            history: &mut history,
            dir: state.path().to_path_buf(),
        }),
        store: Keep::new(writer),
        stamper: Stamper::with_run(run_id),
        errors: Errors::default(),
        started,
        printer,
    };

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
            &file_failed,
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
    let mut conftests = OneConftestError::default();
    let mut once = OneModuleErrors::default();
    let pytest_runs = settings
        .prepared
        .iter()
        .any(|(name, _)| *name == aye_adapter_pytest::NAME);
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
        conftests.see(None, &mut diagnostic);
        if pytest_runs {
            once.refined(&aye_adapter_pytest::NAME, &diagnostic);
        }
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
    // pytest refuses to run when an explicit argument names nothing.
    let usage_error = out.errors.usage;
    // A hook that failed while pytest collected (aye's pytest front-end
    // runs the collection hooks in refinement) is pytest's
    // `INTERNALERROR`: the session ends there, status 3, `--co` too.
    let internal_error = out.errors.internal;
    // Errors so far belong to collection; only later ones are the run's.
    out.errors = Errors::default();

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
            &file_failed,
        ));
    }

    // -- run ----------------------------------------------------------------
    let rerun_args = settings.rerun_args.clone();
    let plan = Plan {
        cases,
        policy: settings.policy.clone(),
        contexts: settings
            .prepared
            .iter()
            .map(|(name, r)| (name.clone(), r.context()))
            .collect(),
        requested_ids: requested_ids(&settings.positionals),
        rerun_args: rerun_args.clone(),
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
    let summary = {
        let Stream {
            reporters,
            history,
            store,
            stamper,
            errors,
            ..
        } = &mut out;
        let here = Here {
            cwd: &settings.cwd,
            root: &settings.root,
            rerun_args: &rerun_args,
        };
        running.store(true, Ordering::SeqCst);
        runner.run(&plan, stamper, &mut |mut record| {
            here.rerun_from_here(&mut record);
            once.see(&mut record);
            conftests.see_record(&mut record);
            Stream::deliver_to(reporters, history, store, errors, printer, &mut record);
        })
    };
    let counts = Counts {
        deselected: selected.deselected,
        ..summary.counts
    };
    let verdict = collected.with_run(&summary, out.errors, counts);
    Ok(out.finish(
        verdict.exit_code(RunStage::Run),
        verdict.counts,
        summary.stopped,
        &file_failed,
    ))
}

/// The run's reporters: its output format's on stdout (and GitHub's
/// annotations beside text, in a step of GitHub Actions), and the JUnit
/// report, whose failure at the end `printer` says and `file_failed`
/// remembers.
fn reporters<'a>(
    settings: &RunSettings,
    junit: Option<JunitTarget>,
    file_failed: &Arc<AtomicBool>,
    printer: Printer,
) -> Tee<'a> {
    let mut tee = Tee::new();
    let verbosity = settings.output.verbosity.level();
    match settings.output.format.value {
        OutputFormat::Text => {
            tee.push(Stdout::new(Text::new(
                io::stdout(),
                TextOptions {
                    verbosity,
                    color: settings.output.color,
                    live: io::stdout().is_terminal(),
                },
            )));
            if aye_render::github_actions_enabled(&settings.env.vars) {
                let prefix =
                    repository_prefix(&settings.root, settings.env.github_workspace.as_deref());
                tee.push(Stdout::new(
                    GithubAnnotations::new(io::stdout()).with_path_prefix(prefix),
                ));
            }
        }
        OutputFormat::Json => tee.push(Stdout::new(JsonLines::new(io::stdout(), verbosity))),
    }
    if let Some(junit) = junit {
        tee.push(File {
            inner: Junit::new(junit.path)
                .with_prefix(junit.prefix)
                .with_options(junit.options),
            failed: Arc::clone(file_failed),
            printer,
        });
    }
    tee
}

/// Where aye runs, for the reruns it prints.
struct Here<'a> {
    cwd: &'a Path,
    root: &'a Path,
    /// aye's options that shape the run, which a rerun repeats before the
    /// id ([`aye_protocol::BatchContext::rerun_args`]).
    rerun_args: &'a [String],
}

impl Here<'_> {
    /// Make a failed test's rerun (`aye <id>`) rerun it from the current
    /// directory. Ids are relative to the root, and a path is taken from
    /// the current directory first, then from the root: from a
    /// subdirectory holding a path of the same name (`tests/tests/…`), the
    /// id would name another file. There the rerun names the test's file
    /// relative to the current directory; elsewhere it stays the id.
    fn rerun_from_here(&self, record: &mut Record) {
        let Event::TestFinished(result) = &mut record.event else {
            return;
        };
        if result.reproduce.is_none() && !result.outcome.is_bad() {
            return;
        }
        let Some(local) = self.local_id(&result.id) else {
            return;
        };
        match &mut result.reproduce {
            Some(reproduce) => {
                if let Some(mut rerun) = RerunCommand::parse(&reproduce.command, &result.id) {
                    rerun.target = local;
                    reproduce.command = rerun.to_command();
                }
            }
            None if result.outcome.is_bad() => {
                let rerun = RerunCommand {
                    options: self.rerun_args.to_vec(),
                    target: local,
                    framework_args: Vec::new(),
                };
                result.reproduce = Some(aye_events::Reproduce {
                    command: rerun.to_command(),
                    ..aye_events::Reproduce::default()
                });
            }
            None => {}
        }
    }

    /// `id` spelled from the current directory, when spelled from the root
    /// it would name another file from here; see [`Self::rerun_from_here`].
    fn local_id(&self, id: &TestId) -> Option<String> {
        if self.cwd == self.root {
            return None;
        }
        let path = id.path().as_str();
        let from_cwd = normalize_absolute(&self.cwd.join(path));
        let from_root = normalize_absolute(&self.root.join(path));
        if from_cwd == from_root || !from_cwd.exists() {
            return None;
        }
        // The names, from their `::` on, follow the file as they are.
        let mut local = relative_below_root(&from_root, self.cwd);
        local.push_str(&id.as_str()[path.len()..]);
        Some(local)
    }
}

/// A file's collection errors from the batches of adapters pytest collects
/// in one module ([`registry::collects_in_one_module`]: the unittest and
/// pytest adapters, which may each have tests in the file), counted as
/// pytest, the referee, counts them: its `Module` fails once, however many
/// of these adapters load the file. So a file shows as many errors as the
/// adapter reporting the most for it (pytest's batch may report its doctest
/// collector's too); another adapter's beyond those becomes an info saying
/// so. It isn't dropped, so the log's `seq` keeps counting every record.
/// Collection's errors count too ([`Self::refined`]): aye's pytest
/// front-end lists every pytest file in refinement, so a file that fails
/// to import there has its error from collection, not from a batch.
#[derive(Debug, Default)]
struct OneModuleErrors {
    /// Each batch's adapter, from its `batch_started`.
    adapters: HashMap<aye_events::BatchIndex, AdapterName>,
    /// By file, how many collection errors each adapter reported for it,
    /// in the order the adapters first did.
    reported: HashMap<String, Vec<(AdapterName, usize)>>,
}

impl OneModuleErrors {
    /// A collection error collection reported (refinement's), counted as
    /// `adapter`'s: refinement doesn't say whose it is, and with the pytest
    /// adapter in the run, it lists every pytest file (the doctest
    /// adapter's errors, which pytest counts apart, then count as its too,
    /// so another adapter's batch error beyond them still becomes an info).
    /// A conftest's is [`OneConftestError`]'s.
    fn refined(&mut self, adapter: &AdapterName, diagnostic: &Diagnostic) {
        if diagnostic.severity != Severity::Error
            || diagnostic.code != DiagnosticCode::CollectionError
            || diagnostic.path.is_nowhere()
            || registry::is_conftest(diagnostic.path.as_str())
        {
            return;
        }
        let counts = self
            .reported
            .entry(diagnostic.path.to_string())
            .or_default();
        match counts.iter_mut().find(|(by, _)| by == adapter) {
            Some((_, count)) => *count += 1,
            None => counts.push((adapter.clone(), 1)),
        }
    }

    fn see(&mut self, record: &mut Record) {
        let batch = record.envelope.batch;
        match &mut record.event {
            Event::BatchStarted { adapter, .. } => {
                if let Some(batch) = batch {
                    self.adapters.insert(batch, adapter.clone());
                }
            }
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    && diagnostic.code == DiagnosticCode::CollectionError
                    && !diagnostic.path.is_nowhere() =>
            {
                let Some(adapter) = batch.and_then(|batch| self.adapters.get(&batch)) else {
                    return;
                };
                if !registry::collects_in_one_module(adapter) {
                    return;
                }
                let counts = self
                    .reported
                    .entry(diagnostic.path.to_string())
                    .or_default();
                let mine = if let Some((_, count)) = counts.iter_mut().find(|(by, _)| by == adapter)
                {
                    *count += 1;
                    *count
                } else {
                    counts.push((adapter.clone(), 1));
                    1
                };
                let mut others = counts.iter().filter(|(by, _)| by != adapter);
                let Some((first, _)) = others.clone().next() else {
                    return;
                };
                if others.all(|(_, count)| *count < mine) {
                    return;
                }
                diagnostic.severity = Severity::Info;
                diagnostic.message = format!(
                    "one collection error with the {first} adapter's for this file, as \
                     pytest collects the file once; the {adapter} adapter reported: {}",
                    diagnostic.message
                );
            }
            _ => {}
        }
    }
}

/// A conftest that fails to load is one error in pytest, which loads it
/// once per session, however many adapters load it: the doctest adapter
/// imports the conftests pytest would and words their failures as the
/// pytest adapter does, but a frame or a word apart would make two errors
/// of one. So a `usage-error` or `collection-error` on a `conftest.py`
/// (`registry::is_conftest`) that another adapter already reported with
/// that code, in refinement (where who reported it isn't known, and each
/// adapter reports one once) or in a batch, becomes an info saying so,
/// with its own words when they differ, kept in the stream as
/// [`OneModuleErrors`] keeps its. One adapter's own repeats (every pytest
/// batch reports an initial conftest's failure) stay errors, which
/// renderers show once.
#[derive(Debug, Default)]
struct OneConftestError {
    /// Each batch's adapter, from its `batch_started`.
    adapters: HashMap<aye_events::BatchIndex, AdapterName>,
    /// By code and path, who reported the error first (an adapter, or
    /// `None` for refinement), and its message.
    first: HashMap<(DiagnosticCode, String), (Option<AdapterName>, String)>,
}

impl OneConftestError {
    /// A record from a batch.
    fn see_record(&mut self, record: &mut Record) {
        let batch = record.envelope.batch;
        match &mut record.event {
            Event::BatchStarted { adapter, .. } => {
                if let Some(batch) = batch {
                    self.adapters.insert(batch, adapter.clone());
                }
            }
            Event::Diagnostic(diagnostic) => {
                let adapter = batch.and_then(|batch| self.adapters.get(&batch)).cloned();
                if adapter.is_some() {
                    self.see(adapter.as_ref(), diagnostic);
                }
            }
            _ => {}
        }
    }

    /// A diagnostic `adapter` reported, or refinement (`None`).
    fn see(&mut self, adapter: Option<&AdapterName>, diagnostic: &mut Diagnostic) {
        let kind = match diagnostic.code {
            DiagnosticCode::UsageError => "usage error",
            DiagnosticCode::CollectionError => "collection error",
            _ => return,
        };
        if diagnostic.severity != Severity::Error
            || !registry::is_conftest(diagnostic.path.as_str())
        {
            return;
        }
        let key = (diagnostic.code.clone(), diagnostic.path.to_string());
        let Some((first, message)) = self.first.get(&key) else {
            let first = (adapter.cloned(), diagnostic.message.clone());
            self.first.insert(key, first);
            return;
        };
        if first.is_some() && first.as_ref() == adapter {
            return;
        }
        let by = adapter.map_or_else(
            || "another adapter".to_string(),
            |adapter| format!("the {adapter} adapter"),
        );
        let words = if *message == diagnostic.message {
            String::new()
        } else {
            format!(", in its words: {}", diagnostic.message)
        };
        diagnostic.severity = Severity::Info;
        diagnostic.message =
            format!("also reported by {by}, one {kind} as pytest loads a conftest once{words}");
    }
}

/// What the positional arguments reach when some of them aren't paths
/// (dotted names, keys): collection then walks the whole project to
/// resolve them, and refinement reports on all of it, its sessions
/// starting from the framework's default (the context the adapters
/// prepared saw only the path arguments). A diagnostic about a place the
/// user's run never meets is not the run's: `pytest <the paths and the
/// selected files>` wouldn't report it. With no positional argument, or
/// only paths, collection walked only what the run reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reach {
    /// The path arguments, absolute, spelled from the root.
    paths: Vec<PathBuf>,
    /// The paths the user's run starts from: each path argument, and the
    /// files of the collected cases each other argument selects, in the
    /// order of the arguments, each once
    /// ([`aye_protocol::BatchContext::selected_paths`]).
    selected: Vec<PathBuf>,
    /// The arguments that aren't paths, as given: dotted names and keys.
    words: Vec<String>,
    /// Whether some argument named nothing: an error collection reported
    /// elsewhere may be why, and is kept.
    unmatched: bool,
}

impl Reach {
    /// `None` when every positional argument is a path, or there is none.
    fn of(positionals: &[Positional], root: &Path, cases: &[Case]) -> Option<Self> {
        if positionals.iter().all(|p| p.path.is_some()) {
            return None;
        }
        let at_root = |relative: &str| normalize_absolute(&root.join(relative));
        let mut paths = Vec::new();
        let mut words = Vec::new();
        let mut selected: Vec<PathBuf> = Vec::new();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        for positional in positionals {
            if let Some(path) = &positional.path {
                let path = at_root(&root_relative(path, root));
                paths.push(path.clone());
                if seen.insert(path.clone()) {
                    selected.push(path);
                }
                continue;
            }
            words.push(positional.selector.clone());
            let one = Selection::default().with_ids([positional.selector.as_str()]);
            for case in cases {
                if one.truth(case).is_false() {
                    continue;
                }
                let file = at_root(case.id.path().as_str());
                if seen.insert(file.clone()) {
                    selected.push(file);
                }
            }
        }
        Some(Self {
            paths,
            words,
            selected,
            unmatched: false,
        })
    }

    /// Whether a diagnostic collection reported is the run's. A `skipped`
    /// info (a module or directory the framework skipped, which the
    /// summaries count as a skip) is when a path argument holds it, or lies
    /// in it, or a dotted name reaches it ([`dotted_reach`]): what keys
    /// select are tests, and a skipped module has none. A `usage-error` on
    /// a file (a conftest that failed at start-up in refinement's session)
    /// is when it lies in a path argument, or its directory holds a path
    /// the run starts from (as `pytest <those paths>` loads it first), or
    /// some argument named nothing. Everything else is the run's.
    fn keeps(&self, root: &Path, diagnostic: &Diagnostic) -> bool {
        let at = || normalize_absolute(&root.join(diagnostic.path.as_str()));
        match (diagnostic.severity, &diagnostic.code) {
            (Severity::Info, DiagnosticCode::Skipped) => {
                let at = at();
                self.paths
                    .iter()
                    .any(|path| at.starts_with(path) || path.starts_with(&at))
                    || self
                        .words
                        .iter()
                        .any(|word| dotted_reach(word, diagnostic.path.as_str()) != Named::Nothing)
            }
            (Severity::Error, DiagnosticCode::UsageError) if !diagnostic.path.is_nowhere() => {
                if self.unmatched {
                    return true;
                }
                let at = at();
                let dir = if at.is_dir() {
                    at.as_path()
                } else {
                    at.parent().unwrap_or(&at)
                };
                self.paths.iter().any(|path| at.starts_with(path))
                    || self.selected.iter().any(|path| path.starts_with(dir))
            }
            _ => true,
        }
    }
}

/// The directories of the conftests collection reported an error for (a
/// `conftest.py` that failed to load, `registry::is_conftest`), relative to
/// the root, each with its trailing `/` (the root's is empty).
fn broken_conftest_dirs(diagnostics: &[Diagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error && registry::is_conftest(d.path.as_str()))
        .map(|d| &d.path.as_str()[..d.path.as_str().len() - "conftest.py".len()])
        .collect()
}

/// Whether a diagnostic is collection's `no-tests` note on a file below a
/// conftest that failed to load (`broken`, from [`broken_conftest_dirs`]):
/// the framework found no tests there because of it, which the conftest's
/// error says (an initial one's session runs without the tests below it;
/// another fails its directory), so the note, which says only that the file
/// has none, is left out.
fn explained_by_a_conftest(broken: &[&str], diagnostic: &Diagnostic) -> bool {
    diagnostic.severity == Severity::Info
        && diagnostic.code == DiagnosticCode::NoTests
        && broken
            .iter()
            .any(|dir| diagnostic.path.as_str().starts_with(dir))
}

/// What a dotted name names of a path collection reported skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Nothing,
    /// Something the skip holds or that holds it, but not the name's
    /// whole: tests of a skipped module (`mod.test_x`), a module in a
    /// skipped directory. pytest counts the skip and says it found no such
    /// test.
    Part,
    /// The skipped module or package itself, or a package holding it:
    /// pytest counts the skip, and nothing is missing.
    Whole,
}

/// How the dotted name `word` (`pkg.mod.Class.test`, read from the root as
/// `aye_select::Selector` reads one: `pkg/mod.py` then the names in it, or
/// the package `pkg/mod`; `__init__` left out) reaches `path`, which
/// collection reported skipped (a module, or a directory a conftest
/// skipped), relative to the root. A word that isn't a dotted name (a key,
/// an id) names nothing.
fn dotted_reach(word: &str, path: &str) -> Named {
    if word.contains("::") || word.contains('/') {
        return Named::Nothing;
    }
    let parts: Vec<&str> = word.split('.').filter(|part| *part != "__init__").collect();
    if parts.is_empty() || parts.iter().any(|part| part.is_empty()) {
        return Named::Nothing;
    }
    let inside = |inner: &str, outer: &str| {
        inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    let mut named = Named::Nothing;
    for k in 1..=parts.len() {
        let base = parts[..k].join("/");
        let module = format!("{base}.py");
        let whole = k == parts.len();
        if whole && (path == base || path == module || inside(path, &base)) {
            return Named::Whole;
        }
        if path == module || inside(&module, path) || (whole && inside(&base, path)) {
            named = Named::Part;
        }
    }
    named
}

/// The reporters, history, the run store, the stamper for the binary's own
/// records, and the errors the stream has carried so far.
struct Stream<'a> {
    reporters: Tee<'a>,
    /// History: what each bad outcome says of the test's earlier runs, and
    /// a consumer of every record after the reporters.
    history: Option<Remember<'a>>,
    /// Keeps the run's log for `aye show`: the last consumer of each
    /// record.
    store: Keep,
    stamper: Stamper,
    errors: Errors,
    started: Instant,
    printer: Printer,
}

impl Stream<'_> {
    fn emit(&mut self, event: Event) {
        let mut record = self.stamper.stamp(Record::new(event));
        Self::deliver_to(
            &mut self.reporters,
            &mut self.history,
            &mut self.store,
            &mut self.errors,
            self.printer,
            &mut record,
        );
    }

    /// A bad outcome first gets the test's `history` (how it ended before
    /// this run). Then every reporter gets every record: one that fails
    /// (stdout, a closed pipe) stops writing, and the others carry on.
    /// Then history folds it, and the run store keeps it.
    fn deliver_to(
        reporters: &mut Tee<'_>,
        history: &mut Option<Remember<'_>>,
        store: &mut Keep,
        errors: &mut Errors,
        printer: Printer,
        record: &mut Record,
    ) {
        if let Some(history) = history.as_ref() {
            history.tell(record);
        }
        if let Event::Diagnostic(d) = &record.event {
            errors.see(d, DiagnosticOrigin::of(&record.envelope));
        }
        if let Err(err) = reporters.report(record)
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            printer.note(format_args!("cannot write output: {err}"));
        }
        if let Some(history) = history.as_mut() {
            history.history.record(record);
        }
        store.record(record);
    }

    fn finish(
        mut self,
        code: ExitCode,
        counts: Counts,
        stopped: Option<StopReason>,
        file_failed: &AtomicBool,
    ) -> RunEnd {
        self.emit(Event::RunFinished {
            exit_code: code,
            counts,
            duration: self.started.elapsed(),
            stopped,
        });
        if let Err(err) = self.reporters.finish()
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            self.printer
                .note(format_args!("cannot write output: {err}"));
        }
        if let Some(history) = &self.history {
            history.save(self.printer);
        }
        // Kept for `aye show`; never fails the run.
        let runs = self.store.dir().to_path_buf();
        if let Err(err) = self.store.finish() {
            self.printer.warning(format_args!(
                "could not keep this run for `aye show` in {}: {err}",
                runs.display()
            ));
        }
        RunEnd {
            code,
            report_failed: file_failed.load(Ordering::Relaxed),
        }
    }
}

/// The run store's writer, and what the run's summary counts beside the
/// tests' counts, which it keeps beside the log (`aye show --runs` words
/// them as the summary did).
struct Keep {
    writer: RunWriter,
    tally: Tally,
}

impl Keep {
    fn new(writer: RunWriter) -> Self {
        Self {
            writer,
            tally: Tally::default(),
        }
    }

    fn record(&mut self, record: &Record) {
        self.tally.note(record);
        self.writer.record(record);
    }

    fn dir(&self) -> &Path {
        self.writer.dir()
    }

    fn finish(self) -> io::Result<Option<StoredRunId>> {
        self.writer.finish(self.tally.extra().named())
    }
}

/// A reporter writing to stdout. After its first error (a closed pipe) it
/// writes nothing more, so the error is reported once and every other
/// reporter still sees the whole stream.
struct Stdout<R> {
    inner: R,
    failed: bool,
}

impl<R> Stdout<R> {
    const fn new(inner: R) -> Self {
        Self {
            inner,
            failed: false,
        }
    }
}

impl<R: Reporter> Reporter for Stdout<R> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner
            .report(record)
            .inspect_err(|_| self.failed = true)
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.failed {
            return Ok(());
        }
        self.inner.finish().inspect_err(|_| self.failed = true)
    }
}

/// A reporter writing a file when the stream ends: its error is reported
/// here, naming the file, and remembered for the exit code.
struct File<R> {
    inner: R,
    failed: Arc<AtomicBool>,
    printer: Printer,
}

impl<R: Reporter> Reporter for File<R> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        self.inner.report(record)
    }

    fn finish(&mut self) -> io::Result<()> {
        if let Err(err) = self.inner.finish() {
            self.printer.error(err);
            self.failed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// History as a consumer of the stream, saved in `dir` at the end; and,
/// before anything else sees a bad outcome, what it says of the test's
/// earlier runs.
struct Remember<'a> {
    history: &'a mut History,
    dir: PathBuf,
}

impl Remember<'_> {
    /// Sets a bad outcome's `history`, from history as it was before this
    /// run, when the test has some.
    fn tell(&self, record: &mut Record) {
        if let Event::TestFinished(result) = &mut record.event
            && result.outcome.is_bad()
            && result.history.is_none()
        {
            result.history = self.history.previous(&result.id);
        }
    }

    fn save(&self, printer: Printer) {
        if self.history.changed()
            && let Err(err) = self.history.save(&self.dir)
        {
            printer.warning(format_args!(
                "could not save history to {}: {err}",
                self.dir.display()
            ));
        }
    }
}

/// The ids with parameters the command line names (`path::test[x]`),
/// relative to the root as collection spells them, each once
/// ([`aye_protocol::BatchContext::requested_ids`]).
fn requested_ids(positionals: &[Positional]) -> Vec<TestId> {
    let mut seen = HashSet::new();
    positionals
        .iter()
        .filter(|p| p.id && p.path.is_some())
        .filter(|p| {
            p.selector
                .split_once("::")
                .is_some_and(|(_, rest)| rest.contains('['))
        })
        .map(|p| p.selector.clone())
        .filter(|id| seen.insert(id.clone()))
        .map(TestId::new)
        .collect()
}

/// By adapter, the files collection found its tests in (selected or not),
/// in collection order: the session every worker of an adapter whose
/// executor lists the session lists before its first batch
/// ([`Plan::session_files`]). Read from the cases, so a claimed file with
/// none (one that failed to import) is left out. With dotted names or keys
/// among the arguments, collection walked the whole project to resolve
/// them, but the user's run is `pytest <the paths it starts from>`
/// ([`Reach::selected`], absolute, with the root, as `within`): only the
/// files in those paths are its session, and listing the others would load
/// conftests that run never loads (a `test*` directory's, whose hooks
/// reach every test).
fn session_files(
    cases: &[Case],
    within: Option<(&[PathBuf], &Path)>,
) -> BTreeMap<AdapterName, Vec<ProjectPath>> {
    let mut files: BTreeMap<AdapterName, Vec<ProjectPath>> = BTreeMap::new();
    let mut seen: HashSet<(&AdapterName, &str)> = HashSet::new();
    for case in cases {
        let path = case.id.path();
        let reached = within.is_none_or(|(paths, root)| {
            let file = normalize_absolute(&root.join(path.as_str()));
            paths.iter().any(|start| file.starts_with(start))
        });
        if reached && seen.insert((&case.adapter, path.as_str())) {
            files
                .entry(case.adapter.clone())
                .or_default()
                .push(path.to_project_path());
        }
    }
    files
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

/// Where the project root lies in the repository, for GitHub annotations:
/// relative to `GITHUB_WORKSPACE` when the root is inside it, else to the
/// nearest directory above the root holding `.git`; empty at the top.
fn repository_prefix(root: &Path, workspace: Option<&Path>) -> String {
    let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = real(root);
    let inside = |top: &Path| {
        root.strip_prefix(real(top)).ok().map(|rest| {
            rest.components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        })
    };
    workspace
        .filter(|w| !w.as_os_str().is_empty())
        .and_then(inside)
        .or_else(|| {
            root.ancestors()
                .find(|dir| dir.join(".git").exists())
                .and_then(inside)
        })
        .unwrap_or_default()
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

/// When collection reports its progress: not before it has taken half a
/// second, so that a quick collection sends nothing, then at most four times
/// a second.
struct Pace {
    started: Instant,
    last: Option<Instant>,
}

impl Pace {
    const AFTER: Duration = Duration::from_millis(500);
    const EVERY: Duration = Duration::from_millis(250);

    const fn new(started: Instant) -> Self {
        Self {
            started,
            last: None,
        }
    }

    fn due(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.started) < Self::AFTER
            || self
                .last
                .is_some_and(|last| now.duration_since(last) < Self::EVERY)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// How long a stop signal before the run (while collecting) may take to
/// end aye cleanly before it exits anyway: whatever runs then (a walk, an
/// import in a worker) may not be watching.
const COLLECTING_GRACE: Duration = Duration::from_millis(1500);

/// Ctrl-C (or SIGTERM, or SIGHUP) stops the run cleanly; a second one
/// kills every worker, with the processes its tests started, and exits at
/// once. A signal within [`REPEAT_WINDOW`] of the first is that one
/// delivered again, not a second request (see [`Presses`]).
///
/// Before the run, a stop signal stops collection (`stop` the collector
/// watches), and aye exits anyway if that takes too long; during the run
/// (`running`), it stops the run cleanly.
#[expect(
    clippy::exit,
    reason = "a second signal, or a collection that doesn't stop in time, ends aye at once"
)]
fn install_interrupt(
    stop: aye_runner::StopHandle,
    collecting: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    printer: Printer,
) {
    let mut presses = Presses::default();
    let _ = ctrlc::set_handler(move || match presses.press(Instant::now()) {
        Press::Stop => {
            collecting.store(true, Ordering::SeqCst);
            stop.stop();
            if !running.load(Ordering::SeqCst) {
                let stop = stop.clone();
                let running = Arc::clone(&running);
                std::thread::spawn(move || {
                    std::thread::sleep(COLLECTING_GRACE);
                    if !running.load(Ordering::SeqCst) {
                        stop.kill();
                        printer.interrupted();
                        std::process::exit(i32::from(u8::from(ExitCode::Interrupted)));
                    }
                });
            }
        }
        Press::Repeat => {}
        Press::Kill => {
            stop.kill();
            std::process::exit(i32::from(u8::from(ExitCode::Interrupted)));
        }
    });
}

/// How long after the first stop signal another one is the same request
/// delivered twice: GNU `timeout` signals aye and then its own process
/// group, aye included, and a terminal and a wrapper passing signals on
/// (`uv run`) may each deliver one Ctrl-C. Shorter than a person takes to
/// see that the run is still stopping and ask again.
const REPEAT_WINDOW: Duration = Duration::from_millis(500);

/// What a stop signal means, by when it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Press {
    /// The first: stop the run cleanly.
    Stop,
    /// Within [`REPEAT_WINDOW`] of the first: the same request again.
    Repeat,
    /// Later: a second request, to stop at once.
    Kill,
}

/// The stop signals seen so far.
#[derive(Debug, Default)]
struct Presses {
    first: Option<Instant>,
}

impl Presses {
    fn press(&mut self, now: Instant) -> Press {
        match self.first {
            None => {
                self.first = Some(now);
                Press::Stop
            }
            Some(first) if now.saturating_duration_since(first) < REPEAT_WINDOW => Press::Repeat,
            Some(_) => Press::Kill,
        }
    }
}

#[cfg(test)]
mod tests;
