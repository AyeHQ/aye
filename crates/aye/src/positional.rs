//! The positional arguments, as a run and `aye show` read them
//! ([`Positional`], each run's all of them: [`Positionals`]): a path taken
//! from the current directory, or from the project root when only the root
//! has it; a test id in such a file; else a word, a dotted name or a key.
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
use aye_select::SelectorIndex;

/// A positional argument, read: a file or directory, a test in a file, or
/// a word. What each gives selection is its [`selector`](Self::selector).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Positional {
    /// A file or directory, whether it exists or not.
    Path {
        /// The path, as given.
        given: String,
        /// The file or directory, absolute and normalized.
        absolute: PathBuf,
        /// Relative to the root, with `/` separators; `.` for the root, and
        /// climbing out with `..` for a path outside it.
        selector: String,
    },
    /// A test in a file: the argument had a `::` part.
    Id {
        /// The file's path, as given (before `::`).
        given: String,
        /// The file, absolute and normalized.
        file: PathBuf,
        /// The file relative to the root, as a path's selector, then the
        /// `::` part, as given.
        selector: String,
    },
    /// Not a path: a dotted name or a key (or anything else that names no
    /// file), as given, any `::` part included.
    Word(String),
}

impl Positional {
    /// Read a positional argument: a path is taken from the current
    /// directory, as pytest does, or from the project root when only the
    /// root has it, so that the ids aye prints (which are relative to the
    /// root) work from any directory. Its selector is relative to the root,
    /// keeping any `::` part; dotted names and keys pass through.
    pub(crate) fn resolve(cwd: &Path, root: &Path, arg: &str) -> Self {
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
            return Self::Word(arg.to_string());
        };
        let mut selector = root_relative(&absolute, root);
        if selector.is_empty() {
            selector.push('.');
        }
        let given = path.to_string();
        match rest {
            Some(rest) => {
                selector.push_str("::");
                selector.push_str(rest);
                Self::Id {
                    given,
                    file: absolute,
                    selector,
                }
            }
            None => Self::Path {
                given,
                absolute,
                selector,
            },
        }
    }

    /// What selection gets of the argument.
    pub(crate) fn selector(&self) -> &str {
        match self {
            Self::Path { selector, .. } | Self::Id { selector, .. } => selector,
            Self::Word(word) => word,
        }
    }

    /// A path's file or directory, or an id's file: absolute, normalized.
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Path { absolute, .. } => Some(absolute),
            Self::Id { file, .. } => Some(file),
            Self::Word(_) => None,
        }
    }

    /// Whether the argument's file or directory exists.
    pub(crate) fn exists(&self) -> bool {
        self.path().is_some_and(Path::exists)
    }

    /// Whether the argument is an id with parameters (`path::test[x]`).
    pub(crate) fn has_params(&self) -> bool {
        match self {
            Self::Id { selector, .. } => selector
                .split_once("::")
                .is_some_and(|(_, rest)| rest.contains('[')),
            Self::Path { .. } | Self::Word(_) => false,
        }
    }

    /// A path's or an id's file as the adapters take it
    /// ([`aye_adapter::Project::args`]): as given when that is what it names
    /// from `cwd`, else absolute (only the project root makes sense of it).
    fn project_arg(&self, cwd: &Path) -> Option<PathBuf> {
        let (given, path) = match self {
            Self::Path {
                given, absolute, ..
            } => (given, absolute),
            Self::Id { given, file, .. } => (given, file),
            Self::Word(_) => return None,
        };
        Some(if normalize_absolute(&cwd.join(given)) == *path {
            PathBuf::from(given)
        } else {
            path.clone()
        })
    }
}

/// A run's positional arguments, each read, in the order given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Positionals(Vec<Positional>);

impl Positionals {
    /// Read each of `args` ([`Positional::resolve`]).
    pub(crate) fn resolve(cwd: &Path, root: &Path, args: &[String]) -> Self {
        args.iter()
            .map(|arg| Positional::resolve(cwd, root, arg))
            .collect()
    }

    pub(crate) fn iter(&self) -> std::slice::Iter<'_, Positional> {
        self.0.iter()
    }

    /// Whether every argument is a path or an id, as with none: collection
    /// can then be limited to them, and they reach all it finds.
    pub(crate) fn only_paths(&self) -> bool {
        self.iter().all(|positional| positional.path().is_some())
    }

    /// The paths and the ids' files, in order, as the adapters take them
    /// ([`aye_adapter::Project::args`]): as given, or absolute when only
    /// the project root makes sense of them.
    pub(crate) fn project_args(&self, cwd: &Path) -> Vec<PathBuf> {
        self.iter()
            .filter_map(|positional| positional.project_arg(cwd))
            .collect()
    }

    /// What static selection gets: each argument's selector, in order.
    pub(crate) fn selectors(&self) -> impl Iterator<Item = &str> {
        self.iter().map(Positional::selector)
    }

    /// The argument the run reports for selection's selector `at` (the
    /// run's selection has one per argument, in order): the first that
    /// reads alike. Selection reports a word it reads more than one way
    /// each time it is given, and the run reports each as the first.
    pub(crate) fn first_alike(&self, at: SelectorIndex) -> &Positional {
        let selector = self.0[at.as_usize()].selector();
        self.iter()
            .find(|positional| positional.selector() == selector)
            .unwrap_or(&self.0[at.as_usize()])
    }
}

impl FromIterator<Positional> for Positionals {
    fn from_iter<I: IntoIterator<Item = Positional>>(positionals: I) -> Self {
        Self(positionals.into_iter().collect())
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

#[cfg(test)]
mod tests {
    use std::fs;

    use aye_fs::relative_below_root;

    use super::*;

    fn selector(cwd: &Path, root: &Path, arg: &str) -> String {
        Positional::resolve(cwd, root, arg).selector().to_owned()
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
        let id = Positional::resolve(&cwd, root, "tests/test_a.py::test_fail");
        assert_eq!(id.selector(), "tests/test_a.py::test_fail");
        assert_eq!(id.path(), Some(root.join("tests/test_a.py").as_path()));
        assert!(id.exists() && matches!(id, Positional::Id { .. }));
        // The current directory comes first, as in pytest.
        assert_eq!(selector(&cwd, root, "test_a.py"), "tests/test_a.py");
        // A bare word naming a top-level directory, from below.
        let word = Positional::resolve(&root.join("tests/sub"), root, "tests");
        assert_eq!(word.selector(), "tests");
        assert!(word.exists() && matches!(word, Positional::Path { .. }));
        // Neither has it: kept relative to the current directory.
        let missing = Positional::resolve(&cwd, root, "nope.py");
        assert_eq!(missing.selector(), "tests/nope.py");
        assert!(!missing.exists());
        // Not a path: a dotted name or a key, `::` part and all.
        let dotted = Positional::resolve(&cwd, root, "pkg.mod::x");
        assert_eq!(dotted, Positional::Word("pkg.mod::x".to_owned()));
        assert_eq!(dotted.path(), None);
    }

    /// The adapters get each path as given when the current directory has
    /// it, else absolute; words give them nothing, and only paths and ids
    /// are only paths.
    #[test]
    fn the_adapters_get_paths_as_given_unless_only_the_root_has_them() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("tests/test_a.py"), "").unwrap();
        let cwd = root.join("tests");
        let args =
            |args: &[&str]| -> Vec<String> { args.iter().map(|a| (*a).to_owned()).collect() };
        let positionals = Positionals::resolve(
            &cwd,
            root,
            &args(&[
                "test_a.py::t",
                "../tests/./test_a.py",
                "tests/test_a.py",
                "nope.py",
            ]),
        );
        assert!(positionals.only_paths());
        assert_eq!(
            positionals.project_args(&cwd),
            [
                PathBuf::from("test_a.py"),
                PathBuf::from("../tests/./test_a.py"),
                root.join("tests/test_a.py"),
                PathBuf::from("nope.py"),
            ]
        );
        let mixed = Positionals::resolve(&cwd, root, &args(&["test_a.py", "pkg.mod"]));
        assert!(!mixed.only_paths());
        assert_eq!(mixed.project_args(&cwd), [PathBuf::from("test_a.py")]);
        assert!(Positionals::default().only_paths());
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
        let id = Positional::resolve(&root, &root, &arg);
        assert_eq!(id.selector(), "tests/test_a.py::test_x");
        assert!(id.exists());
        assert_eq!(root_relative(&alias.join("tests"), &root), "tests");
        // Outside the root either way: climbs out, as before.
        assert_eq!(
            root_relative(&dir.path().join("elsewhere/x.py"), &root),
            "../elsewhere/x.py"
        );
    }
}
