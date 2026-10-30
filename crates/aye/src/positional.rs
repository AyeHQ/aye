//! The positional arguments, as a run and `aye show` read them
//! ([`Positional`]): a path taken from the current directory, or from the
//! project root when only the root has it, else a dotted name or a key.
//!
//! Decided (implementation, October 2026):
//! - **Positional paths** are relative to the current directory, with `.`
//!   and `..` resolved lexically before the project root is searched for. A
//!   path only the root has is taken from the root, so the ids aye prints
//!   rerun from any directory. One spelled through another name of a
//!   directory in the project (macOS's `/tmp` is `/private/tmp`) is spelled
//!   from the root, as collection's ids are (`aye_fs::relative_to`).
//!   Paths mixed with dotted names or keys are collected along with the
//!   adapters' default roots. Cases are listed in the order of the arguments
//!   that select them, as pytest orders them (selection and collection do
//!   it); the runner still keeps a file's cases together in a batch.

use std::path::{Path, PathBuf};

use aye_fs::{normalize_absolute, root_relative};

/// A positional argument and how it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Positional {
    /// What selection gets: relative to the root, with `/` separators.
    pub(crate) selector: String,
    /// For a path or an id, its file or directory, absolute and normalized.
    pub(crate) path: Option<PathBuf>,
    /// Whether it has a `::` part.
    pub(crate) id: bool,
}

impl Positional {
    pub(crate) fn exists(&self) -> bool {
        self.path.as_deref().is_some_and(Path::exists)
    }
}

/// The path part of a positional argument: everything before `::`.
pub(crate) fn path_part(arg: &str) -> &str {
    arg.split_once("::").map_or(arg, |(path, _)| path)
}

/// A positional argument names a path when it exists, contains a separator,
/// or ends in `.py`; otherwise it is a dotted name or a key.
pub(crate) fn is_path_like(cwd: &Path, arg: &str) -> bool {
    arg.contains('/')
        || arg.contains(std::path::MAIN_SEPARATOR)
        || Path::new(arg)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("py"))
        || cwd.join(arg).exists()
}

/// Read a positional argument: a path is taken from the current directory,
/// as pytest does, or from the project root when only the root has it, so
/// that the ids aye prints (which are relative to the root) work from any
/// directory. The selector is relative to the root, keeping any `::` part;
/// dotted names and keys pass through.
pub(crate) fn resolve(cwd: &Path, root: &Path, arg: &str) -> Positional {
    let (path, rest) = arg
        .split_once("::")
        .map_or((arg, None), |(p, r)| (p, Some(r)));
    let from_root = || {
        let at_root = normalize_absolute(&root.join(path));
        (Path::new(path).is_relative() && at_root.starts_with(root) && at_root.exists())
            .then_some(at_root)
    };
    let absolute = if is_path_like(cwd, path) {
        let from_cwd = normalize_absolute(&cwd.join(path));
        if from_cwd.exists() {
            from_cwd
        } else {
            from_root().unwrap_or(from_cwd)
        }
    } else if let Some(at_root) = from_root() {
        // A bare word naming a top-level file or directory.
        at_root
    } else {
        return Positional {
            selector: arg.to_string(),
            path: None,
            id: rest.is_some(),
        };
    };
    let mut selector = root_relative(&absolute, root);
    if selector.is_empty() {
        selector.push('.');
    }
    if let Some(rest) = rest {
        selector.push_str("::");
        selector.push_str(rest);
    }
    Positional {
        selector,
        path: Some(absolute),
        id: rest.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use aye_fs::relative_below_root;

    use super::*;

    fn selector(cwd: &Path, root: &Path, arg: &str) -> String {
        resolve(cwd, root, arg).selector
    }

    #[test]
    fn selectors_are_made_relative_to_the_root() {
        let root = Path::new("/repo");
        let cwd = Path::new("/repo/tests");
        assert_eq!(
            selector(cwd, root, "test_a.py::test_x[1]"),
            "tests/test_a.py::test_x[1]"
        );
        assert_eq!(selector(cwd, root, "../src/"), "src");
        assert_eq!(selector(cwd, root, "pkg.mod.TestX"), "pkg.mod.TestX");
        assert_eq!(selector(root, root, "."), ".");
    }

    #[test]
    fn paths_outside_the_root_climb_out_instead_of_doubling_the_slash() {
        let root = Path::new("/repo/sub");
        assert_eq!(selector(root, root, "../x/test_a.py"), "../x/test_a.py");
        assert_eq!(relative_below_root(Path::new("/repo/x"), root), "../x");
        assert_eq!(relative_below_root(Path::new("/repo/sub/a/b"), root), "a/b");
    }

    #[test]
    fn a_path_only_the_root_has_is_taken_from_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("tests/sub")).unwrap();
        fs::write(root.join("tests/test_a.py"), "").unwrap();
        let cwd = root.join("tests");
        // From tests/, `tests/test_a.py` is the root's: the id aye printed.
        let id = resolve(&cwd, root, "tests/test_a.py::test_fail");
        assert_eq!(id.selector, "tests/test_a.py::test_fail");
        assert_eq!(
            id.path.as_deref(),
            Some(root.join("tests/test_a.py").as_path())
        );
        assert!(id.exists() && id.id);
        // The current directory comes first, as in pytest.
        assert_eq!(selector(&cwd, root, "test_a.py"), "tests/test_a.py");
        // A bare word naming a top-level directory, from below.
        let word = resolve(&root.join("tests/sub"), root, "tests");
        assert_eq!(word.selector, "tests");
        assert!(word.exists() && !word.id);
        // Neither has it: kept relative to the current directory.
        let missing = resolve(&cwd, root, "nope.py");
        assert_eq!(missing.selector, "tests/nope.py");
        assert!(!missing.exists());
        // Not a path: a dotted name or a key.
        let dotted = resolve(&cwd, root, "pkg.mod::x");
        assert_eq!(dotted.path, None);
        assert!(dotted.id);
    }

    #[test]
    fn a_root_reached_through_dot_dot_is_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("pyproject.toml"), "").unwrap();
        fs::write(root.join("tests/test_b.py"), "").unwrap();
        for (cwd, arg, want) in [
            (root.join("tests"), "..", "."),
            (root.join("src"), "../tests/test_b.py", "tests/test_b.py"),
            (
                root.clone(),
                "tests/../tests/test_b.py::t",
                "tests/test_b.py::t",
            ),
        ] {
            let given = [normalize_absolute(&cwd.join(path_part(arg)))];
            let found = normalize_absolute(&aye_config::project_root(&cwd, &given));
            assert_eq!(found, root, "{arg} from {}", cwd.display());
            assert_eq!(selector(&cwd, &found, arg), want);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_path_through_another_name_of_the_root_is_spelled_from_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("real");
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/test_a.py"), "").unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        // As macOS's `/tmp` is `/private/tmp`: the current directory reads as
        // the real path, the argument names the other.
        let given = alias.join("tests/test_a.py");
        let arg = format!("{}::test_x", given.display());
        let id = resolve(&root, &root, &arg);
        assert_eq!(id.selector, "tests/test_a.py::test_x");
        assert!(id.exists());
        assert_eq!(root_relative(&alias.join("tests"), &root), "tests");
        // Outside the root either way: climbs out, as before.
        assert_eq!(
            root_relative(&dir.path().join("elsewhere/x.py"), &root),
            "../elsewhere/x.py"
        );
    }
}
