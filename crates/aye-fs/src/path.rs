//! Lexical path algebra: normalizing, making absolute and relative, the
//! common ancestor, paths shown with `/`, and expansion.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// `path` without `.` and `..` components, folded lexically as Python's
/// `os.path.normpath` does: `..` at the top of an absolute path stays at the
/// top, and leading `..` of a relative path are kept. The empty path is `.`.
pub fn normpath(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// `path` without `.` and `..` components, each `..` removing the component
/// before it, if any, without touching the filesystem: `os.path.abspath`'s
/// folding for a path already absolute, where it is [`normpath`]. Of a
/// relative path, a leading `..` is dropped and the empty path stays empty.
pub fn normalize_absolute(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `path` made absolute against `cwd` and normalized, as Python's
/// `os.path.abspath` does: no filesystem access, symlinks untouched.
pub fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    normpath(&cwd.join(path))
}

/// `path` relative to `root` (both absolute): lexically when `path` is
/// spelled under `root`, otherwise through both resolved on the filesystem
/// (the deepest existing ancestor of `path`, then the rest as written), so
/// `/tmp/x/t.py` is under `/private/tmp/x`. `None` when it is not under
/// `root` either way.
pub fn relative_to(root: &Path, path: &Path) -> Option<PathBuf> {
    let (root, path) = (normpath(root), normpath(path));
    if let Ok(rest) = path.strip_prefix(&root) {
        return Some(rest.to_path_buf());
    }
    let root = fs::canonicalize(&root).ok()?;
    resolve_existing(&path)?
        .strip_prefix(&root)
        .ok()
        .map(Path::to_path_buf)
}

/// `path` with its deepest existing ancestor resolved on the filesystem and
/// the rest appended as written; `None` when nothing of it exists.
fn resolve_existing(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    for ancestor in path.ancestors() {
        if let Ok(resolved) = fs::canonicalize(ancestor) {
            return Some(rest.iter().rev().fold(resolved, |acc, part| acc.join(part)));
        }
        rest.push(ancestor.file_name()?);
    }
    None
}

/// The deepest directory containing every existing path in `paths` (files
/// count as their parent directory), or `cwd` when none exist. Paths are
/// relative to `cwd` or absolute, and are normalized lexically first (see
/// the crate docs); so is the result.
pub fn common_ancestor(cwd: &Path, paths: &[PathBuf]) -> PathBuf {
    let cwd = normpath(cwd);
    let dirs = existing_dirs(&cwd, paths);
    let Some(lexical) = deepest_common(&dirs) else {
        return cwd;
    };
    if lexical.parent().is_none() && dirs.len() > 1 {
        // Nothing in common but the filesystem's root: perhaps one directory
        // spelled two ways.
        if let Some(common) = resolved_common(&dirs)
            && common.parent().is_some()
        {
            return common;
        }
    }
    lexical
}

/// The deepest directory containing every existing path in `paths`, as
/// [`common_ancestor`] finds it but with each resolved on the filesystem;
/// `None` when fewer than two exist or one does not resolve. For the
/// project root, which looks here when two spellings of one directory share
/// more than the filesystem's root.
pub fn resolved_common_ancestor(cwd: &Path, paths: &[PathBuf]) -> Option<PathBuf> {
    let dirs = existing_dirs(&normpath(cwd), paths);
    if dirs.len() > 1 {
        resolved_common(&dirs)
    } else {
        None
    }
}

/// The existing paths of `paths` made absolute against `cwd` (already
/// normalized), files as their parent directory.
fn existing_dirs(cwd: &Path, paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|p| absolute(cwd, p))
        .filter(|p| p.exists())
        .map(|p| {
            if p.is_file() {
                p.parent().map_or(p.clone(), Path::to_path_buf)
            } else {
                p
            }
        })
        .collect()
}

/// The deepest common ancestor of `dirs` resolved on the filesystem;
/// `None` when one of them does not resolve.
fn resolved_common(dirs: &[PathBuf]) -> Option<PathBuf> {
    let resolved: Option<Vec<PathBuf>> = dirs.iter().map(|d| fs::canonicalize(d).ok()).collect();
    deepest_common(&resolved?)
}

/// The deepest common ancestor of absolute paths, by whole components.
fn deepest_common(dirs: &[PathBuf]) -> Option<PathBuf> {
    let (first, rest) = dirs.split_first()?;
    Some(rest.iter().fold(first.clone(), |acc, dir| {
        acc.ancestors()
            .find(|a| dir.starts_with(a))
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
    }))
}

/// A relative path's components joined with `/`.
pub fn slashed(path: &Path) -> String {
    path.components()
        .filter(|c| !matches!(c, Component::RootDir | Component::Prefix(_)))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` relative to `root` with `/` separators, climbing out with `..`
/// when it lies elsewhere and the two share their first component, the
/// filesystem's root included: `/x` from `/a/b` is `../../x`. Collection's
/// ids are spelled so.
pub fn relative_through_root(path: &Path, root: &Path) -> String {
    let parts = |path: &Path| -> Vec<String> {
        path.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    if let Ok(rest) = path.strip_prefix(root) {
        return parts(rest).join("/");
    }
    let (path_parts, root_parts) = (parts(path), parts(root));
    let common = path_parts
        .iter()
        .zip(&root_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return path.to_string_lossy().replace('\\', "/");
    }
    std::iter::repeat_n("..".to_string(), root_parts.len() - common)
        .chain(path_parts[common..].iter().cloned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` relative to `base` with `/` separators, climbing out with `..`
/// when it lies elsewhere below a directory they share; absolute when they
/// share nothing but the filesystem's root. Both are absolute and
/// normalized. The binary's selectors are spelled so.
pub fn relative_below_root(path: &Path, base: &Path) -> String {
    if let Ok(rest) = path.strip_prefix(base) {
        return slashed(rest);
    }
    let shared = path
        .components()
        .zip(base.components())
        .take_while(|(a, b)| a == b)
        .count();
    let anchored = path
        .components()
        .take(shared)
        .any(|c| matches!(c, Component::Normal(_)));
    if !anchored {
        return path.to_string_lossy().replace('\\', "/");
    }
    let path_parts: Vec<Component> = path.components().collect();
    let up = base.components().count() - shared;
    std::iter::repeat_n("..".to_string(), up)
        .chain(
            path_parts[shared..]
                .iter()
                .map(|c| c.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// `path` relative to the root as collection's ids spell it: also when it
/// is spelled through another name of a directory in the project (macOS's
/// `/tmp` is `/private/tmp`), by [`relative_to`]; else
/// [`relative_below_root`], climbing out with `..`.
pub fn root_relative(path: &Path, root: &Path) -> String {
    relative_to(root, path).map_or_else(|| relative_below_root(path, root), |rest| slashed(&rest))
}

/// `os.path.expanduser(os.path.expandvars(path))`, as pytest reads a
/// report's path: `$NAME` and `${NAME}` become the variable's value when
/// `var` finds it and it is Unicode, then a leading `~` becomes the home
/// directory, `HOME` or else `USERPROFILE` (`~user` is left as it is).
pub fn expand_path(path: &str, var: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let word = |s: &str| {
            s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(s.len())
        };
        // The name, and how long the reference is after the `$`.
        let reference = match after.strip_prefix('{') {
            Some(braced) => braced.find('}').map(|end| (&braced[..end], end + 2)),
            None => Some(word(after))
                .filter(|end| *end > 0)
                .map(|end| (&after[..end], end)),
        };
        let Some((name, len)) = reference else {
            out.push('$');
            rest = after;
            continue;
        };
        match var(name).and_then(|value| value.into_string().ok()) {
            Some(value) if !name.is_empty() => out.push_str(&value),
            _ => out.push_str(&rest[at..=at + len]),
        }
        rest = &after[len..];
    }
    out.push_str(rest);
    let separators = ['/', std::path::MAIN_SEPARATOR];
    if let Some(tail) = out.strip_prefix('~')
        && (tail.is_empty() || tail.starts_with(separators))
        && let Some(home) = var("HOME").or_else(|| var("USERPROFILE"))
    {
        let home = home.to_string_lossy();
        let home = home.trim_end_matches(separators);
        let expanded = format!("{home}{tail}");
        return PathBuf::from(if expanded.is_empty() { "/" } else { &expanded });
    }
    PathBuf::from(out)
}

#[cfg(test)]
mod tests;
