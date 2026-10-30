//! Python's import system, on the filesystem: the file an import finds,
//! looked up as `FileFinder` does, and where a chain of packages ends.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ruff_python_ast::visitor::{self, Visitor};
use ruff_python_ast::{ModModule, Stmt};

/// A module name as code in `dir` imports it: relative to `dir`'s package
/// when it starts with dots, else from `sys.path` as pytest's import mode
/// leaves it: the directory `prepend` puts first for `dir` (its package
/// base), then `bases`; with `append` and `importlib` (`bases_first`),
/// `bases` first, then that directory (where `append` puts it, and the
/// last place `importlib` could find it).
pub fn resolve_from(
    name: &str,
    dir: &Path,
    own: &Path,
    bases: &[PathBuf],
    bases_first: bool,
) -> Option<PathBuf> {
    let level = name.chars().take_while(|c| *c == '.').count();
    if level > 0 {
        let base = dir.ancestors().nth(level - 1)?;
        let rest = &name[level..];
        if rest.is_empty() {
            let init = base.join("__init__.py");
            return init.is_file().then_some(init);
        }
        return resolve_module(rest, &[base.to_path_buf()]);
    }
    let mut search: Vec<PathBuf> = Vec::new();
    if !bases_first {
        search.push(own.to_path_buf());
    }
    search.extend(bases.iter().filter(|b| *b != own).cloned());
    if bases_first {
        search.push(own.to_path_buf());
    }
    resolve_module(name, &search)
}

/// The first part of each module a module imports absolutely, anywhere in
/// it (`import a.b`, `from a.b import c`): `a`.
pub fn absolute_imports(module: &ModModule) -> BTreeSet<String> {
    struct Names(BTreeSet<String>);
    impl<'a> Visitor<'a> for Names {
        fn visit_stmt(&mut self, stmt: &'a Stmt) {
            match stmt {
                Stmt::Import(import) => {
                    for alias in &import.names {
                        let top = alias.name.id.split('.').next().unwrap_or_default();
                        self.0.insert(top.to_string());
                    }
                }
                Stmt::ImportFrom(import) if import.level == 0 => {
                    if let Some(module) = &import.module {
                        let top = module.id.split('.').next().unwrap_or_default();
                        self.0.insert(top.to_string());
                    }
                }
                _ => {}
            }
            visitor::walk_stmt(self, stmt);
        }
    }
    let mut names = Names(BTreeSet::new());
    for stmt in &module.body {
        names.visit_stmt(stmt);
    }
    names.0
}

/// The file Python imports for the absolute dotted `name` with `dirs` as
/// `sys.path`, each part looked up as `FileFinder` does.
pub fn find_module(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    resolve_module(name, dirs)
}

/// Whether some leading part of the absolute dotted `path` is a module
/// under one of `dirs` (looked up afresh, not cached).
pub fn found_in(path: &str, dirs: &[PathBuf]) -> bool {
    if dirs.is_empty() || path.starts_with('.') {
        return false;
    }
    let parts: Vec<&str> = path.split('.').collect();
    (1..=parts.len()).any(|split| resolve_module(&parts[..split].join("."), dirs).is_some())
}

/// The directory pytest's `prepend` import mode puts on `sys.path` for a
/// module in `dir`: the first ancestor that is not a package.
pub fn package_base(dir: &Path) -> PathBuf {
    let mut base = dir;
    while base.join("__init__.py").is_file() {
        match base.parent() {
            Some(parent) => base = parent,
            None => break,
        }
    }
    base.to_path_buf()
}

/// A dotted module name as the file Python imports for it, with `bases`
/// as `sys.path`: each part is looked up where the previous one says
/// (`sys.path` for the first, then the package's directory, or a
/// namespace package's directories), and in each directory as Python's
/// `FileFinder` does: a package (`name/__init__.py`) before a module
/// (`name.py`), either before a namespace portion (a directory without
/// `__init__.py`). A namespace package has no file.
fn resolve_module(name: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.starts_with('.') {
        return None;
    }
    let parts: Vec<&str> = name.split('.').collect();
    let mut locations: Vec<PathBuf> = bases.to_vec();
    for (at, part) in parts.iter().enumerate() {
        let last = at + 1 == parts.len();
        let mut portions: Vec<PathBuf> = Vec::new();
        let mut found = None;
        for location in &locations {
            let dir = location.join(part);
            // One look at the directory spares two for a missing name.
            let is_dir = dir.is_dir();
            if is_dir {
                let init = dir.join("__init__.py");
                if init.is_file() {
                    found = Some((dir, init));
                    break;
                }
            }
            let module = location.join(format!("{part}.py"));
            if module.is_file() {
                // A module holds no submodules.
                return last.then_some(module);
            }
            if is_dir {
                portions.push(dir);
            }
        }
        match found {
            Some((_, init)) if last => return Some(init),
            Some((dir, _)) => locations = vec![dir],
            None if last || portions.is_empty() => return None,
            None => locations = portions,
        }
    }
    None
}
