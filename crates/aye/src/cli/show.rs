//! `aye show`'s command line: [`ShowArgs`].

use aye_config::OutputFormat;
use clap::{ArgAction, Parser};

use super::Color;
use super::output::output_format;

/// Show a stored run's results again, without running anything.
///
/// aye keeps each run that ran something in `.aye/runs` (`[tool.aye]
/// keep-runs`, 10 by default), numbered `r1`, `r2`, … With no test named,
/// the list of the last run: a line per result (each test with its outcome,
/// and for a failure its key and why; each failed lifetime, collection
/// error or module skipped while collecting), then the run's summary; paths
/// alone filter it. Name tests to see them in detail, as the run showed
/// them: by the key the run printed (`aye show 3fae6`) or its start (from 4
/// digits), test id or dotted name (a class's id shows its failed setup or
/// teardown too), and paths then select their tests too; `--failed` gives
/// the details of everything that failed. Details come at the second level
/// unless `-v` or `-q` says otherwise; the flags mean what they mean in a
/// run (`aye show 3fae6 --output json` is what a coding agent wants;
/// `-vv`, every frame, the output and the locals). `--log` writes the
/// run's raw events. A file of a test shown that changed since the run is
/// said first. The summary's exit code is the run's. Exit codes: 0 when it
/// could show what was asked, 4 for a name no test of the run has (or the
/// start of several keys), 5 when no run is stored.
// `aye show`'s command line: `main` drops `show` before parsing it, so the
// binary's name is set here, for the usage lines.
#[derive(Debug, Parser)]
#[command(
    name = "aye show",
    bin_name = "aye show",
    about = "Show a stored run's results again, without running anything",
    max_term_width = 100,
    args_override_self = true
)]
pub(crate) struct ShowArgs {
    /// Test keys (`3fae6`) or their start, test ids, dotted names or paths.
    #[arg(value_name = "TEST")]
    pub tests: Vec<String>,

    /// The details of everything that failed: tests that failed or
    /// errored, failed lifetimes, collection errors and failed workers.
    #[arg(long)]
    pub failed: bool,

    /// Only these outcomes: `passed`, `failed`, `error`, `skipped`,
    /// `expected_failure`, `unexpected_pass`.
    #[arg(
        long,
        value_name = "OUTCOME[,OUTCOME]",
        value_delimiter = ',',
        value_parser = outcome,
        action = ArgAction::Append
    )]
    pub outcome: Vec<aye_events::Outcome>,

    /// Only tests matching the keyword expression, as the run's `-k`.
    #[arg(short = 'k', value_name = "EXPR", allow_hyphen_values = true)]
    pub keyword: Option<String>,

    /// Only tests matching the tag expression, as the run's `-m`.
    #[arg(short = 'm', value_name = "EXPR", allow_hyphen_values = true)]
    pub tags: Option<String>,

    /// An older stored run instead of the last: `r81` or `81` (`aye show
    /// --runs` lists them).
    #[arg(long, value_name = "RUN")]
    pub run: Option<String>,

    /// List the stored runs, newest first: run, start time, exit code and
    /// counts.
    #[arg(long, conflicts_with_all = ["tests", "failed", "outcome", "keyword", "tags", "run"])]
    pub runs: bool,

    /// Write the run's log as stored, every event, one JSON object per
    /// line, whatever the output format.
    #[arg(long, conflicts_with_all = ["tests", "failed", "outcome", "keyword", "tags", "runs"])]
    pub log: bool,

    /// More detail; repeat for more (up to -vvv). Without -v or -q, details
    /// come at -v in text, -vv in JSON.
    #[arg(short = 'v', long, action = ArgAction::Count)]
    pub verbose: u8,

    /// Less: only the summary.
    #[arg(short = 'q', long, action = ArgAction::Count)]
    pub quiet: u8,

    /// Output format, as for a run.
    #[arg(
        long,
        ignore_case = true,
        value_name = "FORMAT",
        value_parser = output_format()
    )]
    pub output: Option<OutputFormat>,

    /// When to use colour.
    #[arg(long, value_enum, default_value_t = Color::Auto, value_name = "WHEN")]
    pub color: Color,
}

/// An outcome's name, as the event log spells it (`-` for `_` too).
fn outcome(name: &str) -> Result<aye_events::Outcome, String> {
    use aye_events::Outcome;
    let name = name.trim().to_ascii_lowercase().replace('-', "_");
    [
        Outcome::Passed,
        Outcome::Failed,
        Outcome::Error,
        Outcome::Skipped,
        Outcome::ExpectedFailure,
        Outcome::UnexpectedPass,
    ]
    .into_iter()
    .find(|outcome| outcome.as_str() == name)
    .ok_or_else(|| {
        format!(
            "expected passed, failed, error, skipped, expected_failure or unexpected_pass, \
             got `{name}`"
        )
    })
}
