//! Regression tests for the fifth round: F21 (without the doctest adapter
//! in the run, pytest's own doctest plugin collects and runs doctests, as it
//! would without aye, rather than nobody), C16 (tests a
//! `pytest_collection_modifyitems` hook adds) and the name refinement
//! reasons give `PYTEST_PLUGINS` plugins.

use super::*;

/// C16: a conftest whose `pytest_collection_modifyitems` makes items
/// (`Function.from_parent`) may add tests to any file: every test file is
/// refined, so that `--co` and `-k` see them. One that only marks or
/// deselects keeps the ids.
#[test]
fn hooks_that_make_items_refine_every_test_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "tests/conftest.py",
        "import pytest\n\ndef pytest_collection_modifyitems(session, config, items):\n    items.extend([pytest.Function.from_parent(i.parent, name=i.name + '_again', callobj=i.obj) for i in items if i.get_closest_marker('twice')])\n",
    );
    write(
        root,
        "tests/test_mod.py",
        "import pytest\n\n@pytest.mark.twice\ndef test_t():\n    pass\n\ndef test_u():\n    pass\n",
    );
    // Session-wide: a test file in another directory too.
    write(root, "other/test_o.py", "def test_o():\n    pass\n");
    let found = recognizer(root);
    assert!(found.scan().adds_items.is_some());
    for rel in ["tests/test_mod.py", "other/test_o.py"] {
        let recognition = recognize_with(&found, root, rel);
        assert!(
            recognition.refine.as_deref().is_some_and(|r| r.contains(
                "a conftest.py implements pytest_collection_modifyitems and makes items: it may add tests"
            )),
            "{rel}: {recognition:?}"
        );
        assert!(
            recognition
                .cases
                .iter()
                .all(|c| c.certainty == Certainty::Partial)
        );
    }
    if let Some(ids) = live_ids(root) {
        assert!(ids.contains(&"tests/test_mod.py::test_t_again".to_string()));
    }
    // A hook that only deselects keeps the ids, `Partial`.
    write(
        root,
        "tests/conftest.py",
        "def pytest_collection_modifyitems(config, items):\n    items[:] = [i for i in items if 'u' not in i.name]\n",
    );
    let found = recognizer(root);
    assert!(found.scan().adds_items.is_none());
    let recognition = recognize_with(&found, root, "tests/test_mod.py");
    assert_eq!(recognition.refine, None);
    // Items made elsewhere in the module (a collector of its own) don't
    // count; those a function the hook calls makes do.
    let collector = "import pytest\n\nclass YamlFile(pytest.File):\n    def collect(self):\n        return []\n\ndef pytest_collect_file(parent, file_path):\n    if file_path.suffix == '.yaml':\n        return YamlFile.from_parent(parent, path=file_path)\n\n";
    for (hook, adds) in [
        (
            "def pytest_collection_modifyitems(items):\n    items.reverse()\n",
            false,
        ),
        (
            "def again(item):\n    return pytest.Function.from_parent(item.parent, name=item.name + '_again', callobj=item.obj)\n\ndef pytest_collection_modifyitems(items):\n    items.extend([again(i) for i in items])\n",
            true,
        ),
    ] {
        write(root, "tests/conftest.py", &format!("{collector}{hook}"));
        assert_eq!(recognizer(root).scan().adds_items.is_some(), adds, "{hook}");
    }
    assert_eq!(
        certainties(&recognition),
        [
            ("tests/test_mod.py::test_t", Certainty::Partial),
            ("tests/test_mod.py::test_u", Certainty::Partial),
        ]
    );
}

/// What `pytest --collect-only` lists, when a Python with pytest is at
/// hand.
fn live_ids(root: &Path) -> Option<Vec<String>> {
    python().map(|python| pytest_ids(&python, root).0)
}

/// A hook a `PYTEST_PLUGINS` plugin implements is named as one, not as a
/// `-p` plugin's.
#[test]
fn reasons_name_pytest_plugins_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]
",
    );
    write(
        root,
        "idplugin.py",
        "def pytest_make_parametrize_id(config, val, argname):\n    return f'P{val}'\n",
    );
    write(
        root,
        "test_hk.py",
        "import pytest\n@pytest.mark.parametrize('x', [3])\ndef test_hk(x):\n    pass\n",
    );
    let reason = |addopts: Option<&str>, plugins: Option<&str>| {
        let project = Project {
            root,
            cwd: root,
            args: &[],
            config_file: None,
            framework_args: &[],
        };
        let recognizer = PytestRecognizer::with_environment(
            &project,
            PytestEnv {
                pytest_addopts: addopts,
                pytest_plugins: plugins,
                ..PytestEnv::default()
            },
        )
        .unwrap();
        recognize_with(&recognizer, root, "test_hk.py")
            .refine
            .unwrap_or_default()
    };
    assert_eq!(
        reason(None, Some("idplugin")),
        "a PYTEST_PLUGINS plugin implements pytest_make_parametrize_id"
    );
    assert_eq!(
        reason(Some("-p idplugin"), None),
        "a -p plugin implements pytest_make_parametrize_id"
    );
}

/// A project whose pytest options turn doctests on, as F21's repro has it.
fn project(root: &Path) {
    write(
        root,
        "pyproject.toml",
        "[tool.aye]\nadapters = [\"pytest\"]\n\n[tool.pytest.ini_options]\naddopts = \"--doctest-modules -ra\"\n",
    );
    write(
        root,
        "m.py",
        "def f():\n    \"\"\"\n    >>> 1\n    2\n    \"\"\"\n\ndef test_helper():\n    pass\n",
    );
    write(root, "test_a.txt", ">>> 1\n2\n");
    write(root, "docs/notes.txt", ">>> 1\n1\n");
    write(root, "test_x.py", "def test_x():\n    pass\n");
    write(
        root,
        "test_y.py",
        "\"\"\"\n>>> 2\n2\n\"\"\"\n\ndef test_y():\n    pass\n",
    );
    write(root, "pkg/__main__.py", "\"\"\"\n>>> 1\n1\n\"\"\"\n");
    write(
        root,
        "setup.py",
        "from setuptools import setup\n\"\"\"\n>>> 1\n1\n\"\"\"\n",
    );
}

/// The pytest adapter as the binary prepares it among `adapters`.
fn prepared(
    root: &Path,
    args: &[&str],
    framework_args: &[&str],
    adapters: &[&str],
) -> PytestRecognizer {
    let args: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let framework_args: Vec<String> = framework_args.iter().map(|s| (*s).to_string()).collect();
    let project = Project {
        root,
        cwd: root,
        args: &args,
        config_file: None,
        framework_args: &framework_args,
    };
    // What `Pytest::prepare_among` builds, with no `PYTEST_ADDOPTS`.
    let recognizer = PytestRecognizer::with_env(&project, None).unwrap();
    if adapters.contains(&"doctest") {
        recognizer
    } else {
        recognizer.with_pytest_doctests()
    }
}

/// A text file as collection hands it over: its text, an empty module.
fn recognize_text(recognizer: &PytestRecognizer, root: &Path, rel: &str) -> Recognition {
    let abs = root.join(rel);
    let source = std::fs::read_to_string(&abs).unwrap();
    let empty = ruff_python_parser::parse_module("").unwrap();
    let lines = LineIndex::from_source_text(&source);
    let file = SourceFile {
        rel,
        abs: &abs,
        source: &source,
        module: empty.syntax(),
        lines: &lines,
    };
    recognizer.recognize(&file, &[])
}

#[test]
fn the_adapter_prepares_pytests_doctests_only_without_the_doctest_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    let args = |recognizer: &dyn Recognizer| recognizer.context()["native"]["args"].clone();
    let alone = Pytest::default()
        .prepare_among(&project, &["pytest"])
        .unwrap()
        .unwrap();
    assert_eq!(
        args(&*alone),
        serde_json::json!(["--doctest-modules", "-ra"])
    );
    assert!(alone.claim("m.py", false).has_tests());
    let beside = Pytest::default()
        .prepare_among(&project, &["doctest", "pytest"])
        .unwrap()
        .unwrap();
    assert_eq!(args(&*beside), serde_json::json!(["-ra"]));
    assert_eq!(beside.claim("m.py", false), Claim::Ignore);
    assert_eq!(beside.claim("test_a.txt", false), Claim::Ignore);
    // `prepare` alone keeps the doctest adapter's division.
    let plain = Pytest::default().prepare(&project).unwrap().unwrap();
    assert_eq!(args(&*plain), serde_json::json!(["-ra"]));
    assert_eq!(plain.claim("m.py", false), Claim::Ignore);
}

#[test]
fn pytests_doctest_plugin_claims_what_it_collects() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    let recognizer = prepared(root, &[], &[], &["pytest"]);
    // `--doctest-modules`: every module, but `__main__.py` and a setuptools
    // `setup.py`; text files by `test*.txt`, or named as `.txt`/`.rst`.
    for (rel, explicit, claimed) in [
        ("m.py", false, true),
        ("pkg/__main__.py", false, false),
        ("setup.py", false, false),
        ("test_a.txt", false, true),
        ("docs/notes.txt", false, false),
        ("docs/notes.txt", true, true),
        ("docs/notes.md", true, false),
        ("test_x.py", false, true),
    ] {
        assert_eq!(
            recognizer.claim(rel, explicit).has_tests(),
            claimed,
            "{rel} (explicit: {explicit})"
        );
    }
    // A conftest is support and a doctest module.
    assert_eq!(
        recognizer.claim("conftest.py", false),
        Claim::TestsAndSupport
    );
    // `--doctest-glob` replaces `test*.txt`; `--ignore` holds.
    let recognizer = prepared(
        root,
        &[],
        &["--doctest-glob", "*.rst", "--ignore=m.py"],
        &["pytest"],
    );
    assert!(!recognizer.claim("test_a.txt", false).has_tests());
    assert!(recognizer.claim("docs/guide.rst", false).has_tests());
    assert!(!recognizer.claim("m.py", false).has_tests());
    assert!(recognizer.claim("m.py", true).has_tests());
    // Without `--doctest-modules`, pytest still collects `test*.txt`.
    write(root, "pyproject.toml", "[tool.pytest.ini_options]\n");
    let recognizer = prepared(root, &[], &[], &["pytest"]);
    assert!(!recognizer.claim("m.py", false).has_tests());
    assert!(recognizer.claim("test_a.txt", false).has_tests());
    // `-p no:doctest` blocks the plugin: it collects nothing.
    let recognizer = prepared(root, &[], &["-p", "no:doctest"], &["pytest"]);
    assert!(!recognizer.claim("test_a.txt", false).has_tests());
    assert!(recognizer.doctests.is_none());
}

#[test]
fn pytests_doctests_are_refined() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    let recognizer = prepared(root, &[], &[], &["pytest"]);
    // Only pytest's doctest plugin collects `m.py`: none of its functions
    // is a test, and only the import tells its doctests.
    recognizer.claim("m.py", false);
    let m = recognize_with(&recognizer, root, "m.py");
    assert!(m.cases.is_empty(), "{m:?}");
    assert!(
        m.refine
            .as_deref()
            .is_some_and(|r| r.contains("pytest's doctest plugin")),
        "{m:?}"
    );
    // Named on the command line, it is a test module too.
    let named = prepared(root, &["m.py"], &[], &["pytest"]);
    assert!(named.claim("m.py", true).has_tests());
    let m = recognize_with(&named, root, "m.py");
    assert_eq!(certainties(&m), [("m.py::test_helper", Certainty::Partial)]);
    assert!(m.refine.is_some());
    // A test module with a doctest prompt is refined, its tests `Partial`;
    // one without stays exact.
    let y = recognize_with(&recognizer, root, "test_y.py");
    assert_eq!(certainties(&y), [("test_y.py::test_y", Certainty::Partial)]);
    assert!(y.refine.is_some());
    let x = recognize_with(&recognizer, root, "test_x.py");
    assert_eq!(certainties(&x), [("test_x.py::test_x", Certainty::Exact)]);
    assert_eq!(x.refine, None);
    // A text file with a prompt is refined; one without has no test.
    let text = recognize_text(&recognizer, root, "test_a.txt");
    assert!(text.refine.is_some() && text.cases.is_empty(), "{text:?}");
    write(root, "test_b.txt", "No examples here.\n");
    let text = recognize_text(&recognizer, root, "test_b.txt");
    assert!(text.refine.is_none() && text.cases.is_empty(), "{text:?}");
    // `aye config` says who collects them.
    let setting = recognizer
        .settings()
        .into_iter()
        .find(|s| s.name == "doctests")
        .unwrap();
    assert_eq!(
        setting.value,
        "collected by pytest's doctest plugin: modules (--doctest-modules), text files matching test*.txt"
    );
    // Beside the doctest adapter, they are its own.
    let beside = prepared(root, &[], &[], &["doctest", "pytest"]);
    assert!(beside.settings().iter().all(|s| s.name != "doctests"));
    let y = recognize_with(&beside, root, "test_y.py");
    assert_eq!(certainties(&y), [("test_y.py::test_y", Certainty::Exact)]);
}

#[test]
fn pytests_doctest_options_reach_the_front_end_without_the_doctest_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --doctest-glob='*.rst' -ra -n 2\n",
    );
    let framework_args = [
        "--doctest-report",
        "udiff",
        "-q",
        "--doctest-continue-on-failure",
    ];
    let args: Vec<String> = framework_args.iter().map(|s| (*s).to_string()).collect();
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &args,
    };
    let recognizer = PytestRecognizer::with_env(
        &project,
        Some("--doctest-ignore-import-errors --doctest-glob=*.md -v"),
    )
    .unwrap()
    .with_pytest_doctests();
    // As given, but what the runner takes over (`-n`).
    assert_eq!(
        recognizer.config().args,
        [
            "--doctest-modules",
            "--doctest-glob=*.rst",
            "-ra",
            "--doctest-ignore-import-errors",
            "--doctest-glob=*.md",
            "-v",
            "--doctest-report",
            "udiff",
            "-q",
            "--doctest-continue-on-failure"
        ]
    );
    // pytest appends the globs, in its order.
    assert_eq!(
        recognizer.config().doctest,
        config::DoctestOptions {
            modules: true,
            globs: vec!["*.rst".into(), "*.md".into()],
        }
    );
    assert!(recognizer.claim("docs/a.md", false).has_tests());
    assert!(!recognizer.claim("test_a.txt", false).has_tests());
}

/// pytest itself, when at hand, collects what the recognizer claims, and
/// nothing from `m.py` but its doctest.
#[test]
fn pytest_collects_the_doctests_the_recognizer_claims() {
    let Some(python) = python() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    project(root);
    let (ids, failed) = pytest_ids(&python, root);
    assert!(failed.is_empty(), "{failed:?}");
    let recognizer = prepared(root, &[], &[], &["pytest"]);
    for id in &ids {
        let file = id.split("::").next().unwrap();
        assert!(recognizer.claim(file, false).has_tests(), "{id}");
    }
    assert!(ids.contains(&"m.py::m.f".to_string()), "{ids:?}");
    assert!(
        ids.contains(&"test_a.txt::test_a.txt".to_string()),
        "{ids:?}"
    );
    assert!(!ids.contains(&"m.py::test_helper".to_string()), "{ids:?}");
}
