//! The services a runner may provide inside a worker, by name.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A service the runner may provide inside a worker. Engines look services
/// up by name and must cope with any of them being absent; the handshake
/// names those a worker provides, and the worker drops a name aye's engine
/// doesn't provide. On the wire, a service is its name
/// ([`ServiceName::as_str`]), which is also the value of the
/// `aye_engine.Services` constant of the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceName {
    /// Captures stdout, stderr and logging per test.
    Capture,
    /// The test or lifetime currently running.
    Context,
    /// Teardown registration on the current test or lifetime.
    Cleanup,
    Coverage,
    Sandbox,
    Timeout,
    Resources,
    Patching,
    Clock,
    Randomness,
    Network,
    Filesystem,
    Snapshots,
    Data,
}

impl ServiceName {
    /// Every service, in the order `aye_engine.Services` lists them.
    pub const ALL: [Self; 14] = [
        Self::Capture,
        Self::Context,
        Self::Cleanup,
        Self::Coverage,
        Self::Sandbox,
        Self::Timeout,
        Self::Resources,
        Self::Patching,
        Self::Clock,
        Self::Randomness,
        Self::Network,
        Self::Filesystem,
        Self::Snapshots,
        Self::Data,
    ];

    /// The services a worker provides unless told otherwise: those aye's
    /// engine provides itself.
    pub const DEFAULT: [Self; 2] = [Self::Context, Self::Cleanup];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Context => "context",
            Self::Cleanup => "cleanup",
            Self::Coverage => "coverage",
            Self::Sandbox => "sandbox",
            Self::Timeout => "timeout",
            Self::Resources => "resources",
            Self::Patching => "patching",
            Self::Clock => "clock",
            Self::Randomness => "randomness",
            Self::Network => "network",
            Self::Filesystem => "filesystem",
            Self::Snapshots => "snapshots",
            Self::Data => "data",
        }
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
