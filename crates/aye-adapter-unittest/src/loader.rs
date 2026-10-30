//! What unittest's loader takes from a module: each class's tests in loader
//! order ([`Loaded`]), found along its MRO, or why only an import can tell.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use aye_adapter::TextRange;
use aye_python_semantic::c3::{InconsistentMro, linearize};
use aye_python_stdlib::unittest::{TEST_METHOD_PREFIX, looks_like_tests};

use crate::module::{Base, Binding, Member, Summary, is_dunder};
use crate::project::{MAX_DEPTH, Modules, Target};

/// A test of a class: its name and where its method is defined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Method {
    pub name: String,
    pub file: PathBuf,
    pub range: TextRange,
    pub line: u32,
}

/// One class's tests, as the loader takes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Loaded {
    /// The class's `__qualname__`.
    pub class: String,
    /// The name the loader found it under in the module's namespace (an
    /// alias's, or an imported class's local name): the test ids' class.
    pub name: String,
    pub methods: Vec<Method>,
    /// The project classes of its MRO, by file and index.
    pub mro: Vec<(PathBuf, usize)>,
}

/// A class of an MRO.
#[derive(Clone)]
enum Node {
    /// A project class: its module's summary and its position there.
    Class(Arc<Summary>, usize),
    TestCase,
    Object,
    Plain(String),
}

impl PartialEq for Node {
    /// A project class is its file's class at its position, whichever read
    /// of the file found it: the test module as collection parsed it, or the
    /// project's read of it, which two threads may each have made.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Class(a, i), Self::Class(b, j)) => a.path == b.path && i == j,
            (Self::TestCase, Self::TestCase) | (Self::Object, Self::Object) => true,
            (Self::Plain(a), Self::Plain(b)) => a == b,
            _ => false,
        }
    }
}

impl Modules {
    /// The tests `loadTestsFromModule` takes from `summary` (the module's
    /// own file), in its order: by name in the namespace, then by method
    /// name; or why only an import can tell.
    pub(crate) fn loaded(&self, summary: &Arc<Summary>) -> Result<Vec<Loaded>, String> {
        if let Some(reason) = &summary.dynamic {
            return Err(reason.clone());
        }
        let mut out = Vec::new();
        for (name, binding) in &summary.names {
            if let Some(mut loaded) = self.binding_tests(summary, name, binding)? {
                loaded.name.clone_from(name);
                out.push(loaded);
            }
        }
        Ok(out)
    }

    fn binding_tests(
        &self,
        summary: &Arc<Summary>,
        name: &str,
        binding: &Binding,
    ) -> Result<Option<Loaded>, String> {
        match binding {
            Binding::Class(index) => self.class_tests(summary, *index, 0),
            Binding::Import(path) => match self.target(path, &summary.path, 0) {
                Target::Class(other, index) => self.class_tests(&other, index, 0),
                Target::Binding(other, binding) => self.binding_tests(&other, name, &binding),
                Target::External(path) if looks_like_tests(name, &path) => Err(format!(
                    "{name} is imported from {path}, which aye doesn't read, and may hold tests"
                )),
                Target::Module | Target::Stdlib(_) | Target::External(_) => Ok(None),
                Target::Unknown(reason) => Err(format!("{name} may be a test class: {reason}")),
            },
            Binding::Returned(path) => match self.target(path, &summary.path, 0) {
                Target::Binding(_, Binding::Function(info)) if !info.may_return_class => Ok(None),
                Target::Class(other, index) if !other.classes[index].metaclass => Ok(None),
                Target::Stdlib(p) if !p.ends_with("new_class") => Ok(None),
                Target::External(_) => Ok(None),
                _ => Err(format!(
                    "{name} is the result of {path}(), which may be a class"
                )),
            },
            Binding::Unknown(reason) => {
                if is_dunder(name) {
                    Ok(None)
                } else {
                    Err(format!("{name} may be a test class: {reason}"))
                }
            }
            Binding::Maybe(options) => {
                for option in options {
                    match self.binding_tests(summary, name, option) {
                        Ok(None) => {}
                        Ok(Some(_)) | Err(_) => {
                            return Err(format!("{name} is bound in a branch that may not run"));
                        }
                    }
                }
                Ok(None)
            }
            Binding::Function(_)
            | Binding::Module(_)
            | Binding::Constant
            | Binding::Plain
            | Binding::Unbound => Ok(None),
        }
    }

    /// A class's MRO, as C3 linearizes it; external classes count for
    /// themselves and `object`.
    fn mro(&self, summary: &Arc<Summary>, index: usize, depth: usize) -> Result<Vec<Node>, String> {
        if depth > MAX_DEPTH {
            return Err("classes nest too deep".into());
        }
        let class = &summary.classes[index];
        if let Some(reason) = &class.dynamic {
            return Err(reason.clone());
        }
        let bases = class.bases.clone()?;
        let mut sequences: Vec<Vec<Node>> = Vec::new();
        let mut direct = Vec::new();
        for base in &bases {
            let (head, sequence) = match base {
                Base::Local(j) => (
                    Node::Class(Arc::clone(summary), *j),
                    self.mro(summary, *j, depth + 1)?,
                ),
                Base::TestCase => (Node::TestCase, vec![Node::TestCase, Node::Object]),
                Base::Object => (Node::Object, vec![Node::Object]),
                Base::Plain(path) => (
                    Node::Plain(path.clone()),
                    vec![Node::Plain(path.clone()), Node::Object],
                ),
                Base::Path(path) => match self.target(path, &summary.path, depth + 1) {
                    Target::Class(other, j) => (
                        Node::Class(Arc::clone(&other), j),
                        self.mro(&other, j, depth + 1)?,
                    ),
                    Target::Stdlib(path) => (
                        Node::Plain(path.clone()),
                        vec![Node::Plain(path), Node::Object],
                    ),
                    Target::External(path) => {
                        return Err(format!(
                            "{} inherits from {path}, which aye doesn't read",
                            class.name
                        ));
                    }
                    Target::Module | Target::Binding(..) => {
                        return Err(format!(
                            "{} inherits from {path}, which is not a class",
                            class.name
                        ));
                    }
                    Target::Unknown(reason) => {
                        return Err(format!("{} inherits from {path}: {reason}", class.name));
                    }
                },
            };
            direct.push(head);
            sequences.push(sequence);
        }
        if direct.is_empty() {
            direct.push(Node::Object);
            sequences.push(vec![Node::Object]);
        }
        sequences.push(direct);
        linearize(Node::Class(Arc::clone(summary), index), sequences).map_err(|InconsistentMro| {
            format!(
                "{} has an inconsistent MRO; Python refuses to make it",
                class.name
            )
        })
    }

    /// [`Modules::class_tests`] of a class of `summary`, for a static guess.
    pub(crate) fn class_tests_for_guess(
        &self,
        summary: &Arc<Summary>,
        index: usize,
    ) -> Result<Option<Loaded>, String> {
        self.class_tests(summary, index, 0)
    }

    /// The tests of a class, when it is a `TestCase`: callable `test*`
    /// attributes along its MRO (the first binding of a name wins), sorted;
    /// with none, `runTest` when it has one.
    fn class_tests(
        &self,
        summary: &Arc<Summary>,
        index: usize,
        depth: usize,
    ) -> Result<Option<Loaded>, String> {
        let mro = self.mro(summary, index, depth)?;
        if !mro.contains(&Node::TestCase) {
            return Ok(None);
        }
        let class = &summary.classes[index];
        let mut seen: HashSet<&str> = HashSet::new();
        let mut methods = Vec::new();
        let mut run_test = None;
        for (position, node) in mro.iter().enumerate() {
            let Node::Class(owner, j) = node else {
                continue;
            };
            let defining = &owner.classes[*j];
            if position > 0 && defining.init_subclass {
                return Err(format!(
                    "{} defines __init_subclass__, which may change {}",
                    defining.name, class.name
                ));
            }
            for (name, member) in &defining.members {
                if !seen.insert(name.as_str()) {
                    continue;
                }
                let is_test = name.starts_with(TEST_METHOD_PREFIX);
                if !is_test && name != "runTest" {
                    continue;
                }
                let located = |range: TextRange, line: u32| Method {
                    name: name.clone(),
                    file: owner.path.clone(),
                    range,
                    line,
                };
                let found = match member {
                    Member::Callable { range, line } => Some(located(*range, *line)),
                    Member::NotCallable => None,
                    Member::Import(path) => match self.target(path, &owner.path, depth + 1) {
                        Target::Class(..) | Target::Binding(_, Binding::Function(_)) => {
                            Some(located(defining.range, defining.line))
                        }
                        Target::Binding(_, Binding::Constant) | Target::Module => None,
                        _ => {
                            return Err(format!(
                                "{}.{name} is imported, and may or may not be callable",
                                defining.name
                            ));
                        }
                    },
                    Member::Unknown(reason) => return Err(reason.clone()),
                };
                if name == "runTest" {
                    // `hasattr`: any binding counts.
                    run_test =
                        Some(found.unwrap_or_else(|| located(defining.range, defining.line)));
                } else if let Some(method) = found {
                    methods.push(method);
                }
            }
        }
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        if methods.is_empty()
            && let Some(run) = run_test
        {
            methods.push(run);
        }
        let mro = mro
            .iter()
            .filter_map(|node| match node {
                Node::Class(owner, j) => Some((owner.path.clone(), *j)),
                _ => None,
            })
            .collect();
        Ok(Some(Loaded {
            class: class.name.clone(),
            name: class.name.clone(),
            methods,
            mro,
        }))
    }
}
