//! Recognition of one test module, the way pytest's `Module`, `Class` and
//! `UnitTestCase` collectors walk it (`_pytest/python.py`,
//! `_pytest/unittest.py`), without importing it.
//!
//! Decided (implementation, October 2026), each revisable until 0.1.0:
//! - **Refinement versus `Partial`.** A file is refined when its ids, or
//!   which tests exist, may differ from the static reading (Core decision 5:
//!   when unsure, pytest decides): parametrize arguments that are not
//!   static, `ids=` callables, parametrized fixtures in the test's closure
//!   (through arguments, fixtures, `usefixtures` and autouse, with pytest's
//!   override rules), `pytest_generate_tests` in the module, the class or a
//!   conftest in scope, `pytest_make_parametrize_id` with parametrize, the
//!   collection hooks `pytest_pycollect_makeitem`,
//!   `pytest_pycollect_makemodule`, `pytest_collection`,
//!   `pytest_ignore_collect` and `pytest_collect_directory`,
//!   `pytest_collect_file` when it mentions Python files, a conftest in
//!   scope that loads plugins aye can't name (`pytest_plugins` it can't read,
//!   `pluginmanager.register(make())`), a test module's own
//!   `pytest_plugins`, classes whose bases aren't all known (whatever their
//!   name, since a unittest subclass inherits its tests; classes from
//!   packages that ship no test cases, such as the standard library's,
//!   don't count), class members added at runtime (class decorators aye
//!   can't see through, a metaclass other than `ABCMeta`, a base's
//!   `__init_subclass__`, `exec` or `locals()` in a class body, test names
//!   the module sets on the class), decorators aye can't see through on a
//!   test (they may parametrize, rename, replace or multiply it: ddt,
//!   `parameterized.expand`, registries), including the module's own unless
//!   they visibly return their argument or a `functools.wraps` wrapper of
//!   it, `pytestmark` entries it can't read and `test.pytestmark = …` (each
//!   may be a parametrize; `.append`, `.extend` and `+=` on a list are
//!   read), generator tests (pytest fails the module, which refinement
//!   reports once; a decorated one may be a plain function), imported or
//!   assigned names that may be tests (see "What may change which tests
//!   exist" below; a capitalized name from a project file
//!   that may be a unittest case, which pytest collects whatever its name,
//!   counts: a class whose bases may make it one, or a name aye can't tell
//!   is no class, followed through the project's re-exports), module
//!   namespaces reached in ways aye can't follow (`sys.modules[…]` for a
//!   computed name, frames, `f_globals`, `__globals__`, `gc`,
//!   `importlib.import_module` or `__import__` of a computed name, the
//!   module importing itself), and likewise a conftest in scope that does
//!   so, imports a module of the test module's name or implements
//!   `pytest_collectstart` or `pytest_make_collect_report` (they get the
//!   module before pytest collects it), code that changes `pytest`'s API or
//!   collection internals, a test file with no test aye can see (pytest
//!   imports it all the same: an import error is a collection error, and
//!   tests may exist only at runtime), star imports, test names defined in
//!   a branch that may not run (`if`, loops, `match`, `except`, a `try`
//!   body after a statement that may raise) or differently in alternative
//!   branches, a
//!   `del` that may not run, conditional `collect_ignore` entries,
//!   parametrization of package, session or unknown scope (pytest reorders
//!   the whole session by it), parametrize arguments pytest refuses (a
//!   `scope` other than its five names, an `indirect` that is neither a
//!   boolean nor a sequence of the argument names, `pytest.param` marks
//!   that are a set, which isn't hashable, or hold something that is surely
//!   no mark), `pytest.skip` or `pytest.importorskip` run while the
//!   module or a conftest in scope is imported, and whatever pytest reports
//!   as a collection error. The static guess keeps the unparametrized id
//!   with `Partial` certainty. Hooks that only mark or deselect
//!   (`pytest_collection_modifyitems`, unless its module makes items: see
//!   "Tests a `pytest_collection_modifyitems` adds" below;
//!   `pytest_itemcollected`), decorators of well-known libraries that keep
//!   the test (Hypothesis, `flaky`), unread `pytestmark` entries of
//!   unittest classes (parametrize doesn't apply there), fixtures imported
//!   from modules that aren't project files and `-p NAME` or
//!   `PYTEST_PLUGINS` plugins aye can't read (neither pytest's own nor
//!   project modules: they may mark, deselect or parametrize anything) keep
//!   their ids but make the case `Partial`.
//! - **`pytest_plugins` in a non-top-level conftest fails its directory**,
//!   as `_check_non_top_pytest_plugins` makes pytest fail it: a conftest
//!   outside pytest's initial conftests (those of the collection arguments'
//!   directories, their `test*` subdirectories and above; see
//!   [`Recognizer::roots`]) that surely defines `pytest_plugins` (a plain
//!   top-level assignment, never deleted) gives the tests below it no cases
//!   and one `collection-error` diagnostic on its directory, with pytest's
//!   message, unless `--pyargs` is given. A definition aye isn't sure of is
//!   left alone, and so is a directory holding no test file: the
//!   front-end starts from the user's paths, so it fails such a directory
//!   there as pytest would in the user's run.
//! - **What may change which tests exist** (Core decision 5: whatever aye
//!   can't prove is refined, and its guess is `Partial`, never `Exact`).
//!   Refined, besides the causes above:
//!   - a class whose tests may be added or removed: one handed to code aye
//!     can't follow (passed to a call, iterated over, held by a name whose
//!     attributes are set, `setattr`, `del K.attr`, a method called on it;
//!     the escape analysis, which doesn't count `super(K, …)`,
//!     reads, or the module's own `K.attr = …` statements), one holding a
//!     value when the module defines a `__set_name__`, one whose class or
//!     base the module sets a test name, `pytestmark`, `runTest` or a dunder
//!     on (`__bases__`, `__init__`; not `__doc__`, `__name__`,
//!     `__qualname__`, `__module__`); its cases are `Partial`. A non-test
//!     class with test methods handed on may get `__test__`;
//!   - a test function handed to code aye can't follow (`mark(test)`,
//!     `disable(test)`, `vars(test)[…] = …`, `TestK.test_a` handed on):
//!     its marks or `__test__` may change; its case is `Partial`;
//!   - a value bound by a call whatever its name, when the callee may make
//!     a class: `type(…)` with three arguments, `types.new_class`, and
//!     functions (or lambdas, or methods of the module's classes) of the
//!     module or of project files whose bodies hold a class statement,
//!     `type(…)`, `new_class` or `__test__`. Under a test function's name,
//!     every call but a builtin that returns data or a class of the module
//!     whose instances aren't callable (no `__new__`, `__call__`,
//!     metaclass or unknown base) may make a function (`functools.partial`,
//!     `update_wrapper`, a capitalized factory);
//!   - names imported from project files (or bound to them) whose
//!     `__test__` may be true (a class body setting it to anything but
//!     `False`, a project base that does, or the module's `name.__test__ =
//!     …`), and classes inheriting from them;
//!   - a non-test class whose bases from project files are no unittest case
//!     but whose other bases make it one or set `__test__` (pytest collects
//!     it); its guess, from the bases aye reads, is `Partial`;
//!   - names a function that runs at import binds or deletes through
//!     `global` (unless a non-test name is bound to a literal); their cases
//!     are `Partial`;
//!   - test names defined, and `del`s, in a `with` body after a statement
//!     that may raise (any context manager may swallow it, as
//!     `contextlib.suppress` does), as in a `try` body;
//!   - `from unittest import *` (it binds `FunctionTestCase`, which pytest
//!     collects);
//!   - module-level code that surely or may raise: a division of number
//!     literals by zero, a `raise` statement (`unittest.SkipTest` skips the
//!     module), an `assert` of a false literal.
//!
//!   `__test__` is read along the MRO from class bodies and from the
//!   module's own `K.__test__ = <literal>` statements, which win over the
//!   body's; a conditional or computed one is left to pytest. Private names
//!   in class bodies are mangled as Python stores them (`__x` in `K` is
//!   `_K__x`), which custom `python_functions`/`python_classes` globs may
//!   match. A reason that holds for the whole module (it may raise or skip
//!   itself at import, star imports, reflection, collection hooks, plugins
//!   aye can't read, a `collect_ignore` that may exclude it) makes every
//!   case of its guess `Partial`. Reasons name what implements a hook: "a
//!   conftest.py (in scope)", "a -p plugin", "a `PYTEST_PLUGINS` plugin",
//!   "a plugin a conftest.py loads". Known gaps: descriptors defined in
//!   other modules, factories from installed packages, classes taken out
//!   of containers (`A = CASES[0]`), and changes made by other modules.
//! - **Diagnostics**: `class-has-init` (pytest's collection warning),
//!   `duplicate-definition` (a test name bound twice in one namespace,
//!   outside exclusive branches and with no `del` between; the last
//!   definition is the one collected, at the position of the first, as
//!   Python's dicts keep it), and `collection-error` for a non-top-level
//!   `pytest_plugins`. A generator test is pytest's collection error too,
//!   but it is left to refinement, so it is reported once, as pytest
//!   reports it.
//!
//! [`Recognizer::roots`]: aye_adapter::Recognizer::roots

mod case;
mod class;
mod closure;
mod function;
mod module;
mod refinement;
mod reorder;
mod scope;
mod unittest;

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use aye_adapter::ast::{Stmt, StmtClassDef};
use aye_adapter::{Ranged, Recognition, Requested, SourceFile, TextRange, TextSize};
use aye_events::{
    AffinityKey, Case, Certainty, Diagnostic, DiagnosticCode, DiagnosticPath, Severity,
};
use aye_pytest::conftest::in_confcutdir;
use aye_pytest::doctest::DoctestFile;
use aye_python_semantic::flow::{BodyFacts, conditional_deletes, guarded_by_try};
use aye_python_semantic::namespace::Namespace;
use aye_python_semantic::resolve::Lookup;
use aye_python_stdlib::fnmatch::matches_prefix_or_glob;

pub(crate) use scope::{Imports, ModuleImports, SupportScope, names_module, reached};

use crate::PytestRecognizer;
use crate::claims::doctest_plugin::{PYTEST_DOCTESTS, doctest_text, may_hold_doctests};
use crate::claims::ignore::Ignored;
use crate::conftest::{self, FixtureDef, MARKING_HOOKS, Summary, fixture_of};
use crate::marks::ids::IdOptions;
use crate::marks::{EmptyMark, Mark};
use crate::session::plugins::{BUILTIN_PLUGINS, PLUGIN_OF_SCOPED_CONFTEST, SCOPED_CONFTEST};
use crate::session::scan::MODIFYITEMS;
use crate::{PYTEST, SETS};
use refinement::Refinement;
use reorder::{ParamKey, reorder_items};
use scope::ProjectImports;

impl PytestRecognizer {
    /// What pytest collects from `file`, predicted without importing
    /// anything.
    pub(crate) fn predict(
        &self,
        file: &SourceFile<'_>,
        requested: Requested,
        support: &[SourceFile<'_>],
    ) -> Recognition {
        let explicit = requested.is_named();
        let doctest = self.doctest_file(file.rel.as_str(), explicit);
        if self.settings.python_blocked && doctest.is_none() {
            // `-p no:python`: nothing collects Python files.
            return Recognition::default();
        }
        let bases = self.plugin_bases();
        let mut local: Vec<(&Path, Arc<Summary>)> = Vec::new();
        if !self.config.noconftest {
            for conftest in support {
                let dir = conftest.abs.parent().unwrap_or(conftest.abs);
                if in_confcutdir(&self.config.confcutdir, dir) {
                    local.push((dir, self.supports.of(conftest)));
                }
            }
        }
        if let Some((dir, message)) = self.non_top_level_plugins(&local) {
            // pytest fails the directory, so none of the tests below it
            // exist; the error is reported once.
            let mut recognition = Recognition::default();
            if self.reported.lock().expect("lock").insert(dir.clone()) {
                let rel = dir.strip_prefix(&self.root).map_or_else(
                    |_| dir.to_string_lossy().into_owned(),
                    |rel| rel.to_string_lossy().replace('\\', "/"),
                );
                recognition.diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    code: DiagnosticCode::CollectionError,
                    path: if rel.is_empty() {
                        DiagnosticPath::Root
                    } else {
                        DiagnosticPath::as_written(rel)
                    },
                    span: None,
                    message,
                });
            }
            return recognition;
        }
        match doctest {
            Some(DoctestFile::Text) => return doctest_text(file),
            Some(DoctestFile::Module)
                if self.settings.python_blocked
                    || !self.python_module(file.rel.as_str(), explicit) =>
            {
                // Only pytest's doctest plugin collects it, importing it: an
                // import error is a collection error.
                return Recognition {
                    refine: Some(
                        format!(
                            "pytest's doctest plugin imports it for its doctests ({PYTEST_DOCTESTS})"
                        )
                        .into(),
                    ),
                    ..Recognition::default()
                };
            }
            _ => {}
        }
        let mut conftests: Vec<Arc<Summary>> = self.outer.clone();
        conftests.extend(local.into_iter().map(|(_, summary)| summary));
        // `-p` plugins first, then each conftest's plugins, then the
        // conftests: the order pytest registers their fixtures in.
        let mut plugins: Vec<(Arc<Summary>, &str)> = Vec::new();
        for (summary, label) in self.preparse.iter().zip(&self.preparse_labels) {
            let expanded = self.supports.expand(summary, &bases);
            plugins.extend(expanded.into_iter().map(|s| (s, *label)));
        }
        for conftest in &conftests {
            let expanded = self.supports.expand(conftest, &bases).into_iter().skip(1);
            plugins.extend(expanded.map(|s| (s, PLUGIN_OF_SCOPED_CONFTEST)));
        }
        let global = self.global_hooks();
        let mut scope = SupportScope {
            global_hooks: global.keys().cloned().collect(),
            global_hook_sources: global.clone(),
            ..SupportScope::default()
        };
        // A collection ini value a support module in scope changes at
        // runtime (`config.addinivalue_line("python_functions", …)` in a
        // `pytest_configure`): pytest's reading of the file may differ.
        let ini_change = plugins
            .iter()
            .map(|(s, _)| s)
            .chain(&conftests)
            .find_map(|summary| summary.ini.collection.clone());
        let labelled = conftests.iter().map(|s| (s, SCOPED_CONFTEST));
        for (summary, source) in plugins.iter().map(|(s, l)| (s, *l)).chain(labelled) {
            let exports = self.supports.exports(summary, &bases);
            for hook in &exports.hooks {
                scope
                    .hook_sources
                    .entry(hook.clone())
                    .or_insert_with(|| source.to_string());
            }
            scope.hooks.extend(exports.hooks.iter().cloned());
            scope.collect_file_touches_py |= exports.collect_file_touches_py;
            scope.skips_at_import |= exports.skips_at_import;
            scope.fixtures.extend(exports.fixtures.iter().cloned());
            scope
                .maybe_fixtures
                .extend(exports.maybe_fixtures.iter().cloned());
            scope.any_fixture |= exports.any_fixture;
            scope.hooks_unknown |= summary.hooks_unknown();
            scope.patched.extend(summary.patched.iter().cloned());
            scope.reflective |= summary.reflective;
            scope.imports_module |= names_module(&summary.modules, file.rel.as_str());
        }
        // Fixtures the test module imports from project files register
        // with its own.
        let dir = file.abs.parent().unwrap_or(file.abs);
        let mut namespace = Namespace::build(&file.module.body, PYTEST);
        // What support files do to modules holds here too.
        namespace.assume_patched(scope.patched.iter().cloned(), scope.reflective);
        if !reached(&namespace, &scope, file.rel.as_str()).is_empty() {
            // Its names may change in ways the module's code doesn't show:
            // none is a constant.
            namespace.assume_reached();
        }
        let own = Summary::of_test_module(dir, file.source, file.module, &namespace);
        // Only names the module may request as fixtures are followed, and
        // those the fixtures found request in turn.
        let mut requested = conftest::requested_names(file.module);
        let exports = loop {
            let exports = self
                .supports
                .exports_of(&own, &bases, |name| requested.contains(name));
            let before = requested.len();
            for fixture in &exports.fixtures {
                requested.extend(fixture.args.iter().cloned());
            }
            if requested.len() == before {
                break exports;
            }
        };
        scope.module_imports = ModuleImports {
            fixtures: exports.fixtures[own.fixtures.len()..].to_vec(),
            maybe: exports.maybe_fixtures,
            any: exports.any_fixture,
        };
        if !own.plugins.is_empty() || own.plugins_unknown {
            scope.module_plugins = true;
        }
        // `sys.path` may hold directories aye can't tell: those a conftest
        // puts there in ways it can't read, or that the module puts there
        // itself (its own imports would be looked up there).
        let own_dirs: Vec<PathBuf> = own
            .sys_path
            .added
            .iter()
            .filter(|d| !bases.contains(d))
            .cloned()
            .collect();
        let path_unknown = self.sys_path().unknown || own.sys_path.unknown;
        if self.sys_path().unknown {
            // A plugin a conftest in scope loads that aye can't find may be
            // a project file there, with fixtures of its own.
            scope.hidden_fixtures =
                plugins
                    .iter()
                    .map(|(s, _)| s)
                    .chain(&conftests)
                    .any(|summary| {
                        summary
                            .plugins
                            .iter()
                            .chain(&summary.registered)
                            .any(|name| {
                                !BUILTIN_PLUGINS.contains(&name.as_str())
                                    && !conftest::no_tests(name)
                                    && self.supports.module(name, &summary.dir, &bases).is_none()
                            })
                    });
        }

        let imports = ProjectImports {
            supports: &self.supports,
            dir,
            bases: &bases,
            path_unknown,
            own_dirs: &own_dirs,
        };
        let (mut recognition, mut refinement) = recognize(
            &self.settings,
            &scope,
            &imports,
            file,
            &namespace,
            &self.path_keywords(file),
        );
        if recognition.cases.is_empty() && refinement.is_empty() {
            // Nothing to run statically, but pytest imports the module all
            // the same: an import error is a collection error, and tests
            // may only exist at runtime.
            refinement
                .after("no test is visible statically; pytest imports the module to tell".into());
        }
        let maybe_ignored = file
            .abs
            .ancestors()
            .take_while(|dir| dir.starts_with(&self.root) || *dir == file.abs)
            .any(|path| self.ignored(path) == Ignored::Maybe);
        if maybe_ignored {
            // None of its tests may exist.
            refinement.before("collect_ignore may exclude this file at runtime".into());
        }
        if let Some(changed) = ini_change {
            refinement.after(format!(
                "a conftest.py in scope changes {changed} at runtime, which collection reads"
            ));
        }
        let mut key = self.import_key(file.abs);
        if let Some(name) = self.colliding_import(file) {
            refinement.after(format!(
                "it imports {name}, a module of more than one directory pytest puts on sys.path: \
                 which one it gets depends on what pytest imported before"
            ));
            // Test modules that import it run in one session, as pytest
            // runs them.
            key = AffinityKey::import(&name);
        }
        for case in &mut recognition.cases {
            case.affinity.insert(0, key.clone());
        }
        if !self.unseen_plugins.is_empty() {
            // A plugin aye can't read may mark, deselect or parametrize any
            // test: the ids stay, pytest has the final word.
            refinement.demote();
        }
        if let Some(who) = &self.scan().adds_items {
            // pytest lists what such a hook adds (`Function.from_parent`),
            // which only the session that runs it can tell.
            refinement.after(format!(
                "{who} implements {MODIFYITEMS} and makes items: it may add tests"
            ));
        }
        if doctest == Some(DoctestFile::Module) && may_hold_doctests(file.source) {
            refinement.after(format!(
                "pytest's doctest plugin collects its doctests ({PYTEST_DOCTESTS})"
            ));
        }
        refinement.apply(&mut recognition);
        recognition
    }

    /// The names `-k` matches for a file's position: the directories
    /// between the rootdir and the file, and the file name.
    fn path_keywords(&self, file: &SourceFile<'_>) -> Vec<String> {
        match file.abs.strip_prefix(&self.config.rootdir) {
            Ok(rel) => rel
                .components()
                .filter_map(|c| match c {
                    Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect(),
            Err(_) => file.rel.as_str().split('/').map(str::to_string).collect(),
        }
    }
}

/// The configuration recognition depends on.
#[derive(Debug, Clone)]
pub(crate) struct Settings {
    pub python_classes: Vec<String>,
    pub python_functions: Vec<String>,
    pub ids: IdOptions,
    pub empty: EmptyMark,
    pub collect_imported_tests: bool,
    /// The `usefixtures` ini option.
    pub usefixtures: Vec<String>,
    /// `-p no:unittest`: `TestCase` classes are plain classes to pytest.
    pub unittest_blocked: bool,
    /// `-p no:python`: nothing collects Python files.
    pub python_blocked: bool,
}

/// Recognize one test module: its cases and diagnostics, and why it is
/// refined (the recognition's own reason is the refinement's to say).
/// `module` is its namespace ([`Namespace::build`] over its body).
pub(crate) fn recognize<'a>(
    settings: &Settings,
    scope: &SupportScope,
    imports: &dyn Imports,
    file: &SourceFile<'a>,
    module: &Namespace<'a>,
    path_keywords: &[String],
) -> (Recognition, Refinement) {
    let mut classes = HashMap::new();
    class_namespaces(&file.module.body, None, &mut classes);
    let empty = Namespace::default();
    let mut recognizer = ModuleRecognizer {
        settings,
        scope,
        imports,
        file,
        module,
        classes: &classes,
        empty: &empty,
        path_keywords,
        cases: Vec::new(),
        keys: Vec::new(),
        facts: std::cell::RefCell::default(),
        diagnostics: Vec::new(),
        refine: Vec::new(),
        module_marks: Vec::new(),
        module_marks_unknown: false,
        module_fixtures: Vec::new(),
        class_fixtures: HashMap::new(),
        module_wide: false,
        partial_names: HashSet::new(),
        sets_names: classes
            .values()
            .any(|info| info.ns.get("__set_name__").is_some()),
    };
    recognizer.run();
    if recognizer.module_wide || !recognizer.partial_names.is_empty() {
        let prefix = format!("{}::", file.rel);
        for case in &mut recognizer.cases {
            let name = case.id.as_str().strip_prefix(&prefix).unwrap_or_default();
            let name = name.split("::").next().unwrap_or_default();
            let name = name.split('[').next().unwrap_or_default();
            if (recognizer.module_wide || recognizer.partial_names.contains(name))
                && case.certainty == Certainty::Exact
            {
                case.certainty = Certainty::Partial;
            }
        }
    }
    if recognizer.keys.iter().any(|k| !k.is_empty()) {
        let order = reorder_items(&recognizer.keys);
        let mut cases: Vec<Option<Case>> = recognizer.cases.drain(..).map(Some).collect();
        recognizer.cases = order.into_iter().filter_map(|i| cases[i].take()).collect();
    }
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    for diagnostic in recognizer.diagnostics {
        if !diagnostics.contains(&diagnostic) {
            diagnostics.push(diagnostic);
        }
    }
    diagnostics.sort_by_key(|d| d.span.as_ref().map_or(0, |s| s.start));
    let recognition = Recognition {
        cases: recognizer.cases,
        refine: None,
        diagnostics,
    };
    (recognition, Refinement::of(recognizer.refine))
}

/// A class body's namespace, and the class body its statement sits in.
struct ClassInfo<'a> {
    def: &'a StmtClassDef,
    ns: Namespace<'a>,
    site: Option<&'a StmtClassDef>,
}

fn class_namespaces<'a>(
    body: &'a [Stmt],
    site: Option<&'a StmtClassDef>,
    out: &mut HashMap<TextSize, ClassInfo<'a>>,
) {
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class) => {
                out.insert(
                    class.start(),
                    ClassInfo {
                        def: class,
                        ns: Namespace::build(&class.body, PYTEST),
                        site,
                    },
                );
                class_namespaces(&class.body, Some(class), out);
            }
            Stmt::If(s) => {
                class_namespaces(&s.body, site, out);
                for clause in &s.elif_else_clauses {
                    class_namespaces(&clause.body, site, out);
                }
            }
            Stmt::Try(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
                class_namespaces(&s.finalbody, site, out);
                for handler in &s.handlers {
                    if let Some(handler) = handler.as_except_handler() {
                        class_namespaces(&handler.body, site, out);
                    }
                }
            }
            Stmt::With(s) => class_namespaces(&s.body, site, out),
            Stmt::For(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::While(s) => {
                class_namespaces(&s.body, site, out);
                class_namespaces(&s.orelse, site, out);
            }
            Stmt::Match(s) => {
                for case in &s.cases {
                    class_namespaces(&case.body, site, out);
                }
            }
            _ => {}
        }
    }
}

struct ModuleRecognizer<'s, 'a> {
    settings: &'s Settings,
    scope: &'s SupportScope,
    imports: &'s dyn Imports,
    file: &'s SourceFile<'a>,
    module: &'s Namespace<'a>,
    classes: &'s HashMap<TextSize, ClassInfo<'a>>,
    /// Stands in for a class body that was not walked (never expected).
    empty: &'s Namespace<'a>,
    path_keywords: &'s [String],
    cases: Vec<Case>,
    /// Each case's class- and module-scoped parameters, for `reorder_items`.
    keys: Vec<Vec<ParamKey>>,
    /// [`ModuleRecognizer::body_facts`], by class (`None` for the module).
    facts: std::cell::RefCell<HashMap<Option<TextSize>, std::rc::Rc<BodyFacts<'a>>>>,
    diagnostics: Vec<Diagnostic>,
    /// Why the file is refined, as found: a reason may come more than once
    /// ([`Refinement::of`] keeps each once), and [`Self::trial`] drops those
    /// a trial found.
    refine: Vec<String>,
    module_marks: Vec<Mark<'s, 'a>>,
    module_marks_unknown: bool,
    /// Fixtures defined in the module, in order.
    module_fixtures: Vec<FixtureDef>,
    /// Fixtures defined in each class body, by class.
    class_fixtures: HashMap<TextSize, Vec<FixtureDef>>,
    /// A reason that holds for the whole module was found: none of its
    /// cases is exact.
    module_wide: bool,
    /// Module names whose tests may not be what the source shows (bound
    /// or deleted through `global`): their cases are partial.
    partial_names: HashSet<String>,
    /// Some class of the module defines `__set_name__`, which Python calls
    /// with the class that holds an instance as an attribute.
    sets_names: bool,
}

impl<'s, 'a> ModuleRecognizer<'s, 'a> {
    fn lookup(&self, class: Option<&'a StmtClassDef>, at: TextSize) -> Lookup<'s, 'a> {
        Lookup {
            module: self.module,
            class: class.map(|c| self.class_ns(c)),
            source: self.file.source,
            at,
        }
    }

    fn class_ns(&self, class: &StmtClassDef) -> &'s Namespace<'a> {
        self.classes
            .get(&class.start())
            .map_or(self.empty, |info| &info.ns)
    }

    fn end(&self) -> TextSize {
        TextSize::try_from(self.file.source.len()).unwrap_or(TextSize::new(u32::MAX))
    }

    /// Whether some hook may add marks to this file's tests.
    fn marking_hooks(&self) -> bool {
        MARKING_HOOKS
            .iter()
            .any(|hook| self.scope.hooks.contains(*hook))
            || self
                .scope
                .global_hooks
                .contains("pytest_collection_modifyitems")
    }

    /// Whether a name bound to `path` may be a unittest case of the
    /// project, which pytest collects whatever its name: a capitalized
    /// name (as classes are) from a project file, unless unittest is off.
    fn may_be_project_unittest(&self, name: &str, path: &str) -> bool {
        !self.settings.unittest_blocked
            && name.chars().next().is_some_and(char::is_uppercase)
            && self.imports.project_unittest(path)
    }

    fn is_test_function_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_functions, name, SETS)
    }

    fn is_test_class_name(&self, name: &str) -> bool {
        matches_prefix_or_glob(&self.settings.python_classes, name, SETS)
    }

    /// What a module (`None`) or class body may skip at runtime, worked out
    /// once per body.
    fn body_facts(&self, site: Option<&'a StmtClassDef>) -> std::rc::Rc<BodyFacts<'a>> {
        let key = site.map(Ranged::start);
        if let Some(found) = self.facts.borrow().get(&key) {
            return std::rc::Rc::clone(found);
        }
        let body = site.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let facts = std::rc::Rc::new(BodyFacts {
            deletes: conditional_deletes(body),
            guarded: guarded_by_try(body),
        });
        self.facts
            .borrow_mut()
            .insert(key, std::rc::Rc::clone(&facts));
        facts
    }

    fn doubt(&mut self, reason: impl Into<String>) {
        self.refine.push(reason.into());
    }

    fn diagnostic(
        &mut self,
        severity: Severity,
        code: DiagnosticCode,
        range: TextRange,
        message: String,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            code,
            path: self.file.rel.to_project_path().into(),
            span: Some(self.file.span(range)),
            message,
        });
    }

    fn nodeid(&self, classes: &[String]) -> String {
        let mut id = self.file.rel.to_string();
        for class in classes {
            id.push_str("::");
            id.push_str(class);
        }
        id
    }

    /// Fixtures defined directly in a namespace, as [`fixture_of`] reads
    /// them (alternative branches, call forms and aliases included).
    fn fixtures_in(&self, ns: &Namespace<'a>, class: Option<&'a StmtClassDef>) -> Vec<FixtureDef> {
        let lookup = self.lookup(class, self.end());
        let body = class.map_or(&self.file.module.body[..], |c| &c.body[..]);
        let guarded = guarded_by_try(body);
        ns.iter()
            .filter_map(|(name, _)| fixture_of(lookup, ns, name, class.is_some(), &guarded))
            .collect()
    }
}
