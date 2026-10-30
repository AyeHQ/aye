//! How to start a worker, and the handshake that says what it installs.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;

use aye_engine::ENGINE;
use aye_protocol::{ModuleSet, PROTOCOL_VERSION, Request, Runtime, ServiceName};

use crate::stop::Registry;

use super::Echo;

/// How to start a worker.
#[derive(Debug, Clone)]
pub struct WorkerSpec {
    /// The interpreter.
    pub python: PathBuf,
    /// The project root, absolute; sent in the handshake.
    pub root: PathBuf,
    /// The directory the worker starts in.
    pub cwd: PathBuf,
    /// Installed after aye's engine and before every runtime's modules,
    /// whatever the runtimes: the binary's prelude, aye's `pytest` module
    /// and what it stands on.
    pub prelude: ModuleSet,
    /// The runtimes whose executors the worker builds. Their modules are
    /// installed after the prelude, in order, each name once.
    pub runtimes: Vec<Runtime>,
    /// Services to provide: [`ServiceName::DEFAULT`] unless told
    /// otherwise.
    pub services: Vec<ServiceName>,
    /// Environment variables set on top of the inherited environment, after
    /// `PYTHONIOENCODING=utf-8`.
    pub env: Vec<(OsString, OsString)>,
    /// Echo the worker's stderr (where its stdout goes too) live, as with
    /// `-s`, and where to. It is kept for crash reports either way.
    pub echo: Option<Echo>,
    /// The runner's live workers, which this one joins.
    pub(crate) registry: Option<Registry>,
}

impl WorkerSpec {
    pub fn new(
        python: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self {
            python: python.into(),
            root: root.into(),
            cwd: cwd.into(),
            prelude: ModuleSet::default(),
            runtimes: Vec::new(),
            services: ServiceName::DEFAULT.to_vec(),
            env: Vec::new(),
            echo: None,
            registry: None,
        }
    }

    /// The handshake this spec sends: aye's engine, the prelude, then each
    /// runtime's modules, each name once, in that order.
    pub fn hello(&self) -> Request {
        let mut seen = HashSet::new();
        let modules = ENGINE
            .iter()
            .chain(self.prelude)
            .chain(self.runtimes.iter().flat_map(|runtime| runtime.modules))
            .filter(|module| seen.insert(&*module.name))
            .cloned()
            .collect();
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            root: self.root.to_string_lossy().into_owned(),
            modules,
            executors: self.runtimes.iter().map(Runtime::spec).collect(),
            services: self.services.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use aye_engine::ENGINE_MODULE;
    use aye_events::AdapterName;
    use aye_protocol::{EntryPoint, ModuleSource, SessionMode};

    use super::*;

    const ONE: ModuleSource = ModuleSource::embedded("one", "");
    const TWO: ModuleSource = ModuleSource::embedded("two", "");

    #[test]
    fn hello_installs_the_engine_the_prelude_then_each_module_once() {
        const ZERO: ModuleSource = ModuleSource::embedded("zero", "");
        let mut spec = WorkerSpec::new("python", "/repo", "/repo");
        spec.prelude = ModuleSet::new(const { &[ZERO, ONE] });
        spec.runtimes = vec![
            Runtime {
                adapter: AdapterName::new_static("a"),
                modules: ModuleSet::new(const { &[ONE, TWO] }),
                entry: EntryPoint::new("two", "A"),
                session: SessionMode::PerBatch,
            },
            Runtime {
                adapter: AdapterName::new_static("b"),
                modules: ModuleSet::new(const { &[TWO, ENGINE_MODULE] }),
                entry: EntryPoint::new("two", "B"),
                session: SessionMode::ListsSession,
            },
        ];
        let Request::Hello {
            protocol,
            root,
            modules,
            executors,
            services,
        } = spec.hello()
        else {
            panic!("expected hello");
        };
        assert_eq!(protocol, PROTOCOL_VERSION);
        assert_eq!(root, "/repo");
        let names: Vec<&str> = modules.iter().map(|m| &*m.name).collect();
        assert_eq!(names, ["aye_engine", "zero", "one", "two"]);
        assert_eq!(modules, [ENGINE_MODULE, ZERO, ONE, TWO]);
        assert_eq!(executors.len(), 2);
        assert_eq!(executors[1].entry.to_string(), "two:B");
        assert_eq!(
            executors.iter().map(|e| e.session).collect::<Vec<_>>(),
            [SessionMode::PerBatch, SessionMode::ListsSession]
        );
        assert_eq!(services, ServiceName::DEFAULT);
    }
}
