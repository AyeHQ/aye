//! What ends aye before a run's event stream starts, or `aye show` before
//! it shows what was asked ([`AyeError`]), and the exit code each gives.
//! Once the stream starts, a run ends with the code it decides itself
//! (`RunVerdict`), and `run_finished` carries it.

use std::fmt::{Debug, Display};
use std::io;

use aye_events::ExitCode;
use clap::error::ErrorKind;

use crate::printer::Printer;

/// Why aye stops before a run's stream starts, or `aye show` before it
/// shows anything.
#[derive(Debug)]
pub(crate) enum AyeError {
    /// The command line, as clap reads it: a usage error, or the help or
    /// the version asked for. clap prints it.
    Cli(clap::Error),
    /// A bad command line, configuration or expression, no interpreter,
    /// a report that can't be written, a test `aye show` can't show.
    Usage(anyhow::Error),
    /// `aye show` with no run stored.
    NoRun(String),
    /// `aye show`'s output that can't be written.
    Output(io::Error),
}

impl AyeError {
    /// A usage error that says `message`.
    pub(crate) fn usage(message: impl Display + Debug + Send + Sync + 'static) -> Self {
        Self::Usage(anyhow::Error::msg(message))
    }

    /// Say what went wrong, through `printer` (clap prints its own), and
    /// give the exit code: 0 for the help or the version, 4 for a usage
    /// error, as pytest exits (clap's own would be 2), and 5 when no run
    /// is stored. A usage error is one line, anyhow's chain joined by `: `.
    pub(crate) fn report(self, printer: Printer) -> ExitCode {
        match self {
            Self::Cli(err) => {
                let _ = err.print();
                match err.kind() {
                    ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::Ok,
                    _ => ExitCode::UsageError,
                }
            }
            Self::Usage(err) => {
                printer.error(format_args!("{err:#}"));
                ExitCode::UsageError
            }
            Self::NoRun(message) => {
                printer.note(message);
                ExitCode::NoTests
            }
            Self::Output(err) => {
                printer.error(format_args!("cannot write output: {err}"));
                ExitCode::UsageError
            }
        }
    }
}

impl From<clap::Error> for AyeError {
    fn from(err: clap::Error) -> Self {
        Self::Cli(err)
    }
}

impl From<anyhow::Error> for AyeError {
    fn from(err: anyhow::Error) -> Self {
        Self::Usage(err)
    }
}

impl From<io::Error> for AyeError {
    fn from(err: io::Error) -> Self {
        Self::Output(err)
    }
}
