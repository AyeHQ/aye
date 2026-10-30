//! The JUnit XML report: where it goes, what reaches it, and the
//! framework's own.

use std::fs;

use aye_test::{code, project, stdout};

use crate::{aye, aye_command, python};

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
