//! A Python module's doctests: the name pytest imports it under, the
//! tests the finder sees in it, and why the file is refined.

use std::path::Path;

use aye_adapter::{Recognition, SourceFile};
use aye_events::{AffinityKey, Diagnostic, DiagnosticCode, Severity};
use aye_pytest::config::ImportMode;
use aye_pytest::pathlib::resolve_pkg_root_and_module_name;
use aye_python_stdlib::fnmatch;

use crate::{DoctestRecognizer, SETS, finder};

/// Why a module with no doctest aye can see is refined.
pub(crate) const NO_STATIC_DOCTESTS: &str =
    "no doctest is visible statically; pytest imports the module all the same";

/// Whether importing a module surely succeeds: it holds nothing but
/// constants (a docstring, `...`) and `pass`, as most packages'
/// `__init__.py` files do (or nothing at all).
fn surely_imports(module: &aye_adapter::ast::ModModule) -> bool {
    use aye_adapter::ast::{Expr, Stmt};
    module.body.iter().all(|stmt| match stmt {
        Stmt::Pass(_) => true,
        Stmt::Expr(expr) => matches!(
            expr.value.as_ref(),
            Expr::StringLiteral(_)
                | Expr::BytesLiteral(_)
                | Expr::NumberLiteral(_)
                | Expr::BooleanLiteral(_)
                | Expr::NoneLiteral(_)
                | Expr::EllipsisLiteral(_)
        ),
        _ => false,
    })
}

impl DoctestRecognizer {
    /// Whether `from … import` with `level` and `module` reads a project
    /// module: any relative import, or a top-level name found as a package
    /// or module where imports look in the project.
    fn imports_project(&self, level: u32, module: Option<&str>, pkg_root: &Path) -> bool {
        if level > 0 {
            return true;
        }
        let Some(top) = module.and_then(|m| m.split('.').next()) else {
            return false;
        };
        let mut bases = vec![
            pkg_root.to_path_buf(),
            self.root.clone(),
            self.root.join("src"),
            self.config.rootdir.clone(),
        ];
        bases.extend(self.config.pythonpath.iter().cloned());
        bases
            .iter()
            .any(|base| base.join(top).is_dir() || base.join(format!("{top}.py")).is_file())
    }

    /// Whether the pytest adapter, beside this one, imports `file` and so
    /// reports its import error: a `conftest.py`, or a module matching
    /// pytest's `python_files`.
    fn pytest_adapter_imports(&self, file: &Path) -> bool {
        self.config.beside_pytest
            && (file.file_name().is_some_and(|n| n == "conftest.py")
                || self
                    .config
                    .python_files
                    .iter()
                    .any(|glob| fnmatch::fnmatch_ex(glob, file, SETS)))
    }

    pub(crate) fn recognize_module(&self, file: &SourceFile<'_>) -> Recognition {
        let mut recognition = Recognition::default();
        let mut reasons: Vec<String> = Vec::new();
        if self.stripped {
            reasons.push("PYTHONOPTIMIZE=2 strips docstrings".into());
        }
        if self.config.import_mode == ImportMode::Importlib {
            reasons.push("pytest's importlib import mode names the module".into());
        }
        if self.config.consider_namespace_packages {
            reasons.push("consider_namespace_packages: the import names the module".into());
        }
        let (name, pkg_root) = match resolve_pkg_root_and_module_name(file.abs) {
            Ok(found) => found,
            Err(why) => {
                reasons.push(why);
                let stem = file
                    .abs
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (stem, file.abs.parent().unwrap_or(file.abs).to_path_buf())
            }
        };
        if self.ignore_import_mismatch {
            reasons.push(format!(
                "PY_IGNORE_IMPORTMISMATCH=1: pytest takes whichever module it finds as {name:?}, \
                 maybe another file's"
            ));
        }
        if file.abs.file_name().is_some_and(|n| n == "conftest.py")
            && let Some(other) = self.conftest_mismatch(file.abs)
        {
            // pytest finds the other one under `conftest`: its import file
            // mismatch, a collection error.
            recognition.diagnostics.push(Diagnostic {
                severity: Severity::Error,
                code: DiagnosticCode::CollectionError,
                path: file.rel.to_project_path().into(),
                span: None,
                message: format!(
                    "import file mismatch:\nimported module 'conftest' has this __file__ \
                     attribute:\n  {}\nwhich is not the same as the test file we want to \
                     collect:\n  {}\nHINT: remove __pycache__ / .pyc files and/or use a unique \
                     basename for your test file modules",
                    other.display(),
                    file.abs.display()
                ),
            });
            return recognition;
        }
        let finding = finder::Finder::new(file).find(&name);
        if let Some(why) = finding.refine {
            reasons.push(why);
        }
        if finding
            .from_imports
            .iter()
            .any(|(level, module)| self.imports_project(*level, module.as_deref(), &pkg_root))
            && self.may_be_claimed(&name)
        {
            reasons.push(format!(
                "it imports from project modules, and a project file assigns `__module__` \
                 (maybe {name:?})"
            ));
        }
        if !finding.tests.is_empty()
            && let Some(why) = self.skipping_conftest(file.abs)
        {
            reasons.push(why);
        }
        if finding.tests.is_empty()
            && reasons.is_empty()
            && !surely_imports(file.module)
            && !self.pytest_adapter_imports(file.abs)
        {
            // pytest imports every module all the same: an import error is
            // its collection error.
            reasons.push(NO_STATIC_DOCTESTS.to_string());
        }
        let affinity = vec![AffinityKey::import(&name), AffinityKey::file(file.rel)];
        for test in finding.tests {
            let span = file.span(test.range);
            let case = self.case(file, &test.name, span, affinity.clone());
            recognition.cases.push(case);
        }
        if !reasons.is_empty() {
            recognition.refine = Some(reasons.join("; ").into());
        }
        recognition
    }
}
