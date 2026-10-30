//! What the positional arguments name: paths, from the current directory
//! or the project root, ids, dotted names and keys; what names nothing
//! (`not-found`); the order they give collection; and what a run of dotted
//! names or keys starts from.

use std::fs;

use aye_test::{code, events, project, stdout};

use crate::{
    DOCTEST, DOCTESTS_ON, PASSING, PYTEST_CONFIG, SKIPS, aye, diagnosed, finished, listed, logged,
    python, summary,
};

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

/// Two arguments that read alike, the word `nope` and the path `./nope`
/// (neither of which exists), are one selector to selection, and each
/// that names nothing is reported as the first of them is: a word's
/// `not-found`, or nothing beyond collection's for a missing path.
#[test]
fn arguments_that_read_alike_are_reported_as_the_first_of_them() {
    let Some(python) = python() else { return };
    let dir = project(&[("tests/test_a.py", PASSING)]);
    let not_found = |args: &[&str]| {
        let mut argv = vec!["--output", "json", "-vvv"];
        argv.extend(args);
        let (output, records) = logged(&python, dir.path(), &argv);
        assert_eq!(code(&output), 4, "{args:?}: {}", stdout(&output));
        events(&records, "diagnostic")
            .iter()
            .filter(|d| d["code"] == "not-found")
            .map(|d| {
                (
                    d["path"].as_str().unwrap().to_owned(),
                    d["message"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>()
    };
    let missing = (
        "nope".to_owned(),
        "file or directory not found: ./nope".to_owned(),
    );
    assert_eq!(
        not_found(&["nope", "./nope"]),
        [
            missing.clone(),
            (String::new(), "not found: nope".to_owned())
        ]
    );
    assert_eq!(not_found(&["./nope", "nope"]), [missing]);
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

/// A dotted name reaches what collection skipped by the rule the binary
/// has read skips with since 0.0.1, which takes any parts, where selection
/// reads a word as a dotted name only when each part is an identifier: so
/// `test_my-mod` names a skipped `test_my-mod.py` whole (no `not-found`,
/// the skip counted), and `test_my-mod.x` names something in it (the skip
/// counted, and a `not-found`).
#[test]
fn a_word_reaches_what_collection_skipped_whatever_its_parts() {
    let Some(python) = python() else { return };
    let dir = project(&[
        ("pyproject.toml", PYTEST_CONFIG),
        ("test_a.py", PASSING),
        (
            "test_my-mod.py",
            "import pytest\npytest.skip('later', allow_module_level=True)\n",
        ),
    ]);
    let output = aye(&python, dir.path(), &["test_my-mod"]);
    assert_eq!(code(&output), 5, "{}", stdout(&output));
    assert_eq!(summary(&output), "no tests ran · 1 skipped");
    let output = aye(&python, dir.path(), &["test_my-mod.x"]);
    assert_eq!(code(&output), 4, "{}", stdout(&output));
    assert_eq!(
        summary(&output),
        "usage error · 1 skipped · ! 1 usage error"
    );
    assert!(
        stdout(&output).contains("not found: test_my-mod.x"),
        "{}",
        stdout(&output)
    );
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
