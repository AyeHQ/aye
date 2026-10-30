//! The project's own modules, read on demand ([`ModuleIndex`]): what an
//! import leads to, and what importing a module may change elsewhere.
//!
//! [`ModuleIndex`] follows imports into the project's own files, so that a
//! class imported from a helper module, or a base defined there, is read too;
//! classes from installed packages are not read. It is an arena: each file it
//! meets gets a [`ModuleId`], and is read, and its imports and what importing
//! it changes are resolved, once, however many recognitions ask at once. A
//! project class is a [`ClassId`]: its module's id and its place among the
//! module's classes, whichever read of the file found it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use aye_adapter::LineIndex;
use aye_index::{IndexVec, newtype_index};

use crate::loader::Loaded;
use crate::module::{Binding, ClassIndex, Summary, Touch, is_stdlib, summarize};

newtype_index! {
    /// A project file's place in the [`ModuleIndex`].
    pub(crate) struct ModuleId;
}

/// A project class: its module and its place among the module's classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClassId {
    pub(crate) module: ModuleId,
    pub(crate) class: ClassIndex,
}

/// A project module, read: its file's id and its summary, the index's read
/// of the file or, for the test module, collection's.
#[derive(Debug, Clone)]
pub(crate) struct Module {
    pub(crate) id: ModuleId,
    pub(crate) summary: Arc<Summary>,
}

impl Module {
    /// Its class at `class`.
    pub(crate) fn class_id(&self, class: ClassIndex) -> ClassId {
        ClassId {
            module: self.id,
            class,
        }
    }
}

/// Where an imported dotted path leads.
#[derive(Debug, Clone)]
pub(crate) enum Target {
    Class(Module, ClassIndex),
    /// Another binding of a project module.
    Binding(Module, Binding),
    Module,
    /// The standard library.
    Stdlib(String),
    /// An installed package aye doesn't read.
    External(String),
    Unknown(String),
}

/// The project's own modules, read on demand: what imports lead to.
#[derive(Debug)]
pub(crate) struct ModuleIndex {
    root: PathBuf,
    /// Where absolute imports are looked up: the top-level directory, the
    /// project root and its `src`.
    search: Vec<PathBuf>,
    /// Every file met.
    files: Mutex<Files>,
}

/// The index's files, by id and by path.
#[derive(Debug, Default)]
struct Files {
    entries: IndexVec<ModuleId, Arc<Entry>>,
    ids: HashMap<PathBuf, ModuleId>,
}

/// A file of the index, and what was read of it, each once.
#[derive(Debug)]
struct Entry {
    path: PathBuf,
    /// Its summary; `None` when it can't be read or doesn't parse.
    summary: OnceLock<Option<Arc<Summary>>>,
    /// What importing it may change elsewhere, resolved.
    effects: OnceLock<Arc<Vec<Effect>>>,
    /// The project files importing it imports (its imports' modules and
    /// their packages' `__init__.py`).
    edges: OnceLock<Arc<Vec<ModuleId>>>,
}

/// A [`Touch`], resolved where it was made.
#[derive(Debug, Clone)]
enum Effect {
    /// A project class may change.
    Class(ClassId, String),
    /// Something aye can't resolve may change: named so (its last name),
    /// or not at all.
    Unknown(Option<String>, String),
}

/// Modules followed from a test module before refinement takes over.
const MAX_MODULES: usize = 4000;

/// What [`ModuleIndex::module_file`] finds.
enum Found {
    File(PathBuf),
    Namespace,
}

/// Imports followed this deep are left to refinement.
pub(crate) const MAX_DEPTH: usize = 16;

impl ModuleIndex {
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
            files: Mutex::new(Files::default()),
        }
    }

    /// The id of `path`, met now if not before.
    pub(crate) fn id(&self, path: &Path) -> ModuleId {
        let mut files = self.files.lock().expect("lock");
        if let Some(id) = files.ids.get(path) {
            return *id;
        }
        let id = files.entries.push(Arc::new(Entry {
            path: path.to_path_buf(),
            summary: OnceLock::new(),
            effects: OnceLock::new(),
            edges: OnceLock::new(),
        }));
        files.ids.insert(path.to_path_buf(), id);
        id
    }

    fn entry(&self, id: ModuleId) -> Arc<Entry> {
        Arc::clone(&self.files.lock().expect("lock").entries[id])
    }

    /// The test module, as collection parsed it: under its file's id, so
    /// that a class of it is the class an import of the file finds.
    pub(crate) fn test_module(&self, summary: Summary) -> Module {
        Module {
            id: self.id(&summary.path),
            summary: Arc::new(summary),
        }
    }

    /// A project file, read and summarized once.
    pub(crate) fn summary(&self, path: &Path) -> Option<Module> {
        self.module(self.id(path))
    }

    /// The file of `id`, read and summarized once: a thread asking while
    /// another reads it waits for that read.
    fn module(&self, id: ModuleId) -> Option<Module> {
        let entry = self.entry(id);
        let summary = entry.summary.get_or_init(|| {
            let path = &entry.path;
            std::fs::read_to_string(path).ok().and_then(|source| {
                let parsed = ruff_python_parser::parse_module(&source).ok()?;
                let lines = LineIndex::from_source_text(&source);
                Some(Arc::new(summarize(path, parsed.syntax(), &lines)))
            })
        });
        Some(Module {
            id,
            summary: Arc::clone(summary.as_ref()?),
        })
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

    /// The project files importing `module` imports (its imports' modules
    /// and their packages' `__init__.py`), resolved once.
    fn edges_of(&self, module: &Module) -> Arc<Vec<ModuleId>> {
        let entry = self.entry(module.id);
        let edges = entry.edges.get_or_init(|| {
            let summary = &module.summary;
            let mut out: Vec<ModuleId> = Vec::new();
            for import in &summary.imports {
                if let Some(file) = self.module_path(import, &summary.path) {
                    out.extend(self.packages_of(&file).iter().map(|init| self.id(init)));
                    out.push(self.id(&file));
                }
            }
            out.dedup();
            Arc::new(out)
        });
        Arc::clone(edges)
    }

    /// What importing `module` may change elsewhere, resolved once.
    fn effects_of(&self, module: &Module) -> Arc<Vec<Effect>> {
        let entry = self.entry(module.id);
        let effects = entry
            .effects
            .get_or_init(|| Arc::new(self.resolve_effects(&module.summary)));
        Arc::clone(effects)
    }

    fn resolve_effects(&self, summary: &Summary) -> Vec<Effect> {
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
                    out.push(Effect::Class(owner.class_id(index), reason));
                }
                (_, Target::Unknown(why)) => {
                    out.push(Effect::Unknown(Some(last), format!("{reason}: {why}")));
                }
                _ => {}
            }
        }
        out
    }

    /// Why importing the test module `test` may give the classes it
    /// loads (`loaded`) other tests than their source says: a module that
    /// runs before the loader reads them (the packages holding it, which
    /// discovery imports first; it; the project modules it imports, and
    /// theirs) changes one of those classes or its bases (`base.Base.test_x
    /// = f`, `setattr`, a classmethod called on it, the class passed to a
    /// call), or something aye can't follow. `None` when nothing may.
    pub(crate) fn effects(&self, test: &Module, loaded: &[Loaded]) -> Option<String> {
        if loaded.is_empty() {
            return None;
        }
        let classes: HashSet<ClassId> = loaded.iter().flat_map(|l| l.mro.iter().copied()).collect();
        // The names those classes go by, for what aye can't resolve.
        let mut names: HashSet<String> = loaded.iter().map(|l| l.name.clone()).collect();
        #[expect(
            clippy::iter_over_hash_type,
            reason = "fills a set: the order shows nowhere"
        )]
        for id in &classes {
            if let Some(owner) = self.module(id.module)
                && let Some(class) = owner.summary.classes.get(id.class)
            {
                names.insert(class.name.clone());
            }
        }
        let summary = &test.summary;
        let mut queue: Vec<ModuleId> = self
            .packages_of(&summary.path)
            .iter()
            .map(|init| self.id(init))
            .collect();
        queue.push(test.id);
        let mut seen: HashSet<ModuleId> = HashSet::new();
        while let Some(id) = queue.pop() {
            if !seen.insert(id) {
                continue;
            }
            if seen.len() > MAX_MODULES {
                return Some(format!(
                    "{} imports more than {MAX_MODULES} project modules, which may change its \
                     classes",
                    summary.path.display()
                ));
            }
            let Some(module) = (if id == test.id {
                Some(test.clone())
            } else {
                self.module(id)
            }) else {
                continue;
            };
            for effect in self.effects_of(&module).iter() {
                match effect {
                    Effect::Class(class, reason) if classes.contains(class) => {
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
            queue.extend(self.edges_of(&module).iter().copied());
        }
        None
    }

    fn member(&self, module: &Module, names: &[&str], depth: usize) -> Target {
        let summary = &module.summary;
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
            Binding::Class(index) if rest.is_empty() => Target::Class(module.clone(), *index),
            other if rest.is_empty() => Target::Binding(module.clone(), other.clone()),
            _ => Target::Unknown(format!("{} is an attribute aye can't follow", shown())),
        }
    }
}
