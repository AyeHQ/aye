//! Summaries of the support modules, shared across files and threads
//! ([`Supports`]), and the questions asked across them: which plugins a
//! module loads, what a module's namespace holds once its imports are
//! followed into project files, and whether a name may be a unittest case,
//! a test or a class factory.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **Imports.** A module is looked up as Python's `FileFinder` does: each
//!   part where the previous one says, a package (`name/__init__.py`)
//!   before a module (`name.py`), either before a namespace portion. The
//!   importing directory's package base comes first in the `prepend`
//!   import mode, after the other bases with `append` (where pytest
//!   appends it) and `importlib` (which adds none).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aye_adapter::SourceFile;
use aye_events::Fnv64;
use aye_python_semantic::class::is_unittest_base;
use aye_python_semantic::module_resolver::{package_base, resolve_from};

use super::fixtures::FixtureDef;
use super::summary::{BaseRef, Summary};

/// What [`Supports::project_name`] asks of the project file and name a
/// path leads to (with the files and names being visited).
type NameCheck<'c> = dyn Fn(&Supports, &Summary, &str, &mut Vec<(PathBuf, String)>) -> bool + 'c;

/// `module.attribute`, for a `from module import attribute` (the module
/// with its leading dots).
fn joined(module: &str, attribute: &str) -> String {
    if module.ends_with('.') {
        format!("{module}{attribute}")
    } else {
        format!("{module}.{attribute}")
    }
}

/// What a module's namespace holds once it has run, as far as collection
/// is concerned: the fixtures and hooks it defines, and those it imports
/// from project files.
#[derive(Debug, Clone, Default)]
pub(crate) struct Exports {
    pub fixtures: Vec<FixtureDef>,
    pub hooks: BTreeSet<String>,
    /// Names imported from modules that are not project files: each may be
    /// a fixture.
    pub maybe_fixtures: BTreeSet<String>,
    /// A star import from a module that is not a project file: any name
    /// may be a fixture.
    pub any_fixture: bool,
    pub collect_file_touches_py: bool,
    pub skips_at_import: bool,
    /// Its `pytest_collection_modifyitems`, or a module's it star-imports,
    /// may add tests ([`Summary::makes_items`]).
    pub makes_items: bool,
}

/// Summaries of support modules, shared across files.
#[derive(Debug, Default)]
pub(crate) struct Supports {
    /// By path and a hash of the source.
    parsed: Mutex<HashMap<(PathBuf, u64), Arc<Summary>>>,
    /// `conftest.py` in a directory, read from disk.
    dirs: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// Plugin modules, read from disk.
    plugins: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// [`Supports::expand`] results, by summary.
    expanded: Mutex<HashMap<usize, Vec<Arc<Summary>>>>,
    /// [`Supports::exports`] results, by summary.
    exported: Mutex<HashMap<usize, Arc<Exports>>>,
    /// Module files by name and importing directory.
    resolved: Mutex<HashMap<(String, PathBuf), Option<PathBuf>>>,
    /// The import mode puts a module's own directory after `sys.path`
    /// (`append`), or not at all (`importlib`): see [`resolve_from`].
    pub bases_first: bool,
    /// [`package_base`] by directory.
    package_bases: Mutex<HashMap<PathBuf, PathBuf>>,
}

fn parse_file(path: &Path) -> Option<Summary> {
    let source = std::fs::read_to_string(path).ok()?;
    let parsed = ruff_python_parser::parse_module(&source).ok()?;
    let dir = path.parent()?;
    Some(Summary::build(dir, &source, parsed.syntax()))
}

impl Supports {
    /// The summary of a support file handed to the recognizer.
    pub(crate) fn of(&self, file: &SourceFile<'_>) -> Arc<Summary> {
        let key = (file.abs.to_path_buf(), Fnv64::hash(file.source.as_bytes()));
        if let Some(found) = self.parsed.lock().expect("lock").get(&key) {
            return Arc::clone(found);
        }
        let dir = file.abs.parent().unwrap_or(file.abs);
        let summary = Arc::new(Summary::build(dir, file.source, file.module));
        // Another thread may have summarized it meanwhile: the summary the
        // map keeps is the one returned, so that it lives as long as `self`
        // and its address keys `expanded` and `exported` (a summary
        // dropped here could have its address reused by another file's).
        Arc::clone(
            self.parsed
                .lock()
                .expect("lock")
                .entry(key)
                .or_insert(summary),
        )
    }

    /// The summary of `dir/conftest.py`, read from disk, if there is one.
    pub(crate) fn conftest_in(&self, dir: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.dirs.lock().expect("lock").get(dir) {
            return found.clone();
        }
        let path = dir.join("conftest.py");
        let summary = path.is_file().then(|| {
            Arc::new(parse_file(&path).unwrap_or_else(|| Summary {
                dir: dir.to_path_buf(),
                ..Summary::default()
            }))
        });
        // The map's summary, whichever thread put it there (see `of`).
        self.dirs
            .lock()
            .expect("lock")
            .entry(dir.to_path_buf())
            .or_insert(summary)
            .clone()
    }

    fn plugin_at(&self, path: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.plugins.lock().expect("lock").get(path) {
            return found.clone();
        }
        let summary = parse_file(path).map(Arc::new);
        // The map's summary, whichever thread put it there (see `of`).
        self.plugins
            .lock()
            .expect("lock")
            .entry(path.to_path_buf())
            .or_insert(summary)
            .clone()
    }

    /// `summary` and the plugin modules it loads through `pytest_plugins`,
    /// transitively. Plugins that are not files of the project (installed
    /// packages) are not seen, like any installed plugin.
    pub(crate) fn expand(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Vec<Arc<Summary>> {
        if summary.plugins.is_empty() && summary.registered.is_empty() {
            return vec![Arc::clone(summary)];
        }
        // Summaries live as long as `self` caches them, so their address
        // identifies them.
        let key = Arc::as_ptr(summary) as usize;
        if let Some(found) = self.expanded.lock().expect("lock").get(&key) {
            return found.clone();
        }
        let out = self.expand_uncached(summary, bases);
        self.expanded.lock().expect("lock").insert(key, out.clone());
        out
    }

    fn expand_uncached(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Vec<Arc<Summary>> {
        let mut out = vec![Arc::clone(summary)];
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack: Vec<(Arc<Summary>, usize)> = vec![(Arc::clone(summary), 0)];
        while let Some((current, depth)) = stack.pop() {
            if depth > 8 {
                continue;
            }
            for plugin in current.plugins.iter().chain(&current.registered) {
                let Some(path) = self.resolve(plugin, &current.dir, bases) else {
                    continue;
                };
                if !seen.insert(path.clone()) {
                    continue;
                }
                if let Some(found) = self.plugin_at(&path) {
                    out.push(Arc::clone(&found));
                    stack.push((found, depth + 1));
                }
            }
        }
        out
    }
}

/// What an attribute of a module is, as far as fixtures go.
#[derive(Debug, Clone)]
enum Attribute {
    Fixture(FixtureDef),
    /// It comes from a module that is not a project file.
    Maybe,
    No,
}

impl Supports {
    /// The summary of a project module, by its dotted name as code in
    /// `dir` imports it (relative names allowed).
    pub(crate) fn module(&self, name: &str, dir: &Path, bases: &[PathBuf]) -> Option<Arc<Summary>> {
        let path = self.resolve(name, dir, bases)?;
        self.plugin_at(&path)
    }

    /// [`package_base`], cached.
    pub(crate) fn package_base(&self, dir: &Path) -> PathBuf {
        if let Some(found) = self.package_bases.lock().expect("lock").get(dir) {
            return found.clone();
        }
        let found = package_base(dir);
        self.package_bases
            .lock()
            .expect("lock")
            .insert(dir.to_path_buf(), found.clone());
        found
    }

    /// Forget the modules looked up so far, and what was read through
    /// them: the places they are looked up in have grown.
    pub(crate) fn forget_resolutions(&self) {
        self.resolved.lock().expect("lock").clear();
        self.expanded.lock().expect("lock").clear();
        self.exported.lock().expect("lock").clear();
    }

    /// [`resolve_from`], cached: files are looked for once per name and
    /// directory.
    fn resolve(&self, name: &str, dir: &Path, bases: &[PathBuf]) -> Option<PathBuf> {
        let key = (name.to_string(), dir.to_path_buf());
        if let Some(found) = self.resolved.lock().expect("lock").get(&key) {
            return found.clone();
        }
        let own = self.package_base(dir);
        let found = resolve_from(name, dir, &own, bases, self.bases_first);
        self.resolved
            .lock()
            .expect("lock")
            .insert(key, found.clone());
        found
    }

    /// What `summary`'s namespace holds, following its imports into project
    /// files. `summary` must be one these caches hold (a conftest's, or a
    /// plugin's): results are kept by its address.
    pub(crate) fn exports(&self, summary: &Arc<Summary>, bases: &[PathBuf]) -> Arc<Exports> {
        let key = Arc::as_ptr(summary) as usize;
        if let Some(found) = self.exported.lock().expect("lock").get(&key) {
            return Arc::clone(found);
        }
        let out = Arc::new(self.exports_of(summary, bases, |_| true));
        self.exported
            .lock()
            .expect("lock")
            .insert(key, Arc::clone(&out));
        out
    }

    /// [`Supports::exports`] for a summary of any lifetime, uncached, with
    /// only the names `wanted` keeps followed through `from … import`.
    pub(crate) fn exports_of(
        &self,
        summary: &Summary,
        bases: &[PathBuf],
        wanted: impl Fn(&str) -> bool,
    ) -> Exports {
        let mut visiting = Vec::new();
        self.exports_inner(summary, bases, &wanted, &mut visiting)
    }

    fn exports_inner(
        &self,
        summary: &Summary,
        bases: &[PathBuf],
        wanted: &dyn Fn(&str) -> bool,
        visiting: &mut Vec<PathBuf>,
    ) -> Exports {
        let mut out = Exports {
            fixtures: summary.fixtures.clone(),
            hooks: summary.hooks.clone(),
            collect_file_touches_py: summary.collect_file_touches_py,
            skips_at_import: summary.skips_at_import,
            makes_items: summary.makes_items,
            ..Exports::default()
        };
        for (local, module, attribute) in &summary.from_imports {
            if !wanted(local) || super::no_tests(module) {
                continue;
            }
            match self.attribute(module, attribute, &summary.dir, bases, visiting) {
                Attribute::Fixture(mut fixture) => {
                    if !fixture.explicit_name {
                        fixture.name.clone_from(local);
                    }
                    fixture.attribute.clone_from(local);
                    out.fixtures.push(fixture);
                }
                Attribute::Maybe => {
                    out.maybe_fixtures.insert(local.clone());
                }
                Attribute::No => {}
            }
        }
        for module in &summary.star_imports {
            if super::no_tests(module) {
                continue;
            }
            let Some(path) = self.resolve(module, &summary.dir, bases) else {
                out.any_fixture = true;
                continue;
            };
            let Some(source) = self.plugin_at(&path) else {
                out.any_fixture = true;
                continue;
            };
            if visiting.contains(&path) || visiting.len() > 8 {
                continue;
            }
            visiting.push(path);
            let theirs = self.exports_inner(&source, bases, &|_| true, visiting);
            visiting.pop();
            let public = |name: &str| match &source.all {
                Some(all) => all.iter().any(|n| n == name),
                None => !name.starts_with('_'),
            };
            out.fixtures.extend(
                theirs
                    .fixtures
                    .iter()
                    .filter(|f| public(&f.attribute))
                    .cloned(),
            );
            out.hooks
                .extend(theirs.hooks.iter().filter(|h| public(h)).cloned());
            out.maybe_fixtures
                .extend(theirs.maybe_fixtures.iter().filter(|n| public(n)).cloned());
            out.any_fixture |= theirs.any_fixture;
            out.collect_file_touches_py |= theirs.collect_file_touches_py;
            out.skips_at_import |= theirs.skips_at_import;
            out.makes_items |= theirs.makes_items;
        }
        out
    }

    /// Whether the class `path` names, as code in `dir` refers to it, may be
    /// a unittest case: one of pytest's or unittest's own, or a project class
    /// deriving from one or from a class aye can't follow. Classes from
    /// packages that ship no test cases are not.
    pub(crate) fn may_be_unittest(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.may_be_unittest_inner(path, dir, bases, &mut visiting)
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names
    /// something a project file defines that may be a unittest case: a
    /// class whose bases may make it one, or a name aye can't tell is no
    /// class. Modules are not, nor what installed packages define unless
    /// it is a known unittest base or named like one (`…TestCase`).
    pub(crate) fn project_unittest(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.defined_unittest(path, dir, bases, &mut visiting, true)
    }

    /// Whether some leading part of the dotted `path` (as code in `dir`
    /// imports it), its last aside (the name imported from it), is a
    /// project module.
    pub(crate) fn resolvable(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        (1..parts.len()).any(|split| {
            let module = format!("{dots}{}", parts[..split].join("."));
            self.resolve(&module, dir, bases).is_some()
        })
    }

    /// [`Supports::project_unittest`]; `first` while `path` is the name a
    /// test module imports (it must lead to a project file).
    fn defined_unittest(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        first: bool,
    ) -> bool {
        // What a test module imports must come from a project file.
        if !first && is_unittest_base(path) {
            return true;
        }
        if super::no_tests(path) {
            return false;
        }
        if visiting.len() > 16 {
            return true;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [class] = &parts[split..] else {
                return true;
            };
            let Some(source) = self.plugin_at(&file) else {
                return true;
            };
            let bound = source.plain.contains(*class)
                || source.classes.contains_key(*class)
                || source.from_imports.iter().any(|(local, ..)| local == class);
            if !bound {
                // Not bound by the module: a submodule (`from pkg import
                // sub`), or bound in ways aye can't read.
                return self.resolve(path, dir, bases).is_none();
            }
            let key = (file, (*class).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = self.defined_in(&source, class, bases, visiting, 0);
            visiting.pop();
            return found;
        }
        // An installed package: only what looks like a unittest case.
        !first && parts.last().is_some_and(|name| name.ends_with("TestCase"))
    }

    /// [`Supports::project_unittest`] for a name `source` binds.
    fn defined_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return true;
        }
        if source.plain.contains(name) {
            return false;
        }
        if let Some(refs) = source.classes.get(name) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(local) => self.defined_in(source, local, bases, visiting, depth + 1),
                BaseRef::Path(path) => {
                    self.defined_unittest(path, &source.dir, bases, visiting, false)
                }
                BaseRef::Unknown => true,
            });
        }
        if let Some((_, module, attribute)) =
            source.from_imports.iter().find(|(local, ..)| local == name)
        {
            let path = if module.ends_with('.') {
                format!("{module}{attribute}")
            } else {
                format!("{module}.{attribute}")
            };
            return self.defined_unittest(&path, &source.dir, bases, visiting, false);
        }
        true
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names a
    /// class or function of a project file whose `__test__` may be true,
    /// directly or through the project classes it inherits from. Names
    /// aye can't read, and installed packages, don't count.
    pub(crate) fn project_test(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.project_name(
            path,
            dir,
            bases,
            &mut visiting,
            &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
        )
    }

    /// Whether `path` (a dotted path as code in `dir` imports it) names a
    /// function of a project file that may make classes.
    pub(crate) fn project_factory(&self, path: &str, dir: &Path, bases: &[PathBuf]) -> bool {
        let mut visiting = Vec::new();
        self.project_name(
            path,
            dir,
            bases,
            &mut visiting,
            &|this, source, name, visiting| this.factory_in(source, name, bases, visiting),
        )
    }

    /// The project file and name `path` leads to, put to `check`; `false`
    /// when it names no project file's attribute.
    fn project_name(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        check: &NameCheck<'_>,
    ) -> bool {
        if super::no_tests(path) || visiting.len() > 16 {
            return false;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [name] = &parts[split..] else {
                return false;
            };
            let Some(source) = self.plugin_at(&file) else {
                return false;
            };
            let key = (file, (*name).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = check(self, &source, name, visiting);
            visiting.pop();
            return found;
        }
        false
    }

    /// [`Supports::project_test`] for a name `source` binds.
    fn test_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return false;
        }
        if source.tests.contains(name) {
            return true;
        }
        if let Some(refs) = source.classes.get(name) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(local) => self.test_in(source, local, bases, visiting, depth + 1),
                BaseRef::Path(path) => self.project_name(
                    path,
                    &source.dir,
                    bases,
                    visiting,
                    &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
                ),
                BaseRef::Unknown => false,
            });
        }
        match source.from_imports.iter().find(|(local, ..)| local == name) {
            Some((_, module, attribute)) => self.project_name(
                &joined(module, attribute),
                &source.dir,
                bases,
                visiting,
                &|this, source, name, visiting| this.test_in(source, name, bases, visiting, 0),
            ),
            None => false,
        }
    }

    /// [`Supports::project_factory`] for a name `source` binds.
    fn factory_in(
        &self,
        source: &Summary,
        name: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
    ) -> bool {
        if source.factories.contains(name) {
            return true;
        }
        match source.from_imports.iter().find(|(local, ..)| local == name) {
            Some((_, module, attribute)) => self.project_name(
                &joined(module, attribute),
                &source.dir,
                bases,
                visiting,
                &|this, source, name, visiting| this.factory_in(source, name, bases, visiting),
            ),
            None => false,
        }
    }

    fn may_be_unittest_inner(
        &self,
        path: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
    ) -> bool {
        if is_unittest_base(path) {
            return true;
        }
        if super::no_tests(path) {
            return false;
        }
        if visiting.len() > 16 {
            return true;
        }
        let level = path.chars().take_while(|c| *c == '.').count();
        let parts: Vec<&str> = path[level..].split('.').collect();
        let dots = &path[..level];
        // The longest prefix that is a project module holds the class.
        for split in (1..parts.len()).rev() {
            let module = format!("{dots}{}", parts[..split].join("."));
            let Some(file) = self.resolve(&module, dir, bases) else {
                continue;
            };
            let [class] = &parts[split..] else {
                return true;
            };
            let Some(source) = self.plugin_at(&file) else {
                return true;
            };
            let key = (file, (*class).to_string());
            if visiting.contains(&key) {
                return false;
            }
            visiting.push(key);
            let found = self.class_may_be_unittest(&source, class, bases, visiting, 0);
            visiting.pop();
            return found;
        }
        // Not a project file: an installed package aye can't read.
        true
    }

    /// [`Supports::may_be_unittest`] for a class of `source`.
    fn class_may_be_unittest(
        &self,
        source: &Summary,
        class: &str,
        bases: &[PathBuf],
        visiting: &mut Vec<(PathBuf, String)>,
        depth: usize,
    ) -> bool {
        if depth > 16 {
            return true;
        }
        if source.plain.contains(class) {
            return false;
        }
        if let Some(refs) = source.classes.get(class) {
            return refs.iter().any(|base| match base {
                BaseRef::Local(name) => {
                    self.class_may_be_unittest(source, name, bases, visiting, depth + 1)
                }
                BaseRef::Path(path) => {
                    self.may_be_unittest_inner(path, &source.dir, bases, visiting)
                }
                BaseRef::Unknown => true,
            });
        }
        // Imported into the module, under that name.
        if let Some((_, module, attribute)) = source
            .from_imports
            .iter()
            .find(|(local, ..)| local == class)
        {
            let path = if module.ends_with('.') {
                format!("{module}{attribute}")
            } else {
                format!("{module}.{attribute}")
            };
            return self.may_be_unittest_inner(&path, &source.dir, bases, visiting);
        }
        // Bound some other way, or through a star import.
        true
    }

    /// What `module.attribute` is, as code in `dir` imports it, following
    /// re-exports through the project's files only as far as needed.
    fn attribute(
        &self,
        module: &str,
        attribute: &str,
        dir: &Path,
        bases: &[PathBuf],
        visiting: &mut Vec<PathBuf>,
    ) -> Attribute {
        let Some(path) = self.resolve(module, dir, bases) else {
            return Attribute::Maybe;
        };
        let Some(source) = self.plugin_at(&path) else {
            return Attribute::Maybe;
        };
        if visiting.contains(&path) || visiting.len() > 8 {
            return Attribute::No;
        }
        if let Some(fixture) = source.fixtures.iter().find(|f| f.attribute == attribute) {
            return Attribute::Fixture(fixture.clone());
        }
        visiting.push(path);
        let found = if let Some((_, from, name)) = source
            .from_imports
            .iter()
            .find(|(local, ..)| local == attribute)
        {
            if super::no_tests(from) {
                Attribute::No
            } else {
                self.attribute(from, name, &source.dir, bases, visiting)
            }
        } else {
            let mut found = Attribute::No;
            let public = match &source.all {
                Some(all) => all.iter().any(|n| n == attribute),
                None => !attribute.starts_with('_'),
            };
            if public {
                for star in source.star_imports.iter().rev() {
                    if super::no_tests(star) {
                        continue;
                    }
                    match self.attribute(star, attribute, &source.dir, bases, visiting) {
                        Attribute::No => {}
                        other => {
                            found = other;
                            break;
                        }
                    }
                }
            }
            found
        };
        visiting.pop();
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_resolve_to_project_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("tests/plugins")).unwrap();
        std::fs::write(
            root.join("conftest.py"),
            "pytest_plugins = ['tests.plugins.extra', 'pytest_cov']\n",
        )
        .unwrap();
        std::fs::write(root.join("tests/__init__.py"), "").unwrap();
        std::fs::write(root.join("tests/plugins/__init__.py"), "").unwrap();
        std::fs::write(
            root.join("tests/plugins/extra.py"),
            "def pytest_generate_tests(metafunc): pass\n",
        )
        .unwrap();
        let supports = Supports::default();
        let conftest = supports.conftest_in(root).unwrap();
        let expanded = supports.expand(&conftest, &[root.to_path_buf()]);
        assert_eq!(expanded.len(), 2);
        assert!(expanded[1].hooks.contains("pytest_generate_tests"));
        assert!(supports.conftest_in(&root.join("tests")).is_none());
    }

    /// Summaries are told apart by their address in `expanded` and
    /// `exported`, so every caller must get the one the cache keeps: one
    /// that a thread built and dropped, having lost the race to cache it,
    /// could leave its address to another file's summary, which would then
    /// read the first file's cached exports.
    #[test]
    fn threads_summarizing_one_file_get_the_cached_summary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("conftest.py"), "import pytest\n").unwrap();
        let abs = root.join("conftest.py");
        let source = "import pytest\npytest.importorskip('missing')\n";
        let parsed = ruff_python_parser::parse_module(source).unwrap();
        let lines = aye_adapter::LineIndex::from_source_text(source);
        let file = SourceFile {
            rel: aye_events::ProjectPathRef::as_written("conftest.py"),
            abs: &abs,
            source,
            module: parsed.syntax(),
            lines: &lines,
        };
        for _ in 0..50 {
            let supports = Supports::default();
            let barrier = std::sync::Barrier::new(8);
            let found: Vec<[Arc<Summary>; 3]> = std::thread::scope(|s| {
                let handles: Vec<_> = (0..8)
                    .map(|_| {
                        s.spawn(|| {
                            barrier.wait();
                            [
                                supports.of(&file),
                                supports.conftest_in(root).unwrap(),
                                supports.plugin_at(&abs).unwrap(),
                            ]
                        })
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            for kind in 0..3 {
                assert!(found.iter().all(|f| Arc::ptr_eq(&f[kind], &found[0][kind])));
            }
            assert!(Arc::ptr_eq(&found[0][0], &supports.of(&file)));
            assert!(found[0][0].skips_at_import);
        }
    }
}
