//! pytest 9.1.1's built-in options (`_pytest/*.py`'s `addoption`), with
//! their dests and defaults, and pytest's argparse over them: the native
//! context's `options` and `unknown_options`.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// What an option does with what it is given.
#[derive(Debug, Clone, Copy)]
enum Action {
    /// `store_true`.
    True,
    /// `store_false`.
    False,
    /// `store_const`: the value, as JSON text.
    Const(&'static str),
    /// `count`, adding this.
    Count(i64),
    /// `store` of a string.
    Text,
    /// `store` of an int.
    Int,
    /// `store` of a float.
    Float,
    /// `append` of a string.
    Append,
    /// `nargs="?"`: the value when attached (`--debug=FILE`), else the
    /// constant.
    Optional(&'static str),
}

/// One of pytest's built-in options.
struct Opt {
    names: &'static [&'static str],
    dest: &'static str,
    action: Action,
    /// Its default, as JSON text.
    default: &'static str,
}

const fn opt(
    names: &'static [&'static str],
    dest: &'static str,
    action: Action,
    default: &'static str,
) -> Opt {
    Opt {
        names,
        dest,
        action,
        default,
    }
}

/// pytest 9.1.1's built-in options (`_pytest/*.py`'s `addoption`), with
/// their dests and defaults. `--strict…` override ini values too
/// ([`aye_pytest::getini::apply_overrides`]).
const OPTIONS: &[Opt] = &[
    opt(&["-k"], "keyword", Action::Text, r#""""#),
    opt(&["-m"], "markexpr", Action::Text, r#""""#),
    opt(&["--markers"], "markers", Action::True, "false"),
    opt(&["-x", "--exitfirst"], "maxfail", Action::Const("1"), "0"),
    opt(&["--maxfail"], "maxfail", Action::Int, "0"),
    opt(&["--strict-config"], "strict_config", Action::True, "null"),
    opt(
        &["--strict-markers"],
        "strict_markers",
        Action::True,
        "null",
    ),
    opt(&["--strict"], "strict", Action::True, "null"),
    opt(
        &["--fixtures", "--funcargs"],
        "showfixtures",
        Action::True,
        "false",
    ),
    opt(
        &["--fixtures-per-test"],
        "show_fixtures_per_test",
        Action::True,
        "false",
    ),
    opt(&["--pdb"], "usepdb", Action::True, "false"),
    opt(&["--pdbcls"], "usepdb_cls", Action::Text, "null"),
    opt(&["--trace"], "trace", Action::True, "false"),
    opt(&["--capture"], "capture", Action::Text, r#""fd""#),
    opt(&["-s"], "capture", Action::Const(r#""no""#), r#""fd""#),
    opt(&["--runxfail"], "runxfail", Action::True, "false"),
    opt(&["--lf", "--last-failed"], "lf", Action::True, "false"),
    opt(
        &["--ff", "--failed-first"],
        "failedfirst",
        Action::True,
        "false",
    ),
    opt(&["--nf", "--new-first"], "newfirst", Action::True, "false"),
    opt(
        &["--cache-show"],
        "cacheshow",
        Action::Optional(r#""*""#),
        "null",
    ),
    opt(&["--cache-clear"], "cacheclear", Action::True, "false"),
    opt(
        &["--lfnf", "--last-failed-no-failures"],
        "last_failed_no_failures",
        Action::Text,
        r#""all""#,
    ),
    opt(&["--sw", "--stepwise"], "stepwise", Action::True, "false"),
    opt(
        &["--sw-skip", "--stepwise-skip"],
        "stepwise_skip",
        Action::True,
        "false",
    ),
    opt(
        &["--sw-reset", "--stepwise-reset"],
        "stepwise_reset",
        Action::True,
        "false",
    ),
    opt(&["--durations"], "durations", Action::Int, "null"),
    opt(&["--durations-min"], "durations_min", Action::Float, "null"),
    opt(&["-v", "--verbose"], "verbose", Action::Count(1), "0"),
    opt(&["-q", "--quiet"], "verbose", Action::Count(-1), "0"),
    opt(&["--verbosity"], "verbose", Action::Int, "0"),
    opt(&["--no-header"], "no_header", Action::True, "false"),
    opt(&["--no-summary"], "no_summary", Action::True, "false"),
    opt(
        &["--no-fold-skipped"],
        "fold_skipped",
        Action::False,
        "true",
    ),
    opt(
        &["--force-short-summary"],
        "force_short_summary",
        Action::True,
        "false",
    ),
    opt(
        &["-r", "--report-chars"],
        "reportchars",
        Action::Text,
        r#""fE""#,
    ),
    opt(
        &["--disable-warnings", "--disable-pytest-warnings"],
        "disable_warnings",
        Action::True,
        "false",
    ),
    opt(&["-l", "--showlocals"], "showlocals", Action::True, "false"),
    opt(&["--no-showlocals"], "showlocals", Action::False, "false"),
    opt(&["--tb"], "tbstyle", Action::Text, r#""auto""#),
    opt(&["--xfail-tb"], "xfail_tb", Action::True, "false"),
    opt(&["--show-capture"], "showcapture", Action::Text, r#""all""#),
    opt(
        &["--fulltrace", "--full-trace"],
        "fulltrace",
        Action::True,
        "false",
    ),
    opt(&["--color"], "color", Action::Text, r#""auto""#),
    opt(
        &["--code-highlight"],
        "code_highlight",
        Action::Text,
        r#""yes""#,
    ),
    opt(&["--pastebin"], "pastebin", Action::Text, "null"),
    opt(
        &["--junitxml", "--junit-xml"],
        "xmlpath",
        Action::Text,
        "null",
    ),
    opt(
        &["--junitprefix", "--junit-prefix"],
        "junitprefix",
        Action::Text,
        "null",
    ),
    opt(
        &["-W", "--pythonwarnings"],
        "pythonwarnings",
        Action::Append,
        "null",
    ),
    opt(&["--max-warnings"], "max_warnings", Action::Int, "null"),
    opt(
        &["--collectonly", "--collect-only", "--co"],
        "collectonly",
        Action::True,
        "false",
    ),
    opt(&["--pyargs"], "pyargs", Action::True, "false"),
    opt(&["--ignore"], "ignore", Action::Append, "null"),
    opt(&["--ignore-glob"], "ignore_glob", Action::Append, "null"),
    opt(&["--deselect"], "deselect", Action::Append, "null"),
    opt(&["--confcutdir"], "confcutdir", Action::Text, "null"),
    opt(&["--noconftest"], "noconftest", Action::True, "false"),
    opt(
        &["--keepduplicates", "--keep-duplicates"],
        "keepduplicates",
        Action::True,
        "false",
    ),
    opt(
        &["--collect-in-virtualenv"],
        "collect_in_virtualenv",
        Action::True,
        "false",
    ),
    opt(
        &["--continue-on-collection-errors"],
        "continue_on_collection_errors",
        Action::True,
        "false",
    ),
    opt(
        &["--import-mode"],
        "importmode",
        Action::Text,
        r#""prepend""#,
    ),
    opt(
        &["--doctest-modules"],
        "doctestmodules",
        Action::True,
        "false",
    ),
    opt(
        &["--doctest-report"],
        "doctestreport",
        Action::Text,
        r#""udiff""#,
    ),
    opt(&["--doctest-glob"], "doctestglob", Action::Append, "[]"),
    opt(
        &["--doctest-ignore-import-errors"],
        "doctest_ignore_import_errors",
        Action::True,
        "false",
    ),
    opt(
        &["--doctest-continue-on-failure"],
        "doctest_continue_on_failure",
        Action::True,
        "false",
    ),
    opt(
        &["-c", "--config-file"],
        "inifilename",
        Action::Text,
        "null",
    ),
    opt(&["--rootdir"], "rootdir", Action::Text, "null"),
    opt(&["--basetemp"], "basetemp", Action::Text, "null"),
    opt(&["-V", "--version"], "version", Action::Count(1), "0"),
    opt(&["-h", "--help"], "help", Action::True, "false"),
    opt(&["-p"], "plugins", Action::Append, "[]"),
    opt(
        &["--disable-plugin-autoload"],
        "disable_plugin_autoload",
        Action::True,
        "false",
    ),
    opt(
        &["--traceconfig", "--trace-config"],
        "traceconfig",
        Action::True,
        "false",
    ),
    opt(
        &["--debug"],
        "debug",
        Action::Optional(r#""pytestdebug.log""#),
        "null",
    ),
    opt(
        &["-o", "--override-ini"],
        "override_ini",
        Action::Append,
        "null",
    ),
    opt(&["--assert"], "assertmode", Action::Text, r#""rewrite""#),
    opt(
        &["--setuponly", "--setup-only"],
        "setuponly",
        Action::True,
        "false",
    ),
    opt(
        &["--setupshow", "--setup-show"],
        "setupshow",
        Action::True,
        "false",
    ),
    opt(
        &["--setupplan", "--setup-plan"],
        "setupplan",
        Action::True,
        "false",
    ),
    opt(&["--log-level"], "log_level", Action::Text, "null"),
    opt(&["--log-format"], "log_format", Action::Text, "null"),
    opt(
        &["--log-date-format"],
        "log_date_format",
        Action::Text,
        "null",
    ),
    opt(&["--log-cli-level"], "log_cli_level", Action::Text, "null"),
    opt(
        &["--log-cli-format"],
        "log_cli_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-cli-date-format"],
        "log_cli_date_format",
        Action::Text,
        "null",
    ),
    opt(&["--log-file"], "log_file", Action::Text, "null"),
    opt(&["--log-file-mode"], "log_file_mode", Action::Text, "null"),
    opt(
        &["--log-file-level"],
        "log_file_level",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-file-format"],
        "log_file_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-file-date-format"],
        "log_file_date_format",
        Action::Text,
        "null",
    ),
    opt(
        &["--log-auto-indent"],
        "log_auto_indent",
        Action::Text,
        "null",
    ),
    opt(&["--log-disable"], "logger_disable", Action::Append, "[]"),
];

/// pytest's arguments, parsed.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    /// Every built-in option's dest, with its value (its default when not
    /// given), and `file_or_dir`.
    pub(crate) values: BTreeMap<String, Value>,
    /// The options no built-in one matches, as given.
    pub(crate) unknown: Vec<String>,
}

fn find_option(name: &str) -> Option<&'static Opt> {
    OPTIONS.iter().find(|o| o.names.contains(&name))
}

/// Whether `action` takes a value.
const fn takes_value(action: Action) -> bool {
    matches!(
        action,
        Action::Text | Action::Int | Action::Float | Action::Append
    )
}

/// argparse's test for a negative number (`-1`, `-1.5`, `-.5`), which is
/// a value, not an option.
fn is_negative_number(arg: &str) -> bool {
    arg.strip_prefix('-').is_some_and(|rest| {
        !rest.is_empty()
            && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
            && rest.chars().filter(|c| *c == '.').count() <= 1
            && rest != "."
    })
}

/// Apply one option with its value (`None` when it takes none).
fn apply(values: &mut BTreeMap<String, Value>, option: &Opt, value: Option<&str>) {
    let dest = option.dest.to_string();
    let parsed = match option.action {
        Action::True => json!(true),
        Action::False => json!(false),
        Action::Const(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        Action::Count(step) => {
            let now = values.get(&dest).and_then(Value::as_i64).unwrap_or(0);
            json!(now + step)
        }
        Action::Text => json!(value.unwrap_or_default()),
        Action::Int => value
            .and_then(|v| v.trim().parse::<i64>().ok())
            .map_or(Value::Null, |n| json!(n)),
        Action::Float => value
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map_or(Value::Null, |n| json!(n)),
        Action::Append => {
            let mut list = values
                .get(&dest)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            list.push(json!(value.unwrap_or_default()));
            Value::Array(list)
        }
        Action::Optional(constant) => value.map_or_else(
            || serde_json::from_str(constant).unwrap_or(Value::Null),
            |v| json!(v),
        ),
    };
    values.insert(dest, parsed);
}

/// `args` parsed as pytest's argparse parses its built-in options: long
/// options whole (`--name value`, `--name=value`; pytest allows no
/// abbreviation), short ones alone or in clusters (`-vv`, `-ra`, `-kexpr`,
/// `-vk expr`), a lone `--` ending the options. What it doesn't know is
/// left in `unknown`, and doesn't consume the next argument.
pub(crate) fn parse_options(args: &[String]) -> Parsed {
    let mut parsed = Parsed::default();
    for option in OPTIONS {
        parsed
            .values
            .entry(option.dest.to_string())
            .or_insert_with(|| serde_json::from_str(option.default).unwrap_or(Value::Null));
    }
    let mut positional: Vec<Value> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--" {
            positional.extend(iter.by_ref().map(|a| json!(a)));
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, attached) = match long.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            let Some(option) = find_option(&name) else {
                parsed.unknown.push(arg.clone());
                continue;
            };
            let value = if takes_value(option.action) {
                attached.or_else(|| iter.next().cloned())
            } else {
                attached.filter(|_| matches!(option.action, Action::Optional(_)))
            };
            apply(&mut parsed.values, option, value.as_deref());
            continue;
        }
        if arg.len() > 1 && arg.starts_with('-') && !is_negative_number(arg) {
            let cluster = &arg[1..];
            for (at, flag) in cluster.char_indices() {
                let Some(option) = find_option(&format!("-{flag}")) else {
                    parsed.unknown.push(arg.clone());
                    break;
                };
                if takes_value(option.action) {
                    let rest = &cluster[at + flag.len_utf8()..];
                    let value = if rest.is_empty() {
                        iter.next().cloned()
                    } else {
                        Some(rest.to_string())
                    };
                    apply(&mut parsed.values, option, value.as_deref());
                    break;
                }
                apply(&mut parsed.values, option, None);
            }
            continue;
        }
        positional.push(json!(arg));
    }
    parsed
        .values
        .insert("file_or_dir".into(), Value::Array(positional));
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn options_parse_as_pytests_argparse_parses_them() {
        let parsed = parse_options(&strings(&[
            "-vv",
            "-q",
            "-ra",
            "-kslow and not fast",
            "-p",
            "no:cacheprovider",
            "-pxdist",
            "--runxfail",
            "-W",
            "error",
            "-Wignore::DeprecationWarning",
            "--basetemp=/tmp/b",
            "-s",
            "--tb",
            "short",
            "--durations=5",
            "--runslow",
            "--debug",
            "-x",
            "tests/a.py",
            "--",
            "-notanoption",
        ]));
        let v = &parsed.values;
        assert_eq!(v["verbose"], json!(1));
        assert_eq!(v["reportchars"], json!("a"));
        assert_eq!(v["keyword"], json!("slow and not fast"));
        assert_eq!(v["markexpr"], json!(""));
        assert_eq!(v["plugins"], json!(["no:cacheprovider", "xdist"]));
        assert_eq!(v["runxfail"], json!(true));
        assert_eq!(
            v["pythonwarnings"],
            json!(["error", "ignore::DeprecationWarning"])
        );
        assert_eq!(v["basetemp"], json!("/tmp/b"));
        assert_eq!(v["capture"], json!("no"));
        assert_eq!(v["tbstyle"], json!("short"));
        assert_eq!(v["durations"], json!(5));
        assert_eq!(v["debug"], json!("pytestdebug.log"));
        assert_eq!(v["maxfail"], json!(1));
        assert_eq!(v["file_or_dir"], json!(["tests/a.py", "-notanoption"]));
        assert_eq!(parsed.unknown, ["--runslow"]);
        // Defaults for what isn't given.
        assert_eq!(v["importmode"], json!("prepend"));
        assert_eq!(v["assertmode"], json!("rewrite"));
        assert_eq!(v["ignore"], Value::Null);
        assert_eq!(v["doctestglob"], json!([]));
        // An unknown short option ends its cluster; a negative number is a
        // value.
        let parsed = parse_options(&strings(&["-vZq", "-1"]));
        assert_eq!(parsed.values["verbose"], json!(1));
        assert_eq!(parsed.unknown, ["-vZq"]);
        assert_eq!(parsed.values["file_or_dir"], json!(["-1"]));
    }
}
