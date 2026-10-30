//! What an argument vector says, read as pytest's argparse reads it: the
//! arguments after `--`, which are the framework's (what they say of `-k`,
//! `-m` and `maxfail`, and whether they replace the test run), and the
//! values aye's own command line gives where pytest's rules decide
//! (`-k`/`-m` values starting with `-`, `-x` and `--maxfail`).
//!
//! Decided (implementation, October 2026):
//! - **The arguments after `--`** are the framework's: aye's pytest
//!   front-end parses them after `addopts` and `PYTEST_ADDOPTS`, as pytest
//!   would, so those that replace a test run (pytest's `-h`, `-V`, `--co`,
//!   `--fixtures`, `--fixtures-per-test`, `--markers`, also inside a cluster
//!   such as `-vh`) are usage errors naming the aye equivalent, before
//!   anything runs. So are `--setup-plan` and `--setup-only`: they run no
//!   test, and every test would be reported, and remembered for `--lf`, as
//!   skipped. `--setup-show` runs the tests and is allowed.

use aye_pytest::argv::is_value;

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

impl Maxfail {
    /// What aye's own `-x` and `--maxfail N` ask for: the last given, as
    /// clap keeps it.
    pub(crate) const fn given(exitfirst: bool, maxfail: Option<u32>) -> Self {
        match (exitfirst, maxfail) {
            (true, _) => Self::After(1),
            (false, None) => Self::Unset,
            (false, Some(0)) => Self::Unlimited,
            (false, Some(n)) => Self::After(n),
        }
    }
}

/// What clap can't check of aye's own `-k` and `-m`: a value starting with
/// `-` is accepted only where pytest's argparse accepts it, when it looks
/// like a negative number or contains a space; otherwise it was meant as a
/// flag.
pub(crate) fn check_expressions(keyword: Option<&str>, tags: Option<&str>) -> Result<(), String> {
    for (flag, value) in [("-k", keyword), ("-m", tags)] {
        if let Some(value) = value
            && !is_value(value)
        {
            return Err(format!("argument {flag}: expected one argument"));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use aye_pytest::argv::replaced_run;
    use clap::Parser as _;

    use super::*;
    use crate::cli::RunArgs;

    fn parse(argv: &[&str]) -> RunArgs {
        RunArgs::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
    }

    /// What `check_expressions` and `replaced_run` say of a run's arguments.
    fn check(args: &RunArgs) -> Result<(), String> {
        check_expressions(args.keyword.as_deref(), args.tags.as_deref())?;
        replaced_run(&args.passthrough).map_or(Ok(()), Err)
    }

    fn maxfail(args: &RunArgs) -> Maxfail {
        Maxfail::given(args.exitfirst, args.maxfail)
    }

    #[test]
    fn the_last_of_exitfirst_and_maxfail_wins() {
        assert_eq!(
            maxfail(&parse(&["-x", "--maxfail", "3"])),
            Maxfail::After(3)
        );
        assert_eq!(
            maxfail(&parse(&["--maxfail", "3", "-x"])),
            Maxfail::After(1)
        );
        assert_eq!(
            maxfail(&parse(&["--maxfail=3", "--exitfirst"])),
            Maxfail::After(1)
        );
        assert_eq!(
            maxfail(&parse(&["--maxfail", "2", "--maxfail", "5"])),
            Maxfail::After(5)
        );
        assert_eq!(maxfail(&parse(&["-x", "-x"])), Maxfail::After(1));
        assert_eq!(maxfail(&parse(&["--maxfail", "0"])), Maxfail::Unlimited);
        assert_eq!(
            maxfail(&parse(&["-x", "--maxfail", "0"])),
            Maxfail::Unlimited
        );
        assert_eq!(maxfail(&parse(&[])), Maxfail::Unset);
    }

    #[test]
    fn expressions_may_start_with_a_dash_where_pytest_allows_it() {
        let args = parse(&["-k", "-1", "-m", "-2.5", "tests"]);
        assert_eq!(args.keyword.as_deref(), Some("-1"));
        assert_eq!(args.tags.as_deref(), Some("-2.5"));
        assert_eq!(args.tests, ["tests"]);
        assert!(check(&args).is_ok());
        assert!(check(&parse(&["-k", "-1 and x"])).is_ok());
        assert!(check(&parse(&["-k", "-.5"])).is_ok());
        assert_eq!(
            check(&parse(&["-k", "-x"])),
            Err("argument -k: expected one argument".into())
        );
        assert!(check(&parse(&["-m", "--lf"])).is_err());
        assert!(check(&parse(&["-k", "-1."])).is_err());
        // The last one given wins, as in pytest.
        assert_eq!(parse(&["-k", "a", "-k", "b"]).keyword.as_deref(), Some("b"));
        // Counted and repeated arguments still add up.
        let args = parse(&["-v", "-v", "-q", "a", "b", "-v", "--", "-x"]);
        assert_eq!((args.verbose, args.quiet), (3, 1));
        assert_eq!(args.tests, ["a", "b"]);
        assert_eq!(args.passthrough, ["-x"]);
    }

    #[test]
    fn framework_args_that_replace_the_run_are_usage_errors() {
        let check = |framework: &[&str]| {
            let mut argv = vec!["tests", "--"];
            argv.extend(framework);
            check(&parse(&argv))
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
