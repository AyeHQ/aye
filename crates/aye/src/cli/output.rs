//! What a run's and `aye show`'s command lines share: how much the output
//! shows ([`Verbosity`]), when to use colour ([`Color`]), and `--output`'s
//! parser. Each command keeps its own `-v`, `-q`, `--output` and `--color`,
//! in its own place and words in its help.

use std::fmt;

use aye_config::OutputFormat;
use aye_render::Level;
use clap::ValueEnum;
use clap::builder::{PossibleValue, PossibleValuesParser, TypedValueParser};

/// How much the output shows, as the command line asks it: the number of
/// `-v` and of `-q`, which a run and `aye show` each take.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Verbosity {
    verbose: u8,
    quiet: u8,
}

impl Verbosity {
    pub(crate) const fn new(verbose: u8, quiet: u8) -> Self {
        Self { verbose, quiet }
    }

    /// The renderers' level: the number of `-v` less the number of `-q`,
    /// from -1 (`-q`: the summary alone) to 3 (`-vvv`: everything). A
    /// batch's policy carries the count unclamped ([`Verbosity::framework`]).
    pub(crate) fn level(self) -> Level {
        Level::from_count(self.net())
    }

    /// What a batch's policy carries (`Policy::verbosity`): the number of
    /// `-v` less the number of `-q`, unclamped, as pytest counts them.
    pub(crate) fn framework(self) -> i8 {
        let net = self.net().clamp(i16::from(i8::MIN), i16::from(i8::MAX));
        i8::try_from(net).unwrap_or_default()
    }

    /// Neither `-v` nor `-q` (`-v -q` is both: a run's default).
    pub(crate) const fn is_unset(self) -> bool {
        self.verbose == 0 && self.quiet == 0
    }

    fn net(self) -> i16 {
        i16::from(self.verbose) - i16::from(self.quiet)
    }
}

/// As `aye config` shows it after the format, as the command line would
/// ask for its level: ` -q`, nothing, ` -v`, ` -vv` or ` -vvv`.
impl fmt::Display for Verbosity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.level() {
            Level::Quiet => " -q",
            Level::Default => "",
            Level::Verbose => " -v",
            Level::VeryVerbose => " -vv",
            Level::Engine => " -vvv",
        })
    }
}

/// `--output`'s parser: the formats in any case, listed in the help, and
/// the names they had until October 2026 (`human`, `agent`; never
/// published), hidden, rejected by [`OutputFormat::from_str`] with what to
/// use instead. How much each shows is the verbosity's ([`Verbosity`]).
pub(super) fn output_format() -> impl TypedValueParser<Value = OutputFormat> {
    PossibleValuesParser::new([
        PossibleValue::new("text").help("For people: quiet on success, readable failures"),
        PossibleValue::new("json").help(
            "JSON lines: the summary with -q, bad outcomes, failures in full with -v \
             (for coding agents), every result with -vv, the raw event stream with -vvv",
        ),
        PossibleValue::new("human").hide(true),
        PossibleValue::new("agent").hide(true),
    ])
    .try_map(|name| name.parse::<OutputFormat>())
}

/// When to use colour: `--color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Color {
    Auto,
    Always,
    Never,
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::*;
    use crate::cli::RunArgs;

    fn parse(argv: &[&str]) -> RunArgs {
        RunArgs::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
    }

    fn verbosity(argv: &[&str]) -> Verbosity {
        let parsed = parse(argv);
        Verbosity::new(parsed.verbose, parsed.quiet)
    }

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
            let verbosity = verbosity(argv);
            assert_eq!(verbosity.level().count(), output, "{argv:?}");
            assert_eq!(verbosity.framework(), framework, "{argv:?}");
        }
        let many: Vec<&str> = std::iter::repeat_n("-v", 200).collect();
        assert_eq!(verbosity(&many).level(), Level::Engine);
        assert_eq!(verbosity(&many).framework(), 127);
        let quiet: Vec<&str> = std::iter::repeat_n("-q", 200).collect();
        assert_eq!(verbosity(&quiet).framework(), -128);
        for (argv, shown) in [
            (&[][..], ""),
            (&["-q"], " -q"),
            (&["-qqq"], " -q"),
            (&["-v"], " -v"),
            (&["-vv", "-q"], " -v"),
            (&["-vv"], " -vv"),
            (&["-vvvvv"], " -vvv"),
            (&["-v", "-q"], ""),
        ] {
            assert_eq!(verbosity(argv).to_string(), shown, "{argv:?}");
        }
    }
}
