//! Regression tests for the fourth round's configuration, conftest, fixture
//! and import findings (C5, C6, C8, C9, C12, C13, C17) and the option items
//! (D1: doctest options lifted, D3: `PYTEST_PLUGINS`; D2's is in the parent
//! module).

use std::collections::BTreeSet;

use super::*;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn doctest_options_are_lifted_out() {
    let (kept, lifted) = config::split_doctest(&strings(&[
        "-ra",
        "--doctest-modules",
        "--doctest-glob=*.rst",
        "--doctest-glob",
        "*.txt",
        "--doctest-report",
        "udiff",
        "--doctest-report=ndiff",
        "--doctest-continue-on-failure",
        "--doctest-ignore-import-errors",
        "-q",
    ]));
    assert_eq!(kept, strings(&["-ra", "-q"]));
    assert_eq!(
        lifted,
        strings(&[
            "--doctest-modules",
            "--doctest-glob=*.rst",
            "--doctest-glob",
            "*.txt",
            "--doctest-report",
            "udiff",
            "--doctest-report=ndiff",
            "--doctest-continue-on-failure",
            "--doctest-ignore-import-errors",
        ])
    );
    // A valued option missing its value is pytest's to reject; pytest takes
    // no abbreviations.
    let (kept, _) = config::split_doctest(&strings(&["--doctest-glob", "-v", "--doctest-mod"]));
    assert_eq!(kept, strings(&["--doctest-glob", "-v", "--doctest-mod"]));
}

#[test]
fn doctest_options_never_reach_the_front_end_beside_the_doctest_adapter() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = --doctest-modules --doctest-glob='*.rst' -ra\n",
    );
    write(root, "test_a.py", "def test_a(): pass\n");
    let recognizer = recognizer_with(
        root,
        &[],
        &[
            "--doctest-report",
            "udiff",
            "-q",
            "--doctest-continue-on-failure",
        ],
        Some("--doctest-ignore-import-errors --doctest-glob=*.md -v"),
    );
    assert_eq!(recognizer.config().args, ["-ra", "-v", "-q"]);
    // Static collection is unchanged: pytest's doctest items are the
    // doctest adapter's.
    assert_eq!(
        certainties(&recognize_with(&recognizer, root, "test_a.py")),
        [("test_a.py::test_a", Certainty::Exact)]
    );
}

#[test]
fn pytest_plugins_from_the_environment_are_session_plugins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "myplugin.py",
        "import pytest\n@pytest.fixture(params=[1, 2])\ndef num(request):\n    return request.param\n",
    );
    write(
        root,
        "test_a.py",
        "def test_a(): pass\ndef test_n(num): pass\n",
    );
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    // A project module: its fixtures count.
    let with_plugins = |plugins: &str| {
        PytestRecognizer::with_environment(
            &project,
            PytestEnv {
                pytest_plugins: Some(plugins),
                ..PytestEnv::default()
            },
        )
        .unwrap()
    };
    let recognizer = with_plugins("myplugin");
    let recognition = recognize_with(&recognizer, root, "test_a.py");
    assert!(recognition.refine.is_some(), "{recognition:?}");
    assert_eq!(
        recognizer.context().into_value()["env_plugins"],
        serde_json::json!(["-p", "myplugin"])
    );
    // A module aye can't read may mark, deselect or parametrize any test.
    let recognizer = with_plugins(" installed_plugin , pytest_cov");
    let recognition = recognize_with(&recognizer, root, "test_a.py");
    assert_eq!(recognition.cases[0].certainty, Certainty::Partial);
    let context = recognizer.context().into_value();
    assert_eq!(
        context["env_plugins"],
        serde_json::json!(["-p", "installed_plugin", "-p", "pytest_cov"])
    );
    // `-p no:NAME` blocks it.
    write(
        root,
        "pytest.ini",
        "[pytest]\naddopts = -p no:installed_plugin\n",
    );
    let recognizer = with_plugins("installed_plugin");
    assert_eq!(
        recognizer.context().into_value()["env_plugins"],
        serde_json::json!([])
    );
    assert_eq!(
        certainties(&recognize_with(&recognizer, root, "test_a.py"))[0],
        ("test_a.py::test_a", Certainty::Exact)
    );
}

/// Whether `rel` is refined, and its cases' ids and certainties.
fn verdict(root: &Path, rel: &str) -> (bool, Vec<(String, Certainty)>) {
    let recognition = recognize_at(root, rel);
    (
        recognition.refine.is_some(),
        recognition
            .cases
            .iter()
            .map(|c| (c.id.as_str().to_owned(), c.certainty))
            .collect(),
    )
}

/// The ids `pytest --collect-only` lists under `root`, when a Python with
/// pytest is at hand.
fn live_ids(root: &Path) -> Option<Vec<String>> {
    python().map(|python| pytest_ids(&python, root).0)
}

// ---------------------------------------------------------------- C5

#[test]
fn conftest_ini_changes_refine_the_files_below_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "tests/conftest.py",
        "import pytest\n\ndef pytest_configure(config):\n    config.addinivalue_line(\n        'markers', 'slow: slow tests'\n    )\n    config.addinivalue_line('python_functions', 'check_*')\n    config.addinivalue_line('python_classes', '*Suite')\n    config.addinivalue_line('python_files', 'check_*.py')\n    config.addinivalue_line('usefixtures', 'flavour')\n\n@pytest.fixture(params=['vanilla', 'chocolate'])\ndef flavour(request):\n    return request.param\n",
    );
    write(
        root,
        "tests/test_cfg.py",
        "def test_a():\n    pass\n\ndef check_b():\n    pass\n\nclass OrderSuite:\n    def test_c(self):\n        pass\n",
    );
    write(
        root,
        "tests/check_things.py",
        "def test_in_check_file():\n    pass\n",
    );
    write(root, "other/test_o.py", "def test_o():\n    pass\n");
    // The file a literal `python_files` pattern adds is a test module.
    let recognizer = recognizer(root);
    assert!(
        recognizer
            .claim("tests/check_things.py", Requested::Found)
            .has_tests()
    );
    assert!(
        !recognizer
            .claim("other/check_things.py", Requested::Found)
            .has_tests()
    );
    let (refined, cases) = verdict(root, "tests/test_cfg.py");
    assert!(refined);
    assert_eq!(
        cases,
        [("tests/test_cfg.py::test_a".to_string(), Certainty::Partial)]
    );
    assert!(verdict(root, "tests/check_things.py").0);
    // Outside the conftest's directory: unchanged.
    assert_eq!(
        verdict(root, "other/test_o.py"),
        (
            false,
            vec![("other/test_o.py::test_o".to_string(), Certainty::Exact)]
        )
    );
    if let Some(ids) = live_ids(root) {
        // pytest's ids for the refined files are what refinement reports;
        // the walk claims every file pytest collects.
        let files: BTreeSet<&str> = ids
            .iter()
            .map(|id| id.split("::").next().unwrap())
            .collect();
        let claimed: BTreeSet<String> = collect(root).into_iter().map(|(rel, _)| rel).collect();
        assert_eq!(
            files
                .into_iter()
                .map(str::to_string)
                .collect::<BTreeSet<_>>(),
            claimed
        );
        assert!(ids.contains(&"tests/test_cfg.py::OrderSuite::test_c[chocolate]".to_string()));
    }

    // What doesn't change collection leaves the files exact.
    for (conftest, refined) in [
        (
            "def pytest_configure(config):\n    config.addinivalue_line('markers', 'slow: slow')\n    config.option.verbose = 2\n",
            false,
        ),
        (
            "def pytest_configure(config):\n    config.addinivalue_line(NAME, 'check_*')\nNAME = 'python_functions'\n",
            true,
        ),
        (
            "def pytest_configure(config):\n    name = 'python_' + 'functions'\n    config.addinivalue_line(name, 'check_*')\n",
            true,
        ),
        (
            "def pytest_configure(config):\n    config.getini('python_functions').append('check_*')\n",
            true,
        ),
        (
            "def pytest_configure(config):\n    config.option.ignore.append('tests/test_cfg.py')\n",
            true,
        ),
        (
            "def pytest_configure(config):\n    config._inicache['python_classes'] = ['Suite']\n",
            true,
        ),
        (
            "def pytest_configure(config):\n    setattr(config.option, 'importmode', 'append')\n",
            true,
        ),
    ] {
        write(root, "tests/conftest.py", conftest);
        assert_eq!(verdict(root, "tests/test_cfg.py").0, refined, "{conftest}");
    }
}

// ---------------------------------------------------------------- C6

#[test]
fn unittest_cases_from_modules_aye_cannot_read_are_refined() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(
        root,
        "tests/support/apicases.py",
        "import unittest\nclass HealthChecks(unittest.TestCase):\n    def test_health(self):\n        pass\n",
    );
    write(
        root,
        "tests/test_api.py",
        "from apicases import HealthChecks\n\ndef test_users():\n    pass\n",
    );
    // A conftest puts the directory on `sys.path` in a way aye reads.
    for conftest in [
        "import os\nimport sys\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), 'support'))\n",
        "import sys\nfrom pathlib import Path\nsys.path.append(str(Path(__file__).parent / 'support'))\n",
        "import sys\nfrom os.path import abspath, dirname, join\nHERE = abspath(join(dirname(__file__), 'support'))\nsys.path.insert(1, HERE)\n",
        "import site\nimport pathlib\nsite.addsitedir(str(pathlib.Path(__file__).resolve().parents[0].joinpath('support')))\n",
    ] {
        write(root, "tests/conftest.py", conftest);
        let recognition = recognize_at(root, "tests/test_api.py");
        assert!(
            recognition
                .refine
                .as_deref()
                .is_some_and(|r| r.contains("HealthChecks")),
            "{conftest}: {recognition:?}"
        );
    }
    if let Some(ids) = live_ids(root) {
        assert!(ids.contains(&"tests/test_api.py::HealthChecks::test_health".to_string()));
    }
    // In a way aye can't read: any capitalized name from a module it can't
    // find may be a unittest case there.
    write(
        root,
        "tests/conftest.py",
        "import sys\nsys.path.insert(0, 'tests/support')\n",
    );
    assert!(verdict(root, "tests/test_api.py").0);
    // With no such conftest, a module aye can't find is an installed
    // package's: only names like unittest cases' are doubted.
    std::fs::remove_file(root.join("tests/conftest.py")).unwrap();
    write(
        root,
        "tests/test_api.py",
        "from somepkg import Helper, Config\n\ndef test_users():\n    pass\n",
    );
    assert!(!verdict(root, "tests/test_api.py").0);
    write(
        root,
        "tests/test_api.py",
        "from somepkg.testing import ApiTests\n\ndef test_users():\n    pass\n",
    );
    assert!(verdict(root, "tests/test_api.py").0);
    // A directory the module itself puts on `sys.path`, read: what isn't
    // there is an installed package's.
    write(
        root,
        "tests/test_api.py",
        "import sys\nfrom pathlib import Path\nsys.path.append(str(Path(__file__).parent / 'support'))\nfrom apicases import HealthChecks\nfrom somepkg import Helper\n\ndef test_users():\n    pass\n",
    );
    let recognition = recognize_at(root, "tests/test_api.py");
    let reason = recognition.refine.unwrap();
    assert!(
        reason.contains("HealthChecks") && !reason.contains("Helper"),
        "{reason}"
    );
}

#[test]
fn pythonpath_directories_are_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    let ext = dir.path().join("ext");
    write(&root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(&ext, "extlib/__init__.py", "");
    write(
        &ext,
        "extlib/cases.py",
        "import unittest\nclass ApiChecks(unittest.TestCase):\n    def test_api(self):\n        pass\n",
    );
    write(
        &root,
        "test_ext.py",
        "from extlib.cases import ApiChecks\n\ndef test_local():\n    pass\n",
    );
    let project = Project {
        root: &root,
        cwd: &root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    let pythonpath = ext.to_string_lossy().into_owned();
    let recognizer = PytestRecognizer::with_environment(
        &project,
        PytestEnv {
            pythonpath: Some(&pythonpath),
            ..PytestEnv::default()
        },
    )
    .unwrap();
    let recognition = recognize_with(&recognizer, &root, "test_ext.py");
    assert!(recognition.refine.is_some(), "{recognition:?}");
    // Without it, the module is an installed package's.
    assert!(
        recognize_with(
            &recognizer_with(&root, &[], &[], None),
            &root,
            "test_ext.py"
        )
        .refine
        .is_none()
    );
}

// ---------------------------------------------------------------- C8

#[test]
fn fixtures_aye_cannot_read_refine_the_tests_that_use_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    let refined = |conftest: &str, test: &str| -> bool {
        write(root, "conftest.py", conftest);
        write(root, "test_fx.py", test);
        verdict(root, "test_fx.py").0
    };
    // Call forms, with and without parameters.
    assert!(refined(
        "import pytest\ndef _backend(request):\n    return request.param\nbackend = pytest.fixture(params=['sqlite', 'postgres'])(_backend)\n",
        "def test_backend(backend):\n    pass\n",
    ));
    assert!(refined(
        "import pytest\ndef _level(request):\n    return request.param\nlevel = pytest.fixture(_level, params=[1, 2])\n",
        "def test_level(level):\n    pass\n",
    ));
    assert!(!refined(
        "import pytest\ndef _plain():\n    return 1\nplain = pytest.fixture(_plain)\n",
        "def test_plain(plain):\n    pass\n",
    ));
    // What the call form requests counts.
    assert!(refined(
        "import pytest\n@pytest.fixture(params=[1, 2])\ndef num(request):\n    return request.param\ndef _plain(num):\n    return num\nplain = pytest.fixture()(_plain)\n",
        "def test_plain(plain):\n    pass\n",
    ));
    // A wrapper factory, a rebinding, an alias aye can't follow, names
    // bound at runtime and a computed name.
    for conftest in [
        "import pytest\ndef param_fixture(*values):\n    return pytest.fixture(params=list(values))\n@param_fixture('a', 'b')\ndef mode(request):\n    return request.param\n",
        "import pytest\ndef register(f):\n    return f\n@pytest.fixture\ndef mode():\n    return 1\nmode = register(mode)\n",
        "import pytest\nclass _Holder:\n    @staticmethod\n    @pytest.fixture(params=[7, 8])\n    def held(request):\n        return request.param\nmode = _Holder.held\n",
        "import pytest\nfor _name in ['mode']:\n    globals()[_name] = pytest.fixture(params=['x', 'y'])(lambda request: request.param)\n",
        "import os\nimport pytest\n@pytest.fixture(name=os.environ.get('NAME', 'mode'), params=['m', 'n'])\ndef _impl(request):\n    return request.param\n",
    ] {
        assert!(
            refined(conftest, "def test_mode(mode):\n    pass\n"),
            "{conftest}"
        );
    }
    // An alias of a fixture is that fixture.
    assert!(refined(
        "import pytest\n@pytest.fixture(params=[1, 2])\ndef num(request):\n    return request.param\nalias = num\n",
        "def test_alias(alias):\n    pass\n",
    ));
    assert!(!refined(
        "import pytest\n@pytest.fixture\ndef num():\n    return 1\nalias = num\n",
        "def test_alias(alias):\n    pass\n",
    ));
    // In the test module too.
    assert!(refined(
        "",
        "import pytest\ndef _local(request):\n    return request.param\nloc = pytest.fixture(params=['p', 'q'])(_local)\n\ndef test_loc(loc):\n    pass\n",
    ));
    // Values that surely make no fixture don't count, nor do names no test
    // requests.
    assert!(!refined(
        "import logging\nimport pytest\nlogger = logging.getLogger(__name__)\nclient = object()\n@pytest.fixture\ndef app():\n    return 1\n",
        "class TestClient:\n    pass\napp_client = TestClient()\n\ndef test_a(app, tmp_path):\n    pass\n",
    ));
    assert!(!refined(
        "import pytest\ndef helper(item):\n    setattr(item, 'rep_' + item.name, 1)\n@pytest.fixture\ndef app():\n    return 1\n",
        "def test_a(app):\n    pass\n",
    ));
    // An instance of a class of the module is no fixture, unless the class
    // may make something else (`__new__`, a metaclass, a decorator).
    assert!(!refined(
        "import pytest\nclass Settings:\n    debug = False\n    def __init__(self):\n        self.debug = True\nsettings = Settings()\n",
        "def test_s(settings):\n    pass\n",
    ));
    for class in [
        "class Settings:\n    def __new__(cls):\n        return pytest.fixture(params=[1, 2])(lambda request: request.param)\n",
        "class Meta(type):\n    pass\nclass Settings(metaclass=Meta):\n    pass\n",
        "def deco(c):\n    return c\n@deco\nclass Settings:\n    pass\n",
        "from collections import OrderedDict as Base\nclass Settings(Base):\n    pass\n",
    ] {
        assert!(
            refined(
                &format!("import pytest\n{class}settings = Settings()\n"),
                "def test_s(settings):\n    pass\n",
            ),
            "{class}"
        );
    }
    if let Some(python) = python() {
        // pytest agrees on the files aye keeps exact.
        assert!(differential(&python, root).is_ok());
    }
    // A capitalized callee may be a factory: whatever it is called, pytest
    // registers the fixture it returns (the rule for test names, C3, holds
    // for fixtures too).
    assert!(refined(
        "import pytest\ndef Param(*values):\n    def make(request):\n        return request.param\n    return pytest.fixture(params=list(values))(make)\nbackend = Param('sqlite', 'postgres')\n",
        "def test_backend(backend):\n    pass\n",
    ));
    if let Some(ids) = live_ids(root) {
        assert_eq!(
            ids,
            [
                "test_fx.py::test_backend[sqlite]",
                "test_fx.py::test_backend[postgres]"
            ]
        );
    }
}

#[test]
fn plugins_on_a_path_a_conftest_adds_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    write(
        root,
        "tests/support/fixtures_db.py",
        "import pytest\n@pytest.fixture(params=['sqlite', 'postgres'])\ndef db(request):\n    return request.param\n",
    );
    write(
        root,
        "tests/test_db.py",
        "def test_db(db):\n    pass\n\ndef test_tmp(tmp_path):\n    pass\n",
    );
    write(
        root,
        "conftest.py",
        "import os\nimport sys\nsys.path.insert(0, os.path.join(os.path.dirname(__file__), 'tests', 'support'))\npytest_plugins = ['fixtures_db']\n",
    );
    let recognition = recognize_at(root, "tests/test_db.py");
    assert!(
        recognition
            .refine
            .as_deref()
            .is_some_and(|r| r.contains("parametrized fixture db")),
        "{recognition:?}"
    );
    if let Some(ids) = live_ids(root) {
        assert!(ids.contains(&"tests/test_db.py::test_db[postgres]".to_string()));
    }
    // A path aye can't tell: a name no readable definition has may be one
    // of the plugin's fixtures; pytest's own are not.
    write(
        root,
        "conftest.py",
        "import sys\nsys.path.insert(0, 'tests/support')\npytest_plugins = ['fixtures_db']\n",
    );
    let recognition = recognize_at(root, "tests/test_db.py");
    let reason = recognition.refine.unwrap();
    assert!(reason.contains("test_db uses db"), "{reason}");
    assert!(!reason.contains("tmp_path"), "{reason}");
    // A plugin aye can't find, with no such path: an installed one, as
    // before.
    write(
        root,
        "conftest.py",
        "pytest_plugins = ['installed_plugin']\n",
    );
    assert!(recognize_at(root, "tests/test_db.py").refine.is_none());
}

// ---------------------------------------------------------------- C9

#[test]
fn fixtures_defined_in_alternative_branches_refine_their_tests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pytest.ini", "[pytest]\n");
    write(
        root,
        "tests/conftest.py",
        "import sys\nimport pytest\n\ntry:\n    import json  # noqa: F401\n    @pytest.fixture(params=['json', 'pickle'])\n    def codec(request):\n        return request.param\nexcept ImportError:\n    @pytest.fixture\n    def codec():\n        return 'repr'\n\nif sys.version_info >= (3, 8):\n    @pytest.fixture(params=['new', 'old'])\n    def api(request):\n        return request.param\nelse:\n    @pytest.fixture\n    def api():\n        return 'old'\n\n@pytest.fixture(params=[1, 2])\ndef outer(request):\n    return request.param\n",
    );
    write(
        root,
        "tests/test_condfx.py",
        "def test_codec(codec):\n    pass\n\ndef test_api(api):\n    pass\n",
    );
    let reason = recognize_at(root, "tests/test_condfx.py").refine.unwrap();
    assert!(
        reason.contains("parametrized fixture codec")
            && reason.contains("parametrized fixture api"),
        "{reason}"
    );
    // In the module, and a definition that may not run doesn't hide the
    // conftest's.
    write(
        root,
        "tests/test_m.py",
        "import sys\nimport pytest\nif sys.platform == 'win32':\n    @pytest.fixture\n    def outer():\n        return 0\n\ndef test_outer(outer):\n    pass\n",
    );
    assert!(verdict(root, "tests/test_m.py").0);
    if let Some(ids) = live_ids(root) {
        // On Windows the module's definition runs, and replaces the
        // conftest's parametrized one.
        let id = if cfg!(windows) {
            "tests/test_m.py::test_outer"
        } else {
            "tests/test_m.py::test_outer[2]"
        };
        assert!(ids.contains(&id.to_string()), "{ids:?}");
    }
    // Two plain alternatives stay exact; so does a definition that surely
    // replaces an earlier one.
    write(
        root,
        "tests/test_m.py",
        "import sys\nimport pytest\nif sys.platform == 'win32':\n    @pytest.fixture\n    def plain():\n        return 0\nelse:\n    @pytest.fixture\n    def plain():\n        return 1\n\n@pytest.fixture(params=[1, 2])\ndef again(request):\n    return request.param\n\n@pytest.fixture\ndef again():\n    return 3\n\ndef test_plain(plain, again):\n    pass\n",
    );
    assert_eq!(
        verdict(root, "tests/test_m.py"),
        (
            false,
            vec![("tests/test_m.py::test_plain".to_string(), Certainty::Exact)]
        )
    );
    if let Some(ids) = live_ids(root) {
        assert!(ids.contains(&"tests/test_condfx.py::test_codec[pickle]".to_string()));
        assert!(ids.contains(&"tests/test_m.py::test_plain".to_string()));
    }
}

// ---------------------------------------------------------------- C12

#[test]
fn pyargs_testpaths_are_looked_up_on_sys_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "pyproject.toml",
        "[project]\nname = 'x'\n[tool.pytest.ini_options]\npythonpath = ['src']\naddopts = '--pyargs'\ntestpaths = ['mypkg.tests', 'mypkg.single', 'tests']\n",
    );
    write(root, "src/mypkg/__init__.py", "");
    write(root, "src/mypkg/tests/__init__.py", "");
    write(
        root,
        "src/mypkg/tests/test_p.py",
        "def test_p():\n    pass\n",
    );
    write(root, "src/mypkg/single.py", "");
    std::fs::create_dir_all(root.join("tests")).unwrap();
    let recognizer = recognizer(root);
    assert_eq!(
        recognizer.collection_args,
        [
            root.join("src/mypkg/tests"),
            root.join("src/mypkg/single.py"),
            root.join("tests")
        ]
    );
    if let Some(ids) = live_ids(root) {
        assert_eq!(ids, ["src/mypkg/tests/test_p.py::test_p"]);
    }
}

// ---------------------------------------------------------------- C13

#[test]
fn imports_resolve_as_python_does_in_each_import_mode() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[project]\nname = 'x'\n");
    // A package wins over a module of the same name.
    write(
        root,
        "tests/helpers.py",
        "import pytest\n@pytest.fixture\ndef db():\n    return 'memory'\n",
    );
    write(
        root,
        "tests/helpers/__init__.py",
        "import pytest\n@pytest.fixture(params=['sqlite', 'postgres'])\ndef db(request):\n    return request.param\n",
    );
    write(
        root,
        "tests/test_pm.py",
        "from helpers import db\n\ndef test_db(db):\n    pass\n",
    );
    assert!(verdict(root, "tests/test_pm.py").0);
    if let Some(ids) = live_ids(root) {
        assert!(ids.contains(&"tests/test_pm.py::test_db[postgres]".to_string()));
    }
    std::fs::remove_dir_all(root.join("tests")).unwrap();

    // The test module's directory comes first in `prepend` mode, after the
    // rootdir in `append` and `importlib` modes.
    write(
        root,
        "fixtures_shared.py",
        "import pytest\n@pytest.fixture(params=['sqlite', 'postgres'])\ndef db(request):\n    return request.param\n",
    );
    write(
        root,
        "tests/fixtures_shared.py",
        "import pytest\n@pytest.fixture\ndef db():\n    return 'memory'\n",
    );
    write(
        root,
        "tests/test_imp.py",
        "from fixtures_shared import db\n\ndef test_db(db):\n    pass\n",
    );
    for (mode, refined) in [("prepend", false), ("append", true), ("importlib", true)] {
        write(
            root,
            "pyproject.toml",
            &format!(
                "[project]\nname = 'x'\n[tool.pytest.ini_options]\naddopts = '--import-mode={mode}'\n"
            ),
        );
        assert_eq!(verdict(root, "tests/test_imp.py").0, refined, "{mode}");
        if let Some(ids) = live_ids(root) {
            assert_eq!(
                ids.contains(&"tests/test_imp.py::test_db[postgres]".to_string()),
                refined,
                "{mode}"
            );
        }
    }
}

// ---------------------------------------------------------------- C17

#[test]
fn test_modules_importing_a_colliding_helper_are_refined_together() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "[tool.pytest.ini_options]\n");
    for side in ["a", "b"] {
        write(
            root,
            &format!("tests/{side}/helpers.py"),
            &format!("class Base:\n    def test_from_{side}(self):\n        pass\n"),
        );
        write(
            root,
            &format!("tests/{side}/test_{side}.py"),
            "from helpers import Base\n\nclass TestK(Base):\n    pass\n",
        );
    }
    write(
        root,
        "tests/c/test_c.py",
        "import json\n\ndef test_c():\n    pass\n",
    );
    let found = recognizer(root);
    assert_eq!(
        found.context().into_value()["colliding_modules"],
        serde_json::json!(["helpers"])
    );
    for rel in ["tests/a/test_a.py", "tests/b/test_b.py"] {
        let recognition = recognize_with(&found, root, rel);
        assert!(
            recognition
                .refine
                .as_deref()
                .is_some_and(|r| r.contains("it imports helpers")),
            "{rel}: {recognition:?}"
        );
        for case in &recognition.cases {
            assert_eq!(case.affinity[0], "import:helpers");
        }
    }
    let other = recognize_with(&found, root, "tests/c/test_c.py");
    assert_eq!(other.refine, None);
    assert_eq!(other.cases[0].affinity[0], "import:test_c");
    if let Some(ids) = live_ids(root) {
        // In one session, Python keeps the first `helpers` it imports.
        assert!(ids.contains(&"tests/b/test_b.py::TestK::test_from_a".to_string()));
    }
    // `importlib` puts no test directory on `sys.path`; packages give the
    // helpers names of their own.
    write(
        root,
        "pyproject.toml",
        "[tool.pytest.ini_options]\naddopts = '--import-mode=importlib'\n",
    );
    let found = recognizer(root);
    assert_eq!(
        found.context().into_value()["colliding_modules"],
        serde_json::json!([])
    );
    assert!(
        !recognize_with(&found, root, "tests/a/test_a.py")
            .refine
            .as_deref()
            .unwrap_or_default()
            .contains("it imports helpers")
    );
    write(root, "pyproject.toml", "[tool.pytest.ini_options]\n");
    write(root, "tests/a/__init__.py", "");
    write(root, "tests/b/__init__.py", "");
    write(root, "tests/__init__.py", "");
    assert_eq!(
        recognizer(root).context().into_value()["colliding_modules"],
        serde_json::json!([])
    );
}
