use std::path::{Path, PathBuf};

use aye_adapter::{Adapter, FrameworkDefaults, Project};
use aye_config::{AyeSettings, OutputFormat, Sourced, ValueSource, WorkerCount};
use aye_events::AdapterName;
use aye_protocol::MaxFail;
use aye_provision::{PythonRequest, PythonSource, RequestSource};
use aye_render::{JunitDuration, JunitFamily, JunitLogging, JunitOptions};
use aye_select::Selection;
use aye_static::ProcessEnv;
use clap::Parser as _;

use super::*;
use crate::cli::ShowArgs;

/// The static selection the command line `argv` asks for, with the
/// adapters' `defaults`.
fn select(
    argv: &[&str],
    defaults: &[(AdapterName, FrameworkDefaults)],
) -> Result<Selection, aye_select::SelectError> {
    let given = args(argv);
    selection::resolve(
        given.keyword.as_deref(),
        given.tags.as_deref(),
        &given.passthrough,
        [],
        defaults,
    )
}

/// The `maxfail` the command line `argv` asks for, with the adapters'
/// `defaults`.
fn maxfail_of(argv: &[&str], defaults: &[(AdapterName, FrameworkDefaults)]) -> Option<u32> {
    let given = args(argv);
    maxfail::resolve(&given.passthrough, given.exitfirst, given.maxfail, defaults)
}

/// What aye 0.0.1's binary read from an adapter's context, each key as it
/// read it: its text (a worker count it couldn't read was one), a limit
/// above 0, the strings of `deselect`, and `junit_log_passing_tests` as a
/// bool, or as text pytest's `_strtobool` reads, but for spaces. The
/// adapters' contexts still carry these keys; their [`FrameworkDefaults`]
/// must say the same.
fn hints(context: &serde_json::Value) -> FrameworkDefaults {
    let text = |key: &str| {
        context
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    FrameworkDefaults {
        workers: text("xdist_workers").map(|n| n.parse().unwrap_or(WorkerCount::Count(1))),
        maxfail: context
            .get("maxfail")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .and_then(MaxFail::new),
        keyword: text("keyword"),
        tags: text("tags"),
        deselect: context
            .get("deselect")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(aye_events::IdPrefix::from)
            .collect(),
        junit: aye_adapter::JunitDefaults {
            path: text("junitxml").map(PathBuf::from),
            prefix: text("junit_prefix"),
            suite_name: text("junit_suite_name"),
            logging: text("junit_logging"),
            log_passing_tests: match context.get("junit_log_passing_tests") {
                Some(serde_json::Value::Bool(value)) => Some(*value),
                Some(serde_json::Value::String(value)) => {
                    match value.to_ascii_lowercase().as_str() {
                        "y" | "yes" | "t" | "true" | "on" | "1" => Some(true),
                        "n" | "no" | "f" | "false" | "off" | "0" => Some(false),
                        _ => None,
                    }
                }
                _ => None,
            },
            duration_report: text("junit_duration_report"),
            family: text("junit_family"),
        },
    }
}

/// Adapters whose frameworks say what these contexts' keys say ([`hints`]),
/// named `pytest`, `doctest` and `unittest` in turn.
fn from_contexts(contexts: &[serde_json::Value]) -> Vec<(AdapterName, FrameworkDefaults)> {
    contexts
        .iter()
        .zip(["pytest", "doctest", "unittest"])
        .map(|(context, name)| (name.into(), hints(context)))
        .collect()
}

fn args(argv: &[&str]) -> RunArgs {
    use clap::Parser as _;
    RunArgs::try_parse_from(std::iter::once("aye").chain(argv.iter().copied())).unwrap()
}

/// The `-k` and `-m` texts static selection applies to the first
/// adapter's (pytest's) cases.
fn expressions(argv: &[&str], contexts: &[serde_json::Value]) -> (Option<String>, Option<String>) {
    expressions_for("pytest", argv, contexts)
}

/// The `-k` and `-m` texts static selection applies to `adapter`'s cases.
fn expressions_for(
    adapter: &str,
    argv: &[&str],
    contexts: &[serde_json::Value],
) -> (Option<String>, Option<String>) {
    let selection = select(argv, &from_contexts(contexts)).unwrap();
    let text = |e: Option<&aye_select::Expression>| e.map(|e| e.as_str().to_string());
    (
        text(selection.keyword_for(&AdapterName::from(adapter.to_owned()))),
        text(selection.tags_for(&AdapterName::from(adapter.to_owned()))),
    )
}

fn pair(keyword: Option<&str>, tags: Option<&str>) -> (Option<String>, Option<String>) {
    (keyword.map(str::to_string), tags.map(str::to_string))
}

#[test]
fn the_frameworks_own_expressions_apply_when_the_command_line_gives_none() {
    let addopts = serde_json::json!({"keyword": "not slow", "tags": "db", "maxfail": null});
    let both = std::slice::from_ref(&addopts);
    assert_eq!(expressions(&[], both), pair(Some("not slow"), Some("db")));
    // Each one separately: the command line's wins where it is given.
    assert_eq!(
        expressions(&["-k", "fast"], both),
        pair(Some("fast"), Some("db"))
    );
    assert_eq!(
        expressions(&["-m", "net"], both),
        pair(Some("not slow"), Some("net"))
    );
    // An empty expression turns the framework's off, as in pytest.
    assert_eq!(expressions(&["-k", ""], both), pair(None, Some("db")));
    // The arguments after `--` come last in pytest's arguments: theirs wins.
    assert_eq!(
        expressions(&["-k", "fast", "--", "-k", "quick", "-vmnet"], both),
        pair(Some("quick"), Some("net"))
    );
    // When they can't be read, the command line's, else the framework's
    // own: nothing re-applies an expression in a batch.
    assert_eq!(
        expressions(&["-k", "fast", "--", "@more-args"], both),
        pair(Some("fast"), Some("db"))
    );
    assert_eq!(
        expressions(&["--", "-Zk", "x"], both),
        pair(Some("not slow"), Some("db"))
    );
    // Nothing in the context, or something that isn't text.
    assert_eq!(
        expressions(&[], &[serde_json::Value::Null]),
        pair(None, None)
    );
    assert_eq!(
        expressions(&[], &[serde_json::json!({"keyword": 3, "tags": null})]),
        pair(None, None)
    );
    // Each adapter's own applies to its own cases: doctest's context, which
    // names none, leaves its cases alone.
    let other = serde_json::json!({"keyword": "not slow", "tags": "net"});
    let two = [addopts.clone(), other];
    assert_eq!(expressions(&[], &two), pair(Some("not slow"), Some("db")));
    assert_eq!(
        expressions_for("doctest", &[], &two),
        pair(Some("not slow"), Some("net"))
    );
    let silent = [addopts.clone(), serde_json::json!({"rootdir": "/x"})];
    assert_eq!(expressions_for("doctest", &[], &silent), pair(None, None));
    assert_eq!(
        expressions(&[], &silent),
        pair(Some("not slow"), Some("db"))
    );
    // The command line's apply to every adapter's cases.
    assert_eq!(
        expressions_for("doctest", &["-k", "fast"], &silent),
        pair(Some("fast"), None)
    );
}

#[test]
fn only_the_command_lines_malformed_expressions_are_usage_errors() {
    let malformed = serde_json::json!({"keyword": "not (", "tags": "a and"});
    // The framework's own: left for it to report, and not applied.
    assert_eq!(
        expressions(&[], std::slice::from_ref(&malformed)),
        pair(None, None)
    );
    assert_eq!(expressions(&["--", "-k", "or"], &[]), pair(None, None));
    // The command line's: an error, even where `--` overrides it.
    for argv in [
        &["-k", "not ("][..],
        &["-m", "a and"],
        &["-k", "not (", "--", "-k", "x"],
    ] {
        assert!(select(argv, &from_contexts(&[])).is_err(), "{argv:?}");
    }
}

#[test]
fn the_junit_report_can_come_from_the_framework() {
    let cwd = Path::new("/work/sub");
    let no_env = ProcessEnv::new();
    let context = |path: &str| from_contexts(&[serde_json::json!({ "junitxml": path })]);
    assert_eq!(
        junit::path(&no_env, cwd, None, &context("reports/out.xml")),
        Some(PathBuf::from("/work/sub/reports/out.xml"))
    );
    // The command line's wins.
    assert_eq!(
        junit::path(
            &no_env,
            cwd,
            Some(Path::new("mine.xml")),
            &context("reports/out.xml")
        ),
        Some(PathBuf::from("/work/sub/mine.xml"))
    );
    assert_eq!(
        junit::path(&no_env, cwd, None, &context("/abs/out.xml")),
        Some(PathBuf::from("/abs/out.xml"))
    );
    // pytest writes no report for an empty path.
    assert_eq!(junit::path(&no_env, cwd, None, &context("")), None);
    assert_eq!(
        junit::path(
            &no_env,
            cwd,
            None,
            &from_contexts(&[serde_json::Value::Null])
        ),
        None
    );
    assert_eq!(junit::path(&no_env, cwd, None, &[]), None);
    // The command line's is read as pytest reads it, over the environment
    // aye started in; the framework's is the path the adapter worked out.
    let home = ProcessEnv::from_vars([("HOME", "/home/me")]);
    assert_eq!(
        junit::path(&home, cwd, Some(Path::new("~/r.xml")), &[]),
        Some(PathBuf::from("/home/me/r.xml"))
    );
    assert_eq!(
        junit::path(&home, cwd, Some(Path::new("$HOME/${AYE_UNSET}.xml")), &[]),
        Some(PathBuf::from("/home/me/${AYE_UNSET}.xml"))
    );
    assert_eq!(
        junit::path(&no_env, cwd, Some(Path::new("")), &context("x.xml")),
        None
    );
    assert_eq!(
        junit::path(&no_env, cwd, None, &context("$HOME/r.xml")),
        Some(PathBuf::from("/work/sub/$HOME/r.xml"))
    );
    // pytest's `--junitprefix` comes with it.
    assert_eq!(
        junit::prefix(&from_contexts(&[
            serde_json::Value::Null,
            serde_json::json!({ "junit_prefix": "ci" })
        ])),
        "ci"
    );
    assert_eq!(junit::prefix(&context("x.xml")), "");
}

/// `--python`, then `$AYE_PYTHON`, then `[tool.aye] python`: the first
/// given is the request, the command line's and the environment's from the
/// current directory, the configuration's from the root.
#[test]
fn the_interpreter_asked_for_is_the_first_given() {
    let (root, cwd) = (Path::new("/root"), Path::new("/root/sub"));
    let tool_aye = |python: Option<&str>| aye_config::AyeSettings {
        python: python.map(|python| aye_config::Sourced::default_value(PathBuf::from(python))),
        ..Default::default()
    };
    let asked = |flag: Option<&str>, env: Option<&str>, config: Option<&str>| {
        python::python_request(
            flag.map(Path::new),
            env.map(std::ffi::OsStr::new),
            &tool_aye(config),
            root,
            cwd,
        )
    };
    let file = |path: &str, source| Some((PythonRequest::File(PathBuf::from(path)), source));
    assert_eq!(
        asked(Some("flag/python"), Some("env/python"), Some("cfg/python")),
        file("/root/sub/flag/python", RequestSource::Flag)
    );
    assert_eq!(
        asked(None, Some("env/python"), Some("cfg/python")),
        file("/root/sub/env/python", RequestSource::Env)
    );
    assert_eq!(
        asked(None, None, Some("cfg/python")),
        file("/root/cfg/python", RequestSource::Config)
    );
    assert_eq!(
        asked(None, None, Some("python3")),
        Some((
            PythonRequest::ExecutableName("python3".to_string()),
            RequestSource::Config
        ))
    );
    assert_eq!(asked(None, None, None), None);
}

/// `--output`, else `[tool.aye] output`, else text, and where it came
/// from, as `aye config` shows it.
#[test]
fn the_output_is_the_command_lines_else_the_configurations_else_text() {
    let tool_aye = |output: Option<OutputFormat>| aye_config::AyeSettings {
        output: output.map(|output| {
            Sourced::new(
                output,
                ValueSource::file("/repo/pyproject.toml", "[tool.aye]"),
            )
        }),
        ..Default::default()
    };
    let shown = |argv: &[&str], tool_aye: &AyeSettings| {
        let Sourced { value, source } = output::format(args(argv).output, tool_aye);
        (value, source.display_for("output").to_string())
    };
    assert_eq!(
        shown(&[], &tool_aye(None)),
        (OutputFormat::Text, "default".to_string())
    );
    assert_eq!(
        shown(&[], &tool_aye(Some(OutputFormat::Json))),
        (OutputFormat::Json, "[tool.aye] output".to_string())
    );
    assert_eq!(
        shown(&["--output", "text"], &tool_aye(Some(OutputFormat::Json))),
        (OutputFormat::Text, "--output".to_string())
    );
    assert_eq!(
        shown(&["--output", "JSON", "-vv"], &tool_aye(None)),
        (OutputFormat::Json, "--output".to_string())
    );
}

/// Where the interpreter came from, as `aye config` shows each source, as
/// 0.0.1 spelt it: what asked for it, or the rule discovery followed.
#[test]
fn the_interpreters_source_is_shown_by_its_name() {
    use aye_provision::VenvDir;
    let config = ValueSource::file("/repo/pyproject.toml", "[tool.aye]");
    let shown = [
        PythonSource::Requested(RequestSource::Flag),
        PythonSource::Requested(RequestSource::Env),
        PythonSource::Requested(RequestSource::Config),
        PythonSource::VirtualEnv,
        PythonSource::ProjectVenv(VenvDir::Cwd),
        PythonSource::ProjectVenv(VenvDir::Root),
        PythonSource::BesideAye,
        PythonSource::SearchPath,
    ]
    .map(|source| {
        python::interpreter_source(source, Some(&config))
            .display_for("python")
            .to_string()
    });
    assert_eq!(
        shown,
        [
            "--python",
            "$AYE_PYTHON",
            "[tool.aye] python",
            "$VIRTUAL_ENV",
            ".venv",
            ".venv",
            "beside aye",
            "PATH",
        ]
    );
}

/// C24: the JUnit report follows the framework's settings that the
/// adapters' contexts give, in pytest's spellings, each from the first
/// context that gives it; aye's own otherwise.
#[test]
fn the_junit_report_follows_the_frameworks_settings() {
    assert_eq!(junit::options(&[]), JunitOptions::default());
    assert_eq!(
        junit::options(&from_contexts(&[serde_json::json!({ "rootdir": "/repo" })])),
        JunitOptions::default()
    );
    let pytest = serde_json::json!({
        "junit_suite_name": "mysuite",
        "junit_logging": "out-err",
        "junit_log_passing_tests": false,
        "junit_duration_report": "call",
        "junit_family": "legacy",
    });
    assert_eq!(
        junit::options(&from_contexts(&[serde_json::json!({}), pytest])),
        JunitOptions {
            suite_name: "mysuite".into(),
            logging: JunitLogging::OutErr,
            log_passing_tests: false,
            duration: JunitDuration::Body,
            family: JunitFamily::Xunit1,
        }
    );
    // pytest's defaults, as the context gives them.
    let defaults = serde_json::json!({
        "junit_suite_name": "pytest",
        "junit_logging": "no",
        "junit_log_passing_tests": true,
        "junit_duration_report": "total",
        "junit_family": "xunit2",
    });
    assert_eq!(
        junit::options(&from_contexts(&[defaults])),
        JunitOptions {
            suite_name: "pytest".into(),
            logging: JunitLogging::No,
            log_passing_tests: true,
            duration: JunitDuration::Total,
            family: JunitFamily::Xunit2,
        }
    );
    // A bool written as an ini file writes it; one pytest can't read keeps
    // the default.
    for (value, expected) in [
        ("False", false),
        ("off", false),
        ("YES", true),
        ("maybe", true),
    ] {
        let context = serde_json::json!({ "junit_log_passing_tests": value });
        assert_eq!(
            junit::options(&from_contexts(&[context])).log_passing_tests,
            expected,
            "{value}"
        );
    }
}

/// Each adapter's [`FrameworkDefaults`] say what its context's keys say, as
/// aye 0.0.1's binary read them ([`hints`]): with pytest's options and ini
/// setting every one, a `-n` aye can't read, `-n logical`, `--maxfail=0`,
/// and `junit_log_passing_tests` as text pytest reads and doesn't.
#[test]
fn the_adapters_defaults_are_what_their_contexts_say() {
    let names = [
        aye_adapter_pytest::NAME,
        aye_adapter_doctest::NAME,
        aye_adapter_unittest::NAME,
    ];
    let adapters: [Box<dyn Adapter>; 3] = [
        Box::new(aye_adapter_pytest::Pytest::default()),
        Box::new(aye_adapter_doctest::Doctest),
        Box::new(aye_adapter_unittest::Unittest),
    ];
    let projects: [(&str, &str); 6] = [
        ("pyproject.toml", "[project]\nname = \"x\"\n"),
        (
            "pytest.ini",
            "[pytest]\naddopts = -n 3 --maxfail=2 --junitxml=out.xml --junitprefix=ci \
             -k \"not slow\" -m db --deselect t.py::x --doctest-modules\n\
             junit_family = xunit1\njunit_log_passing_tests = no\njunit_suite_name = s\n\
             junit_logging = all\njunit_duration_report = call\n",
        ),
        (
            "pytest.ini",
            "[pytest]\naddopts = -n many -x --doctest-modules\n",
        ),
        (
            "pytest.ini",
            "[pytest]\naddopts = -n logical --maxfail=0 --doctest-modules\n\
             junit_log_passing_tests = maybe\n",
        ),
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\naddopts = \"-n auto --doctest-modules\"\n\
             junit_log_passing_tests = \" false\"\n",
        ),
        (
            "pyproject.toml",
            "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n\
             junit_log_passing_tests = false\n",
        ),
    ];
    let mut seen = Vec::new();
    for (at, (name, body)) in projects.into_iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join(name), body).unwrap();
        let project = Project::new(root, root);
        for adapter in &adapters {
            let Some(recognizer) = adapter.prepare_among(&project, &names).unwrap() else {
                continue;
            };
            let defaults = recognizer.defaults();
            assert_eq!(
                defaults,
                hints(recognizer.context().as_value()),
                "{} with {body}",
                adapter.name()
            );
            seen.push((at, adapter.name(), defaults));
        }
    }
    let of = |name: AdapterName, at: usize| {
        &seen
            .iter()
            .find(|(seen_at, seen_name, _)| *seen_at == at && *seen_name == name)
            .unwrap()
            .2
    };
    let pytest = |at: usize| of(aye_adapter_pytest::NAME, at);
    assert_eq!(pytest(1).workers, Some(WorkerCount::Count(3)));
    assert_eq!(pytest(1).maxfail, MaxFail::new(2));
    assert_eq!(pytest(1).keyword.as_deref(), Some("not slow"));
    assert_eq!(pytest(1).junit.log_passing_tests, Some(false));
    assert_eq!(pytest(2).workers, Some(WorkerCount::Count(1)));
    assert_eq!(pytest(3).workers, Some(WorkerCount::Auto));
    assert_eq!(pytest(3).maxfail, None);
    let doctest = |at: usize| of(aye_adapter_doctest::NAME, at);
    assert_eq!(doctest(1).deselect, [aye_events::IdPrefix::from("t.py::x")]);
    assert_eq!(doctest(1).junit.log_passing_tests, Some(false));
    assert_eq!(doctest(1).junit.family.as_deref(), Some("xunit1"));
    assert_eq!(doctest(2).maxfail, MaxFail::new(1));
    assert_eq!(doctest(3).junit.log_passing_tests, None);
    assert_eq!(doctest(4).junit.log_passing_tests, None);
    assert_eq!(
        *of(aye_adapter_unittest::NAME, 0),
        FrameworkDefaults::default()
    );
}

/// C26: a rerun repeats aye's own options that shape the run, as given:
/// `-c FILE` (or `--config-file`), not `--python` or the output's.
#[test]
fn reruns_repeat_ayes_options_that_shape_the_run() {
    assert_eq!(
        rerun_args(args(&[]).config_file.as_deref()),
        Vec::<String>::new()
    );
    assert_eq!(
        rerun_args(args(&["-c", "alt.ini"]).config_file.as_deref()),
        ["-c", "alt.ini"]
    );
    assert_eq!(
        rerun_args(
            args(&[
                "--config-file",
                "../cfg/alt.ini",
                "--python",
                "/usr/bin/python3",
                "-v",
                "--output",
                "json",
                "--junitxml",
                "out.xml",
                "-x",
            ])
            .config_file
            .as_deref()
        ),
        ["-c", "../cfg/alt.ini"]
    );
}

/// `--native`, the prototype's switch, does nothing now that aye's engine
/// runs every test: a rerun doesn't repeat it.
#[test]
fn reruns_leave_native_out() {
    assert_eq!(
        rerun_args(args(&["--native"]).config_file.as_deref()),
        Vec::<String>::new()
    );
    assert_eq!(
        rerun_args(args(&["-c", "alt.ini", "--native"]).config_file.as_deref()),
        ["-c", "alt.ini"]
    );
}

#[test]
fn maxfail_after_the_double_dash_wins_over_the_command_lines() {
    let none = from_contexts(&[serde_json::json!({ "maxfail": null })]);
    // As pytest's `--maxfail 3 -x` and `-x --maxfail 3`: the arguments
    // after `--` come last.
    assert_eq!(maxfail_of(&["--maxfail", "3", "--", "-x"], &none), Some(1));
    assert_eq!(maxfail_of(&["-x", "--", "--maxfail", "3"], &none), Some(3));
    assert_eq!(maxfail_of(&["-x", "--", "--maxfail=0"], &none), None);
    assert_eq!(maxfail_of(&["-x", "--", "-v"], &none), Some(1));
    // Without either, the framework's own (pytest's addopts).
    let addopts = from_contexts(&[serde_json::json!({ "maxfail": 2 })]);
    assert_eq!(maxfail_of(&[], &addopts), Some(2));
    assert_eq!(maxfail_of(&["--maxfail", "5"], &addopts), Some(5));
    assert_eq!(maxfail_of(&["--maxfail", "0"], &addopts), None);
    assert_eq!(maxfail_of(&["--", "--maxfail", "4"], &addopts), Some(4));
}

/// Finding F28: an `@file` after `--` hides what pytest's `-k` and `-m`
/// would be, but no adapter applies an expression at runtime (aye's engine
/// runs pytest's tests too), so every adapter's cases get the command
/// line's (else their own), rather than run unfiltered.
#[test]
fn unreadable_framework_args_leave_every_adapter_the_command_lines() {
    let pytest = serde_json::json!({"keyword": "not slow", "tags": "db"});
    let doctest = serde_json::json!({"keyword": "own", "tags": null});
    let unittest = serde_json::json!({});
    let contexts = [pytest, doctest, unittest];
    let argv = ["-k", "one", "-m", "fast", "--", "@args.txt"];
    for adapter in ["pytest", "doctest", "unittest"] {
        assert_eq!(
            expressions_for(adapter, &argv, &contexts),
            pair(Some("one"), Some("fast")),
            "{adapter}"
        );
    }
    // Without the command line's, each keeps its own.
    let argv = ["--", "@args.txt"];
    assert_eq!(
        expressions_for("pytest", &argv, &contexts),
        pair(Some("not slow"), Some("db"))
    );
    // `--native` changes nothing.
    assert_eq!(
        expressions_for(
            "pytest",
            &["--native", "-k", "one", "--", "@args.txt"],
            &contexts
        ),
        pair(Some("one"), Some("db"))
    );
    assert_eq!(
        expressions_for("doctest", &argv, &contexts),
        pair(Some("own"), None)
    );
    assert_eq!(
        expressions_for("unittest", &argv, &contexts),
        pair(None, None)
    );
    // An empty one turns selection off for them too.
    assert_eq!(
        expressions_for("doctest", &["-k", "", "--", "@args.txt"], &contexts),
        pair(None, None)
    );
    // An unknown short option that may hold the letter is as unreadable.
    assert_eq!(
        expressions_for("unittest", &["-k", "one", "--", "-Zk", "x"], &contexts),
        pair(Some("one"), None)
    );
    assert_eq!(
        expressions_for("pytest", &["-k", "one", "--", "-Zk", "x"], &contexts),
        pair(Some("one"), Some("db"))
    );
}

#[test]
fn details_come_at_the_second_level_unless_the_flags_say() {
    let verbosity = |argv: &[&str], details, format| {
        let mut all = vec!["aye show"];
        all.extend(argv);
        let args = ShowArgs::try_parse_from(all).unwrap();
        let settings = ShowSettings {
            cwd: PathBuf::from("/repo"),
            root: PathBuf::from("/repo"),
            keep_runs: 10,
            format,
            verbosity: Verbosity::new(args.verbose, args.quiet),
            color: args.color.into(),
        };
        settings.level(details).count()
    };
    // Details: text's -v, JSON's -vv.
    assert_eq!(verbosity(&[], true, OutputFormat::Text), 1);
    assert_eq!(verbosity(&[], true, OutputFormat::Json), 2);
    // The list: a run's default.
    assert_eq!(verbosity(&[], false, OutputFormat::Text), 0);
    assert_eq!(verbosity(&[], false, OutputFormat::Json), 0);
    // Flags mean what they mean in a run.
    assert_eq!(verbosity(&["-v"], true, OutputFormat::Json), 1);
    assert_eq!(verbosity(&["-vvv"], true, OutputFormat::Text), 3);
    assert_eq!(verbosity(&["-q"], true, OutputFormat::Json), -1);
    assert_eq!(verbosity(&["-v", "-q"], true, OutputFormat::Text), 0);
    assert_eq!(verbosity(&["-vv"], false, OutputFormat::Text), 2);
}
