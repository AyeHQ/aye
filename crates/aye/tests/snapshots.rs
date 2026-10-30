//! Snapshots of what the binary prints (insta, insta-cmd), recorded with aye
//! 0.0.1: `aye --help` and `aye show --help`, `aye config` and the text and
//! JSON output at each verbosity on each example, and the usage errors
//! (a bad `-k`, `-- --pdb`, `--output human`, `-n x`, an unknown adapter,
//! `[tool.aye]` type errors), with their exit codes and stderr.
//!
//! Hermetic: each run gets a fresh copy of its project, and the snapshots
//! leave out what varies between machines and runs (the project's path, the
//! interpreter's path and version, durations, pids, run ids, aye's
//! version: aye-test's filters). Runs with the interpreter aye-test finds,
//! which has pytest, so that aye's own `pytest` module is shown to win over
//! it; without one the tests are skipped, loudly. Unix only: the paths in
//! the output are written otherwise on Windows.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use aye_static::EnvVars;
use aye_test::{Needs, TestContext, copy_project, examples_dir, filters, test_python};
use insta::internals::Content;
use insta_cmd::assert_cmd_snapshot;

/// The interpreter, as it names itself (`sys.executable`): the path the
/// output shows, which the snapshots leave out.
fn python() -> Option<PathBuf> {
    let python = test_python(Needs::Pytest)?;
    let output = Command::new(&python)
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", python.display());
    let executable = String::from_utf8(output.stdout).unwrap();
    Some(PathBuf::from(executable.trim_end()))
}

/// A fresh copy of a project, in a directory of its own name (the text
/// format's header names it).
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Project {
    /// A copy of the example `name`.
    fn example(name: &str) -> Self {
        let project = Self::empty(name);
        copy_project(&examples_dir().join(name), &project.root, &[]);
        project
    }

    /// A project of `files`, (path, contents).
    fn of(name: &str, files: &[(&str, &str)]) -> Self {
        let project = Self::empty(name);
        for (path, contents) in files {
            let path = project.root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        project
    }

    fn empty(name: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(name);
        fs::create_dir_all(&root).unwrap();
        Self { _dir: dir, root }
    }
}

/// `aye` with `args`, in `cwd`, in aye-test's hermetic environment, which
/// names nothing of the machine's beyond the interpreter, without colour
/// and 120 columns wide.
fn aye(cwd: &Path, args: &[&str]) -> Command {
    let mut command = TestContext::new(env!("CARGO_BIN_EXE_aye")).command(cwd);
    command
        .args(args)
        .env(EnvVars::NO_COLOR, "1")
        .env("COLUMNS", "120");
    command
}

/// `aye --python <python> args`, or `aye config --python <python>`.
fn run(project: &Project, python: &Path, args: &[&str]) -> Command {
    let python = python.to_str().unwrap();
    let mut all = Vec::new();
    if args.first() == Some(&"config") {
        all.push("config");
    }
    all.extend(["--python", python]);
    all.extend(args.iter().filter(|arg| **arg != "config"));
    aye(&project.root, &all)
}

/// What the snapshots leave out: aye-test's [`filters`] (`root`'s path,
/// the interpreter's path and version, durations, pids, run ids and aye's
/// version), and the interpreter's path in the snapshot's command.
fn settings(root: Option<&Path>, python: Option<&Path>) -> insta::Settings {
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    for (pattern, replacement) in filters(root, python) {
        settings.add_filter(&pattern, replacement);
    }
    if let Some(python) = python {
        let python = python.to_str().unwrap().to_owned();
        settings.add_dynamic_redaction(".args[]", move |value: Content, _| {
            if value.as_str() == Some(python.as_str()) {
                Content::from("[PYTHON]")
            } else {
                value
            }
        });
    }
    settings
}

#[test]
fn help() {
    let dir = tempfile::tempdir().unwrap();
    settings(None, None).bind(|| {
        assert_cmd_snapshot!("help", aye(dir.path(), &["--help"]));
        assert_cmd_snapshot!("show_help", aye(dir.path(), &["show", "--help"]));
    });
}

const EXAMPLES: [&str; 3] = ["pytest", "unittest", "doctest"];

#[test]
fn config() {
    let Some(python) = python() else { return };
    for example in EXAMPLES {
        let project = Project::example(example);
        settings(Some(&project.root), Some(&python)).bind(|| {
            assert_cmd_snapshot!(
                format!("config_{example}"),
                run(&project, &python, &["config"])
            );
        });
    }
}

/// The text and JSON formats at each verbosity, on each example: every
/// test there passes, is skipped or is expected to fail.
#[test]
fn outputs() {
    let Some(python) = python() else { return };
    let verbosities: [(&str, &[&str]); 5] = [
        ("q", &["-q"]),
        ("default", &[]),
        ("v", &["-v"]),
        ("vv", &["-vv"]),
        ("vvv", &["-vvv"]),
    ];
    for example in EXAMPLES {
        for (format, format_args) in [("text", &[][..]), ("json", &["--output", "json"][..])] {
            for (level, level_args) in verbosities {
                let project = Project::example(example);
                let args: Vec<&str> = format_args.iter().chain(level_args).copied().collect();
                settings(Some(&project.root), Some(&python)).bind(|| {
                    assert_cmd_snapshot!(
                        format!("{example}_{format}_{level}"),
                        run(&project, &python, &args)
                    );
                });
            }
        }
    }
}

const PYPROJECT: &str = "[project]\nname = \"p\"\nversion = \"0\"\n";

const TEST_FILE: (&str, &str) = ("tests/test_one.py", "def test_one():\n    pass\n");

#[test]
fn usage_errors() {
    let Some(python) = python() else { return };
    let cases: [(&str, &[&str]); 4] = [
        ("usage_bad_k", &["-k", "and"]),
        ("usage_pdb_after_dashes", &["--", "--pdb"]),
        ("usage_output_human", &["--output", "human"]),
        ("usage_workers_x", &["-n", "x"]),
    ];
    for (name, args) in cases {
        let project = Project::example("pytest");
        settings(Some(&project.root), Some(&python)).bind(|| {
            assert_cmd_snapshot!(name, run(&project, &python, args));
        });
    }
    let tables = [
        ("usage_unknown_adapter", "adapters = [\"pytest\", \"nose\"]"),
        ("usage_tool_aye_keep_runs", "keep-runs = \"ten\""),
        ("usage_tool_aye_workers", "workers = \"many\""),
        ("usage_tool_aye_output", "output = \"human\""),
        ("usage_tool_aye_adapters", "adapters = \"pytest\""),
    ];
    for (name, table) in tables {
        let pyproject = format!("{PYPROJECT}\n[tool.aye]\n{table}\n");
        let project = Project::of("p", &[("pyproject.toml", &pyproject), TEST_FILE]);
        settings(Some(&project.root), Some(&python)).bind(|| {
            assert_cmd_snapshot!(name, run(&project, &python, &[]));
        });
    }
}
