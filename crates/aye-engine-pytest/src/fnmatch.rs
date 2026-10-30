//! Python's `fnmatch` and `glob`, as pytest uses them.
//!
//! `globset` is not used on purpose: it reads `{a,b}` as alternation, while
//! `fnmatch` treats braces literally (pytest's default `norecursedirs` holds
//! the literal `{arch}`).

use std::path::{Path, PathBuf};

/// `fnmatch.fnmatch(name, pattern)`: `*`, `?`, `[seq]` and `[!seq]`, no
/// escapes. Both go through `os.path.normcase` first, which on Windows
/// folds case and turns `/` into `\`.
pub(crate) fn fnmatch(name: &str, pattern: &str) -> bool {
    if cfg!(windows) {
        return fnmatchcase(&nt_normcase(name), &nt_normcase(pattern));
    }
    fnmatchcase(name, pattern)
}

/// `fnmatch.fnmatchcase(name, pattern)`.
fn fnmatchcase(name: &str, pattern: &str) -> bool {
    matches(
        &name.chars().collect::<Vec<_>>(),
        &compile(&pattern.chars().collect::<Vec<_>>()),
    )
}

/// `ntpath.normcase`: lower case, `\` separators.
fn nt_normcase(s: &str) -> String {
    s.replace('/', "\\").to_lowercase()
}

/// pytest's `fnmatch_ex`: a pattern without a separator matches the file
/// name; one with a separator matches the whole path, anchored anywhere when
/// the path is absolute and the pattern is not.
pub(crate) fn fnmatch_ex(pattern: &str, path: &Path) -> bool {
    let sep = std::path::MAIN_SEPARATOR;
    let pattern = if cfg!(windows) && !pattern.contains(sep) && pattern.contains('/') {
        pattern.replace('/', &sep.to_string())
    } else {
        pattern.to_string()
    };
    if pattern.contains(sep) {
        let full = path.to_string_lossy();
        if path.is_absolute() && !Path::new(&pattern).is_absolute() {
            fnmatch(&full, &format!("*{sep}{pattern}"))
        } else {
            fnmatch(&full, &pattern)
        }
    } else {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        fnmatch(&name, &pattern)
    }
}

/// Whether a `python_classes` or `python_functions` option matches `name`:
/// a prefix, or an `fnmatch` pattern when it holds glob characters.
pub(crate) fn matches_prefix_or_glob(options: &[String], name: &str) -> bool {
    options.iter().any(|option| {
        name.starts_with(option.as_str())
            || (option.contains(['*', '?', '[']) && fnmatch(name, option))
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Char(char),
    Any,
    Star,
    Class {
        negated: bool,
        items: Vec<(char, char)>,
    },
}

/// Translate a pattern the way `fnmatch.translate` does.
fn compile(pattern: &[char]) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < pattern.len() {
        let c = pattern[i];
        i += 1;
        match c {
            '*' => {
                if tokens.last() != Some(&Token::Star) {
                    tokens.push(Token::Star);
                }
            }
            '?' => tokens.push(Token::Any),
            '[' => {
                let mut j = i;
                if j < pattern.len() && pattern[j] == '!' {
                    j += 1;
                }
                if j < pattern.len() && pattern[j] == ']' {
                    j += 1;
                }
                while j < pattern.len() && pattern[j] != ']' {
                    j += 1;
                }
                if j >= pattern.len() {
                    tokens.push(Token::Char('['));
                    continue;
                }
                let mut stuff = &pattern[i..j];
                i = j + 1;
                let negated = stuff.first() == Some(&'!');
                if negated {
                    stuff = &stuff[1..];
                }
                let mut items = Vec::new();
                let mut k = 0;
                while k < stuff.len() {
                    if k + 2 < stuff.len() && stuff[k + 1] == '-' {
                        // An empty range (`z-a`) matches nothing.
                        if stuff[k] <= stuff[k + 2] {
                            items.push((stuff[k], stuff[k + 2]));
                        }
                        k += 3;
                    } else {
                        items.push((stuff[k], stuff[k]));
                        k += 1;
                    }
                }
                tokens.push(Token::Class { negated, items });
            }
            other => tokens.push(Token::Char(other)),
        }
    }
    tokens
}

fn matches(name: &[char], tokens: &[Token]) -> bool {
    // Iterative matching with backtracking to the last star.
    let (mut n, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while n < name.len() {
        let step = match tokens.get(t) {
            Some(Token::Star) => {
                star = Some((t, n));
                t += 1;
                continue;
            }
            Some(Token::Any) => true,
            Some(Token::Char(c)) => *c == name[n],
            Some(Token::Class { negated, items }) => {
                let inside = items.iter().any(|(lo, hi)| (*lo..=*hi).contains(&name[n]));
                inside != *negated
            }
            None => false,
        };
        if step {
            n += 1;
            t += 1;
        } else if let Some((star_t, star_n)) = star {
            t = star_t + 1;
            n = star_n + 1;
            star = Some((star_t, star_n + 1));
        } else {
            return false;
        }
    }
    tokens[t..].iter().all(|token| *token == Token::Star)
}

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
        .filter(|name| fnmatch(name, pattern))
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
    fn fnmatch_follows_python() {
        assert!(fnmatch("test_x.py", "test_*.py"));
        assert!(fnmatch("x_test.py", "*_test.py"));
        assert!(!fnmatch("test_x.pyc", "test_*.py"));
        assert!(fnmatch(".git", ".*"));
        assert!(fnmatch("{arch}", "{arch}"));
        assert!(!fnmatch("arch", "{arch}"));
        assert!(fnmatch("a.egg", "*.egg"));
        assert!(fnmatch("abc", "a?c"));
        assert!(fnmatch("abc", "a[bx]c"));
        assert!(!fnmatch("abc", "a[!b]c"));
        assert!(fnmatch("a]c", "a[]]c"));
        assert!(fnmatch("a-c", "a[x-]c"));
        assert!(fnmatch("a[c", "a[c"));
        assert!(fnmatch("a/b/c", "a*c"));
        assert!(fnmatch("", "*"));
        assert!(!fnmatch("", "?"));
        assert!(fnmatch("aXbXc", "a*b*c"));
        assert!(!fnmatch("aXbXd", "a*b*c"));
        assert!(fnmatch("b", "[a-c]"));
        assert!(!fnmatch("b", "[c-a]"));
    }

    #[test]
    fn fnmatch_folds_case_and_separators_on_windows() {
        assert_eq!(fnmatch("Test_X.py", "test_*.py"), cfg!(windows));
        assert_eq!(fnmatch("a/b.py", r"a\*.py"), cfg!(windows));
        assert_eq!(fnmatch(r"a\b.py", "A/*.py"), cfg!(windows));
        assert!(fnmatch("a/b.py", "a/*.py"));
    }

    #[test]
    fn fnmatch_ex_uses_the_name_or_the_whole_path() {
        // Absolute, with a drive on Windows; joined as aye joins paths.
        let root = if cfg!(windows) { r"C:\repo" } else { "/repo" };
        let path = &Path::new(root).join("tests/foo/bar/doc/test_foo.py");
        assert!(fnmatch_ex("test_*.py", path));
        assert!(fnmatch_ex("tests/**/doc/test*.py", path));
        assert!(fnmatch_ex("foo/*/doc/*.py", path));
        assert!(!fnmatch_ex("other/*.py", path));
        assert_eq!(fnmatch_ex("TESTS/**/doc/Test*.py", path), cfg!(windows));
    }

    #[test]
    fn name_options_are_prefixes_or_globs() {
        let options = vec!["Test".to_string(), "*Suite".to_string()];
        assert!(matches_prefix_or_glob(&options, "TestThing"));
        assert!(matches_prefix_or_glob(&options, "LoginSuite"));
        assert!(!matches_prefix_or_glob(&options, "Helper"));
    }

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
