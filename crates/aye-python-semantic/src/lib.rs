//! What a Python module binds, read statically on Ruff's AST: what aye's
//! adapters ask of a module's source without importing it.
//!
//! A name's value, the class a base names, the module an import finds:
//! each answer holds only when nothing that runs while the module is
//! imported can change it, so every reading is conservative, and says
//! when it can't tell.
//!
//! - [`namespace`]: a module's or class body's namespace in dict order,
//!   each name's bindings with the branches they sit in, and the escape
//!   analysis's verdicts on it.
//! - [`resolve`]: names and attribute chains resolved through imports and
//!   the builtins, where an expression is evaluated.
//! - [`eval`]: static values, and which of them building surely succeeds.
//! - [`flow`]: generators, exclusive bindings, what may skip or raise
//!   while a module is imported, and the statements that may not run.
//! - [`class`]: classes: their bases, known classes from elsewhere, and
//!   names as a class body stores them.
//! - [`decorator`]: whether a decorator hands back what it decorates.
//!
//! Owns: namespaces and the escape analysis over them, names resolved
//! through imports, static values, classes and their C3 linearization,
//! decorator and import-time facts, `sys.path` changes and module
//! resolution. Does not own: the standard library's behaviour
//! (aye-python-stdlib), nor a framework's rules: what a framework does with
//! a module (pytest's marks, hooks and skips) is a parameter.
//!
//! Depends on: aye-python-stdlib (the builtins, `re`, reprs), and Ruff's
//! AST. Knows no framework but the standard library's own unittest; only
//! the adapters depend on it.

pub mod class;
pub mod decorator;
mod escape;
pub mod eval;
pub mod flow;
mod hash;
pub mod namespace;
pub mod resolve;
