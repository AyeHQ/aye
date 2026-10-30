//! What a Python module binds, read statically on Ruff's AST: what aye's
//! adapters ask of a module's source without importing it.
//!
//! A name's value, the class a base names, the module an import finds:
//! each answer holds only when nothing that runs while the module is
//! imported can change it, so every reading is conservative, and says
//! when it can't tell.
//!
//! Owns: namespaces and the escape analysis over them, names resolved
//! through imports, static values, classes and their C3 linearization,
//! decorator and import-time facts, `sys.path` changes and module
//! resolution. Does not own: the standard library's behaviour
//! (aye-python-stdlib), nor a framework's rules: what a framework does with
//! a module (pytest's marks, hooks and skips) is a parameter.
//!
//! Depends on no aye crate. Knows no framework but the standard library's
//! own unittest; only the adapters depend on it.
