//! The project's own modules, read on demand ([`Modules`]): what an import
//! leads to, and what importing a module may change elsewhere.
//!
//! [`Modules`] follows imports into the project's own files, so that a class
//! imported from a helper module, or a base defined there, is read too;
//! classes from installed packages are not read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aye_adapter::LineIndex;

use crate::loader::Loaded;
use crate::module::{Binding, ClassIndex, Summary, Touch, is_stdlib, summarize};

/// Where an imported dotted path leads.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    Class(Arc<Summary>, ClassIndex),
    /// Another binding of a project module.
    Binding(Arc<Summary>, Binding),
    Module,
    /// The standard library.
    Stdlib(String),
    /// An installed package aye doesn't read.
    External(String),
    Unknown(String),
}

/// The project's own modules, read on demand: what imports lead to.
#[derive(Debug)]
pub(crate) struct Modules {
    root: PathBuf,
    /// Where absolute imports are looked up: the top-level directory, the
    /// project root and its `src`.
    search: Vec<PathBuf>,
    cache: Mutex<HashMap<PathBuf, Option<Arc<Summary>>>>,
    /// By module file: what importing it may change elsewhere, resolved.
    effects: Mutex<HashMap<PathBuf, Arc<Vec<Effect>>>>,
    /// By module file: the project files importing it imports (its
    /// imports' modules and their packages' `__init__.py`).
    edges: Mutex<HashMap<PathBuf, Arc<Vec<PathBuf>>>>,
}

/// A [`Touch`], resolved where it was made.
#[derive(Debug, Clone)]
enum Effect {
    /// A project class may change.
    Class(PathBuf, ClassIndex, String),
    /// Something aye can't resolve may change: named so (its last name),
    /// or not at all.
    Unknown(Option<String>, String),
}

/// Modules followed from a test module before refinement takes over.
const MAX_MODULES: usize = 4000;

/// What [`Modules::module_file`] finds.
enum Found {
    File(PathBuf),
    Namespace,
}

/// Imports followed this deep are left to refinement.
pub(crate) const MAX_DEPTH: usize = 16;

impl Modules {
    pub(crate) fn new(top: &Path, root: &Path) -> Self {
        let mut search = Vec::new();
        for dir in [top.to_path_buf(), root.to_path_buf(), root.join("src")] {
            if !search.contains(&dir) {
                search.push(dir);
            }
        }
        Self {
            root: root.to_path_buf(),
            search,
            cache: Mutex::new(HashMap::new()),
            effects: Mutex::new(HashMap::new()),
            edges: Mutex::new(HashMap::new()),
        }
    }

    /// A project file, read and summarized once.
    pub(crate) fn summary(&self, path: &Path) -> Option<Arc<Summary>> {
        if let Some(found) = self.cache.lock().expect("lock").get(path) {
            return found.clone();
        }
        let summary = std::fs::read_to_string(path).ok().and_then(|source| {
            let parsed = ruff_python_parser::parse_module(&source).ok()?;
            let lines = LineIndex::from_source_text(&source);
            Some(Arc::new(summarize(path, parsed.syntax(), &lines)))
        });
        self.cache
            .lock()
            .expect("lock")
            .insert(path.to_path_buf(), summary.clone());
        summary
    }

    /// The file of module `parts` under `base`: `a/b.py` or
    /// `a/b/__init__.py`, or a directory without one (a namespace package).
    fn module_file(base: &Path, parts: &[&str]) -> Option<Found> {
        let mut dir = base.to_path_buf();
        for part in parts {
            dir.push(part);
        }
        if parts.is_empty() {
            let init = dir.join("__init__.py");
            return init.is_file().then_some(Found::File(init));
        }
        let file = dir.with_extension("py");
        if file.is_file() {
            return Some(Found::File(file));
        }
        let init = dir.join("__init__.py");
        if init.is_file() {
            return Some(Found::File(init));
        }
        dir.is_dir().then_some(Found::Namespace)
    }

    /// Where `path` (as imported in `from`) leads.
    pub(crate) fn target(&self, path: &str, from: &Path, depth: usize) -> Target {
        if depth > MAX_DEPTH {
            return Target::Unknown(format!("{path} is imported through too many modules"));
        }
        let dots = path.chars().take_while(|c| *c == '.').count();
        let rest = &path[dots..];
        let parts: Vec<&str> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('.').collect()
        };
        let bases: Vec<PathBuf> = if dots == 0 {
            if is_stdlib(path) {
                return Target::Stdlib(path.to_string());
            }
            self.search.clone()
        } else {
            let mut base = from.parent().unwrap_or(from).to_path_buf();
            for _ in 1..dots {
                base = base.parent().unwrap_or(&base).to_path_buf();
            }
            vec![base]
        };
        let lowest = usize::from(dots == 0);
        for split in (lowest..=parts.len()).rev() {
            for base in &bases {
                let found = Self::module_file(base, &parts[..split]);
                if found.is_some() && split == parts.len() {
                    return Target::Module;
                }
                // A namespace package holds only submodules, and the longer
                // paths found none: a directory of the same name, not this
                // import.
                let Some(Found::File(file)) = found else {
                    continue;
                };
                let Some(summary) = self.summary(&file) else {
                    return Target::Unknown(format!("{} can't be read", file.display()));
                };
                return self.member(&summary, &parts[split..], depth);
            }
        }
        if dots > 0 {
            return Target::Unknown(format!("{path} is not a module aye can find"));
        }
        // Found beside the importing module (or above it), where only a
        // `sys.path` change makes Python look: maybe the project's.
        let beside = from
            .ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.root))
            .filter(|dir| !self.search.iter().any(|searched| searched == dir))
            .find(|dir| matches!(Self::module_file(dir, &parts[..1]), Some(Found::File(_))));
        if let Some(dir) = beside {
            return Target::Unknown(format!(
                "{path} may be imported from {}, through a sys.path change",
                dir.display()
            ));
        }
        Target::External(path.to_string())
    }

    /// The project file of the module `path` (as imported in `from`):
    /// `a/b.py` or `a/b/__init__.py`; `None` for a name that is no module,
    /// the standard library and installed packages.
    fn module_path(&self, path: &str, from: &Path) -> Option<PathBuf> {
        let dots = path.chars().take_while(|c| *c == '.').count();
        let rest = &path[dots..];
        let parts: Vec<&str> = if rest.is_empty() {
            Vec::new()
        } else {
            rest.split('.').collect()
        };
        let bases: Vec<PathBuf> = if dots == 0 {
            if is_stdlib(path) {
                return None;
            }
            self.search.clone()
        } else {
            let mut base = from.parent().unwrap_or(from).to_path_buf();
            for _ in 1..dots {
                base = base.parent().unwrap_or(&base).to_path_buf();
            }
            vec![base]
        };
        bases
            .iter()
            .find_map(|base| match Self::module_file(base, &parts) {
                Some(Found::File(file)) => Some(file),
                _ => None,
            })
    }

    /// The `__init__.py` of each package holding `file`, nearest first,
    /// within the project: importing the file runs them.
    fn packages_of(&self, file: &Path) -> Vec<PathBuf> {
        file.ancestors()
            .skip(1)
            .take_while(|dir| dir.starts_with(&self.root))
            .map(|dir| dir.join("__init__.py"))
            .take_while(|init| init.is_file())
            .filter(|init| init != file)
            .collect()
    }

    /// See [`Modules::edges`].
    fn edges_of(&self, summary: &Summary) -> Arc<Vec<PathBuf>> {
        if let Some(found) = self.edges.lock().expect("lock").get(&summary.path) {
            return Arc::clone(found);
        }
        let mut out: Vec<PathBuf> = Vec::new();
        for import in &summary.imports {
            if let Some(file) = self.module_path(import, &summary.path) {
                out.extend(self.packages_of(&file));
                out.push(file);
            }
        }
        out.dedup();
        let out = Arc::new(out);
        self.edges
            .lock()
            .expect("lock")
            .insert(summary.path.clone(), Arc::clone(&out));
        out
    }

    /// See [`Modules::effects`].
    fn effects_of(&self, summary: &Arc<Summary>) -> Arc<Vec<Effect>> {
        if let Some(found) = self.effects.lock().expect("lock").get(&summary.path) {
            return Arc::clone(found);
        }
        let mut out = Vec::new();
        let shown = |what: &str, path: &str| {
            format!(
                "{} {what} {path} while it is imported",
                summary.path.display()
            )
        };
        for touch in &summary.touches {
            let (path, how) = match touch {
                Touch::Object(path) => (path, "may change"),
                Touch::Bound(path) => (path, "rebinds"),
                Touch::Call(path) => (path, "calls"),
                Touch::Unknown(reason) => {
                    out.push(Effect::Unknown(None, reason.clone()));
                    continue;
                }
            };
            let reason = shown(how, path);
            // unittest itself: its loader decides what a test is
            // (`TestLoader.testMethodPrefix = …`); `TestCase.maxDiff` and
            // the like change none.
            if path == "unittest" || path.starts_with("unittest.") {
                let loader = path
                    .split('.')
                    .any(|part| part.ends_with("Loader") || part == "loader");
                if loader && !matches!(touch, Touch::Call(_)) {
                    out.push(Effect::Unknown(None, reason));
                }
                continue;
            }
            let last = path.rsplit('.').next().unwrap_or(path).to_string();
            match (touch, self.target(path, &summary.path, 0)) {
                (Touch::Call(_), Target::Binding(_, Binding::Function(info))) => {
                    if info.foreign {
                        out.push(Effect::Unknown(None, reason));
                    }
                }
                (Touch::Call(_), _) => {}
                (_, Target::Class(owner, index)) => {
                    out.push(Effect::Class(owner.path.clone(), index, reason));
                }
                (_, Target::Unknown(why)) => {
                    out.push(Effect::Unknown(Some(last), format!("{reason}: {why}")));
                }
                _ => {}
            }
        }
        let out = Arc::new(out);
        self.effects
            .lock()
            .expect("lock")
            .insert(summary.path.clone(), Arc::clone(&out));
        out
    }

    /// Why importing the test module `summary` may give the classes it
    /// loads (`loaded`) other tests than their source says: a module that
    /// runs before the loader reads them (the packages holding it, which
    /// discovery imports first; it; the project modules it imports, and
    /// theirs) changes one of those classes or its bases (`base.Base.test_x
    /// = f`, `setattr`, a classmethod called on it, the class passed to a
    /// call), or something aye can't follow. `None` when nothing may.
    pub(crate) fn effects(&self, summary: &Arc<Summary>, loaded: &[Loaded]) -> Option<String> {
        if loaded.is_empty() {
            return None;
        }
        let classes: HashSet<(&Path, ClassIndex)> = loaded
            .iter()
            .flat_map(|l| l.mro.iter().map(|(path, index)| (path.as_path(), *index)))
            .collect();
        // The names those classes go by, for what aye can't resolve.
        let mut names: HashSet<String> = loaded.iter().map(|l| l.name.clone()).collect();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills a set: the order shows nowhere"
        )]
        for (path, index) in &classes {
            if let Some(owner) = self.summary(path)
                && let Some(class) = owner.classes.get(*index)
            {
                names.insert(class.name.clone());
            }
        }
        let mut queue: Vec<PathBuf> = self.packages_of(&summary.path);
        queue.push(summary.path.clone());
        let mut seen: HashSet<PathBuf> = HashSet::new();
        while let Some(file) = queue.pop() {
            if !seen.insert(file.clone()) {
                continue;
            }
            if seen.len() > MAX_MODULES {
                return Some(format!(
                    "{} imports more than {MAX_MODULES} project modules, which may change its \
                     classes",
                    summary.path.display()
                ));
            }
            let Some(module) = (if file == summary.path {
                Some(Arc::clone(summary))
            } else {
                self.summary(&file)
            }) else {
                continue;
            };
            for effect in self.effects_of(&module).iter() {
                match effect {
                    Effect::Class(path, index, reason)
                        if classes.contains(&(path.as_path(), *index)) =>
                    {
                        return Some(reason.clone());
                    }
                    Effect::Unknown(name, reason)
                        if name.as_ref().is_none_or(|name| names.contains(name)) =>
                    {
                        return Some(reason.clone());
                    }
                    _ => {}
                }
            }
            queue.extend(self.edges_of(&module).iter().cloned());
        }
        None
    }

    fn member(&self, summary: &Arc<Summary>, names: &[&str], depth: usize) -> Target {
        let shown = || format!("{} in {}", names.join("."), summary.path.display());
        if let Some(reason) = &summary.dynamic {
            return Target::Unknown(format!("{}: {reason}", shown()));
        }
        let Some(binding) = summary.names.get(names[0]) else {
            return Target::Unknown(format!("{} is not bound statically", shown()));
        };
        let rest = &names[1..];
        let join = |base: &str| {
            if rest.is_empty() {
                base.to_string()
            } else {
                format!("{base}.{}", rest.join("."))
            }
        };
        match binding {
            Binding::Import(path) | Binding::Module(path) => {
                self.target(&join(path), &summary.path, depth + 1)
            }
            Binding::Class(index) if rest.is_empty() => Target::Class(Arc::clone(summary), *index),
            other if rest.is_empty() => Target::Binding(Arc::clone(summary), other.clone()),
            _ => Target::Unknown(format!("{} is an attribute aye can't follow", shown())),
        }
    }
}
