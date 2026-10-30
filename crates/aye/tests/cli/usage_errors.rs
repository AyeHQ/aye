//! Usage errors: what the command line, `[tool.aye]` or the framework's
//! arguments get wrong, a run of the whole home directory, and which error
//! wins when there are several.

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Output;

use aye_test::{code, project, stdout};

use crate::{PASSING, aye, aye_config, finished, logged, python, summary};
#[cfg(unix)]
use crate::{aye_command, context};

#[test]
fn framework_args_that_replace_the_run_are_usage_errors() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING)]);
    for (arg, hint) in [
        ("--co", "use `aye --co`"),
        ("--collect-only", "use `aye --co`"),
        ("--help", "use `aye --help`"),
        ("-V", "use `aye --version`"),
        ("--fixtures", "no equivalent"),
        ("--fixtures-per-test", "no equivalent"),
        ("--markers", "no equivalent"),
    ] {
        let output = aye(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "--", arg],
        );
        assert_eq!(code(&output), 4, "{arg}");
        // Refused before anything is reported or run.
        assert!(output.stdout.is_empty(), "{arg}: {}", stdout(&output));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("`-- {arg}`")), "{arg}: {stderr}");
        assert!(stderr.contains(hint), "{arg}: {stderr}");
    }
    // pytest's setup-only modes run no test: every test would be reported,
    // and remembered for `--lf`, as skipped.
    for arg in ["--setup-plan", "--setupplan", "--setup-only", "--setuponly"] {
        let output = aye(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "--", arg],
        );
        assert_eq!(code(&output), 4, "{arg}");
        assert!(output.stdout.is_empty(), "{arg}: {}", stdout(&output));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(&format!("`-- {arg}`")), "{arg}: {stderr}");
        assert!(
            stderr.contains("python -m pytest --setup-"),
            "{arg}: {stderr}"
        );
    }
    assert!(!dir.path().join(".aye/history.json").exists());
    // `--setup-show` runs the tests.
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "--", "--setup-show"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(
        finished(&output_log),
        [("tests/test_a.py::test_a".to_string(), "passed".to_string())]
    );
}

/// Finding C34: every `[tool.aye]` value is checked, and a bad one is a
/// usage error, `aye config` included.
#[test]
fn bad_tool_aye_values_are_usage_errors() {
    let Some(python) = python() else { return };
    for table in [
        "adapters = [\"bogus\"]",
        "adapters = []",
        "timeout = -5",
        "python = \"\"",
        "[tool.aye.unittest]\nfoo = 1",
        "adapters = [\"pytest\"]\n[tool.aye.doctest]\nmodules = \"yes\"",
    ] {
        let dir = project(&[
            (
                "pyproject.toml",
                &format!("[project]\nname = \"p\"\n\n[tool.aye]\n{table}\n"),
            ),
            ("test_a.py", PASSING),
        ]);
        let output = aye(&python, dir.path(), &[]);
        assert_eq!(code(&output), 4, "{table}: {}", stdout(&output));
        assert_eq!(code(&aye_config(&python, dir.path())), 4, "{table}");
    }
}

/// pytest rejects an option nothing defines (status 4, `unrecognized
/// arguments`), in every session; aye shows it once.
#[test]
fn an_option_nothing_defines_is_a_usage_error() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/test_b.py", PASSING),
        ("tests/test_c.py", PASSING),
    ]);
    let output = aye(
        &python,
        dir.path(),
        &["--color", "never", "-n", "3", "--", "--bogus-opt"],
    );
    assert_eq!(code(&output), 4, "{}", stdout(&output));
    let text = stdout(&output);
    assert_eq!(text.matches("error[usage-error]").count(), 1, "{text}");
    assert_eq!(summary(&output), "usage error · ! 1 usage error", "{text}");
}

/// With no path, a run from the home directory (a project root when it is
/// under git) would collect every project in it, as pytest would: aye
/// refuses at once and says what to do. A path given runs as usual.
#[cfg(unix)]
#[test]
fn a_run_from_the_home_directory_without_paths_is_refused() {
    let Some(python) = python() else { return };
    let home = project(&[("tests/test_a.py", "def test_a():\n    pass\n")]);
    fs::create_dir(home.path().join(".git")).unwrap();
    for args in [&[][..], &["run"][..], &["-k", "a", "--co"][..]] {
        let output = aye_command(&python, home.path(), args)
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert_eq!(code(&output), 4, "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("aye would collect your whole home directory"),
            "{stderr}"
        );
        assert_eq!(stdout(&output), "");
    }
    let output = aye_command(&python, home.path(), &["tests"])
        .env("HOME", home.path())
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stdout(&output));
}

/// aye in `cwd` with only the environment given: `PATH` an empty
/// directory and `HOME` as given, so that no interpreter is found unless
/// one is asked for, and the home directory is the one the test chose.
#[cfg(unix)]
fn aye_alone(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    let path = tempfile::tempdir().unwrap();
    context()
        .bare(cwd)
        .args(args)
        .env("PATH", path.path())
        .env("HOME", home)
        .env("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1")
        .output()
        .unwrap()
}

/// When a command line and a project have several things wrong, the one
/// found first in the order aye resolves a run's settings is the one
/// reported: the command line's own values, `[tool.aye]`, the interpreter,
/// the whole-home rule, the adapters' names, the tables of those that don't
/// run, `--no-refine` without `--co`, the configuration of those that do,
/// then the command line's `-k` and `-m`. Each pair below has both, and
/// `aye config` resolves as a run does. `aye show` checks its own flags,
/// then `[tool.aye]`, then that a run is stored, before its expressions.
#[cfg(unix)]
#[test]
fn the_first_usage_error_in_resolution_order_wins() {
    let python = "/nonexistent/bin/python";
    let bad_table = "[tool.aye.unittest]\nbogus = \"x\"\n";
    let cases: [(&str, &[&str], &str, &str); 12] = [
        (
            "a -k value that is a flag, before [tool.aye]",
            &["-k", "-x"],
            "[tool.aye]\noutput = \"xml\"\n",
            "aye: error: argument -k: expected one argument",
        ),
        (
            "arguments after -- that replace the run, before [tool.aye]",
            &["--", "--co"],
            "[tool.aye]\noutput = \"xml\"\n",
            "aye: error: `-- --co` would make pytest collect the tests without running them; \
             use `aye --co`",
        ),
        (
            "[tool.aye], before the interpreter",
            &[],
            "[tool.aye]\noutput = \"xml\"\n",
            "aye: error: invalid [tool.aye] in <root>/pyproject.toml: output = \"xml\": \
             expected text or json",
        ),
        (
            "the interpreter, before the whole-home rule",
            &[],
            "",
            "aye: error: no Python interpreter found: pass --python, or set [tool.aye] python",
        ),
        (
            "the whole-home rule, before the adapters' names",
            &["--python", python],
            "[tool.aye]\nadapters = [\"nose\"]\n",
            "aye: error: with no path given, aye would collect your whole home directory \
             (<root>). Run aye inside a project, or name what to test (aye tests/)",
        ),
        (
            "an unknown adapter, before the table of one that doesn't run",
            &["--python", python, "."],
            &format!("[tool.aye]\nadapters = [\"nose\"]\n\n{bad_table}"),
            "aye: error: [tool.aye] adapters: unknown adapter \"nose\" (known: pytest, \
             unittest, doctest)",
        ),
        (
            "the table of an adapter that doesn't run, before --no-refine",
            &["--python", python, "--no-refine", "."],
            &format!("[tool.aye]\nadapters = [\"pytest\"]\n\n{bad_table}"),
            "aye: error: invalid unittest configuration: invalid [tool.aye.unittest] in \
             <root>/pyproject.toml: unknown key \"bogus\" (expected start-directory, pattern \
             or top-level-directory)",
        ),
        (
            "--no-refine without --co, before preparing the adapters",
            &["--python", python, "--no-refine", "."],
            &format!("[tool.aye]\nadapters = [\"pytest\", \"unittest\"]\n\n{bad_table}"),
            "aye: error: --no-refine lists tests without importing them, with --co; a run of \
             pytest's tests imports every file it runs (aye's pytest front-end reads each \
             one), so it can't do without",
        ),
        (
            "an adapter's configuration, before the command line's expressions",
            &["--python", python, "-k", "not (", "."],
            &format!("[tool.aye]\nadapters = [\"unittest\"]\n\n{bad_table}"),
            "aye: error: invalid unittest configuration: invalid [tool.aye.unittest] in \
             <root>/pyproject.toml: unknown key \"bogus\" (expected start-directory, pattern \
             or top-level-directory)",
        ),
        (
            "aye config, in the same order",
            &["config", "--python", python, "-k", "not (", "."],
            &format!("[tool.aye]\nadapters = [\"unittest\"]\n\n{bad_table}"),
            "aye: error: invalid unittest configuration: invalid [tool.aye.unittest] in \
             <root>/pyproject.toml: unknown key \"bogus\" (expected start-directory, pattern \
             or top-level-directory)",
        ),
        (
            "aye show's flags, before [tool.aye]",
            &["show", "-m", "-x"],
            "[tool.aye]\noutput = \"xml\"\n",
            "aye: error: argument -m: expected one argument",
        ),
        (
            "aye show's [tool.aye]",
            &["show", "-k", "not ("],
            "[tool.aye]\noutput = \"xml\"\n",
            "aye: error: invalid [tool.aye] in <root>/pyproject.toml: output = \"xml\": \
             expected text or json",
        ),
    ];
    for (case, args, table, error) in cases {
        let pyproject = format!("[project]\nname = \"p\"\nversion = \"0\"\n\n{table}");
        let dir = project(&[("pyproject.toml", &pyproject)]);
        let root = fs::canonicalize(dir.path()).unwrap();
        // The home directory is the project only where the whole-home rule
        // is meant to apply.
        let other = tempfile::tempdir().unwrap();
        let home = if args.contains(&".") || args.first() == Some(&"show") {
            other.path()
        } else {
            &root
        };
        let output = aye_alone(&root, home, args);
        let stderr =
            String::from_utf8_lossy(&output.stderr).replace(root.to_str().unwrap(), "<root>");
        assert_eq!((code(&output), stderr.trim_end()), (4, error), "{case}");
    }
    // The last of them alone; and `aye show`'s expressions come after it
    // finds no run.
    let dir = project(&[]);
    let root = fs::canonicalize(dir.path()).unwrap();
    let output = aye_alone(&root, &root, &["--python", python, "-k", "not (", "."]);
    assert_eq!(
        (
            code(&output),
            String::from_utf8_lossy(&output.stderr).trim_end()
        ),
        (
            4,
            "aye: error: wrong expression passed to '-k': not (: at column 6: expected not OR \
             left parenthesis OR identifier; got end of input"
        )
    );
    let output = aye_alone(&root, &root, &["show", "-k", "not ("]);
    let stderr = String::from_utf8_lossy(&output.stderr).replace(root.to_str().unwrap(), "<root>");
    assert_eq!(
        (code(&output), stderr.trim_end()),
        (
            5,
            "aye: no run is stored in <root>/.aye/runs yet: run aye first"
        )
    );
}
