//! pytest 9.1.1 as a library, for the adapters that need it: what pytest
//! itself decides about a run before it imports a test, read the way pytest
//! reads it, without running it.
//!
//! - [`config`]: pytest's configuration, by one of two named rules
//!   ([`config::ConfigRules`]): pytest's own ([`config::PytestConfig`], the
//!   pytest adapter's) or the doctest adapter's approximation of them
//!   ([`config::approximate`]), each with the files pytest looks for, how
//!   it loads and locates them, and how it expands `testpaths`.
//! - [`ini`]: the INI reader pytest reads its files with (iniconfig's).
//! - [`getini`]: the raw ini values the front-end's `getini` coerces.
//! - [`env`](mod@env): what each adapter reads from the environment pytest, and
//!   Python under it, read.
//! - [`argv`]: pytest's command line as argparse reads it: what an
//!   argument list gives of an option ([`argv::ArgValue`]), read by one of
//!   two named sets of short options ([`argv::ShortOptions`]), the options
//!   that replace the test run, and those aye lifts out for the runner,
//!   for its own JUnit report and for the doctest adapter.
//! - [`conftest`]: which conftests pytest loads, and which first.
//! - [`doctest`]: pytest's doctest plugin: the files it collects, and the
//!   option flags and report styles it accepts.
//! - [`pathlib`]: where a file's packages end and the name pytest imports
//!   it under.
//! - [`session`]: what pytest's session doesn't collect, and `--deselect`
//!   in aye's ids.
//! - [`names`]: the two adapters' names, which each tests the run for.
//!
//! Where the adapters' copies of a rule differed, each behaviour is kept
//! under a name or a mode of its own, and each adapter asks for the one it
//! had.
//!
//! Owns: pytest's rules, as the adapters and the binary read them. Does
//! not own: what a module binds (aye-python-semantic), the standard
//! library's behaviour (aye-python-stdlib), nor what an adapter predicts.
//!
//! Depends on: aye-adapter (the project an adapter prepares for, its
//! errors and settings), aye-config (where a value came from, the TOML
//! loader), aye-events (the adapters' names), aye-fs (pytest's common
//! ancestor), aye-python-stdlib (`fnmatch`, `glob`, `os.path`, reprs,
//! identifiers) and aye-static (the variables pytest reads). Only the
//! adapters and the binary depend on it.

pub mod argv;
pub mod config;
pub mod conftest;
pub mod doctest;
pub mod env;
pub mod getini;
pub mod ini;
pub mod names;
pub mod pathlib;
pub mod session;
