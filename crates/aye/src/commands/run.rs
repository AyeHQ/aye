//! One run, from its settings ([`RunSettings`]): collection, selection, the
//! runner and the reporters, wired around the event stream.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use aye_adapter::Project;
use aye_collect::{Collector, Options};
use aye_config::OutputFormat;
use aye_events::{
    AdapterName, Case, Counts, Diagnostic, DiagnosticCode, DiagnosticPath, Event, ExitCode,
    ProjectPath, RunId, SCHEMA_VERSION, Severity, Stamper, StopReason, TestId,
};
use aye_fs::{StateBucket, StateDir, normalize_absolute, root_relative};
use aye_history::{History, RunWriter};
use aye_protocol::Refiner;
use aye_provision::EnvironmentFingerprint;
use aye_runner::{Echo, Plan, Runner, RunnerOptions};
use aye_select::Selection;
use aye_static::EnvVars;

use crate::error::AyeError;
use crate::positional::Positional;
use crate::printer::Printer;
use crate::registry;
use crate::settings::RunSettings;
use crate::stream::{Here, Keep, OneConftestError, OneModuleErrors, Remember, Stream, reporters};

mod exit;

pub(crate) use exit::{Errors, RunEnd};
use exit::{RunStage, RunVerdict};

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
