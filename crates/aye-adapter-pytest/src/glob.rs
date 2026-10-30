//! Python's `glob`, as pytest expands `testpaths`.

use std::path::{Path, PathBuf};

use aye_python_stdlib::fnmatch::fnmatch;

use crate::SETS;

fn has_magic(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// `sorted(glob.iglob(pattern, recursive=True))`, relative to `base`.
/// Results are spelled as Python spells them (`tests/` for `tests/**`).
pub(crate) fn glob(base: &Path, pattern: &str) -> Vec<String> {
    let mut out = iglob(base, pattern, false);
    // `iglob` drops the empty match `**` makes for the base itself.
    if (pattern.is_empty() || pattern.starts_with("**"))
        && out.first().is_some_and(String::is_empty)
    {
        out.remove(0);
    }
    out.sort();
    out
}

fn split(pathname: &str) -> (&str, &str) {
    match pathname.rfind('/') {
        Some(at) => {
            let head = &pathname[..at];
            let head = if head.is_empty() || head.chars().all(|c| c == '/') {
                &pathname[..=at]
            } else {
                head.trim_end_matches('/')
            };
            (head, &pathname[at + 1..])
        }
        None => ("", pathname),
    }
}

fn join(dirname: &str, name: &str) -> String {
    if dirname.is_empty() {
        name.to_string()
    } else if dirname.ends_with('/') {
        format!("{dirname}{name}")
    } else {
        format!("{dirname}/{name}")
    }
}

fn resolve(base: &Path, rel: &str) -> PathBuf {
    if rel.is_empty() {
        base.to_path_buf()
    } else {
        base.join(rel)
    }
}

fn iglob(base: &Path, pathname: &str, dironly: bool) -> Vec<String> {
    let (dirname, basename) = split(pathname);
    if !has_magic(pathname) {
        let exists = if basename.is_empty() {
            resolve(base, dirname).is_dir()
        } else {
            let path = resolve(base, pathname);
            path.symlink_metadata().is_ok()
        };
        return if exists {
            vec![pathname.to_string()]
        } else {
            Vec::new()
        };
    }
    if dirname.is_empty() {
        return if basename == "**" {
            glob2(base, "", dironly)
        } else {
            glob1(base, "", basename, dironly)
        };
    }
    let dirs = if dirname != pathname && has_magic(dirname) {
        iglob(base, dirname, true)
    } else {
        vec![dirname.to_string()]
    };
    let mut out = Vec::new();
    for dir in dirs {
        let names = if !has_magic(basename) {
            glob0(base, &dir, basename)
        } else if basename == "**" {
            glob2(base, &dir, dironly)
        } else {
            glob1(base, &dir, basename, dironly)
        };
        out.extend(names.into_iter().map(|name| join(&dir, &name)));
    }
    out
}

fn listdir(base: &Path, dirname: &str, dironly: bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(resolve(base, dirname)) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| !dironly || entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

fn glob0(base: &Path, dirname: &str, basename: &str) -> Vec<String> {
    let exists = if basename.is_empty() {
        resolve(base, dirname).is_dir()
    } else {
        resolve(base, &join(dirname, basename))
            .symlink_metadata()
            .is_ok()
    };
    if exists {
        vec![basename.to_string()]
    } else {
        Vec::new()
    }
}

fn glob1(base: &Path, dirname: &str, pattern: &str, dironly: bool) -> Vec<String> {
    listdir(base, dirname, dironly)
        .into_iter()
        .filter(|name| is_hidden(pattern) || !is_hidden(name))
        .filter(|name| fnmatch(name, pattern, SETS))
        .collect()
}

fn glob2(base: &Path, dirname: &str, dironly: bool) -> Vec<String> {
    let mut out = Vec::new();
    if dirname.is_empty() || resolve(base, dirname).is_dir() {
        out.push(String::new());
    }
    rlistdir(base, dirname, dironly, &mut out, "");
    out
}

fn rlistdir(base: &Path, dirname: &str, dironly: bool, out: &mut Vec<String>, prefix: &str) {
    for name in listdir(base, dirname, dironly) {
        if is_hidden(&name) {
            continue;
        }
        let shown = join(prefix, &name);
        out.push(shown.clone());
        let path = join(dirname, &name);
        rlistdir(base, &path, dironly, out, &shown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_pythons_recursive_glob() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for rel in [
            "tests/test_a.py",
            "tests/unit/test_b.py",
            "tests/.hidden/test_c.py",
            "other/x.py",
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        assert_eq!(glob(root, "tests"), ["tests"]);
        assert_eq!(glob(root, "missing"), Vec::<String>::new());
        assert_eq!(glob(root, "t*"), ["tests"]);
        assert_eq!(
            glob(root, "tests/**"),
            [
                "tests/",
                "tests/test_a.py",
                "tests/unit",
                "tests/unit/test_b.py"
            ]
        );
        assert_eq!(
            glob(root, "**/test_*.py"),
            ["tests/test_a.py", "tests/unit/test_b.py"]
        );
        assert_eq!(glob(root, "*/unit"), ["tests/unit"]);
        assert_eq!(glob(root, "tests/.*"), ["tests/.hidden"]);
        assert_eq!(glob(root, "tests/"), ["tests/"]);
        assert_eq!(glob(root, "tests/*/"), ["tests/unit/"]);
        assert_eq!(
            glob(root, "**"),
            [
                "other",
                "other/x.py",
                "tests",
                "tests/test_a.py",
                "tests/unit",
                "tests/unit/test_b.py"
            ]
        );
    }
}
