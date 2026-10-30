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
pub(crate) fn replaced_run(passthrough: &[String]) -> Option<String> {
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

#[cfg(test)]
mod tests {
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
