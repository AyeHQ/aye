//! One run: configuration, collection, selection, the runner and the
//! reporters, wired around the event stream.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, IsTerminal as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use aye_adapter::{Adapter, FrameworkDefaults, Project};
use aye_collect::{Collector, Environment, Options, Prepared};
use aye_config::{AyeConfig, WorkerCount};
use aye_events::{
    AdapterName, Case, Counts, Diagnostic, DiagnosticCode, DiagnosticOrigin, DiagnosticPath, Event,
    ExitCode, ProjectPath, Record, RerunCommand, RunId, SCHEMA_VERSION, Severity, Stamper,
    StopReason, TestId,
};
use aye_fs::{StateBucket, StateDir, normalize_absolute, relative_below_root, root_relative};
use aye_history::{History, RunWriter, StoredRunId};
use aye_protocol::{MaxFail, Policy, Refiner};
use aye_render::{
    ColorChoice, GithubAnnotations, JsonLines, Junit, JunitDuration, JunitFamily, JunitLogging,
    JunitOptions, Reporter, Tally, Tee, Text, TextOptions,
};
use aye_runner::{Echo, Plan, Runner, RunnerOptions};
use aye_select::Selection;
use aye_static::{EnvVars, ProcessEnv};

use crate::args::{Args, Color, Maxfail, Output, Passed, passed, passed_maxfail};
use crate::busy::Busy;
use crate::registry;

/// A run's inputs, resolved: everything wrong with the command line or the
/// configuration fails here, before anything is reported. Selectors that
/// match nothing are found after collection, and reported in the stream.
pub(crate) struct Session {
    args: Args,
    cwd: PathBuf,
    root: PathBuf,
    /// Each positional argument, resolved.
    positionals: Vec<Positional>,
    /// Positional arguments that are paths: as given, or absolute when only
    /// the project root makes sense of them.
    paths: Vec<PathBuf>,
    /// Whether every positional argument is a path (collection can then be
    /// limited to them).
    only_paths: bool,
    config: AyeConfig,
    /// The adapters the run uses, in priority order, and why.
    adapters: Vec<Box<dyn Adapter>>,
    adapters_why: registry::Why,
    prepared: Vec<Prepared>,
    /// What each prepared adapter's framework says about the run, in the
    /// same order.
    defaults: Vec<(AdapterName, FrameworkDefaults)>,
    selection: Selection,
    python: PathBuf,
    python_source: aye_runner::PythonSource,
    workers: u32,
    /// The environment aye started in, read once: what the adapters, the
    /// interpreter's discovery and the reporters read.
    env: ProcessEnv,
}

/// A positional argument and how it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Positional {
    /// What selection gets: relative to the root, with `/` separators.
    pub(crate) selector: String,
    /// For a path or an id, its file or directory, absolute and normalized.
    pub(crate) path: Option<PathBuf>,
    /// Whether it has a `::` part.
    pub(crate) id: bool,
}

impl Positional {
    fn exists(&self) -> bool {
        self.path.as_deref().is_some_and(Path::exists)
    }
}

impl Session {
    pub(crate) fn new(args: Args, env: ProcessEnv) -> Result<Self> {
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        // `.` and `..` are resolved lexically, as pytest's `os.path.abspath`
        // does, before the root is searched for.
        let given: Vec<PathBuf> = args
            .tests
            .iter()
            .map(|test| path_part(test))
            .filter(|path| is_path_like(&cwd, path))
            .map(|path| normalize_absolute(&cwd.join(path)))
            .collect();
        let root = normalize_absolute(&aye_config::find_root(&aye_fs::common_ancestor(
            &cwd, &given,
        )));
        let positionals: Vec<Positional> = args
            .tests
            .iter()
            .map(|test| resolve(&cwd, &root, test))
            .collect();
        let mut paths = Vec::new();
        let mut only_paths = true;
        for (test, positional) in args.tests.iter().zip(&positionals) {
            let Some(path) = &positional.path else {
                only_paths = false;
                continue;
            };
            let as_given = path_part(test);
            if normalize_absolute(&cwd.join(as_given)) == *path {
                paths.push(PathBuf::from(as_given));
            } else {
                paths.push(path.clone());
            }
        }

        let config = AyeConfig::load(&root)?;
        let (python, python_source) = aye_runner::find_python(
            args.python.as_deref(),
            config.settings.python.as_deref(),
            &root,
            &cwd,
            &env,
        )
        .context("no Python interpreter found: pass --python, or set [tool.aye] python")?;
        let project = Project {
            root: &root,
            cwd: &cwd,
            args: &paths,
            config_file: args.config_file.as_deref(),
            framework_args: &args.passthrough,
            env: &env,
            tool_aye: &config.settings,
        };
        // Before anything walks the tree: pytest would walk all of it, and
        // nobody means that.
        if paths.is_empty()
            && let Some(whole) = everything(&cwd)
        {
            anyhow::bail!(
                "with no path given, aye would collect {whole}. Run aye inside a project, \
                 or name what to test (aye tests/)"
            );
        }
        // Choosing and preparing the adapters can walk the project; on a
        // terminal, say so if it takes a while.
        let text = output(&args, &config).0 == Output::Text;
        let busy = Busy::start(text && io::stdout().is_terminal());
        // `--co --no-refine` lists without importing: pytest's cases are the
        // static recognizer's prediction.
        let static_listing = args.no_refine && args.collect_only;
        let chosen = registry::choose(&project, static_listing)?;
        let adapters_why = chosen.why;
        let adapters = chosen.adapters;
        // A run imports every pytest file it runs: aye's pytest front-end
        // lists each one, so there is no static run to fall back on.
        if args.no_refine
            && !args.collect_only
            && adapters
                .iter()
                .any(|adapter| adapter.name() == aye_adapter_pytest::NAME)
        {
            anyhow::bail!(
                "--no-refine lists tests without importing them, with --co; a run of \
                 pytest's tests imports every file it runs (aye's pytest front-end reads \
                 each one), so it can't do without"
            );
        }
        let prepared = prepare(&adapters, &project)?;
        let defaults: Vec<(AdapterName, FrameworkDefaults)> = prepared
            .iter()
            .map(|(name, recognizer)| (name.clone(), recognizer.defaults()))
            .collect();

        let selection = selection(
            &args,
            positionals.iter().map(|p| p.selector.as_str()),
            &defaults,
        )?;

        let workers = match (args.workers, config.settings.workers) {
            (Some(w), _) => w.resolve(),
            (None, Some(aye_config::Workers::Count(n))) => n.max(1),
            (None, Some(aye_config::Workers::Keyword(_))) => WorkerCount::Auto.resolve(),
            (None, None) => first(&defaults, |d| d.workers).map_or(1, WorkerCount::resolve),
        };

        drop(busy);
        Ok(Self {
            args,
            cwd,
            root,
            positionals,
            paths,
            only_paths,
            config,
            adapters,
            adapters_why,
            prepared,
            defaults,
            selection,
            python,
            python_source,
            workers,
            env,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn python(&self) -> &Path {
        &self.python
    }

    pub(crate) const fn python_source(&self) -> aye_runner::PythonSource {
        self.python_source
    }

    pub(crate) const fn workers(&self) -> u32 {
        self.workers
    }

    pub(crate) const fn config(&self) -> &AyeConfig {
        &self.config
    }

    pub(crate) fn prepared(&self) -> &[Prepared] {
        &self.prepared
    }

    /// The names of the adapters the run uses, in priority order, and why
    /// those.
    pub(crate) fn adapters(&self) -> (Vec<AdapterName>, registry::Why) {
        (
            self.adapters.iter().map(|a| a.name()).collect(),
            self.adapters_why,
        )
    }

    #[expect(
        clippy::print_stderr,
        reason = "aye's own messages, outside the event stream"
    )]
    pub(crate) fn run(mut self) -> ExitCode {
        let started = Instant::now();
        // A report that can't be written fails before anything runs, not
        // after the whole suite.
        let junit = self.junit();
        if let Some(path) = &junit
            && let Err(err) = writable(path)
        {
            eprintln!(
                "aye: error: cannot write the JUnit XML report to {}: {err}",
                path.display()
            );
            return ExitCode::UsageError;
        }
        let state = StateDir::for_root(&self.root);
        let mut history = History::load(state.path());
        let durations = history.durations();
        let last_failed = history.last_failed();
        let mut selection = std::mem::take(&mut self.selection);
        if self.args.last_failed {
            selection = selection.with_last_failed(last_failed.iter().cloned());
        }

        let run_id = RunId::random();
        let runner = Runner::new(RunnerOptions {
            python: self.python.clone(),
            root: self.root.clone(),
            cwd: self.cwd.clone(),
            workers: self.workers,
            executors: self.adapters.iter().map(|a| a.executor()).collect(),
            // pytest-xdist's `testrun_uid`, which its workers share: one per
            // run, the run's id.
            env: vec![(
                EnvVars::PYTEST_XDIST_TESTRUNUID.into(),
                run_id.as_str().into(),
            )],
            // `-s` output stays off a machine-readable stdout.
            echo: match self.output() {
                Output::Text => Echo::Stdout,
                Output::Json => Echo::Stderr,
            },
            collect_only: self.args.collect_only,
        });
        // Ctrl-C stops collection as well as the run: the walk of a big tree
        // and refinement can take a while, and must not ignore it.
        let interrupted = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(false));
        install_interrupt(
            runner.stop_handle(),
            Arc::clone(&interrupted),
            Arc::clone(&running),
        );

        let file_failed = Arc::new(AtomicBool::new(false));
        // The run's number in the store (`r81`), which `run_started` says,
        // when the run will be stored.
        let mut writer = RunWriter::new(
            state.bucket(StateBucket::Runs),
            self.config.settings.keep_runs(),
        );
        let number = if self.args.collect_only {
            None
        } else {
            writer.claim()
        };
        let mut out = Stream {
            reporters: self.reporters(junit, &file_failed),
            history: Some(Remember {
                history: &mut history,
                dir: state.path().to_path_buf(),
            }),
            store: Keep::new(writer),
            stamper: Stamper::with_run(run_id),
            errors: Errors::default(),
            started,
        };

        out.emit(Event::RunStarted {
            schema_version: SCHEMA_VERSION,
            aye_version: env!("CARGO_PKG_VERSION").to_string(),
            root: self.root.display().to_string(),
            collect_only: self.args.collect_only,
            workers: self.workers,
            run: number,
        });

        // -- collect and select ---------------------------------------------
        out.emit(Event::CollectionStarted);
        let collect_started = Instant::now();
        let project = Project {
            root: &self.root,
            cwd: &self.cwd,
            args: &self.paths,
            config_file: self.args.config_file.as_deref(),
            framework_args: &self.args.passthrough,
            env: &self.env,
            tool_aye: &self.config.settings,
        };
        // The refinement cache is keyed on what refining depends on: the
        // interpreter and its packages, the arguments after `--`, and the
        // environment workers inherit (aye's own; `PYTEST_XDIST_TESTRUNUID`
        // changes every run and refinement has no use for it).
        let collector = Collector::new(
            project,
            &self.prepared,
            Options {
                strict: self.args.strict,
                cache_dir: Some(state.bucket(StateBucket::Cache)),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
        .with_environment(Environment {
            interpreter: Some(self.python.clone()),
            framework_args: self.args.passthrough.clone(),
            variables: None,
        })
        .with_stop(Arc::clone(&interrupted));
        let targets: Vec<PathBuf> = if self.only_paths {
            self.paths.clone()
        } else if self.paths.is_empty() {
            Vec::new()
        } else {
            // Paths mixed with dotted names or keys: the paths, and wherever
            // the adapters look when no path is given, for the rest.
            let bare = Project {
                args: &[],
                ..project
            };
            let mut targets: Vec<PathBuf> = self
                .prepared
                .iter()
                .flat_map(|(_, recognizer)| recognizer.roots(&bare))
                .filter(|root| root.exists())
                .collect();
            targets.extend(self.paths.iter().cloned());
            targets
        };
        let refiner: Option<&(dyn Refiner + Sync)> = if self.args.no_refine {
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
            return out.finish(
                ExitCode::Interrupted,
                Counts::default(),
                Some(StopReason::Interrupted),
                &file_failed,
            );
        }
        // Dotted names and keys are resolved among what collection found,
        // over the whole project: what the run reaches decides which of
        // collection's diagnostics are the run's (see `Reach`).
        let reach = Reach::of(&self.positionals, &self.root, &collection.cases);
        let session_files = session_files(
            &collection.cases,
            reach
                .as_ref()
                .map(|reach| (reach.selected.as_slice(), self.root.as_path())),
        );
        let selected = selection.apply(collection.cases);
        let kept = |diagnostic: &Diagnostic| {
            reach
                .as_ref()
                .is_none_or(|reach| reach.keeps(&self.root, diagnostic))
        };
        // The modules and directories skipped while collecting that the run
        // reaches: a dotted name for one names no test, but pytest found it.
        let skips: Vec<&str> = collection
            .diagnostics
            .iter()
            .filter(|d| {
                d.code == DiagnosticCode::Skipped && d.severity == Severity::Info && kept(d)
            })
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
                let positional = self.positionals.iter().find(|p| p.selector == *selector)?;
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
        let pytest_runs = self
            .prepared
            .iter()
            .any(|(name, _)| *name == aye_adapter_pytest::NAME);
        let mut collection_errors = false;
        let broken = broken_conftest_dirs(&collection.diagnostics);
        for diagnostic in &collection.diagnostics {
            if reach
                .as_ref()
                .is_some_and(|reach| !reach.keeps(&self.root, diagnostic))
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
                .map(|path| root_relative(path, &self.root))
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
        if self.args.failed_first && !last_failed.is_empty() {
            cases.sort_by_key(|case| !last_failed.contains(&case.id));
        }
        for case in &cases {
            out.emit(Event::CaseCollected(Box::new(case.clone())));
        }
        out.emit(Event::CollectionFinished {
            selected: u32::try_from(cases.len()).unwrap_or(u32::MAX),
            deselected: selected.deselected,
            refined_files: collection.refined_files,
            duration: collect_started.elapsed(),
        });

        let counts = Counts {
            deselected: selected.deselected,
            ..Counts::default()
        };
        if internal_error {
            return out.finish(ExitCode::InternalError, counts, None, &file_failed);
        }
        if usage_error {
            return out.finish(ExitCode::UsageError, counts, None, &file_failed);
        }
        // pytest counts each collection error as a failure: it stops the run
        // (2), or, with `--continue-on-collection-errors`, fails it (1)
        // whatever the tests do, `--co` included.
        let continuing = self.args.continue_on_collection_errors;
        if self.args.collect_only {
            let code = if collection_errors && !continuing {
                ExitCode::Interrupted
            } else if collection_errors {
                ExitCode::TestsFailed
            } else if cases.is_empty() {
                ExitCode::NoTests
            } else {
                ExitCode::Ok
            };
            return out.finish(code, counts, None, &file_failed);
        }
        if collection_errors && !continuing {
            return out.finish(ExitCode::Interrupted, counts, None, &file_failed);
        }
        if cases.is_empty() {
            let code = if collection_errors {
                ExitCode::TestsFailed
            } else {
                ExitCode::NoTests
            };
            return out.finish(code, counts, None, &file_failed);
        }

        // -- run ----------------------------------------------------------------
        let rerun_args = rerun_args(&self.args);
        let plan = Plan {
            cases,
            policy: self.policy(),
            contexts: self
                .prepared
                .iter()
                .map(|(name, r)| (name.clone(), r.context()))
                .collect(),
            requested_ids: requested_ids(&self.positionals),
            rerun_args: rerun_args.clone(),
            selected_paths: reach
                .map(|reach| reach.selected)
                .unwrap_or_default()
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            durations,
            first: if self.args.failed_first {
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
                cwd: &self.cwd,
                root: &self.root,
                rerun_args: &rerun_args,
            };
            running.store(true, Ordering::SeqCst);
            runner.run(&plan, stamper, &mut |mut record| {
                here.rerun_from_here(&mut record);
                once.see(&mut record);
                conftests.see_record(&mut record);
                Stream::deliver_to(reporters, history, store, errors, &mut record);
            })
        };
        let counts = Counts {
            deselected: selected.deselected,
            ..summary.counts
        };
        let stopped = summary.stopped.clone();
        let mut errors = out.errors;
        // Collection's errors, which `--continue-on-collection-errors` let
        // the run go past, still fail it.
        errors.any |= collection_errors;
        let code = run_exit_code(&summary, errors, &counts, continuing);
        out.finish(code, counts, stopped, &file_failed)
    }

    fn policy(&self) -> Policy {
        Policy {
            maxfail: self.maxfail(),
            capture: !self.args.no_capture,
            verbosity: self.args.framework_verbosity(),
            timeout: self.config.settings.timeout(),
            keyword: self.args.keyword.clone(),
            tags: self.args.tags.clone(),
            framework_args: self.args.passthrough.clone(),
        }
    }

    /// The last of `-x` and `--maxfail`: the arguments after `--` come last
    /// in the framework's arguments, so theirs wins over the command line's.
    /// Else the framework's own setting (pytest's `addopts`,
    /// `PYTEST_ADDOPTS`); 0 means no limit.
    fn maxfail(&self) -> Option<u32> {
        maxfail(&self.args, &self.defaults)
    }

    /// Where the JUnit XML report goes: `--junitxml`, read as pytest reads
    /// it (variables and `~` expanded), else the framework's own (the path
    /// pytest's `--junitxml` in addopts, `PYTEST_ADDOPTS` or the arguments
    /// after `--` gives, from the adapter's context). Relative to the
    /// current directory; empty means none.
    fn junit(&self) -> Option<PathBuf> {
        junit_path(
            &self.env,
            &self.cwd,
            self.args.junit_xml.as_deref(),
            &self.defaults,
        )
    }

    /// `--output`, else `[tool.aye] output`, else text.
    fn output(&self) -> Output {
        output(&self.args, &self.config).0
    }

    /// The output format, at its verbosity (`-v` and `-q`), and where the
    /// format came from, for `aye config`.
    pub(crate) fn output_shown(&self) -> (String, &'static str) {
        let (format, origin) = output(&self.args, &self.config);
        let verbosity = self.args.verbosity();
        let flags = match verbosity {
            ..0 => " -q".to_string(),
            0 => String::new(),
            n => format!(" -{}", "v".repeat(usize::from(n.unsigned_abs()))),
        };
        (format!("{}{flags}", format.name()), origin)
    }

    fn reporters<'a>(&self, junit: Option<PathBuf>, file_failed: &Arc<AtomicBool>) -> Tee<'a> {
        let mut tee = Tee::new();
        let verbosity = self.args.verbosity();
        match self.output() {
            Output::Text => {
                let color = match self.args.color {
                    Color::Auto => ColorChoice::Auto,
                    Color::Always => ColorChoice::Always,
                    Color::Never => ColorChoice::Never,
                };
                tee.push(Stdout::new(Text::new(
                    io::stdout(),
                    TextOptions {
                        verbosity,
                        color,
                        live: io::stdout().is_terminal(),
                    },
                )));
                if aye_render::github_actions_enabled(&self.env) {
                    let workspace = self
                        .env
                        .get_os(EnvVars::GITHUB_WORKSPACE)
                        .map(PathBuf::from);
                    let prefix = repository_prefix(&self.root, workspace.as_deref());
                    tee.push(Stdout::new(
                        GithubAnnotations::new(io::stdout()).with_path_prefix(prefix),
                    ));
                }
            }
            Output::Json => tee.push(Stdout::new(JsonLines::new(io::stdout(), verbosity))),
        }
        if let Some(path) = junit {
            tee.push(File {
                inner: Junit::new(path)
                    .with_prefix(junit_prefix(&self.defaults))
                    .with_options(junit_options(&self.defaults)),
                failed: Arc::clone(file_failed),
            });
        }
        tee
    }
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

/// Error diagnostics seen in the stream, by what they mean for the exit code
/// ([`aye_events::ErrorClass::exit_code`]).
#[derive(Debug, Clone, Copy, Default)]
struct Errors {
    any: bool,
    usage: bool,
    internal: bool,
}

impl Errors {
    fn see(&mut self, diagnostic: &Diagnostic, origin: DiagnosticOrigin) {
        let Some(class) = diagnostic.error_class(origin) else {
            return;
        };
        self.any = true;
        match class.exit_code() {
            Some(ExitCode::UsageError) => self.usage = true,
            Some(ExitCode::InternalError) => self.internal = true,
            _ => {}
        }
    }
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
}

impl Stream<'_> {
    fn emit(&mut self, event: Event) {
        let mut record = self.stamper.stamp(Record::new(event));
        Self::deliver_to(
            &mut self.reporters,
            &mut self.history,
            &mut self.store,
            &mut self.errors,
            &mut record,
        );
    }

    /// A bad outcome first gets the test's `history` (how it ended before
    /// this run). Then every reporter gets every record: one that fails
    /// (stdout, a closed pipe) stops writing, and the others carry on.
    /// Then history folds it, and the run store keeps it.
    #[expect(
        clippy::print_stderr,
        reason = "aye's own messages, outside the event stream"
    )]
    fn deliver_to(
        reporters: &mut Tee<'_>,
        history: &mut Option<Remember<'_>>,
        store: &mut Keep,
        errors: &mut Errors,
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
            eprintln!("aye: cannot write output: {err}");
        }
        if let Some(history) = history.as_mut() {
            history.history.record(record);
        }
        store.record(record);
    }

    #[expect(
        clippy::print_stderr,
        reason = "aye's own messages, outside the event stream"
    )]
    fn finish(
        mut self,
        code: ExitCode,
        counts: Counts,
        stopped: Option<StopReason>,
        file_failed: &AtomicBool,
    ) -> ExitCode {
        self.emit(Event::RunFinished {
            exit_code: code,
            counts,
            duration: self.started.elapsed(),
            stopped,
        });
        if let Err(err) = self.reporters.finish()
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("aye: cannot write output: {err}");
        }
        if let Some(history) = &self.history {
            history.save();
        }
        // Kept for `aye show`; never fails the run.
        let runs = self.store.dir().to_path_buf();
        if let Err(err) = self.store.finish() {
            eprintln!(
                "aye: warning: could not keep this run for `aye show` in {}: {err}",
                runs.display()
            );
        }
        // A report file that couldn't be written at the very end (it was
        // writable when the run started) fails the run, as it does pytest's;
        // `run_finished` was already out, so only the process says so.
        if file_failed.load(Ordering::Relaxed) && matches!(code, ExitCode::Ok | ExitCode::NoTests) {
            return ExitCode::TestsFailed;
        }
        code
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
}

impl<R: Reporter> Reporter for File<R> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        self.inner.report(record)
    }

    #[expect(
        clippy::print_stderr,
        reason = "aye's own messages, outside the event stream"
    )]
    fn finish(&mut self) -> io::Result<()> {
        if let Err(err) = self.inner.finish() {
            eprintln!("aye: error: {err}");
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

    #[expect(
        clippy::print_stderr,
        reason = "aye's own messages, outside the event stream"
    )]
    fn save(&self) {
        if self.history.changed()
            && let Err(err) = self.history.save(&self.dir)
        {
            eprintln!(
                "aye: warning: could not save history to {}: {err}",
                self.dir.display()
            );
        }
    }
}

/// The first adapter's `field`, in priority order, that gives one: what
/// the run takes from the frameworks' own configuration where aye's says
/// nothing.
fn first<T>(
    defaults: &[(AdapterName, FrameworkDefaults)],
    field: impl Fn(&FrameworkDefaults) -> Option<T>,
) -> Option<T> {
    defaults.iter().find_map(|(_, defaults)| field(defaults))
}

/// Each adapter's `field`, for those that give one, in priority order.
fn each<T>(
    defaults: &[(AdapterName, FrameworkDefaults)],
    field: impl Fn(&FrameworkDefaults) -> Option<T>,
) -> Vec<(&AdapterName, T)> {
    defaults
        .iter()
        .filter_map(|(name, defaults)| Some((name, field(defaults)?)))
        .collect()
}

/// See [`Session::maxfail`].
fn maxfail(args: &Args, defaults: &[(AdapterName, FrameworkDefaults)]) -> Option<u32> {
    match passed_maxfail(&args.passthrough).unwrap_or_else(|| args.maxfail()) {
        Maxfail::After(n) => Some(n),
        Maxfail::Unlimited => None,
        Maxfail::Unset => first(defaults, |d| d.maxfail).map(MaxFail::get),
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

/// The exit code of a run that ran: an interruption decides first (Ctrl-C,
/// or a framework that stopped a batch as interrupted: `pytest.exit`, a test
/// raising `KeyboardInterrupt`), then an internal error, then a usage error
/// a batch reported (pytest rejecting its arguments or failing to load an
/// initial conftest, which pytest exits 4 for), then other error
/// diagnostics, then the counts, and a framework that failed a batch none
/// of whose tests failed (a conftest's `session.exitstatus`, pytest-cov's
/// `--cov-fail-under`), as pytest exits 1 then.
fn run_exit_code(
    summary: &aye_runner::RunSummary,
    errors: Errors,
    counts: &Counts,
    continue_on_collection_errors: bool,
) -> ExitCode {
    if summary.interrupted {
        ExitCode::Interrupted
    } else if summary.internal_error || errors.internal {
        ExitCode::InternalError
    } else if errors.usage {
        ExitCode::UsageError
    } else if errors.any && !continue_on_collection_errors {
        ExitCode::Interrupted
    } else if counts.bad() > 0 || errors.any || summary.framework_failed {
        ExitCode::TestsFailed
    } else if counts.total() == 0 {
        ExitCode::NoTests
    } else {
        ExitCode::Ok
    }
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

/// Prepare each adapter for the project, knowing the others.
fn prepare(adapters: &[Box<dyn Adapter>], project: &Project<'_>) -> Result<Vec<Prepared>> {
    let names: Vec<AdapterName> = adapters.iter().map(|adapter| adapter.name()).collect();
    let mut prepared: Vec<Prepared> = Vec::new();
    for adapter in adapters {
        if let Some(recognizer) = adapter
            .prepare_among(project, &names)
            .with_context(|| format!("invalid {} configuration", adapter.name()))?
        {
            prepared.push((adapter.name(), recognizer));
        }
    }
    Ok(prepared)
}

/// aye's options that shape the run, as a rerun repeats them
/// ([`aye_protocol::BatchContext::rerun_args`]): `-c FILE`, as given (a
/// rerun runs where aye ran). `--python` is left out (a rerun finds the
/// interpreter as aye did), and so are the options that only shape the
/// output or the selection, which a rerun of one id doesn't need,
/// `--native`, which does nothing, and `--junitxml`, whose report it would
/// overwrite.
fn rerun_args(args: &Args) -> Vec<String> {
    args.config_file
        .as_ref()
        .map(|file| vec!["-c".to_string(), file.to_string_lossy().into_owned()])
        .unwrap_or_default()
}

/// See [`Session::junit`]. `env` is the environment the command line's
/// path is expanded with.
fn junit_path(
    env: &ProcessEnv,
    cwd: &Path,
    given: Option<&Path>,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Option<PathBuf> {
    let path = match given {
        Some(given) => aye_fs::expand_path(&given.to_string_lossy(), |name| {
            env.get_os(name).map(ToOwned::to_owned)
        }),
        None => first(defaults, |d| d.junit.path.clone())?,
    };
    (!path.as_os_str().is_empty()).then(|| cwd.join(path))
}

/// What goes before the JUnit report's classnames: the framework's own
/// (pytest's `--junitprefix`), wherever the report's path came from.
fn junit_prefix(defaults: &[(AdapterName, FrameworkDefaults)]) -> String {
    first(defaults, |d| d.junit.prefix.clone()).unwrap_or_default()
}

/// How the JUnit report is written: the framework's own settings, from the
/// first adapter that gives each (pytest's `junit_suite_name`,
/// `junit_logging`, `junit_log_passing_tests`, `junit_duration_report` and
/// `junit_family`, in pytest's spellings), else aye's own.
fn junit_options(defaults: &[(AdapterName, FrameworkDefaults)]) -> JunitOptions {
    let mut options = JunitOptions::default();
    if let Some(name) = first(defaults, |d| d.junit.suite_name.clone()) {
        options.suite_name = name;
    }
    if let Some(logging) = first(defaults, |d| d.junit.logging.clone()) {
        options.logging = JunitLogging::from(logging.as_str());
    }
    if let Some(duration) = first(defaults, |d| d.junit.duration_report.clone()) {
        options.duration = JunitDuration::from(duration.as_str());
    }
    if let Some(family) = first(defaults, |d| d.junit.family.clone()) {
        options.family = JunitFamily::from(family.as_str());
    }
    if let Some(value) = first(defaults, |d| d.junit.log_passing_tests) {
        options.log_passing_tests = value;
    }
    options
}

/// What static selection keeps: the positional arguments' `selectors`, the
/// `-k` and `-m` expressions [`pick`] picks, and each adapter's
/// `--deselect` prefixes ([`FrameworkDefaults::deselect`]). A malformed
/// expression on the command line is a usage error, even where the
/// arguments after `--` override it; the framework's own, should aye fail
/// to parse it, drops no case.
fn selection<'s>(
    args: &Args,
    selectors: impl IntoIterator<Item = &'s str>,
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Result<Selection, aye_select::SelectError> {
    if let Some(expr) = &args.keyword {
        Selection::default().with_keyword(expr)?;
    }
    if let Some(expr) = &args.tags {
        Selection::default().with_tags(expr)?;
    }
    let mut selection = Selection::default().with_ids(selectors);
    let keyword = |d: &FrameworkDefaults| d.keyword.clone();
    let given = args.keyword.as_deref();
    match pick(given, passed(&args.passthrough, 'k')) {
        Pick::Given(expr) => {
            selection = selection.clone().with_keyword(&expr).unwrap_or(selection);
        }
        Pick::Unknown => {
            for (name, expr) in unread(given, defaults, keyword) {
                selection = selection
                    .clone()
                    .with_own_keyword(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
        Pick::Own => {
            for (name, expr) in each(defaults, keyword) {
                selection = selection
                    .clone()
                    .with_own_keyword(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
    }
    let tags = |d: &FrameworkDefaults| d.tags.clone();
    let given = args.tags.as_deref();
    match pick(given, passed(&args.passthrough, 'm')) {
        Pick::Given(expr) => {
            selection = selection.clone().with_tags(&expr).unwrap_or(selection);
        }
        Pick::Unknown => {
            for (name, expr) in unread(given, defaults, tags) {
                selection = selection
                    .clone()
                    .with_own_tags(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
        Pick::Own => {
            for (name, expr) in each(defaults, tags) {
                selection = selection
                    .clone()
                    .with_own_tags(name.clone(), &expr)
                    .unwrap_or(selection);
            }
        }
    }
    for (name, defaults) in defaults {
        selection = selection.with_deselect(name.clone(), defaults.deselect.iter().cloned());
    }
    Ok(selection)
}

/// The `-k` or `-m` expressions (each adapter's `own`) static selection
/// applies, by adapter, when the arguments after `--` may hold one aye can't
/// read ([`Pick::Unknown`]): the command line's (`given`), else the
/// adapter's own. No adapter applies an expression at runtime (aye's
/// pytest front-end lists every test, and nothing re-reads those
/// arguments' `-k` or `-m` in a batch), so static selection is the last
/// word on every case.
fn unread<'d>(
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
enum Pick {
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
fn pick(given: Option<&str>, framework: Passed) -> Pick {
    match framework {
        Passed::Set(expr) => Pick::Given(expr),
        Passed::Unknown => Pick::Unknown,
        Passed::Unset => given.map_or(Pick::Own, |expr| Pick::Given(expr.to_string())),
    }
}

/// The output format, and where it came from: `--output`, else
/// `[tool.aye] output`, else text. aye-config accepts only the values in
/// `aye_config::OUTPUTS`, each of which parses.
fn output(args: &Args, config: &AyeConfig) -> (Output, &'static str) {
    if let Some(output) = args.output {
        return (output, "--output");
    }
    match config.settings.output.as_deref().map(str::parse) {
        Some(Ok(output)) => (output, "[tool.aye] output"),
        _ => (Output::Text, "default"),
    }
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

/// The path part of a positional argument: everything before `::`.
pub(crate) fn path_part(arg: &str) -> &str {
    arg.split_once("::").map_or(arg, |(path, _)| path)
}

/// A positional argument names a path when it exists, contains a separator,
/// or ends in `.py`; otherwise it is a dotted name or a key.
pub(crate) fn is_path_like(cwd: &Path, arg: &str) -> bool {
    arg.contains('/')
        || arg.contains(std::path::MAIN_SEPARATOR)
        || Path::new(arg)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("py"))
        || cwd.join(arg).exists()
}

/// Read a positional argument: a path is taken from the current directory,
/// as pytest does, or from the project root when only the root has it, so
/// that the ids aye prints (which are relative to the root) work from any
/// directory. The selector is relative to the root, keeping any `::` part;
/// dotted names and keys pass through.
pub(crate) fn resolve(cwd: &Path, root: &Path, arg: &str) -> Positional {
    let (path, rest) = arg
        .split_once("::")
        .map_or((arg, None), |(p, r)| (p, Some(r)));
    let from_root = || {
        let at_root = normalize_absolute(&root.join(path));
        (Path::new(path).is_relative() && at_root.starts_with(root) && at_root.exists())
            .then_some(at_root)
    };
    let absolute = if is_path_like(cwd, path) {
        let from_cwd = normalize_absolute(&cwd.join(path));
        if from_cwd.exists() {
            from_cwd
        } else {
            from_root().unwrap_or(from_cwd)
        }
    } else if let Some(at_root) = from_root() {
        // A bare word naming a top-level file or directory.
        at_root
    } else {
        return Positional {
            selector: arg.to_string(),
            path: None,
            id: rest.is_some(),
        };
    };
    let mut selector = root_relative(&absolute, root);
    if selector.is_empty() {
        selector.push('.');
    }
    if let Some(rest) = rest {
        selector.push_str("::");
        selector.push_str(rest);
    }
    Positional {
        selector,
        path: Some(absolute),
        id: rest.is_some(),
    }
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

/// Ctrl-C (or SIGTERM, or SIGHUP) stops the run cleanly; a second one
/// kills every worker, with the processes its tests started, and exits at
/// once. A signal within [`REPEAT_WINDOW`] of the first is that one
/// delivered again, not a second request (see [`Presses`]).
/// `dir` when collecting from it means the user's whole home directory or a
/// whole filesystem, in words; `None` for any other directory.
fn everything(dir: &Path) -> Option<String> {
    let dir = fs::canonicalize(dir).ok()?;
    if dir.parent().is_none() {
        return Some(format!("the whole filesystem ({})", dir.display()));
    }
    let home = std::env::home_dir().and_then(|home| fs::canonicalize(home).ok())?;
    (dir == home).then(|| format!("your whole home directory ({})", dir.display()))
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

/// Ctrl-C (or SIGTERM, SIGHUP): before the run, stop collection (`stop` the
/// collector watches), and exit anyway if that takes too long; during the
/// run (`running`), stop it cleanly.
#[expect(clippy::print_stderr, reason = "says why aye stopped")]
#[expect(
    clippy::exit,
    reason = "a second signal, or a collection that doesn't stop in time, ends aye at once"
)]
fn install_interrupt(
    stop: aye_runner::StopHandle,
    collecting: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
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
                        eprintln!("\naye: interrupted");
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
