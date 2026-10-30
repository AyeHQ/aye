//! What reaches stdout: the text and JSON lines formats at each verbosity,
//! their names, GitHub's annotations, a test's live output, and a stdout
//! nobody reads.

use std::fs;
use std::process::Stdio;

use aye_test::{code, json_lines, project, stdout};
use serde_json::{Value, json};

use crate::{PASSING, aye, aye_command, aye_config, finished, logged, python, summary};

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
        json_lines(&output);
    }
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
        for output in [
            aye(&python, dir.path(), &[]),
            aye_config(&python, dir.path()),
        ] {
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
        json_lines(&output)
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
