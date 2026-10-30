//! The runs kept in the store, read back for `aye show` ([`RunStore`]):
//! listed, the last first, each one's summary, and its log.

use std::cmp::Reverse;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use aye_events::Record;
use flate2::read::GzDecoder;

use super::layout::{LAST, LOG, META, is_run_id};
use super::{RunMeta, StoredRunId, VERSION};

/// The runs kept in a directory (`.aye/runs`), for `aye show`.
#[derive(Debug, Clone)]
pub struct RunStore {
    pub(super) dir: PathBuf,
}

impl RunStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every run kept, the last first: the one `last` names, then the
    /// others, the one that finished last first. (Two runs finishing at
    /// once may rename `last` in the other order than their summaries
    /// say they finished: `last` wins, so that the list agrees with it.)
    pub fn runs(&self) -> Vec<RunMeta> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut runs: Vec<RunMeta> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let id = name.to_str()?.strip_suffix(META)?;
                is_run_id(id).then(|| self.meta(id))?
            })
            .collect();
        let last = self.named_last();
        runs.sort_by_cached_key(|meta| {
            Reverse((
                last.as_ref() == Some(&meta.run),
                meta.finished_ms,
                meta.run.to_string(),
            ))
        });
        runs
    }

    /// The last run: the one `last` names, or, when that one is gone (or
    /// `last` is), the newest run stored. `None` when no run is.
    pub fn last(&self) -> Option<RunMeta> {
        self.runs().into_iter().next()
    }

    /// The id `last` holds, when it is one.
    fn named_last(&self) -> Option<StoredRunId> {
        let id = fs::read_to_string(self.dir.join(LAST)).ok()?;
        let id = id.trim();
        is_run_id(id).then(|| StoredRunId::from(id))
    }

    /// The summary of the run `id`, when it is kept.
    pub fn meta(&self, id: &str) -> Option<RunMeta> {
        if !is_run_id(id) {
            return None;
        }
        let bytes = fs::read(self.dir.join(format!("{id}{META}"))).ok()?;
        let meta: RunMeta = serde_json::from_slice(&bytes).ok()?;
        (meta.version == VERSION && meta.run == id).then_some(meta)
    }

    /// The runs kept whose id starts with `prefix`, the one that finished
    /// last first.
    pub fn find(&self, prefix: &str) -> Vec<RunMeta> {
        self.runs()
            .into_iter()
            .filter(|meta| meta.run.to_string().starts_with(prefix))
            .collect()
    }

    /// The log of the run `id`, record by record. A line that doesn't read
    /// as a record (the end of a log cut short) is skipped.
    pub fn log(&self, id: &str) -> io::Result<Vec<Record>> {
        if !is_run_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{id:?} is not a run's id"),
            ));
        }
        let file = fs::File::open(self.dir.join(format!("{id}{LOG}")))?;
        let reader = BufReader::with_capacity(1 << 16, GzDecoder::new(file));
        let mut records = Vec::new();
        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                // A log cut short ends early.
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e),
            };
            if let Ok(record) = serde_json::from_str(&line) {
                records.push(record);
            }
        }
        Ok(records)
    }

    /// Writes the log of the run `id` to `out` as stored: every whole line,
    /// one record each, as the run wrote it. A log cut short ends with its
    /// last whole line.
    pub fn copy_log(&self, id: &str, out: &mut impl Write) -> io::Result<()> {
        if !is_run_id(id) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{id:?} is not a run's id"),
            ));
        }
        let file = fs::File::open(self.dir.join(format!("{id}{LOG}")))?;
        let mut reader = BufReader::with_capacity(1 << 16, GzDecoder::new(file));
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) => return Ok(()),
                Ok(_) if line.ends_with(b"\n") => out.write_all(&line)?,
                // The end of a log cut short.
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }
}
