//! pytest 9.1.1 as a library, for the adapters that need it: what pytest
//! itself decides about a run before it imports a test, read the way pytest
//! reads it, without running it.
//!
//! Owns: pytest's configuration discovery (rootdir and inifile, after
//! `_pytest/config/findpaths.py`) and its ini values, pytest's command line
//! as argparse reads it and what aye lifts out of it, pytest's conftest
//! rules, its doctest plugin's file rule and its module names. Does not
//! own: what a module binds (aye-python-semantic), the standard library's
//! behaviour (aye-python-stdlib), nor what an adapter predicts.
//!
//! Only the adapters and the binary depend on it.

pub mod argv;
pub mod config;
pub mod conftest;
pub mod doctest;
pub mod env;
pub mod getini;
pub mod ini;
pub mod names;
pub mod pathlib;
