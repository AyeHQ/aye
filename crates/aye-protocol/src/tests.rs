use std::borrow::Cow;

use aye_events::{Event, Outcome};

use super::*;

/// A module as the handshake carries one. aye's engine is `aye-engine`'s,
/// which this crate can't name.
const MODULE: ModuleSource = ModuleSource::embedded("aye_engine", "PROTOCOL = 3\n");

#[test]
fn requests_round_trip() {
    let requests = [
        Request::Hello {
            protocol: PROTOCOL_VERSION,
            root: "/repo".into(),
            modules: vec![MODULE],
            executors: vec![ExecutorSpec {
                adapter: "pytest".into(),
                entry: EntryPoint::new("aye_engine_pytest", "PytestExecutor"),
                session: SessionMode::ListsSession,
            }],
            services: vec![ServiceName::Context],
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

/// A set lists its modules in install order, and a module copied out of
/// it, as a handshake copies each, still borrows the binary's source; read
/// from the wire, it is written the same.
#[test]
fn a_module_set_lends_its_modules_in_order() {
    const SET: ModuleSet = ModuleSet::new(
        const {
            &[
                ModuleSource::embedded("aye_model", ""),
                ModuleSource::embedded("aye_native_engine", "import aye_engine\n"),
            ]
        },
    );
    assert_eq!(
        SET.names().collect::<Vec<_>>(),
        ["aye_model", "aye_native_engine"]
    );
    let embedded = &SET.modules()[1];
    let copied = embedded.clone();
    assert!(
        matches!((&embedded.source, &copied.source), (Cow::Borrowed(a), Cow::Borrowed(b)) if std::ptr::eq(*a, *b))
    );
    let line = serde_json::to_string(&copied).unwrap();
    assert_eq!(
        line,
        r#"{"name":"aye_native_engine","source":"import aye_engine\n"}"#
    );
    let read: ModuleSource = serde_json::from_str(&line).unwrap();
    assert_eq!(read, copied);
    assert_eq!(ModuleSet::default().iter().count(), 0);
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
    assert_eq!(spec.session, SessionMode::PerBatch);
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

/// Every service, the compiler checking that none is missing, is written
/// as its name, and read back from it.
#[test]
fn a_service_is_written_as_its_name() {
    for service in ServiceName::ALL {
        match service {
            ServiceName::Capture
            | ServiceName::Context
            | ServiceName::Cleanup
            | ServiceName::Coverage
            | ServiceName::Sandbox
            | ServiceName::Timeout
            | ServiceName::Resources
            | ServiceName::Patching
            | ServiceName::Clock
            | ServiceName::Randomness
            | ServiceName::Network
            | ServiceName::Filesystem
            | ServiceName::Snapshots
            | ServiceName::Data => {}
        }
        let line = serde_json::to_string(&service).unwrap();
        assert_eq!(line, format!("\"{service}\""));
        assert_eq!(serde_json::from_str::<ServiceName>(&line).unwrap(), service);
    }
    let names: Vec<&str> = ServiceName::ALL.iter().map(|s| s.as_str()).collect();
    assert_eq!(
        names,
        [
            "capture",
            "context",
            "cleanup",
            "coverage",
            "sandbox",
            "timeout",
            "resources",
            "patching",
            "clock",
            "randomness",
            "network",
            "filesystem",
            "snapshots",
            "data"
        ]
    );
    assert_eq!(
        ServiceName::DEFAULT,
        [ServiceName::Context, ServiceName::Cleanup]
    );
}

/// An executor's entry is written `module:Class`, and read back split at
/// the first colon, as the worker splits it; whether it lists the session
/// is the handshake's `lists_session`.
#[test]
fn a_runtime_is_named_to_the_worker_by_its_spec() {
    const RUNTIME: Runtime = Runtime {
        adapter: AdapterName::new_static("listing"),
        modules: ModuleSet::new(const { &[ModuleSource::embedded("listing", "")] }),
        entry: EntryPoint::new("listing", "ListingExecutor"),
        session: SessionMode::ListsSession,
    };
    let spec = RUNTIME.spec();
    assert_eq!(RUNTIME.entry.to_string(), "listing:ListingExecutor");
    assert_eq!(
        (RUNTIME.entry.module(), RUNTIME.entry.class()),
        ("listing", "ListingExecutor")
    );
    let line = serde_json::to_string(&spec).unwrap();
    assert_eq!(
        line,
        r#"{"adapter":"listing","entry":"listing:ListingExecutor","lists_session":true}"#
    );
    assert_eq!(serde_json::from_str::<ExecutorSpec>(&line).unwrap(), spec);
    let per_batch = Runtime {
        session: SessionMode::PerBatch,
        ..RUNTIME
    };
    assert_eq!(
        serde_json::to_string(&per_batch.spec()).unwrap(),
        r#"{"adapter":"listing","entry":"listing:ListingExecutor"}"#
    );
    let read: EntryPoint = serde_json::from_str(r#""m:C:D""#).unwrap();
    assert_eq!((read.module(), read.class()), ("m", "C:D"));
    assert!(serde_json::from_str::<EntryPoint>(r#""m""#).is_err());
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
