use aye_events::{Event, Outcome};

use super::*;

/// A module as the handshake carries one. aye's engine is `aye-engine`'s,
/// which this crate can't name.
const MODULE: EmbeddedModule = EmbeddedModule {
    name: "aye_engine",
    source: "PROTOCOL = 3\n",
};

#[test]
fn requests_round_trip() {
    let requests = [
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            root: "/repo".into(),
            modules: vec![MODULE.into()],
            executors: vec![ExecutorSpec {
                adapter: "pytest".into(),
                entry: "aye_engine_pytest:PytestExecutor".into(),
                lists_session: true,
            }],
            services: vec![services::CONTEXT.into()],
        },
        Request::Refine(RefineRequest {
            adapter: "pytest".into(),
            files: vec![ProjectPath::new("tests/test_dynamic.py").unwrap()],
            context: AdapterContext::from(serde_json::json!({"rootdir": "/repo"})).into(),
            digests_only: false,
        }),
        Request::Refine(RefineRequest {
            adapter: "pytest".into(),
            files: vec![ProjectPath::new("tests/test_dynamic.py").unwrap()],
            context: BatchContext::default(),
            digests_only: true,
        }),
        Request::Run(Batch {
            id: BatchIndex::from_u32(3),
            adapter: "pytest".into(),
            ids: vec![TestId::new("tests/t.py::test_x")],
            policy: Policy {
                maxfail: Some(1),
                capture: true,
                keyword: Some("login".into()),
                ..Policy::default()
            },
            context: BatchContext::default(),
            ends_session: false,
        }),
        Request::Run(Batch {
            id: BatchIndex::from_u32(4),
            adapter: "pytest".into(),
            ids: Vec::new(),
            policy: Policy::default(),
            context: BatchContext::default(),
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
    assert_eq!(record.envelope.batch, Some(BatchIndex::from_u32(0)));
    let Event::TestFinished(result) = record.event else {
        panic!("expected test_finished");
    };
    assert_eq!(result.outcome, Outcome::Passed);
    let done = r#"{"reply":"batch_done","batch":0,"status":1}"#;
    assert!(matches!(
        serde_json::from_str::<WorkerLine>(done).unwrap(),
        WorkerLine::Reply(Reply::BatchDone {
            batch,
            status: Some(status)
        }) if batch == BatchIndex::from_u32(0) && status == FrameworkStatus::TESTS_FAILED
    ));
}

#[test]
fn session_listing_fields_are_left_out_when_unused_and_default_to_off() {
    let request = RefineRequest {
        adapter: "pytest".into(),
        files: vec![ProjectPath::new("t.py").unwrap()],
        context: BatchContext::default(),
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
    assert_eq!(digests["a.py"].as_str(), "da39");
}

#[test]
fn policy_defaults_to_capturing() {
    let policy: Policy = serde_json::from_str("{}").unwrap();
    assert!(policy.capture);
    assert_eq!(policy.maxfail, None);
    assert_eq!(Policy::default(), policy, "Rust and wire defaults agree");
}

/// A batch ends its worker's session only when it says so: the field is
/// left out otherwise, and a batch without it doesn't.
#[test]
fn a_batch_ends_the_session_only_when_it_says_so() {
    let batch = Batch {
        id: BatchIndex::from_u32(1),
        adapter: "pytest".into(),
        ids: vec![TestId::new("t.py::test_x")],
        policy: Policy::default(),
        context: BatchContext::default(),
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

/// A batch's context as aye 0.0.1 sent it, byte for byte: the binary's
/// keys (as `aye/src/session.rs` added them, pinned there before they
/// moved here) only into an object, then the runner's `ran_ids` into an
/// object, which a `null` context becomes for it; every key in order.
#[test]
fn a_batch_context_is_the_adapters_with_the_keys_added() {
    let sent = |adapter: serde_json::Value, binary: bool, ran: bool| {
        let mut context = BatchContext::new(adapter.into());
        if binary {
            context.requested_ids = vec![TestId::new("tests/t.py::test_x[1]")];
            context.selected_paths = vec!["/repo/tests".into(), "/repo/b.py".into()];
            context.rerun_args = vec!["-c".into(), "alt.ini".into()];
        }
        if ran {
            context.ran_ids = vec![TestId::new("tests/t.py::test_w[0]")];
        }
        let line = serde_json::to_string(&context).unwrap();
        assert_eq!(line, context.to_wire().to_string());
        line
    };
    let object = serde_json::json!({"zeta": 1, "rootdir": "/repo", "roots": ["/x"]});
    assert_eq!(
        sent(object.clone(), true, false),
        r#"{"requested_ids":["tests/t.py::test_x[1]"],"rerun_args":["-c","alt.ini"],"rootdir":"/repo","roots":["/x"],"selected_paths":["/repo/tests","/repo/b.py"],"zeta":1}"#
    );
    assert_eq!(
        sent(object.clone(), false, false),
        r#"{"rootdir":"/repo","roots":["/x"],"zeta":1}"#
    );
    assert_eq!(
        sent(object, true, true),
        r#"{"ran_ids":["tests/t.py::test_w[0]"],"requested_ids":["tests/t.py::test_x[1]"],"rerun_args":["-c","alt.ini"],"rootdir":"/repo","roots":["/x"],"selected_paths":["/repo/tests","/repo/b.py"],"zeta":1}"#
    );
    assert_eq!(
        sent(serde_json::json!({}), true, false),
        r#"{"requested_ids":["tests/t.py::test_x[1]"],"rerun_args":["-c","alt.ini"],"selected_paths":["/repo/tests","/repo/b.py"]}"#
    );
    assert_eq!(sent(serde_json::json!({}), false, false), "{}");
    // Only `ran_ids` makes a `null` context an object.
    assert_eq!(sent(serde_json::Value::Null, true, false), "null");
    assert_eq!(sent(serde_json::Value::Null, false, false), "null");
    assert_eq!(
        sent(serde_json::Value::Null, false, true),
        r#"{"ran_ids":["tests/t.py::test_w[0]"]}"#
    );
    assert_eq!(
        sent(serde_json::Value::Null, true, true),
        r#"{"ran_ids":["tests/t.py::test_w[0]"]}"#
    );
    for other in [serde_json::json!("text"), serde_json::json!([1])] {
        assert_eq!(sent(other.clone(), true, true), other.to_string());
    }
    // An added key replaces the adapter's of the same name.
    assert_eq!(
        sent(serde_json::json!({"rerun_args": ["old"]}), true, false),
        r#"{"requested_ids":["tests/t.py::test_x[1]"],"rerun_args":["-c","alt.ini"],"selected_paths":["/repo/tests","/repo/b.py"]}"#
    );
    assert_eq!(
        sent(serde_json::json!({"rerun_args": ["old"]}), false, false),
        r#"{"rerun_args":["old"]}"#
    );
}

/// Read back, a context is all the adapter's, as written, so it is written
/// again byte for byte; contexts are equal when the worker gets the same.
#[test]
fn a_batch_context_reads_back_as_written() {
    for line in [
        r#"{"ran_ids":[],"requested_ids":["a"],"rootdir":"/repo"}"#,
        r#"{"ran_ids":["x"]}"#,
        "null",
        "{}",
    ] {
        let context: BatchContext = serde_json::from_str(line).unwrap();
        assert!(context.requested_ids.is_empty() && context.ran_ids.is_empty());
        assert_eq!(serde_json::to_string(&context).unwrap(), line);
    }
    let added = BatchContext {
        ran_ids: vec![TestId::new("x")],
        ..BatchContext::default()
    };
    let read: BatchContext = serde_json::from_str(r#"{"ran_ids":["x"]}"#).unwrap();
    assert_eq!(read, added);
    assert_ne!(read, BatchContext::default());
}

/// `serde_json` keeps no insertion order (no `preserve_order`), so a typed
/// context writes its keys sorted, as the objects built by hand before it.
#[test]
fn an_adapter_context_writes_its_keys_in_order() {
    #[derive(Serialize)]
    struct Typed {
        zeta: u32,
        alpha: Option<&'static str>,
        middle: Vec<&'static str>,
    }
    let context = AdapterContext::of(&Typed {
        zeta: 1,
        alpha: None,
        middle: vec!["m"],
    });
    assert_eq!(
        serde_json::to_string(&context).unwrap(),
        r#"{"alpha":null,"middle":["m"],"zeta":1}"#
    );
    assert_eq!(
        context.as_value(),
        &serde_json::json!({"zeta": 1, "middle": ["m"], "alpha": null})
    );
    assert_eq!(
        serde_json::to_string(&AdapterContext::default()).unwrap(),
        "null"
    );
}
