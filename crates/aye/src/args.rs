use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};
use clap::{ArgAction, Parser, ValueEnum};

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
pub(crate) struct Args {
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
    pub workers: Option<Workers>,

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
        value_parser = Output::parser()
    )]
    pub output: Option<Output>,

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

    /// Accepted and does nothing: aye's own engine always runs pytest's
    /// tests, without pytest (this was the prototype's switch).
    #[arg(long)]
    pub native: bool,

    /// Arguments for the framework.
    #[arg(last = true, value_name = "ARGS")]
    pub passthrough: Vec<String>,
}

impl Args {
    /// What clap can't check: a `-k` or `-m` value starting with `-` is
    /// accepted only where pytest's argparse accepts it, when it looks like a
    /// negative number or contains a space; otherwise it was meant as a flag.
    /// And the arguments after `--` must not turn the framework's session
    /// into something other than a test run ([`replaced_run`]).
    pub(crate) fn check(&self) -> Result<(), String> {
        for (flag, value) in [("-k", &self.keyword), ("-m", &self.tags)] {
            if let Some(value) = value
                && !is_value(value)
            {
                return Err(format!("argument {flag}: expected one argument"));
            }
        }
        if let Some(problem) = replaced_run(&self.passthrough) {
            return Err(problem);
        }
        Ok(())
    }

    /// How much the output shows: the number of `-v` less the number of
    /// `-q`, from -1 (`-q`: the summary alone) to 3 (`-vvv`: everything).
    /// A batch's policy carries it unclamped ([`Args::framework_verbosity`]).
    pub(crate) fn verbosity(&self) -> i8 {
        let net = (i16::from(self.verbose) - i16::from(self.quiet)).clamp(-1, 3);
        i8::try_from(net).unwrap_or_default()
    }

    /// The verbosity a batch's policy carries (`Policy::verbosity`): the
    /// number of `-v` less the number of `-q`, unclamped, as pytest counts
    /// them.
    pub(crate) fn framework_verbosity(&self) -> i8 {
        let net = i16::from(self.verbose) - i16::from(self.quiet);
        i8::try_from(net.clamp(i16::from(i8::MIN), i16::from(i8::MAX))).unwrap_or_default()
    }

    /// The `maxfail` the command line asks for.
    pub(crate) fn maxfail(&self) -> Maxfail {
        match (self.exitfirst, self.maxfail) {
            (true, _) => Maxfail::After(1),
            (false, None) => Maxfail::Unset,
            (false, Some(0)) => Maxfail::Unlimited,
            (false, Some(n)) => Maxfail::After(n),
        }
    }
}

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
        value_parser = Output::parser()
    )]
    pub output: Option<Output>,

    /// When to use colour.
    #[arg(long, value_enum, default_value_t = Color::Auto, value_name = "WHEN")]
    pub color: Color,
}

impl ShowArgs {
    /// A `-k` or `-m` value starting with `-`, as a run checks it
    /// ([`Args::check`]).
    pub(crate) fn check(&self) -> Result<(), String> {
        for (flag, value) in [("-k", &self.keyword), ("-m", &self.tags)] {
            if let Some(value) = value
                && !is_value(value)
            {
                return Err(format!("argument {flag}: expected one argument"));
            }
        }
        Ok(())
    }

    /// As a run's ([`Args::verbosity`]) when `-v` or `-q` is given. Without
    /// either, `details` (of tests named, or `--failed`) come at the detail
    /// ladder's second level, which is text's 1 and JSON's 2; the list at a
    /// run's default, 0.
    pub(crate) fn verbosity(&self, details: bool, format: Output) -> i8 {
        if details && self.verbose == 0 && self.quiet == 0 {
            return match format {
                Output::Text => 1,
                Output::Json => 2,
            };
        }
        let net = (i16::from(self.verbose) - i16::from(self.quiet)).clamp(-1, 3);
        i8::try_from(net).unwrap_or_default()
    }
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

/// What the command line says about `maxfail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Maxfail {
    /// Neither `-x` nor `--maxfail`.
    Unset,
    /// `--maxfail 0`.
    Unlimited,
    /// Stop after this many bad outcomes.
    After(u32),
}

/// pytest's options that replace the test run with something else (help, a
/// listing, a fixture plan): each batch's session would print it to a
/// hidden stream and run no test. With what to do instead.
const REPLACING: &[(&[&str], &str, &str)] = &[
    (
        &["--collect-only", "--co", "--collectonly"],
        "would make pytest collect the tests without running them",
        "use `aye --co`",
    ),
    (
        &["-h", "--help"],
        "would make pytest show its help instead of running the tests",
        "use `aye --help`, or `python -m pytest --help` for pytest's options",
    ),
    (
        &["-V", "--version"],
        "would make pytest show its version instead of running the tests",
        "use `aye --version`, or `python -m pytest --version`",
    ),
    (
        &["--fixtures", "--funcargs"],
        "would make pytest list the fixtures instead of running the tests",
        "aye has no equivalent yet: run `python -m pytest --fixtures`",
    ),
    (
        &["--fixtures-per-test"],
        "would make pytest list each test's fixtures instead of running the tests",
        "aye has no equivalent yet: run `python -m pytest --fixtures-per-test`",
    ),
    (
        &["--markers"],
        "would make pytest list the markers instead of running the tests",
        "aye has no equivalent yet: run `python -m pytest --markers`",
    ),
    // These two do run each batch, but no test in it: every test would be
    // reported (and remembered in history) as skipped. `--setup-show` runs
    // the tests and stays allowed.
    (
        &["--setup-plan", "--setupplan"],
        "would make pytest plan the fixtures instead of running the tests",
        "aye has no equivalent yet: run `python -m pytest --setup-plan`",
    ),
    (
        &["--setup-only", "--setuponly"],
        "would make pytest set up the fixtures instead of running the tests",
        "aye has no equivalent yet: run `python -m pytest --setup-only`",
    ),
];

/// pytest's short options that take no value, which argparse lets a cluster
/// combine (`-vh` is `-v -h`).
const SHORT_FLAGS: &str = "vqlsxhV";

/// pytest's short options that take a value, which ends a cluster (`-vkx`
/// is `-v -k x`).
const SHORT_VALUED: &str = "kmporWc";

/// The first argument after `--` that would replace the framework's test
/// run (see [`REPLACING`]), as a usage error; `None` when there is none.
/// Long options are matched whole, since pytest doesn't take abbreviations;
/// `-h` and `-V` also inside a cluster of short flags. A lone `--` ends the
/// options, as in pytest.
fn replaced_run(passthrough: &[String]) -> Option<String> {
    for arg in passthrough.iter().take_while(|arg| *arg != "--") {
        let mut names = vec![arg.as_str()];
        if let Some(cluster) = arg.strip_prefix('-').filter(|rest| !rest.starts_with('-')) {
            for flag in cluster.chars().take_while(|c| SHORT_FLAGS.contains(*c)) {
                match flag {
                    'h' => names.push("-h"),
                    'V' => names.push("-V"),
                    _ => {}
                }
            }
        }
        for name in names {
            if let Some((_, what, instead)) = REPLACING.iter().find(|(o, ..)| o.contains(&name)) {
                return Some(format!("`-- {arg}` {what}; {instead}"));
            }
        }
    }
    None
}

/// What the arguments after `--` say about one of pytest's short options
/// that take a value (`-k`, `-m`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Passed {
    /// They don't give it.
    Unset,
    /// Its last value. They come last in the framework's arguments, after
    /// the command line's own `-k` and `-m`, so it is the one that counts.
    Set(String),
    /// Can't tell: an `@file`, a short option aye doesn't know, a missing
    /// value.
    Unknown,
}

/// Read the arguments after `--` for the short option `option`, as
/// argparse does: alone with its value next (`-k x`), attached (`-kx`), or
/// ending a cluster of flags (`-vk x`, `-vkx`). A lone `--` ends the options.
pub(crate) fn passed(passthrough: &[String], option: char) -> Passed {
    let mut found = Passed::Unset;
    let mut args = passthrough.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('@') {
            return Passed::Unknown;
        }
        let Some(cluster) = arg
            .strip_prefix('-')
            .filter(|rest| !rest.is_empty() && !rest.starts_with('-'))
        else {
            continue;
        };
        for (at, flag) in cluster.char_indices() {
            if SHORT_FLAGS.contains(flag) {
                continue;
            }
            let attached = &cluster[at + flag.len_utf8()..];
            if !SHORT_VALUED.contains(flag) {
                // A plugin's option, or a negative number: whether it takes
                // a value or not, what follows it sets `option` only if it
                // holds it.
                if attached.contains(option) {
                    return Passed::Unknown;
                }
                break;
            }
            let value = if !attached.is_empty() {
                Some(attached.to_string())
            } else if args.peek().is_some_and(|next| is_value(next)) {
                args.next().cloned()
            } else {
                None
            };
            if flag == option {
                found = value.map_or(Passed::Unknown, Passed::Set);
            }
            break;
        }
    }
    found
}

/// pytest's short options that take no value, as the pytest adapter reads
/// them when it lifts `-x` out of a cluster (pytest-xdist adds `-d`, `-f`).
const MAXFAIL_SHORT_FLAGS: &str = "xvqslhVdf";

/// pytest's short options that take a value, as the pytest adapter reads
/// them (pytest-xdist adds `-n`).
const MAXFAIL_SHORT_VALUED: &str = "kmcpoWrn";

/// What the arguments after `--` say about `maxfail`: the last of `-x`,
/// `--exitfirst` and `--maxfail N` (alone, as `--maxfail=N`, or `-x` in a
/// cluster such as `-vx`), as argparse keeps the last; `None` when none is
/// given or they can't be read (an `@file`). Read as the pytest adapter
/// reads them when it lifts them out for the runner: a cluster holding an
/// option pytest doesn't know is left alone, a `--maxfail` value pytest
/// would reject sets nothing, and a negative one stops at the first
/// failure, as pytest does. A lone `--` ends the options.
pub(crate) fn passed_maxfail(passthrough: &[String]) -> Option<Maxfail> {
    fn limit(value: &str) -> Option<Maxfail> {
        let n: i64 = value.trim().parse().ok()?;
        Some(match n {
            0 => Maxfail::Unlimited,
            n if n < 0 => Maxfail::After(1),
            n => Maxfail::After(u32::try_from(n).unwrap_or(u32::MAX)),
        })
    }
    let mut found = None;
    let mut args = passthrough.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('@') {
            return None;
        }
        if arg == "--exitfirst" {
            found = Some(Maxfail::After(1));
            continue;
        }
        if arg == "--maxfail" {
            if let Some(value) = args.peek().and_then(|value| limit(value)) {
                args.next();
                found = Some(value);
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--maxfail=") {
            found = limit(value).or(found);
            continue;
        }
        let Some(cluster) = arg
            .strip_prefix('-')
            .filter(|rest| !rest.is_empty() && !rest.starts_with('-'))
        else {
            continue;
        };
        // `-x` counts when every option before the first that takes a
        // value is one pytest knows.
        let mut exitfirst = false;
        for flag in cluster.chars() {
            if flag == 'x' {
                exitfirst = true;
            } else if MAXFAIL_SHORT_VALUED.contains(flag) {
                break;
            } else if !MAXFAIL_SHORT_FLAGS.contains(flag) {
                exitfirst = false;
                break;
            }
        }
        if exitfirst {
            found = Some(Maxfail::After(1));
        }
        // A cluster ending in an option that takes a value takes the next
        // argument as it.
        let valued = cluster
            .chars()
            .position(|c| MAXFAIL_SHORT_VALUED.contains(c));
        if valued.is_some_and(|at| at + 1 == cluster.chars().count()) {
            args.next();
        }
    }
    found
}

/// Whether argparse takes `arg` as an option's value rather than as an
/// option: it doesn't start with `-`, or is `-`, holds a space or looks
/// like a negative number.
fn is_value(arg: &str) -> bool {
    !arg.starts_with('-') || arg == "-" || arg.contains(' ') || looks_like_negative_number(arg)
}

/// argparse's test: `-1`, `-1.5`, `-.5`.
fn looks_like_negative_number(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('-') else {
        return false;
    };
    let (whole, fraction) = digits
        .split_once('.')
        .map_or((digits, None), |(w, f)| (w, Some(f)));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match fraction {
        None => !whole.is_empty() && all_digits(whole),
        Some(fraction) => !fraction.is_empty() && all_digits(whole) && all_digits(fraction),
    }
}

/// The output formats. How much each shows is the verbosity's
/// ([`Args::verbosity`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Output {
    /// For people: quiet on success, readable failures.
    Text,
    /// JSON lines, for programs and coding agents.
    Json,
}

impl Output {
    /// Its name, as `--output` and `[tool.aye] output` take it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
        }
    }

    /// `--output`'s parser: the formats in any case, listed in the help,
    /// and the names they had until October 2026 (`human`, `agent`; never
    /// published), hidden, rejected by [`Output::from_str`] with what to
    /// use instead.
    fn parser() -> impl TypedValueParser<Value = Self> {
        PossibleValuesParser::new([
            PossibleValue::new("text").help("For people: quiet on success, readable failures"),
            PossibleValue::new("json").help(
                "JSON lines: the summary with -q, bad outcomes, failures in full with -v \
                 (for coding agents), every result with -vv, the raw event stream with -vvv",
            ),
            PossibleValue::new("human").hide(true),
            PossibleValue::new("agent").hide(true),
        ])
        .try_map(|name| name.parse::<Self>())
    }
}

impl FromStr for Output {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "human" => Err("`human` is now `text`, the default: use `--output text`".into()),
            "agent" => Err("`agent` is now `json` at `-v`: use `--output json -v`".into()),
            _ => Err(format!("expected `text` or `json`, got `{s}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Color {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Workers {
    Auto,
    Count(u32),
}

impl Workers {
    pub(crate) fn resolve(self) -> u32 {
        match self {
            Self::Count(n) => n.max(1),
            Self::Auto => std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(1)),
        }
    }
}

impl FromStr for Workers {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" | "logical" => Ok(Self::Auto),
            n => n
                .parse()
                .map(Self::Count)
                .map_err(|_| format!("expected a number or `auto`, got `{n}`")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--native`, the prototype's switch, is still accepted, alone or with
    /// anything else, and its help says it does nothing.
    #[test]
    fn native_is_accepted_and_does_nothing() {
        use clap::CommandFactory as _;
        for argv in [
            &["aye", "--native"][..],
            &["aye", "--native", "--co", "--no-refine"],
            &["aye", "--native", "--no-refine"],
        ] {
            let args = Args::try_parse_from(argv).unwrap();
            assert!(args.native && args.check().is_ok(), "{argv:?}");
        }
        let help = Args::command().render_long_help().to_string();
        assert!(
            help.contains("--native") && help.contains("does nothing"),
            "{help}"
        );
    }

    #[test]
    fn pytest_style_flags_parse() {
        let args = Args::try_parse_from([
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
        assert_eq!(args.workers, Some(Workers::Auto));
        assert_eq!(args.junit_xml, Some(PathBuf::from("out.xml")));
        assert_eq!(args.passthrough, ["--reuse-db"]);
    }

    fn parse(argv: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
    }

    #[test]
    fn the_last_of_exitfirst_and_maxfail_wins() {
        assert_eq!(
            parse(&["-x", "--maxfail", "3"]).maxfail(),
            Maxfail::After(3)
        );
        assert_eq!(
            parse(&["--maxfail", "3", "-x"]).maxfail(),
            Maxfail::After(1)
        );
        assert_eq!(
            parse(&["--maxfail=3", "--exitfirst"]).maxfail(),
            Maxfail::After(1)
        );
        assert_eq!(
            parse(&["--maxfail", "2", "--maxfail", "5"]).maxfail(),
            Maxfail::After(5)
        );
        assert_eq!(parse(&["-x", "-x"]).maxfail(), Maxfail::After(1));
        assert_eq!(parse(&["--maxfail", "0"]).maxfail(), Maxfail::Unlimited);
        assert_eq!(
            parse(&["-x", "--maxfail", "0"]).maxfail(),
            Maxfail::Unlimited
        );
        assert_eq!(parse(&[]).maxfail(), Maxfail::Unset);
    }

    #[test]
    fn expressions_may_start_with_a_dash_where_pytest_allows_it() {
        let args = parse(&["-k", "-1", "-m", "-2.5", "tests"]);
        assert_eq!(args.keyword.as_deref(), Some("-1"));
        assert_eq!(args.tags.as_deref(), Some("-2.5"));
        assert_eq!(args.tests, ["tests"]);
        assert!(args.check().is_ok());
        assert!(parse(&["-k", "-1 and x"]).check().is_ok());
        assert!(parse(&["-k", "-.5"]).check().is_ok());
        assert_eq!(
            parse(&["-k", "-x"]).check(),
            Err("argument -k: expected one argument".into())
        );
        assert!(parse(&["-m", "--lf"]).check().is_err());
        assert!(parse(&["-k", "-1."]).check().is_err());
        // The last one given wins, as in pytest.
        assert_eq!(parse(&["-k", "a", "-k", "b"]).keyword.as_deref(), Some("b"));
        // Counted and repeated arguments still add up.
        let args = parse(&["-v", "-v", "-q", "a", "b", "-v", "--", "-x"]);
        assert_eq!((args.verbose, args.quiet), (3, 1));
        assert_eq!(args.tests, ["a", "b"]);
        assert_eq!(args.passthrough, ["-x"]);
    }

    #[test]
    fn framework_args_are_read_for_k_and_m_as_argparse_reads_them() {
        let read = |args: &[&str], option| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            passed(&args, option)
        };
        let set = |value: &str| Passed::Set(value.into());
        for (args, want) in [
            (&[][..], Passed::Unset),
            (&["-v", "--tb=short", "tests"], Passed::Unset),
            (&["-k", "fast"], set("fast")),
            (&["-kfast"], set("fast")),
            (&["-vk", "fast"], set("fast")),
            (&["-vqkfast"], set("fast")),
            (&["-k", "-1"], set("-1")),
            (&["-k", "a", "-k", "b"], set("b")),
            // `-m`'s value isn't `-k`, and `-r`'s neither.
            (&["-mk"], Passed::Unset),
            (&["-rk", "-p", "k"], Passed::Unset),
            (&["-p", "-k", "x"], set("x")),
            // A missing value, or one argparse wouldn't take.
            (&["-k"], Passed::Unknown),
            (&["-k", "--tb=short"], Passed::Unknown),
            // A plugin's options: unknown only where `k` could be theirs.
            (&["-n", "4", "-k", "x"], set("x")),
            (&["-n4"], Passed::Unset),
            (&["-Zk", "x"], Passed::Unknown),
            (&["@args.txt"], Passed::Unknown),
            // Long options don't abbreviate in pytest.
            (&["--keyword", "x"], Passed::Unset),
            // After a lone `--`, everything is a path.
            (&["--", "-k", "x"], Passed::Unset),
        ] {
            assert_eq!(read(args, 'k'), want, "{args:?}");
        }
        assert_eq!(read(&["-vmdb", "-k", "x"], 'm'), set("db"));
        assert_eq!(read(&["-k", "m"], 'm'), Passed::Unset);
    }

    #[test]
    fn framework_args_that_replace_the_run_are_usage_errors() {
        let check = |framework: &[&str]| {
            let mut argv = vec!["tests", "--"];
            argv.extend(framework);
            parse(&argv).check()
        };
        assert_eq!(
            check(&["--co"]),
            Err(
                "`-- --co` would make pytest collect the tests without running them; use `aye --co`"
                    .into()
            )
        );
        for args in [
            &["--collect-only"][..],
            &["--collectonly"],
            &["-h"],
            &["--help"],
            &["-V"],
            &["--version"],
            &["--fixtures"],
            &["--funcargs"],
            &["--fixtures-per-test"],
            &["--markers"],
            // They run no test, and every test would finish as skipped.
            &["--setup-plan"],
            &["--setupplan"],
            &["--setup-only"],
            &["--setuponly"],
            &["-v", "--setup-only", "--setup-show"],
            // In a cluster of short flags, as argparse reads it.
            &["-vh"],
            &["-qV"],
            &["--tb=short", "--markers"],
        ] {
            assert!(check(args).is_err(), "{args:?}");
        }
        let help = check(&["-vh"]).unwrap_err();
        assert!(
            help.starts_with("`-- -vh` would make pytest show its help"),
            "{help}"
        );
        assert!(
            check(&["--fixtures"])
                .unwrap_err()
                .contains("no equivalent")
        );
        assert_eq!(
            check(&["--setup-plan"]),
            Err(
                "`-- --setup-plan` would make pytest plan the fixtures instead of running the \
                 tests; aye has no equivalent yet: run `python -m pytest --setup-plan`"
                    .into()
            )
        );
        for args in [
            &[][..],
            // It runs the tests.
            &["--setup-show"],
            &["--setupshow"],
            &["-v", "--tb=short"],
            // A value, not a flag: `-r h` and `-k h` show nothing.
            &["-rh"],
            &["-kh"],
            &["-p", "no:help"],
            // Not abbreviations: pytest has none.
            &["--fix"],
            &["--helpful-plugin-flag"],
            // After a lone `--`, arguments are paths.
            &["--", "--help"],
            &["--", "--setup-plan"],
        ] {
            assert_eq!(check(args), Ok(()), "{args:?}");
        }
    }

    #[test]
    fn the_output_format_takes_any_case() {
        assert_eq!(parse(&["--output", "JSON"]).output, Some(Output::Json));
        assert_eq!(parse(&["--output=Text"]).output, Some(Output::Text));
        assert_eq!(parse(&["--output", "text"]).output, Some(Output::Text));
        assert_eq!(parse(&["--output", "json"]).output, Some(Output::Json));
        assert_eq!(parse(&[]).output, None);
        let err = Args::try_parse_from(["aye", "--output", "xml"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidValue);
        let err = err.to_string();
        assert!(err.contains("[possible values: text, json]"), "{err}");
    }

    /// The formats' old names (`human`, `agent`), in any case, are errors
    /// that say what to use instead; the help doesn't list them.
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
            let err = Args::try_parse_from(["aye", "--output", name]).unwrap_err();
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{name}"
            );
            let err = err.to_string();
            assert!(err.contains(instead), "{name}: {err}");
            assert_eq!(name.parse::<Output>(), Err(instead.to_string()));
        }
        let help = Args::command().render_long_help().to_string();
        assert!(
            help.contains("- text:") && help.contains("- json:"),
            "{help}"
        );
        assert!(
            !help.contains("human") && !help.contains("- agent"),
            "{help}"
        );
    }

    /// The output's verbosity is `-v`s less `-q`s, from -1 to 3; the
    /// framework's isn't clamped.
    #[test]
    fn verbosity_is_clamped_for_the_output_alone() {
        for (argv, output, framework) in [
            (&[][..], 0, 0),
            (&["-q"], -1, -1),
            (&["-qq"], -1, -2),
            (&["-qqq", "-v"], -1, -2),
            (&["-v"], 1, 1),
            (&["-vv"], 2, 2),
            (&["-vvv"], 3, 3),
            (&["-vvvv"], 3, 4),
            (&["-v", "-q"], 0, 0),
            (&["-vv", "-q"], 1, 1),
            (&["--verbose", "--verbose", "--quiet"], 1, 1),
        ] {
            let args = parse(argv);
            assert_eq!(args.verbosity(), output, "{argv:?}");
            assert_eq!(args.framework_verbosity(), framework, "{argv:?}");
        }
        let many: Vec<&str> = std::iter::repeat_n("-v", 200).collect();
        assert_eq!(parse(&many).verbosity(), 3);
        assert_eq!(parse(&many).framework_verbosity(), 127);
    }

    #[test]
    fn framework_args_are_read_for_maxfail_as_the_adapter_lifts_them() {
        let read = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            passed_maxfail(&args)
        };
        let after = |n| Some(Maxfail::After(n));
        for (args, want) in [
            (&[][..], None),
            (&["-v", "--tb=short", "tests"], None),
            (&["-x"], after(1)),
            (&["--exitfirst"], after(1)),
            (&["--maxfail", "3"], after(3)),
            (&["--maxfail=3"], after(3)),
            (&["--maxfail", "0"], Some(Maxfail::Unlimited)),
            (&["--maxfail=-2"], after(1)),
            // The last one wins, as in argparse.
            (&["-x", "--maxfail", "3"], after(3)),
            (&["--maxfail", "3", "-x"], after(1)),
            (&["--maxfail", "3", "--maxfail=0"], Some(Maxfail::Unlimited)),
            // A value pytest rejects sets nothing (pytest reports it).
            (&["--maxfail", "many"], None),
            (&["-x", "--maxfail=many"], after(1)),
            // In a cluster, as argparse reads it.
            (&["-vx"], after(1)),
            (&["-xvs"], after(1)),
            (&["-xk", "fast"], after(1)),
            // `-k`'s value, not `-x`.
            (&["-kx"], None),
            (&["-k", "-x"], None),
            (&["-rx"], None),
            (&["-vk", "-x"], None),
            // A cluster holding an option pytest doesn't know is left alone.
            (&["-xZ"], None),
            (&["-n", "4", "-x"], after(1)),
            (&["@args.txt", "-x"], None),
            // After a lone `--`, everything is a path.
            (&["--", "-x"], None),
            // Long options don't abbreviate in pytest.
            (&["--exit", "--maxf", "2"], None),
        ] {
            assert_eq!(read(args), want, "{args:?}");
        }
    }
}
