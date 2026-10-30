//! pytest's command line as its argparse reads it: what an argument list
//! says of an option, and which of pytest's options replace the test run.

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

#[cfg(test)]
mod tests {
    use super::*;

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
