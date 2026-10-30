//! pytest's `_pytest/pathlib.py`: where a file's packages end and the
//! name pytest imports it under. Each adapter's reading of a package
//! directory's name is its own: `str.isidentifier()` on any name, or on
//! ASCII names only, a name that isn't ASCII being unknowable.

use std::path::{Path, PathBuf};

use aye_python_stdlib::identifier::{is_ascii_identifier, is_identifier};

/// pytest's `resolve_package_path`: the last directory upward from `file`
/// that is still a package with an identifier for a name.
pub fn resolve_package_path(file: &Path) -> Option<PathBuf> {
    let mut found = None;
    for dir in file.ancestors().skip(1) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        if !dir.join("__init__.py").is_file() || !is_identifier(&name) {
            break;
        }
        found = Some(dir.to_path_buf());
    }
    found
}

/// pytest's `compute_module_name`: `file`'s dotted name from `root`.
pub fn compute_module_name(root: &Path, file: &Path) -> Option<String> {
    let rel = file.with_extension("");
    let rel = rel.strip_prefix(root).ok()?;
    let mut names: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if names.last().is_some_and(|n| n == "__init__") {
        names.pop();
    }
    (!names.is_empty()).then(|| names.join("."))
}

/// `str.isidentifier()` for the ASCII names it is asked about here, and
/// `None` for any other name (Python's rules for those are Unicode's).
fn ascii_identifier(name: &str) -> Option<bool> {
    if !name.is_ascii() {
        return None;
    }
    Some(is_ascii_identifier(name))
}

/// The name pytest's `prepend` import mode imports a file under, and the
/// directory it puts on `sys.path`: pytest's `resolve_pkg_root_and_module_name`.
/// `Err` holds why the name can't be known statically.
pub fn resolve_pkg_root_and_module_name(file: &Path) -> Result<(String, PathBuf), String> {
    let mut package: Option<&Path> = None;
    for dir in file.ancestors().skip(1) {
        if !dir.join("__init__.py").is_file() {
            break;
        }
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match ascii_identifier(&name) {
            Some(true) => package = Some(dir),
            Some(false) => break,
            None => return Err(format!("the package name {name:?} is not ASCII")),
        }
    }
    let stem = file
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if stem.contains('.') {
        return Err(format!("the file name {stem:?} holds a dot"));
    }
    let Some(package) = package else {
        let dir = file.parent().unwrap_or(file).to_path_buf();
        return Ok((stem, dir));
    };
    let base = package.parent().unwrap_or(package);
    let rel = file.with_extension("");
    let rel = rel.strip_prefix(base).unwrap_or(&rel);
    let mut names: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if names.last().is_some_and(|n| n == "__init__") {
        names.pop();
    }
    Ok((names.join("."), base.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn module_names_follow_pytests_prepend_mode() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "src/app/__init__.py", "");
        write(root, "src/app/core/__init__.py", "");
        write(root, "src/app/core/models.py", "");
        write(root, "scripts/tool.py", "");
        write(root, "my-dir/__init__.py", "");
        write(root, "my-dir/inner.py", "");
        write(root, "loose/foo.bar.py", "");
        let name = |rel: &str| resolve_pkg_root_and_module_name(&root.join(rel));
        assert_eq!(
            name("src/app/core/models.py").unwrap(),
            ("app.core.models".to_string(), root.join("src"))
        );
        assert_eq!(
            name("src/app/__init__.py").unwrap(),
            ("app".to_string(), root.join("src"))
        );
        assert_eq!(
            name("scripts/tool.py").unwrap(),
            ("tool".to_string(), root.join("scripts"))
        );
        // `my-dir` isn't an identifier: no package.
        assert_eq!(
            name("my-dir/inner.py").unwrap(),
            ("inner".to_string(), root.join("my-dir"))
        );
        assert_eq!(
            name("my-dir/__init__.py").unwrap(),
            ("__init__".to_string(), root.join("my-dir"))
        );
        assert!(name("loose/foo.bar.py").is_err());
    }
}
