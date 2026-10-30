//! The Python the pytest adapter embeds: aye's own `pytest` module, the
//! pytest front-end and the executor, in the order a worker installs them;
//! the executor's entry point; and the digest of the sources the executor
//! runs on, which the context, and so the refinement cache's key, holds.

use aye_engine::{MODEL_MODULE, NATIVE_ENGINE_MODULE, REWRITE_MODULE};
use aye_events::Fnv64;
use aye_protocol::{EntryPoint, ModuleSet, ModuleSource};

/// aye's own `pytest` module: the authoring API (marks, fixtures,
/// `raises`, `approx`, outcomes) the front-end reads, installed as
/// `pytest`. The binary installs it in every worker, whatever its adapter,
/// after the model, the rewriter and the engine it stands on, so that it
/// is in `sys.modules` before the front-end and any test import it.
pub const SHIM_MODULE: ModuleSource = ModuleSource::embedded(
    "pytest",
    include_str!("../python/native/aye_pytest_shim.py"),
);

/// The pytest front-end: reads a suite into aye's model, never runs a test.
pub const FRONTEND_MODULE: ModuleSource = ModuleSource::embedded(
    "aye_pytest_frontend",
    include_str!("../python/native/aye_pytest_frontend.py"),
);

/// The glue: the executor (`refine`, `run`) over the front-end and the
/// engine.
pub const NATIVE_MODULE: ModuleSource = ModuleSource::embedded(
    "aye_pytest_native",
    include_str!("../python/native/aye_pytest_native.py"),
);

/// The native executor's own modules, in install order: after aye's
/// engine, and after aye's `pytest` module ([`SHIM_MODULE`]) and what it
/// stands on, which the binary installs in every worker first.
pub const MODULES: ModuleSet = ModuleSet::new(const { &[FRONTEND_MODULE, NATIVE_MODULE] });

/// The modules whose names and sources [`modules_digest`] covers, in the
/// order it has always hashed them: what a pytest worker installs after
/// aye's engine. The digest is in the context, and so in the refinement
/// cache's key, which a change to these, or to their order, changes.
static DIGESTED: [ModuleSource; 6] = [
    MODEL_MODULE,
    REWRITE_MODULE,
    NATIVE_ENGINE_MODULE,
    SHIM_MODULE,
    FRONTEND_MODULE,
    NATIVE_MODULE,
];

/// The native executor.
pub const ENTRY: EntryPoint = EntryPoint::new("aye_pytest_native", "NativeExecutor");

/// A 64-bit FNV-1a digest of the names and sources of the modules the
/// executor runs on ([`DIGESTED`]), in hex.
pub(crate) fn modules_digest() -> String {
    let mut hash = Fnv64::new();
    for module in &DIGESTED {
        for text in [&module.name, &module.source] {
            hash.write(text.as_bytes());
            hash.write(&[0]);
        }
    }
    format!("{:016x}", hash.finish())
}

#[cfg(test)]
mod tests {
    use aye_adapter::Adapter;
    use aye_protocol::SessionMode;

    use super::*;
    use crate::{NAME, Pytest};

    /// The executor's own modules, which a worker installs after aye's
    /// `pytest` module and what it stands on; the context's digest covers
    /// those too, in the order it has always hashed them.
    #[test]
    fn the_executor_installs_its_modules_and_the_digest_covers_what_they_stand_on() {
        let runtime = Pytest::default().runtime();
        assert_eq!(runtime.adapter, "pytest");
        assert_eq!(
            runtime.entry.to_string(),
            "aye_pytest_native:NativeExecutor"
        );
        let names: Vec<&str> = runtime.modules.names().collect();
        assert_eq!(names, ["aye_pytest_frontend", "aye_pytest_native"]);
        let digested: Vec<&str> = DIGESTED.iter().map(|m| &*m.name).collect();
        assert_eq!(
            digested,
            [
                "aye_model",
                "aye_rewrite",
                "aye_native_engine",
                "pytest",
                "aye_pytest_frontend",
                "aye_pytest_native"
            ]
        );
        assert_eq!(runtime.session, SessionMode::ListsSession);
        assert_eq!(Pytest::default().name(), NAME);
        assert_eq!(Pytest::static_listing().runtime(), runtime);
    }
}
