//! `aye config`: the output format at its verbosity, and where the
//! interpreter came from.

use std::fs;
#[cfg(unix)]
use std::path::Path;

use aye_test::{code, project, stdout};

use crate::{PASSING, aye_config, context, python};

/// `aye config` shows the output format at its verbosity, and where the
/// format came from.
#[test]
fn aye_config_shows_the_output() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING)]);
    let shown = |args: &[&str]| {
        let output = context()
            .config(&python, dir.path(), args)
            .output()
            .unwrap();
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        stdout(&output)
            .lines()
            .find_map(|line| line.strip_prefix("output   "))
            .unwrap_or_else(|| panic!("no output line: {}", stdout(&output)))
            .to_owned()
    };
    assert_eq!(shown(&[]), "text  (default)");
    assert_eq!(shown(&["-q"]), "text -q  (default)");
    assert_eq!(shown(&["-qqq"]), "text -q  (default)");
    assert_eq!(shown(&["--output", "json", "-v"]), "json -v  (--output)");
    assert_eq!(
        shown(&["--output", "JSON", "-vvvvv"]),
        "json -vvv  (--output)"
    );
    assert_eq!(
        shown(&["--output", "json", "-vv", "-q"]),
        "json -v  (--output)"
    );
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\noutput = \"Json\"\n",
    )
    .unwrap();
    assert_eq!(shown(&[]), "json  ([tool.aye] output)");
    assert_eq!(shown(&["--output", "text", "-vv"]), "text -vv  (--output)");
    let output = aye_config(&python, dir.path());
    assert!(
        stdout(&output).contains("\n  output = json    # pyproject.toml [tool.aye]\n"),
        "{}",
        stdout(&output)
    );
}

/// `aye config` says where the interpreter came from: the one asked for by
/// `--python`, `$AYE_PYTHON` or `[tool.aye] python`, in that order, else
/// the one an active `$VIRTUAL_ENV`, the project's `.venv` or `PATH`
/// offers. None of them is started.
#[cfg(unix)]
#[test]
fn aye_config_says_where_the_interpreter_came_from() {
    // The arguments, the variables, `[tool.aye]`, whether the project has a
    // `.venv`, and the line.
    type Case<'a> = (
        &'a [&'a str],
        &'a [(&'a str, &'a Path)],
        &'a str,
        bool,
        String,
    );
    let fake = |path: &Path| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    };
    let elsewhere = tempfile::tempdir().unwrap();
    let elsewhere = fs::canonicalize(elsewhere.path()).unwrap();
    let venv = elsewhere.join("venv");
    fake(&venv.join("bin/python"));
    let bin = elsewhere.join("bin");
    fake(&bin.join("python3"));
    let cases: [Case<'_>; 6] = [
        (
            &["--python", "/x/flag/python"],
            &[("AYE_PYTHON", Path::new("/x/env/python"))],
            "python = \"/x/cfg/python\"\n",
            true,
            "/x/flag/python  (--python)".to_string(),
        ),
        (
            &[],
            &[("AYE_PYTHON", Path::new("/x/env/python"))],
            "python = \"/x/cfg/python\"\n",
            true,
            "/x/env/python  ($AYE_PYTHON)".to_string(),
        ),
        (
            &[],
            &[("VIRTUAL_ENV", &venv)],
            "python = \"/x/cfg/python\"\n",
            true,
            "/x/cfg/python  ([tool.aye] python)".to_string(),
        ),
        (
            &[],
            &[("VIRTUAL_ENV", &venv)],
            "",
            true,
            format!("{}  ($VIRTUAL_ENV)", venv.join("bin/python").display()),
        ),
        (
            &[],
            &[("PATH", &bin)],
            "",
            true,
            "<root>/.venv/bin/python  (.venv)".to_string(),
        ),
        (
            &[],
            &[("PATH", &bin)],
            "",
            false,
            format!("{}  (PATH)", bin.join("python3").display()),
        ),
    ];
    for (args, vars, table, project_venv, shown) in cases {
        let pyproject = format!("[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\n{table}");
        let dir = project(&[("pyproject.toml", &pyproject)]);
        let root = fs::canonicalize(dir.path()).unwrap();
        if project_venv {
            fake(&root.join(".venv/bin/python"));
        }
        let mut command = context().bare(&root);
        command
            .arg("config")
            .args(args)
            .env("PATH", &elsewhere)
            .env("HOME", &elsewhere);
        for (name, value) in vars {
            command.env(name, value);
        }
        let output = command.output().unwrap();
        assert_eq!(code(&output), 0, "{shown}: {}", stdout(&output));
        let line = stdout(&output)
            .lines()
            .find_map(|line| line.strip_prefix("python   "))
            .unwrap()
            .replace(root.to_str().unwrap(), "<root>");
        assert_eq!(line, shown);
    }
}
