use std::collections::BTreeMap;
use std::time::Duration;

use crate::*;

fn round_trip(record: &Record) -> Record {
    let line = serde_json::to_string(record).unwrap();
    serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}"))
}

fn every_event() -> Vec<Event> {
    let id = TestId::new("tests/test_cart.py::TestCart::test_add[eur]");
    let failure = Failure {
        phase: Some(Phase::Test),
        exception: Some("AssertionError".into()),
        message: "assert 4 == 5".into(),
        assertion: Some(Assertion {
            op: "==".into(),
            left: "4".into(),
            right: "5".into(),
            diff: vec!["-4".into(), "+5".into()],
            ..Default::default()
        }),
        location: Some(Location {
            path: "tests/test_cart.py".into(),
            line: 12,
        }),
        frames: vec![Frame {
            path: "tests/test_cart.py".into(),
            line: 12,
            function: Some("test_add".into()),
            source: Some("assert add(2, 2) == 5".into()),
            ..Default::default()
        }],
        signature: "abc".into(),
        detail: None,
        ..Default::default()
    };
    let mut case = Case::new(id.as_str(), "pytest");
    case.tags = vec!["slow".into()];
    case.affinity = vec!["tests/test_cart.py".into()];
    case.certainty = Certainty::Partial;
    let mut result = TestResult::new(id.clone(), Outcome::Failed);
    result.duration = Duration::from_micros(1500);
    result.failures = vec![failure.clone()];
    result.output.stdout = "hello\n".into();
    result.reproduce = Some(Reproduce {
        command: vec!["aye".into(), id.as_str().to_owned()],
        inputs: BTreeMap::from([("seed".into(), "42".into())]),
    });
    result.properties = vec![("example_key".into(), "1".into())];
    result.body_duration = Some(Duration::from_micros(250));
    vec![
        Event::RunStarted {
            schema_version: SCHEMA_VERSION,
            aye_version: "0.0.1".into(),
            root: "/repo".into(),
            collect_only: false,
            workers: 2,
            run: None,
        },
        Event::CollectionStarted,
        Event::CollectionProgress {
            walked: 4096,
            test_files: 12,
            refining: 0,
        },
        Event::CollectionProgress {
            walked: 5000,
            test_files: 31,
            refining: 7,
        },
        Event::Diagnostic(Diagnostic {
            severity: Severity::Warning,
            code: "duplicate-definition".into(),
            path: ProjectPath::new("tests/test_cart.py").unwrap().into(),
            span: Some(Span {
                path: ProjectPath::new("tests/test_cart.py").unwrap(),
                start: 10,
                end: 20,
                line: Some(3),
            }),
            message: "test_add is defined twice".into(),
        }),
        Event::CaseCollected(Box::new(case)),
        Event::CollectionFinished {
            selected: 1,
            deselected: 0,
            refined_files: 0,
            duration: Duration::from_millis(3),
        },
        Event::WorkerStarted {
            pid: 42,
            python: "/usr/bin/python3".into(),
            version: Some("3.13.1 (main, Dec  3 2024, 17:59:52) [Clang 16.0.0 ]".into()),
        },
        Event::BatchStarted {
            adapter: "pytest".into(),
            cases: 1,
        },
        Event::LifetimeStarted {
            lifetime: "class:tests/test_cart.py::TestCart".into(),
            kind: Scope::Class,
        },
        Event::TestStarted { id: id.clone() },
        Event::SubOutcome {
            id: id.clone(),
            name: "i=1".into(),
            outcome: Outcome::Skipped,
            failure: None,
            reason: Some("not on Tuesdays".into()),
            duration: Duration::from_millis(1),
        },
        Event::TestFinished(Box::new(result)),
        Event::RunProperty {
            name: "ARCH".into(),
            value: "PPC".into(),
        },
        Event::LifetimeFinished {
            lifetime: "class:tests/test_cart.py::TestCart".into(),
            failure: Some(failure),
            duration: Duration::from_millis(2),
        },
        Event::BatchFinished { status: Some(1) },
        Event::WorkerFinished {
            exit_status: Some(0),
            error: None,
        },
        Event::RunFinished {
            exit_code: ExitCode::TestsFailed,
            counts: Counts {
                failed: 1,
                ..Counts::default()
            },
            duration: Duration::from_millis(40),
            stopped: None,
        },
    ]
}

#[test]
fn every_event_round_trips() {
    for event in every_event() {
        let record = Record {
            envelope: Envelope {
                run: Some("r1".into()),
                environment: Some("py313".into()),
                worker: Some(WorkerIndex::FIRST),
                batch: Some(BatchIndex::from_u32(0)),
                seq: Some(7),
                t_ms: Some(1.25),
            },
            event,
        };
        assert_eq!(round_trip(&record), record);
    }
}

/// The run's number in the store is not the envelope's `run` (its id): a
/// record, flat, has each key once.
#[test]
fn the_runs_number_and_its_id_are_two_keys() {
    let mut record = Record::new(Event::RunStarted {
        schema_version: SCHEMA_VERSION,
        aye_version: "0.0.1".into(),
        root: "/p".into(),
        collect_only: false,
        workers: 1,
        run: Some(81),
    });
    record.envelope.run = Some("e14d5f08d1554fbc".into());
    let line = serde_json::to_string(&record).unwrap();
    assert_eq!(
        line,
        r#"{"run":"e14d5f08d1554fbc","event":"run_started","schema_version":1,"aye_version":"0.0.1","root":"/p","workers":1,"run_number":81}"#
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn records_are_flat_and_internally_tagged() {
    let record = Record {
        envelope: Envelope {
            seq: Some(3),
            ..Envelope::default()
        },
        event: Event::TestStarted {
            id: TestId::new("t.py::test_x"),
        },
    };
    assert_eq!(
        serde_json::to_string(&record).unwrap(),
        r#"{"seq":3,"event":"test_started","id":"t.py::test_x"}"#
    );
}

#[test]
fn unknown_event_kinds_and_fields_are_tolerated() {
    let record: Record =
        serde_json::from_str(r#"{"seq":1,"event":"snapshot_written","path":"x"}"#).unwrap();
    assert_eq!(record.event, Event::Unknown);
    let record: Record = serde_json::from_str(
        r#"{"event":"test_started","id":"t.py::test_x","added_later":{"a":1}}"#,
    )
    .unwrap();
    assert_eq!(record.event.kind(), "test_started");
}

#[test]
fn properties_and_the_bodys_duration_are_optional_on_the_wire() {
    let line = r#"{"event":"test_finished","id":"t.py::test_x","outcome":"passed","duration_ms":2.5,"properties":[["example_key","1"],["url","http://x"]],"body_duration_ms":1.25}"#;
    let record: Record = serde_json::from_str(line).unwrap();
    let Event::TestFinished(result) = &record.event else {
        panic!("{record:?}");
    };
    assert_eq!(
        result.properties,
        [
            ("example_key".to_string(), "1".to_string()),
            ("url".to_string(), "http://x".to_string())
        ]
    );
    assert_eq!(result.body_duration, Some(Duration::from_micros(1250)));
    assert_eq!(serde_json::to_string(&record).unwrap(), line);

    // Absent or null: neither is written back.
    let line = r#"{"event":"test_finished","id":"t.py::test_x","outcome":"passed","duration_ms":2.5,"body_duration_ms":null}"#;
    let record: Record = serde_json::from_str(line).unwrap();
    let Event::TestFinished(result) = &record.event else {
        panic!("{record:?}");
    };
    assert_eq!(
        result.properties,
        [] as [(std::string::String, std::string::String); 0]
    );
    assert_eq!(result.body_duration, None);
    assert_eq!(
        serde_json::to_string(&record).unwrap(),
        r#"{"event":"test_finished","id":"t.py::test_x","outcome":"passed","duration_ms":2.5}"#
    );
}

#[test]
fn missing_or_null_durations_read_as_zero() {
    for line in [
        r#"{"event":"test_finished","id":"t.py::test_a","outcome":"passed"}"#,
        r#"{"event":"test_finished","id":"t.py::test_a","outcome":"passed","duration_ms":null}"#,
    ] {
        let record: Record = serde_json::from_str(line).unwrap();
        let Event::TestFinished(result) = record.event else {
            panic!("expected test_finished");
        };
        assert_eq!(result.duration, Duration::ZERO);
    }
}
