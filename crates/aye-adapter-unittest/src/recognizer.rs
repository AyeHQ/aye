//! The unittest adapter's static face for one project
//! ([`UnittestRecognizer`]): its discovery starts, the cases it predicts
//! for a test module, and why a module is refined.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use aye_adapter::{
    AdapterError, Claim, Project, Recognition, Recognizer, RefineReason, Requested, Setting,
    SourceFile,
};
use aye_events::{AffinityKey, Case, Certainty, ProjectPath, ProjectPathRef, Span};
use aye_protocol::AdapterContext;
use aye_python_stdlib::fnmatch::fnmatch;
use aye_python_stdlib::os_path::normalize_absolute;
use aye_python_stdlib::unittest::valid_module_name;

use crate::context::UnittestContext;
use crate::discovery::{DiscoveryStart, Starts, is_package, may_hold_tests};
use crate::loader::Loaded;
use crate::module;
use crate::project::{Module, ModuleIndex};
use crate::{NAME, SETS, UnittestConfig};

/// Why a test module with no test aye can see is refined.
pub(crate) const NO_STATIC_TESTS: &str =
    "no test is visible statically; discovery imports the module to tell";

/// The unittest adapter's static face for one project.
#[derive(Debug)]
pub struct UnittestRecognizer {
    root: PathBuf,
    pub(crate) config: UnittestConfig,
    /// Where discovery starts.
    pub(crate) starts: Starts,
    modules: ModuleIndex,
    /// By package directory: whether its `__init__.py` may define
    /// `load_tests`.
    pub(crate) takes_over: Mutex<HashMap<PathBuf, bool>>,
    /// The Python files named on the command line: each is a module alone,
    /// as `python -m unittest path.py` loads it, even below a package that
    /// may define `load_tests`.
    pub(crate) named_files: Vec<PathBuf>,
    /// The context, built once.
    context: OnceLock<UnittestContext>,
}

impl UnittestRecognizer {
    pub fn new(project: &Project<'_>) -> Result<Self, AdapterError> {
        let root = normalize_absolute(project.root);
        let config = UnittestConfig::load(&root, project.tool_aye)?;
        let configured = DiscoveryStart {
            start: normalize_absolute(&config.start),
            top: normalize_absolute(&config.top),
        };
        let modules = ModuleIndex::new(&configured.top, &root);
        let starts = Starts::new(configured, project);
        let named_files = project
            .args
            .iter()
            .map(|arg| normalize_absolute(&project.cwd.join(arg)))
            .filter(|path| path.is_file() && path.extension().is_some_and(|e| e == "py"))
            .collect();
        Ok(Self {
            root,
            config,
            starts,
            modules,
            takes_over: Mutex::new(HashMap::new()),
            named_files,
            context: OnceLock::new(),
        })
    }

    /// The resolved configuration.
    pub fn config(&self) -> &UnittestConfig {
        &self.config
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root).map_or_else(
            |_| path.to_string_lossy().replace('\\', "/"),
            |rel| {
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/")
            },
        )
    }

    fn cases(
        &self,
        file: &SourceFile<'_>,
        loaded: &[Loaded],
        package: Option<ProjectPathRef<'_>>,
        certainty: Certainty,
    ) -> Vec<Case> {
        let mut cases = Vec::new();
        let parts: Vec<String> = file.rel.components().map(str::to_string).collect();
        for class in loaded {
            // The name the loader found the class under, as pytest names it.
            let class_names = [class.name.as_str()];
            let class_id = format!("{}::{}", file.rel, class.name);
            for method in &class.methods {
                let mut case = Case::new(format!("{class_id}::{}", method.name), NAME);
                case.span = Some(Span {
                    path: ProjectPath::as_written(self.relative(&method.file)),
                    start: method.range.start().to_u32(),
                    end: method.range.end().to_u32(),
                    line: Some(method.line),
                });
                let mut keywords = parts.clone();
                keywords.extend(class_names.iter().map(|n| (*n).to_string()));
                keywords.push(method.name.clone());
                let mut seen = std::collections::HashSet::new();
                keywords.retain(|k| seen.insert(k.clone()));
                case.keywords = keywords;
                case.affinity = package
                    .map(AffinityKey::package)
                    .into_iter()
                    .chain([
                        AffinityKey::file(file.rel),
                        AffinityKey::file_class(file.rel, &class_names),
                    ])
                    .collect();
                case.certainty = certainty;
                cases.push(case);
            }
        }
        cases
    }
}

impl Recognizer for UnittestRecognizer {
    /// The command-line paths; else the start directory.
    fn roots(&self, project: &Project<'_>) -> Vec<PathBuf> {
        if project.args.is_empty() {
            return vec![self.starts.configured.start.clone()];
        }
        project
            .args
            .iter()
            .map(|arg| project.cwd.join(arg))
            .collect()
    }

    /// Towards a start directory, and below one only into packages, as
    /// discovery recurses.
    fn descend(&self, dir: &Path) -> bool {
        let dir = normalize_absolute(dir);
        let mut inside = false;
        for from in self.starts.iter() {
            if from.start.starts_with(&dir) {
                return true;
            }
            inside |= dir.starts_with(&from.start);
        }
        inside && is_package(&dir)
    }

    /// A Python file named on the command line; else a module discovery
    /// would import: a valid module name matching the pattern below a start
    /// directory (or a named directory), or a package's `__init__.py`.
    fn claim(&self, rel: &str, requested: Requested) -> Claim {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        if Path::new(name).extension().is_none_or(|e| e != "py") {
            return Claim::Ignore;
        }
        if requested.is_named() {
            return Claim::Tests;
        }
        let abs = normalize_absolute(&self.root.join(rel));
        let Some(DiscoveryStart { start, top }) = self.starts.start_for(&abs) else {
            return Claim::Ignore;
        };
        if name == "__init__.py" {
            let dir = abs.parent().unwrap_or(&abs);
            if dir == top || !dir.starts_with(start) {
                return Claim::Ignore;
            }
            // Most are empty: claimed only when they may bind a class, so
            // that collection doesn't call each one a file without tests,
            // or when discovery fails to import them.
            return if may_hold_tests(&abs) || self.unimportable(&abs).is_some() {
                Claim::Tests
            } else {
                Claim::Ignore
            };
        }
        if valid_module_name(name) && fnmatch(name, &self.config.pattern.value, SETS) {
            Claim::Tests
        } else {
            Claim::Ignore
        }
    }

    fn recognize(
        &self,
        file: &SourceFile<'_>,
        _requested: Requested,
        _support: &[SourceFile<'_>],
    ) -> Recognition {
        let abs = normalize_absolute(file.abs);
        if let Some(package) = self.unimportable(&abs) {
            // Its `__init__.py` is refined, to report the import error.
            let refine = (abs == package.join("__init__.py")).then(|| {
                format!(
                    "{} has a dot in its name, so discovery can't import it",
                    self.relative(&package)
                )
            });
            return Recognition {
                refine: refine.map(RefineReason::from),
                ..Recognition::default()
            };
        }
        let test = self
            .modules
            .test_module(module::summarize(&abs, file.module, file.lines));
        // A file named on the command line is loaded alone, as `python -m
        // unittest path.py` loads it: no package's `load_tests` decides it.
        let package = if self.named_files.contains(&abs) {
            None
        } else {
            self.taken_over_by(&abs)
        };
        let package_rel = package.as_deref().map(|p| self.relative(p));
        // Without an import, the guess keeps the classes that are surely
        // test classes, for when refinement is off or fails.
        let (loaded, mut refine) = match self.modules.loaded(&test) {
            // What runs before the loader reads the classes may change them.
            Ok(loaded) => {
                let effect = self.modules.effects(&test, &loaded);
                (loaded, effect)
            }
            Err(reason) => (guess(&self.modules, &test), Some(reason)),
        };
        if let Some(package) = &package_rel {
            let reason = format!(
                "{package}/__init__.py may define load_tests, which decides the tests below it"
            );
            refine = Some(match refine {
                Some(existing) => format!("{reason}; {existing}"),
                None => reason,
            });
        }
        let certainty = if refine.is_some() {
            Certainty::Partial
        } else {
            Certainty::Exact
        };
        let cases = self.cases(
            file,
            &loaded,
            package_rel.as_deref().map(ProjectPathRef::as_written),
            certainty,
        );
        if cases.is_empty() && refine.is_none() {
            // Nothing to run statically, but discovery imports the module
            // all the same: its import error is unittest's error to report.
            refine = Some(NO_STATIC_TESTS.to_string());
        }
        Recognition {
            cases,
            refine: refine.map(RefineReason::from),
            diagnostics: Vec::new(),
        }
    }

    fn settings(&self) -> Vec<Setting> {
        self.config.settings()
    }

    /// What the executor needs to discover as unittest would:
    /// [`UnittestContext`].
    fn context(&self) -> AdapterContext {
        AdapterContext::of(self.context.get_or_init(|| UnittestContext::new(self)))
    }
}

/// The best static guess for a module only an import can tell: the
/// classes that are surely test classes, without what made it uncertain.
fn guess(modules: &ModuleIndex, test: &Module) -> Vec<Loaded> {
    let mut out = Vec::new();
    for (name, binding) in &test.summary.names {
        if let module::Binding::Class(index) = binding
            && let Ok(Some(mut loaded)) = modules.class_tests_for_guess(test, *index)
        {
            loaded.name.clone_from(name);
            out.push(loaded);
        }
    }
    out
}
