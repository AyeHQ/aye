use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use aye_events::{
    BatchIndex, Case, ContentDigest, Counts, Diagnostic, DiagnosticCode, Event, ExitCode, Failure,
    Outcome, Phase, Record, RunId, RunNumber, Severity, TestId, TestResult,
};
use aye_stream::FileChange;

use super::layout::replace;
use super::writer::shows;
use super::*;

/// A run's log as the binary stamps it: its id on every record, and each
/// record's place.
fn run(id: &str, root: &Path, outcomes: &[(&str, Outcome)]) -> Vec<Record> {
    let mut events = vec![
        Event::RunStarted {
            schema_version: 1,
            aye_version: "0.0.1".into(),
            root: root.display().to_string(),
            collect_only: false,
            workers: 1,
            run: None,
        },
        Event::CollectionStarted,
    ];
    for (test, _) in outcomes {
        events.push(Event::CaseCollected(Box::new(Case::new(*test, "pytest"))));
    }
    events.push(Event::CollectionFinished {
        selected: 2,
        deselected: 0,
        refined_files: 0,
        duration: Duration::from_millis(3),
    });
    let mut counts = Counts::default();
    for (test, outcome) in outcomes {
        events.push(Event::TestStarted {
            id: TestId::new(*test),
        });
        let mut result = TestResult::new(TestId::new(*test), *outcome);
        result.duration = Duration::from_millis(1);
        events.push(Event::TestFinished(Box::new(result)));
        counts.record(*outcome);
    }
    events.push(Event::RunFinished {
        exit_code: if counts.bad() > 0 {
            ExitCode::TestsFailed
        } else {
            ExitCode::Ok
        },
        counts,
        duration: Duration::from_millis(1234),
        stopped: None,
    });
    events
        .into_iter()
        .zip(0_u32..)
        .map(|(event, seq)| {
            let mut record = Record::new(event);
            record.envelope.run = Some(id.into());
            record.envelope.seq = Some(u64::from(seq));
            record.envelope.t_ms = Some(f64::from(seq));
            record
        })
        .collect()
}

/// Feeds `records` to a writer keeping `keep` runs in `dir`: the id the run
/// is stored under, when it is.
fn keep(dir: &Path, keep: usize, records: &[Record]) -> Option<String> {
    let mut writer = RunWriter::new(dir, keep);
    for record in records {
        writer.record(record);
    }
    writer
        .finish(BTreeMap::new())
        .unwrap()
        .map(|id| id.to_string())
}

/// The id a run is stored under, from its text.
fn stored(id: &str) -> StoredRunId {
    StoredRunId::from(id)
}

/// The names of the files in `dir`, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A project with two test files.
fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("tests")).unwrap();
    fs::write(dir.path().join("tests/test_a.py"), "def test_a(): pass\n").unwrap();
    fs::write(dir.path().join("tests/test_b.py"), "def test_b(): 1/0\n").unwrap();
    dir
}

const OUTCOMES: &[(&str, Outcome)] = &[
    ("tests/test_a.py::test_a", Outcome::Passed),
    ("tests/test_b.py::test_b", Outcome::Failed),
];

#[test]
fn a_run_is_kept_whole_with_its_summary_and_as_last() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let records = run("0123456789abcdef", project.path(), OUTCOMES);
    assert_eq!(keep(&dir, 10, &records).as_deref(), Some("r1"));
    // The log, gzip-compressed, is every record unchanged.
    let raw = fs::read(dir.join("r1.jsonl.gz")).unwrap();
    assert_eq!(&raw[..2], [0x1f, 0x8b]);
    let store = RunStore::new(&dir);
    assert_eq!(store.log("r1").unwrap(), records);
    // The summary, and `last`.
    let meta = store.last().unwrap();
    assert_eq!(meta.run, "r1");
    assert_eq!(meta.number(), RunNumber::new(1));
    assert_eq!(meta.version, VERSION);
    assert_eq!(meta.exit_code, Some(ExitCode::TestsFailed));
    assert_eq!((meta.counts.passed, meta.counts.failed), (1, 1));
    assert_eq!(meta.ms, 1234);
    assert!(meta.started_ms > 0 && meta.finished_ms >= meta.started_ms);
    assert_eq!(fs::read_to_string(dir.join(LAST)).unwrap(), "r1\n");
    // The state directory is marked as a cache.
    assert!(project.path().join(".aye/CACHEDIR.TAG").is_file());
    // No temporary file is left.
    assert_eq!(names(&dir), ["last", "r1.json", "r1.jsonl.gz"]);
}

#[test]
fn runs_are_numbered_after_the_highest_number_stored_and_last() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let ids: Vec<Option<String>> = (0..3)
        .map(|_| keep(&dir, 10, &run("any", project.path(), OUTCOMES)))
        .collect();
    assert_eq!(
        ids,
        [Some("r1".into()), Some("r2".into()), Some("r3".into())]
    );
    // `last` names a higher one (its summary deleted by hand): after it.
    fs::write(dir.join(LAST), "r7\n").unwrap();
    assert_eq!(
        keep(&dir, 10, &run("any", project.path(), OUTCOMES)).as_deref(),
        Some("r8")
    );
    // A run's number is the store's, whatever id its records carry.
    assert_eq!(
        RunStore::new(&dir).log("r8").unwrap()[0]
            .envelope
            .run
            .as_ref()
            .map(RunId::as_str),
        Some("any")
    );
    assert_eq!(
        stored("r81"),
        StoredRunId::Numbered(RunNumber::new(81).unwrap())
    );
    assert_eq!(stored("r81").to_string(), "r81");
    // Any other text is a legacy id, as it was written: a number's own
    // spelling has no leading zero.
    for not in [
        "81",
        "r",
        "r+1",
        "r-1",
        "rx",
        "0123456789abcdef",
        "",
        "r0",
        "r081",
    ] {
        assert_eq!(stored(not).number(), None, "{not}");
        assert_eq!(stored(not).to_string(), not);
    }
    assert_eq!(serde_json::to_string(&stored("r81")).unwrap(), r#""r81""#);
    assert_eq!(
        serde_json::from_str::<StoredRunId>(r#""r081""#).unwrap(),
        stored("r081")
    );
}

#[test]
fn a_number_another_run_holds_is_skipped() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 10, &run("a", project.path(), OUTCOMES));
    // Another aye claimed r2 and is still writing it.
    let mut other = RunWriter::new(&dir, 10);
    assert_eq!(other.claim(), RunNumber::new(2));
    assert_eq!(other.claim(), RunNumber::new(2), "claimed once");
    assert!(dir.join("r2.jsonl.gz").is_file());
    let mut writer = RunWriter::new(&dir, 10);
    assert_eq!(writer.claim(), RunNumber::new(3));
    // Each run is stored under the number it claimed, whichever ends first.
    for record in &run("b", project.path(), OUTCOMES) {
        writer.record(record);
    }
    assert_eq!(writer.finish(BTreeMap::new()).unwrap(), Some(stored("r3")));
    for record in &run("c", project.path(), OUTCOMES) {
        other.record(record);
    }
    assert_eq!(other.finish(BTreeMap::new()).unwrap(), Some(stored("r2")));
    let store = RunStore::new(&dir);
    assert_eq!(store.log("r3").unwrap(), run("b", project.path(), OUTCOMES));
    assert_eq!(store.log("r2").unwrap(), run("c", project.path(), OUTCOMES));
    // The highest number goes on, though `last` is r2.
    assert_eq!(
        keep(&dir, 10, &run("d", project.path(), OUTCOMES)).as_deref(),
        Some("r4")
    );
}

#[test]
fn a_number_claimed_for_a_run_that_isnt_kept_is_free_again() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    // Claimed, and the run only collected.
    let mut writer = RunWriter::new(&dir, 10);
    assert_eq!(writer.claim(), RunNumber::new(1));
    let mut records = run("a", project.path(), OUTCOMES);
    if let Event::RunStarted { collect_only, .. } = &mut records[0].event {
        *collect_only = true;
    }
    for record in &records {
        writer.record(record);
    }
    assert_eq!(writer.finish(BTreeMap::new()).unwrap(), None);
    assert_eq!(names(&dir), [] as [String; 0]);
    // Claimed, and the run never started.
    let mut writer = RunWriter::new(&dir, 10);
    assert_eq!(writer.claim(), RunNumber::new(1));
    assert_eq!(writer.finish(BTreeMap::new()).unwrap(), None);
    assert_eq!(names(&dir), [] as [String; 0]);
    // Claimed, and the run had nothing to show.
    let mut writer = RunWriter::new(&dir, 10);
    assert_eq!(writer.claim(), RunNumber::new(1));
    for record in &run("b", project.path(), &[]) {
        writer.record(record);
    }
    assert_eq!(writer.finish(BTreeMap::new()).unwrap(), None);
    assert_eq!(names(&dir), [] as [String; 0]);
    // So the next run is r1.
    assert_eq!(
        keep(&dir, 10, &run("c", project.path(), OUTCOMES)).as_deref(),
        Some("r1")
    );
    // Keeping none claims none.
    assert_eq!(RunWriter::new(&dir, 0).claim(), None);
}

#[test]
fn runs_stored_under_hex_ids_count_for_no_number_and_are_pruned_like_any_other() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    fs::create_dir_all(&dir).unwrap();
    let old = "fedcba9876543210";
    let meta = RunMeta {
        version: VERSION,
        run: old.into(),
        root: project.path().display().to_string(),
        started_ms: 1,
        finished_ms: 2,
        exit_code: Some(ExitCode::Ok),
        counts: Counts::default(),
        ms: 1,
        stopped: None,
        extra: BTreeMap::new(),
        files: BTreeMap::new(),
    };
    fs::write(
        dir.join(format!("{old}.json")),
        serde_json::to_vec(&meta).unwrap(),
    )
    .unwrap();
    fs::write(dir.join(format!("{old}.jsonl.gz")), b"").unwrap();
    fs::write(dir.join(LAST), format!("{old}\n")).unwrap();
    assert_eq!(RunStore::new(&dir).last().unwrap().number(), None);
    assert_eq!(
        keep(&dir, 2, &run("a", project.path(), OUTCOMES)).as_deref(),
        Some("r1")
    );
    let ids = |dir: &Path| -> Vec<String> {
        RunStore::new(dir)
            .runs()
            .into_iter()
            .map(|meta| meta.run.to_string())
            .collect()
    };
    assert_eq!(ids(&dir), ["r1", old]);
    keep(&dir, 2, &run("b", project.path(), OUTCOMES));
    assert_eq!(ids(&dir), ["r2", "r1"]);
    assert!(!dir.join(format!("{old}.json")).exists());
    assert!(!dir.join(format!("{old}.jsonl.gz")).exists());
}

#[test]
fn each_file_with_a_test_has_its_digest_and_changes_are_told() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 10, &run("aaaa", project.path(), OUTCOMES));
    let meta = RunStore::new(&dir).meta("r1").unwrap();
    assert_eq!(
        meta.files.keys().collect::<Vec<_>>(),
        ["tests/test_a.py", "tests/test_b.py"]
    );
    assert_eq!(
        meta.files["tests/test_a.py"],
        ContentDigest::of(b"def test_a(): pass\n")
    );
    let root = project.path();
    assert_eq!(meta.change(root, "tests/test_a.py"), None);
    fs::write(root.join("tests/test_a.py"), "def test_a(): assert 0\n").unwrap();
    assert_eq!(
        meta.change(root, "tests/test_a.py"),
        Some(FileChange::Edited)
    );
    fs::remove_file(root.join("tests/test_b.py")).unwrap();
    assert_eq!(meta.change(root, "tests/test_b.py"), Some(FileChange::Gone));
    // A file the run has no digest of is not said to have changed.
    assert_eq!(meta.change(root, "tests/test_c.py"), None);
}

#[test]
fn a_collect_only_run_is_not_kept() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let mut records = run("bbbb", project.path(), OUTCOMES);
    if let Event::RunStarted { collect_only, .. } = &mut records[0].event {
        *collect_only = true;
    }
    assert_eq!(keep(&dir, 10, &records), None);
    assert!(!dir.exists());
}

#[test]
fn the_newest_runs_are_kept_and_the_rest_deleted() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    for _ in 0..3 {
        keep(&dir, 2, &run("any", project.path(), OUTCOMES));
        // Finishing times are milliseconds apart.
        std::thread::sleep(Duration::from_millis(5));
    }
    let store = RunStore::new(&dir);
    let kept: Vec<String> = store
        .runs()
        .into_iter()
        .map(|meta| meta.run.to_string())
        .collect();
    assert_eq!(kept, ["r3", "r2"]);
    assert!(!dir.join("r1.jsonl.gz").exists());
    assert!(!dir.join("r1.json").exists());
    assert_eq!(store.last().unwrap().run, "r3");
    assert_eq!(store.find("r").len(), 2);
    assert_eq!(store.find("r2")[0].run, "r2");
    assert_eq!(store.find("x"), [] as [RunMeta; 0]);
}

#[test]
fn keeping_none_writes_nothing_and_deletes_what_was_kept() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 10, &run("old1", project.path(), OUTCOMES));
    keep(&dir, 10, &run("old2", project.path(), OUTCOMES));
    assert_eq!(keep(&dir, 0, &run("new1", project.path(), OUTCOMES)), None);
    let store = RunStore::new(&dir);
    assert_eq!(store.runs(), [] as [RunMeta; 0]);
    assert!(store.last().is_none());
    assert_eq!(names(&dir), [] as [String; 0]);
    // Without a store, keeping none creates nothing.
    let other = project.path().join("elsewhere");
    assert_eq!(
        keep(&other, 0, &run("new2", project.path(), OUTCOMES)),
        None
    );
    assert!(!other.exists());
}

#[test]
fn two_runs_at_once_each_keep_their_own_log() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let root = project.path().to_path_buf();
    let threads: Vec<_> = ["fast", "slow"]
        .into_iter()
        .map(|id| {
            let dir = dir.clone();
            let root = root.clone();
            std::thread::spawn(move || {
                let mut writer = RunWriter::new(&dir, 10);
                for record in &run(id, &root, OUTCOMES) {
                    writer.record(record);
                    if id == "slow" {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                (id, writer.finish(BTreeMap::new()).unwrap().unwrap())
            })
        })
        .collect();
    let ids: BTreeMap<&str, StoredRunId> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let mut numbers: Vec<String> = ids.values().map(StoredRunId::to_string).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, ["r1", "r2"]);
    let store = RunStore::new(&dir);
    assert_eq!(store.runs().len(), 2);
    for (id, stored) in &ids {
        assert_eq!(
            store.log(&stored.to_string()).unwrap(),
            run(id, &root, OUTCOMES)
        );
    }
    // `last` is the one that finished last.
    assert_eq!(store.last().unwrap().run, ids["slow"]);
}

#[test]
fn last_is_replaced_whole() {
    let dir = tempfile::tempdir().unwrap();
    for id in ["one", "two", "three"] {
        replace(dir.path(), LAST, format!("{id}\n").as_bytes()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(LAST)).unwrap(),
            format!("{id}\n")
        );
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn a_log_cut_short_reads_as_far_as_it_goes() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let records = run("cccc", project.path(), OUTCOMES);
    let id = keep(&dir, 10, &records).unwrap();
    let path = dir.join(format!("{id}.jsonl.gz"));
    let whole = fs::read(&path).unwrap();
    fs::write(&path, &whole[..whole.len() - 20]).unwrap();
    let store = RunStore::new(&dir);
    let read = store.log(&id).unwrap();
    assert!(!read.is_empty() && read.len() <= records.len());
    assert_eq!(read[..], records[..read.len()]);
    // As stored: as many whole lines.
    let mut copied = Vec::new();
    store.copy_log(&id, &mut copied).unwrap();
    let copied = String::from_utf8(copied).unwrap();
    assert!(copied.ends_with('\n'));
    assert_eq!(copied.lines().count(), read.len());
}

#[test]
fn the_log_is_copied_as_stored() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let records = run("cccc", project.path(), OUTCOMES);
    let id = keep(&dir, 10, &records).unwrap();
    let mut copied = Vec::new();
    RunStore::new(&dir).copy_log(&id, &mut copied).unwrap();
    let mut expected = Vec::new();
    for record in &records {
        serde_json::to_writer(&mut expected, record).unwrap();
        expected.push(b'\n');
    }
    assert_eq!(
        String::from_utf8(copied).unwrap(),
        String::from_utf8(expected).unwrap()
    );
    let store = RunStore::new(&dir);
    assert!(store.copy_log("../x", &mut Vec::new()).is_err());
    assert!(store.copy_log("r9", &mut Vec::new()).is_err());
}

#[test]
fn what_runs_that_never_completed_left_goes_once_a_day_old() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 10, &run("done", project.path(), OUTCOMES));
    // A log a killed aye left, and one a running aye is writing.
    for name in ["r5.jsonl.gz", "r6.jsonl.gz", ".tmp-x.tmp"] {
        fs::write(dir.join(name), b"").unwrap();
    }
    let old = SystemTime::now() - Duration::from_hours(48);
    for name in ["r5.jsonl.gz", ".tmp-x.tmp"] {
        fs::File::options()
            .write(true)
            .open(dir.join(name))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
    // Their numbers are taken.
    assert_eq!(
        keep(&dir, 10, &run("next", project.path(), OUTCOMES)).as_deref(),
        Some("r7")
    );
    assert!(!dir.join("r5.jsonl.gz").exists());
    assert!(!dir.join(".tmp-x.tmp").exists());
    assert!(dir.join("r6.jsonl.gz").exists());
    // Neither is a run.
    let store = RunStore::new(&dir);
    assert_eq!(store.runs().len(), 2);
    assert!(store.meta("r6").is_none());
}

#[test]
fn whatever_id_the_records_carry_nothing_is_written_outside_the_store() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    assert_eq!(
        keep(&dir, 10, &run("../evil", project.path(), OUTCOMES)).as_deref(),
        Some("r1")
    );
    assert!(!project.path().join(".aye/evil.jsonl.gz").exists());
    let store = RunStore::new(&dir);
    assert!(store.log("../x").is_err());
    assert!(store.meta("../x").is_none());
    // A `last` naming something else is no run.
    fs::write(dir.join(LAST), "../../etc/passwd\n").unwrap();
    assert_eq!(store.last().unwrap().run, "r1");
    fs::remove_file(dir.join("r1.json")).unwrap();
    assert!(store.last().is_none());
}

#[test]
fn a_summary_of_another_version_is_not_listed() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 10, &run("dddd", project.path(), OUTCOMES));
    let path = dir.join("r1.json");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, text.replace("\"version\":1", "\"version\":99")).unwrap();
    let store = RunStore::new(&dir);
    assert_eq!(store.runs(), [] as [RunMeta; 0]);
    assert!(store.last().is_none());
}

#[test]
fn a_store_unwritable_is_an_error_for_the_caller_to_report() {
    let project = project();
    // A file where the directory should be.
    let dir = project.path().join(".aye");
    fs::write(&dir, b"not a directory").unwrap();
    let mut writer = RunWriter::new(dir.join("runs"), 10);
    for record in &run("eeee", project.path(), OUTCOMES) {
        writer.record(record);
    }
    assert!(writer.finish(BTreeMap::new()).is_err());
    // Claimed before the run started: no number, and the same error.
    let mut writer = RunWriter::new(dir.join("runs"), 10);
    assert_eq!(writer.claim(), None);
    for record in &run("eeee", project.path(), OUTCOMES) {
        writer.record(record);
    }
    assert!(writer.finish(BTreeMap::new()).is_err());
}

/// A run of `outcomes` whose log also holds `events`, before its
/// `run_finished`, which says `exit_code`.
fn run_with(
    id: &str,
    root: &Path,
    outcomes: &[(&str, Outcome)],
    events: Vec<Event>,
    exit_code: ExitCode,
) -> Vec<Record> {
    let mut records = run(id, root, outcomes);
    let finished = records.pop().unwrap();
    let mut seq = finished.envelope.seq.unwrap();
    for event in events {
        let mut record = Record::new(event);
        record.envelope.run = Some(id.into());
        record.envelope.seq = Some(seq);
        seq += 1;
        records.push(record);
    }
    let mut finished = finished;
    finished.envelope.seq = Some(seq);
    if let Event::RunFinished {
        exit_code: code, ..
    } = &mut finished.event
    {
        *code = exit_code;
    }
    records.push(finished);
    records
}

fn error(code: &str, path: &str) -> Event {
    Event::Diagnostic(Diagnostic {
        severity: Severity::Error,
        code: code.into(),
        path: path.into(),
        span: None,
        message: format!("{code} at {path}"),
    })
}

/// Every diagnostic code aye 0.0.1 knows, and one it doesn't, with whether
/// an error with it gives a run something to show, before any batch and
/// from one.
const SHOWS: [(&str, bool, bool); 26] = [
    ("syntax-error", true, true),
    ("unreadable", true, true),
    ("not-found", false, true),
    ("unclaimed", false, false),
    ("duplicate-id", true, true),
    ("no-tests", true, true),
    ("not-refined", true, true),
    ("refine-failed", true, true),
    ("class-has-init", true, true),
    ("duplicate-definition", true, true),
    ("collection-error", true, true),
    ("usage-error", true, true),
    ("unknown-option", true, true),
    ("plugin-not-loaded", true, true),
    ("internal-error", true, true),
    ("skipped", true, true),
    ("ignored", true, true),
    ("batch-stopped", true, true),
    ("session-failed", true, true),
    ("protocol-error", true, true),
    ("batch-failed", true, true),
    ("no-executor", true, true),
    ("listing-differs", true, true),
    ("stand-in", true, true),
    ("adapter-error", true, true),
    ("some-other-code", true, true),
];

#[test]
fn every_code_shows_as_before() {
    let known: Vec<&str> = SHOWS[..25].iter().map(|(code, ..)| *code).collect();
    let all: Vec<&str> = DiagnosticCode::ALL
        .iter()
        .map(DiagnosticCode::as_str)
        .collect();
    assert_eq!(known, all);
    for (code, before, from_batch) in SHOWS {
        let mut record = Record::new(error(code, "tests/t.py"));
        let before_batches = shows(&record);
        record.envelope.batch = Some(BatchIndex::from_u32(0));
        assert_eq!(
            (before_batches, shows(&record)),
            (before, from_batch),
            "{code}"
        );
    }
}

#[test]
fn a_run_with_nothing_to_show_leaves_no_trace() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    keep(&dir, 2, &run("real", project.path(), OUTCOMES));
    let store = RunStore::new(&dir);
    // A path that doesn't exist, as the runner says before any batch, or
    // a `-k` that matches nothing: no test ran, nothing failed.
    let typo = run_with(
        "typo",
        project.path(),
        &[],
        vec![error("not-found", "tests/typo.py")],
        ExitCode::UsageError,
    );
    assert_eq!(keep(&dir, 2, &typo), None);
    let unclaimed = run_with(
        "unclaimed",
        project.path(),
        &[],
        vec![error("unclaimed", "x.txt")],
        ExitCode::UsageError,
    );
    assert_eq!(keep(&dir, 2, &unclaimed), None);
    assert_eq!(keep(&dir, 2, &run("nothing", project.path(), &[])), None);
    // Nothing of them is left, their numbers included.
    assert_eq!(names(&dir), ["last", "r1.json", "r1.jsonl.gz"]);
    // `last` is still the real run, which retention kept.
    assert_eq!(store.last().unwrap().run, "r1");
    assert_eq!(store.runs().len(), 1);
    // What did go wrong is kept, though no test ran: a collection error,
    // a batch's `not-found`, a failed lifetime or worker.
    let broken = run_with(
        "broken",
        project.path(),
        &[],
        vec![error("syntax-error", "tests/test_c.py")],
        ExitCode::Interrupted,
    );
    assert_eq!(keep(&dir, 10, &broken).as_deref(), Some("r2"));
    let mut missing = run_with(
        "missing",
        project.path(),
        &[],
        vec![error("not-found", "tests/test_a.py::test_z")],
        ExitCode::Interrupted,
    );
    for record in &mut missing {
        if matches!(record.event, Event::Diagnostic(_)) {
            record.envelope.batch = Some(BatchIndex::from_u32(0));
        }
    }
    assert_eq!(keep(&dir, 10, &missing).as_deref(), Some("r3"));
    let teardown = Event::LifetimeFinished {
        lifetime: "tests/test_a.py::T".into(),
        failure: Some(Failure {
            phase: Some(Phase::Teardown),
            message: "boom".into(),
            ..Failure::default()
        }),
        duration: Duration::ZERO,
    };
    let lifetime = run_with(
        "lifetime",
        project.path(),
        &[],
        vec![teardown],
        ExitCode::TestsFailed,
    );
    assert_eq!(keep(&dir, 10, &lifetime).as_deref(), Some("r4"));
    let crashed = Event::WorkerFinished {
        exit_status: Some(-9),
        error: Some("killed".into()),
    };
    let worker = run_with(
        "worker",
        project.path(),
        &[],
        vec![crashed],
        ExitCode::InternalError,
    );
    assert_eq!(keep(&dir, 10, &worker).as_deref(), Some("r5"));
}

#[test]
fn what_the_summary_counts_beside_the_tests_is_kept() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    let mut writer = RunWriter::new(&dir, 10);
    for record in &run("ffff", project.path(), OUTCOMES) {
        writer.record(record);
    }
    let extra = BTreeMap::from([
        ("teardown_error".to_owned(), 1),
        ("skipped_modules".to_owned(), 2),
    ]);
    writer.finish(extra.clone()).unwrap();
    assert_eq!(RunStore::new(&dir).meta("r1").unwrap().extra, extra);
    // Left out when there is none, and read as none when absent.
    keep(&dir, 10, &run("gggg", project.path(), OUTCOMES));
    let text = fs::read_to_string(dir.join("r2.json")).unwrap();
    assert!(!text.contains("extra"), "{text}");
    assert!(RunStore::new(&dir).meta("r2").unwrap().extra.is_empty());
}

#[test]
fn last_is_listed_first_and_the_newest_run_stands_for_it_when_gone() {
    let project = project();
    let dir = project.path().join(".aye/runs");
    for _ in 0..3 {
        keep(&dir, 10, &run("any", project.path(), OUTCOMES));
        std::thread::sleep(Duration::from_millis(5));
    }
    let store = RunStore::new(&dir);
    let ids = |store: &RunStore| -> Vec<String> {
        store
            .runs()
            .into_iter()
            .map(|meta| meta.run.to_string())
            .collect()
    };
    assert_eq!(ids(&store), ["r3", "r2", "r1"]);
    // Two runs finishing at once: the one that renamed `last` last is the
    // last, though its summary says it finished first.
    fs::write(dir.join(LAST), "r2\n").unwrap();
    assert_eq!(ids(&store), ["r2", "r3", "r1"]);
    assert_eq!(store.last().unwrap().run, "r2");
    // The run `last` names is gone: the newest stored run stands for it.
    fs::remove_file(dir.join("r2.json")).unwrap();
    assert_eq!(store.last().unwrap().run, "r3");
    assert_eq!(ids(&store), ["r3", "r1"]);
    fs::remove_file(dir.join(LAST)).unwrap();
    assert_eq!(store.last().unwrap().run, "r3");
    // None stored: none.
    for id in ["r1", "r3"] {
        fs::remove_file(dir.join(format!("{id}.json"))).unwrap();
    }
    assert!(store.last().is_none());
}
