//! CPython's standard library behaviour, in Rust: what aye's adapters ask
//! of Python without running it.
//!
//! Each item names the CPython function or table it mirrors and the
//! versions it holds for: 3.10 to 3.14, the Pythons aye supports. Where
//! aye's adapters had copies of one function that differ, each copy's
//! behaviour is kept here, behind a name or a mode of its own, and each
//! adapter asks for the one it had.
//!
//! - [`fnmatch`]: `fnmatch.fnmatch`, with a pattern's `[...]` sets read as
//!   CPython reads them or as the pytest and doctest adapters always have
//!   ([`fnmatch::SetTranslation`]), and pytest's `fnmatch_ex` and name
//!   options, which both of those adapters match with.
//! - [`unittest`]: unittest's loader rules (`VALID_MODULE_NAME`).
//!
//! Owns: the standard library's behaviour, as the adapters read it. Does
//! not own: a framework's rules (pytest's are the pytest and doctest
//! adapters'), nor what a module binds (each adapter reads that itself).
//!
//! Depends on no aye crate. Knows no framework but the standard library's
//! own unittest; only the adapters depend on it.

pub mod fnmatch;
pub mod unittest;
