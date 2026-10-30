//! The command line as clap reads it, and nothing more: which command
//! ([`Command`]), each one's arguments ([`RunArgs`], [`ShowArgs`]), and what
//! they share ([`Verbosity`], [`Color`]). What the arguments mean for a run
//! is the settings' to say.

use std::ffi::OsString;
use std::path::Path;

use clap::Parser as _;

mod compat;
mod output;
mod run;
mod show;

pub(crate) use compat::RunCompatArgs;
pub(crate) use output::{Color, Verbosity};
pub(crate) use run::RunArgs;
pub(crate) use show::ShowArgs;

/// What `aye` is asked to do.
#[derive(Debug)]
pub(crate) enum Command {
    /// Run the tests: `aye …`.
    Run(RunArgs),
    /// Show a run's configuration, and where each value came from, without
    /// running it: `aye config …`, with the run's arguments.
    Config(RunArgs),
    /// Show a stored run again: `aye show …`.
    Show(ShowArgs),
}

impl Command {
    /// The command `argv` (the program's name first) asks for. `show` or
    /// `config` as the first argument is that command, unless a file or
    /// directory of that name is in the current directory, which it then
    /// names, as any positional argument does. Only the first argument is
    /// looked at: in `aye -v show`, `show` is a positional, whose
    /// `not-found` says where the command goes.
    pub(crate) fn parse(mut argv: Vec<OsString>) -> Result<Self, clap::Error> {
        let first_is =
            |name: &str| argv.get(1).is_some_and(|a| a == name) && !Path::new(name).exists();
        if first_is("show") {
            argv.remove(1);
            return ShowArgs::try_parse_from(argv).map(Self::Show);
        }
        let config = first_is("config");
        if config {
            argv.remove(1);
        }
        let run = RunArgs::try_parse_from(argv)?;
        Ok(if config {
            Self::Config(run)
        } else {
            Self::Run(run)
        })
    }
}
