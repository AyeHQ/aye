//! What `aye show` shows a stored run with ([`ShowSettings`]).

use std::path::PathBuf;

use aye_config::{AyeSettings, OutputFormat};
use aye_fs::normalize_absolute;
use aye_render::{ColorChoice, DetailLevel, Ladder, Level};

use super::output;
use crate::cli::{ShowArgs, Verbosity};
use crate::framework_args;
use crate::positional::{is_path_like, path_part};

/// Where `aye show` is, and how it shows what it is asked to: the format,
/// as a run's, how much it shows, and when to use colour. What it shows
/// (which run, which results) is the command's to read.
#[derive(Debug, Clone)]
pub(crate) struct ShowSettings {
    pub(crate) cwd: PathBuf,
    /// The project root, found as a run finds it.
    pub(crate) root: PathBuf,
    /// How many runs `.aye/runs` keeps (`[tool.aye] keep-runs`).
    pub(crate) keep_runs: usize,
    pub(crate) format: OutputFormat,
    pub(crate) verbosity: Verbosity,
    pub(crate) color: ColorChoice,
}

impl ShowSettings {
    /// `aye show`'s `args`, in the project they name. Each step can fail
    /// with a usage error, and the first that does is reported: its `-k`
    /// and `-m` values, as a run checks them, the current directory, then
    /// `[tool.aye]`. Its expressions are read later, once the run they
    /// filter is found.
    pub(crate) fn resolve(args: &ShowArgs) -> Result<Self, String> {
        framework_args::check_expressions(args.keyword.as_deref(), args.tags.as_deref())?;
        let cwd = std::env::current_dir()
            .map_err(|err| format!("cannot read the current directory: {err}"))?;
        let given: Vec<PathBuf> = args
            .tests
            .iter()
            .map(|test| path_part(test))
            .filter(|path| is_path_like(&cwd, path))
            .map(|path| normalize_absolute(&cwd.join(path)))
            .collect();
        let root = normalize_absolute(&aye_config::project_root(&cwd, &given));
        let tool_aye = AyeSettings::load(&root).map_err(|err| err.to_string())?;
        Ok(Self {
            format: output::format(args.output, &tool_aye).value,
            keep_runs: tool_aye.keep_runs(),
            verbosity: Verbosity::new(args.verbose, args.quiet),
            color: args.color.into(),
            cwd,
            root,
        })
    }

    /// How much the output shows: as a run's ([`Verbosity::level`]) when
    /// `-v` or `-q` is given. Without either, `details` (of tests named, or
    /// `--failed`) come at the detail ladder's second level, L2, which is
    /// text's `-v` and JSON's `-vv` ([`Ladder::first_at`]); the list at a
    /// run's default.
    pub(crate) fn level(&self, details: bool) -> Level {
        if details && self.verbosity.is_unset() {
            let ladder = match self.format {
                OutputFormat::Text => Ladder::TEXT,
                OutputFormat::Json => Ladder::JSON,
            };
            return ladder.first_at(DetailLevel::L2);
        }
        self.verbosity.level()
    }
}
