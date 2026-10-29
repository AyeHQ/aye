use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aye_engine::{Claim, Ranged, RefineError, RefineRequest, Refined, Setting, ast};
use aye_events::{Certainty, DiagnosticCode, ProjectPath};

use super::*;

fn extension(rel: &str) -> Option<&str> {
    Path::new(rel).extension()?.to_str()
}

/// Claims `*_check.py` (any `.py` when explicit), `helpers.py` as support,
/// and `both.py` as tests and support. One case per top-level function,
/// carrying the support files it was handed as keywords; a function named
/// `dynamic` asks for refinement, and one named `broken` too, with a static
/// error and a warning for the file.
struct Fake {
    roots: Vec<PathBuf>,
    /// Claims every `.py` file for tests, whatever its name (as pytest's
    /// doctest modules do).
    any_name: bool,
    /// The adapter its cases name.
    adapter: &'static str,
    /// Every `claim` call: the path and `explicit`.
    claims: Arc<Mutex<Vec<(String, bool)>>>,
    setting: String,
    context: serde_json::Value,
}

impl Fake {
    fn new() -> Self {
        Self {
            roots: Vec::new(),
            any_name: false,
            adapter: "fake",
            claims: Arc::default(),
            setting: "1".to_string(),
            context: serde_json::json!({"rootdir": "/fake"}),
        }
    }

    fn rooted(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            ..Self::new()
        }
    }
}

impl Recognizer for Fake {
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if self.roots.is_empty() {
            vec![project.cwd.to_path_buf()]
        } else {
            self.roots.clone()
        }
    }

    fn descend(&self, dir: &Path) -> bool {
        let name = dir.file_name().unwrap().to_string_lossy();
        !name.starts_with('.') && name != "skip"
    }

    fn claim(&self, rel: &str, explicit: bool) -> Claim {
        self.claims
            .lock()
            .unwrap()
            .push((rel.to_string(), explicit));
        let name = rel.rsplit('/').next().unwrap();
        if name == "helpers.py" {
            Claim::Support
        } else if name == "both.py" {
            Claim::TestsAndSupport
        } else if name.ends_with("_check.py")
            || ((explicit || self.any_name) && extension(name) == Some("py"))
        {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    fn recognize(&self, file: &SourceFile<'_>, support: &[SourceFile<'_>]) -> Recognition {
        let mut recognition = Recognition::default();
        for stmt in &file.module.body {
            if let ast::Stmt::FunctionDef(def) = stmt {
                let name = def.name.as_str();
                if name == "dynamic" {
                    recognition.refine = Some("tests made at import".to_string());
                }
                if name == "broken" {
                    recognition.refine = Some("the framework decides".to_string());
                    for (severity, code) in [
                        (Severity::Error, "static-error"),
                        (Severity::Warning, "static-lint"),
                    ] {
                        recognition.diagnostics.push(Diagnostic {
                            severity,
                            code: code.into(),
                            path: file.rel.to_project_path().into(),
                            span: Some(file.span(def.range())),
                            message: format!("{name} is broken"),
                        });
                    }
                }
                let mut case = Case::new(format!("{}::{name}", file.rel), self.adapter);
                case.span = Some(file.span(def.range()));
                case.keywords = support.iter().map(|s| s.rel.to_string()).collect();
                recognition.cases.push(case);
            }
        }
        recognition
    }

    fn settings(&self) -> Vec<Setting> {
        vec![Setting {
            name: "setting".to_string(),
            value: self.setting.clone(),
            origin: "default".to_string(),
        }]
    }

    fn context(&self) -> serde_json::Value {
        self.context.clone()
    }
}

/// Claims `*.txt` files and makes one case per line.
struct Lines;

impl Recognizer for Lines {
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        vec![project.cwd.to_path_buf()]
    }

    fn descend(&self, _: &Path) -> bool {
        true
    }

    fn claim(&self, rel: &str, _: bool) -> Claim {
        if extension(rel) == Some("txt") {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    fn recognize(&self, file: &SourceFile<'_>, _: &[SourceFile<'_>]) -> Recognition {
        assert!(file.module.body.is_empty());
        let cases = file
            .source
            .lines()
            .map(|line| Case::new(format!("{}::{line}", file.rel), "lines"))
            .collect();
        Recognition {
            cases,
            ..Recognition::default()
        }
    }
}

/// Reports `dynamic[1]` and `dynamic[2]` for every file it is asked about.
#[derive(Default)]
struct FakeRefiner {
    requests: Mutex<Vec<RefineRequest>>,
    fail: bool,
}

impl FakeRefiner {
    fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl Refiner for FakeRefiner {
    fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
        self.requests.lock().unwrap().push(request.clone());
        if self.fail {
            return Err(RefineError("the worker died".to_string()));
        }
        let cases = request
            .files
            .iter()
            .flat_map(|file| {
                ["dynamic[1]", "dynamic[2]"].map(|name| {
                    let mut case = Case::new(format!("{file}::{name}"), &request.adapter);
                    case.certainty = Certainty::Refined;
                    case
                })
            })
            .collect();
        Ok(Refined {
            cases,
            diagnostics: Vec::new(),
            digests: None,
        })
    }
}

fn write(root: &Path, rel: &str, body: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn run(
    root: &Path,
    recognizers: &[Prepared],
    targets: &[PathBuf],
    options: Options,
    refiner: Option<&(dyn Refiner + Sync)>,
) -> Collection {
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    Collector::new(project, recognizers, options).collect(targets, refiner)
}

fn fake() -> Vec<Prepared> {
    vec![("fake", Box::new(Fake::new()))]
}

/// A fake interpreter in a venv: `<dir>/venv/bin/python`, its
/// `pyvenv.cfg` and an empty `site-packages`.
fn interpreter(dir: &Path) -> PathBuf {
    write(dir, "venv/pyvenv.cfg", "home = /usr/bin\n");
    fs::create_dir_all(dir.join("venv/lib/python3.13/site-packages")).unwrap();
    write(dir, "venv/bin/python", "");
    dir.join("venv/bin/python")
}

/// `python` with no environment variables and no framework arguments.
fn environment(python: &Path) -> Environment {
    Environment {
        interpreter: Some(python.to_path_buf()),
        framework_args: Vec::new(),
        variables: Some(Vec::new()),
    }
}

/// Collects from the root with a cache in `cache` and `environment`.
fn run_cached(
    root: &Path,
    recognizers: &[Prepared],
    cache: &Path,
    environment: Environment,
    refiner: Option<&(dyn Refiner + Sync)>,
) -> Collection {
    run_cached_with(root, recognizers, cache, environment, refiner, &[])
}

/// [`run_cached`], with `framework_args` as the project's arguments after
/// `--`.
fn run_cached_with(
    root: &Path,
    recognizers: &[Prepared],
    cache: &Path,
    environment: Environment,
    refiner: Option<&(dyn Refiner + Sync)>,
    framework_args: &[String],
) -> Collection {
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args,
    };
    let options = Options {
        cache_dir: Some(cache.to_path_buf()),
        version: "0.0.1".to_string(),
        ..Options::default()
    };
    Collector::new(project, recognizers, options)
        .with_environment(environment)
        .collect(&[], refiner)
}

/// The entries in a cache directory, for the `fake` adapter.
fn entries(cache: &Path) -> usize {
    fs::read_dir(cache.join("refine/fake")).map_or(0, Iterator::count)
}

fn ids(collection: &Collection) -> Vec<&str> {
    collection.cases.iter().map(|c| c.id.as_str()).collect()
}

fn codes_of(collection: &Collection) -> Vec<(DiagnosticCode, Severity)> {
    collection
        .diagnostics
        .iter()
        .map(|d| (d.code.clone(), d.severity))
        .collect()
}

fn case<'c>(collection: &'c Collection, id: &str) -> &'c Case {
    collection
        .cases
        .iter()
        .find(|c| c.id.as_str() == id)
        .unwrap_or_else(|| panic!("no case {id}"))
}

#[test]
fn walks_sorted_and_honours_descend() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "b_check.py", "def test_b(): pass\n");
    write(root, "a/z_check.py", "def test_z(): pass\n");
    write(root, "a/sub/y_check.py", "def test_y(): pass\n");
    write(
        root,
        "a_check.py",
        "def test_a1(): pass\ndef test_a2(): pass\n",
    );
    write(root, "skip/x_check.py", "def test_x(): pass\n");
    write(root, ".hidden/w_check.py", "def test_w(): pass\n");
    write(root, "notes.py", "def test_ignored(): pass\n");
    write(root, "README.md", "hello\n");

    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        ids(&collection),
        [
            "a/sub/y_check.py::test_y",
            "a/z_check.py::test_z",
            "a_check.py::test_a1",
            "a_check.py::test_a2",
            "b_check.py::test_b",
        ]
    );
    assert!(collection.diagnostics.is_empty(), "{collection:?}");
    let span = case(&collection, "a_check.py::test_a2")
        .span
        .as_ref()
        .unwrap();
    assert_eq!((span.path.as_str(), span.line), ("a_check.py", Some(2)));
}

#[test]
fn explicit_files_are_claimed_as_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "plain.py", "def test_p(): pass\n");
    write(root, "tests/one_check.py", "def test_one(): pass\n");
    let recognizer = Fake::new();
    let claims = recognizer.claims.clone();
    let recognizers: Vec<Prepared> = vec![("fake", Box::new(recognizer))];

    let targets = [PathBuf::from("plain.py"), root.join("tests/./../tests")];
    let collection = run(root, &recognizers, &targets, Options::default(), None);
    assert_eq!(
        ids(&collection),
        ["plain.py::test_p", "tests/one_check.py::test_one"]
    );
    let claims = claims.lock().unwrap();
    assert!(claims.contains(&("plain.py".to_string(), true)));
    assert!(claims.contains(&("tests/one_check.py".to_string(), false)));
}

/// pytest's `normalize_collection_arguments` drops an explicit file below
/// an explicit directory, wherever it comes: the file is collected only if
/// the directory's walk collects it.
#[test]
fn an_explicit_file_below_an_explicit_directory_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/plain.py", "def test_p(): pass\n");
    write(root, "tests/one_check.py", "def test_one(): pass\n");
    for targets in [
        [root.join("tests"), root.join("tests/plain.py")],
        [root.join("tests/plain.py"), root.join("tests")],
    ] {
        let collection = run(root, &fake(), &targets, Options::default(), None);
        assert_eq!(ids(&collection), ["tests/one_check.py::test_one"]);
        assert!(collection.diagnostics.is_empty(), "{collection:?}");
    }
    let targets = [root.join("tests/plain.py")];
    let collection = run(root, &fake(), &targets, Options::default(), None);
    assert_eq!(ids(&collection), ["tests/plain.py::test_p"]);
}

/// Finding 28: `tests/sub tests` listed `sub` first; pytest drops
/// `tests/sub` and follows `tests`.
#[test]
fn explicit_paths_are_normalized_as_pytest_does() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/a_check.py", "def test_a(): pass\n");
    write(root, "tests/b_check.py", "def test_b(): pass\n");
    write(root, "tests/pkg/d_check.py", "def test_d(): pass\n");
    write(root, "tests/sub/c_check.py", "def test_c(): pass\n");
    let collect = |targets: &[&str]| {
        let targets: Vec<PathBuf> = targets.iter().map(PathBuf::from).collect();
        let collection = run(root, &fake(), &targets, Options::default(), None);
        assert!(collection.diagnostics.is_empty(), "{collection:?}");
        ids(&collection)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let walk = [
        "tests/a_check.py::test_a",
        "tests/b_check.py::test_b",
        "tests/pkg/d_check.py::test_d",
        "tests/sub/c_check.py::test_c",
    ];
    assert_eq!(collect(&["tests/sub", "tests"]), walk);
    assert_eq!(collect(&["tests/b_check.py", "tests"]), walk);
    assert_eq!(collect(&["tests", "tests/./sub/", "tests"]), walk);
    // Paths given as they come, with `..` and repeats resolved.
    assert_eq!(
        collect(&[
            "tests/b_check.py",
            "tests/sub/../a_check.py",
            "tests/b_check.py"
        ]),
        ["tests/b_check.py::test_b", "tests/a_check.py::test_a"]
    );
    // A missing path below a directory is still an error, as in pytest,
    // and doesn't stop the directory subsuming what comes after it.
    let targets = [
        root.join("tests/b_check.py"),
        root.join("tests/a_missing.py"),
        root.join("tests"),
    ];
    let collection = run(root, &fake(), &targets, Options::default(), None);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NotFound, Severity::Error)]
    );
    assert_eq!(ids(&collection), walk);
}

/// A path spelled through another name of a directory of the project (as
/// macOS's `/tmp` is `/private/tmp`) is spelled from the root, so its ids
/// and support files are the project's.
#[cfg(unix)]
#[test]
fn a_path_spelled_through_a_symlink_is_spelled_from_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    write(&root, "helpers.py", "X = 1\n");
    write(&root, "tests/t_check.py", "def test_t(): pass\n");
    let alias_dir = tempfile::tempdir().unwrap();
    let alias = alias_dir.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    let targets = [alias.join("tests/t_check.py")];
    let collection = run(&root, &fake(), &targets, Options::default(), None);
    assert_eq!(ids(&collection), ["tests/t_check.py::test_t"]);
    assert_eq!(collection.cases[0].keywords, ["helpers.py"]);
}

#[test]
fn missing_and_unclaimed_targets_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "README.md", "hello\n");
    let targets = [PathBuf::from("nope.py"), PathBuf::from("README.md")];
    let collection = run(root, &fake(), &targets, Options::default(), None);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    assert_eq!(
        codes_of(&collection),
        [
            (DiagnosticCode::NotFound, Severity::Error),
            (DiagnosticCode::Unclaimed, Severity::Error),
        ]
    );
    assert_eq!(collection.diagnostics[0].path, "nope.py");
    assert!(collection.diagnostics[0].message.contains("nope.py"));
    assert!(collection.has_errors());
}

#[test]
fn roots_are_walked_only_for_the_recognizers_that_list_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/t_check.py", "def test_t(): pass\n");
    write(root, "other/o_check.py", "def test_o(): pass\n");
    write(root, "other/notes.txt", "first\nsecond\n");
    let recognizers: Vec<Prepared> = vec![
        ("fake", Box::new(Fake::rooted(vec![root.join("tests")]))),
        ("lines", Box::new(Lines)),
    ];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(
        ids(&collection),
        [
            "tests/t_check.py::test_t",
            "other/notes.txt::first",
            "other/notes.txt::second",
        ]
    );
}

#[test]
fn non_python_files_reach_recognizers_with_an_empty_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "doc.txt", "def (:\nalpha\n");
    let recognizers: Vec<Prepared> = vec![("lines", Box::new(Lines))];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(ids(&collection), ["doc.txt::def (:", "doc.txt::alpha"]);
    assert_eq!(collection.diagnostics, [] as [aye_events::Diagnostic; 0]);
}

#[test]
fn support_files_are_handed_outermost_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "helpers.py", "X = 1\n");
    write(root, "tests/helpers.py", "Y = 1\n");
    write(root, "tests/unit/helpers.py", "Z = 1\n");
    write(root, "tests/unit/u_check.py", "def test_u(): pass\n");
    write(root, "tests/t_check.py", "def test_t(): pass\n");
    write(root, "elsewhere/helpers.py", "W = 1\n");

    // Walk only `tests/unit`: the support files above it are still found.
    let targets = [root.join("tests/unit")];
    let collection = run(root, &fake(), &targets, Options::default(), None);
    assert_eq!(ids(&collection), ["tests/unit/u_check.py::test_u"]);
    assert_eq!(
        case(&collection, "tests/unit/u_check.py::test_u").keywords,
        ["helpers.py", "tests/helpers.py", "tests/unit/helpers.py"]
    );

    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        case(&collection, "tests/t_check.py::test_t").keywords,
        ["helpers.py", "tests/helpers.py"]
    );
}

/// Finding 27: a file claimed as tests and support (pytest's `conftest.py`
/// matching `python_files`) yields its tests and is support for the files
/// below it, itself included.
#[test]
fn a_file_claimed_as_tests_and_support_is_both() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def test_a(): pass\n");
    write(root, "both.py", "def test_both(): pass\n");
    write(root, "helpers.py", "X = 1\n");
    write(root, "sub/both.py", "def test_sub(): pass\n");
    write(root, "sub/s_check.py", "def test_s(): pass\n");
    let keywords = |collection: &Collection, id: &str| case(collection, id).keywords.clone();

    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        ids(&collection),
        [
            "a_check.py::test_a",
            "both.py::test_both",
            "sub/both.py::test_sub",
            "sub/s_check.py::test_s",
        ]
    );
    assert!(collection.diagnostics.is_empty(), "{collection:?}");
    assert_eq!(
        keywords(&collection, "a_check.py::test_a"),
        ["both.py", "helpers.py"]
    );
    assert_eq!(
        keywords(&collection, "both.py::test_both"),
        ["both.py", "helpers.py"]
    );
    for id in ["sub/both.py::test_sub", "sub/s_check.py::test_s"] {
        assert_eq!(
            keywords(&collection, id),
            ["both.py", "helpers.py", "sub/both.py"],
            "{id}"
        );
    }

    // Above the walked root it is support only.
    let collection = run(root, &fake(), &[root.join("sub")], Options::default(), None);
    assert_eq!(
        ids(&collection),
        ["sub/both.py::test_sub", "sub/s_check.py::test_s"]
    );
    assert_eq!(
        keywords(&collection, "sub/s_check.py::test_s"),
        ["both.py", "helpers.py", "sub/both.py"]
    );

    // Named on the command line.
    let collection = run(
        root,
        &fake(),
        &[PathBuf::from("both.py")],
        Options::default(),
        None,
    );
    assert_eq!(ids(&collection), ["both.py::test_both"]);
    assert!(collection.diagnostics.is_empty(), "{collection:?}");

    // Parsed once: one syntax error, and left out of everyone's support.
    write(root, "both.py", "def (:\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::SyntaxError, Severity::Error)]
    );
    assert_eq!(collection.diagnostics[0].path, "both.py");
    assert_eq!(keywords(&collection, "a_check.py::test_a"), ["helpers.py"]);
    assert!(!ids(&collection).iter().any(|id| id.starts_with("both.py")));
}

/// A file read for tests and support that can't be read is reported once.
#[cfg(unix)]
#[test]
fn an_unreadable_file_claimed_as_tests_and_support_is_reported_once() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "both.py", "def test_both(): pass\n");
    write(root, "a_check.py", "def test_a(): pass\n");
    let both = root.join("both.py");
    fs::set_permissions(&both, fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root reads it anyway: nothing to check.
    if fs::read(&both).is_ok() {
        return;
    }
    let collection = run(root, &fake(), &[], Options::default(), None);
    fs::set_permissions(&both, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::Unreadable, Severity::Error)]
    );
    assert_eq!(collection.diagnostics[0].path, "both.py");
    assert_eq!(ids(&collection), ["a_check.py::test_a"]);
}

/// A file a recognizer is asked about only while listing support files
/// (its walk never reached it) yields no tests for that recognizer, whatever
/// the order the files come in.
#[test]
fn a_file_yields_tests_only_for_the_recognizers_whose_walk_reached_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/t_check.py", "def test_t(): pass\n");
    write(root, "zoo_check.py", "def test_z(): pass\n");
    let named = |adapter, roots| Fake {
        adapter,
        ..Fake::rooted(roots)
    };
    // `tests_only`, which walks `tests` alone, comes first: a case of its
    // in the root's file would win the id.
    let recognizers: Vec<Prepared> = vec![
        (
            "tests_only",
            Box::new(named("tests_only", vec![root.join("tests")])),
        ),
        (
            "everywhere",
            Box::new(named("everywhere", vec![root.to_path_buf()])),
        ),
    ];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    let adapters: Vec<(&str, &str)> = collection
        .cases
        .iter()
        .map(|c| (c.id.as_str(), c.adapter.as_str()))
        .collect();
    // Both walked `tests`; only `everywhere` walked the root.
    assert_eq!(
        adapters,
        [
            ("tests/t_check.py::test_t", "tests_only"),
            ("zoo_check.py::test_z", "everywhere")
        ]
    );
}

#[test]
fn syntax_errors_are_reported_and_yield_no_cases() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "bad_check.py",
        "def test_ok(): pass\n\ndef (:\n    pass\n",
    );
    write(root, "good_check.py", "def test_good(): pass\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(ids(&collection), ["good_check.py::test_good"]);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::SyntaxError, Severity::Error)]
    );
    let diagnostic = &collection.diagnostics[0];
    assert_eq!(diagnostic.path, "bad_check.py");
    assert!(
        diagnostic.message.starts_with("syntax error at 3:5: "),
        "{}",
        diagnostic.message
    );
    assert_eq!(diagnostic.span.as_ref().unwrap().line, Some(3));
    assert!(collection.has_errors());
}

#[test]
fn a_syntax_error_at_the_end_of_the_file_is_located() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "eof_check.py", "def test_a(): pass\ndef test_b(");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    let diagnostic = &collection.diagnostics[0];
    assert_eq!(diagnostic.code, DiagnosticCode::SyntaxError);
    assert!(diagnostic.message.starts_with("syntax error at 2:"));
}

#[test]
fn a_broken_support_file_is_reported_and_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "helpers.py", "X = 1\n");
    write(root, "tests/helpers.py", "def (:\n");
    write(root, "tests/t_check.py", "def test_t(): pass\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        case(&collection, "tests/t_check.py::test_t").keywords,
        ["helpers.py"]
    );
    assert_eq!(collection.diagnostics.len(), 1);
    assert_eq!(collection.diagnostics[0].path, "tests/helpers.py");
}

#[test]
fn a_byte_order_mark_parses() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "bom_check.py", "\u{feff}def test_bom(): pass\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(ids(&collection), ["bom_check.py::test_bom"]);
    assert_eq!(collection.diagnostics, [] as [aye_events::Diagnostic; 0]);
}

#[test]
fn a_file_that_is_not_utf8_is_refined() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "latin_check.py",
        b"# -*- coding: latin-1 -*-\ndef test_caf\xe9(): pass\nX = '\xe9'\n",
    );
    let refiner = FakeRefiner::default();
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(refiner.calls(), 1);
    assert_eq!(
        refiner.requests.lock().unwrap()[0].files,
        ["latin_check.py"]
    );
    assert_eq!(
        ids(&collection),
        ["latin_check.py::dynamic[1]", "latin_check.py::dynamic[2]"]
    );
    assert!(collection.diagnostics.is_empty(), "{collection:?}");

    // The lossy text doesn't parse, which is not reported: only the
    // framework knows whether the real file does.
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NotRefined, Severity::Info)]
    );
    assert!(
        collection.diagnostics[0]
            .message
            .contains("not valid UTF-8")
    );
}

#[test]
fn the_higher_priority_adapter_wins_a_duplicate_id() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def test_a(): pass\n");
    let named = |adapter| Fake {
        adapter,
        ..Fake::new()
    };
    let recognizers: Vec<Prepared> = vec![
        ("first", Box::new(named("first"))),
        ("second", Box::new(named("second"))),
    ];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(ids(&collection), ["a_check.py::test_a"]);
    assert_eq!(collection.cases[0].adapter, "first");
    // Quietly: listing unittest before pytest, which both collect a
    // `TestCase`'s methods, is how a project runs those natively.
    assert!(collection.diagnostics.is_empty(), "{collection:?}");

    let strict = Options {
        strict: true,
        ..Options::default()
    };
    let collection = run(root, &recognizers, &[], strict, None);
    assert!(!collection.has_errors(), "{collection:?}");

    // An adapter that repeats an id still says so, whoever wins it.
    write(
        root,
        "a_check.py",
        "def test_a(): pass\ndef test_a(): pass\n",
    );
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(ids(&collection), ["a_check.py::test_a"]);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::DuplicateId, Severity::Warning)]
    );
    assert!(
        collection.diagnostics[0]
            .message
            .starts_with("first collects `a_check.py::test_a` more than once"),
        "{collection:?}"
    );
}

#[test]
fn an_adapter_repeating_an_id_keeps_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "a_check.py",
        "def test_a(): pass\ndef test_a(): pass\n",
    );
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(ids(&collection), ["a_check.py::test_a"]);
    assert_eq!(
        case(&collection, "a_check.py::test_a")
            .span
            .as_ref()
            .unwrap()
            .line,
        Some(1)
    );
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::DuplicateId, Severity::Warning)]
    );
    assert_eq!(
        collection.diagnostics[0].span.as_ref().unwrap().line,
        Some(2)
    );
    // Finding 15: the framework selects by id, so both run; the warning
    // must not say that only the first is kept.
    let message = &collection.diagnostics[0].message;
    assert!(
        message.contains("running it runs every test with that id"),
        "{message}"
    );
    assert!(!message.contains("keeping"), "{message}");
}

#[test]
fn a_test_file_without_tests_is_noted() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "empty_check.py", "X = 1\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NoTests, Severity::Info)]
    );
    let strict = Options {
        strict: true,
        ..Options::default()
    };
    assert!(!run(root, &fake(), &[], strict, None).has_errors());
}

/// A file an adapter already reported about (a module the framework skipped
/// while importing it, a file it ignores) gets no `no-tests` note beside
/// that report, which says why it has none.
#[test]
fn a_file_whose_adapter_says_why_it_has_no_tests_gets_no_note() {
    struct Replies(Option<&'static str>);
    impl Refiner for Replies {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            Ok(Refined {
                cases: Vec::new(),
                diagnostics: self
                    .0
                    .map(|code| Diagnostic {
                        severity: Severity::Info,
                        code: code.into(),
                        path: request.files[0].clone().into(),
                        span: None,
                        message: "…".to_string(),
                    })
                    .into_iter()
                    .collect(),
                digests: None,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "dyn_check.py", "def dynamic():\n    pass\n");
    for code in ["skipped", "ignored"] {
        let refiner = Replies(Some(code));
        let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
        assert_eq!(codes_of(&collection), [(code.into(), Severity::Info)]);
    }
    // Nothing said: the note.
    let refiner = Replies(None);
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NoTests, Severity::Info)]
    );
}

/// A file in a directory an adapter reported about (a conftest pytest
/// skipped the directory for, with one `skipped` info on the directory)
/// gets no `no-tests` note either; a file elsewhere does.
#[test]
fn a_file_in_a_directory_the_adapter_reported_about_gets_no_note() {
    struct SkipsSub;
    impl Refiner for SkipsSub {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            let skips = request.files.iter().any(|f| f.as_str().starts_with("sub/"));
            Ok(Refined {
                cases: Vec::new(),
                diagnostics: skips
                    .then(|| Diagnostic {
                        severity: Severity::Info,
                        code: DiagnosticCode::Skipped,
                        path: "sub".into(),
                        span: None,
                        message: "could not import 'missing'".to_string(),
                    })
                    .into_iter()
                    .collect(),
                digests: None,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for rel in ["sub/a_check.py", "sub/deep/b_check.py", "subway/c_check.py"] {
        write(root, rel, "def dynamic():\n    pass\n");
    }
    let collection = run(root, &fake(), &[], Options::default(), Some(&SkipsSub));
    let notes: Vec<(DiagnosticCode, &str)> = collection
        .diagnostics
        .iter()
        .map(|d| (d.code.clone(), d.path.as_str()))
        .collect();
    assert_eq!(
        notes,
        [
            (DiagnosticCode::NoTests, "subway/c_check.py"),
            (DiagnosticCode::Skipped, "sub")
        ]
    );
}

/// A file only recognizers claiming files whatever their names claimed
/// (pytest's doctest modules: every module) may well hold no tests: no
/// `no-tests` note for it. One claimed by name (`test_*.py`) gets one.
#[test]
fn files_claimed_whatever_their_names_may_hold_no_tests() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pkg/plain.py", "X = 1\n");
    write(root, "pkg/empty_check.py", "X = 1\n");
    write(root, "notes.txt", "");
    let every = Fake {
        any_name: true,
        ..Fake::new()
    };
    let recognizers: Vec<Prepared> = vec![("every", Box::new(every)), ("lines", Box::new(Lines))];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    // `empty_check.py` is claimed by name too, by the same recognizer: one
    // that claims any module isn't surprised. `notes.txt` is claimed by
    // `Lines` for any `.txt` name.
    assert!(collection.diagnostics.is_empty(), "{collection:?}");

    let recognizers: Vec<Prepared> = vec![
        (
            "every",
            Box::new(Fake {
                any_name: true,
                ..Fake::new()
            }),
        ),
        ("named", Box::new(Fake::new())),
    ];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NoTests, Severity::Info)]
    );
    assert_eq!(collection.diagnostics[0].path, "pkg/empty_check.py");
    assert_eq!(
        collection.diagnostics[0].message,
        "claimed for tests by every, named, but has none"
    );
}

/// A package's `__init__.py` comes first in its directory, as pytest's
/// `Package` collects it, then files and directories by name.
#[test]
fn a_packages_init_comes_first_in_the_walk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        "pkg/__init__.py",
        "pkg/Alpha.py",
        "pkg/1st.py",
        "pkg/beta.py",
        "pkg/Sub/__init__.py",
        "pkg/Sub/Zed.py",
    ] {
        write(root, file, "def test_x(): pass\n");
    }
    let every = Fake {
        any_name: true,
        ..Fake::new()
    };
    let recognizers: Vec<Prepared> = vec![("every", Box::new(every))];
    let collection = run(root, &recognizers, &[], Options::default(), None);
    assert_eq!(
        ids(&collection),
        [
            "pkg/__init__.py::test_x",
            "pkg/1st.py::test_x",
            "pkg/Alpha.py::test_x",
            "pkg/Sub/__init__.py::test_x",
            "pkg/Sub/Zed.py::test_x",
            "pkg/beta.py::test_x",
        ]
    );
}

/// Finding C27: a directory a recognizer wants to enter but can't read
/// fails collection, as pytest's `PermissionError` does.
#[cfg(unix)]
#[test]
fn an_unreadable_directory_is_an_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def test_a(): pass\n");
    write(root, "locked/l_check.py", "def test_l(): pass\n");
    write(root, "skip/s_check.py", "def test_s(): pass\n");
    let mode = |path: &str, mode| {
        fs::set_permissions(root.join(path), fs::Permissions::from_mode(mode)).unwrap();
    };
    mode("locked", 0o000);
    mode("skip", 0o000);
    let readable = fs::read_dir(root.join("locked")).is_ok();
    let collection = run(root, &fake(), &[], Options::default(), None);
    mode("locked", 0o755);
    mode("skip", 0o755);
    if readable {
        eprintln!("skipping: permissions aren't enforced here (root?)");
        return;
    }
    assert_eq!(ids(&collection), ["a_check.py::test_a"]);
    // `skip` is never entered, so it can't fail.
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::Unreadable, Severity::Error)]
    );
    assert_eq!(collection.diagnostics[0].path, "locked");
    assert!(collection.has_errors());
}

#[test]
fn refined_cases_replace_the_static_ones() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def test_a(): pass\n");
    write(
        root,
        "b_check.py",
        "def dynamic(): pass\ndef test_b(): pass\n",
    );
    write(root, "c_check.py", "def dynamic(): pass\n");
    let refiner = FakeRefiner::default();
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(
        ids(&collection),
        [
            "a_check.py::test_a",
            "b_check.py::dynamic[1]",
            "b_check.py::dynamic[2]",
            "c_check.py::dynamic[1]",
            "c_check.py::dynamic[2]",
        ]
    );
    assert_eq!(collection.refined_files, 2);
    assert_eq!(
        case(&collection, "b_check.py::dynamic[1]").certainty,
        Certainty::Refined
    );
    assert_eq!(
        *refiner.requests.lock().unwrap(),
        [RefineRequest {
            adapter: "fake".to_string(),
            files: vec![
                ProjectPath::new("b_check.py").unwrap(),
                ProjectPath::new("c_check.py").unwrap(),
            ],
            context: serde_json::json!({"rootdir": "/fake"}),
            digests_only: false,
        }]
    );
    assert!(collection.diagnostics.is_empty(), "{collection:?}");
}

#[test]
fn refinement_only_replaces_the_adapter_that_asked() {
    /// Like `Fake`, but never asks for refinement.
    struct Static(Fake);
    impl Recognizer for Static {
        fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
            self.0.roots(project)
        }
        fn descend(&self, dir: &Path) -> bool {
            self.0.descend(dir)
        }
        fn claim(&self, rel: &str, explicit: bool) -> Claim {
            self.0.claim(rel, explicit)
        }
        fn recognize(&self, file: &SourceFile<'_>, support: &[SourceFile<'_>]) -> Recognition {
            let mut recognition = self.0.recognize(file, support);
            recognition.refine = None;
            for case in &mut recognition.cases {
                case.id = TestId::new(format!("{}::static", case.id));
            }
            recognition
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "d_check.py", "def dynamic(): pass\n");
    let recognizers: Vec<Prepared> = vec![
        ("fake", Box::new(Fake::new())),
        ("static", Box::new(Static(Fake::new()))),
    ];
    let refiner = FakeRefiner::default();
    let collection = run(root, &recognizers, &[], Options::default(), Some(&refiner));
    assert_eq!(
        ids(&collection),
        [
            "d_check.py::dynamic[1]",
            "d_check.py::dynamic[2]",
            "d_check.py::dynamic::static",
        ]
    );
    assert_eq!(refiner.calls(), 1);
    assert_eq!(collection.refined_files, 1);
}

#[test]
fn stray_refined_cases_are_kept_after_every_file() {
    struct Wide;
    impl Refiner for Wide {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            let mut cases: Vec<Case> = request
                .files
                .iter()
                .map(|f| Case::new(format!("{f}::found"), "fake"))
                .collect();
            cases.push(Case::new("elsewhere.py::stray", "fake"));
            Ok(Refined {
                cases,
                diagnostics: Vec::new(),
                digests: None,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def dynamic(): pass\n");
    write(root, "b_check.py", "def test_b(): pass\n");
    let collection = run(root, &fake(), &[], Options::default(), Some(&Wide));
    assert_eq!(
        ids(&collection),
        [
            "a_check.py::found",
            "b_check.py::test_b",
            "elsewhere.py::stray"
        ]
    );
}

/// Finding F7: importing a package (unittest's `load_tests` discovering
/// another pattern) can make tests in files of it the walk didn't claim.
/// They go where their files fall in the walk, at the package's place, not
/// after every file; and they are the package's, which has tests then.
#[test]
fn stray_refined_cases_in_a_requested_package_keep_their_place() {
    /// Claims `__init__.py` and `*_check.py`: one case per top-level
    /// function, refined when one is named `dynamic`.
    struct Packages;
    impl Recognizer for Packages {
        fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
            vec![project.cwd.to_path_buf()]
        }
        fn descend(&self, _: &Path) -> bool {
            true
        }
        fn claim(&self, rel: &str, _: bool) -> Claim {
            let name = rel.rsplit('/').next().unwrap();
            if name == "__init__.py" || name.ends_with("_check.py") {
                Claim::Tests
            } else {
                Claim::Ignore
            }
        }
        fn recognize(&self, file: &SourceFile<'_>, _: &[SourceFile<'_>]) -> Recognition {
            let mut recognition = Recognition::default();
            for stmt in &file.module.body {
                if let ast::Stmt::FunctionDef(def) = stmt {
                    if def.name.as_str() == "dynamic" {
                        recognition.refine = Some("tests made at import".to_string());
                    } else {
                        let id = format!("{}::{}", file.rel, def.name);
                        recognition.cases.push(Case::new(id, "packages"));
                    }
                }
            }
            recognition
        }
    }
    /// Answers with tests in files it wasn't asked about.
    struct Discovering;
    impl Refiner for Discovering {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            assert_eq!(
                request.files,
                ["pkg/__init__.py", "pkg/m_check.py", "pkg/sub/__init__.py"]
            );
            Ok(Refined {
                cases: [
                    "pkg/m_check.py::found",
                    "pkg/x_other.py::found",
                    "pkg/sub/c_other.py::found",
                    "elsewhere.py::stray",
                    "pkg/b_other.py::found",
                    "pkg/b_other.py::again",
                ]
                .into_iter()
                .map(|id| Case::new(id, "packages"))
                .collect(),
                diagnostics: Vec::new(),
                digests: None,
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a_check.py", "def test_a(): pass\n");
    write(root, "pkg/__init__.py", "def dynamic(): pass\n");
    write(root, "pkg/m_check.py", "def dynamic(): pass\n");
    write(root, "pkg/sub/__init__.py", "def dynamic(): pass\n");
    write(root, "z_check.py", "def test_z(): pass\n");
    let packages: Vec<Prepared> = vec![("packages", Box::new(Packages))];
    let collection = run(root, &packages, &[], Options::default(), Some(&Discovering));
    assert_eq!(
        ids(&collection),
        [
            "a_check.py::test_a",
            "pkg/b_other.py::found",
            "pkg/b_other.py::again",
            "pkg/m_check.py::found",
            "pkg/sub/c_other.py::found",
            "pkg/x_other.py::found",
            "z_check.py::test_z",
            "elsewhere.py::stray",
        ]
    );
    // The packages' `__init__.py` made those tests: neither has none.
    assert_eq!(codes_of(&collection), []);
}

#[test]
fn refinement_failure_keeps_the_static_guess() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        root,
        "d_check.py",
        "def dynamic(): pass\ndef test_d(): pass\n",
    );
    let refiner = FakeRefiner::failing();
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(
        ids(&collection),
        ["d_check.py::dynamic", "d_check.py::test_d"]
    );
    assert_eq!(collection.refined_files, 0);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::RefineFailed, Severity::Warning)]
    );
    assert!(
        collection.diagnostics[0]
            .message
            .contains("the worker died")
    );
}

#[test]
fn without_a_refiner_the_static_guess_stays() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "d_check.py", "def dynamic(): pass\n");
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(ids(&collection), ["d_check.py::dynamic"]);
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::NotRefined, Severity::Info)]
    );
    let message = &collection.diagnostics[0].message;
    assert!(message.contains("tests made at import"), "{message}");
    // Finding 89: the reason is that refinement is off, not a lack of
    // workers.
    assert!(message.contains("but refinement is off"), "{message}");
    assert!(!message.contains("worker"), "{message}");
}

/// Residue B7: a generator test got the recognizer's static error and then
/// pytest's collection error from refinement, two errors for the one
/// pytest reports.
#[test]
fn a_refined_file_keeps_only_the_frameworks_errors() {
    /// Reports a collection error for every file it is asked about.
    struct Failing;
    impl Refiner for Failing {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            let diagnostics = request
                .files
                .iter()
                .map(|file| Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::CollectionError,
                    path: file.clone().into(),
                    span: None,
                    message: "the framework could not collect it".to_string(),
                })
                .collect();
            Ok(Refined {
                cases: Vec::new(),
                diagnostics,
                digests: None,
            })
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "b_check.py", "def broken(): yield 1\n");
    // The framework's error replaces the static one; warnings stay.
    let collection = run(root, &fake(), &[], Options::default(), Some(&Failing));
    assert_eq!(
        codes_of(&collection),
        [
            (DiagnosticCode::from("static-lint"), Severity::Warning),
            (DiagnosticCode::CollectionError, Severity::Error)
        ]
    );
    assert_eq!(ids(&collection), [] as [&str; 0]);
    // A framework that collects the file has the final word too.
    let refiner = FakeRefiner::default();
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::from("static-lint"), Severity::Warning)]
    );
    assert_eq!(
        ids(&collection),
        ["b_check.py::dynamic[1]", "b_check.py::dynamic[2]"]
    );
    assert!(!collection.has_errors());
    // Without refinement, or when it fails, the static error stands.
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        codes_of(&collection),
        [
            (DiagnosticCode::from("static-error"), Severity::Error),
            (DiagnosticCode::from("static-lint"), Severity::Warning),
            (DiagnosticCode::NotRefined, Severity::Info)
        ]
    );
    let refiner = FakeRefiner::failing();
    let collection = run(root, &fake(), &[], Options::default(), Some(&refiner));
    assert_eq!(
        codes_of(&collection),
        [
            (DiagnosticCode::from("static-error"), Severity::Error),
            (DiagnosticCode::from("static-lint"), Severity::Warning),
            (DiagnosticCode::RefineFailed, Severity::Warning)
        ]
    );
    assert_eq!(ids(&collection), ["b_check.py::broken"]);
}

#[test]
fn refinement_is_cached_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "helpers.py", "X = 1\n");
    write(root, "d_check.py", "def dynamic(): pass\n");
    write(root, "e_check.py", "def dynamic(): pass\n");
    let collect = |refiner: &FakeRefiner, recognizers: &[Prepared], version: &str| {
        let project = Project {
            root,
            cwd: root,
            args: &[],
            config_file: None,
            framework_args: &[],
        };
        let options = Options {
            cache_dir: Some(cache.path().join("cache")),
            version: version.to_string(),
            ..Options::default()
        };
        let collection = Collector::new(project, recognizers, options)
            .with_environment(environment(&python))
            .collect(&[], Some(refiner));
        assert_eq!(collection.refined_files, 2);
        assert_eq!(
            ids(&collection),
            [
                "d_check.py::dynamic[1]",
                "d_check.py::dynamic[2]",
                "e_check.py::dynamic[1]",
                "e_check.py::dynamic[2]",
            ]
        );
        std::mem::take(&mut *refiner.requests.lock().unwrap())
    };

    let refiner = FakeRefiner::default();
    let requests = collect(&refiner, &fake(), "0.0.1");
    assert_eq!(requests[0].files, ["d_check.py", "e_check.py"]);
    assert_eq!(entries(&cache.path().join("cache")), 2);

    // Nothing changed: no worker.
    assert_eq!(
        collect(&refiner, &fake(), "0.0.1"),
        [] as [aye_engine::RefineRequest; 0]
    );
    // No refiner at all: the cache still answers.
    let collection = run_cached(
        root,
        &fake(),
        &cache.path().join("cache"),
        environment(&python),
        None,
    );
    assert_eq!(collection.refined_files, 2);
    assert_eq!(collection.diagnostics, [] as [aye_events::Diagnostic; 0]);

    // A changed file is refined again, alone.
    write(root, "e_check.py", "def dynamic(): pass\n# edited\n");
    let requests = collect(&refiner, &fake(), "0.0.1");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].files, ["e_check.py"]);

    // A changed support file invalidates every file below it.
    write(root, "helpers.py", "X = 2\n");
    let requests = collect(&refiner, &fake(), "0.0.1");
    assert_eq!(requests[0].files, ["d_check.py", "e_check.py"]);
    assert_eq!(
        collect(&refiner, &fake(), "0.0.1"),
        [] as [aye_engine::RefineRequest; 0]
    );

    // So do another version of aye and other settings.
    assert_eq!(collect(&refiner, &fake(), "0.0.2").len(), 1);
    let mut other = Fake::new();
    other.setting = "2".to_string();
    let recognizers: Vec<Prepared> = vec![("fake", Box::new(other))];
    assert_eq!(collect(&refiner, &recognizers, "0.0.1").len(), 1);
}

/// Without an interpreter to key on, nothing is cached (the safe default).
#[test]
fn without_an_interpreter_there_is_no_cache() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    write(root, "d_check.py", "def dynamic(): pass\n");
    let options = || Options {
        cache_dir: Some(cache.path().to_path_buf()),
        ..Options::default()
    };
    let refiner = FakeRefiner::default();
    run(root, &fake(), &[], options(), Some(&refiner));
    run(root, &fake(), &[], options(), Some(&refiner));
    assert_eq!(refiner.calls(), 2);
    assert!(!cache.path().join("refine").exists());

    // Nor with an interpreter whose packages can't be found.
    let bare = cache.path().join("bare/python");
    write(cache.path(), "bare/python", "");
    run_cached(
        root,
        &fake(),
        cache.path(),
        environment(&bare),
        Some(&refiner),
    );
    assert_eq!(refiner.calls(), 3);
    assert_eq!(entries(cache.path()), 0);
}

/// Finding 63: entries made by another interpreter, other packages, other
/// environment variables or framework arguments were reused.
#[test]
fn the_interpreter_and_the_environment_are_in_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    let refiner = FakeRefiner::default();
    let calls = |environment: Environment| {
        run_cached(root, &fake(), cache.path(), environment, Some(&refiner));
        std::mem::take(&mut *refiner.requests.lock().unwrap()).len()
    };
    let with = |variables: &[(&str, &str)], args: &[&str]| Environment {
        variables: Some(
            variables
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect(),
        ),
        framework_args: args.iter().map(|a| (*a).to_string()).collect(),
        ..environment(&python)
    };

    assert_eq!(calls(with(&[("BACKENDS", "sqlite")], &[])), 1);
    assert_eq!(calls(with(&[("BACKENDS", "sqlite")], &[])), 0);
    // Another value, another variable: refined again.
    assert_eq!(calls(with(&[("BACKENDS", "sqlite,pg")], &[])), 1);
    assert_eq!(
        calls(with(&[("BACKENDS", "sqlite"), ("RUN_SLOW", "1")], &[])),
        1
    );
    // The shell's bookkeeping doesn't count.
    assert_eq!(
        calls(with(
            &[("BACKENDS", "sqlite"), ("OLDPWD", "/x"), ("SHLVL", "3")],
            &[]
        )),
        0
    );
    // Framework arguments count.
    assert_eq!(
        calls(with(&[("BACKENDS", "sqlite")], &["--backends=pg"])),
        1
    );
    assert_eq!(
        calls(with(&[("BACKENDS", "sqlite")], &["--backends=pg"])),
        0
    );

    // Installing a package, or editing a .pth file, changes the key.
    let site = cache.path().join("venv/lib/python3.13/site-packages");
    fs::create_dir_all(site.join("plugin-1.0.dist-info")).unwrap();
    assert_eq!(calls(environment(&python)), 1);
    assert_eq!(calls(environment(&python)), 0);
    write(&site, "project.pth", "/src\n");
    assert_eq!(calls(environment(&python)), 1);
    write(&site, "project.pth", "/elsewhere\n");
    assert_eq!(calls(environment(&python)), 1);
    // So does the venv's configuration...
    write(
        cache.path(),
        "venv/pyvenv.cfg",
        "home = /opt/python3.14/bin\n",
    );
    assert_eq!(calls(environment(&python)), 1);
    // ...and another interpreter.
    let other = cache.path().join("other");
    let second = interpreter(&other);
    assert_eq!(calls(environment(&second)), 1);
    assert_eq!(calls(environment(&python)), 0);
}

/// A file claimed as tests and support is refined and cached like any test
/// file, and its content keys the files below it.
#[test]
fn a_file_claimed_as_tests_and_support_keys_itself_and_the_files_below() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "both.py", "def dynamic(): pass\n");
    write(root, "sub/d_check.py", "def dynamic(): pass\n");
    let refiner = FakeRefiner::default();
    let files = || {
        let collection = run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        assert_eq!(collection.refined_files, 2);
        std::mem::take(&mut *refiner.requests.lock().unwrap())
            .into_iter()
            .flat_map(|r| r.files)
            .collect::<Vec<_>>()
    };
    assert_eq!(files(), ["both.py", "sub/d_check.py"]);
    assert_eq!(files(), [] as [&str; 0]);
    write(root, "both.py", "def dynamic(): pass\n# edited\n");
    assert_eq!(files(), ["both.py", "sub/d_check.py"]);
    write(root, "sub/d_check.py", "def dynamic(): pass\n# edited\n");
    assert_eq!(files(), ["sub/d_check.py"]);
}

/// Finding 32: the arguments after `--` reach refinement through the
/// adapters (the project's `framework_args`), so they are in the key, and
/// one naming a project plugin turns the cache off.
#[test]
fn the_project_framework_args_are_in_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    write(root, "myplugins/params.py", "");
    let refiner = FakeRefiner::default();
    let calls = |args: &[&str]| {
        let args: Vec<String> = args.iter().map(|a| (*a).to_string()).collect();
        run_cached_with(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
            &args,
        );
        std::mem::take(&mut *refiner.requests.lock().unwrap()).len()
    };
    assert_eq!(calls(&[]), 1);
    assert_eq!(calls(&[]), 0);
    assert_eq!(calls(&["--backends=sqlite,pg"]), 1);
    assert_eq!(calls(&["--backends=sqlite,pg"]), 0);
    // Arguments are told apart, not just joined.
    assert_eq!(calls(&["--backends=sqlite", ",pg"]), 1);
    assert_eq!(calls(&["--backends=sqlite,pg", "-v"]), 1);
    assert_eq!(calls(&[]), 0);
    assert_eq!(entries(cache.path()), 4);
    // A project plugin's content isn't in the key: no cache at all.
    assert_eq!(calls(&["-p", "myplugins.params"]), 1);
    assert_eq!(calls(&["-p", "myplugins.params"]), 1);
    assert_eq!(entries(cache.path()), 4);
}

/// Findings 62 and 63: a failed or environment-dependent refinement was
/// cached, so tests disappeared until the file changed.
#[test]
fn only_clean_results_are_cached() {
    /// Reports what `reply` says for every request.
    struct Replies(fn(&RefineRequest) -> Refined);
    impl Refiner for Replies {
        fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
            Ok((self.0)(request))
        }
    }
    fn diagnostic(severity: Severity, code: &str, path: &str) -> Diagnostic {
        Diagnostic {
            severity,
            code: code.into(),
            path: path.into(),
            span: None,
            message: "…".to_string(),
        }
    }
    fn cases(request: &RefineRequest) -> Vec<Case> {
        request
            .files
            .iter()
            .map(|f| Case::new(format!("{f}::dynamic[1]"), "fake"))
            .collect()
    }
    type Reply = fn(&RefineRequest) -> Refined;
    let replies: [(&str, Reply); 6] = [
        ("an import error", |r| Refined {
            cases: Vec::new(),
            diagnostics: vec![diagnostic(
                Severity::Error,
                "collection-error",
                r.files[0].as_str(),
            )],
            digests: None,
        }),
        ("a module-level skip", |r| Refined {
            cases: Vec::new(),
            diagnostics: vec![diagnostic(Severity::Info, "skipped", r.files[0].as_str())],
            digests: None,
        }),
        ("no tests", |_| Refined::default()),
        ("a broken conftest elsewhere", |r| Refined {
            cases: cases(r),
            diagnostics: vec![diagnostic(
                Severity::Error,
                "collection-error",
                "sub/conftest.py",
            )],
            digests: None,
        }),
        ("a usage error", |r| Refined {
            cases: cases(r),
            diagnostics: vec![diagnostic(Severity::Error, "usage-error", "")],
            digests: None,
        }),
        ("cases from elsewhere", |r| {
            let mut cases = cases(r);
            cases.push(Case::new("elsewhere.py::x", "fake"));
            Refined {
                cases,
                diagnostics: Vec::new(),
                digests: None,
            }
        }),
    ];
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    for (what, reply) in replies {
        run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&Replies(reply)),
        );
        assert_eq!(entries(cache.path()), 0, "{what}");
        // The environment is fixed: the next run refines again.
        let refiner = FakeRefiner::default();
        let collection = run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        assert_eq!(refiner.calls(), 1, "{what}");
        assert_eq!(collection.cases.len(), 2, "{what}");
        fs::remove_dir_all(cache.path().join("refine")).unwrap();
    }

    let collection = run_cached(
        root,
        &fake(),
        cache.path(),
        environment(&python),
        Some(&FakeRefiner::failing()),
    );
    assert_eq!(
        codes_of(&collection),
        [(DiagnosticCode::RefineFailed, Severity::Warning)]
    );
    assert_eq!(entries(cache.path()), 0);
}

/// Finding 33: a file reading a helper module, a data file or anything
/// else the key leaves out kept its cached cases after that changed.
#[test]
fn only_standalone_files_are_cached() {
    let cases: &[(&str, &str, bool)] = &[
        ("plain", "def dynamic(): pass\n", true),
        (
            "pure imports",
            "import itertools, os\nfrom fake import mark\nimport _fake.x\n\
             CASES = [i for i in itertools.count() if os.environ.get('X')]\n\
             def dynamic(): pass\n",
            true,
        ),
        (
            "a project module",
            "from cases import CASES\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a relative import",
            "from . import x\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a third-party module",
            "import numpy\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a data file",
            "import json\nCASES = json.load(open('cases.json'))\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a directory",
            "from pathlib import Path\nCASES = sorted(Path('data').glob('*'))\ndef dynamic(): pass\n",
            false,
        ),
        (
            "an impure import",
            "from os import listdir\ndef dynamic(): pass\n",
            false,
        ),
        (
            "the clock",
            "import datetime\nDAY = datetime.date.today()\ndef dynamic(): pass\n",
            false,
        ),
        ("randomness", "import random\ndef dynamic(): pass\n", false),
        (
            "code by name",
            "M = __import__('cases')\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a computed attribute",
            "import os\nf = getattr(os, 'list' + 'dir')\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a project module named in a string",
            "plugins = ['support.params']\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a shadowed standard module",
            "import string\ndef dynamic(): pass\n",
            false,
        ),
        (
            "a star import",
            "from fake import *\ndef dynamic(): pass\n",
            false,
        ),
    ];
    for (what, source, cached) in cases {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let cache = tempfile::tempdir().unwrap();
        let python = interpreter(cache.path());
        write(root, "cases.py", "CASES = [1]\n");
        write(root, "support/params.py", "P = 1\n");
        write(root, "sub/string.py", "");
        write(root, "sub/d_check.py", source);
        let refiner = FakeRefiner::default();
        run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        assert_eq!(entries(cache.path()), usize::from(*cached), "{what}");
        run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        assert_eq!(refiner.calls(), if *cached { 1 } else { 2 }, "{what}");
    }
}

#[test]
fn what_makes_a_file_not_standalone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "tests/conftest.py", "");
    write(root, "src/mypkg/__init__.py", "");
    let seen = Mutex::new(HashMap::new());
    let installed: HashSet<String> = ["extmod", "numpy", "installed_plugin", "pytest", "_pytest"]
        .map(String::from)
        .into();
    let scope = standalone::Scope {
        root,
        cwd: root,
        framework: "pytest",
        seen: &seen,
        installed: &installed,
    };
    let check = |source: &str| {
        let module = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax();
        standalone::dependence(&module, &root.join("tests/t.py"), &scope)
    };
    for source in [
        "import pytest\nimport _pytest.mark\nfrom pytest import param\n",
        "import os, re, sys\nX = os.environ.get('A')\nR = re.compile('x')\n",
        "import datetime\nD = datetime.date(2020, 1, 1)\nT = datetime.time(1)\n",
        "import unittest\nclass T(unittest.TestCase):\n    def test_a(self): pass\n",
        "x = getattr(obj, 'name')\nm = re.match('a', 'a').group(0)\n",
        "CASES = ['a', 'b', 'tests and more', 'x.y']\n",
        // The framework and the standard library, though installed or named.
        "CASES = ['pytest', '_pytest.python', 'json', 'os.path']\n",
    ] {
        assert_eq!(check(source), None, "{source}");
    }
    for (source, reason) in [
        ("import mypkg\n", "it imports `mypkg`"),
        ("import numpy as np\n", "it imports `numpy`"),
        ("import pytest_cases\n", "it imports `pytest_cases`"),
        ("from .helpers import X\n", "relative import"),
        ("import subprocess\n", "it imports `subprocess`"),
        ("from os.path import exists\n", "it imports `exists`"),
        ("def f():\n    return open('x').read()\n", "it uses `open`"),
        (
            "import pathlib\nP = pathlib.Path('d').iterdir()\n",
            "`.iterdir`",
        ),
        ("import os\nN = os.getpid()\n", "`.getpid`"),
        ("x = hash('a')\n", "it calls `hash`"),
        ("x = getattr(os, name)\n", "getattr"),
        ("x = __builtins__\n", "`__builtins__`"),
        (
            "pytest_plugins = ['mypkg.plugin']\n",
            "may name a project module",
        ),
        (
            "pytest_plugins = 'tests.plugin'\n",
            "may name a project module",
        ),
        // Residue of finding 33: code named by string outside the project,
        // whose content the key doesn't hold (an editable install, a module
        // edited in site-packages).
        (
            "import pytest\nextmod = pytest.importorskip('extmod')\n",
            "`.importorskip`",
        ),
        (
            "import pytest\nnp = pytest.importorskip('numpy')\n",
            "`.importorskip`",
        ),
        (
            "from pytest import importorskip\n",
            "it imports `importorskip`",
        ),
        (
            "pytest_plugins = ['installed_plugin']\n",
            "may name an installed module",
        ),
        (
            "pytest_plugins = ('extmod.plugin',)\n",
            "may name an installed module",
        ),
        (
            "import builtins\nm = vars(builtins)['__import__']('x')\n",
            "names a way to import by name",
        ),
        (
            "import pytest\nf = getattr(pytest, 'importorskip')\n",
            "getattr",
        ),
        ("M = __import__('json')\n", "it uses `__import__`"),
        (
            "import importlib\nM = importlib.import_module('extmod')\n",
            "it imports `importlib`",
        ),
    ] {
        let why = check(source).unwrap_or_else(|| panic!("{source}: standalone"));
        assert!(why.contains(reason), "{source}: {why}");
    }
    // A project file shadowing a standard module or the framework (looked
    // up afresh: existence is remembered for one collection).
    write(root, "tests/json.py", "");
    write(root, "src/pytest/__init__.py", "");
    let fresh = Mutex::new(HashMap::new());
    let shadowed = |source: &str| {
        let module = ruff_python_parser::parse_module(source)
            .unwrap()
            .into_syntax();
        let scope = standalone::Scope {
            seen: &fresh,
            ..scope
        };
        standalone::dependence(&module, &root.join("tests/t.py"), &scope)
    };
    for source in ["import json\n", "import pytest\n"] {
        let why = shadowed(source).unwrap_or_else(|| panic!("{source}: standalone"));
        assert!(why.contains("may be a project file"), "{source}: {why}");
    }
    // Files outside the project are never standalone.
    let module = ruff_python_parser::parse_module("").unwrap().into_syntax();
    let outside = standalone::dependence(&module, Path::new("/elsewhere/t.py"), &scope);
    assert!(outside.unwrap().contains("outside the project"));
}

#[test]
fn what_loads_a_module_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "src/mypkg/__init__.py", "");
    let seen = Mutex::new(HashMap::new());
    let installed: HashSet<String> = ["extmod", "installed_plugin", "pytest", "_pytest"]
        .map(String::from)
        .into();
    let scope = standalone::Scope {
        root,
        cwd: root,
        framework: "pytest",
        seen: &seen,
        installed: &installed,
    };
    let loads = |texts: &[&str]| standalone::loads_by_name(texts.iter().copied(), &scope);
    for texts in [
        &["-p mypkg.plugin"][..],
        &["-pmypkg"],
        &["--cov=mypkg"],
        &["--opt=a,extmod"],
        // Any plugin loaded by name but the framework's and the standard
        // library's, installed or not (residue of finding 33): an editable
        // install may map it anywhere, and its content isn't in the key.
        &["-p", "installed_plugin"],
        &["-ra", "-p", "unknown_plugin.sub", "-q"],
        &["-p=installed_plugin"],
    ] {
        assert!(loads(texts).is_some(), "{texts:?}");
    }
    let why = loads(&["-p", "installed_plugin"]).unwrap();
    assert!(why.contains("`-p installed_plugin`"), "{why}");
    for texts in [
        &["-p no:cacheprovider"][..],
        &["-p", "no:extmod"],
        &["-ra --strict-markers"],
        &["/abs/path/pytest.ini"],
        &["-p", "pytest"],
        &["-p", "_pytest.pytester"],
        &["-p", "json"],
        &["-p", "*_check.py"],
        &["-k", "not slow", "-m", "db"],
    ] {
        assert_eq!(loads(texts), None, "{texts:?}");
    }
}

/// Residue of finding 33 (/tmp/rc-cc/f33e): a test file parametrized from
/// `pytest.importorskip("extmod")`, extmod installed in editable mode outside
/// the root through a `.pth`, was cached: editing extmod's cases changed
/// nothing in the key, and the new case was never collected or run.
#[test]
fn code_named_by_string_outside_the_project_is_never_cached() {
    let cases: &[(&str, &str, &str, bool)] = &[
        (
            "importorskip",
            "import fake\nextmod = fake.importorskip('extmod')\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "importorskip of a module nothing lists",
            "import fake\nm = fake.importorskip('nowhere')\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "a plugin list in the file",
            "fake_plugins = ['extmod']\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "a plugin list in a support file",
            "def dynamic(): pass\n",
            "fake_plugins = ('extmod.plugin',)\n",
            false,
        ),
        (
            "a module an editable install's finder maps",
            "fake_plugins = ['mapped']\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "a module on PYTHONPATH",
            "fake_plugins = ['onpath']\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "import_module",
            "import importlib\nm = importlib.import_module('extmod')\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "__import__",
            "m = __import__('extmod')\ndef dynamic(): pass\n",
            "",
            false,
        ),
        (
            "plain strings",
            "CASES = ['a', 'b', 'fake', 'json']\ndef dynamic(): pass\n",
            "",
            true,
        ),
    ];
    for (what, source, helpers, cached) in cases {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let cache = tempfile::tempdir().unwrap();
        let python = interpreter(cache.path());
        let site = cache.path().join("venv/lib/python3.13/site-packages");
        // An editable install through a path `.pth`.
        let extlib = dir.path().join("extlib");
        write(&extlib, "extmod.py", "CASES = ['a', 'b']\n");
        write(&site, "zz.pth", format!("{}\n", extlib.display()));
        // One through an import hook, known by its distribution.
        write(&site, "mapped-0.1.dist-info/METADATA", "Name: mapped\n");
        write(&site, "__editable___mapped_0_1_finder.py", "MAPPING = {}\n");
        write(
            &site,
            "__editable__.mapped-0.1.pth",
            "import __editable___mapped_0_1_finder\n",
        );
        write(dir.path(), "onpath/onpath.py", "");
        let mut env = environment(&python);
        env.variables = Some(vec![(
            "PYTHONPATH".into(),
            dir.path().join("onpath").into_os_string(),
        )]);
        write(&root, "d_check.py", source);
        if !helpers.is_empty() {
            write(&root, "helpers.py", helpers);
        }
        let refiner = FakeRefiner::default();
        for _ in 0..2 {
            run_cached(&root, &fake(), cache.path(), env.clone(), Some(&refiner));
        }
        assert_eq!(entries(cache.path()), usize::from(*cached), "{what}");
        assert_eq!(refiner.calls(), if *cached { 1 } else { 2 }, "{what}");
    }
}

#[test]
fn installed_modules_are_listed_from_the_interpreters_paths() {
    let dir = tempfile::tempdir().unwrap();
    let python = interpreter(dir.path());
    let site = dir.path().join("venv/lib/python3.13/site-packages");
    for entry in [
        "pkg/__init__.py",
        "single.py",
        "ext.cpython-313-darwin.so",
        "winext.pyd",
        "Some_Dist-1.0.dist-info/top_level.txt",
        "old.egg-info/top_level.txt",
        "notes.txt",
    ] {
        write(&site, entry, "");
    }
    write(&site, "Some_Dist-1.0.dist-info/top_level.txt", "toplevel\n");
    write(&site, "old.egg-info/top_level.txt", "oldmod\n");
    write(dir.path(), "elsewhere/viapth/__init__.py", "");
    write(dir.path(), "relative/relmod.py", "");
    write(
        &site,
        "paths.pth",
        format!(
            "# a comment\n{}\n../../../../relative\nimport hook; hook.install()\n",
            dir.path().join("elsewhere").display()
        ),
    );
    write(dir.path(), "pp/onpath.py", "");
    let mut env = environment(&python);
    env.variables = Some(vec![(
        "PYTHONPATH".into(),
        dir.path().join("pp").into_os_string(),
    )]);
    let names = env.installed_modules(dir.path());
    for name in [
        "pkg",
        "single",
        "ext",
        "winext",
        "Some_Dist",
        "some_dist",
        "toplevel",
        "old",
        "oldmod",
        "viapth",
        "relmod",
        "onpath",
    ] {
        assert!(names.contains(name), "{name}: {names:?}");
    }
    for name in ["notes", "paths", "hook", "1"] {
        assert!(!names.contains(name), "{name}: {names:?}");
    }
    assert!(
        Environment::default()
            .installed_modules(dir.path())
            .is_empty()
    );
}

/// Whether the interpreter installs a module (pytest, for the default
/// adapters): `None` when its packages can't be found, and a venv that
/// leaves out the system's packages counts only its own.
#[test]
fn whether_the_interpreter_installs_a_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    // The base interpreter, with pytest.
    write(root, "base/bin/python3", "");
    write(
        root,
        "base/lib/python3.13/site-packages/_pytest/__init__.py",
        "",
    );
    write(
        root,
        "base/lib/python3.13/site-packages/pytest-9.1.1.dist-info/METADATA",
        "",
    );
    let base = Environment {
        interpreter: Some(root.join("base/bin/python3")),
        ..Environment::default()
    };
    assert_eq!(base.installs(root, &["pytest"]), Some(true));
    assert_eq!(base.installs(root, &["_pytest"]), Some(true));
    assert_eq!(base.installs(root, &["nose"]), Some(false));

    // A venv of it, as `python -m venv` makes one: the interpreter a link to
    // the base's.
    write(
        root,
        "venv/pyvenv.cfg",
        format!("home = {}\n", root.join("base/bin").display()),
    );
    fs::create_dir_all(root.join("venv/lib/python3.13/site-packages")).unwrap();
    fs::create_dir_all(root.join("venv/bin")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("base/bin/python3"), root.join("venv/bin/python"))
        .unwrap();
    #[cfg(not(unix))]
    write(root, "venv/bin/python", "");
    let venv = Environment {
        interpreter: Some(root.join("venv/bin/python")),
        ..Environment::default()
    };
    assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(false));
    write(
        root,
        "venv/lib/python3.13/site-packages/pytest/__init__.py",
        "",
    );
    assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(true));
    fs::remove_dir_all(root.join("venv/lib/python3.13/site-packages/pytest")).unwrap();
    // One that includes the system's packages sees the base's.
    write(
        root,
        "venv/pyvenv.cfg",
        format!(
            "home = {}\ninclude-system-site-packages = true\n",
            root.join("base/bin").display()
        ),
    );
    #[cfg(unix)]
    assert_eq!(venv.installs(root, &["pytest", "_pytest"]), Some(true));

    // Windows: `Scripts\python.exe` in a venv, `python.exe` beside `Lib` in
    // an installation.
    write(root, "win/python.exe", "");
    write(root, "win/Lib/site-packages/pytest/__init__.py", "");
    let windows = Environment {
        interpreter: Some(root.join("win/python.exe")),
        ..Environment::default()
    };
    assert_eq!(windows.installs(root, &["pytest"]), Some(true));
    write(root, "winvenv/pyvenv.cfg", "home = C:\\Python313\n");
    write(root, "winvenv/Scripts/python.exe", "");
    fs::create_dir_all(root.join("winvenv/Lib/site-packages")).unwrap();
    let windows = Environment {
        interpreter: Some(root.join("winvenv/Scripts/python.exe")),
        ..Environment::default()
    };
    assert_eq!(windows.installs(root, &["pytest"]), Some(false));

    // Can't tell: no interpreter, a missing one, a shim with no packages.
    assert_eq!(Environment::default().installs(root, &["pytest"]), None);
    let missing = Environment {
        interpreter: Some(root.join("nope/bin/python")),
        ..Environment::default()
    };
    assert_eq!(missing.installs(root, &["pytest"]), None);
    write(root, "shims/python", "#!/bin/sh\n");
    let shim = Environment {
        interpreter: Some(root.join("shims/python")),
        ..Environment::default()
    };
    assert_eq!(shim.installs(root, &["pytest"]), None);
}

#[test]
fn a_support_file_that_is_not_standalone_turns_the_cache_off_below_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "a/helpers.py", "from cases import CASES\n");
    write(root, "a/d_check.py", "def dynamic(): pass\n");
    write(root, "b/helpers.py", "import os\n");
    write(root, "b/e_check.py", "def dynamic(): pass\n");
    write(root, "cases.py", "CASES = [1]\n");
    let refiner = FakeRefiner::default();
    run_cached(
        root,
        &fake(),
        cache.path(),
        environment(&python),
        Some(&refiner),
    );
    run_cached(
        root,
        &fake(),
        cache.path(),
        environment(&python),
        Some(&refiner),
    );
    let requests = std::mem::take(&mut *refiner.requests.lock().unwrap());
    assert_eq!(requests[1].files, ["a/d_check.py"]);
    assert_eq!(entries(cache.path()), 1);
}

#[test]
fn package_init_files_are_in_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "pkg/__init__.py", "X = 1\n");
    write(root, "pkg/inner/__init__.py", "");
    write(root, "pkg/inner/d_check.py", "def dynamic(): pass\n");
    let refiner = FakeRefiner::default();
    let calls = || {
        run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        std::mem::take(&mut *refiner.requests.lock().unwrap()).len()
    };
    assert_eq!(calls(), 1);
    assert_eq!(calls(), 0);
    write(root, "pkg/__init__.py", "X = 2\n");
    assert_eq!(calls(), 1);
    assert_eq!(calls(), 0);
    // One that isn't standalone turns the cache off for the files below it.
    write(root, "pkg/__init__.py", "from helpers_mod import X\n");
    write(root, "helpers_mod.py", "X = 1\n");
    assert_eq!(calls(), 1);
    assert_eq!(calls(), 1);
}

#[test]
fn a_plugin_in_the_context_or_arguments_turns_the_cache_off() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    write(root, "myplugins/params.py", "");
    let with_context = |context: serde_json::Value| -> Vec<Prepared> {
        let mut fake = Fake::new();
        fake.context = context;
        vec![("fake", Box::new(fake))]
    };
    let refiner = FakeRefiner::default();
    for context in [
        serde_json::json!({"addopts": ["-p", "myplugins.params"]}),
        serde_json::json!({"addopts": ["-pmyplugins.params"]}),
        serde_json::json!({"env": "-p myplugins"}),
    ] {
        run_cached(
            root,
            &with_context(context),
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        assert_eq!(entries(cache.path()), 0);
    }
    let args = Environment {
        framework_args: vec!["-p".into(), "myplugins.params".into()],
        ..environment(&python)
    };
    run_cached(root, &fake(), cache.path(), args, Some(&refiner));
    assert_eq!(entries(cache.path()), 0);
    // Residue of finding 33: so does an installed plugin, whose content the
    // key doesn't hold (installed in editable mode, or edited in place).
    let installed = with_context(serde_json::json!({"addopts": ["-p", "xdist"]}));
    run_cached(
        root,
        &installed,
        cache.path(),
        environment(&python),
        Some(&refiner),
    );
    assert_eq!(entries(cache.path()), 0);
    let args = Environment {
        framework_args: vec!["-p".into(), "extmod".into()],
        ..environment(&python)
    };
    run_cached(root, &fake(), cache.path(), args, Some(&refiner));
    assert_eq!(entries(cache.path()), 0);
    let project_args = ["-pextmod".to_string()];
    run_cached_with(
        root,
        &fake(),
        cache.path(),
        environment(&python),
        Some(&refiner),
        &project_args,
    );
    assert_eq!(entries(cache.path()), 0);
    // Blocking a plugin loads nothing.
    let blocked = with_context(serde_json::json!({"addopts": ["-p", "no:cacheprovider"]}));
    run_cached(
        root,
        &blocked,
        cache.path(),
        environment(&python),
        Some(&refiner),
    );
    assert_eq!(entries(cache.path()), 1);
}

#[test]
fn files_the_context_names_are_keyed_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    write(root, "pytest.ini", "[pytest]\n");
    let mut fake = Fake::new();
    fake.context = serde_json::json!({"inifile": root.join("pytest.ini").to_string_lossy()});
    let recognizers: Vec<Prepared> = vec![("fake", Box::new(fake))];
    let refiner = FakeRefiner::default();
    let calls = || {
        run_cached(
            root,
            &recognizers,
            cache.path(),
            environment(&python),
            Some(&refiner),
        );
        std::mem::take(&mut *refiner.requests.lock().unwrap()).len()
    };
    assert_eq!(calls(), 1);
    assert_eq!(calls(), 0);
    write(root, "pytest.ini", "[pytest]\npythonpath = src\n");
    assert_eq!(calls(), 1);
}

/// Findings 68 and 78: the cache created `.aye` without its markers.
#[test]
fn the_state_directory_the_cache_creates_is_marked() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let python = interpreter(&root.join("env"));
    write(root, "d_check.py", "def dynamic(): pass\n");
    let state = root.join(".aye");
    run_cached(
        root,
        &fake(),
        &state.join("cache"),
        environment(&python),
        Some(&FakeRefiner::default()),
    );
    assert_eq!(entries(&state.join("cache")), 1);
    let gitignore = fs::read_to_string(state.join(".gitignore")).unwrap();
    assert!(gitignore.lines().any(|line| line == "*"), "{gitignore}");
    assert!(state.join("CACHEDIR.TAG").is_file());
    assert!(!state.join("cache/.gitignore").exists());
}

#[test]
fn files_outside_the_root_are_relative_with_parent_parts() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("project");
    write(dir.path(), "outside/o_check.py", "def test_o(): pass\n");
    fs::create_dir_all(&root).unwrap();
    let targets = [dir.path().join("outside")];
    let collection = run(&root, &fake(), &targets, Options::default(), None);
    assert_eq!(ids(&collection), ["../outside/o_check.py::test_o"]);
}

#[test]
fn the_parallel_map_keeps_order() {
    let items: Vec<u32> = (0..1000).collect();
    let doubled = parallel::map(&items, |n| n * 2);
    assert_eq!(doubled, items.iter().map(|n| n * 2).collect::<Vec<_>>());
    assert_eq!(parallel::map(&[] as &[u32], |n| *n), [] as [u32; 0]);
}

#[test]
fn explicit_paths_are_entered_whatever_descend_says() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "skip/x_check.py", "def test_x(): pass\n");
    write(root, "skip/inner/y_check.py", "def test_y(): pass\n");
    let collection = run(
        root,
        &fake(),
        &[root.join("skip")],
        Options::default(),
        None,
    );
    assert_eq!(
        ids(&collection),
        ["skip/inner/y_check.py::test_y", "skip/x_check.py::test_x"]
    );
    let targets = [root.join("skip/x_check.py")];
    let collection = run(root, &fake(), &targets, Options::default(), None);
    assert_eq!(ids(&collection), ["skip/x_check.py::test_x"]);
}

#[cfg(unix)]
#[test]
fn symlinks_are_followed_and_loops_skipped() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "real/r_check.py", "def test_r(): pass\n");
    symlink(root.join("real"), root.join("linked")).unwrap();
    symlink(root.join("real"), root.join("real/again")).unwrap();
    symlink(root.join("gone"), root.join("broken_check.py")).unwrap();
    let collection = run(root, &fake(), &[], Options::default(), None);
    assert_eq!(
        ids(&collection),
        ["linked/r_check.py::test_r", "real/r_check.py::test_r"]
    );
    assert!(collection.diagnostics.is_empty(), "{collection:?}");
}

#[test]
fn a_corrupt_cache_entry_is_a_miss() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let cache = tempfile::tempdir().unwrap();
    let python = interpreter(cache.path());
    write(root, "d_check.py", "def dynamic(): pass\n");
    let refiner = FakeRefiner::default();
    let collect = || {
        run_cached(
            root,
            &fake(),
            cache.path(),
            environment(&python),
            Some(&refiner),
        )
    };
    collect();
    let entries = cache.path().join("refine/fake");
    let entry = fs::read_dir(&entries).unwrap().next().unwrap().unwrap();
    fs::write(entry.path(), "{not json").unwrap();

    let collection = collect();
    assert_eq!(refiner.calls(), 2);
    assert_eq!(collection.cases.len(), 2);
    collect();
    assert_eq!(refiner.calls(), 2);

    // An entry holding a diagnostic (written by other rules) is a miss too.
    let entry = fs::read_dir(&entries).unwrap().next().unwrap().unwrap();
    fs::write(
        entry.path(),
        r#"{"cases":[],"diagnostics":[{"severity":"info","code":"skipped","path":"d_check.py","message":"off"}]}"#,
    )
    .unwrap();
    collect();
    assert_eq!(refiner.calls(), 3);
}

/// Refines like [`FakeRefiner`], slowly, and can stop collection while it
/// works (as Ctrl-C would).
struct SlowRefiner {
    inner: FakeRefiner,
    stop: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl Refiner for SlowRefiner {
    fn refine(&self, request: &RefineRequest) -> Result<Refined, RefineError> {
        if let Some(stop) = &self.stop {
            stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        self.inner.refine(request)
    }
}

fn collector_at<'a>(root: &'a Path, recognizers: &'a [Prepared]) -> Collector<'a> {
    let project = Project {
        root,
        cwd: root,
        args: &[],
        config_file: None,
        framework_args: &[],
    };
    Collector::new(project, recognizers, Options::default())
}

#[test]
fn a_stopped_collection_ends_at_once_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..10 {
        write(
            dir.path(),
            &format!("t{i}/a_check.py"),
            "def test_a(): pass\n",
        );
    }
    let recognizers = fake();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let collection = collector_at(dir.path(), &recognizers)
        .with_stop(stop)
        .collect(&[], None);
    assert!(collection.interrupted);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
    assert_eq!(collection.diagnostics, [] as [aye_events::Diagnostic; 0]);

    // Not stopped: everything, and not interrupted.
    let collection = collector_at(dir.path(), &recognizers).collect(&[], None);
    assert!(!collection.interrupted);
    assert_eq!(collection.cases.len(), 10);
}

#[test]
fn collection_reports_its_progress_while_it_walks() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..300 {
        write(dir.path(), &format!("d{}/f{i}.txt", i % 7), "");
    }
    for i in 0..5 {
        write(
            dir.path(),
            &format!("d{i}/x{i}_check.py"),
            "def test_x(): pass\n",
        );
    }
    let recognizers = fake();
    let mut reports = Vec::new();
    let collection =
        collector_at(dir.path(), &recognizers)
            .collect_watching(&[], None, &mut |progress| reports.push(progress));
    assert_eq!(collection.cases.len(), 5);
    // One while walking (every 256 entries), one when the walk is done.
    assert!(reports.len() >= 2, "{reports:?}");
    assert!(reports.windows(2).all(|w| w[0].walked <= w[1].walked));
    let last = reports.last().unwrap();
    assert_eq!(last.test_files, 5);
    assert!(last.walked >= 305, "{last:?}");
    assert!(reports.iter().all(|p| p.refining == 0));
}

#[test]
fn collection_reports_its_progress_while_a_file_is_refined() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a_check.py", "def dynamic(): pass\n");
    write(dir.path(), "b_check.py", "def test_b(): pass\n");
    let recognizers = fake();
    let refiner = SlowRefiner {
        inner: FakeRefiner::default(),
        stop: None,
    };
    let mut reports = Vec::new();
    let collection = collector_at(dir.path(), &recognizers).collect_watching(
        &[],
        Some(&refiner),
        &mut |progress| reports.push(progress),
    );
    assert!(!collection.interrupted);
    assert_eq!(collection.refined_files, 1);
    // Every 50ms of the 300 the refiner takes.
    let refining = reports.iter().filter(|p| p.refining == 1).count();
    assert!(refining >= 3, "{reports:?}");
}

#[test]
fn a_stop_while_refining_ends_collection_as_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a_check.py", "def dynamic(): pass\n");
    let recognizers = fake();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let refiner = SlowRefiner {
        inner: FakeRefiner::default(),
        stop: Some(Arc::clone(&stop)),
    };
    let collection = collector_at(dir.path(), &recognizers)
        .with_stop(stop)
        .collect(&[], Some(&refiner));
    assert!(collection.interrupted);
    assert_eq!(collection.cases, [] as [aye_events::Case; 0]);
}
