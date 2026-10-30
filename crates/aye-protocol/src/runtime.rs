//! An adapter's runtime face: what a worker installs and builds to run the
//! adapter's batches and refinements.

use std::borrow::Cow;
use std::fmt;

use aye_events::AdapterName;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{ExecutorSpec, ModuleSet};

/// An adapter's runtime face: the modules a worker installs for it, and the
/// executor it builds from them, which refines the adapter's files and
/// runs its batches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub adapter: AdapterName,
    /// Installed in order, after aye's engine (`aye_engine::ENGINE`) and
    /// the prelude every worker gets, whatever its runtimes.
    pub modules: ModuleSet,
    /// The `aye_engine.Executor` subclass the worker builds.
    pub entry: EntryPoint,
    /// Whether the executor keeps what it lists for the batches it runs
    /// later.
    pub session: SessionMode,
}

impl Runtime {
    /// What the handshake tells a worker of it.
    pub fn spec(&self) -> ExecutorSpec {
        ExecutorSpec {
            adapter: self.adapter.clone(),
            entry: self.entry.clone(),
            session: self.session,
        }
    }
}

/// A class of a module a worker installs: the executor a [`Runtime`]'s
/// worker builds. It is written `module:Class`, which the worker splits at
/// the first colon (`aye_engine.load_entry`), so a module's name holds
/// none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPoint {
    module: Cow<'static, str>,
    class: Cow<'static, str>,
}

impl EntryPoint {
    /// The class `class` of the module `module`, whose name holds no colon.
    pub const fn new(module: &'static str, class: &'static str) -> Self {
        assert!(!has_colon(module), "a module's name holds no colon");
        Self {
            module: Cow::Borrowed(module),
            class: Cow::Borrowed(class),
        }
    }

    pub fn module(&self) -> &str {
        &self.module
    }

    pub fn class(&self) -> &str {
        &self.class
    }
}

const fn has_colon(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b':' {
            return true;
        }
        at += 1;
    }
    false
}

impl fmt::Display for EntryPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.module, self.class)
    }
}

impl Serialize for EntryPoint {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for EntryPoint {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let (module, class) = text
            .split_once(':')
            .ok_or_else(|| serde::de::Error::custom("an entry point is `module:Class`"))?;
        Ok(Self {
            module: Cow::Owned(module.to_owned()),
            class: Cow::Owned(class.to_owned()),
        })
    }
}

/// Whether an executor keeps what it lists for the batches it runs later.
/// On the wire, the handshake's `lists_session` (protocol 2) says it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SessionMode {
    /// A batch needs nothing a refinement listed: unittest's and doctest's
    /// executors list nothing they keep.
    #[default]
    PerBatch,
    /// The executor keeps what it lists (its imported files, its model) for
    /// the batches it runs later, as pytest-xdist's workers each collect
    /// the whole session, so a worker lists the session once: the runner
    /// keeps the worker that refined as its first lane, and every other
    /// lane lists the session's files before its first batch. The pytest
    /// adapter's.
    ListsSession,
}

impl SessionMode {
    pub const fn lists_session(self) -> bool {
        matches!(self, Self::ListsSession)
    }
}

/// `lists_session`'s value.
impl Serialize for SessionMode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(self.lists_session())
    }
}

impl<'de> Deserialize<'de> for SessionMode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(if bool::deserialize(deserializer)? {
            Self::ListsSession
        } else {
            Self::PerBatch
        })
    }
}
