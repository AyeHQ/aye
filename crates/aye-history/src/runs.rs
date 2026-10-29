//! The run store: each run's whole log, kept under `.aye/runs` for `aye
//! show`, another consumer of the stream.
//!
//! Runs are numbered: `r1`, `r2`, … A run takes 1 + the highest number
//! the directory's runs (stored, being written, or left over) and `last`
//! have, and claims it by creating its log with `create_new`, trying the
//! next number while another run has it ([`RunWriter::claim`]): two runs
//! at once never share one. The binary claims the number before the
//! run's `run_started`, which says it (`run_number` on the wire). A run
//! with nothing to show deletes its log, and its number is free again.
//! Runs stored under the hex ids of aye before October 2026 count for no
//! number, and are listed and pruned like any other.
//!
//! Layout, in the directory it is given (`.aye/runs`):
//! - `r<N>.jsonl.gz`: the run's log, every record as `aye show --log`
//!   prints it, one per line, gzip-compressed (`miniz_oxide`'s fastest
//!   level). It is written as the records arrive: each is serialized where
//!   it arrives, and the serialized log is handed in chunks to a thread of
//!   its own, which compresses it, so the run never waits on compression.
//! - `r<N>.json`: what the store knows beside the log ([`RunMeta`]):
//!   when the run started and finished (the store's clock, as it saw
//!   `run_started` and the end of the stream), its exit code, counts and
//!   duration from `run_finished`, what its summary counts beside those
//!   (`extra`: modules skipped while collecting, failed lifetimes that kept
//!   no test from running, error diagnostics; the binary hands them over,
//!   as `aye_render::Tally` counts them), and the digest of each file
//!   holding a test it collected (or that collection reported an error on,
//!   or skipped), read when `collection_finished` arrives, so that `aye
//!   show` can tell a file that changed since. Written when
//!   the log is complete, by writing a temporary file and renaming it
//!   over: a run is in the store once this file exists, so a log being
//!   written, or left by an aye that was killed, is never shown.
//! - `last`: the id of the run that finished last, replaced the same way
//!   once its summary is written. Two runs at once each write their own
//!   files; the one that renames `last` last is `last`, and listed first
//!   ([`RunStore::runs`]). When the run it names is gone (another aye's
//!   retention, a hand), the newest stored run stands for it.
//!
//! A run is kept when it has something to show: a test's result, a failed
//! lifetime, a failed worker, or an error diagnostic other than the
//! command line's (what it names that doesn't exist: `not-found` from
//! outside any batch, `unclaimed`). One that ran nothing else (a typo in a
//! path, a `-k` that matches nothing, a collection interrupted) leaves no
//! trace, so `last` stays the run before it, and it takes no place in
//! retention. A collect-only run is not stored either. When a run is
//! kept, the store keeps the `keep` runs that finished last (and `last`)
//! and deletes the rest (`[tool.aye] keep-runs`, 10 by default), and logs
//! left without a summary for a day. `keep` 0 writes nothing and deletes
//! every stored run. Nothing here may fail a run: the binary reports
//! errors as warnings.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aye_events::{
    Counts, DiagnosticCode, DiagnosticOrigin, ErrorClass, Event, ExitCode, Fnv128, ProjectPath,
    Record, RunId, RunNumber, Severity, StopReason,
};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The store's directory, inside aye's state directory.
pub const DIR: &str = aye_fs::StateBucket::Runs.name();

/// The file naming the run that finished last.
pub const LAST: &str = "last";

/// The format of a run's summary file. One of another version is not
/// listed.
pub const VERSION: u32 = 1;

/// After a run's id: its log.
const LOG: &str = ".jsonl.gz";
/// After a run's id: its summary.
const META: &str = ".json";
/// Before a temporary file's name.
const TEMPORARY: &str = ".tmp-";
/// How old a log without a summary, or a temporary file, must be before
/// it is deleted: older than any run still writing it.
const LEFT_OVER: Duration = Duration::from_hours(24);

/// How many numbers a run tries before it gives up claiming one.
const CLAIMS: u64 = 1000;

/// What the store keeps about a run beside its log: `r<N>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMeta {
    pub version: u32,
    /// The run's id in the store: `r81` (a run stored before runs were
    /// numbered has its records' 16 hex digits).
    pub run: StoredRunId,
    /// The project root, absolute, from `run_started`.
    pub root: String,
    /// When the store saw `run_started`, in milliseconds since the Unix
    /// epoch.
    pub started_ms: u64,
    /// When the store had the whole log, likewise.
    pub finished_ms: u64,
    /// From `run_finished`; absent when the log has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<ExitCode>,
    #[serde(default)]
    pub counts: Counts,
    /// The run's own duration, from `run_finished`, in whole milliseconds.
    #[serde(default)]
    pub ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StopReason>,
    /// What the run's summary counts beside `counts`, by name, the non-zero
    /// ones (`aye_render::Extra::named`: `skipped_modules`, `setup_error`,
    /// `teardown_error`, `lifetime_error`, `collection_error`,
    /// `usage_error`, `internal_error`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, u32>,
    /// Each file holding a test the run collected, or that collection
    /// reported an error on or skipped, relative to the root as ids spell
    /// it, and the digest of its content ([`digest`]) when collection
    /// finished. A file that couldn't be read (a directory) is left out.
    #[serde(default)]
    pub files: BTreeMap<ProjectPath, String>,
}

/// How a file differs from what a stored run read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Its content changed.
    Edited,
    /// It no longer exists.
    Gone,
}

impl RunMeta {
    /// How the file `path` (relative to `root`, as ids spell it) changed
    /// since the run; `None` when it didn't, or when the run recorded no
    /// digest for it.
    pub fn change(&self, root: &Path, path: &str) -> Option<Change> {
        let recorded = self.files.get(path)?;
        match fs::read(root.join(path)) {
            Ok(bytes) => (digest(&bytes) != *recorded).then_some(Change::Edited),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Some(Change::Gone),
            Err(_) => None,
        }
    }

    /// The run's number: 81 for `r81`; `None` for a run stored under a hex
    /// id.
    pub fn number(&self) -> Option<RunNumber> {
        self.run.number()
    }
}

/// A stored run's id, which names its files: `r81` for the run numbered
/// 81, or, for a run stored before runs were numbered, its records' run id
/// (16 hex digits). Any text but a number's own spelling, `r` and its
/// digits without leading zeros, is such a legacy id, so an id reads back
/// as it was written, and two ids are equal when their texts are.
///
/// It prints and serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StoredRunId {
    Numbered(RunNumber),
    Legacy(RunId),
}

impl StoredRunId {
    /// The run's number, when it has one.
    pub fn number(&self) -> Option<RunNumber> {
        match self {
            Self::Numbered(number) => Some(*number),
            Self::Legacy(_) => None,
        }
    }
}

/// The number `id` is the spelling of: `r` and its digits, without
/// leading zeros.
fn numbered(id: &str) -> Option<RunNumber> {
    let digits = id.strip_prefix('r')?;
    if digits.starts_with('0') {
        return None;
    }
    id.parse().ok()
}

impl From<&str> for StoredRunId {
    fn from(id: &str) -> Self {
        numbered(id).map_or_else(|| Self::Legacy(RunId::from(id)), Self::Numbered)
    }
}

impl fmt::Display for StoredRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numbered(number) => number.fmt(f),
            Self::Legacy(id) => id.fmt(f),
        }
    }
}

/// Whether `other` is its text.
impl PartialEq<str> for StoredRunId {
    fn eq(&self, other: &str) -> bool {
        match self {
            Self::Numbered(number) => numbered(other) == Some(*number),
            Self::Legacy(id) => numbered(other).is_none() && *id == *other,
        }
    }
}

impl PartialEq<&str> for StoredRunId {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl Serialize for StoredRunId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for StoredRunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = Box::<str>::deserialize(deserializer)?;
        Ok(Self::from(&*id))
    }
}

/// The digest of a file's content: FNV-1a, 128 bits, as 32 hex digits.
/// Stable across builds and platforms; it tells whether a file changed,
/// and nothing more.
pub fn digest(bytes: &[u8]) -> String {
    format!("{:032x}", Fnv128::hash(bytes))
}

/// Keeps the log of the run fed to it, as a consumer of the stream: each
/// record is serialized as it arrives, into a buffer handed in chunks to a
/// thread that compresses them into the log and reads the digests of the
/// collected files; [`RunWriter::finish`] completes the log, records it as
/// `last` and applies retention.
///
/// Serializing where the records are, rather than moving them to the
/// thread, keeps their memory where it was allocated: freeing a run's
/// records on another thread made the run measurably slower (macOS's
/// allocator), serializing them doesn't.
#[derive(Debug)]
pub struct RunWriter {
    dir: PathBuf,
    keep: usize,
    /// The number claimed for the run, and its log, created; `None` before
    /// a claim, and once the run took it.
    claimed: Option<io::Result<Claimed>>,
    /// The run being written, from its `run_started`.
    run: Option<Run>,
    /// Whether a `run_started` came, and said the run only collects.
    started: Option<bool>,
    /// Why the run can't be kept, for `finish` to report.
    failed: Option<io::Error>,
}

/// A run's number, and its log, created empty.
#[derive(Debug)]
struct Claimed {
    number: RunNumber,
    log: fs::File,
}

/// A run whose log is being written.
#[derive(Debug)]
struct Run {
    /// Records serialized, not yet handed to the thread.
    lines: Vec<u8>,
    sender: Option<mpsc::Sender<Chunk>>,
    thread: thread::JoinHandle<io::Result<BTreeMap<ProjectPath, String>>>,
    meta: RunMeta,
    /// The files of the tests collected so far, each once.
    files: Vec<ProjectPath>,
    seen: HashSet<ProjectPath>,
    /// Whether the run has something to show (see the module's docs).
    worth: bool,
}

/// What the writer's thread is handed.
enum Chunk {
    /// Serialized records, one per line.
    Lines(Vec<u8>),
    /// Collection finished: the digests of these files.
    Digests(Vec<ProjectPath>),
}

/// How much serialized log is handed to the thread at once.
const CHUNK: usize = 1 << 18;

impl RunWriter {
    /// Keeps the run in `dir` (created when the run claims its number, with
    /// `aye_fs::prepare_dir`), and at most `keep` runs there. With `keep`
    /// 0, nothing is written, and the runs stored are deleted.
    pub fn new(dir: impl Into<PathBuf>, keep: usize) -> Self {
        Self {
            dir: dir.into(),
            keep,
            claimed: None,
            run: None,
            started: None,
            failed: None,
        }
    }

    /// Where the runs are kept.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The number the run will be stored under (`r81`), claimed now, before
    /// its `run_started`, which should say it: by creating its log. `None`
    /// when no run is kept (`keep` 0), or the claim failed (`finish` says
    /// why). Called by a run that will be stored (not one that only
    /// collects); a writer fed a `run_started` without a claim claims then.
    /// A number claimed for a run that turns out to have nothing to show,
    /// or only to collect, is free again once it finishes.
    pub fn claim(&mut self) -> Option<RunNumber> {
        if self.keep == 0 || self.started.is_some() {
            return None;
        }
        let claimed = self.claimed.get_or_insert_with(|| claim(&self.dir));
        claimed.as_ref().ok().map(|claimed| claimed.number)
    }

    /// Takes one record of the run's log.
    pub fn record(&mut self, record: &Record) {
        if let Event::RunStarted {
            collect_only, root, ..
        } = &record.event
            && self.started.is_none()
        {
            self.started = Some(*collect_only);
            if self.keep > 0 && !*collect_only {
                match self.claimed.take().unwrap_or_else(|| claim(&self.dir)) {
                    Ok(claimed) => {
                        let number = claimed.number;
                        self.run = Run::start(&self.dir, claimed, root);
                        if self.run.is_none() {
                            let _ = fs::remove_file(self.log(number));
                            self.failed =
                                Some(io::Error::other("the run store's writer could not start"));
                        }
                    }
                    Err(err) => self.failed = Some(err),
                }
            }
        }
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.sender.is_none() {
            // The thread stopped on an error, which `finish` reports.
            return;
        }
        if serde_json::to_writer(&mut run.lines, record).is_err() {
            run.sender = None;
            return;
        }
        run.lines.push(b'\n');
        run.note(record);
        if run.lines.len() >= CHUNK {
            let lines = std::mem::replace(&mut run.lines, Vec::with_capacity(CHUNK + (1 << 12)));
            run.send(Chunk::Lines(lines));
        }
    }

    /// The stream has ended: completes the log, writes its summary, with
    /// `extra` (what the run's summary counts beside the tests' counts, by
    /// name), and `last`, then deletes what retention doesn't keep. The
    /// run's id (`r81`), when it was kept: a run with nothing to show is
    /// not (its log is deleted, and its number free again).
    pub fn finish(mut self, extra: BTreeMap<String, u32>) -> io::Result<Option<StoredRunId>> {
        let store = RunStore::new(&self.dir);
        // A number claimed for a run that never started, or only collected.
        if let Some(Ok(claimed)) = self.claimed.take() {
            drop(claimed.log);
            remove(&self.log(claimed.number))?;
        }
        if let Some(err) = self.failed.take() {
            return Err(err);
        }
        let Some(mut run) = self.run.take() else {
            if self.keep == 0 && self.started == Some(false) {
                store.retain(0, None)?;
            }
            return Ok(None);
        };
        let lines = std::mem::take(&mut run.lines);
        run.send(Chunk::Lines(lines));
        drop(run.sender.take());
        let files = run
            .thread
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("the run store's writer panicked")))?;
        if !run.worth {
            remove(&self.log_of(&run.meta.run))?;
            return Ok(None);
        }
        let meta = RunMeta {
            finished_ms: now_ms(),
            files,
            extra,
            ..run.meta
        };
        store.add(&meta)?;
        store.retain(self.keep, Some(&meta.run))?;
        Ok(Some(meta.run))
    }

    /// The log of the run numbered `number`.
    fn log(&self, number: RunNumber) -> PathBuf {
        self.log_of(&StoredRunId::Numbered(number))
    }

    fn log_of(&self, id: &StoredRunId) -> PathBuf {
        self.dir.join(format!("{id}{LOG}"))
    }
}

/// Claims the next run number in `dir` (created if need be, with
/// `aye_fs::prepare_dir`): 1 + the highest number of the runs there and
/// of `last`, or the first after it no other run has claimed meanwhile.
fn claim(dir: &Path) -> io::Result<Claimed> {
    aye_fs::prepare_dir(dir)?;
    let first = highest(dir).saturating_add(1);
    for number in (first..first.saturating_add(CLAIMS)).filter_map(RunNumber::new) {
        let path = dir.join(format!("{number}{LOG}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(log) => return Ok(Claimed { number, log }),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "no run number from {first} on is free in {}",
        dir.display()
    )))
}

/// The highest run number in `dir`: of its runs' summaries and logs
/// (stored, being written, or left over), and of `last`; 0 when none.
fn highest(dir: &Path) -> u64 {
    let number = |id: &str| numbered(id).map(RunNumber::get);
    let mut highest = fs::read_to_string(dir.join(LAST))
        .ok()
        .and_then(|last| number(last.trim()))
        .unwrap_or(0);
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(n) = name
            .strip_suffix(META)
            .or_else(|| name.strip_suffix(LOG))
            .and_then(number)
        {
            highest = highest.max(n);
        }
    }
    highest
}

/// Deletes `path`, which may be gone already.
fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

impl Run {
    /// Starts the thread that writes the log of the run `claimed` in `dir`.
    fn start(dir: &Path, claimed: Claimed, root: &str) -> Option<Self> {
        let (sender, chunks) = mpsc::channel();
        let id = StoredRunId::Numbered(claimed.number);
        let path = dir.join(format!("{id}{LOG}"));
        let writer_root = PathBuf::from(root);
        let thread = thread::Builder::new()
            .name("aye-runs".into())
            .spawn(move || write_log(claimed.log, &path, &writer_root, &chunks))
            .ok()?;
        Some(Self {
            lines: Vec::with_capacity(CHUNK + (1 << 12)),
            sender: Some(sender),
            thread,
            meta: RunMeta {
                version: VERSION,
                run: id,
                root: root.to_string(),
                started_ms: now_ms(),
                finished_ms: 0,
                exit_code: None,
                counts: Counts::default(),
                ms: 0,
                stopped: None,
                extra: BTreeMap::new(),
                files: BTreeMap::new(),
            },
            files: Vec::new(),
            seen: HashSet::new(),
            worth: false,
        })
    }

    fn send(&mut self, chunk: Chunk) {
        if let Some(sender) = &self.sender
            && sender.send(chunk).is_err()
        {
            self.sender = None;
        }
    }

    /// A file whose digest the summary keeps, once.
    fn file(&mut self, path: &str) {
        if !path.is_empty() && !self.seen.contains(path) {
            let path = ProjectPath::as_written(path);
            self.seen.insert(path.clone());
            self.files.push(path);
        }
    }

    /// What the summary takes from a record, and whether it makes the run
    /// one to keep.
    fn note(&mut self, record: &Record) {
        self.worth = self.worth || shows(record);
        match &record.event {
            Event::CaseCollected(case) => self.file(case.id.path().as_str()),
            // A file that failed to collect, or was skipped while collecting,
            // which `aye show` shows too.
            Event::Diagnostic(diagnostic)
                if diagnostic.severity == Severity::Error
                    || (diagnostic.severity == Severity::Info
                        && diagnostic.code == DiagnosticCode::Skipped) =>
            {
                self.file(diagnostic.path.as_str());
            }
            Event::CollectionFinished { .. } => {
                let files = std::mem::take(&mut self.files);
                self.send(Chunk::Digests(files));
            }
            Event::RunFinished {
                exit_code,
                counts,
                duration,
                stopped,
            } => {
                self.meta.exit_code = Some(*exit_code);
                self.meta.counts = *counts;
                self.meta.ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
                self.meta.stopped.clone_from(stopped);
            }
            _ => {}
        }
    }
}

/// Whether a record gives a run something to show: a test's result, a
/// failed lifetime or worker, an error diagnostic other than the command
/// line's (what it names that doesn't exist; [`ErrorClass::shows`]).
fn shows(record: &Record) -> bool {
    match &record.event {
        Event::TestFinished(_)
        | Event::LifetimeFinished {
            failure: Some(_), ..
        }
        | Event::WorkerFinished { error: Some(_), .. } => true,
        Event::Diagnostic(diagnostic) => diagnostic
            .error_class(DiagnosticOrigin::of(&record.envelope))
            .is_some_and(ErrorClass::shows),
        _ => false,
    }
}

/// The writer's thread: compresses what it is handed into the log, `file`
/// at `path`, and reads the digests of the files collected (relative to
/// `root`), which it returns. A log that fails to be written is deleted.
fn write_log(
    file: fs::File,
    path: &Path,
    root: &Path,
    chunks: &mpsc::Receiver<Chunk>,
) -> io::Result<BTreeMap<ProjectPath, String>> {
    let mut digests = BTreeMap::new();
    let written = (|| {
        let mut out = GzEncoder::new(file, Compression::fast());
        for chunk in chunks {
            match chunk {
                Chunk::Lines(lines) => out.write_all(&lines)?,
                Chunk::Digests(files) => {
                    for file in files {
                        if let Ok(bytes) = fs::read(root.join(file.as_str())) {
                            digests.insert(file, digest(&bytes));
                        }
                    }
                }
            }
        }
        out.finish().map(drop)
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(path);
        return Err(err);
    }
    Ok(digests)
}

/// The runs kept in a directory (`.aye/runs`), for `aye show`.
#[derive(Debug, Clone)]
pub struct RunStore {
    dir: PathBuf,
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

    /// Makes a complete log a stored run: its summary, then `last`.
    fn add(&self, meta: &RunMeta) -> io::Result<()> {
        let mut summary = serde_json::to_vec(meta)?;
        summary.push(b'\n');
        replace(&self.dir, &format!("{}{META}", meta.run), &summary)?;
        replace(&self.dir, LAST, format!("{}\n", meta.run).as_bytes())
    }

    /// Keeps the `keep` runs that finished last, and `current`, deleting
    /// the others, and what is left over from runs that never completed
    /// (logs without a summary, temporary files) once a day old. With
    /// `keep` 0, `last` goes too.
    fn retain(&self, keep: usize, current: Option<&StoredRunId>) -> io::Result<()> {
        if !self.dir.is_dir() {
            return Ok(());
        }
        for meta in self.runs().iter().skip(keep) {
            if Some(&meta.run) == current {
                continue;
            }
            // The log first: a summary without its log is deleted next
            // time, a log without its summary only once it is old.
            match fs::remove_file(self.dir.join(format!("{}{LOG}", meta.run))) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            let _ = fs::remove_file(self.dir.join(format!("{}{META}", meta.run)));
        }
        let now = SystemTime::now();
        for entry in fs::read_dir(&self.dir)?.filter_map(Result::ok) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let left_over = match name.strip_suffix(LOG) {
                Some(id) => !self.dir.join(format!("{id}{META}")).exists(),
                None => name.starts_with(TEMPORARY),
            };
            let old = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > LEFT_OVER);
            if left_over && old {
                let _ = fs::remove_file(entry.path());
            }
        }
        if keep == 0 {
            match fs::remove_file(self.dir.join(LAST)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Writes `name` in `dir` whole or not at all (`aye_fs::atomic_write`).
fn replace(dir: &Path, name: &str, content: &[u8]) -> io::Result<()> {
    aye_fs::atomic_write(dir, name, TEMPORARY, |file| {
        file.write_all(content)?;
        file.flush()
    })
}

/// A run's id as the runner makes them (16 hex digits), or anything as
/// harmless in a file name: letters, digits, `-` and `_`.
fn is_run_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests;
