use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use super::*;

fn write(root: &Path, rel: &str, body: &str) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, body).unwrap();
    path
}

#[test]
fn normpath_folds_dots_lexically() {
    let cases = [
        ("/a/b/../c", "/a/c"),
        ("/a/./b/", "/a/b"),
        ("/a/./b/../c", "/a/c"),
        ("/a/b/../../..", "/"),
        ("/..", "/"),
        ("/../a", "/a"),
        ("a/../..", ".."),
        ("../a/./b", "../a/b"),
        ("../a/..", ".."),
        ("a/..", "."),
        ("", "."),
    ];
    for (path, expected) in cases {
        assert_eq!(normpath(Path::new(path)), Path::new(expected), "{path}");
    }
    assert_eq!(
        absolute(Path::new("/r/sub"), Path::new("../other")),
        Path::new("/r/other")
    );
    assert_eq!(
        absolute(Path::new("/r/sub"), Path::new("/x/./y/..")),
        Path::new("/x")
    );
}

/// The two normalizers agree on absolute paths, and differ on relative
/// ones.
#[test]
fn normalize_absolute_lets_every_dot_dot_pop() {
    for path in ["/a/b/../c", "/a/./b/", "/a/b/../../..", "/..", "/../a"] {
        assert_eq!(
            normalize_absolute(Path::new(path)),
            normpath(Path::new(path)),
            "{path}"
        );
    }
    let cases = [("../a/./b", "a/b"), ("a/../..", ""), ("a/..", ""), ("", "")];
    for (path, expected) in cases {
        assert_eq!(
            normalize_absolute(Path::new(path)),
            Path::new(expected),
            "{path}"
        );
    }
}

#[test]
fn common_ancestor_of_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "a/b/t1.py", "");
    write(root, "a/c/t2.py", "");
    let paths = [PathBuf::from("a/b/t1.py"), PathBuf::from("a/c")];
    assert_eq!(common_ancestor(root, &paths), root.join("a"));
    assert_eq!(common_ancestor(root, &[PathBuf::from("missing")]), root);
    let sub = root.join("a/b");
    assert_eq!(
        common_ancestor(&sub, &[PathBuf::from("..")]),
        root.join("a")
    );
    assert_eq!(
        common_ancestor(&sub, &[PathBuf::from("../c/./t2.py")]),
        root.join("a/c")
    );
    assert_eq!(common_ancestor(&root.join("a/../a/b"), &[]), sub);
}

#[cfg(unix)]
#[test]
fn one_directory_spelled_two_ways_is_one_ancestor() {
    let dir = tempfile::tempdir().unwrap();
    let real = fs::canonicalize(dir.path()).unwrap();
    write(&real, "proj/pyproject.toml", "");
    write(&real, "proj/tests/test_a.py", "");
    write(&real, "proj/tests/test_b.py", "");
    // A second spelling of `proj`, as macOS's /tmp is /private/tmp.
    let alias_dir = tempfile::tempdir().unwrap();
    let alias = alias_dir.path().join("alias");
    std::os::unix::fs::symlink(real.join("proj"), &alias).unwrap();
    let cwd = real.join("proj");
    let paths = [
        alias.join("tests/test_a.py"),
        PathBuf::from("tests/test_b.py"),
    ];
    // The two spellings are resolved only when they share nothing but
    // the filesystem's root (macOS's /private/var against /var); where
    // both temporary directories share more (/tmp on Linux), that
    // ancestor is kept, as pytest's own `get_common_ancestor` does.
    let lexical = deepest_common(&[alias.join("tests"), real.join("proj/tests")]).unwrap();
    if lexical.parent().is_none() {
        assert_eq!(common_ancestor(&cwd, &paths), real.join("proj/tests"));
    } else {
        assert_eq!(common_ancestor(&cwd, &paths), lexical);
    }
    // One spelling alone is kept as given, symlinks unresolved.
    assert_eq!(
        common_ancestor(&cwd, &[alias.join("tests")]),
        alias.join("tests")
    );
    // A symlinked directory inside the project keeps its own name.
    std::os::unix::fs::symlink(real.join("proj/tests"), real.join("proj/linked")).unwrap();
    assert_eq!(
        common_ancestor(&cwd, &[PathBuf::from("linked"), PathBuf::from("tests")]),
        real.join("proj")
    );

    let root = real.join("proj");
    assert_eq!(
        relative_to(&root, &alias.join("tests/test_a.py")),
        Some(PathBuf::from("tests/test_a.py"))
    );
    assert_eq!(
        relative_to(&root, &alias.join("tests/missing.py")),
        Some(PathBuf::from("tests/missing.py"))
    );
    assert_eq!(
        relative_to(&root, &root.join("linked/test_a.py")),
        Some(PathBuf::from("linked/test_a.py"))
    );
    assert_eq!(relative_to(&root, &real), None);
    assert_eq!(relative_to(&root, &root), Some(PathBuf::new()));
    assert_eq!(root_relative(&alias.join("tests"), &root), "tests");
}

#[test]
fn relative_paths_use_slashes_and_parent_parts() {
    let root = Path::new("/repo/project");
    for relative in [relative_through_root, relative_below_root] {
        assert_eq!(
            relative(Path::new("/repo/project/tests/t.py"), root),
            "tests/t.py"
        );
        assert_eq!(
            relative(Path::new("/repo/other/t.py"), root),
            "../other/t.py"
        );
        assert_eq!(relative(root, root), "");
        assert_eq!(
            relative(Path::new("/elsewhere"), Path::new("/")),
            "elsewhere"
        );
    }
    assert_eq!(slashed(Path::new("a/b/c.py")), "a/b/c.py");
}

/// Finding 3 of the reorganisation plan: outside the root, sharing only
/// `/` with it, collection climbs out and the binary stays absolute.
#[test]
fn the_relativizers_differ_when_only_the_root_is_shared() {
    let root = Path::new("/repo/project");
    assert_eq!(
        relative_through_root(Path::new("/x/t.py"), root),
        "../../x/t.py"
    );
    assert_eq!(relative_below_root(Path::new("/x/t.py"), root), "/x/t.py");
}

#[test]
fn paths_expand_as_pytest_expands_a_report_path() {
    let vars = |vars: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            vars.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| OsString::from(value))
        }
    };
    let env = vars(&[("NAME", "value"), ("HOME", "/home/me/")]);
    for (given, want) in [
        ("out.xml", "out.xml"),
        ("$NAME/a.xml", "value/a.xml"),
        ("${NAME}/a.xml", "value/a.xml"),
        ("x${NAME}.xml", "xvalue.xml"),
        // Unset variables, and what isn't a reference, stay as written.
        ("$UNSET/a${UNSET}", "$UNSET/a${UNSET}"),
        ("a$/b$", "a$/b$"),
        ("${unclosed/a", "${unclosed/a"),
        ("${}x", "${}x"),
        ("~someone/a.xml", "~someone/a.xml"),
        ("a/~/b", "a/~/b"),
        ("~", "/home/me"),
        ("~/r/a.xml", "/home/me/r/a.xml"),
    ] {
        assert_eq!(expand_path(given, env), PathBuf::from(want), "{given}");
    }
    let profile = vars(&[("USERPROFILE", "/users/me")]);
    assert_eq!(
        expand_path("~/a.xml", profile),
        Path::new("/users/me/a.xml")
    );
    let top = vars(&[("HOME", "/")]);
    assert_eq!(expand_path("~", top), Path::new("/"));
    let homeless = vars(&[]);
    assert_eq!(expand_path("~/a.xml", homeless), Path::new("~/a.xml"));
}

#[cfg(unix)]
#[test]
fn a_value_that_is_not_unicode_is_not_expanded() {
    use std::os::unix::ffi::OsStringExt;

    let var = |_: &str| Some(OsString::from_vec(vec![0xff]));
    assert_eq!(expand_path("$NAME/a", var), Path::new("$NAME/a"));
}
