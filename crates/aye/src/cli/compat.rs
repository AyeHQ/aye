//! Flags a run still accepts that do nothing: [`RunCompatArgs`].

/// A run's flags that do nothing now. They are accepted, so that a command
/// line giving them still runs, and listed where they always were, saying
/// so; a rerun leaves them out.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
pub(crate) struct RunCompatArgs {
    /// Accepted and does nothing: aye's own engine always runs pytest's
    /// tests, without pytest (this was the prototype's switch).
    #[arg(long)]
    pub native: bool,
}
