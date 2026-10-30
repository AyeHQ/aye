//! The run store's writer ([`RunWriter`]): it claims the run's number,
//! keeps its log as the records arrive, and stores the run when the stream
//! ends, with its summary and `last`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use aye_events::{
    ContentDigest, Counts, DiagnosticCode, DiagnosticOrigin, ErrorClass, Event, ProjectPath,
    Record, RunNumber, Severity,
};
use flate2::Compression;
use flate2::write::GzEncoder;

use super::id::numbered;
use super::layout::{LAST, LOG, META, replace};
use super::{RunMeta, RunStore, StoredRunId, VERSION};

/// How many numbers a run tries before it gives up claiming one.
const CLAIMS: u64 = 1000;

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
    thread: thread::JoinHandle<io::Result<BTreeMap<ProjectPath, ContentDigest>>>,
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
pub(super) fn shows(record: &Record) -> bool {
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
) -> io::Result<BTreeMap<ProjectPath, ContentDigest>> {
    let mut digests = BTreeMap::new();
    let written = (|| {
        let mut out = GzEncoder::new(file, Compression::fast());
        for chunk in chunks {
            match chunk {
                Chunk::Lines(lines) => out.write_all(&lines)?,
                Chunk::Digests(files) => {
                    for file in files {
                        if let Ok(bytes) = fs::read(root.join(file.as_str())) {
                            digests.insert(file, ContentDigest::of(&bytes));
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

impl RunStore {
    /// Makes a complete log a stored run: its summary, then `last`.
    fn add(&self, meta: &RunMeta) -> io::Result<()> {
        let mut summary = serde_json::to_vec(meta)?;
        summary.push(b'\n');
        replace(&self.dir, &format!("{}{META}", meta.run), &summary)?;
        replace(&self.dir, LAST, format!("{}\n", meta.run).as_bytes())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}
