//! Golden fixtures of the worker protocol: every message as aye 0.0.1's
//! types serialized it, read back and written again byte for byte, so that
//! a change to a message's shape (a field's name, its place, when it is left
//! out, the tag) fails here.
//!
//! `fixtures/requests.jsonl` holds every [`Request`] (runner to worker),
//! `fixtures/replies.jsonl` every [`Reply`], and `fixtures/worker_lines.jsonl`
//! every kind of [`WorkerLine`] (worker to runner): the replies, then the
//! records the engine sends, with the partial envelope a worker stamps and
//! with none. Each field is set in one message and left at its default in
//! another.

use std::fs;
use std::path::Path;

use aye_protocol::{Reply, Request, WorkerLine};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Every line of the fixture `name` read as a `T` and written back as it
/// was.
fn assert_round_trips<T: Serialize + DeserializeOwned>(name: &str) -> Vec<T> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'), "{name}");
    text.lines()
        .enumerate()
        .map(|(at, line)| {
            let message: T =
                serde_json::from_str(line).unwrap_or_else(|e| panic!("{name}:{}: {e}", at + 1));
            assert_eq!(
                serde_json::to_string(&message).unwrap(),
                line,
                "{name}:{}",
                at + 1
            );
            message
        })
        .collect()
}

#[test]
fn requests_round_trip_byte_for_byte() {
    let requests: Vec<Request> = assert_round_trips("requests.jsonl");
    let kinds: Vec<&str> = requests
        .iter()
        .map(|request| match request {
            Request::Hello { .. } => "hello",
            Request::Refine(_) => "refine",
            Request::Run(_) => "run",
            Request::Shutdown => "shutdown",
        })
        .collect();
    for kind in ["hello", "refine", "run", "shutdown"] {
        assert!(kinds.contains(&kind), "no {kind} in {kinds:?}");
    }
}

#[test]
fn replies_round_trip_byte_for_byte() {
    let replies: Vec<Reply> = assert_round_trips("replies.jsonl");
    let kinds: Vec<&str> = replies
        .iter()
        .map(|reply| match reply {
            Reply::Ready { .. } => "ready",
            Reply::Refined(_) => "refined",
            Reply::BatchDone { .. } => "batch_done",
            Reply::Failed { .. } => "failed",
        })
        .collect();
    for kind in ["ready", "refined", "batch_done", "failed"] {
        assert!(kinds.contains(&kind), "no {kind} in {kinds:?}");
    }
}

#[test]
fn worker_lines_round_trip_byte_for_byte() {
    let lines: Vec<WorkerLine> = assert_round_trips("worker_lines.jsonl");
    let replies = lines
        .iter()
        .filter(|line| matches!(line, WorkerLine::Reply(_)))
        .count();
    let records = lines.len() - replies;
    assert_eq!((replies, records), (8, 8));
}
