use std::fs;

use super::*;
use crate::positional;

/// Recheck regressions 2 and 4: dotted names and keys are resolved by
/// collecting the whole project, whose refinement reports on all of it
/// from the framework's default start. What the run reaches decides which
/// of collection's diagnostics are the run's, and the paths it starts from
/// go to the adapters.
#[test]
fn what_dotted_names_reach_decides_what_collection_reports() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for file in [
        "tests/test_a.py",
        "tests/sub/test_s.py",
        "testing/test_g.py",
        "other/test_o.py",
    ] {
        fs::create_dir_all(root.join(file).parent().unwrap()).unwrap();
        fs::write(root.join(file), "").unwrap();
    }
    let positionals = |args: &[&str]| -> Vec<Positional> {
        args.iter()
            .map(|arg| positional::resolve(root, root, arg))
            .collect()
    };
    let cases: Vec<Case> = [
        "tests/test_a.py::TestX::test_y",
        "tests/test_a.py::TestX::test_z",
        "testing/test_g.py::test_g",
        "other/test_o.py::test_o",
    ]
    .iter()
    .map(|id| Case::new(*id, "pytest"))
    .collect();
    let diagnostic = |severity, code: DiagnosticCode, path: &str| Diagnostic {
        severity,
        code,
        path: path.into(),
        span: None,
        message: "m".to_string(),
    };
    let skipped = |path: &str| diagnostic(Severity::Info, DiagnosticCode::Skipped, path);
    let usage = |path: &str| diagnostic(Severity::Error, DiagnosticCode::UsageError, path);

    // No argument, or only paths: collection walked only what the run
    // reaches, and everything is the run's.
    assert_eq!(Reach::of(&[], root, &cases), None);
    assert_eq!(
        Reach::of(&positionals(&["tests", "other/test_o.py"]), root, &cases),
        None
    );

    // A dotted name: the files of the cases it selects.
    let reach = Reach::of(&positionals(&["tests.test_a"]), root, &cases).unwrap();
    assert_eq!(reach.paths, Vec::<PathBuf>::new());
    assert_eq!(reach.selected, [root.join("tests/test_a.py")]);
    // A skipped directory or module elsewhere is not counted.
    assert!(!reach.keeps(root, &skipped("tests/sub")));
    assert!(!reach.keeps(root, &skipped("testing/test_g.py")));
    // A conftest pytest loads first only in its default start isn't the
    // run's; one on the way to a selected file is.
    assert!(!reach.keeps(root, &usage("testing/conftest.py")));
    assert!(reach.keeps(root, &usage("tests/conftest.py")));
    assert!(reach.keeps(root, &usage("conftest.py")));
    // Nor is a conftest above the root.
    assert!(reach.keeps(root, &usage("../conftest.py")));
    // pytest rejecting its arguments is always the run's, and so is
    // everything else.
    assert!(reach.keeps(root, &usage("")));
    let error = diagnostic(
        Severity::Error,
        DiagnosticCode::CollectionError,
        "testing/test_g.py",
    );
    assert!(reach.keeps(root, &error));
    let info = diagnostic(Severity::Info, DiagnosticCode::NoTests, "testing/test_g.py");
    assert!(reach.keeps(root, &info));
    // When an argument named nothing, an error elsewhere may be why.
    let lost = Reach {
        unmatched: true,
        ..reach.clone()
    };
    assert!(lost.keeps(root, &usage("testing/conftest.py")));
    assert!(!lost.keeps(root, &skipped("tests/sub")));

    // A dotted name reaches the skipped module or package it names.
    let reach = Reach::of(&positionals(&["testing.test_g"]), root, &cases).unwrap();
    assert!(reach.keeps(root, &skipped("testing/test_g.py")));
    assert!(!reach.keeps(root, &skipped("tests/sub")));

    // Paths beside them: what a path holds is reached, in the order of the
    // arguments; a key resolves like a dotted name.
    let key = cases[3].id.key().to_string();
    let reach = Reach::of(
        &positionals(&["tests", &key, "tests.test_a", "testing/test_g.py::test_g"]),
        root,
        &cases,
    )
    .unwrap();
    assert_eq!(
        reach.paths,
        [root.join("tests"), root.join("testing/test_g.py")]
    );
    assert_eq!(
        reach.selected,
        [
            root.join("tests"),
            root.join("other/test_o.py"),
            root.join("tests/test_a.py"),
            root.join("testing/test_g.py"),
        ]
    );
    assert!(reach.keeps(root, &skipped("tests/sub")));
    assert!(reach.keeps(root, &skipped("testing/test_g.py")));
    assert!(!reach.keeps(root, &skipped("other")));
    assert!(reach.keeps(root, &usage("tests/sub/conftest.py")));
    assert!(reach.keeps(root, &usage("other/conftest.py")));
    assert!(reach.keeps(root, &usage("testing/conftest.py")));
}

/// pytest counts a skipped module or package that `--pyargs` names, and
/// finds it; a test in it, or a module in a skipped directory, it counts
/// the skip for but doesn't find.
#[test]
fn a_dotted_name_reaches_the_skips_of_what_it_names() {
    for (word, path, named) in [
        ("other.test_mod", "other/test_mod.py", Named::Whole),
        ("other.test_mod.__init__", "other/test_mod.py", Named::Whole),
        ("other", "other/test_mod.py", Named::Whole),
        ("tests.sub", "tests/sub", Named::Whole),
        ("tests", "tests/sub", Named::Whole),
        ("other.test_mod.test_m", "other/test_mod.py", Named::Part),
        (
            "other.test_mod.Class.test_m",
            "other/test_mod.py",
            Named::Part,
        ),
        ("tests.sub.test_s", "tests/sub", Named::Part),
        ("tests.sub.deep.test_s.test_x", "tests/sub", Named::Part),
        ("tests.test_a", "tests/sub", Named::Nothing),
        ("tests.test_a.TestX", "tests/test_a.py.bak", Named::Nothing),
        ("tests.subway", "tests/sub", Named::Nothing),
        ("other.test_mod", "other/test_mod_two.py", Named::Nothing),
        ("cd12fbc3", "tests/sub", Named::Nothing),
        ("tests.sub::x", "tests/sub", Named::Nothing),
        ("tests..sub", "tests/sub", Named::Nothing),
    ] {
        assert_eq!(dotted_reach(word, path), named, "{word} {path}");
    }
}

/// A file below a conftest that failed to load has no tests because of it,
/// which the conftest's error says: collection's `no-tests` note, which
/// says only that the file has none, is left out (it showed beside a
/// start-up conftest's usage error, and beside a directory's collection
/// error, where pytest reports the error alone).
#[test]
fn a_file_below_a_broken_conftest_gets_no_no_tests_note() {
    let diagnostic = |severity: Severity, code: DiagnosticCode, path: &str| Diagnostic {
        severity,
        code,
        path: path.into(),
        span: None,
        message: String::new(),
    };
    let collected = [
        diagnostic(
            Severity::Error,
            DiagnosticCode::UsageError,
            "tests/conftest.py",
        ),
        diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "sub/deep/conftest.py",
        ),
        // Not a conftest's: no directory is explained by it.
        diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "other/test_x.py",
        ),
        // A conftest's info explains nothing either.
        diagnostic(
            Severity::Info,
            DiagnosticCode::UsageError,
            "more/conftest.py",
        ),
    ];
    let broken = broken_conftest_dirs(&collected);
    assert_eq!(broken, ["tests/", "sub/deep/"]);
    let explained = |path: &str| {
        explained_by_a_conftest(
            &broken,
            &diagnostic(Severity::Info, DiagnosticCode::NoTests, path),
        )
    };
    assert!(explained("tests/test_t.py"));
    assert!(explained("tests/pkg/test_p.py"));
    assert!(explained("sub/deep/test_d.py"));
    assert!(!explained("tests2/test_t.py"));
    assert!(!explained("sub/test_s.py"));
    assert!(!explained("other/test_y.py"));
    assert!(!explained("more/test_m.py"));
    // Only the `no-tests` info.
    assert!(!explained_by_a_conftest(
        &broken,
        &diagnostic(
            Severity::Error,
            DiagnosticCode::CollectionError,
            "tests/test_t.py"
        )
    ));
    // A root conftest's failure explains every file.
    let root = [diagnostic(
        Severity::Error,
        DiagnosticCode::UsageError,
        "conftest.py",
    )];
    let broken = broken_conftest_dirs(&root);
    assert!(explained_by_a_conftest(
        &broken,
        &diagnostic(Severity::Info, DiagnosticCode::NoTests, "test_top.py")
    ));
}
