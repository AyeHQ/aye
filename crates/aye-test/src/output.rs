//! aye's output, read back: its exit code, stdout and stderr, its JSON
//! lines ([`json_lines`]), and those lines as the records a run's log would
//! hold ([`as_records`]), for a run that keeps no log.

use std::process::Output;

use serde_json::{Map, Value, json};

/// The exit code; a test's run is never killed by a signal it didn't send.
pub fn code(output: &Output) -> i32 {
    output.status.code().unwrap()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Each line of stdout, read as JSON (`--output json`); a line that isn't
/// fails the test.
pub fn json_lines(output: &Output) -> Vec<Value> {
    stdout(output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect()
}

/// What a run's JSON lines say, as the records its log would hold: each
/// case, diagnostic and failed worker, and the summary as
/// `collection_finished` and `run_finished`. For runs that keep no log
/// (collect-only, or nothing to show).
pub fn as_records(output: &Output) -> Vec<Value> {
    let mut log = Vec::new();
    for line in json_lines(output) {
        let record = match line["type"].as_str().unwrap_or_default() {
            "case" => json!({
                "event": "case_collected",
                "id": line["id"],
                "certainty": if line["partial"] == true { "partial" } else { "exact" },
            }),
            "collection_error" => json!({
                "event": "diagnostic",
                "severity": "error",
                "code": line["code"],
                "path": line["path"],
                "message": line["error"],
            }),
            "diagnostic" => json!({
                "event": "diagnostic",
                "severity": line["severity"],
                "code": line["code"],
                "path": line["path"],
                "message": line["message"],
            }),
            "worker_error" => json!({"event": "worker_finished", "error": line["error"]}),
            "summary" => {
                let count = |name: &str| line.get(name).cloned().unwrap_or(json!(0));
                log.push(json!({
                    "event": "collection_finished",
                    "selected": count("tests"),
                    "deselected": count("deselected"),
                }));
                let mut finished = Map::new();
                finished.insert("event".into(), json!("run_finished"));
                finished.insert("exit_code".into(), line["exit"].clone());
                finished.insert(
                    "counts".into(),
                    json!({
                        "passed": count("passed"),
                        "failed": count("failed"),
                        "error": count("errors"),
                        "skipped": count("skipped"),
                        "expected_failure": count("expected_failure"),
                        "unexpected_pass": count("unexpected_pass"),
                        "deselected": count("deselected"),
                    }),
                );
                if let Some(stopped) = line.get("stopped") {
                    finished.insert("stopped".into(), stopped.clone());
                }
                Value::Object(finished)
            }
            _ => continue,
        };
        log.push(record);
    }
    log
}

/// The records of `records` that are `kind` events, in order.
pub fn events<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records.iter().filter(|r| r["event"] == kind).collect()
}
