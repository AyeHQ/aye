//! Golden fixtures: records as aye 0.0.1 wrote them, read into the wire
//! types and written back byte for byte, so that a change to a type that
//! would change the event log on the wire (a field's name, its place, when
//! it is left out, how a number is written) fails here.
//!
//! `fixtures/every_kind.jsonl` holds every event kind, each field set in
//! one record and left at its default in another, serialized by 0.0.1's
//! types. `fixtures/logs/*.jsonl` are run logs the 0.0.1 binary stored
//! (`.aye/runs/r1.jsonl.gz`, uncompressed) on copies of the harness suites
//! `outcomes`, `unittest`, `pytest-doctest` and `dynamic`, the `doctest`
//! example, and a project with a syntax error and a failed `tearDownClass`.
//! Their absolute paths were replaced: the project root by `/project`, the
//! interpreter by `/usr/bin/python3` and its installation by `/usr`.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use aye_events::{Event, Record};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every line of `path` read as a [`Record`] and written back as it was.
fn assert_round_trips(path: &Path) -> Vec<Record> {
    let text = fs::read_to_string(path).unwrap();
    assert!(text.ends_with('\n'), "{}", path.display());
    text.lines()
        .enumerate()
        .map(|(at, line)| {
            let record: Record = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{}:{}: {e}", path.display(), at + 1));
            let written = serde_json::to_string(&record).unwrap();
            assert_eq!(written, line, "{}:{}", path.display(), at + 1);
            record
        })
        .collect()
}

#[test]
fn every_kind_round_trips_byte_for_byte() {
    let records = assert_round_trips(&fixtures().join("every_kind.jsonl"));
    let kinds: BTreeSet<&str> = records.iter().map(|r| r.event.kind()).collect();
    let expected = BTreeSet::from([
        "run_started",
        "collection_started",
        "collection_progress",
        "diagnostic",
        "case_collected",
        "collection_finished",
        "worker_started",
        "batch_started",
        "lifetime_started",
        "lifetime_finished",
        "test_started",
        "sub_outcome",
        "test_finished",
        "run_property",
        "batch_finished",
        "worker_finished",
        "run_finished",
        "unknown",
    ]);
    assert_eq!(kinds, expected);
}

#[test]
fn stored_logs_round_trip_byte_for_byte() {
    let mut logs: Vec<PathBuf> = fs::read_dir(fixtures().join("logs"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    logs.sort();
    assert_eq!(logs.len(), 6, "{logs:?}");
    for log in logs {
        let records = assert_round_trips(&log);
        assert!(
            matches!(
                records.first().map(|r| &r.event),
                Some(Event::RunStarted { .. })
            ),
            "{}",
            log.display()
        );
        assert!(
            matches!(
                records.last().map(|r| &r.event),
                Some(Event::RunFinished { .. })
            ),
            "{}",
            log.display()
        );
    }
}

/// A kind this version doesn't know, and fields it doesn't know, are read
/// rather than rejected: a newer aye's log stays readable.
#[test]
fn unknown_kinds_and_fields_are_read() {
    let record: Record = serde_json::from_str(
        r#"{"run":"cea3278d3c5cd9ca","seq":3,"event":"snapshot_written","path":"t.py","value":{"a":1}}"#,
    )
    .unwrap();
    assert_eq!(record.event, Event::Unknown);
    assert_eq!(record.envelope.seq, Some(3));
    let record: Record = serde_json::from_str(
        r#"{"event":"test_started","id":"t.py::test_x","attempt":2,"fakes":["clock"]}"#,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_string(&record).unwrap(),
        r#"{"event":"test_started","id":"t.py::test_x"}"#
    );
}
