//! Names resolved where an expression is evaluated: through the class
//! body being run and the module, their imports and the builtins, to a
//! dotted path or to a binding of the file.

use aye_python_stdlib::builtins::builtin_name;
use ruff_python_ast::Expr;
use ruff_text_size::TextSize;

use crate::namespace::{Binding, Bound, Namespace};

/// What an expression refers to.
#[derive(Debug, Clone)]
pub enum Resolved<'a> {
    /// A dotted path through imports or builtins (`pytest.mark.slow`,
    /// `builtins.int`). Unbound names resolve to themselves.
    Path(String),
    /// A definition or value in this file.
    Local(Bound<'a>),
    Unknown,
}

/// The namespaces a name is looked up in at some point of the source: the
/// class body being run, if any, then the module. (Class bodies do not
/// nest for lookup.)
#[derive(Clone, Copy)]
pub struct Lookup<'s, 'a> {
    pub module: &'s Namespace<'a>,
    pub class: Option<&'s Namespace<'a>>,
    pub source: &'a str,
    /// Where the expression is evaluated.
    pub at: TextSize,
}

pub(crate) const DEPTH: u8 = 8;

impl<'s, 'a> Lookup<'s, 'a> {
    #[must_use]
    pub fn at(self, at: TextSize) -> Self {
        Self { at, ..self }
    }

    pub(crate) fn scopes(&self) -> impl Iterator<Item = &'s Namespace<'a>> {
        self.class.into_iter().chain(std::iter::once(self.module))
    }

    /// The binding a name has at `self.at`, if it is bound in the file.
    pub fn binding(&self, name: &str) -> Option<Bound<'a>> {
        for ns in self.scopes() {
            if let Some(bound) = ns.get(name) {
                return Some(bound.clone());
            }
        }
        None
    }

    /// Whether a name may come from a star import.
    pub fn maybe_star(&self) -> bool {
        self.scopes().any(|ns| !ns.star_imports.is_empty())
    }

    /// Resolve a name or attribute chain.
    pub fn resolve(&self, expr: &'a Expr) -> Resolved<'a> {
        self.resolve_depth(expr, 0)
    }

    fn resolve_depth(&self, expr: &'a Expr, depth: u8) -> Resolved<'a> {
        if depth > DEPTH {
            return Resolved::Unknown;
        }
        match expr {
            Expr::Name(name) => {
                let id = name.id.as_str();
                match self.binding(id) {
                    Some(Bound {
                        binding: Binding::Import(path),
                        ..
                    }) => Resolved::Path(path),
                    Some(
                        bound @ Bound {
                            binding: Binding::Value(value),
                            ..
                        },
                    ) => match value {
                        Expr::Name(_) | Expr::Attribute(_) => {
                            match self.resolve_depth(value, depth + 1) {
                                Resolved::Unknown => Resolved::Local(bound),
                                other => other,
                            }
                        }
                        _ => Resolved::Local(bound),
                    },
                    Some(bound) => Resolved::Local(bound),
                    None if builtin_name(id).is_some() => Resolved::Path(format!("builtins.{id}")),
                    None if self.maybe_star() => Resolved::Unknown,
                    None => Resolved::Path(id.to_string()),
                }
            }
            Expr::Attribute(attr) => match self.resolve_depth(&attr.value, depth + 1) {
                Resolved::Path(path) => Resolved::Path(format!("{path}.{}", attr.attr.id)),
                _ => Resolved::Unknown,
            },
            _ => Resolved::Unknown,
        }
    }

    /// The dotted path of an expression, if it names something imported.
    pub fn path(&self, expr: &'a Expr) -> Option<String> {
        match self.resolve(expr) {
            Resolved::Path(path) => Some(path),
            _ => None,
        }
    }

    /// [`Lookup::path`] for evaluating values: `None` when what the path
    /// names may not be what its module defines, because code that runs
    /// may change the module (or one above it, or `builtins`), or a
    /// function may rebind the name (`global`).
    pub(crate) fn trusted_path(&self, expr: &'a Expr) -> Option<String> {
        let path = self.path(expr)?;
        let mut root = expr;
        while let Expr::Attribute(attr) = root {
            root = &attr.value;
        }
        if let Expr::Name(name) = root
            && self
                .scopes()
                .any(|ns| ns.globals.contains(name.id.as_str()))
        {
            return None;
        }
        (!self.module.untrusted(&path)).then_some(path)
    }
}
