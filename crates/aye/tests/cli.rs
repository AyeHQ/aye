//! The binary end to end, on small projects written for each test: how
//! arguments are read, exit codes, and what reaches stdout, the JUnit report
//! and history.
//!
//! Runs with the workspace's `.venv` (from `uv sync`), or `AYE_TEST_PYTHON`,
//! which have pytest installed, so that aye's own `pytest` module is shown
//! to win over it (aye never imports pytest); some tests also make a venv
//! without pytest. Without one the tests are skipped, loudly. Every run has
//! `PYTEST_DISABLE_PLUGIN_AUTOLOAD=1`, so that the plugins the interpreter
//! happens to install (platformdirs ships one) don't add warnings to what a
//! test checks.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Map, Value, json};

#[expect(clippy::print_stderr, reason = "a test says why it skips")]
fn python() -> Option<PathBuf> {
    let candidate = std::env::var_os("AYE_TEST_PYTHON").map_or_else(
        || {
            let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv");
            if cfg!(windows) {
                venv.join("Scripts/python.exe")
            } else {
                venv.join("bin/python")
            }
        },
        PathBuf::from,
    );
    let ok = Command::new(&candidate)
        .args(["-c", "import pytest"])
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Some(candidate)
    } else {
        eprintln!("skipping: no Python with pytest (run `uv sync`, or set AYE_TEST_PYTHON)");
        None
    }
}

/// A project in a fresh directory: `files` are (path, contents), and a
/// `pyproject.toml` is added at the top unless one is given.
fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    if !files.iter().any(|(path, _)| *path == "pyproject.toml") {
        fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"p\"\nversion = \"0\"\n",
        )
        .unwrap();
    }
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}

/// What pytest would autoload from the interpreter's packages is left out
/// of every run (see the module's docs).
const NO_AUTOLOAD: (&str, &str) = ("PYTEST_DISABLE_PLUGIN_AUTOLOAD", "1");

fn aye_command(python: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aye"));
    command
        .arg("--python")
        .arg(python)
        .args(args)
        .current_dir(cwd)
        .env_remove("GITHUB_ACTIONS")
        .env_remove("PYTEST_ADDOPTS")
        .env(NO_AUTOLOAD.0, NO_AUTOLOAD.1);
    command
}

fn aye(python: &Path, cwd: &Path, args: &[&str]) -> Output {
    aye_command(python, cwd, args).output().unwrap()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Each line of stdout, read as JSON.
fn records(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

/// The run store of the project `cwd` is in.
fn store(cwd: &Path) -> aye_history::RunStore {
    let root = cwd
        .ancestors()
        .find(|dir| dir.join("pyproject.toml").is_file())
        .unwrap_or(cwd);
    aye_history::RunStore::new(root.join(".aye").join(aye_history::runs::DIR))
}

/// The runs the project `cwd` is in keeps.
fn runs_in(cwd: &Path) -> HashSet<String> {
    store(cwd)
        .runs()
        .into_iter()
        .map(|meta| meta.run.to_string())
        .collect()
}

/// The log of the run the project `cwd` is in kept since `before`, every
/// record as the run wrote it (`aye show --log`; what `--output json
/// -vvv` printed before the detail ladder); `None` when it kept none.
fn kept(cwd: &Path, before: &HashSet<String>) -> Option<Vec<Value>> {
    let store = store(cwd);
    let meta = store
        .runs()
        .into_iter()
        .find(|meta| !before.contains(&meta.run.to_string()))?;
    let log = store.log(&meta.run.to_string()).unwrap();
    Some(
        log.iter()
            .map(|record| serde_json::to_value(record).unwrap())
            .collect(),
    )
}

/// Runs aye with `args` in `cwd`, and the run's log: the one it kept, or
/// for a run that keeps none (collect-only, or nothing to show) what its
/// JSON lines say, as records ([`as_records`]; `args` must ask for JSON).
fn logged(python: &Path, cwd: &Path, args: &[&str]) -> (Output, Vec<Value>) {
    let before = runs_in(cwd);
    let output = aye(python, cwd, args);
    let log = kept(cwd, &before).unwrap_or_else(|| as_records(&output));
    (output, log)
}

/// What a run's JSON lines say, as the records its log would hold: each
/// case, diagnostic and failed worker, and the summary as
/// `collection_finished` and `run_finished`. For runs that keep no log.
fn as_records(output: &Output) -> Vec<Value> {
    let mut log = Vec::new();
    for line in records(output) {
        let record = match line["type"].as_str().unwrap_or_default() {
            "case" => json!({
                "event": "case_collected",
                "id": line["id"],
                "certainty": if line["partial"] == true { "partial" } else { "exact" },
            }),
            "collection_error" => json!({
                "event": "diagnostic",
                "severity": "error",
                "code": line["code"],
                "path": line["path"],
                "message": line["error"],
            }),
            "diagnostic" => json!({
                "event": "diagnostic",
                "severity": line["severity"],
                "code": line["code"],
                "path": line["path"],
                "message": line["message"],
            }),
            "worker_error" => json!({"event": "worker_finished", "error": line["error"]}),
            "summary" => {
                let count = |name: &str| line.get(name).cloned().unwrap_or(json!(0));
                log.push(json!({
                    "event": "collection_finished",
                    "selected": count("tests"),
                    "deselected": count("deselected"),
                }));
                let mut finished = Map::new();
                finished.insert("event".into(), json!("run_finished"));
                finished.insert("exit_code".into(), line["exit"].clone());
                finished.insert(
                    "counts".into(),
                    json!({
                        "passed": count("passed"),
                        "failed": count("failed"),
                        "error": count("errors"),
                        "skipped": count("skipped"),
                        "expected_failure": count("expected_failure"),
                        "unexpected_pass": count("unexpected_pass"),
                        "deselected": count("deselected"),
                    }),
                );
                if let Some(stopped) = line.get("stopped") {
                    finished.insert("stopped".into(), stopped.clone());
                }
                Value::Object(finished)
            }
            _ => continue,
        };
        log.push(record);
    }
    log
}

fn events<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["event"] == kind).collect()
}

/// `(id, outcome)` of every `test_finished`.
fn finished(records: &[Value]) -> Vec<(String, String)> {
    events(records, "test_finished")
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["outcome"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

const PASSING: &str = "def test_a():\n    pass\n";

#[test]
fn an_explicit_path_without_tests_is_no_tests_not_a_usage_error() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/test_empty.py", "x = 1\n"),
        ("tests/emptydir/README", ""),
        ("tests/pkg/__init__.py", ""),
        (
            "tests/test_modskip.py",
            "import pytest\npytest.skip('module', allow_module_level=True)\n\ndef test_never():\n    pass\n",
        ),
    ]);
    for arg in [
        "tests/test_empty.py",
        "tests/emptydir",
        "tests/pkg/__init__.py",
        "tests/test_modskip.py",
    ] {
        let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv", arg]);
        assert_eq!(code(&output), 5, "{arg}: {}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "diagnostic")
                .iter()
                .all(|d| d["severity"] != "error"),
            "{arg}: {records:#?}"
        );
    }
    let collect = aye(&python, dir.path(), &["--co", "tests/test_empty.py"]);
    assert_eq!(code(&collect), 5);
    // With another argument that has tests, those run.
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &[
            "--output",
            "json",
            "-vvv",
            "tests/test_empty.py",
            "tests/emptydir",
            "tests/test_a.py",
        ],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(
        finished(&output_log),
        [("tests/test_a.py::test_a".to_owned(), "passed".to_owned())]
    );
    // Or another kind of selector: a dotted name.
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &[
            "--output",
            "json",
            "-vvv",
            "tests/test_empty.py",
            "tests.test_a",
        ],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(finished(&output_log).len(), 1);
}

#[test]
fn what_names_nothing_is_a_not_found_diagnostic_in_the_stream() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING), ("data.json", "{}")]);
    for (args, path, message) in [
        (
            vec!["tests/test_a.py::test_nope"],
            "tests/test_a.py",
            "not found: tests/test_a.py::test_nope",
        ),
        (vec!["abcd1234"], "", "not found: abcd1234"),
        (
            vec!["pkg.nomod", "tests/test_a.py"],
            "",
            "not found: pkg.nomod",
        ),
        // A missing path is reported once, by collection.
        (
            vec!["tests/nope.py", "tests/test_a.py"],
            "tests/nope.py",
            "file or directory not found: tests/nope.py",
        ),
        // Also when collection isn't limited to paths.
        (
            vec!["tests/nope.py", "tests.test_a"],
            "tests/nope.py",
            "file or directory not found: tests/nope.py",
        ),
        (vec!["tests", "nope_key"], "", "not found: nope_key"),
    ] {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 4, "{args:?}: {}", stdout(&output));
        let records = output_log;
        let errors: Vec<_> = events(&records, "diagnostic")
            .into_iter()
            .filter(|d| d["severity"] == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{args:?}: {errors:#?}");
        assert_eq!(errors[0]["code"], "not-found", "{args:?}");
        assert_eq!(errors[0]["path"], path, "{args:?}");
        assert_eq!(errors[0]["message"], message, "{args:?}");
        assert_eq!(
            finished(&records),
            [] as [(std::string::String, std::string::String); 0]
        );
        assert_eq!(records.last().unwrap()["exit_code"], 4);
    }
    // The JSON lines say why it exits 4, on stdout alone.
    let lines = aye(
        &python,
        dir.path(),
        &["--output", "json", "-v", "tests/test_a.py::test_nope"],
    );
    assert!(
        stdout(&lines).starts_with(
            r#"{"type":"collection_error","path":"tests/test_a.py","code":"not-found","error":"not found: tests/test_a.py::test_nope"}"#
        ),
        "{}",
        stdout(&lines)
    );
    // A file no adapter collects is pytest's "found no collectors".
    assert_eq!(code(&aye(&python, dir.path(), &["data.json"])), 4);
    // Both are usage errors, as pytest's are, and say so.
    for arg in ["tests/test_a.py::test_nope", "data.json"] {
        let output = aye(&python, dir.path(), &["--color", "never", arg]);
        let text = stdout(&output);
        assert!(
            text.contains("\nusage error · ! 1 usage error   "),
            "{arg}: {text}"
        );
        assert!(!text.contains("collection error"), "{arg}: {text}");
    }
}

#[test]
fn paths_through_dot_dot_find_the_root() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_b.py",
            "def test_ok1():\n    pass\n\ndef test_ok2():\n    pass\n",
        ),
        ("src/app.py", ""),
    ]);
    let root = dir.path();
    for (cwd, args, ran) in [
        (root.join("tests"), vec![".."], 2),
        (root.join("src"), vec!["../tests/test_b.py"], 2),
        (
            root.to_path_buf(),
            vec!["tests/../tests/test_b.py::test_ok1"],
            1,
        ),
    ] {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, &cwd, &argv);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        let records = output_log;
        let started = events(&records, "run_started")[0]["root"].as_str().unwrap();
        assert!(!started.contains(".."), "{started}");
        assert_eq!(finished(&records).len(), ran, "{args:?}");
    }
}

#[test]
fn printed_reruns_work_from_a_subdirectory() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", "def test_fail():\n    assert 0\n")]);
    let tests = dir.path().join("tests");
    let first = aye(&python, &tests, &["--color", "never", "-v", "test_a.py"]);
    assert_eq!(code(&first), 1);
    let rerun = stdout(&first)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .map(str::to_owned)
        .unwrap();
    assert_eq!(rerun, "tests/test_a.py::test_fail");
    let (again, again_log) = logged(&python, &tests, &["--output", "json", "-vvv", &rerun]);
    assert_eq!(code(&again), 1, "{}", stdout(&again));
    assert_eq!(
        finished(&again_log),
        [("tests/test_a.py::test_fail".to_owned(), "failed".to_owned())]
    );
    // The current directory still comes first.
    let (_local, local_log) = logged(
        &python,
        &tests,
        &["--output", "json", "-vvv", "test_a.py::test_fail"],
    );
    assert_eq!(finished(&local_log).len(), 1);
}

#[test]
fn annotations_are_relative_to_the_repository() {
    let Some(python) = python() else { return };
    let top = tempfile::tempdir().unwrap();
    fs::create_dir_all(top.path().join(".git")).unwrap();
    let sub = top.path().join("sub");
    fs::create_dir_all(sub.join("tests")).unwrap();
    fs::write(sub.join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
    fs::write(
        sub.join("tests/test_x.py"),
        "def test_x():\n    assert 1 == 2\n",
    )
    .unwrap();
    // Beside text output, at every verbosity.
    for args in [&[][..], &["-q"], &["-vvv"]] {
        let output = aye_command(&python, &sub, args)
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_WORKSPACE", top.path())
            .output()
            .unwrap();
        assert_eq!(code(&output), 1);
        let text = stdout(&output);
        let annotation = text
            .lines()
            .find(|line| line.starts_with("::error"))
            .unwrap_or_else(|| panic!("{args:?}: {text}"));
        assert!(
            annotation.starts_with("::error file=sub/tests/test_x.py,line=2,"),
            "{annotation}"
        );
    }
    // Never in JSON lines.
    for verbosity in ["-q", "-v", "-vvv"] {
        let output = aye_command(&python, &sub, &["--output", "json", verbosity])
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_WORKSPACE", top.path())
            .output()
            .unwrap();
        assert_eq!(code(&output), 1);
        records(&output);
    }
}

#[test]
fn an_unwritable_junit_report_fails_before_the_run() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_a.py",
        "def test_a():\n    open('ran', 'w').close()\n",
    )]);
    let output = aye(
        &python,
        dir.path(),
        &["--junitxml", "pyproject.toml/out.xml"],
    );
    assert_eq!(code(&output), 4);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("pyproject.toml/out.xml"), "{stderr}");
    assert!(!dir.path().join("ran").exists());
    assert_eq!(stdout(&output), "");
    // A directory that doesn't exist yet is created.
    let output = aye(&python, dir.path(), &["--junitxml", "reports/ci/out.xml"]);
    assert_eq!(code(&output), 0);
    let xml = fs::read_to_string(dir.path().join("reports/ci/out.xml")).unwrap();
    assert!(xml.contains("tests=\"1\""), "{xml}");
}

#[test]
fn a_closed_stdout_still_gets_the_report_and_history_written() {
    let Some(python) = python() else { return };
    let tests: String = std::iter::once("def test_fail():\n    assert 0\n".to_owned())
        .chain((0..30).map(|i| format!("\ndef test_{i}():\n    pass\n")))
        .collect();
    let dir = project(&[
        ("tests/test_a.py", &tests),
        (
            "tests/test_zz.py",
            "def test_zz_last():\n    open('ran_last', 'w').close()\n",
        ),
    ]);
    for format in [
        &["--output", "json", "-vvv"][..],
        &["--output", "json", "-q"],
        &["--output", "json"],
        &["--output", "json", "-v"],
        &["--output", "text"],
        &["--output", "text", "-vvv"],
    ] {
        let _ = fs::remove_dir_all(dir.path().join(".aye"));
        let mut child = aye_command(
            &python,
            dir.path(),
            &[format, &["--junitxml", "out.xml"]].concat(),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
        // Nobody reads stdout: every write to it fails.
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        assert_eq!(code(&output), 1, "{format:?}");
        assert!(dir.path().join("ran_last").exists());
        let xml = fs::read_to_string(dir.path().join("out.xml")).unwrap();
        assert!(
            xml.contains(r#"<testsuites name="aye" tests="32" failures="1""#),
            "{format:?}: {xml}"
        );
        let history: Value = serde_json::from_str(
            &fs::read_to_string(dir.path().join(".aye/history.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            history["tests"].as_object().unwrap().len(),
            32,
            "{format:?}"
        );
    }
}

#[test]
fn the_last_of_exitfirst_and_maxfail_wins() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "test_m.py",
        "import pytest\n\n@pytest.mark.parametrize('i', range(8))\ndef test_t(i):\n    assert i % 2 == 0\n",
    )]);
    let failed = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "-n", "1"];
        argv.extend(args);
        let (_output, output_log) = logged(&python, dir.path(), &argv);
        let records = output_log;
        let run = events(&records, "run_finished")[0].clone();
        (run["counts"]["failed"].as_u64().unwrap(), run)
    };
    assert_eq!(failed(&["-x", "--maxfail", "3"]).0, 3);
    assert_eq!(failed(&["--maxfail", "3", "-x"]).0, 1);
    // The arguments after `--` come last, as in pytest's `--maxfail 3 -x`.
    assert_eq!(failed(&["--maxfail", "3", "--", "-x"]).0, 1);
    assert_eq!(failed(&["-x", "--", "--maxfail", "3"]).0, 3);
    assert_eq!(failed(&["-x", "--", "--maxfail=0"]).0, 4);
    // 0 is no limit, as in pytest.
    let (count, run) = failed(&["--maxfail", "0"]);
    assert_eq!(count, 4);
    assert!(run.get("stopped").is_none(), "{run}");
}

#[test]
fn a_keyword_may_look_like_a_negative_number() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_s.py",
        "import pytest\n\n@pytest.mark.parametrize('a', [1, 2])\n@pytest.mark.parametrize('b', ['x', 'y'])\ndef test_stacked(a, b):\n    pass\n",
    )]);
    let ids = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "--co"];
        argv.extend(args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}");
        events(&output_log, "case_collected")
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&["-k", "-1"]),
        [
            "tests/test_s.py::test_stacked[x-1]",
            "tests/test_s.py::test_stacked[y-1]"
        ]
    );
    assert_eq!(
        ids(&["-k", "-1 and x"]),
        ["tests/test_s.py::test_stacked[x-1]"]
    );
    // As in pytest, a value that looks like a flag is not one.
    assert_eq!(code(&aye(&python, dir.path(), &["-k", "-x"])), 4);
}

/// A hook that fails while pytest collects is pytest's `INTERNALERROR`
/// (status 3), which aye's pytest front-end meets while it lists the
/// files, in collection: the run exits 3 before anything runs, `--co` and
/// `--continue-on-collection-errors` too.
#[test]
fn internal_and_usage_errors_from_pytest_keep_their_exit_codes() {
    let Some(python) = python() else { return };
    let broken = project(&[
        ("tests/test_a.py", PASSING),
        (
            "tests/conftest.py",
            "def pytest_collection_modifyitems(items):\n    raise RuntimeError('hook boom')\n",
        ),
    ]);
    for args in [&["--color", "never"][..], &["--co", "--color", "never"]] {
        let output = aye(&python, broken.path(), args);
        assert_eq!(code(&output), 3, "{args:?}: {}", stdout(&output));
        assert!(
            stdout(&output).contains("internal error"),
            "{}",
            stdout(&output)
        );
        assert!(
            !stdout(&output).contains("collection error"),
            "{}",
            stdout(&output)
        );
    }
    let (output, output_log) = logged(
        &python,
        broken.path(),
        &[
            "--continue-on-collection-errors",
            "--output",
            "json",
            "-vvv",
        ],
    );
    assert_eq!(code(&output), 3);
    assert_eq!(finished(&output_log), []);

    // An initial conftest that fails to load: pytest exits 4.
    let dir = project(&[
        ("tests/conftest.py", "import nonexistent_module_xyz\n"),
        ("tests/test_a.py", PASSING),
        ("tests/sub/test_b.py", PASSING),
    ]);
    for workers in ["1", "2"] {
        let output = aye(&python, dir.path(), &["--color", "never", "-n", workers]);
        assert_eq!(code(&output), 4, "-n {workers}: {}", stdout(&output));
        let text = stdout(&output);
        assert_eq!(summary(&output), "usage error · ! 1 usage error", "{text}");
    }
}

/// A test's parameters come from the front-end's listing: a typed id with
/// parameters it doesn't make is `not-found` (pytest's `not found:`, status
/// 4), and the test's other parameters don't run in its place.
#[test]
fn a_typed_id_with_parameters_the_framework_cant_find_is_not_found() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_p.py",
        "import pytest\n\ndef gen():\n    return [i for i in range(2)]\n\n@pytest.mark.parametrize('x', gen())\ndef test_dyn(x):\n    pass\n",
    )]);
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "tests/test_p.py::test_dyn[9]"],
    );
    let missing = output_log;
    assert!(finished(&missing).is_empty(), "{}", stdout(&output));
    let errors: Vec<_> = events(&missing, "diagnostic")
        .into_iter()
        .filter(|d| d["severity"] == "error")
        .collect();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0]["code"], "not-found");
    assert_eq!(code(&output), 4);
    // One that exists runs alone.
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "tests/test_p.py::test_dyn[1]"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(
        finished(&output_log),
        [(
            "tests/test_p.py::test_dyn[1]".to_string(),
            "passed".to_string()
        )]
    );
}

#[test]
fn junit_keeps_the_failure_of_a_failed_subtest() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_s.py",
        "def test_sub(subtests):\n    for i in range(3):\n        with subtests.test(msg='case', i=i):\n            assert i != 1\n",
    )]);
    let output = aye(
        &python,
        dir.path(),
        &["--color", "never", "--junitxml", "j.xml"],
    );
    assert_eq!(code(&output), 1);
    let xml = fs::read_to_string(dir.path().join("j.xml")).unwrap();
    assert!(xml.contains("assert 1 != 1"), "{xml}");
    // aye's engine writes the traceback as Python does, the location with
    // the platform's separator.
    assert!(
        (xml.contains("tests/test_s.py") || xml.contains(r"tests\test_s.py"))
            && xml.contains("line 4"),
        "{xml}"
    );
    // One line, for the subtest; none for the test itself.
    let text = stdout(&output);
    assert_eq!(
        text.matches("tests/test_s.py::test_sub").count(),
        1,
        "{text}"
    );
    assert!(
        text.contains("tests/test_s.py::test_sub › case (i=1)\n"),
        "{text}"
    );
    let lines = aye(&python, dir.path(), &["--output", "json", "-v"]);
    let fails = stdout(&lines)
        .lines()
        .filter(|line| line.contains(r#""type":"fail""#))
        .count();
    assert_eq!(fails, 1, "{}", stdout(&lines));
}

#[test]
fn a_test_failed_only_by_its_subtests_keeps_its_output_and_rerun() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_s.py",
        "def test_sub(subtests):\n    print('diagnostic output here')\n    for i in range(3):\n        with subtests.test(msg='case', i=i):\n            assert i != 1\n",
    )]);
    // As pytest shows the test's captured stdout after the subtest's
    // section: at L3, everything.
    let output = aye(&python, dir.path(), &["--color", "never", "-vv"]);
    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    let key = aye_events::TestKey::of("tests/test_s.py::test_sub").to_string()[..5].to_owned();
    assert!(
        text.contains(&format!(
            "✗ {key}  tests/test_s.py::test_sub › case (i=1)\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "✗ {key}  tests/test_s.py::test_sub (contains 1 failed subtest)\n  stdout   diagnostic output here\n  rerun    aye tests/test_s.py::test_sub\n"
        )),
        "{text}"
    );
    let lines = aye(&python, dir.path(), &["--output", "json", "-vvv"]);
    let fails: Vec<Value> = stdout(&lines)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|line| line["type"] == "fail")
        .collect();
    assert_eq!(fails.len(), 1, "{}", stdout(&lines));
    assert_eq!(fails[0]["sub"], "case (i=1)");
    assert_eq!(fails[0]["output"]["stdout"], "diagnostic output here");
    assert_eq!(fails[0]["rerun"], "aye tests/test_s.py::test_sub");
}

#[test]
fn a_run_cut_short_says_why() {
    let Some(python) = python() else { return };
    let mut files = vec![];
    let failing = "def test_f():\n    assert 0\n\ndef test_p():\n    pass\n";
    for name in ["tests/test_1.py", "tests/test_2.py", "tests/test_3.py"] {
        files.push((name, failing));
    }
    let dir = project(&files);
    let output = aye(&python, dir.path(), &["--color", "never", "-x"]);
    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    assert!(
        text.contains("\nstopped after 1 failure (-x/--maxfail)\n✗ 1 failed   "),
        "{text}"
    );
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "-x", "-n", "2"],
    );
    assert_eq!(events(&output_log, "run_finished")[0]["stopped"], "maxfail");
    let lines = aye(
        &python,
        dir.path(),
        &["--output", "json", "-v", "--maxfail", "2"],
    );
    assert!(
        records(&lines).last().unwrap()["stopped"] == "maxfail",
        "{}",
        stdout(&lines)
    );
    // Nothing is said when every test ran.
    let (_all, all_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert!(events(&all_log, "run_finished")[0].get("stopped").is_none());

    let exits = project(&[(
        "tests/test_1.py",
        "import pytest\n\ndef test_a():\n    pass\n\ndef test_b():\n    pytest.exit('bye')\n\ndef test_c():\n    pass\n",
    )]);
    let output = aye(&python, exits.path(), &["--color", "never"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    assert!(
        summary(&output).starts_with("interrupted"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn maxfail_in_addopts_is_counted_across_workers() {
    let Some(python) = python() else { return };
    let failing = "def test_f():\n    assert 0\n\ndef test_p():\n    pass\n";
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"-x\"\n",
        ),
        ("tests/test_1.py", failing),
        ("tests/test_2.py", failing),
        ("tests/test_3.py", failing),
    ]);
    let run = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv", "-n", "3"];
        argv.extend(args);
        let records = logged(&python, dir.path(), &argv).1;
        events(&records, "run_finished")[0]["counts"]["failed"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(run(&[]), 1);
    // The command line has the last word, as it does in pytest.
    assert_eq!(run(&["--maxfail", "2"]), 2);
    assert_eq!(run(&["--maxfail", "0"]), 3);
}

#[test]
fn live_output_keeps_machine_readable_output_whole() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_a.py",
        "import sys\n\ndef test_prints():\n    print('{not json')\n    print('to stderr', file=sys.stderr)\n\ndef test_fails():\n    assert 0\n",
    )]);
    for format in [
        &["--output", "json", "-vvv"][..],
        &["--output", "json", "-v"],
        &["--output", "json"],
        &["--output", "json", "-q"],
    ] {
        let output = aye(&python, dir.path(), &[&["-s"], format].concat());
        assert_eq!(code(&output), 1, "{format:?}");
        for line in stdout(&output).lines() {
            assert!(
                serde_json::from_str::<Value>(line).is_ok(),
                "{format:?}: {line:?} in\n{}",
                stdout(&output)
            );
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("{not json"), "{format:?}: {stderr}");
    }
    // Beside text, on stdout, at every verbosity.
    for verbosity in ["-q", "-v", "-vvv"] {
        let output = aye(&python, dir.path(), &["-s", "--color", "never", verbosity]);
        assert_eq!(code(&output), 1, "{verbosity}");
        assert!(
            stdout(&output).contains("{not json"),
            "{verbosity}: {}",
            stdout(&output)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("{not json"), "{verbosity}: {stderr}");
    }
}

#[test]
fn failed_first_runs_last_failures_first() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "def test_a1():\n    pass\n\ndef test_a2():\n    pass\n",
        ),
        ("tests/test_b.py", "def test_b():\n    assert 0\n"),
    ]);
    assert_eq!(code(&aye(&python, dir.path(), &[])), 1);
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &["--ff", "-n", "1", "--output", "json", "-vvv"],
    );
    let started: Vec<_> = events(&output_log, "test_started")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        started,
        [
            "tests/test_b.py::test_b",
            "tests/test_a.py::test_a1",
            "tests/test_a.py::test_a2"
        ]
    );
}

#[cfg(unix)]
#[test]
fn sigterm_stops_the_run_and_its_workers() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow.py",
        "import os, time\n\ndef test_slow():\n    with open('pid', 'w') as f:\n        f.write(str(os.getpid()))\n    time.sleep(60)\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(&python, dir.path(), &["--output", "json", "-vvv"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = dir.path().join("pid");
    let started = Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the test never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let worker = fs::read_to_string(&pid_file).unwrap();
    let signal = |name: &str, pid: &str| {
        Command::new("kill")
            .args([name, pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(signal("-TERM", &child.id().to_string()));
    let stopped = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(stopped.elapsed() < Duration::from_secs(20));
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    assert_eq!(records.last().unwrap()["event"], "run_finished");
    assert_eq!(records.last().unwrap()["stopped"], "interrupted");
    // The worker is gone with it.
    let deadline = Instant::now();
    while signal("-0", worker.trim()) {
        assert!(
            deadline.elapsed() < Duration::from_secs(5),
            "worker {worker} outlived aye"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Finding F40: GNU `timeout` sends its signal to aye and then to its own
/// process group, aye included. The two are one stop request: the run still
/// stops cleanly, with `run_finished`, the JUnit report and history.
#[cfg(unix)]
#[test]
fn a_stop_signal_delivered_twice_still_stops_the_run_cleanly() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow.py",
        "import time\n\ndef test_fast():\n    pass\n\ndef test_slow():\n    open('started', 'w').close()\n    time.sleep(60)\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "--junitxml", "junit.xml"],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::null())
    .spawn()
    .unwrap();
    let started_file = dir.path().join("started");
    let started = Instant::now();
    while !started_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the test never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let pid = child.id().to_string();
    for signal in ["-TERM", "-TERM"] {
        let sent = Command::new("kill")
            .args([signal, &pid])
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(sent.success());
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    let last = records.last().unwrap();
    assert_eq!(last["event"], "run_finished");
    assert_eq!(last["stopped"], "interrupted");
    assert_eq!(
        finished(&records),
        [(
            "tests/test_slow.py::test_fast".to_string(),
            "passed".to_string()
        )]
    );
    let junit = fs::read_to_string(dir.path().join("junit.xml")).unwrap();
    assert!(junit.contains("test_fast"), "{junit}");
    assert!(dir.path().join(".aye/history.json").is_file());
}

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

#[test]
fn the_output_format_takes_any_case() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING)]);
    let (output, output_log) = logged(&python, dir.path(), &["--output", "JSON", "-vvv"]);
    assert_eq!(code(&output), 0);
    assert_eq!(finished(&output_log).len(), 1);
}

/// The output formats' old names (`human`, `agent`) are usage errors that
/// say what to use instead, on the command line and in `[tool.aye]`,
/// before anything runs.
#[test]
fn the_output_formats_old_names_are_usage_errors() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_a.py",
        "def test_a():\n    open('ran', 'w').close()\n",
    )]);
    for (name, instead) in [
        ("human", "use `--output text`"),
        ("HUMAN", "use `--output text`"),
        ("agent", "use `--output json -v`"),
        ("Agent", "use `--output json -v`"),
    ] {
        let output = aye(&python, dir.path(), &["--output", name]);
        assert_eq!(code(&output), 4, "{name}");
        assert_eq!(stdout(&output), "", "{name}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("invalid value '{name}' for '--output <FORMAT>'")),
            "{name}: {stderr}"
        );
        assert!(stderr.contains(instead), "{name}: {stderr}");
    }
    for (name, instead) in [
        ("human", r#""human" is now "text""#),
        ("agent", r#""agent" is now "json" at -v"#),
    ] {
        fs::write(
            dir.path().join("pyproject.toml"),
            format!("[project]\nname = \"p\"\n\n[tool.aye]\noutput = \"{name}\"\n"),
        )
        .unwrap();
        for output in [aye(&python, dir.path(), &[]), config(&python, dir.path())] {
            assert_eq!(code(&output), 4, "{name}");
            assert_eq!(stdout(&output), "", "{name}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(instead), "{name}: {stderr}");
        }
    }
    assert!(!dir.path().join("ran").exists());
    // The new names, from the file too, in any case.
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\noutput = \"JSON\"\n",
    )
    .unwrap();
    let (output, output_log) = logged(&python, dir.path(), &["-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(finished(&output_log).len(), 1);
    // The command line's wins.
    let output = aye(
        &python,
        dir.path(),
        &["--output", "text", "--color", "never"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert!(summary(&output) == "✓ 1 passed", "{}", stdout(&output));
}

/// `aye config` shows the output format at its verbosity, and where the
/// format came from.
#[test]
fn aye_config_shows_the_output() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING)]);
    let shown = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_aye"))
            .arg("config")
            .arg("--python")
            .arg(&python)
            .args(args)
            .current_dir(dir.path())
            .env_remove("PYTEST_ADDOPTS")
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
    let output = config(&python, dir.path());
    assert!(
        stdout(&output).contains("\n  output = json    # pyproject.toml [tool.aye]\n"),
        "{}",
        stdout(&output)
    );
}

/// A project with a test of each kind: one that passes, one that fails
/// (with output and a comparison) and one that skips.
const MIXED: &str = "import pytest\n\n\ndef test_pass():\n    pass\n\n\ndef test_fail():\n    print('some output')\n    assert [1, 2, 3] == [1, 2, 4]\n\n\n@pytest.mark.skip(reason='later')\ndef test_skip():\n    pass\n";

/// The JSON lines that name `MIXED`'s test `name`.
fn naming<'a>(lines: &'a [Value], name: &str) -> Vec<&'a Value> {
    let id = format!("tests/test_m.py::{name}");
    lines
        .iter()
        .filter(|line| line["id"] == id.as_str())
        .collect()
}

/// The keys of `MIXED`'s test `name`.
fn key_of(name: &str) -> String {
    aye_events::TestKey::of(&format!("tests/test_m.py::{name}")).to_string()[..5].to_owned()
}

/// JSON lines without what changes from run to run: durations, the run's
/// number and the test's history.
fn timeless(lines: &[Value]) -> Vec<Value> {
    lines
        .iter()
        .map(|line| {
            let mut line = line.clone();
            if let Some(object) = line.as_object_mut() {
                object.remove("s");
                object.remove("ms");
                object.remove("run");
                object.remove("history");
            }
            line
        })
        .collect()
}

/// `--output json` at each verbosity: the summary alone (`-q`), with the
/// keys of what failed (the default), a line per bad outcome (`-v`), the
/// detail of each (`-vv`), everything about each (`-vvv`). The verbosity
/// is `-v`s less `-q`s, from -1 to 3. The run's log is `aye show --log`.
#[test]
fn json_output_follows_the_verbosity() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_m.py", MIXED)]);
    let lines = |args: &[&str]| {
        let output = aye(&python, dir.path(), &[&["--output", "json"], args].concat());
        assert_eq!(code(&output), 1, "{args:?}: {}", stdout(&output));
        records(&output)
    };
    let key = key_of("test_fail");

    let quiet = lines(&["-q"]);
    assert_eq!(quiet.len(), 1, "{quiet:#?}");
    assert_eq!(quiet[0]["type"], "summary", "{quiet:#?}");
    assert_eq!(quiet[0]["exit"], 1, "{quiet:#?}");
    assert_eq!(quiet[0]["tests"], 3, "{quiet:#?}");
    assert_eq!(quiet[0]["failed"], 1, "{quiet:#?}");
    assert_eq!(quiet[0]["passed"], 1, "{quiet:#?}");
    assert_eq!(quiet[0]["skipped"], 1, "{quiet:#?}");
    assert!(quiet[0].get("failures").is_none(), "{quiet:#?}");
    assert_eq!(lines(&["-qq"]).len(), 1);

    let summary = lines(&[]);
    assert_eq!(summary.len(), 1, "{summary:#?}");
    assert_eq!(summary[0]["failures"], json!([key]), "{summary:#?}");
    assert_eq!(summary[0]["more"], "aye show <key> --output json");
    assert_eq!(timeless(&lines(&["-v", "-q"])), timeless(&summary));

    let line = lines(&["-v"]);
    let fail = naming(&line, "test_fail");
    assert_eq!(fail.len(), 1, "{line:#?}");
    assert_eq!(fail[0]["type"], "fail", "{line:#?}");
    assert_eq!(fail[0]["key"], key.as_str(), "{line:#?}");
    assert_eq!(fail[0]["at"], "tests/test_m.py:10", "{line:#?}");
    assert!(fail[0]["why"].is_string(), "{line:#?}");
    for field in ["code", "diff", "frames", "output", "rerun"] {
        assert!(fail[0].get(field).is_none(), "{field}: {line:#?}");
    }
    assert!(naming(&line, "test_pass").is_empty(), "{line:#?}");
    assert!(naming(&line, "test_skip").is_empty(), "{line:#?}");
    assert_eq!(line.last().unwrap()["type"], "summary");

    let detail = lines(&["-vv"]);
    let fail = naming(&detail, "test_fail");
    assert_eq!(
        fail[0]["code"], "    assert [1, 2, 3] == [1, 2, 4]",
        "{detail:#?}"
    );
    assert!(fail[0].get("diff").is_some(), "{detail:#?}");
    assert_eq!(
        fail[0]["rerun"], "aye tests/test_m.py::test_fail",
        "{detail:#?}"
    );
    assert!(fail[0].get("output").is_none(), "{detail:#?}");

    let all = lines(&["-vvv"]);
    let fail = naming(&all, "test_fail");
    assert!(fail[0]["frames"].is_array(), "{all:#?}");
    assert_eq!(fail[0]["output"]["stdout"], "some output", "{all:#?}");
    // Never the log itself: every line typed.
    assert!(all.iter().all(|line| line["type"].is_string()), "{all:#?}");
    assert_eq!(timeless(&lines(&["-vvvvv"])), timeless(&all));
}

/// Text output at each verbosity: the summary alone (`-q`, error
/// diagnostics aside), a line per bad outcome (the default), its detail
/// (`-v`), everything and a line per test (`-vv`), with the engine's
/// detail (`-vvv`).
#[test]
fn text_output_follows_the_verbosity() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_m.py", MIXED)]);
    let text = |args: &[&str]| {
        let output = aye(&python, dir.path(), &[&["--color", "never"], args].concat());
        assert_eq!(code(&output), 1, "{args:?}: {}", stdout(&output));
        stdout(&output)
    };
    let mentions = |text: &str, name: &str| text.lines().any(|line| line.contains(name));
    let key = key_of("test_fail");

    let quiet = text(&["-q"]);
    let lines: Vec<&str> = quiet.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1, "{quiet}");
    assert!(
        lines[0].starts_with("✗ 1 failed · ✓ 1 passed · 1 skipped   "),
        "{quiet}"
    );
    assert_eq!(text(&["-qq"]).lines().count(), quiet.lines().count());

    let normal = text(&[]);
    assert!(
        normal.contains(&format!("\n✗ {key}  tests/test_m.py::test_fail\n")),
        "{normal}"
    );
    assert!(normal.contains("\nskipped  later (1)\n"), "{normal}");
    assert!(
        normal.ends_with(&format!("\ndetails  aye show {key}\n")),
        "{normal}"
    );
    assert!(!mentions(&normal, "test_pass"), "{normal}");
    assert!(!mentions(&normal, "test_skip"), "{normal}");
    assert_eq!(text(&["-v", "-q"]).lines().count(), normal.lines().count());

    let verbose = text(&["-v"]);
    assert!(
        verbose.contains("\n  rerun    aye tests/test_m.py::test_fail\n"),
        "{verbose}"
    );
    // The code around the failing line prints it; the captured output isn't.
    assert!(!verbose.contains("\n  stdout   "), "{verbose}");
    assert!(!mentions(&verbose, "test_pass"), "{verbose}");

    let more = text(&["-vv"]);
    assert!(mentions(&more, "test_pass"), "{more}");
    assert!(mentions(&more, "test_skip"), "{more}");
    assert!(more.contains("\n  stdout   some output\n"), "{more}");
    assert!(more.lines().count() > verbose.lines().count(), "{more}");

    let most = text(&["-vvv"]);
    assert!(
        most.lines().count() > more.lines().count(),
        "{more}\n---\n{most}"
    );
    assert_eq!(
        text(&["-vvvvv"]).lines().count(),
        most.lines().count(),
        "-vvvvv is -vvv"
    );

    // Error diagnostics are never hidden.
    let broken = project(&[
        ("tests/test_m.py", MIXED),
        ("tests/test_broken.py", "def test_b(:\n    pass\n"),
    ]);
    let output = aye(&python, broken.path(), &["--color", "never", "-q"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    let quiet = stdout(&output);
    assert!(
        quiet.contains("error[syntax-error] tests/test_broken.py:1:"),
        "{quiet}"
    );
    assert!(
        quiet.lines().last().unwrap().starts_with("interrupted · "),
        "{quiet}"
    );
}

/// The ids `--co --output json` lists, in order.
fn listed(output: &Output) -> Vec<String> {
    events(&as_records(output), "case_collected")
        .iter()
        .map(|e| e["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn the_frameworks_own_k_and_m_select_statically() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"-k 'not slow'\"\nmarkers = [\"db\"]\n",
        ),
        (
            "tests/test_a.py",
            "import pytest\n\ndef test_fast():\n    pass\n\ndef test_slow():\n    pass\n\n@pytest.mark.db\ndef test_db():\n    pass\n",
        ),
    ]);
    let run = |args: &[&str], addopts: Option<&str>| {
        let mut argv = vec!["--co", "--output", "json", "-vvv"];
        argv.extend(args);
        let mut command = aye_command(&python, dir.path(), &argv);
        if let Some(addopts) = addopts {
            command.env("PYTEST_ADDOPTS", addopts);
        }
        let output = command.output().unwrap();
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        let deselected = events(&as_records(&output), "collection_finished")[0]["deselected"]
            .as_u64()
            .unwrap();
        (listed(&output), deselected)
    };
    let a = |name: &str| format!("tests/test_a.py::{name}");
    // addopts' `-k` deselects statically, so `--co` and the counts match
    // pytest's.
    assert_eq!(run(&[], None), (vec![a("test_fast"), a("test_db")], 1));
    // `PYTEST_ADDOPTS` adds to addopts.
    assert_eq!(run(&[], Some("-m 'not db'")), (vec![a("test_fast")], 2));
    // The command line's `-k` wins over addopts'.
    assert_eq!(run(&["-k", "slow"], None), (vec![a("test_slow")], 2));
    // The arguments after `--` come last, so theirs wins over both.
    assert_eq!(
        run(&["-k", "slow", "--", "-k", "db"], None),
        (vec![a("test_db")], 2)
    );
    // And the tests run as pytest runs them.
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0);
    assert_eq!(finished(&output_log).len(), 2);
}

#[test]
fn collection_follows_the_order_of_the_arguments() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "import pytest\n\ndef test_a1():\n    pass\n\n@pytest.mark.parametrize('x', [1, 2])\ndef test_p(x):\n    pass\n\nclass TestK:\n    def test_k(self):\n        pass\n",
        ),
        ("tests/test_b.py", "def test_b1():\n    pass\n"),
        ("tests/sub/test_c.py", "def test_c1():\n    pass\n"),
        ("tests/pkg/__init__.py", ""),
        ("tests/pkg/test_d.py", "def test_d():\n    pass\n"),
    ]);
    // Each list is what pytest 9.1.1's `--co` gives for the same arguments.
    let all_a = [
        "tests/test_a.py::test_a1",
        "tests/test_a.py::test_p[1]",
        "tests/test_a.py::test_p[2]",
        "tests/test_a.py::TestK::test_k",
    ];
    for (args, want) in [
        (
            vec!["tests/test_a.py::TestK::test_k", "tests/test_a.py::test_a1"],
            vec!["tests/test_a.py::TestK::test_k", "tests/test_a.py::test_a1"],
        ),
        (
            vec!["tests/test_a.py::test_p[2]", "tests/test_a.py::test_p[1]"],
            vec!["tests/test_a.py::test_p[2]", "tests/test_a.py::test_p[1]"],
        ),
        (
            vec![
                "tests/test_a.py::test_a1",
                "tests/test_b.py",
                "tests/test_a.py",
            ],
            [&["tests/test_b.py::test_b1"][..], &all_a].concat(),
        ),
        (
            vec!["tests/sub", "tests"],
            [
                &[
                    "tests/pkg/test_d.py::test_d",
                    "tests/sub/test_c.py::test_c1",
                ][..],
                &all_a,
                &["tests/test_b.py::test_b1"],
            ]
            .concat(),
        ),
        (
            vec![
                "tests/test_a.py::TestK",
                "tests/test_b.py",
                "tests/test_a.py::test_a1",
            ],
            vec![
                "tests/test_a.py::TestK::test_k",
                "tests/test_b.py::test_b1",
                "tests/test_a.py::test_a1",
            ],
        ),
    ] {
        let mut argv = vec!["--co", "--output", "json", "-vvv"];
        argv.extend(&args);
        let output = aye(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}");
        assert_eq!(listed(&output), want, "{args:?}");
    }
}

#[test]
fn a_runs_workers_share_one_testrun_uid() {
    let Some(python) = python() else { return };
    let writes = "import os\n\ndef test_uid():\n    with open(os.path.join(os.path.dirname(__file__), '..', 'uid-' + __name__), 'w') as f:\n        f.write(os.environ['PYTEST_XDIST_TESTRUNUID'])\n";
    let dir = project(&[("tests/test_a.py", writes), ("tests/test_b.py", writes)]);
    let uids = || {
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "-n", "2"],
        );
        assert_eq!(code(&output), 0, "{}", stdout(&output));
        let workers = events(&output_log, "worker_started").len();
        assert_eq!(workers, 2);
        ["uid-test_a", "uid-test_b"].map(|name| fs::read_to_string(dir.path().join(name)).unwrap())
    };
    let [a, b] = uids();
    assert_ne!(a, "");
    assert_eq!(a, b);
    // One per run.
    assert_ne!(uids()[0], a);
}

#[test]
fn refinement_is_cached_for_the_interpreter_and_the_framework_args() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_p.py",
        "import pytest\n\ndef cases():\n    return [1, 2]\n\n@pytest.mark.parametrize('x', cases())\ndef test_p(x):\n    pass\n",
    )]);
    let ids = |args: &[&str]| {
        let mut argv = vec!["--co", "--output", "json", "-vvv"];
        argv.extend(args);
        let output = aye(&python, dir.path(), &argv);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        listed(&output)
    };
    let refined = ["tests/test_p.py::test_p[1]", "tests/test_p.py::test_p[2]"];
    // Without refining, the static guess holds one case for every parameter.
    assert_eq!(ids(&["--no-refine"]), ["tests/test_p.py::test_p"]);
    assert_eq!(ids(&[]), refined);
    let entries = fs::read_dir(dir.path().join(".aye/cache/refine/pytest"))
        .unwrap()
        .count();
    assert_eq!(entries, 1);
    // A hit needs no worker, so it answers even without refining.
    assert_eq!(ids(&["--no-refine"]), refined);
    // The arguments after `--` are part of the key.
    assert_eq!(
        ids(&["--no-refine", "--", "--tb=short"]),
        ["tests/test_p.py::test_p"]
    );
}

#[cfg(unix)]
#[test]
fn paths_through_another_name_of_the_project_select_its_tests() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING), ("tests/test_b.py", PASSING)]);
    let real = dir.path().canonicalize().unwrap();
    let link = tempfile::tempdir().unwrap();
    let alias = link.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let through = alias.join("tests/test_a.py");
    let through = through.to_str().unwrap();
    // The current directory reads as the real path (as `/tmp` does as
    // `/private/tmp` on macOS); one argument names the other.
    for args in [
        vec![format!("{through}::test_a"), "tests/test_b.py".to_string()],
        vec![format!("{through}::test_a")],
        vec![through.to_string(), "tests/test_b.py::test_a".to_string()],
    ] {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args.iter().map(String::as_str));
        let (output, output_log) = logged(&python, &real, &argv);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        assert_eq!(finished(&output_log).len(), args.len(), "{args:?}");
    }
}

#[test]
fn the_frameworks_junit_report_is_aye_s_whole_report() {
    let Some(python) = python() else { return };
    let mut files = vec![(
        "pyproject.toml",
        "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--junitxml=reports/out.xml --junitprefix=ci\"\n",
    )];
    let two = "def test_a():\n    pass\n\ndef test_b():\n    pass\n";
    let names: Vec<String> = (1..=6).map(|i| format!("tests/test_t{i}.py")).collect();
    files.extend(names.iter().map(|name| (name.as_str(), two)));
    let dir = project(&files);
    let testcases = |path: &str| {
        let xml = fs::read_to_string(dir.path().join(path)).unwrap();
        assert!(xml.contains(r#"<testsuites name="aye""#), "{xml}");
        xml.matches("<testcase ").count()
    };
    // Each batch's pytest session would have written its own over the
    // others'; aye writes the one report, with every test.
    let output = aye(
        &python,
        dir.path(),
        &["-n", "2", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(testcases("reports/out.xml"), 12);
    let xml = fs::read_to_string(dir.path().join("reports/out.xml")).unwrap();
    assert!(xml.contains(r#"classname="ci.tests.test_t1""#), "{xml}");
    // From the arguments after `--`, or `PYTEST_ADDOPTS`, too.
    assert_eq!(
        code(&aye(
            &python,
            dir.path(),
            &["-n", "2", "--", "--junitxml=other.xml"]
        )),
        0
    );
    assert_eq!(testcases("other.xml"), 12);
    let output = aye_command(&python, dir.path(), &["-n", "2"])
        .env("PYTEST_ADDOPTS", "--junit-xml=env.xml")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0);
    assert_eq!(testcases("env.xml"), 12);
    // The command line's own wins.
    assert_eq!(
        code(&aye(&python, dir.path(), &["--junitxml", "mine.xml"])),
        0
    );
    assert_eq!(testcases("mine.xml"), 12);
}

/// `aye config`, run in `cwd` with `python`.
fn config(python: &Path, cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aye"))
        .arg("config")
        .arg("--python")
        .arg(python)
        .current_dir(cwd)
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap()
}

/// What `aye config` says about the adapters: their names and why.
#[cfg(unix)]
fn adapters_line(python: &Path, cwd: &Path) -> String {
    let output = config(python, cwd);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    stdout(&output)
        .lines()
        .find_map(|line| line.strip_prefix("adapters "))
        .unwrap_or_else(|| panic!("no adapters line: {}", stdout(&output)))
        .trim()
        .to_string()
}

/// A venv of `python`'s base interpreter without pytest, laid out as
/// `python -m venv` lays one out (an interpreter beside a `pyvenv.cfg`
/// leaving out the system's packages, and an empty `site-packages`), in
/// `dir`. `None` when the base interpreter itself has pytest.
#[cfg(unix)]
#[expect(clippy::print_stderr, reason = "a test says why it skips")]
fn venv_without_pytest(python: &Path, dir: &Path) -> Option<PathBuf> {
    let probe = Command::new(python)
        .args([
            "-c",
            "import sys; print(sys._base_executable); print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8(probe.stdout).unwrap();
    let mut lines = text.lines();
    let base = PathBuf::from(lines.next()?);
    let version = lines.next()?;
    let venv = dir.join("venv");
    fs::create_dir_all(venv.join("bin")).unwrap();
    fs::create_dir_all(venv.join(format!("lib/python{version}/site-packages"))).unwrap();
    fs::write(
        venv.join("pyvenv.cfg"),
        format!(
            "home = {}\ninclude-system-site-packages = false\nversion = {version}\n",
            base.parent()?.display()
        ),
    )
    .unwrap();
    let interpreter = venv.join("bin/python");
    std::os::unix::fs::symlink(&base, &interpreter).unwrap();
    let has_pytest = Command::new(&interpreter)
        .args(["-c", "import pytest"])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if has_pytest {
        eprintln!(
            "skipping: the base interpreter {} has pytest",
            base.display()
        );
        return None;
    }
    Some(interpreter)
}

/// Without `[tool.aye] adapters`, the default is doctest then pytest, for
/// every project, whatever the interpreter installs: a project of
/// `TestCase` classes with no pytest configuration and no conftest.py, run
/// with an interpreter that has no pytest, runs them by pytest's rules on
/// aye's engine; `[tool.aye] adapters = ["unittest"]` gives them to
/// unittest's discovery.
#[cfg(unix)]
#[test]
fn the_default_adapters_dont_depend_on_what_is_installed() {
    let Some(python) = python() else { return };
    let venvs = tempfile::tempdir().unwrap();
    let Some(bare) = venv_without_pytest(&python, venvs.path()) else {
        return;
    };
    let dir = project(&[
        ("tests/__init__.py", ""),
        (
            "tests/test_shop.py",
            "import unittest\n\n\nclass CartTests(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(1 + 1, 2)\n\n    def test_total(self):\n        self.assertEqual(sum([1, 2]), 4)\n",
        ),
    ]);
    let want = [
        (
            "tests/test_shop.py::CartTests::test_add".to_owned(),
            "passed".to_owned(),
        ),
        (
            "tests/test_shop.py::CartTests::test_total".to_owned(),
            "failed".to_owned(),
        ),
    ];
    for interpreter in [&bare, &python] {
        assert_eq!(
            adapters_line(interpreter, dir.path()),
            "doctest, pytest  (default: pytest's rules, which run unittest's TestCase classes too)"
        );
        let (output, output_log) = logged(interpreter, dir.path(), &["--output", "json", "-vvv"]);
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "case_collected")
                .iter()
                .all(|e| e["adapter"] == "pytest"),
            "{}",
            stdout(&output)
        );
        assert_eq!(finished(&records), want);
    }
    // Selection by dotted names.
    let (one, one_log) = logged(
        &bare,
        dir.path(),
        &[
            "--output",
            "json",
            "-vvv",
            "tests.test_shop.CartTests.test_add",
        ],
    );
    assert_eq!(code(&one), 0, "{}", stdout(&one));
    assert_eq!(finished(&one_log).len(), 1);
    // unittest's discovery, when the project asks for it.
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
    )
    .unwrap();
    assert_eq!(
        adapters_line(&bare, dir.path()),
        "unittest  (from [tool.aye] adapters)"
    );
    let (output, output_log) = logged(&bare, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let records = output_log;
    assert!(
        events(&records, "case_collected")
            .iter()
            .all(|e| e["adapter"] == "unittest")
    );
    assert_eq!(finished(&records), want);
}

/// With `["unittest", "pytest"]`, unittest runs the `TestCase` methods both
/// collect, without a duplicate-id warning, and pytest the rest.
#[test]
fn a_higher_priority_adapter_takes_the_tests_both_collect_quietly() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\"]\n",
        ),
        (
            "test_mixed.py",
            "import unittest\n\n\nclass T(unittest.TestCase):\n    def test_case(self):\n        pass\n\n\ndef test_function():\n    pass\n",
        ),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let collected: Vec<(String, String)> = events(&records, "case_collected")
        .iter()
        .map(|e| {
            (
                e["id"].as_str().unwrap().to_owned(),
                e["adapter"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        collected,
        [
            (
                "test_mixed.py::T::test_case".to_owned(),
                "unittest".to_owned()
            ),
            (
                "test_mixed.py::test_function".to_owned(),
                "pytest".to_owned()
            ),
        ]
    );
    assert!(events(&records, "diagnostic").is_empty(), "{records:#?}");
}

/// Finding F32: a file holding a `TestCase` (the unittest adapter's) and a
/// test function (pytest's) that fails to import is one collection error,
/// as pytest, which collects the file once, reports it.
#[test]
fn a_file_two_adapters_load_fails_to_import_once() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\"]\n",
        ),
        (
            "test_imp.py",
            "import os\nimport unittest\n\nif not os.environ.get('NOPE'):\n    import nonexistent_zzz\n\n\nclass TestA(unittest.TestCase):\n    def test_a(self):\n        pass\n\n\ndef test_f():\n    pass\n",
        ),
    ]);
    let output = aye(&python, dir.path(), &["--output", "json", "-v"]);
    assert_eq!(code(&output), 2, "{}", stdout(&output));
    let errors = stdout(&output)
        .lines()
        .filter(|line| line.contains(r#""type":"collection_error""#))
        .count();
    assert_eq!(errors, 1, "{}", stdout(&output));
    let output = aye(
        &python,
        dir.path(),
        &["--continue-on-collection-errors", "--junitxml", "j.xml"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert!(
        summary(&output) == "! 1 collection error",
        "{}",
        stdout(&output)
    );
    let junit = fs::read_to_string(dir.path().join("j.xml")).unwrap();
    assert!(junit.contains(r#"errors="1""#), "{junit}");
    // The other adapter's error is still in the stream, as an info (its
    // batch runs past the error, which aye's pytest front-end met while it
    // listed the file).
    let (_output, output_log) = logged(
        &python,
        dir.path(),
        &[
            "--continue-on-collection-errors",
            "--output",
            "json",
            "-vvv",
        ],
    );
    let records = output_log;
    let severities: Vec<&str> = events(&records, "diagnostic")
        .iter()
        .filter(|d| d["code"] == "collection-error")
        .map(|d| d["severity"].as_str().unwrap())
        .collect();
    assert_eq!(severities, ["error", "info"]);
}

/// Finding F28: an `@file` after `--` hides what pytest's `-k` and `-m`
/// would be from aye, but no adapter applies an expression itself (aye's
/// engine runs pytest's tests too), so the command line's selects every
/// adapter's cases.
#[test]
fn an_argument_file_leaves_the_command_lines_k_to_other_adapters() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\", \"pytest\", \"doctest\"]\n\n[tool.aye.doctest]\nmodules = true\n",
        ),
        (
            "test_a.py",
            "import unittest\n\n\nclass TestU(unittest.TestCase):\n    def test_one(self):\n        pass\n\n    def test_two(self):\n        pass\n\n\ndef test_func():\n    pass\n",
        ),
        (
            "lib.py",
            "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
        ),
        ("args.txt", "-v\n"),
    ]);
    for args in [
        &["-k", "one", "--output", "json", "-vvv", "--", "@args.txt"][..],
        &["-k", "one", "--output", "json", "-vvv"],
    ] {
        let (output, output_log) = logged(&python, dir.path(), args);
        assert_eq!(code(&output), 0, "{args:?}: {}", stdout(&output));
        assert_eq!(
            finished(&output_log),
            [(
                "test_a.py::TestU::test_one".to_string(),
                "passed".to_string()
            )],
            "{args:?}"
        );
    }
    let output = aye(
        &python,
        dir.path(),
        &["-m", "slow", "--output", "json", "-vvv", "--", "@args.txt"],
    );
    assert_eq!(code(&output), 5, "{}", stdout(&output));
}

/// Finding F7: a package's `load_tests` that discovers another pattern
/// makes tests in files aye's walk doesn't claim; they run at the
/// package's place, as unittest's discovery runs them.
#[test]
fn tests_a_packages_load_tests_finds_elsewhere_keep_the_packages_place() {
    let Some(python) = python() else { return };
    let case = |class: &str, method: &str| {
        format!(
            "import unittest\n\n\nclass {class}(unittest.TestCase):\n    def {method}(self):\n        pass\n"
        )
    };
    let (check, a, z) = (
        case("Check", "test_check"),
        case("A", "test_a"),
        case("Z", "test_z"),
    );
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
        ),
        (
            "pkg/__init__.py",
            "import os\n\n\ndef load_tests(loader, standard_tests, pattern):\n    here = os.path.dirname(__file__)\n    standard_tests.addTests(\n        loader.discover(start_dir=here, pattern='check_*.py', top_level_dir=os.path.dirname(here))\n    )\n    return standard_tests\n",
        ),
        ("pkg/check_one.py", check.as_str()),
        ("test_a.py", a.as_str()),
        ("test_z.py", z.as_str()),
    ]);
    let order = [
        "pkg/check_one.py::Check::test_check",
        "test_a.py::A::test_a",
        "test_z.py::Z::test_z",
    ];
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--co", "--output", "json", "-vvv", "-v"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let listed = output_log;
    let collected: Vec<&str> = events(&listed, "case_collected")
        .iter()
        .map(|e| e["id"].as_str().unwrap())
        .collect();
    assert_eq!(collected, order);
    // The package's `load_tests` made a test: it has one.
    assert!(
        events(&listed, "diagnostic")
            .iter()
            .all(|d| d["code"] != "no-tests"),
        "{listed:#?}"
    );
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let ran: Vec<String> = finished(&output_log)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(ran, order);
}

/// No pytest session runs, so nothing needs one: an option of a plugin
/// that writes its data once per pytest session (pytest-cov's `--cov`)
/// neither pins the run to one worker nor takes pytest's doctests from the
/// doctest adapter, as `aye config` shows. (aye's engine loads no
/// third-party plugin: what the option does in a run is the front-end's.)
#[test]
fn session_plugins_options_pin_nothing() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules --cov=pkg\"\n",
        ),
        (
            "pkg/__init__.py",
            "def f():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n",
        ),
        ("test_a.py", PASSING),
        ("test_b.py", PASSING),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_aye"))
        .args(["config", "-n", "2", "--python"])
        .arg(&python)
        .current_dir(dir.path())
        .env_remove("PYTEST_ADDOPTS")
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let text = stdout(&output);
    assert!(text.contains("\nworkers  2\n"), "{text}");
    assert!(
        text.contains(
            "\nadapters doctest, pytest  (default: pytest's rules, which run unittest's \
             TestCase classes too)\n"
        ),
        "{text}"
    );
}

/// pytest's doctests, run by the doctest adapter beside pytest by default:
/// a package's `__init__.py` comes first in its directory, a file's
/// doctests before its tests, dotted names select doctests, and modules
/// without doctests are no files without tests.
#[test]
fn doctests_run_beside_pytest_in_pytests_order() {
    let Some(python) = python() else { return };
    let doc = |name: &str| format!("def {name}():\n    \"\"\"\n    >>> 1 + 1\n    2\n    \"\"\"\n");
    let init = doc("f");
    let alpha = format!("{}\ndef test_alpha():\n    pass\n", doc("g"));
    let beta = doc("h");
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\npython_files = [\"*.py\"]\n",
        ),
        ("pkg/__init__.py", init.as_str()),
        ("pkg/Alpha.py", alpha.as_str()),
        ("pkg/beta.py", beta.as_str()),
        ("pkg/plain.py", "X = 1\n"),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--co", "--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    // pytest 9.1.1's `--co -q`, in its order.
    assert_eq!(
        listed(&output),
        [
            "pkg/__init__.py::pkg.f",
            "pkg/Alpha.py::pkg.Alpha.g",
            "pkg/Alpha.py::test_alpha",
            "pkg/beta.py::pkg.beta.h",
        ]
    );
    let collected = output_log;
    assert!(
        events(&collected, "diagnostic").is_empty(),
        "{:#?}",
        events(&collected, "diagnostic")
    );
    for (selector, ids) in [
        ("pkg.beta.h", vec!["pkg/beta.py::pkg.beta.h"]),
        (
            "pkg.Alpha",
            vec!["pkg/Alpha.py::pkg.Alpha.g", "pkg/Alpha.py::test_alpha"],
        ),
        ("pkg.f", vec!["pkg/__init__.py::pkg.f"]),
    ] {
        let (output, output_log) =
            logged(&python, dir.path(), &["--output", "json", "-vvv", selector]);
        assert_eq!(code(&output), 0, "{selector}: {}", stdout(&output));
        let ran: Vec<String> = finished(&output_log)
            .into_iter()
            .map(|(id, outcome)| {
                assert_eq!(outcome, "passed", "{id}");
                id
            })
            .collect();
        assert_eq!(ran, ids, "{selector}");
    }
}

/// With one worker, a run goes in plan order across adapters, as pytest
/// runs a file's doctests before its tests.
#[test]
fn one_worker_runs_the_adapters_batches_in_plan_order() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n",
        ),
        (
            "test_alpha.py",
            "def g():\n    \"\"\"\n    >>> 1 + 1\n    2\n    \"\"\"\n\ndef test_alpha():\n    pass\n",
        ),
    ]);
    let (output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let ran: Vec<String> = finished(&records).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ran,
        ["test_alpha.py::test_alpha.g", "test_alpha.py::test_alpha"]
    );
    // The worker that listed pytest's session waits for the doctests'
    // (it lists once), and each worker's events come between its start and
    // its end.
    let lifecycle: Vec<String> = records
        .iter()
        .filter_map(|r| match r["event"].as_str()? {
            "batch_started" => Some(format!("batch {}", r["adapter"].as_str()?)),
            "worker_started" | "worker_finished" => Some(r["event"].as_str()?.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle,
        [
            "worker_started",
            "batch doctest",
            "worker_finished",
            "worker_started",
            "batch pytest",
            "worker_finished"
        ],
        "{}",
        stdout(&output)
    );
}

/// pytest counts a collection error as a failure: with
/// `--continue-on-collection-errors` the run goes on but exits 1, also
/// when nothing else runs, and under `--co`.
#[test]
fn collection_errors_fail_a_run_that_continues_past_them() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/test_b.py", "def test_b(:\n    pass\n"),
    ]);
    for (args, want) in [
        (vec![], 2),
        (vec!["--continue-on-collection-errors"], 1),
        (vec!["--continue-on-collection-errors", "--co"], 1),
        (
            vec!["--continue-on-collection-errors", "tests/test_b.py"],
            1,
        ),
        (vec!["--co"], 2),
    ] {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), want, "{args:?}: {}", stdout(&output));
        assert_eq!(output_log.last().unwrap()["exit_code"], want);
    }
}

/// Finding C27: pytest fails the collection of a directory it can't read,
/// and so the run (exit 2); aye did run the rest and exit 0.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_a_collection_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", PASSING),
        ("tests/locked/test_l.py", PASSING),
    ]);
    let locked = dir.path().join("tests/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&locked).is_ok();
    let run = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args);
        logged(&python, dir.path(), &argv)
    };
    let outputs = [
        run(&[]),
        run(&["--continue-on-collection-errors"]),
        run(&["--co"]),
    ];
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    if readable {
        eprintln!("skipping: permissions aren't enforced here (root?)");
        return;
    }
    for ((output, records), want) in outputs.iter().zip([2, 1, 2]) {
        assert_eq!(code(output), want, "{}", stdout(output));
        let errors: Vec<_> = events(records, "diagnostic")
            .into_iter()
            .filter(|d| d["severity"] == "error")
            .collect();
        assert_eq!(errors.len(), 1, "{errors:#?}");
        assert_eq!(errors[0]["code"], "unreadable");
        assert_eq!(errors[0]["path"], "tests/locked");
    }
    assert_eq!(finished(&outputs[1].1).len(), 1);
}

/// Round 3's #81: from `root/tests`, holding a `tests/test_a.py` of its
/// own, the printed rerun `aye tests/test_a.py::test_fail` ran that other
/// file. It names the test from where aye ran.
#[test]
fn a_printed_rerun_names_the_test_from_where_aye_ran() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("tests/test_a.py", "def test_fail():\n    assert 0\n"),
        ("tests/tests/test_a.py", "def test_fail():\n    pass\n"),
    ]);
    let tests = dir.path().join("tests");
    let first = aye(&python, &tests, &["--color", "never", "-v", "test_a.py"]);
    assert_eq!(code(&first), 1, "{}", stdout(&first));
    let rerun = stdout(&first)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .map(str::to_owned)
        .unwrap();
    assert_eq!(rerun, "test_a.py::test_fail");
    let (again, again_log) = logged(&python, &tests, &["--output", "json", "-vvv", &rerun]);
    assert_eq!(code(&again), 1, "{}", stdout(&again));
    assert_eq!(
        finished(&again_log),
        [("tests/test_a.py::test_fail".to_owned(), "failed".to_owned())]
    );
    // The stream says so too, for every renderer.
    let (_json, json_log) = logged(&python, &tests, &["--output", "json", "-vvv", "test_a.py"]);
    let records = json_log;
    let result = events(&records, "test_finished")[0];
    assert_eq!(result["reproduce"]["command"][1], "test_a.py::test_fail");
    // From the root the rerun is the id.
    let root = aye(
        &python,
        dir.path(),
        &["--color", "never", "-v", "tests/test_a.py"],
    );
    assert!(
        stdout(&root).contains("rerun    aye tests/test_a.py::test_fail"),
        "{}",
        stdout(&root)
    );
}

/// pytest's `--deselect`, from `addopts`, `PYTEST_ADDOPTS` or the arguments
/// after `--`, drops the tests it names (by id prefix, as a string) and
/// counts them as deselected, as pytest's summary does.
#[test]
fn the_frameworks_deselect_is_counted() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\naddopts = \"--deselect tests/test_a.py::test_b\"\n",
        ),
        (
            "tests/test_a.py",
            "def test_pass():\n    pass\n\ndef test_b():\n    pass\n\ndef test_b2():\n    pass\n",
        ),
        ("tests/test_o.py", PASSING),
    ]);
    let run = |args: &[&str], env: Option<&str>| {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args);
        let mut command = aye_command(&python, dir.path(), &argv);
        if let Some(env) = env {
            command.env("PYTEST_ADDOPTS", env);
        }
        let before = runs_in(dir.path());
        let output = command.output().unwrap();
        let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
        let finished = records.last().unwrap();
        (
            code(&output),
            finished["counts"]["passed"].as_u64().unwrap_or(0),
            finished["counts"]["deselected"].as_u64().unwrap_or(0),
        )
    };
    // pytest 9.1.1: "2 passed, 2 deselected" (`test_b` also drops
    // `test_b2`), "1 passed, 3 deselected" twice, then "1 deselected" (the
    // argument names `test_b` alone) and exit 5.
    assert_eq!(run(&[], None), (0, 2, 2));
    assert_eq!(run(&[], Some("--deselect tests/test_o.py")), (0, 1, 3));
    assert_eq!(
        run(&["--", "--deselect", "tests/test_a.py::test_pass"], None),
        (0, 1, 3)
    );
    assert_eq!(run(&["tests/test_a.py::test_b"], None), (5, 0, 1));
}

/// Findings C23 and C31: under `--lf`, pytest's summary counts as
/// deselected only what its last-failed plugin let collection keep.
#[test]
fn last_failed_counts_deselected_as_pytest_does() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/test_a.py",
            "def test_a1():\n    assert 0\n\ndef test_a2():\n    pass\n",
        ),
        (
            "tests/test_b.py",
            "def test_b1():\n    pass\n\ndef test_b2():\n    pass\n",
        ),
    ]);
    assert_eq!(code(&aye(&python, dir.path(), &[])), 1);
    // pytest 9.1.1 (cache on), after the same first run.
    for (args, deselected) in [
        (vec![], 0),
        (vec!["tests"], 0),
        (vec!["tests/test_a.py", "tests/test_b.py"], 1),
        (vec!["tests/test_a.py"], 1),
    ] {
        let mut argv = vec!["--lf", "--output", "json", "-vvv"];
        argv.extend(&args);
        let (output, output_log) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 1, "{args:?}");
        let records = output_log;
        assert_eq!(finished(&records).len(), 1, "{args:?}");
        assert_eq!(
            records.last().unwrap()["counts"]["deselected"],
            deselected,
            "{args:?}"
        );
    }
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
        assert_eq!(code(&config(&python, dir.path())), 4, "{table}");
    }
}

/// The `rerun` row of text output at `-v`, split into arguments (none of
/// the tests' have spaces).
fn printed_rerun(output: &Output) -> Vec<String> {
    stdout(output)
        .lines()
        .find_map(|line| line.trim().strip_prefix("rerun    aye "))
        .unwrap_or_else(|| panic!("no rerun in:\n{}", stdout(output)))
        .split(' ')
        .map(str::to_owned)
        .collect()
}

/// Finding C26: aye's own `-c FILE` shapes the run (here, which tests
/// exist), so a printed rerun repeats it, as given, from where aye ran.
#[test]
fn a_printed_rerun_keeps_the_config_file() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("alt.ini", "[pytest]\npython_functions = check_*\n"),
        ("tests/test_a.py", "def check_a():\n    assert 0\n"),
    ]);
    for (cwd, flag, file) in [
        (dir.path().to_path_buf(), "-c", "alt.ini"),
        (dir.path().to_path_buf(), "--config-file", "alt.ini"),
        (dir.path().join("tests"), "-c", "../alt.ini"),
    ] {
        let first = aye(&python, &cwd, &["--color", "never", "-v", flag, file]);
        assert_eq!(code(&first), 1, "{}", stdout(&first));
        let rerun = printed_rerun(&first);
        assert_eq!(rerun, ["-c", file, "tests/test_a.py::check_a"]);
        let args: Vec<&str> = rerun.iter().map(String::as_str).collect();
        let (again, again_log) = logged(
            &python,
            &cwd,
            &[&["--output", "json", "-vvv"], &args[..]].concat(),
        );
        assert_eq!(code(&again), 1, "{}", stdout(&again));
        assert_eq!(
            finished(&again_log),
            [("tests/test_a.py::check_a".to_owned(), "failed".to_owned())]
        );
    }
    // With arguments after `--` too: aye's options, the id, then those.
    let (_json, json_log) = logged(
        &python,
        dir.path(),
        &["--output", "json", "-vvv", "-c", "alt.ini", "--", "-v"],
    );
    let records = json_log;
    assert_eq!(
        events(&records, "test_finished")[0]["reproduce"]["command"],
        serde_json::json!([
            "aye",
            "-c",
            "alt.ini",
            "tests/test_a.py::check_a",
            "--",
            "-v"
        ])
    );
}

/// Finding C24: what tests record with pytest's `record_property` and
/// `record_testsuite_property` reaches aye's JUnit report, as it reaches
/// pytest's: each run property once, however many batches' sessions
/// recorded it.
#[test]
fn what_tests_record_reaches_the_junit_report() {
    let Some(python) = python() else { return };
    let test = |name: &str| {
        format!(
            "def test_{name}(record_property, record_testsuite_property):\n    record_property(\"example_key\", \"{name}\")\n    record_testsuite_property(\"ARCH\", \"PPC\")\n"
        )
    };
    let (a, b) = (test("a"), test("b"));
    let dir = project(&[("tests/test_a.py", &a), ("tests/test_b.py", &b)]);
    let output = aye(&python, dir.path(), &["-n", "2", "--junitxml", "out.xml"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let xml = fs::read_to_string(dir.path().join("out.xml")).unwrap();
    assert_eq!(
        xml.matches(r#"<property name="ARCH" value="PPC"/>"#)
            .count(),
        1,
        "{xml}"
    );
    let case = |name: &str| {
        let start = xml.find(&format!(r#"name="test_{name}""#)).unwrap();
        let end = start + xml[start..].find("</testcase>").unwrap();
        xml[start..end].to_owned()
    };
    for name in ["a", "b"] {
        assert!(
            case(name).contains(&format!(r#"<property name="example_key" value="{name}"/>"#)),
            "{xml}"
        );
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

/// Finding C22: a conftest or plugin that fails a session none of whose
/// tests failed (`session.exitstatus`, pytest-cov's `--cov-fail-under`)
/// fails the run, as pytest exits 1, however many workers run it.
#[test]
fn a_session_the_framework_failed_fails_the_run() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "def pytest_sessionfinish(session, exitstatus):\n    session.exitstatus = 1\n",
        ),
        ("tests/test_a.py", PASSING),
        ("tests/test_b.py", PASSING),
    ]);
    for workers in ["1", "2"] {
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["-n", workers, "--output", "json", "-vvv"],
        );
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "diagnostic")
                .iter()
                .any(|d| d["code"] == "session-failed"),
            "{}",
            stdout(&output)
        );
        assert_eq!(events(&records, "run_finished")[0]["exit_code"], 1);
    }
}

/// A session-scoped fixture whose teardown fails: pytest tears the session
/// down in its last test's teardown, which errors (exit 1). With one worker
/// aye ends the session with the same test; with several, each worker's
/// session ends with its last batch, as under pytest-xdist, and the run
/// fails all the same.
#[test]
fn a_session_teardown_that_fails_is_the_last_tests_error() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "import pytest\n\n@pytest.fixture(scope='session')\ndef res():\n    yield 1\n    raise RuntimeError('broke')\n",
        ),
        (
            "tests/test_a.py",
            "def test_a1(res):\n    pass\n\ndef test_a2(res):\n    pass\n",
        ),
        ("tests/test_b.py", "def test_b(res):\n    pass\n"),
    ]);
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["-n", "1", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    assert_eq!(
        finished(&output_log),
        [
            ("tests/test_a.py::test_a1".to_string(), "passed".to_string()),
            ("tests/test_a.py::test_a2".to_string(), "passed".to_string()),
            ("tests/test_b.py::test_b".to_string(), "error".to_string()),
        ]
    );
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["-n", "2", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 1, "{}", stdout(&output));
    let records = output_log;
    let teardowns = events(&records, "lifetime_finished")
        .iter()
        .filter(|e| !e["failure"].is_null())
        .count()
        + finished(&records)
            .iter()
            .filter(|(_, outcome)| outcome == "error")
            .count();
    assert!(teardowns >= 1, "{}", stdout(&output));
}

/// A session's teardown runs to its end, however long it takes, in every
/// worker that set the session up: pytest waits for it, and the worker's
/// shutdown mustn't cut it short.
#[test]
fn a_session_teardown_runs_to_its_end_in_every_worker() {
    let Some(python) = python() else { return };
    let dir = project(&[
        (
            "tests/conftest.py",
            "import os, time\nimport pytest\n\n@pytest.fixture(scope='session')\ndef res():\n    worker = os.environ['AYE_WORKER']\n    open(f'up-{worker}', 'w').close()\n    yield 1\n    time.sleep(3.5)\n    open(f'done-{worker}', 'w').close()\n",
        ),
        ("tests/test_a.py", "def test_a(res):\n    pass\n"),
        ("tests/test_b.py", "def test_b(res):\n    pass\n"),
    ]);
    for workers in ["1", "2"] {
        let output = aye(
            &python,
            dir.path(),
            &["-n", workers, "--output", "json", "-vvv"],
        );
        assert_eq!(code(&output), 0, "{}", stdout(&output));
        let mut up = 0;
        for lane in 0..2 {
            if dir.path().join(format!("up-{lane}")).exists() {
                up += 1;
                assert!(
                    dir.path().join(format!("done-{lane}")).exists(),
                    "-n {workers}: worker {lane}'s session teardown was cut short"
                );
            }
            let _ = fs::remove_file(dir.path().join(format!("up-{lane}")));
            let _ = fs::remove_file(dir.path().join(format!("done-{lane}")));
        }
        assert!(up >= 1, "-n {workers}: {}", stdout(&output));
    }
}

const PYTEST_CONFIG: &str = "[tool.pytest.ini_options]\n";
const DOCTESTS_ON: &str = "[tool.pytest.ini_options]\naddopts = \"--doctest-modules\"\n";
const DOCTEST: &str = "def g():\n    \"\"\"\n    >>> 1\n    1\n    \"\"\"\n";
const SKIPS: &str = "import pytest\npytest.importorskip(\"aye_missing_zz\")\n";

/// The `(severity, path)` of each diagnostic with `code`.
fn diagnosed(records: &[Value], code: &str) -> Vec<(String, String)> {
    events(records, "diagnostic")
        .iter()
        .filter(|d| d["code"] == code)
        .map(|d| {
            (
                d["severity"].as_str().unwrap().to_owned(),
                d["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// The text summary line, without its duration: the last line but the
/// details hint.
fn summary(output: &Output) -> String {
    stdout(output)
        .lines()
        .rfind(|line| !line.is_empty() && !line.starts_with("details  "))
        .unwrap_or_default()
        .rsplit_once("   ")
        .map_or("", |(counts, _)| counts)
        .to_owned()
}

/// Recheck regression 2: a dotted name or a key selects among what
/// collection finds in the whole project, but the run starts from the
/// files they select, as `pytest <those files>` does: a conftest that
/// pytest's default start would load first (a `test*` directory's) and
/// that `pytest tests/test_a.py` never loads changes nothing. Before, it
/// was a usage error (exit 4), or its hook skipped the selected test.
#[test]
fn dotted_names_and_keys_start_the_run_from_the_files_they_select() {
    let Some(python) = python() else { return };
    let hook = "import pytest\n\ndef pytest_collection_modifyitems(items):\n    for item in items:\n        item.add_marker(pytest.mark.skip(reason=\"integration only\"))\n";
    for conftest in [SKIPS, "import aye_missing_zz\n", hook] {
        let dir = project(&[
            ("pyproject.toml", PYTEST_CONFIG),
            ("tests/__init__.py", ""),
            ("tests/test_a.py", "def test_a():\n    pass\n"),
            ("testing/conftest.py", conftest),
            ("testing/test_g.py", "def test_g():\n    pass\n"),
        ]);
        let key = aye_events::TestId::new("tests/test_a.py::test_a")
            .key()
            .to_string();
        for arg in ["tests.test_a", key.as_str()] {
            let (output, output_log) =
                logged(&python, dir.path(), &["--output", "json", "-vvv", arg]);
            assert_eq!(code(&output), 0, "{arg}: {}", stdout(&output));
            let records = output_log;
            assert_eq!(
                finished(&records),
                [("tests/test_a.py::test_a".to_owned(), "passed".to_owned())],
                "{arg}: {records:#?}"
            );
            assert!(
                events(&records, "diagnostic")
                    .iter()
                    .all(|d| d["severity"] != "error"),
                "{arg}: {records:#?}"
            );
        }
        // The default start loads it, as pytest's does: a skip or an import
        // error there stops the run, a usage error (pytest crashes, or
        // exits 4).
        if conftest != hook {
            let output = aye(&python, dir.path(), &[]);
            assert_eq!(code(&output), 4, "{}", stdout(&output));
        }
        // A dotted name for a module that skips itself names no test, but
        // names something: no usage error, from it or from elsewhere.
        fs::create_dir_all(dir.path().join("other")).unwrap();
        fs::write(
            dir.path().join("other/test_mod.py"),
            "import pytest\npytest.skip('later', allow_module_level=True)\n",
        )
        .unwrap();
        let output = aye(&python, dir.path(), &["other.test_mod"]);
        assert_eq!(code(&output), 5, "{}", stdout(&output));
        assert_eq!(summary(&output), "no tests ran · 1 skipped");
    }
    // The doctest adapter's start-up conftests too: `pytest top.py` never
    // loads `tests/conftest.py`.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        ("tests/conftest.py", SKIPS),
        ("tests/m.py", DOCTEST),
        ("tests/test_t.py", "def test_t():\n    pass\n"),
    ]);
    let output = aye(&python, dir.path(), &["top"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert_eq!(summary(&output), "✓ 1 passed", "{}", stdout(&output));
}

/// Recheck regression 4: a module or directory skipped while collecting
/// counts as one skip, but only where the run's selection reaches: with
/// a dotted name, the whole project is refined, and a skip elsewhere isn't
/// the run's (`pytest tests/test_a.py` says `2 passed`).
#[test]
fn skips_outside_what_the_arguments_select_are_not_counted() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("pyproject.toml", PYTEST_CONFIG),
        (
            "tests/conftest.py",
            "import pytest\n\n@pytest.fixture\ndef fx():\n    return 1\n",
        ),
        ("tests/__init__.py", ""),
        (
            "tests/test_a.py",
            "class TestX:\n    def test_y(self, fx):\n        assert fx == 1\n\n    def test_z(self, fx):\n        assert fx == 1\n",
        ),
        ("tests/sub/__init__.py", ""),
        ("tests/sub/conftest.py", SKIPS),
        ("tests/sub/test_s.py", "def test_s():\n    pass\n"),
        (
            "other/test_mod.py",
            "import pytest\npytest.skip('later', allow_module_level=True)\n\ndef test_m():\n    pass\n",
        ),
    ]);
    for (args, want, exit) in [
        (vec!["tests.test_a"], "✓ 2 passed", 0),
        (vec!["tests.test_a.TestX.test_y"], "✓ 1 passed", 0),
        // What a path holds is the run's, as pytest counts it.
        (vec!["tests"], "✓ 2 passed · 1 skipped", 0),
        (vec!["tests.test_a", "other"], "✓ 2 passed · 1 skipped", 0),
        (vec![], "✓ 2 passed · 2 skipped", 0),
        // So is what a dotted name names (`pytest --pyargs`): the skipped
        // module or package is found, and a test in it is not.
        (vec!["other.test_mod"], "no tests ran · 1 skipped", 5),
        (vec!["tests.sub"], "no tests ran · 1 skipped", 5),
        (
            vec!["other.test_mod.test_m"],
            "usage error · 1 skipped · ! 1 usage error",
            4,
        ),
        (
            vec!["tests.sub.test_s"],
            "usage error · 1 skipped · ! 1 usage error",
            4,
        ),
    ] {
        let output = aye(&python, dir.path(), &args);
        assert_eq!(code(&output), exit, "{args:?}: {}", stdout(&output));
        assert_eq!(summary(&output), want, "{args:?}: {}", stdout(&output));
    }
    // An argument naming nothing, with doctests on: pytest says `no tests
    // ran` (exit 4), not a skip of a directory it never met.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        ("sub/conftest.py", SKIPS),
        ("sub/m.py", DOCTEST),
        ("sub/test_t.py", "def test_t():\n    pass\n"),
    ]);
    let output = aye(&python, dir.path(), &["tests"]);
    assert_eq!(code(&output), 4, "{}", stdout(&output));
    assert_eq!(summary(&output), "usage error · ! 1 usage error");
    let (_output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv", "tests"]);
    assert_eq!(diagnosed(&output_log, "skipped"), []);
}

/// A file below a conftest that fails to load gets no `no-tests` note: the
/// conftest's error says why it has no tests, as pytest reports only the
/// error (a start-up one: pytest stops, aye exits 4; one met while
/// collecting: `1 error`, exit 2).
#[test]
fn a_file_below_a_broken_conftest_is_explained_by_its_error() {
    let Some(python) = python() else { return };
    let dynamic = "import pytest\n\nV = [x for x in range(2)]\n\n\
                   @pytest.mark.parametrize('v', V)\ndef test_x(v):\n    pass\n";
    for (conftest, kind, exit) in [
        ("tests/conftest.py", "usage-error", 4),
        ("tests/sub/conftest.py", "collection-error", 2),
    ] {
        let dir = project(&[
            ("pyproject.toml", PYTEST_CONFIG),
            ("tests/test_a.py", "def test_a():\n    pass\n"),
            (conftest, "import zz_missing_module\n"),
            ("tests/sub/test_x.py", dynamic),
        ]);
        let (output, output_log) = logged(
            &python,
            dir.path(),
            &["--output", "json", "-vvv", "-n", "1"],
        );
        assert_eq!(code(&output), exit, "{conftest}: {}", stdout(&output));
        let records = output_log;
        assert_eq!(
            diagnosed(&records, kind),
            [("error".to_owned(), conftest.to_owned())],
            "{conftest}"
        );
        assert_eq!(diagnosed(&records, "no-tests"), [], "{conftest}");
    }
}

/// Recheck regression 3: with the default adapters and doctests on, one
/// conftest that fails to load is one error, as pytest loads it once,
/// though both adapters report it.
#[test]
fn a_conftest_both_adapters_fail_to_load_is_one_error() {
    let Some(python) = python() else { return };
    // At start-up (a `test*` directory's): pytest stops with a traceback,
    // aye with one usage error.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        ("tests/conftest.py", SKIPS),
        ("tests/m.py", DOCTEST),
        ("tests/test_t.py", "def test_t():\n    pass\n"),
    ]);
    let output = aye(&python, dir.path(), &[]);
    assert_eq!(code(&output), 4, "{}", stdout(&output));
    assert_eq!(summary(&output), "usage error · ! 1 usage error");
    assert_eq!(stdout(&output).matches("error[usage-error]").count(), 1);
    let (_output, output_log) = logged(&python, dir.path(), &["--output", "json", "-vvv"]);
    assert_eq!(
        diagnosed(&output_log, "usage-error"),
        [
            ("error".to_owned(), "tests/conftest.py".to_owned()),
            ("info".to_owned(), "tests/conftest.py".to_owned())
        ]
    );
    // While collecting (`pytest.fail`, which the adapters worded apart):
    // pytest says `1 passed, 1 error`.
    let dir = project(&[
        ("pyproject.toml", DOCTESTS_ON),
        ("top.py", DOCTEST),
        (
            "sub/conftest.py",
            "import pytest\npytest.fail(\"broken\")\n",
        ),
        ("sub/m.py", DOCTEST),
        ("sub/test_t.py", "def test_t():\n    pass\n"),
    ]);
    for workers in ["1", "3"] {
        let output = aye(
            &python,
            dir.path(),
            &["--continue-on-collection-errors", "-n", workers],
        );
        assert_eq!(code(&output), 1, "{}", stdout(&output));
        assert_eq!(summary(&output), "✓ 1 passed · ! 1 collection error");
    }
}

/// Ctrl-C while collecting (here, while a worker imports a file to know its
/// tests) stops aye at once, cleanly: `run_finished` says it was
/// interrupted, exit 2, and the worker is gone.
#[cfg(unix)]
#[test]
fn a_stop_signal_while_collecting_stops_at_once() {
    use std::time::{Duration, Instant};

    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_slow_import.py",
        "import os, time\n\nimport pytest\n\nwith open('pid', 'w') as f:\n    f.write(str(os.getpid()))\ntime.sleep(60)\nCASES = [n * 2 for n in range(3)]\n\n\n@pytest.mark.parametrize('n', CASES)\ndef test_n(n):\n    pass\n",
    )]);
    let before = runs_in(dir.path());
    let child = aye_command(&python, dir.path(), &["--output", "json", "-vvv"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let pid_file = dir.path().join("pid");
    let started = Instant::now();
    while !pid_file.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "refinement never started"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(50));
    let worker = fs::read_to_string(&pid_file).unwrap();
    let signal = |name: &str, pid: &str| {
        Command::new("kill")
            .args([name, pid])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    assert!(signal("-INT", &child.id().to_string()));
    let stopped = Instant::now();
    let output = child.wait_with_output().unwrap();
    assert!(
        stopped.elapsed() < Duration::from_secs(3),
        "took {:?} to stop",
        stopped.elapsed()
    );
    assert_eq!(code(&output), 2);
    let records = kept(dir.path(), &before).unwrap_or_else(|| as_records(&output));
    let last = records.last().unwrap();
    assert_eq!(last["event"], "run_finished");
    assert_eq!(last["stopped"], "interrupted");
    let deadline = Instant::now();
    while signal("-0", worker.trim()) {
        assert!(
            deadline.elapsed() < Duration::from_secs(5),
            "worker {worker} outlived aye"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
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

/// A pytest project runs on aye's own engine, with an interpreter that has
/// no pytest, and with one that has (aye's `pytest` module wins), and gets
/// the ids, the order and the outcomes plain pytest gives it: conftest
/// fixtures of two scopes with teardown, parametrize with an id, a class,
/// a mark, skip, xfail, `tmp_path`, `monkeypatch`, `capsys`, `raises`, and
/// failing asserts with their structured comparison. `--native`, the
/// prototype's switch, is accepted and changes nothing, and `aye config`
/// says which engine runs.
#[cfg(unix)]
#[test]
fn a_pytest_project_runs_on_ayes_engine_without_pytest() {
    let Some(python) = python() else { return };
    let venvs = tempfile::tempdir().unwrap();
    let Some(bare) = venv_without_pytest(&python, venvs.path()) else {
        return;
    };
    let dir = project(&[
        (
            "pyproject.toml",
            "[project]\nname = \"p\"\nversion = \"0\"\n\n[tool.pytest.ini_options]\nmarkers = [\"slow: slow tests\"]\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\n\n\n@pytest.fixture(scope=\"session\")\ndef db():\n    data = {\"rows\": []}\n    yield data\n    data.clear()\n\n\n@pytest.fixture\ndef row(db):\n    db[\"rows\"].append(1)\n    yield len(db[\"rows\"])\n    db[\"rows\"].pop()\n",
        ),
        (
            "tests/test_app.py",
            "import os\n\nimport pytest\n\n\ndef test_row(row, db):\n    assert row == 1\n    assert db[\"rows\"] == [1]\n\n\n@pytest.mark.parametrize(\"n, expected\", [(1, 2), (2, 4), pytest.param(3, 7, id=\"odd\")])\ndef test_double(n, expected):\n    assert n * 2 == expected\n\n\nclass TestThings:\n    @pytest.mark.slow\n    def test_tmp(self, tmp_path):\n        (tmp_path / \"f\").write_text(\"x\")\n        assert (tmp_path / \"f\").read_text() == \"x\"\n\n    def test_env(self, monkeypatch):\n        monkeypatch.setenv(\"AYE_X\", \"1\")\n        assert os.environ[\"AYE_X\"] == \"1\"\n\n    def test_out(self, capsys):\n        print(\"hello\")\n        assert capsys.readouterr().out == \"hello\\n\"\n\n\n@pytest.mark.skip(reason=\"not today\")\ndef test_skipped():\n    pass\n\n\n@pytest.mark.xfail(reason=\"known\")\ndef test_xfail():\n    assert 1 == 2\n\n\ndef test_raises():\n    with pytest.raises(ZeroDivisionError):\n        1 / 0\n\n\ndef test_dict():\n    assert {\"a\": 1, \"b\": 2} == {\"a\": 1, \"b\": 3}\n",
        ),
    ]);
    // Plain pytest 9.1.1's ids, in its order, and outcomes.
    let want: Vec<(String, String)> = [
        ("test_row", "passed"),
        ("test_double[1-2]", "passed"),
        ("test_double[2-4]", "passed"),
        ("test_double[odd]", "failed"),
        ("TestThings::test_tmp", "passed"),
        ("TestThings::test_env", "passed"),
        ("TestThings::test_out", "passed"),
        ("test_skipped", "skipped"),
        ("test_xfail", "expected_failure"),
        ("test_raises", "passed"),
        ("test_dict", "failed"),
    ]
    .iter()
    .map(|(id, outcome)| (format!("tests/test_app.py::{id}"), (*outcome).to_string()))
    .collect();
    let sorted = |mut pairs: Vec<(String, String)>| {
        pairs.sort();
        pairs
    };
    let failure = |records: &[Value], id: &str| -> Value {
        events(records, "test_finished")
            .into_iter()
            .find(|e| e["id"] == id)
            .map_or(Value::Null, |e| e["failures"][0].clone())
    };

    for (interpreter, args) in [
        (&bare, &["--output", "json", "-vvv"][..]),
        (&python, &["--output", "json", "-vvv"]),
        (&bare, &["--native", "--output", "json", "-vvv"]),
    ] {
        let (output, output_log) = logged(interpreter, dir.path(), args);
        assert_eq!(code(&output), 1, "{args:?}: {}", stdout(&output));
        let records = output_log;
        assert!(
            events(&records, "case_collected")
                .iter()
                .all(|e| e["adapter"] == "pytest"),
            "{records:#?}"
        );
        let collected: Vec<&str> = events(&records, "case_collected")
            .iter()
            .map(|e| e["id"].as_str().unwrap())
            .collect();
        let want_ids: Vec<&str> = want.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(collected, want_ids, "pytest's collection order");
        assert_eq!(sorted(finished(&records)), sorted(want.clone()));
        // Failing asserts carry the comparison, as pytest's do.
        for id in [
            "tests/test_app.py::test_dict",
            "tests/test_app.py::test_double[odd]",
        ] {
            let failure = failure(&records, id);
            assert_eq!(failure["exception"], "AssertionError", "{id}: {failure:#}");
            assert_eq!(failure["assertion"]["op"], "==", "{id}: {failure:#}");
        }
        // A rerun is plain `aye <id>`.
        let rerun = events(&records, "test_finished")
            .into_iter()
            .find(|e| e["id"] == "tests/test_app.py::test_dict")
            .map(|e| e["reproduce"]["command"].clone())
            .unwrap();
        assert_eq!(
            rerun,
            serde_json::json!(["aye", "tests/test_app.py::test_dict"])
        );
    }

    // `aye config` says which engine runs the tests.
    for interpreter in [&bare, &python] {
        let config = config(interpreter, dir.path());
        assert_eq!(code(&config), 0, "{}", stdout(&config));
        assert!(
            stdout(&config).contains("\nengine   native\n"),
            "{}",
            stdout(&config)
        );
    }
}

/// aye's pytest front-end imports every pytest file a run runs, so a run
/// can't do without refinement: `--no-refine` is a usage error before
/// anything runs, unless with `--co`, which lists the static prediction
/// without importing anything; a run with no pytest adapter takes it.
#[test]
fn a_run_of_pytests_tests_cant_do_without_refinement() {
    let Some(python) = python() else { return };
    let dir = project(&[(
        "tests/test_a.py",
        "import pytest\n\n@pytest.mark.parametrize('x', [1, 2])\ndef test_a(x):\n    pass\n",
    )]);
    for args in [&["--no-refine"][..], &["--native", "--no-refine"]] {
        let output = aye(&python, dir.path(), args);
        assert_eq!(code(&output), 4, "{args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("--no-refine lists tests without importing them, with --co"),
            "{stderr}"
        );
    }
    let (output, output_log) = logged(
        &python,
        dir.path(),
        &["--co", "--no-refine", "--output", "json", "-vvv"],
    );
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    let records = output_log;
    let listed: Vec<(&str, &str)> = events(&records, "case_collected")
        .iter()
        .map(|e| (e["id"].as_str().unwrap(), e["certainty"].as_str().unwrap()))
        .collect();
    assert_eq!(
        listed,
        [
            ("tests/test_a.py::test_a[1]", "exact"),
            ("tests/test_a.py::test_a[2]", "exact")
        ]
    );
    assert_eq!(events(&records, "worker_started").len(), 0);
    fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"p\"\n\n[tool.aye]\nadapters = [\"unittest\"]\n",
    )
    .unwrap();
    let output = aye(&python, dir.path(), &["--no-refine"]);
    assert_eq!(code(&output), 5, "{}", stdout(&output));
}
