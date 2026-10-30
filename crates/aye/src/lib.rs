//! The `aye` binary, as a library: [`main`] reads the command line
//! (`cli.rs`: which command, and its arguments as clap reads them), the
//! environment aye started in, once (`environment.rs`), resolves the
//! command's settings from them and `[tool.aye]`, once and in the order that
//! decides which usage error wins (`settings.rs`, with the positional
//! arguments, each a path, an id or a word: `positional.rs`), and runs the
//! command (`commands.rs`): a run (`commands/run.rs`, in the steps of
//! architecture.md's "A run", whose exit code `commands/run/exit.rs`
//! decides), `aye config` (`commands/config.rs`) or `aye show`
//! (`commands/show.rs`). A run's records go through its event stream
//! (`stream.rs`: the passes that rewrite them, and the sink: the
//! reporters, history and the run store). What ends aye before a run's
//! event stream starts is an `AyeError` (`error.rs`), and everything the
//! binary writes beside the stream goes through its `Printer`
//! (`printer.rs`). `main.rs` only calls [`main`].
//!
//! Holds the static list of adapters the runner knows about, what the
//! binary knows of each beyond the contract (its traits), and which of
//! them a project runs (`registry.rs`); orchestration lives in
//! `aye-runner`, presentation in `aye-render`. Linking an adapter here is
//! the only place an adapter is depended on.
//!
//! Depends on: every runner crate and every `aye-adapter-*` crate. Its
//! end-to-end tests (`tests/`) run the binary, through aye-test.
//!
//! What each part decided (implementation, October 2026) is said by the
//! module that does it.

use std::ffi::OsString;
use std::process;

use aye_events::ExitCode;

mod cli;
mod commands;
mod environment;
mod error;
mod framework_args;
mod positional;
mod printer;
mod registry;
mod settings;
mod stream;

use cli::Command;
use commands::run::RunEnd;
use environment::EnvironmentOptions;
use error::AyeError;
use printer::Printer;
use settings::RunSettings;

/// What `aye` does with the command line `args` (the program's name
/// first), and the exit code the process ends with.
pub fn main(args: impl IntoIterator<Item = OsString>) -> process::ExitCode {
    let printer = Printer;
    run(args.into_iter().collect(), printer)
        .unwrap_or_else(|err| err.report(printer))
        .into()
}

/// The command `args` ask for, run: its arguments read, its settings
/// resolved, then the command itself. The environment is read once, here,
/// for a run or `aye config`: everything after reads what was read.
fn run(args: Vec<OsString>, printer: Printer) -> Result<ExitCode, AyeError> {
    match Command::parse(args)? {
        Command::Show(args) => {
            commands::show::show(&args, printer)?;
            Ok(ExitCode::Ok)
        }
        Command::Config(args) => {
            let settings = RunSettings::resolve(args, EnvironmentOptions::from_process(), printer)?;
            commands::config::print(&settings, printer)?;
            Ok(ExitCode::Ok)
        }
        Command::Run(args) => {
            let settings = RunSettings::resolve(args, EnvironmentOptions::from_process(), printer)?;
            Ok(process_exit_code(commands::run::run(settings, printer)?))
        }
    }
}

/// The exit code a run ends the process with: the one `run_finished`
/// carried, except that a report file that couldn't be written at the very
/// end (it was writable when the run started) fails the run, as it does
/// pytest's: 0 or 5 become 1. `run_finished` was out already, so only the
/// process can say so.
fn process_exit_code(ended: RunEnd) -> ExitCode {
    if ended.report_failed && matches!(ended.code, ExitCode::Ok | ExitCode::NoTests) {
        ExitCode::TestsFailed
    } else {
        ended.code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_that_fails_at_the_end_fails_the_process() {
        let ended = |code, report_failed| {
            process_exit_code(RunEnd {
                code,
                report_failed,
            })
        };
        assert_eq!(ended(ExitCode::Ok, true), ExitCode::TestsFailed);
        assert_eq!(ended(ExitCode::NoTests, true), ExitCode::TestsFailed);
        for code in [
            ExitCode::TestsFailed,
            ExitCode::Interrupted,
            ExitCode::InternalError,
            ExitCode::UsageError,
        ] {
            assert_eq!(ended(code, true), code);
        }
        for code in [ExitCode::Ok, ExitCode::NoTests, ExitCode::Interrupted] {
            assert_eq!(ended(code, false), code);
        }
    }
}
