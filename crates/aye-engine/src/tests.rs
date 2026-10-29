use aye_events::{Event, Outcome};

use super::*;

#[test]
fn requests_round_trip() {
    let requests = [
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            root: "/repo".into(),
            modules: vec![ENGINE_MODULE.into()],
            executors: vec![ExecutorSpec {
                adapter: "pytest".into(),
                entry: "aye_engine_pytest:PytestExecutor".into(),
                lists_session: true,
            }],
            services: vec![services::CONTEXT.into()],
        },
        Request::Refine(RefineRequest {
            adapter: "pytest".into(),
            files: vec!["tests/test_dynamic.py".into()],
            context: serde_json::json!({"rootdir": "/repo"}),
            digests_only: false,
        }),
        Request::Refine(RefineRequest {
            adapter: "pytest".into(),
            files: vec!["tests/test_dynamic.py".into()],
            context: serde_json::Value::Null,
            digests_only: true,
        }),
        Request::Run(Batch {
            id: 3,
            adapter: "pytest".into(),
            ids: vec![TestId::new("tests/t.py::test_x")],
            policy: Policy {
                maxfail: Some(1),
                capture: true,
                keyword: Some("login".into()),
                ..Policy::default()
            },
            context: serde_json::Value::Null,
            ends_session: false,
        }),
        Request::Run(Batch {
            id: 4,
            adapter: "pytest".into(),
            ids: Vec::new(),
            policy: Policy::default(),
            context: serde_json::Value::Null,
            ends_session: true,
        }),
        Request::Shutdown,
    ];
    for request in requests {
        let line = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<Request>(&line).unwrap(),
            request,
            "{line}"
        );
    }
}

#[test]
fn worker_lines_are_replies_or_records() {
    let ready =
        r#"{"reply":"ready","protocol":1,"pid":7,"python":"/usr/bin/python3","version":"3.13.1"}"#;
    assert!(matches!(
        serde_json::from_str::<WorkerLine>(ready).unwrap(),
        WorkerLine::Reply(Reply::Ready { pid: 7, .. })
    ));
    let event = r#"{"batch":0,"t_ms":1.5,"event":"test_finished","id":"t.py::test_x","outcome":"passed","duration_ms":0.2}"#;
    let WorkerLine::Record(record) = serde_json::from_str::<WorkerLine>(event).unwrap() else {
        panic!("expected a record");
    };
    assert_eq!(record.envelope.batch, Some(0));
    let Event::TestFinished(result) = record.event else {
        panic!("expected test_finished");
    };
    assert_eq!(result.outcome, Outcome::Passed);
    let done = r#"{"reply":"batch_done","batch":0,"status":1}"#;
    assert!(matches!(
        serde_json::from_str::<WorkerLine>(done).unwrap(),
        WorkerLine::Reply(Reply::BatchDone {
            batch: 0,
            status: Some(1)
        })
    ));
}

#[test]
fn session_listing_fields_are_left_out_when_unused_and_default_to_off() {
    let request = RefineRequest {
        adapter: "pytest".into(),
        files: vec!["t.py".into()],
        context: serde_json::Value::Null,
        digests_only: false,
    };
    let line = serde_json::to_string(&request).unwrap();
    assert!(!line.contains("digests_only"), "{line}");
    let read: RefineRequest =
        serde_json::from_str(r#"{"adapter":"pytest","files":["t.py"]}"#).unwrap();
    assert_eq!(read, request);
    let listed = serde_json::to_string(&RefineRequest {
        digests_only: true,
        ..request
    })
    .unwrap();
    assert!(listed.contains(r#""digests_only":true"#), "{listed}");

    let spec: ExecutorSpec = serde_json::from_str(r#"{"adapter":"a","entry":"m:C"}"#).unwrap();
    assert!(!spec.lists_session);
    assert!(
        !serde_json::to_string(&spec)
            .unwrap()
            .contains("lists_session")
    );

    let refined = Refined::default();
    assert_eq!(
        serde_json::to_string(&refined).unwrap(),
        r#"{"cases":[],"diagnostics":[]}"#
    );
    let digests = r#"{"reply":"refined","cases":[],"digests":{"a.py":"da39","b.py":"5ba9"}}"#;
    let WorkerLine::Reply(Reply::Refined(refined)) = serde_json::from_str(digests).unwrap() else {
        panic!("expected refined");
    };
    let digests = refined.digests.unwrap();
    assert_eq!(digests.keys().collect::<Vec<_>>(), ["a.py", "b.py"]);
    assert_eq!(digests["a.py"], "da39");
}

#[test]
fn policy_defaults_to_capturing() {
    let policy: Policy = serde_json::from_str("{}").unwrap();
    assert!(policy.capture);
    assert_eq!(policy.maxfail, None);
    assert_eq!(Policy::default(), policy, "Rust and wire defaults agree");
}

#[test]
fn source_files_turn_ranges_into_spans() {
    let source = "import pytest\n\n\ndef test_x():\n    pass\n";
    let parsed = ruff_python_parser::parse_module(source).unwrap();
    let lines = LineIndex::from_source_text(source);
    let path = Path::new("/repo/t.py");
    let file = SourceFile {
        rel: ProjectPathRef::as_written("t.py"),
        abs: path,
        source,
        module: parsed.syntax(),
        lines: &lines,
    };
    let func = file.module.body[1].as_function_def_stmt().unwrap();
    let span = file.span(func.name.range());
    assert_eq!(span.line, Some(4));
    assert_eq!(file.text(func.name.range()), "test_x");
    assert_eq!((span.start, span.end), (20, 26));
}

#[test]
fn the_engine_module_is_embedded() {
    assert_eq!(ENGINE_MODULE.name, "aye_engine");
    assert!(ENGINE_MODULE.source.contains("class EngineExecutor"));
    assert!(
        ENGINE_MODULE
            .source
            .contains(&format!("PROTOCOL = {PROTOCOL_VERSION}"))
    );
}

/// A batch ends its worker's session only when it says so: the field is
/// left out otherwise, and a batch without it doesn't.
#[test]
fn a_batch_ends_the_session_only_when_it_says_so() {
    let batch = Batch {
        id: 1,
        adapter: "pytest".into(),
        ids: vec![TestId::new("t.py::test_x")],
        policy: Policy::default(),
        context: serde_json::Value::Null,
        ends_session: false,
    };
    let line = serde_json::to_string(&Request::Run(batch.clone())).unwrap();
    assert!(!line.contains("ends_session"), "{line}");
    let ending = Batch {
        ends_session: true,
        ..batch
    };
    let line = serde_json::to_string(&Request::Run(ending)).unwrap();
    assert!(line.contains(r#""ends_session":true"#), "{line}");
    let without: Batch = serde_json::from_str(
        r#"{"id":1,"adapter":"pytest","ids":[],"policy":{"capture":true,"verbosity":0}}"#,
    )
    .unwrap();
    assert!(!without.ends_session);
}
