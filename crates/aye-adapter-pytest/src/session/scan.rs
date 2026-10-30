//! What walking pytest's session once finds ([`Scan`]): the session-wide
//! hooks of its conftests and the plugins they and test modules load, what
//! they put on `sys.path`, and the module names test directories share.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **The project scan covers what pytest's session can load**: the
//!   directories the run collects (the paths given, else `testpaths` or the
//!   current directory) and the conftests above them up to the root, not the
//!   whole project. pytest loads no other conftest, so hooks, `sys.path`
//!   changes and colliding module names elsewhere can't matter, and a run
//!   of one directory of a large repository doesn't walk all of it.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **`sys.path` is read where it can be.** A support module's code that
//!   runs while pytest collects (at import, and in `pytest_*` hooks) may
//!   put directories on `sys.path` (`insert`, `append`, `extend`, `+=`,
//!   slice assignment, `site.addsitedir`). Those built from `__file__` and
//!   literals through `os.path` (`dirname`, `join`, `abspath`, `realpath`,
//!   `normpath`) and `pathlib` (`Path`, `.parent`, `.parents[n]`, `/`,
//!   `joinpath`, `resolve`, `absolute`), directly or through a module
//!   constant, are read, from every conftest of the project (found once by
//!   walking it), and their modules are project files, as `PYTHONPATH`'s
//!   are. Others (a relative path, a computed one, a new list) leave
//!   `sys.path` unknown: then a capitalized name imported from a module
//!   aye can't find may be a unittest case (pytest collects one whatever
//!   its name), and a fixture name no readable definition has (pytest's own
//!   aside) may come from a plugin a conftest in scope names but aye can't
//!   find: both refine. What a test module puts on `sys.path` itself counts
//!   for its own imports. Without such doubt, a module aye can't find is an
//!   installed package's, whose classes are unittest cases only when named
//!   like one (`…Test`, `…Tests`, `…TestCase`, `…TestCases`).
//! - **Helpers that collide across test directories.** With `prepend` or
//!   `append`, pytest puts each test module's package base on `sys.path`
//!   and Python keeps the first module it imports under a name: a test
//!   module that imports (absolutely) a module its own base holds and
//!   another test directory's base holds too (`helpers.py` in two
//!   rootdir-inserted directories) gets whichever pytest imported first.
//!   Such a module is refined (its static cases `Partial`), and its first
//!   affinity key is `import:NAME`, NAME being that module's name, so that
//!   the modules sharing it run in one session, as pytest runs them. The
//!   context's `colliding_modules` lists the names, for the front-end to
//!   give the files it lists the same key.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aye_adapter::{Recognizer, SourceFile};
use aye_pytest::config;
use aye_pytest::conftest::in_confcutdir;
use aye_pytest::doctest::DoctestFile;
use aye_python_semantic::module_resolver::absolute_imports;
use aye_python_semantic::sys_path::SysPath;
use aye_python_stdlib::os_path::normalize_absolute;

use super::plugins::HookSource;
use crate::PytestRecognizer;
use crate::conftest::{self, Summary};

/// What walking the project once finds ([`PytestRecognizer::scan`]).
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// [`conftest::GLOBAL_HOOKS`] implemented by any conftest of the project,
    /// each with what implements it first (for messages).
    hooks: BTreeMap<String, HookSource>,
    /// The summaries of test modules that load plugins (kept, since
    /// [`conftest::Supports`] caches by address).
    modules: Vec<Arc<Summary>>,
    /// What the project's conftests put on `sys.path`.
    sys_path: SysPath,
    /// Module names more than one test directory's `sys.path` entry holds
    /// (see [`PytestRecognizer::colliding`]).
    colliding: BTreeSet<String>,
    /// What implements a `pytest_collection_modifyitems` that may add
    /// tests (its module makes items: [`conftest::Summary::makes_items`]),
    /// the first found, for messages.
    pub(crate) adds_items: Option<HookSource>,
}

/// The session-wide hook that may add tests (finding C16).
pub(crate) const MODIFYITEMS: &str = "pytest_collection_modifyitems";

impl PytestRecognizer {
    /// [`conftest::GLOBAL_HOOKS`] implemented by any conftest pytest may
    /// load in this project (and the plugins they load, or test modules
    /// load through `pytest_plugins`, which hook every module collected
    /// after them), each with what implements it first, for messages.
    pub(crate) fn global_hooks(&self) -> &BTreeMap<String, HookSource> {
        &self.scan().hooks
    }

    /// What pytest's session can load, walked once the way collection walks
    /// it: the session-wide hooks of its conftests (see
    /// [`Self::global_hooks`]), what they put on `sys.path` (see
    /// [`Self::sys_path`]) and the module names its test directories share
    /// (see [`Self::colliding`]). That is the directories the run collects
    /// (the paths given, else `testpaths` or the current directory, as
    /// pytest's arguments), each with the conftests above it up to the root:
    /// pytest loads no other conftest, so a run of one directory doesn't
    /// pay for the whole project. The walk claims files, so claiming must
    /// not need it: it looks no module up.
    pub(crate) fn scan(&self) -> &Scan {
        self.scan.get_or_init(|| {
            let mut scan = Scan::default();
            let mut conftests: Vec<Arc<Summary>> = Vec::new();
            let mut test_bases: BTreeSet<PathBuf> = BTreeSet::new();
            let (mut stack, files) = self.scan_starts();
            for dir in self.scan_above(&stack, &files) {
                if let Some(summary) = self.conftest_in(&dir) {
                    if in_confcutdir(&self.config.confcutdir, &dir) {
                        add_sys_path(&mut scan.sys_path, &summary);
                    }
                    conftests.push(summary);
                }
            }
            for file in &files {
                if let (Some(dir), Ok(rel)) = (file.parent(), file.strip_prefix(&self.root)) {
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.scan_file(&mut scan, &mut test_bases, &mut None, dir, file, &rel);
                }
            }
            while let Some(dir) = stack.pop() {
                if let Some(summary) = self.conftest_in(&dir) {
                    if in_confcutdir(&self.config.confcutdir, &dir) {
                        add_sys_path(&mut scan.sys_path, &summary);
                    }
                    conftests.push(summary);
                }
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut base: Option<PathBuf> = None;
                for entry in entries.filter_map(Result::ok) {
                    let path = entry.path();
                    let Ok(kind) = entry.file_type() else {
                        continue;
                    };
                    if kind.is_dir() {
                        if self.descend(&path) {
                            stack.push(path);
                        }
                        continue;
                    }
                    let Ok(rel) = path.strip_prefix(&self.root) else {
                        continue;
                    };
                    let rel = rel.to_string_lossy().replace('\\', "/");
                    self.scan_file(&mut scan, &mut test_bases, &mut base, &dir, &path, &rel);
                }
            }
            let session = self.preparse.iter().map(|plugin| &plugin.summary);
            for summary in session.chain(&self.outer) {
                add_sys_path(&mut scan.sys_path, summary);
            }
            // Modules looked up so far (`-p` plugins) are looked up again,
            // where the conftests' directories count too.
            self.supports.forget_resolutions();
            let mut bases = self.basic_bases();
            for dir in &scan.sys_path.added {
                if !bases.contains(dir) {
                    bases.push(dir.clone());
                }
            }
            // Each with what implements its hooks: the summary itself
            // (`own`), or a plugin it loads (`loaded`).
            let labelled = self
                .preparse
                .iter()
                .map(|plugin| {
                    let source = plugin.source.hook_source();
                    (&plugin.summary, source, source)
                })
                .chain(
                    self.outer
                        .iter()
                        .chain(&conftests)
                        .map(|s| (s, HookSource::Conftest, HookSource::PluginOfConftest)),
                )
                .chain(scan.modules.iter().map(|s| {
                    let source = HookSource::PluginOfTestModule;
                    (s, source, source)
                }));
            for (summary, own, loaded) in labelled {
                for (i, expanded) in self.supports.expand(summary, &bases).iter().enumerate() {
                    let exports = self.supports.exports(expanded, &bases);
                    let source = if i == 0 { own } else { loaded };
                    for hook in conftest::GLOBAL_HOOKS {
                        if exports.hooks.contains(hook) {
                            scan.hooks.entry(hook.to_string()).or_insert(source);
                        }
                    }
                    if exports.makes_items
                        && exports.hooks.contains(MODIFYITEMS)
                        && scan.adds_items.is_none()
                    {
                        scan.adds_items = Some(source);
                    }
                }
            }
            scan.colliding = self.colliding_names(&test_bases);
            scan
        })
    }

    /// Where [`Self::scan`] walks: the directories the run collects, none
    /// inside another, and the files it names outside them. The project
    /// root when it names nothing that exists.
    fn scan_starts(&self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut files: Vec<PathBuf> = Vec::new();
        for arg in &self.collection_args {
            let arg = normalize_absolute(arg);
            if arg.is_dir() {
                dirs.push(arg);
            } else if arg.is_file() {
                files.push(arg);
            }
        }
        if dirs.is_empty() && files.is_empty() {
            dirs.push(self.root.clone());
        }
        dirs.sort();
        let mut kept: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if !kept.iter().any(|outer| dir.starts_with(outer)) {
                kept.push(dir);
            }
        }
        files.retain(|file| !kept.iter().any(|dir| file.starts_with(dir)));
        files.dedup();
        (kept, files)
    }

    /// The directories above where [`Self::scan`] walks, up to the root,
    /// outermost first: their conftests load before anything collected
    /// below them. A file's own directory counts, since it isn't walked.
    fn scan_above(&self, dirs: &[PathBuf], files: &[PathBuf]) -> Vec<PathBuf> {
        let mut above: Vec<PathBuf> = Vec::new();
        let starts = dirs
            .iter()
            .filter_map(|dir| dir.parent())
            .chain(files.iter().filter_map(|file| file.parent()));
        for start in starts {
            for dir in start.ancestors() {
                if !dir.starts_with(&self.root) {
                    break;
                }
                if !above.iter().any(|seen| seen == dir) {
                    above.push(dir.to_path_buf());
                }
            }
        }
        above.sort_by_key(|dir| dir.components().count());
        above
    }

    /// One file met by [`Self::scan`] in `dir`: a test or doctest module
    /// marks its package base (`base`, once per directory), and a test
    /// module's `pytest_plugins` joins the plugins the session loads.
    fn scan_file(
        &self,
        scan: &mut Scan,
        test_bases: &mut BTreeSet<PathBuf>,
        base: &mut Option<PathBuf>,
        dir: &Path,
        path: &Path,
        rel: &str,
    ) {
        // pytest imports test modules, and doctest modules with
        // `--doctest-modules` (text files it only reads).
        let python = self.python_module(rel, false);
        let doctest = self.doctest_file(rel, false) == Some(DoctestFile::Module);
        if !python && !doctest {
            return;
        }
        if base.is_none() {
            let found = self.supports.package_base(dir);
            test_bases.insert(found.clone());
            *base = Some(found);
        }
        if !python {
            // A doctest module's `pytest_plugins` loads nothing.
            return;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            return;
        };
        if !source.contains("pytest_plugins") {
            return;
        }
        let Ok(parsed) = ruff_python_parser::parse_module(&source) else {
            return;
        };
        let module = Arc::new(Summary::build(dir, &source, parsed.syntax()));
        if !module.plugins.is_empty() {
            scan.modules.push(module);
        }
    }

    /// What the conftests pytest may load in this project (and those above
    /// it, and the `-p` plugins) put on `sys.path`: pytest keeps them there
    /// for the rest of the session, so their modules are project files
    /// too, wherever they are imported from.
    pub(crate) fn sys_path(&self) -> &SysPath {
        &self.scan().sys_path
    }

    /// Module names that more than one directory pytest puts on
    /// `sys.path` for test modules holds. With the `prepend` and `append`
    /// import modes, pytest puts each test module's package base (its
    /// directory, outside packages) on `sys.path`, and Python keeps the
    /// first module it imports under a name for the rest of the session: a
    /// test module importing such a name gets whichever of them pytest
    /// imported first, which depends on the other files of the session.
    /// Empty with `importlib`, which puts no test directory on `sys.path`,
    /// and with `consider_namespace_packages`.
    pub(crate) fn colliding(&self) -> &BTreeSet<String> {
        &self.scan().colliding
    }

    /// [`Self::colliding`], from the package bases of the test modules.
    fn colliding_names(&self, bases: &BTreeSet<PathBuf>) -> BTreeSet<String> {
        if self.config.import_mode == config::ImportMode::Importlib
            || self.config.consider_namespace_packages
            || bases.len() < 2
        {
            return BTreeSet::new();
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        for base in bases {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let name = if path.extension().is_some_and(|e| e == "py") {
                    path.file_stem().map(|n| n.to_string_lossy().into_owned())
                } else if path.join("__init__.py").is_file() {
                    path.file_name().map(|n| n.to_string_lossy().into_owned())
                } else {
                    None
                };
                if let Some(name) = name
                    && name != "conftest"
                    && name != "__init__"
                {
                    *seen.entry(name).or_default() += 1;
                }
            }
        }
        seen.into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, _)| name)
            .collect()
    }

    /// The first module name the test module at `file` imports (anywhere
    /// in it, absolutely) that its own `sys.path` entry holds and another
    /// test directory's holds too (see [`Self::colliding`]).
    pub(crate) fn colliding_import(&self, file: &SourceFile<'_>) -> Option<String> {
        let colliding = self.colliding();
        if colliding.is_empty() {
            return None;
        }
        let base = self.supports.package_base(file.abs.parent()?);
        // In order: the first name is the key.
        absolute_imports(file.module)
            .into_iter()
            .filter(|name| colliding.contains(name))
            .find(|name| {
                base.join(format!("{name}.py")).is_file()
                    || base.join(name).join("__init__.py").is_file()
            })
    }
}

/// Add what `summary`'s code puts on `sys.path` to `out`.
fn add_sys_path(out: &mut SysPath, summary: &Summary) {
    for dir in &summary.sys_path.added {
        if !out.added.contains(dir) {
            out.added.push(dir.clone());
        }
    }
    out.unknown |= summary.sys_path.unknown;
}
