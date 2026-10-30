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
pub fn replaced_run(passthrough: &[String]) -> Option<String> {
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
pub enum Passed {
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
pub fn passed(passthrough: &[String], option: char) -> Passed {
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

/// pytest's short options that take no value, which argparse lets a
/// cluster combine (`-vx`); pytest-xdist adds `-d` and `-f`.
const XDIST_SHORT_FLAGS: &str = "xvqslhVdf";
/// pytest's short options that take a value: the rest of a cluster, or the
/// next argument, is it (`-rx` is `-r x`, not `-x`).
const XDIST_SHORT_VALUED: &str = "kmcpoWrn";

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
    let mut kept = String::from("-");
    let mut found = false;
    for (at, c) in cluster.char_indices() {
        if c == 'x' {
            found = true;
        } else if XDIST_SHORT_FLAGS.contains(c) {
            kept.push(c);
        } else if XDIST_SHORT_VALUED.contains(c) {
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
pub(crate) enum MaxFail {
    /// `--maxfail 0`: no limit.
    Unlimited,
    /// Stop after this many failures.
    After(u32),
}

impl MaxFail {
    /// The limit, if any.
    pub(crate) fn limit(self) -> Option<u32> {
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
    fn limit(value: &str) -> Option<MaxFail> {
        let n: i64 = value.trim().parse().ok()?;
        // pytest stops once the failures reach the limit, checked after
        // each failure: a negative limit stops at the first, like 1.
        Some(match n {
            0 => MaxFail::Unlimited,
            n if n < 0 => MaxFail::After(1),
            n => MaxFail::After(u32::try_from(n).unwrap_or(u32::MAX)),
        })
    }
    let mut kept = Vec::new();
    let mut maxfail = None;
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--exitfirst" {
            maxfail = Some(MaxFail::After(1));
            continue;
        }
        if arg == "--maxfail" {
            if let Some(found) = iter.peek().and_then(|v| limit(v)) {
                iter.next();
                maxfail = Some(found);
                continue;
            }
            kept.push(arg.clone());
            continue;
        }
        if let Some(value) = arg.strip_prefix("--maxfail=") {
            if let Some(found) = limit(value) {
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
                    .position(|c| XDIST_SHORT_VALUED.contains(c))
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
/// (pytest has no option that does), or it holds a space.
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
}
