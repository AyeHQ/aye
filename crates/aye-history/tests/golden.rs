//! Golden fixtures of what aye 0.0.1 stores in `.aye`: each run's summary
//! (`runs/r<N>.json`, [`RunMeta`]), the history file (`history.json`) and
//! a whole run store, read back and written again byte for byte, so that
//! a change to either format (a field's name, its place, when it is left
//! out, how a number is written) fails here, and stores 0.0.1 left stay
//! readable.
//!
//! `fixtures/runs/<project>-r<N>.json` and `fixtures/history/<project>.json`
//! are what the 0.0.1 binary stored on copies of the harness suites
//! `outcomes` (three runs, the last with `-x`), `unittest` (two, the second
//! with `-k`), `pytest-doctest` and `dynamic`, the `doctest` example, and a
//! project with a syntax error and a failed `tearDownClass`, with the
//! project root replaced by `/project`. `every-field.json` and
//! `legacy-hex.json` were serialized by 0.0.1's `RunMeta`: every field set,
//! and a run stored under a hex id with no `run_finished`.
//! `fixtures/store/runs` is a store the 0.0.1 binary left, unchanged: one
//! run of the `rootdir` suite, its log and `last`.

use std::fs;
use std::path::{Path, PathBuf};

use aye_events::{Event, Record};
use aye_history::{History, RunMeta, RunStore};

fn fixtures(dir: &str) -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir);
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn run_summaries_round_trip_byte_for_byte() {
    let summaries = fixtures("runs");
    assert_eq!(summaries.len(), 11, "{summaries:?}");
    for path in summaries {
        let bytes = fs::read(&path).unwrap();
        let meta: RunMeta =
            serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        // As the store writes it: the summary, then a newline.
        let mut written = serde_json::to_vec(&meta).unwrap();
        written.push(b'\n');
        assert_eq!(
            String::from_utf8(written).unwrap(),
            String::from_utf8(bytes).unwrap(),
            "{}",
            path.display()
        );
    }
}

#[test]
fn history_files_round_trip_byte_for_byte() {
    let files = fixtures("history");
    assert_eq!(files.len(), 6, "{files:?}");
    for path in files {
        let bytes = fs::read(&path).unwrap();
        let from = tempfile::tempdir().unwrap();
        fs::write(from.path().join(aye_history::FILE), &bytes).unwrap();
        let history = History::load(from.path());
        assert!(!history.is_empty(), "{} loads as empty", path.display());
        let to = tempfile::tempdir().unwrap();
        history.save(to.path()).unwrap();
        let written = fs::read(to.path().join(aye_history::FILE)).unwrap();
        assert_eq!(
            String::from_utf8(written).unwrap(),
            String::from_utf8(bytes).unwrap(),
            "{}",
            path.display()
        );
    }
}

/// A store as 0.0.1 left it lists its run, gives its log record by record,
/// and copies the log as stored, every line of which round-trips.
#[test]
fn a_store_0_0_1_left_is_read() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/store/runs");
    let store = RunStore::new(&dir);
    let runs: Vec<String> = store.runs().into_iter().map(|meta| meta.run).collect();
    assert_eq!(runs, ["r1"]);
    let last = store.last().unwrap();
    assert_eq!(last.number(), Some(1));
    assert_eq!(last.root, "/private/tmp/aye-fixtures/rootdir");
    let records = store.log("r1").unwrap();
    assert_eq!(records.len(), 11);
    assert!(matches!(
        records[0].event,
        Event::RunStarted { run: Some(1), .. }
    ));
    let mut copied = Vec::new();
    store.copy_log("r1", &mut copied).unwrap();
    let copied = String::from_utf8(copied).unwrap();
    assert_eq!(copied.lines().count(), records.len());
    for line in copied.lines() {
        let record: Record = serde_json::from_str(line).unwrap();
        assert_eq!(serde_json::to_string(&record).unwrap(), line);
    }
}
