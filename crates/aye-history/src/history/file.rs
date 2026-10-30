//! The history file, `<dir>/history.json`: read whole or not at all
//! ([`History::load`]), written whole or not at all ([`History::save`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use aye_events::TestId;
use aye_fs::{HISTORY_TEMPORARY, StateBucket, atomic_write, prepare_dir};
use serde::{Deserialize, Serialize};

use super::{History, TestStats};

/// The history file, inside the directory history is given.
pub const FILE: &str = StateBucket::History.name();

/// The format of the history file. A file of any other version loads as
/// empty history.
pub const VERSION: u32 = 1;

#[derive(Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    tests: BTreeMap<TestId, TestStats>,
    #[serde(default)]
    last_failed: BTreeSet<TestId>,
}

#[derive(Serialize)]
struct StoredRef<'a> {
    version: u32,
    tests: &'a BTreeMap<TestId, TestStats>,
    last_failed: &'a BTreeSet<TestId>,
}

impl History {
    /// Reads `<dir>/history.json`. Anything short of a valid file of this
    /// version gives empty history.
    pub fn load(dir: &Path) -> Self {
        fs::read(dir.join(FILE))
            .ok()
            .and_then(|bytes| Self::parse(&bytes))
            .unwrap_or_default()
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let stored: Stored = serde_json::from_slice(bytes).ok()?;
        if stored.version != VERSION || !stored.tests.values().all(TestStats::is_valid) {
            return None;
        }
        Some(Self {
            tests: stored.tests,
            last_failed: stored.last_failed,
            ..Self::default()
        })
    }

    /// Writes `<dir>/history.json` atomically, creating `dir` (see
    /// `aye_fs::prepare_dir`) if needed.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        prepare_dir(dir)?;
        atomic_write(dir, FILE, HISTORY_TEMPORARY, |file| {
            let mut out = io::BufWriter::new(file);
            serde_json::to_writer(
                &mut out,
                &StoredRef {
                    version: VERSION,
                    tests: &self.tests,
                    last_failed: &self.last_failed,
                },
            )?;
            out.write_all(b"\n")?;
            out.flush()
        })
    }
}
