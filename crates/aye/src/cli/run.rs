//! A run's command line (and `aye config`'s): [`RunArgs`].

use std::path::PathBuf;

use aye_config::{OutputFormat, WorkerCount};
use clap::{ArgAction, Parser};

use super::output::output_format;
use super::{Color, RunCompatArgs};

/// A fast test runner for Python suites.
///
/// Flags mirror pytest's. Anything after `--` goes to the framework unchanged.
/// `aye config` shows the resolved configuration and where each value came
/// from. `aye show` shows the last run's results again, and any test's
/// details, without running anything (`aye show --help`).
// As with pytest's argparse, an option given twice keeps its last value.
#[derive(Debug, Parser)]
#[command(
    name = "aye",
    version,
    about,
    max_term_width = 100,
    args_override_self = true
)]
pub(crate) struct RunArgs {
    /// Files, directories, test ids (`path::Class::test[param]`), dotted
    /// names (`pkg.mod.Class.test`) or test keys.
    #[arg(value_name = "TEST")]
    pub tests: Vec<String>,

    /// Only run tests matching the keyword expression.
    // Values may start with `-` (`-k -1`); see `Args::check`.
    #[arg(short = 'k', value_name = "EXPR", allow_hyphen_values = true)]
    pub keyword: Option<String>,

    /// Only run tests matching the tag expression (pytest's markers).
    #[arg(short = 'm', value_name = "EXPR", allow_hyphen_values = true)]
    pub tags: Option<String>,

    /// Stop after the first failure (`--maxfail 1`).
    // `-x` and `--maxfail` set one value, as in pytest: the last one wins.
    #[arg(short = 'x', long, overrides_with = "maxfail")]
    pub exitfirst: bool,

    /// Stop after N failures; 0 means no limit.
    #[arg(long, value_name = "N", overrides_with = "exitfirst")]
    pub maxfail: Option<u32>,

    /// More output, in either format; repeat for more (up to -vvv).
    #[arg(short = 'v', long, action = ArgAction::Count)]
    pub verbose: u8,

    /// Less output: only the summary.
    #[arg(short = 'q', long, action = ArgAction::Count)]
    pub quiet: u8,

    /// Don't capture output.
    #[arg(short = 's')]
    pub no_capture: bool,

    /// Rerun only the tests that failed last time (all, if none did).
    #[arg(long = "lf", visible_alias = "last-failed")]
    pub last_failed: bool,

    /// Run last time's failures first.
    #[arg(long = "ff", visible_alias = "failed-first")]
    pub failed_first: bool,

    /// Worker processes, or `auto` for one per CPU.
    #[arg(short = 'n', long, value_name = "N|auto")]
    pub workers: Option<WorkerCount>,

    /// List the selected tests without running them.
    #[arg(long = "collect-only", visible_alias = "co")]
    pub collect_only: bool,

    /// Don't import files whose tests can't be known statically.
    #[arg(long)]
    pub no_refine: bool,

    /// Run the tests even when some files fail to collect.
    #[arg(long)]
    pub continue_on_collection_errors: bool,

    /// Treat collection warnings as errors.
    #[arg(long)]
    pub strict: bool,

    /// Output format; -v and -q set how much it shows.
    // Any case, as `[tool.aye] output` takes it. The formats' old names are
    // accepted by the first parser, hidden, so that the second can say what
    // to use instead.
    #[arg(
        long,
        ignore_case = true,
        value_name = "FORMAT",
        value_parser = output_format()
    )]
    pub output: Option<OutputFormat>,

    /// Also write a JUnit XML report.
    #[arg(long = "junitxml", visible_alias = "junit-xml", value_name = "PATH")]
    pub junit_xml: Option<PathBuf>,

    /// The Python interpreter that runs the tests.
    #[arg(long, value_name = "PATH")]
    pub python: Option<PathBuf>,

    /// The framework's config file (pytest's `-c`).
    #[arg(short = 'c', long = "config-file", value_name = "FILE")]
    pub config_file: Option<PathBuf>,

    /// When to use colour.
    #[arg(long, value_enum, default_value_t = Color::Auto, value_name = "WHEN")]
    pub color: Color,

    /// Flags that do nothing now, in their place.
    #[command(flatten)]
    pub compat: RunCompatArgs,

    /// Arguments for the framework.
    #[arg(last = true, value_name = "ARGS")]
    pub passthrough: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_is_accepted_and_does_nothing() {
        use clap::CommandFactory as _;
        for argv in [
            &["aye", "--native"][..],
            &["aye", "--native", "--co", "--no-refine"],
            &["aye", "--native", "--no-refine"],
        ] {
            let args = RunArgs::try_parse_from(argv).unwrap();
            assert!(args.compat.native, "{argv:?}");
            assert_eq!(
                crate::framework_args::replaced_run(&args.passthrough),
                None,
                "{argv:?}"
            );
        }
        let help = RunArgs::command().render_long_help().to_string();
        assert!(
            help.contains("--native") && help.contains("does nothing"),
            "{help}"
        );
    }

    #[test]
    fn pytest_style_flags_parse() {
        let args = RunArgs::try_parse_from([
            "aye",
            "tests/",
            "-k",
            "login and not slow",
            "-m",
            "db",
            "-x",
            "-vv",
            "-n",
            "auto",
            "--lf",
            "--co",
            "--junitxml",
            "out.xml",
            "--",
            "--reuse-db",
        ])
        .unwrap();
        assert_eq!(args.tests, ["tests/"]);
        assert_eq!(args.keyword.as_deref(), Some("login and not slow"));
        assert_eq!(args.tags.as_deref(), Some("db"));
        assert!(args.exitfirst && args.last_failed && args.collect_only);
        assert_eq!(args.verbose, 2);
        assert_eq!(args.workers, Some(WorkerCount::Auto));
        assert_eq!(args.junit_xml, Some(PathBuf::from("out.xml")));
        assert_eq!(args.passthrough, ["--reuse-db"]);
    }

    fn parse(argv: &[&str]) -> RunArgs {
        RunArgs::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
    }

    #[test]
    fn the_output_format_takes_any_case() {
        assert_eq!(
            parse(&["--output", "JSON"]).output,
            Some(OutputFormat::Json)
        );
        assert_eq!(parse(&["--output=Text"]).output, Some(OutputFormat::Text));
        assert_eq!(
            parse(&["--output", "text"]).output,
            Some(OutputFormat::Text)
        );
        assert_eq!(
            parse(&["--output", "json"]).output,
            Some(OutputFormat::Json)
        );
        assert_eq!(parse(&[]).output, None);
        let err = RunArgs::try_parse_from(["aye", "--output", "xml"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        let err = err.to_string();
        assert!(err.contains("[possible values: text, json]"), "{err}");
    }

    #[test]
    fn the_output_formats_old_names_say_what_to_use_instead() {
        use clap::CommandFactory as _;
        for (name, instead) in [
            (
                "human",
                "`human` is now `text`, the default: use `--output text`",
            ),
            (
                "Human",
                "`human` is now `text`, the default: use `--output text`",
            ),
            (
                "agent",
                "`agent` is now `json` at `-v`: use `--output json -v`",
            ),
            (
                "AGENT",
                "`agent` is now `json` at `-v`: use `--output json -v`",
            ),
        ] {
            let err = RunArgs::try_parse_from(["aye", "--output", name]).unwrap_err();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{name}"
            );
            let err = err.to_string();
            assert!(err.contains(instead), "{name}: {err}");
            assert_eq!(name.parse::<OutputFormat>(), Err(instead.to_string()));
        }
        let help = RunArgs::command().render_long_help().to_string();
        assert!(
            help.contains("- text:") && help.contains("- json:"),
            "{help}"
        );
        assert!(
            !help.contains("human") && !help.contains("- agent"),
            "{help}"
        );
    }
}
