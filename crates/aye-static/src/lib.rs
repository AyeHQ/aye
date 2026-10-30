//! The environment: which variables aye knows by name, and the environment
//! as aye read it.
//!
//! [`EnvVars`] lists every variable aye reads by name or sets, in its Rust
//! or in the Python it sends to workers, with what each one does. Rust names
//! them only through it, so a misspelt name doesn't compile. The Python side
//! (the worker, the engine and the executors, which import only the
//! standard library) spells the names itself; the catalogue lists those
//! too, and the binary's tests check that it does.
//!
//! [`ProcessEnv`] is the environment as aye read it: every variable, each
//! name's case kept, looked up as Python's `os.environ` looks one up. The
//! binary reads it once ([`ProcessEnv::from_process`]) and hands it down;
//! `clippy.toml` disallows `std::env`'s readers everywhere else, but for the
//! tests' own settings.
//!
//! Two reads of what is handed down go by no name and stay out of the
//! catalogue: the refinement cache's key holds the whole environment
//! workers inherit, and the paths aye and pytest expand (`--junitxml`,
//! `--rootdir`) read whichever variable the user names.
//!
//! Owns: the catalogue and [`ProcessEnv`]. Does not own: what a variable
//! means to the code that reads it, nor when it is read.
//!
//! Depends on: the standard library only. Must not depend on any aye crate.

mod env_vars;
mod process_env;

pub use env_vars::EnvVars;
pub use process_env::ProcessEnv;
