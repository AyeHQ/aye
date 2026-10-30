//! What a support module's code changes in pytest's configuration that
//! collection reads.
//!
//! Decided (fourth round: configuration, conftests, fixtures and imports),
//! October 2026:
//! - **A support module that changes collection's ini values refines the
//!   files below it.** pytest calls a conftest's `pytest_configure` when it
//!   registers it, before collecting the files below it, so what its code
//!   (any hook, or the module itself) does to `config` holds there:
//!   `addinivalue_line` of `python_functions`, `python_classes`,
//!   `usefixtures`, `collect_imported_tests`, `empty_parameter_set_mark`,
//!   `strict`, `strict_parametrization_ids`, the id-escaping option or
//!   `consider_namespace_packages` (or of a name aye can't read), a
//!   mutation of `config.getini(…)` of one of them, a write to the ini
//!   cache (`_inicache`, `inicfg`), or to `config.option`'s `ignore`,
//!   `ignore_glob`, `deselect`, `pyargs`, `importmode`, `noconftest`,
//!   `collect_in_virtualenv`, `confcutdir` (`setattr` included). Those
//!   files are refined and their static cases are `Partial`. A literal
//!   `python_files` pattern is read instead: files below the conftest that
//!   match it are test modules too (`claim`). Known gaps: a conftest that
//!   pytest loads earlier but that is not among the file's (a sibling
//!   directory's, collected first), a computed `python_files` pattern
//!   (which can't claim files) or one a plugin the conftest loads adds,
//!   `norecursedirs` additions, and a hook imported from another module.

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{Expr, ModModule, Stmt};
use aye_python_semantic::resolve::Lookup;

/// What a support module's code changes in pytest's configuration that
/// collection reads while it walks: `config.addinivalue_line(…)`, writes
/// to `config.option` or to the ini values themselves. pytest calls a
/// conftest's `pytest_configure` as soon as it registers it, so the change
/// holds for the files collected after it: those below the conftest, at
/// least.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IniChanges {
    /// `python_files` patterns it adds, as literals: the files they match
    /// are test modules too.
    pub python_files: Vec<String>,
    /// Why the tests below it may differ from the static reading: a change
    /// to another value collection reads (`python_functions`,
    /// `python_classes`, `usefixtures`, …), to `python_files` with a value
    /// aye can't read, to an option that decides which files are collected
    /// (`config.option.ignore`, …), or one aye can't name.
    pub collection: Option<String>,
}

/// Ini values collection reads, whose change a conftest makes at runtime
/// may change which tests exist or their ids. `python_files` additions
/// are read when literal; `markers` and the rest only describe or report.
const COLLECTION_INI: [&str; 10] = [
    "python_files",
    "python_classes",
    "python_functions",
    "usefixtures",
    "collect_imported_tests",
    "empty_parameter_set_mark",
    "strict",
    "strict_parametrization_ids",
    "disable_test_id_escaping_and_forfeit_all_rights_to_community_support",
    "consider_namespace_packages",
];

/// `config.option` attributes that decide which files pytest collects and
/// how it imports them.
const COLLECTION_OPTIONS: [&str; 9] = [
    "ignore",
    "ignore_glob",
    "deselect",
    "pyargs",
    "importmode",
    "noconftest",
    "collect_in_virtualenv",
    "confcutdir",
    "python_files",
];

/// The visitor behind [`IniChanges`].
struct IniVisitor<'s, 'a> {
    lookup: Lookup<'s, 'a>,
    out: IniChanges,
}

impl IniVisitor<'_, '_> {
    fn changes(&mut self, what: String) {
        if self.out.collection.is_none() {
            self.out.collection = Some(what);
        }
    }

    /// A store into `target`: `config.option.NAME = …`, `….option.NAME +=
    /// …`, `config._inicache[…] = …`.
    fn store(&mut self, target: &Expr) {
        match target {
            Expr::Attribute(attr) => {
                if is_option(&attr.value) && COLLECTION_OPTIONS.contains(&attr.attr.id.as_str()) {
                    self.changes(format!("config.option.{}", attr.attr.id));
                } else if INI_STORES.contains(&attr.attr.id.as_str()) {
                    self.changes(format!("config.{}", attr.attr.id));
                } else if attr.attr.id.as_str() == "option" && is_config(&attr.value) {
                    self.changes("config.option".to_string());
                }
            }
            Expr::Subscript(sub) => self.store(&sub.value),
            Expr::Tuple(tuple) => tuple.elts.iter().for_each(|e| self.store(e)),
            Expr::List(list) => list.elts.iter().for_each(|e| self.store(e)),
            _ => {}
        }
    }
}

/// Where pytest keeps ini values: writing there changes them.
const INI_STORES: [&str; 4] = ["_inicache", "inicfg", "_inicfg", "_override_ini"];

/// `….option` of a pytest config (`config.option`,
/// `request.config.option`, `pytestconfig.option`): of a name or attribute
/// that says `config`.
fn is_option(expr: &Expr) -> bool {
    matches!(expr, Expr::Attribute(attr) if attr.attr.id.as_str() == "option" && is_config(&attr.value))
}

/// A name or attribute that says `config`.
fn is_config(expr: &Expr) -> bool {
    let name = match expr {
        Expr::Name(name) => name.id.as_str(),
        Expr::Attribute(attr) => attr.attr.id.as_str(),
        _ => return false,
    };
    name.to_ascii_lowercase().contains("config")
}

impl<'a> Visitor<'a> for IniVisitor<'_, 'a> {
    fn visit_stmt(&mut self, stmt: &'a Stmt) {
        match stmt {
            Stmt::Assign(assign) => assign.targets.iter().for_each(|t| self.store(t)),
            Stmt::AugAssign(assign) => self.store(&assign.target),
            Stmt::AnnAssign(assign) if assign.value.is_some() => self.store(&assign.target),
            Stmt::Delete(delete) => delete.targets.iter().for_each(|t| self.store(t)),
            _ => {}
        }
        visitor::walk_stmt(self, stmt);
    }

    fn visit_expr(&mut self, expr: &'a Expr) {
        if let Expr::Call(call) = expr
            && let Expr::Attribute(method) = &*call.func
        {
            let first = call.arguments.args.first();
            match method.attr.id.as_str() {
                "addinivalue_line" => match first.and_then(|arg| self.lookup.string(arg)) {
                    Some(name) if name == "python_files" => {
                        match call
                            .arguments
                            .args
                            .get(1)
                            .and_then(|v| self.lookup.string(v))
                        {
                            Some(pattern) => self.out.python_files.push(pattern),
                            None => self.changes("python_files".to_string()),
                        }
                    }
                    Some(name) if COLLECTION_INI.contains(&name.as_str()) => self.changes(name),
                    Some(_) => {}
                    None => self.changes("an ini value it computes".to_string()),
                },
                name if MUTATORS.contains(&name) => match &*method.value {
                    // `config.getini("python_functions").append(…)`: the
                    // list is the cached value itself.
                    Expr::Call(inner) if matches!(&*inner.func, Expr::Attribute(a) if a.attr.id.as_str() == "getini") => {
                        match inner
                            .arguments
                            .args
                            .first()
                            .and_then(|a| self.lookup.string(a))
                        {
                            Some(ini) if !COLLECTION_INI.contains(&ini.as_str()) => {}
                            Some(ini) => self.changes(ini),
                            None => self.changes("an ini value it computes".to_string()),
                        }
                    }
                    // `config.option.ignore.append(…)`.
                    Expr::Attribute(attr)
                        if is_option(&attr.value)
                            && COLLECTION_OPTIONS.contains(&attr.attr.id.as_str()) =>
                    {
                        self.changes(format!("config.option.{}", attr.attr.id));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        if let Expr::Call(call) = expr
            && let Expr::Name(func) = &*call.func
            && matches!(func.id.as_str(), "setattr" | "delattr")
            && let Some(target) = call.arguments.args.first()
        {
            let name = call
                .arguments
                .args
                .get(1)
                .and_then(|n| self.lookup.string(n));
            if is_option(target) {
                match name {
                    Some(name) if !COLLECTION_OPTIONS.contains(&name.as_str()) => {}
                    Some(name) => self.changes(format!("config.option.{name}")),
                    None => self.changes("config.option".to_string()),
                }
            } else if name.is_none_or(|n| INI_STORES.contains(&n.as_str())) {
                // `setattr(config, name, …)` may reach the ini values.
                if is_config(target) {
                    self.changes("config attributes it computes".to_string());
                }
            }
        }
        visitor::walk_expr(self, expr);
    }
}

/// Methods that change a list in place.
const MUTATORS: [&str; 6] = [
    "append",
    "extend",
    "insert",
    "remove",
    "clear",
    "__setitem__",
];

/// [`IniChanges`] of a module: what any of its code may change, whichever
/// hook runs it.
pub(super) fn ini_changes<'a>(module: &'a ModModule, lookup: Lookup<'_, 'a>) -> IniChanges {
    let mut visitor = IniVisitor {
        lookup,
        out: IniChanges::default(),
    };
    for stmt in &module.body {
        visitor.visit_stmt(stmt);
    }
    visitor.out
}
