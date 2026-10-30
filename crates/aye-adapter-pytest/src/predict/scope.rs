//! What a test module's scope does to it: the support modules pytest has
//! loaded when it collects the module ([`SupportScope`]), and what its
//! imports refer to ([`Imports`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use aye_python_semantic::module_resolver::found_in;
use aye_python_semantic::namespace::Namespace;
use aye_python_stdlib::unittest;

use crate::conftest::{self, FixtureDef, Supports};

/// What the support files in scope do.
#[derive(Debug, Clone, Default)]
pub(crate) struct SupportScope {
    /// Hooks of the conftests in scope (and the plugins they load).
    pub hooks: BTreeSet<String>,
    /// [`GLOBAL_HOOKS`](crate::conftest::GLOBAL_HOOKS) implemented by any
    /// conftest of the project: pytest applies them to every test of the
    /// session once that conftest is loaded.
    pub global_hooks: BTreeSet<String>,
    pub collect_file_touches_py: bool,
    /// A conftest in scope calls `pytest.skip` or `pytest.importorskip`
    /// while it is imported.
    pub skips_at_import: bool,
    pub fixtures: Vec<FixtureDef>,
    /// Names conftests in scope import from modules that are not project
    /// files: each may be a fixture.
    pub maybe_fixtures: BTreeSet<String>,
    /// A conftest in scope star-imports a module that is not a project
    /// file: any name may be a fixture.
    pub any_fixture: bool,
    /// A conftest in scope loads plugins aye cannot name.
    pub hooks_unknown: bool,
    /// What the test module imports from other modules.
    pub module_imports: ModuleImports,
    /// The test module names plugins in `pytest_plugins`.
    pub module_plugins: bool,
    /// Modules whose attributes support files in scope may change.
    pub patched: BTreeSet<String>,
    /// A support file in scope reaches module namespaces in ways aye can't
    /// follow: the test module's names may change.
    pub reflective: bool,
    /// A support file in scope imports or looks up a module of the test
    /// module's name (by literal name).
    pub imports_module: bool,
    /// What implements each of `hooks` ("a conftest.py in scope", "a -p
    /// plugin"), for messages.
    pub hook_sources: BTreeMap<String, String>,
    /// The same for `global_hooks`.
    pub global_hook_sources: BTreeMap<String, String>,
    /// A conftest in scope loads plugins aye can't find while some
    /// conftest puts directories aye can't tell on `sys.path`: a name no
    /// readable definition has (but pytest's own) may be one of their
    /// fixtures.
    pub hidden_fixtures: bool,
}

impl SupportScope {
    /// What implements `hook`, of those in scope.
    pub(super) fn hook_source(&self, hook: &str) -> &str {
        self.hook_sources
            .get(hook)
            .map_or("a conftest.py in scope", String::as_str)
    }

    /// What implements `hook`, of the session-wide ones.
    pub(super) fn global_hook_source(&self, hook: &str) -> &str {
        self.global_hook_sources
            .get(hook)
            .map_or("a conftest.py", String::as_str)
    }
}

/// What the module's imports refer to, beyond its own source.
pub(crate) trait Imports {
    /// Whether the class `path` names (a dotted path through the module's
    /// imports) may be a unittest case, which a subclass inherits tests
    /// from whatever its name.
    fn may_be_unittest(&self, path: &str) -> bool;

    /// Whether `path` names something in a project file (not a module)
    /// that may be a unittest case, which pytest collects whatever its
    /// name.
    fn project_unittest(&self, path: &str) -> bool;

    /// Whether `path` names a class or function of a project file whose
    /// `__test__` may be true (set in a class body, inherited from one, or
    /// set by its module), which pytest collects whatever its name.
    fn project_test(&self, path: &str) -> bool;

    /// Whether `path` names a function of a project file that may make
    /// classes (a class statement, `type(…)`, `__test__`), whose result
    /// may then be a test class whatever the name it is bound to.
    fn project_factory(&self, path: &str) -> bool;
}

/// Fixtures a test module imports.
#[derive(Debug, Clone, Default)]
pub(crate) struct ModuleImports {
    /// Read from project files.
    pub fixtures: Vec<FixtureDef>,
    /// Imported from modules that are not project files: each may be a
    /// fixture.
    pub maybe: BTreeSet<String>,
    /// Star-imported from a module that is not a project file.
    pub any: bool,
}

/// Why code other than the module's own statements may change the test
/// module's names (empty when nothing does): the module imports itself by
/// name, or a support file in scope reaches module namespaces, imports a
/// module of its name, or implements a hook that gets the module before
/// pytest collects it. `rel` is the module's path.
pub(crate) fn reached(module: &Namespace<'_>, scope: &SupportScope, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    if names_module(module.imported_modules(), rel) {
        out.push("the module imports itself, which may change its names".to_string());
    }
    if scope.reflective {
        out.push("a conftest.py in scope reaches module namespaces at runtime".to_string());
    }
    if scope.imports_module {
        out.push("a conftest.py in scope imports a module of this name".to_string());
    }
    for hook in ["pytest_collectstart", "pytest_make_collect_report"] {
        if scope.hooks.contains(hook) {
            out.push(format!(
                "{} implements {hook}, which sees the module before pytest collects it",
                scope.hook_source(hook)
            ));
        }
    }
    out
}

/// Whether one of `modules` (dotted names, as imported) may be the module
/// at `rel`: its last part is the file's stem.
pub(crate) fn names_module(modules: &[String], rel: &str) -> bool {
    let stem = std::path::Path::new(rel)
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    modules.iter().any(|m| m.rsplit('.').next() == Some(&*stem))
}

/// A test module's imports, followed into the project's files.
pub(super) struct ProjectImports<'r> {
    pub(super) supports: &'r Supports,
    /// The test module's directory.
    pub(super) dir: &'r Path,
    pub(super) bases: &'r [PathBuf],
    /// Code that runs before the module's imports (its own, or a
    /// conftest's) puts directories aye can't tell on `sys.path`: a module
    /// it can't find may be a project file there.
    pub(super) path_unknown: bool,
    /// Directories the module itself puts on `sys.path` that aren't among
    /// `bases`.
    pub(super) own_dirs: &'r [PathBuf],
}

impl Imports for ProjectImports<'_> {
    fn may_be_unittest(&self, path: &str) -> bool {
        self.supports.may_be_unittest(path, self.dir, self.bases)
    }

    /// [`Supports::project_unittest`], and for a module aye can't find
    /// (an installed package, or a directory put on `sys.path` in a way it
    /// can't tell): any class when `sys.path` may hold project files it
    /// can't see, else one named like a unittest case.
    fn project_unittest(&self, path: &str) -> bool {
        if self.supports.project_unittest(path, self.dir, self.bases) {
            return true;
        }
        if conftest::no_tests(path) || self.supports.resolvable(path, self.dir, self.bases) {
            return false;
        }
        let name = path.rsplit('.').next().unwrap_or(path);
        self.path_unknown || found_in(path, self.own_dirs) || unittest::named_like_test_case(name)
    }

    fn project_test(&self, path: &str) -> bool {
        self.supports.project_test(path, self.dir, self.bases)
    }

    fn project_factory(&self, path: &str) -> bool {
        self.supports.project_factory(path, self.dir, self.bases)
    }
}
