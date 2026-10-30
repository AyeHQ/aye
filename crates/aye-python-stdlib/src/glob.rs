//! Python's `glob.glob(pattern, recursive=True)` from a directory, as
//! pytest expands `testpaths`, and the doctest adapter's approximation of
//! it, which differs from it on purpose: each adapter asks for the one it
//! had.

use std::path::{Path, PathBuf};

use crate::fnmatch::{SetTranslation, fnmatch};

fn has_magic(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// `sorted(glob.iglob(pattern, recursive=True))`, relative to `base`, its
/// names matched by `fnmatch` with sets read the `sets` way. Results are
/// spelled as Python spells them (`tests/` for `tests/**`).
pub fn glob(base: &Path, pattern: &str, sets: SetTranslation) -> Vec<String> {
    let mut out = iglob(base, pattern, false, sets);
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

fn iglob(base: &Path, pathname: &str, dironly: bool, sets: SetTranslation) -> Vec<String> {
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
            glob1(base, "", basename, dironly, sets)
        };
    }
    let dirs = if dirname != pathname && has_magic(dirname) {
        iglob(base, dirname, true, sets)
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
            glob1(base, &dir, basename, dironly, sets)
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

fn glob1(
    base: &Path,
    dirname: &str,
    pattern: &str,
    dironly: bool,
    sets: SetTranslation,
) -> Vec<String> {
    listdir(base, dirname, dironly)
        .into_iter()
        .filter(|name| is_hidden(pattern) || !is_hidden(name))
        .filter(|name| fnmatch(name, pattern, sets))
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

/// The doctest adapter's approximation of `glob.glob(pattern,
/// recursive=True)` from `base`: the existing paths matching `pattern`,
/// `**` matching any depth, names matched by `fnmatch` with sets read the
/// `sets` way. It differs from [`glob`]: its paths are absolute
/// (`os.path.abspath`'s), sorted as a whole and without duplicates; `**`
/// takes directories only, the one it starts from among them, so
/// `tests/**` gives no files and `**` gives `base`; and a trailing `/` is
/// dropped, so `tests/*/` gives files too.
pub fn glob_absolute(base: &Path, pattern: &str, sets: SetTranslation) -> Vec<PathBuf> {
    let absolute = aye_fs::absolute(base, Path::new(pattern));
    let has_magic = |s: &str| s.contains(['*', '?', '[']);
    if !has_magic(pattern) {
        return if absolute.exists() {
            vec![absolute]
        } else {
            Vec::new()
        };
    }
    // Split the pattern after the longest literal prefix.
    let mut prefix = PathBuf::new();
    let mut parts: Vec<String> = Vec::new();
    for component in absolute.components() {
        let text = component.as_os_str().to_string_lossy().into_owned();
        if parts.is_empty() && !has_magic(&text) {
            prefix.push(component);
        } else {
            parts.push(text);
        }
    }
    let mut found = Vec::new();
    expand(&prefix, &parts, sets, &mut found);
    // `sorted(glob.iglob(...))`: by the whole path.
    found.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));
    found.dedup();
    found
}

fn expand(dir: &Path, parts: &[String], sets: SetTranslation, out: &mut Vec<PathBuf>) {
    let Some((part, rest)) = parts.split_first() else {
        if dir.exists() {
            out.push(dir.to_path_buf());
        }
        return;
    };
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    if part == "**" {
        // Zero directories, then each subdirectory, recursively (hidden
        // ones aside, as glob does).
        expand(dir, rest, sets, out);
        for entry in &entries {
            let hidden = entry
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if entry.is_dir() && !hidden {
                expand(entry, parts, sets, out);
            }
        }
        return;
    }
    for entry in entries {
        let name = entry
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name.starts_with('.') && !part.starts_with('.') {
            continue;
        }
        if fnmatch(&name, part, sets) && (rest.is_empty() || entry.is_dir()) {
            expand(&entry, rest, sets, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use SetTranslation::Legacy;

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
        assert_eq!(glob(root, "tests", Legacy), ["tests"]);
        assert_eq!(glob(root, "missing", Legacy), Vec::<String>::new());
        assert_eq!(glob(root, "t*", Legacy), ["tests"]);
        assert_eq!(
            glob(root, "tests/**", Legacy),
            [
                "tests/",
                "tests/test_a.py",
                "tests/unit",
                "tests/unit/test_b.py"
            ]
        );
        assert_eq!(
            glob(root, "**/test_*.py", Legacy),
            ["tests/test_a.py", "tests/unit/test_b.py"]
        );
        assert_eq!(glob(root, "*/unit", Legacy), ["tests/unit"]);
        assert_eq!(glob(root, "tests/.*", Legacy), ["tests/.hidden"]);
        assert_eq!(glob(root, "tests/", Legacy), ["tests/"]);
        assert_eq!(glob(root, "tests/*/", Legacy), ["tests/unit/"]);
        assert_eq!(
            glob(root, "**", Legacy),
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

    #[test]
    fn the_absolute_glob_approximates_it() {
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
        // As the doctest adapter's copy gave them.
        let found = |pattern: &str| -> Vec<PathBuf> { glob_absolute(root, pattern, Legacy) };
        let under =
            |rels: &[&str]| -> Vec<PathBuf> { rels.iter().map(|rel| root.join(rel)).collect() };
        assert_eq!(found("tests"), under(&["tests"]));
        assert_eq!(found("missing"), under(&[]));
        assert_eq!(found("t*"), under(&["tests"]));
        assert_eq!(found("tests/**"), under(&["tests", "tests/unit"]));
        assert_eq!(
            found("**/test_*.py"),
            under(&["tests/test_a.py", "tests/unit/test_b.py"])
        );
        assert_eq!(found("*/unit"), under(&["tests/unit"]));
        assert_eq!(found("tests/.*"), under(&["tests/.hidden"]));
        assert_eq!(found("tests/"), under(&["tests"]));
        assert_eq!(found("tests/*/"), under(&["tests/test_a.py", "tests/unit"]));
        assert_eq!(found("**"), under(&["", "other", "tests", "tests/unit"]));
        assert_eq!(found("tests/../other"), under(&["other"]));
        assert_eq!(found("[ot]*/x.py"), under(&["other/x.py"]));
        assert_eq!(
            found("**/*.py"),
            under(&["other/x.py", "tests/test_a.py", "tests/unit/test_b.py"])
        );
    }
}
