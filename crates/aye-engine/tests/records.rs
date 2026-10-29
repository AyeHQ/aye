//! The strict round trip of the engine's records: what `Sink`, `Result`
//! and `Failure` (`python/aye_engine.py`) send on a fixture set
//! (`records.py`), each record read as a Rust [`Record`], written back and
//! compared with what Python sent as JSON values. A field Python sends that
//! the Rust types don't have (misspelled, or never added) is silently
//! dropped by serde on the runner's side; here it is a failure.
//!
//! The comparison allows what serde's defaults make equal: a key one side
//! leaves out where the other has the type's default (`null`, `false`, 0,
//! `""`, `[]`, `{}`), as `skip_serializing_if` and `default` do. Numbers
//! compare by value.
//!
//! Runs the engine with the workspace's `.venv` (from `uv sync`), or
//! `AYE_TEST_PYTHON`; without either it is skipped, loudly.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use aye_engine::ENGINE_MODULE;
use aye_events::{Event, Record};
use serde_json::Value;

fn python() -> Option<PathBuf> {
    let candidate = std::env::var_os("AYE_TEST_PYTHON").map_or_else(
        || {
            let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.venv");
            if cfg!(windows) {
                venv.join("Scripts/python.exe")
            } else {
                venv.join("bin/python")
            }
        },
        PathBuf::from,
    );
    let ok = Command::new(&candidate)
        .args(["-c", "import sys; sys.exit(sys.version_info < (3, 10))"])
        .output()
        .is_ok_and(|o| o.status.success());
    if ok {
        Some(candidate)
    } else {
        eprintln!("skipping: no Python 3.10+ (run `uv sync`, or set AYE_TEST_PYTHON)");
        None
    }
}

/// What `records.py` writes, line by line, given the embedded engine.
fn python_records(python: &Path) -> Vec<String> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/records.py");
    let mut child = Command::new(python)
        .arg("-I")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(ENGINE_MODULE.source.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// A value serde leaves out, or reads in place of one left out.
fn is_default(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Where `sent` and `read` differ, but for serde's defaults; `at` is the
/// path to them.
fn differences(sent: &Value, read: &Value, at: &str, found: &mut Vec<String>) {
    match (sent, read) {
        (Value::Object(sent), Value::Object(read)) => {
            for (key, value) in sent {
                let at = format!("{at}.{key}");
                match read.get(key) {
                    Some(other) => differences(value, other, &at, found),
                    None if is_default(value) => {}
                    None => found.push(format!("{at}: sent {value}, dropped")),
                }
            }
            for (key, value) in read {
                if !sent.contains_key(key) && !is_default(value) {
                    found.push(format!("{at}.{key}: not sent, read as {value}"));
                }
            }
        }
        (Value::Array(sent), Value::Array(read)) if sent.len() == read.len() => {
            for (index, (one, other)) in sent.iter().zip(read).enumerate() {
                differences(one, other, &format!("{at}[{index}]"), found);
            }
        }
        (Value::Number(one), Value::Number(other)) if one.as_f64() == other.as_f64() => {}
        _ if sent == read => {}
        _ => found.push(format!("{at}: sent {sent}, read as {read}")),
    }
}

#[test]
fn every_record_the_engine_sends_reads_back_whole() {
    let Some(python) = python() else { return };
    let lines = python_records(&python);
    let mut kinds = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let sent: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {}: {e}: {line}", index + 1));
        let record: Record = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {}: {e}: {line}", index + 1));
        assert!(
            !matches!(record.event, Event::Unknown),
            "line {}: an unknown kind: {line}",
            index + 1
        );
        kinds.insert(record.event.kind());
        let read = serde_json::to_value(&record).unwrap();
        differences(&sent, &read, &format!("line {}", index + 1), &mut found);
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
    assert_eq!(
        kinds.into_iter().collect::<Vec<_>>(),
        [
            "diagnostic",
            "lifetime_finished",
            "lifetime_started",
            "run_property",
            "sub_outcome",
            "test_finished",
            "test_started",
        ]
    );
}

/// The comparison itself: a field serde drops, or reads otherwise, is
/// found; one left out where the other side has its default is not.
#[test]
fn the_comparison_finds_what_serde_drops() {
    let line = r#"{"batch":1,"t_ms":0.5,"event":"test_finished","id":"t.py::test_x","outcome":"failed","duration_ms":1.0,"failures":[{"phase":null,"message":"m","signature":"abc","site":"t.py","assertion":{"op":"==","left":"1","right":"2","diff":[]}}],"reasons":"typo"}"#;
    let sent: Value = serde_json::from_str(line).unwrap();
    let record: Record = serde_json::from_str(line).unwrap();
    let mut found = Vec::new();
    differences(
        &sent,
        &serde_json::to_value(&record).unwrap(),
        "line",
        &mut found,
    );
    assert_eq!(
        found,
        [
            r#"line.failures[0].site: sent "t.py", dropped"#,
            r#"line.reasons: sent "typo", dropped"#,
        ]
    );
}
