//! What a run is asked to do, resolved once ([`RunSettings`]), and what
//! `aye show` is ([`ShowSettings`]). A run's settings come from its command
//! line ([`RunArgs`]), `[tool.aye]` ([`AyeSettings`]), the environment aye
//! started in ([`EnvironmentOptions`]) and, where aye's say nothing, the
//! adapters' own frameworks ([`FrameworkDefaults`]), in one order, which
//! decides which usage error a command line and a project with several
//! report. Nothing after resolution reads the arguments again.
//!
//! Decided (implementation, October 2026):
//! - **No whole-home runs.** With no path, a run from the home directory or
//!   a filesystem root is a usage error before anything walks (a home
//!   directory under git is a project root, and pytest would collect every
//!   project in it); a path given (`aye .`) runs.
//! - **The arguments after `--`** are the framework's: aye's pytest
//!   front-end parses them after `addopts` and `PYTEST_ADDOPTS`, as pytest
//!   would, so those that replace a test run (pytest's `-h`, `-V`, `--co`,
//!   `--fixtures`, `--fixtures-per-test`, `--markers`, also inside a cluster
//!   such as `-vh`) are usage errors naming the aye equivalent, before
//!   anything runs. So are `--setup-plan` and `--setup-only`: they run no
//!   test, and every test would be reported, and remembered for `--lf`, as
//!   skipped. `--setup-show` runs the tests and is allowed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use aye_adapter::{FrameworkDefaults, Project};
use aye_config::{AyeSettings, Sourced};
use aye_events::AdapterName;
use aye_fs::normalize_absolute;
use aye_protocol::Policy;
use aye_provision::Interpreter;
use aye_pytest::argv::is_value;
use aye_select::Selection;

use crate::cli::{RunArgs, RunCompatArgs, Verbosity};
use crate::environment::EnvironmentOptions;
use crate::positional::{Positionals, is_path_like, path_part};
use crate::printer::Printer;
use crate::registry::{self, ActiveAdapters, Listing};

mod junit;
mod maxfail;
mod output;
mod python;
mod selection;
mod show;
mod workers;

pub(crate) use junit::JunitTarget;
pub(crate) use output::OutputSettings;
pub(crate) use show::ShowSettings;

/// What clap can't check of aye's own `-k` and `-m`: a value starting with
/// `-` is accepted only where pytest's argparse accepts it, when it looks
/// like a negative number or contains a space; otherwise it was meant as a
/// flag.
fn check_expressions(keyword: Option<&str>, tags: Option<&str>) -> Result<(), String> {
    for (flag, value) in [("-k", keyword), ("-m", tags)] {
        if let Some(value) = value
            && !is_value(value)
        {
            return Err(format!("argument {flag}: expected one argument"));
        }
    }
    Ok(())
}

/// A run's inputs, resolved: everything wrong with the command line or the
/// configuration fails here, before anything is reported. Selectors that
/// match nothing are found after collection, and reported in the stream.
pub(crate) struct RunSettings {
    pub(crate) cwd: PathBuf,
    pub(crate) root: PathBuf,
    /// Each positional argument, resolved.
    pub(crate) positionals: Positionals,
    /// The positional arguments' paths and ids' files, as the adapters take
    /// them: as given, or absolute when only the project root makes sense
    /// of them ([`Positionals::project_args`]).
    pub(crate) paths: Vec<PathBuf>,
    /// `[tool.aye]`, the project's.
    pub(crate) tool_aye: AyeSettings,
    /// The environment aye started in, read once: what the adapters, the
    /// refinement cache's key and the reporters read.
    pub(crate) env: EnvironmentOptions,
    /// The interpreter that runs the tests, and where it came from, as `aye
    /// config` shows it.
    pub(crate) interpreter: Sourced<Interpreter>,
    /// The adapters the run uses, in priority order, why those, and those
    /// prepared for the project.
    pub(crate) adapters: ActiveAdapters,
    /// What static selection keeps.
    pub(crate) selection: Selection,
    pub(crate) workers: u32,
    pub(crate) output: OutputSettings,
    /// Where the JUnit XML report goes, and how it is written.
    pub(crate) junit: Option<JunitTarget>,
    /// What each batch is run with.
    pub(crate) policy: Policy,
    /// The framework's config file (pytest's `-c`).
    pub(crate) config_file: Option<PathBuf>,
    /// The arguments after `--`.
    pub(crate) framework_args: Vec<String>,
    /// aye's options that shape the run, as a rerun repeats them.
    pub(crate) rerun_args: Vec<String>,
    pub(crate) collect_only: bool,
    pub(crate) no_refine: bool,
    pub(crate) last_failed: bool,
    pub(crate) failed_first: bool,
    pub(crate) strict: bool,
    pub(crate) continue_on_collection_errors: bool,
}

impl RunSettings {
    /// The run `args` ask for, in the project they name, in the environment
    /// aye started in. Each step can fail with a usage error, and the first
    /// that does is reported: what clap can't check of the arguments, the
    /// current directory, `[tool.aye]`, the interpreter, the whole-home
    /// rule, the adapters `[tool.aye]` names and the tables of those that
    /// don't run (the registry), `--no-refine` without `--co`, the adapters'
    /// own configuration (preparing them), then the command line's `-k` and
    /// `-m`. The rest can't fail. `printer` shows that aye is collecting,
    /// if preparing the adapters takes a while.
    pub(crate) fn resolve(
        args: RunArgs,
        env: EnvironmentOptions,
        printer: Printer,
    ) -> Result<Self> {
        let RunArgs {
            tests,
            keyword,
            tags,
            exitfirst,
            maxfail,
            verbose,
            quiet,
            no_capture,
            last_failed,
            failed_first,
            workers,
            collect_only,
            no_refine,
            continue_on_collection_errors,
            strict,
            output,
            junit_xml,
            python,
            config_file,
            color,
            compat: RunCompatArgs { native: _ },
            passthrough,
        } = args;
        // What clap can't check: aye's own `-k` and `-m` values, and
        // arguments after `--` that would replace the test run.
        check_expressions(keyword.as_deref(), tags.as_deref()).map_err(anyhow::Error::msg)?;
        if let Some(problem) = aye_pytest::argv::replaced_run(&passthrough) {
            return Err(anyhow::Error::msg(problem));
        }
        let cwd = std::env::current_dir().context("cannot read the current directory")?;
        // `.` and `..` are resolved lexically, as pytest's `os.path.abspath`
        // does, before the root is searched for.
        let given: Vec<PathBuf> = tests
            .iter()
            .map(|test| path_part(test))
            .filter(|path| is_path_like(&cwd, path))
            .map(|path| normalize_absolute(&cwd.join(path)))
            .collect();
        let root = normalize_absolute(&aye_config::project_root(&cwd, &given));
        let positionals = Positionals::resolve(&cwd, &root, &tests);
        let paths = positionals.project_args(&cwd);

        let tool_aye = AyeSettings::load(&root)?;
        let interpreter = python::find(python.as_deref(), &env, &tool_aye, &root, &cwd)?;
        let project = Project {
            root: &root,
            cwd: &cwd,
            args: &paths,
            config_file: config_file.as_deref(),
            framework_args: &passthrough,
            env: &env.vars,
            tool_aye: &tool_aye,
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
        let output =
            OutputSettings::resolve(output, &tool_aye, Verbosity::new(verbose, quiet), color);
        // Choosing and preparing the adapters can walk the project; on a
        // terminal, say so if it takes a while.
        let busy = printer.collecting(output.format.value);
        // `--co --no-refine` lists without importing: pytest's cases are the
        // static recognizer's prediction.
        let listing = if no_refine && collect_only {
            Listing::Static
        } else {
            Listing::Refine
        };
        let chosen = registry::choose(&project, listing)?;
        // A run imports every pytest file it runs: aye's pytest front-end
        // lists each one, so there is no static run to fall back on.
        if no_refine && !collect_only && chosen.refine_every_file() {
            anyhow::bail!(
                "--no-refine lists tests without importing them, with --co; a run of \
                 pytest's tests imports every file it runs (aye's pytest front-end reads \
                 each one), so it can't do without"
            );
        }
        let adapters = chosen.prepare(&project)?;
        // What each prepared adapter's framework says about the run, in the
        // same order.
        let defaults: Vec<(AdapterName, FrameworkDefaults)> = adapters
            .prepared()
            .iter()
            .map(|(name, recognizer)| (name.clone(), recognizer.defaults()))
            .collect();

        let selection = selection::resolve(
            keyword.as_deref(),
            tags.as_deref(),
            &passthrough,
            positionals.selectors(),
            &defaults,
        )?;
        let workers = workers::resolve(workers, &tool_aye, &defaults);
        let policy = Policy {
            maxfail: maxfail::resolve(&passthrough, exitfirst, maxfail, &defaults),
            capture: !no_capture,
            verbosity: output.verbosity.framework(),
            timeout: tool_aye.timeout(),
            keyword,
            tags,
            framework_args: passthrough.clone(),
        };
        let junit = JunitTarget::resolve(&env.vars, &cwd, junit_xml.as_deref(), &defaults);
        let rerun_args = rerun_args(config_file.as_deref());
        drop(busy);
        Ok(Self {
            cwd,
            root,
            positionals,
            paths,
            tool_aye,
            env,
            interpreter,
            adapters,
            selection,
            workers,
            output,
            junit,
            policy,
            config_file,
            framework_args: passthrough,
            rerun_args,
            collect_only,
            no_refine,
            last_failed,
            failed_first,
            strict,
            continue_on_collection_errors,
        })
    }

    /// The project as the adapters and collection see it.
    pub(crate) fn project(&self) -> Project<'_> {
        Project {
            root: &self.root,
            cwd: &self.cwd,
            args: &self.paths,
            config_file: self.config_file.as_deref(),
            framework_args: &self.framework_args,
            env: &self.env.vars,
            tool_aye: &self.tool_aye,
        }
    }
}

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

/// aye's options that shape the run, as a rerun repeats them
/// ([`aye_protocol::BatchContext::rerun_args`]): `-c FILE`, as given (a
/// rerun runs where aye ran). `--python` is left out (a rerun finds the
/// interpreter as aye did), and so are the options that only shape the
/// output or the selection, which a rerun of one id doesn't need,
/// `--native`, which does nothing, and `--junitxml`, whose report it would
/// overwrite.
fn rerun_args(config_file: Option<&Path>) -> Vec<String> {
    config_file
        .map(|file| vec!["-c".to_string(), file.to_string_lossy().into_owned()])
        .unwrap_or_default()
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

#[cfg(test)]
mod tests;
