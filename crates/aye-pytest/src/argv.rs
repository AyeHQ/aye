//! pytest's command line as its argparse reads it: what an argument list
//! says of an option, which of pytest's options replace the test run, and
//! the options aye lifts out of pytest's arguments, since the runner, aye
//! itself or the doctest adapter does what they ask.

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

/// The first argument after `--` that would replace the framework's test
/// run (see `REPLACING`), as a usage error; `None` when there is none.
/// Long options are matched whole, since pytest doesn't take abbreviations;
/// `-h` and `-V` also inside a cluster of short flags. A lone `--` ends the
/// options, as in pytest.
pub fn replaced_run(passthrough: &[String]) -> Option<String> {
    for arg in passthrough.iter().take_while(|arg| *arg != "--") {
        let mut names = vec![arg.as_str()];
        if let Some(cluster) = arg.strip_prefix('-').filter(|rest| !rest.starts_with('-')) {
            let flags = ShortOptions::PYTEST.flags;
            for flag in cluster.chars().take_while(|c| flags.contains(*c)) {
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

/// pytest's short options, which argparse lets a cluster combine (`-vh` is
/// `-v -h`) up to one that takes a value: the rest of the cluster is that
/// value, or else the next argument (`-vkx` is `-v -k x`, and `-rx` is
/// `-r x`, not `-x`). Two sets are read, each where it always was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShortOptions {
    /// Those that take no value.
    pub flags: &'static str,
    /// Those that take one.
    pub valued: &'static str,
}

impl ShortOptions {
    /// pytest's own, as the binary reads the arguments after `--` for `-k`
    /// and `-m` and for the options that replace the test run.
    pub const PYTEST: Self = Self {
        flags: "vqlsxhV",
        valued: "kmporWc",
    };

    /// pytest's with pytest-xdist's (`-d` and `-f`; `-n`), as the pytest
    /// adapter lifts `-x` out of a cluster, and as the binary and the
    /// doctest adapter read `-x`.
    pub const WITH_XDIST: Self = Self {
        flags: "xvqslhVdf",
        valued: "kmcpoWrn",
    };
}

/// What an argument list says about one of pytest's options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgValue<T> {
    /// It doesn't give it.
    Unset,
    /// Its last value, the one that counts, as argparse keeps the last. The
    /// arguments after `--` come last in the framework's arguments, after
    /// the command line's own `-k`, `-m` and `-x`, so theirs counts.
    Set(T),
    /// Can't tell: an `@file`, a short option aye doesn't know, a missing
    /// value.
    Unreadable,
}

/// Read the arguments after `--` for the short option `option`, as
/// argparse does: alone with its value next (`-k x`), attached (`-kx`), or
/// ending a cluster of flags (`-vk x`, `-vkx`). A lone `--` ends the options.
pub fn passed(passthrough: &[String], option: char) -> ArgValue<String> {
    let ShortOptions { flags, valued } = ShortOptions::PYTEST;
    let mut found = ArgValue::Unset;
    let mut args = passthrough.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('@') {
            return ArgValue::Unreadable;
        }
        let Some(cluster) = arg
            .strip_prefix('-')
            .filter(|rest| !rest.is_empty() && !rest.starts_with('-'))
        else {
            continue;
        };
        for (at, flag) in cluster.char_indices() {
            if flags.contains(flag) {
                continue;
            }
            let attached = &cluster[at + flag.len_utf8()..];
            if !valued.contains(flag) {
                // A plugin's option, or a negative number: whether it takes
                // a value or not, what follows it sets `option` only if it
                // holds it.
                if attached.contains(option) {
                    return ArgValue::Unreadable;
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
                found = value.map_or(ArgValue::Unreadable, ArgValue::Set);
            }
            break;
        }
    }
    found
}

/// What the arguments after `--` say about `maxfail`: the last of `-x`,
/// `--exitfirst` and `--maxfail N` (alone, as `--maxfail=N`, or `-x` in a
/// cluster such as `-vx`), as argparse keeps the last; `None` when none is
/// given, [`ArgValue::Unreadable`] when they can't be read (an `@file`).
/// Read as the pytest adapter
/// reads them when it lifts them out for the runner: a cluster holding an
/// option pytest doesn't know is left alone, a `--maxfail` value pytest
/// would reject sets nothing, and a negative one stops at the first
/// failure, as pytest does. A lone `--` ends the options.
pub fn passed_maxfail(passthrough: &[String]) -> ArgValue<MaxFail> {
    let ShortOptions { flags, valued } = ShortOptions::WITH_XDIST;
    let mut found = None;
    let mut args = passthrough.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if arg.starts_with('@') {
            return ArgValue::Unreadable;
        }
        if arg == "--exitfirst" {
            found = Some(MaxFail::After(1));
            continue;
        }
        if arg == "--maxfail" {
            if let Some(value) = args.peek().and_then(|value| MaxFail::parse(value)) {
                args.next();
                found = Some(value);
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("--maxfail=") {
            found = MaxFail::parse(value).or(found);
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
            } else if valued.contains(flag) {
                break;
            } else if !flags.contains(flag) {
                exitfirst = false;
                break;
            }
        }
        if exitfirst {
            found = Some(MaxFail::After(1));
        }
        // A cluster ending in an option that takes a value takes the next
        // argument as it.
        let takes = cluster.chars().position(|c| valued.contains(c));
        if takes.is_some_and(|at| at + 1 == cluster.chars().count()) {
            args.next();
        }
    }
    found.map_or(ArgValue::Unset, ArgValue::Set)
}

/// Whether argparse takes `arg` as an option's value rather than as an
/// option: it doesn't start with `-`, or is `-`, holds a space or looks
/// like a negative number.
pub fn is_value(arg: &str) -> bool {
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

/// pytest-xdist's options: those taking a value, and flags. With
/// `-p no:xdist` in workers, any of them left in `addopts` would be an
/// unknown argument.
const XDIST_VALUED: [&str; 11] = [
    "-n",
    "--numprocesses",
    "--maxprocesses",
    "--max-worker-restart",
    "--dist",
    "--tx",
    "--px",
    "--rsyncdir",
    "--rsyncignore",
    "--testrunuid",
    "--maxschedchunk",
];
const XDIST_FLAGS: [&str; 5] = [
    "-d",
    "--loadscope-reorder",
    "--no-loadscope-reorder",
    "-f",
    "--looponfail",
];

/// `addopts` without pytest-xdist's options, and the `-n` value.
pub fn split_xdist(args: &[String]) -> (Vec<String>, Option<String>) {
    let mut kept = Vec::new();
    let mut workers = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if XDIST_FLAGS.contains(&arg.as_str()) {
            continue;
        }
        if XDIST_VALUED.contains(&arg.as_str()) {
            let value = iter.next().cloned();
            if matches!(arg.as_str(), "-n" | "--numprocesses") {
                workers = value.or(workers);
            }
            continue;
        }
        if let Some((option, value)) = arg.split_once('=')
            && XDIST_VALUED.contains(&option)
        {
            if matches!(option, "-n" | "--numprocesses") {
                workers = Some(value.to_string());
            }
            continue;
        }
        if let Some(value) = arg.strip_prefix("-n")
            && !value.is_empty()
            && !arg.starts_with("--")
        {
            workers = Some(value.to_string());
            continue;
        }
        kept.push(arg.clone());
    }
    (kept, workers)
}

/// A cluster of short options (`-vxs`) with `-x` taken out: whether it held
/// `-x`, and what is left of it (`None` when nothing is). An argument
/// that isn't such a cluster, or holds an option pytest doesn't know, is
/// left alone.
fn take_exitfirst(arg: &str) -> (bool, Option<String>) {
    let Some(cluster) = arg.strip_prefix('-') else {
        return (false, Some(arg.to_string()));
    };
    if cluster.is_empty() || cluster.starts_with('-') {
        return (false, Some(arg.to_string()));
    }
    let ShortOptions { flags, valued } = ShortOptions::WITH_XDIST;
    let mut kept = String::from("-");
    let mut found = false;
    for (at, c) in cluster.char_indices() {
        if c == 'x' {
            found = true;
        } else if flags.contains(c) {
            kept.push(c);
        } else if valued.contains(c) {
            kept.push_str(&cluster[at..]);
            break;
        } else {
            return (false, Some(arg.to_string()));
        }
    }
    if !found {
        return (false, Some(arg.to_string()));
    }
    (true, (kept.len() > 1).then_some(kept))
}

/// What `-x` or `--maxfail N` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaxFail {
    /// `--maxfail 0`: no limit.
    Unlimited,
    /// Stop after this many failures.
    After(u32),
}

impl MaxFail {
    /// A `--maxfail` value, as pytest takes it; `None` for one it rejects.
    pub fn parse(value: &str) -> Option<Self> {
        let n: i64 = value.trim().parse().ok()?;
        // pytest stops once the failures reach the limit, checked after
        // each failure: a negative limit stops at the first, like 1.
        Some(match n {
            0 => Self::Unlimited,
            n if n < 0 => Self::After(1),
            n => Self::After(u32::try_from(n).unwrap_or(u32::MAX)),
        })
    }

    /// The limit, if any.
    pub fn limit(self) -> Option<u32> {
        match self {
            Self::Unlimited => None,
            Self::After(n) => Some(n),
        }
    }
}

/// `args` without `-x`, `--exitfirst` and `--maxfail`, and what the last
/// of them asks for, as argparse lets the last one win (`None` when none
/// appears). A `--maxfail` value pytest would reject is left in place, for
/// the front-end to report, as pytest does.
pub(crate) fn split_maxfail(args: &[String]) -> (Vec<String>, Option<MaxFail>) {
    let mut kept = Vec::new();
    let mut maxfail = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--exitfirst" {
            maxfail = Some(MaxFail::After(1));
            continue;
        }
        if arg == "--maxfail" {
            if let Some(found) = iter.peek().and_then(|v| MaxFail::parse(v)) {
                iter.next();
                maxfail = Some(found);
                continue;
            }
            kept.push(arg.clone());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--maxfail=") {
            if let Some(found) = MaxFail::parse(value) {
                maxfail = Some(found);
                continue;
            }
            kept.push(arg.clone());
            continue;
        }
        let (exitfirst, rest) = take_exitfirst(arg);
        if exitfirst {
            maxfail = Some(MaxFail::After(1));
        }
        // A cluster ending in an option that takes a value takes the next
        // argument too.
        let takes_next = rest.as_deref().is_some_and(|r| {
            !r.starts_with("--")
                && r.len() >= 2
                && r.chars()
                    .skip(1)
                    .position(|c| ShortOptions::WITH_XDIST.valued.contains(c))
                    .is_some_and(|at| at + 2 == r.len())
        });
        kept.extend(rest);
        if takes_next && let Some(value) = iter.next() {
            kept.push(value.clone());
        }
    }
    (kept, maxfail)
}

/// pytest's JUnit options: the report's path, and the prefix of its class
/// names.
const JUNIT_XML: [&str; 2] = ["--junitxml", "--junit-xml"];
const JUNIT_PREFIX: [&str; 2] = ["--junitprefix", "--junit-prefix"];

/// Whether argparse takes `arg` as an option's value rather than as an
/// option: it doesn't start with `-`, or it looks like a negative number
/// (pytest has no option that does), or it holds a space; as the pytest
/// adapter has always read the values of the options it lifts, a number
/// being any float Rust reads (`-1e3`, `-inf`), where [`is_value`] takes
/// argparse's plain negative numbers only.
fn is_value_or_float(arg: &str) -> bool {
    !arg.starts_with('-') || arg == "-" || arg.contains(' ') || arg.parse::<f64>().is_ok()
}

/// What the JUnit options in an argument list ask for: the last value of
/// each, `Some("")` for an empty one (which turns the report off).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Junit {
    pub xml: Option<String>,
    pub prefix: Option<String>,
}

/// `args` without pytest's JUnit options (`--junitxml`/`--junit-xml PATH`,
/// `--junitprefix`/`--junit-prefix STR`, each also with `=`), and the last
/// value of each, as argparse lets the last one win. An option missing its
/// value is left in place, for the front-end to reject, as pytest does.
pub(crate) fn split_junit(args: &[String]) -> (Vec<String>, Junit) {
    let mut kept = Vec::new();
    let mut found = Junit::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if arg.starts_with("--") => (name, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let slot = if JUNIT_XML.contains(&name) {
            &mut found.xml
        } else if JUNIT_PREFIX.contains(&name) {
            &mut found.prefix
        } else {
            kept.push(arg.clone());
            continue;
        };
        let value = if let Some(value) = inline {
            value
        } else if let Some(value) = iter.next_if(|next| is_value_or_float(next)) {
            value.clone()
        } else {
            kept.push(arg.clone());
            continue;
        };
        *slot = Some(value);
    }
    (kept, found)
}

/// pytest's doctest options that take no value, and those that take one.
const DOCTEST_FLAGS: [&str; 3] = [
    "--doctest-modules",
    "--doctest-continue-on-failure",
    "--doctest-ignore-import-errors",
];
const DOCTEST_VALUED: [&str; 2] = ["--doctest-glob", "--doctest-report"];

/// `args` without pytest's doctest options (`--doctest-modules`,
/// `--doctest-continue-on-failure`, `--doctest-ignore-import-errors`, and
/// `--doctest-glob PAT` and `--doctest-report STYLE`, each also with `=`,
/// the glob repeatable), and those options, as given: the doctest adapter
/// runs doctests and reads them itself. A valued option missing its value
/// is left in place, for the front-end to reject, as pytest does. pytest's parser takes no abbreviations, so
/// only the full names count.
pub fn split_doctest(args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept = Vec::new();
    let mut lifted = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if DOCTEST_FLAGS.contains(&arg.as_str()) {
            lifted.push(arg.clone());
            continue;
        }
        if let Some((name, _)) = arg.split_once('=')
            && DOCTEST_VALUED.contains(&name)
        {
            lifted.push(arg.clone());
            continue;
        }
        if DOCTEST_VALUED.contains(&arg.as_str())
            && let Some(value) = iter.next_if(|next| is_value_or_float(next))
        {
            lifted.push(arg.clone());
            lifted.push(value.clone());
            continue;
        }
        kept.push(arg.clone());
    }
    (kept, lifted)
}

/// One argument list with what the runner and aye take over lifted out:
/// pytest-xdist's options, `-x`/`--maxfail`, the JUnit options, and the
/// doctest options (the doctest adapter's, when it runs).
pub(crate) struct Lifted {
    pub(crate) kept: Vec<String>,
    /// `kept` with the doctest options in their places.
    pub(crate) with_doctest: Vec<String>,
    pub(crate) workers: Option<String>,
    pub(crate) maxfail: Option<MaxFail>,
    pub(crate) junit: Junit,
    pub(crate) doctest: Vec<String>,
}

impl Lifted {
    pub(crate) fn from(args: &[String]) -> Self {
        let (kept, workers) = split_xdist(args);
        let (kept, maxfail) = split_maxfail(&kept);
        let (with_doctest, junit) = split_junit(&kept);
        let (kept, doctest) = split_doctest(&with_doctest);
        Self {
            kept,
            with_doctest,
            workers,
            maxfail,
            junit,
            doctest,
        }
    }
}

/// `args` without `-o addopts=…` (`--override-ini` too, in each spelling):
/// its value is the `addopts` already at the front of the options.
pub(crate) fn without_addopts_override(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "-o" | "--override-ini")
            && iter.peek().is_some_and(|next| next.starts_with("addopts="))
        {
            iter.next();
            continue;
        }
        let inline = arg
            .strip_prefix("--override-ini=")
            .or_else(|| arg.strip_prefix("-o=").or_else(|| arg.strip_prefix("-o")));
        if inline.is_some_and(|value| value.starts_with("addopts=")) {
            continue;
        }
        kept.push(arg.clone());
    }
    kept
}

/// The values of an option in an argument list: `--opt v`, `--opt=v`, and
/// `-ov` for short options.
pub(crate) fn option_values(args: &[String], long: &str, short: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let names = std::iter::once(long).chain(short);
        for name in names {
            if arg == name {
                if let Some(value) = iter.next() {
                    out.push(value.clone());
                }
                break;
            }
            if let Some(value) = arg.strip_prefix(name) {
                if let Some(value) = value.strip_prefix('=') {
                    out.push(value.to_string());
                    break;
                }
                if name.len() == 2 && !value.is_empty() {
                    out.push(value.to_string());
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framework_args_are_read_for_maxfail_as_the_adapter_lifts_them() {
        let read = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            passed_maxfail(&args)
        };
        let after = |n| ArgValue::Set(MaxFail::After(n));
        let unset = ArgValue::Unset;
        for (args, want) in [
            (&[][..], unset),
            (&["-v", "--tb=short", "tests"], unset),
            (&["-x"], after(1)),
            (&["--exitfirst"], after(1)),
            (&["--maxfail", "3"], after(3)),
            (&["--maxfail=3"], after(3)),
            (&["--maxfail", "0"], ArgValue::Set(MaxFail::Unlimited)),
            (&["--maxfail=-2"], after(1)),
            // The last one wins, as in argparse.
            (&["-x", "--maxfail", "3"], after(3)),
            (&["--maxfail", "3", "-x"], after(1)),
            (
                &["--maxfail", "3", "--maxfail=0"],
                ArgValue::Set(MaxFail::Unlimited),
            ),
            // A value pytest rejects sets nothing (pytest reports it).
            (&["--maxfail", "many"], unset),
            (&["-x", "--maxfail=many"], after(1)),
            // In a cluster, as argparse reads it.
            (&["-vx"], after(1)),
            (&["-xvs"], after(1)),
            (&["-xk", "fast"], after(1)),
            // `-k`'s value, not `-x`.
            (&["-kx"], unset),
            (&["-k", "-x"], unset),
            (&["-rx"], unset),
            (&["-vk", "-x"], unset),
            // A cluster holding an option pytest doesn't know is left alone.
            (&["-xZ"], unset),
            (&["-n", "4", "-x"], after(1)),
            (&["@args.txt", "-x"], ArgValue::Unreadable),
            // After a lone `--`, everything is a path.
            (&["--", "-x"], unset),
            // Long options don't abbreviate in pytest.
            (&["--exit", "--maxf", "2"], unset),
        ] {
            assert_eq!(read(args), want, "{args:?}");
        }
    }

    #[test]
    fn xdist_flags_are_lifted_out() {
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_xdist(&args(&["-nauto", "-ra"])),
            (args(&["-ra"]), Some("auto".into()))
        );
        assert_eq!(
            split_xdist(&args(&["-n=3", "--maxprocesses=2", "-d", "--tx", "popen"])),
            (Vec::new(), Some("3".into()))
        );
        assert_eq!(
            split_xdist(&args(&["--numprocesses", "logical", "-q"])),
            (args(&["-q"]), Some("logical".into()))
        );
        assert_eq!(
            split_xdist(&args(&["--dist=load", "--looponfail", "-x"])),
            (args(&["-x"]), None)
        );
    }

    #[test]
    fn maxfail_is_lifted_out() {
        // Each verified with pytest 9.1.1: `-vx` stops at the first
        // failure, `-rx` doesn't, and the last of `-x`/`--maxfail` wins.
        let args = |items: &[&str]| items.iter().map(|s| (*s).to_string()).collect::<Vec<_>>();
        assert_eq!(
            split_maxfail(&args(&["-x", "-ra"])),
            (args(&["-ra"]), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["--exitfirst", "--maxfail", "3", "-q"])),
            (args(&["-q"]), Some(MaxFail::After(3)))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=2", "-x"])),
            (Vec::new(), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=0"])),
            (Vec::new(), Some(MaxFail::Unlimited))
        );
        assert_eq!(
            split_maxfail(&args(&["--maxfail=-4"])),
            (Vec::new(), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["-vxs", "-rx", "-k", "-x"])),
            (args(&["-vs", "-rx", "-k", "-x"]), Some(MaxFail::After(1)))
        );
        assert_eq!(
            split_maxfail(&args(&["-xk", "slow", "-vm", "x"])),
            (args(&["-k", "slow", "-vm", "x"]), Some(MaxFail::After(1)))
        );
        // Left for pytest to reject.
        assert_eq!(
            split_maxfail(&args(&["--maxfail", "many", "-Zx"])),
            (args(&["--maxfail", "many", "-Zx"]), None)
        );
        assert_eq!(split_maxfail(&args(&["-q"])), (args(&["-q"]), None));
    }

    #[test]
    fn framework_args_are_read_for_k_and_m_as_argparse_reads_them() {
        let read = |args: &[&str], option| {
            let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
            passed(&args, option)
        };
        let set = |value: &str| ArgValue::Set(value.into());
        for (args, want) in [
            (&[][..], ArgValue::Unset),
            (&["-v", "--tb=short", "tests"], ArgValue::Unset),
            (&["-k", "fast"], set("fast")),
            (&["-kfast"], set("fast")),
            (&["-vk", "fast"], set("fast")),
            (&["-vqkfast"], set("fast")),
            (&["-k", "-1"], set("-1")),
            (&["-k", "a", "-k", "b"], set("b")),
            // `-m`'s value isn't `-k`, and `-r`'s neither.
            (&["-mk"], ArgValue::Unset),
            (&["-rk", "-p", "k"], ArgValue::Unset),
            (&["-p", "-k", "x"], set("x")),
            // A missing value, or one argparse wouldn't take.
            (&["-k"], ArgValue::Unreadable),
            (&["-k", "--tb=short"], ArgValue::Unreadable),
            // A plugin's options: unknown only where `k` could be theirs.
            (&["-n", "4", "-k", "x"], set("x")),
            (&["-n4"], ArgValue::Unset),
            (&["-Zk", "x"], ArgValue::Unreadable),
            (&["@args.txt"], ArgValue::Unreadable),
            // Long options don't abbreviate in pytest.
            (&["--keyword", "x"], ArgValue::Unset),
            // After a lone `--`, everything is a path.
            (&["--", "-k", "x"], ArgValue::Unset),
        ] {
            assert_eq!(read(args, 'k'), want, "{args:?}");
        }
        assert_eq!(read(&["-vmdb", "-k", "x"], 'm'), set("db"));
        assert_eq!(read(&["-k", "m"], 'm'), ArgValue::Unset);
    }
}
