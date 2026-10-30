//! The constants Python and Rust must agree on, read from the Python aye
//! embeds (parsed, not run): the outcomes and their severities, the
//! phases, the scopes, the frameworks' statuses, the service names and the
//! services the engine provides itself, the protocol version, the
//! comparisons an assertion makes and the kinds a comparison tells apart,
//! the keys of a batch's context that don't start another session, and
//! the diagnostic codes and severities Python emits. Each is written
//! once on each side, and a difference is a value one side sends that the
//! other misreads or drops.

use std::collections::{BTreeMap, BTreeSet};

use aye_adapter::ast::visitor::{self, Visitor};
use aye_adapter::ast::{DictItem, Expr, ModModule, Stmt};
use aye_events::{
    CompareKind, ComparisonOp, DiagnosticCode, FrameworkStatus, Outcome, Phase, Scope, Severity,
};
use aye_protocol::{ModuleSource, PROTOCOL_VERSION, ServiceName, context_keys};
use serde_json::{Value, json};

/// Every Python module aye embeds, once each: the engine's, each adapter's,
/// aye's `pytest` module, and the worker.
fn embedded() -> Vec<ModuleSource> {
    let mut modules = vec![
        aye_engine::ENGINE_MODULE,
        aye_engine::MODEL_MODULE,
        aye_engine::REWRITE_MODULE,
        aye_engine::NATIVE_ENGINE_MODULE,
        aye_adapter_pytest::embedded::SHIM_MODULE,
    ];
    modules.extend(aye_adapter_pytest::embedded::MODULES.iter().cloned());
    modules.push(aye_adapter_unittest::MODULE);
    modules.push(aye_adapter_doctest::MODULE);
    modules.push(ModuleSource::embedded(
        "aye_worker",
        aye_runner::WORKER_SOURCE,
    ));
    let mut seen = BTreeSet::new();
    modules.retain(|module| seen.insert(module.name.clone()));
    modules
}

fn parse(module: &ModuleSource) -> ModModule {
    ruff_python_parser::parse_module(&module.source)
        .unwrap_or_else(|e| panic!("{}: {e}", module.name))
        .into_syntax()
}

fn literal(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::StringLiteral(s) => Some(s.value.to_str()),
        _ => None,
    }
}

fn integer(expr: &Expr) -> Option<u64> {
    expr.as_number_literal_expr()
        .and_then(|n| n.value.as_int())
        .and_then(aye_adapter::ast::Int::as_u64)
}

/// The value of the module-level `name = <int>`.
fn module_int(module: &ModModule, name: &str) -> Option<u64> {
    module.body.iter().find_map(|stmt| match stmt {
        Stmt::Assign(assign)
            if matches!(&assign.targets[..], [Expr::Name(target)] if target.id.as_str() == name) =>
        {
            integer(&assign.value)
        }
        _ => None,
    })
}

/// The items of the module-level dict `name = {…}` (or `name: T = {…}`),
/// in order.
fn module_dict<'m>(module: &'m ModModule, name: &str) -> &'m [DictItem] {
    module
        .body
        .iter()
        .find_map(|stmt| {
            let (target, value) = match stmt {
                Stmt::Assign(assign) => (&assign.targets[..], &*assign.value),
                Stmt::AnnAssign(assign) => (
                    std::slice::from_ref(&*assign.target),
                    assign.value.as_deref()?,
                ),
                _ => return None,
            };
            match (target, value) {
                ([Expr::Name(target)], Expr::Dict(dict)) if target.id.as_str() == name => {
                    Some(&dict.items[..])
                }
                _ => None,
            }
        })
        .unwrap_or_else(|| panic!("no dict {name}"))
}

/// The `NAME = <value>` assignments in the body of the class `name` whose
/// value `read` reads, in order.
fn class_values<T>(
    module: &ModModule,
    name: &str,
    read: impl Fn(&Expr) -> Option<T>,
) -> Vec<(String, T)> {
    let class = module
        .body
        .iter()
        .find_map(|stmt| stmt.as_class_def_stmt().filter(|c| c.name.as_str() == name))
        .unwrap_or_else(|| panic!("no class {name}"));
    class
        .body
        .iter()
        .filter_map(|stmt| {
            let assign = stmt.as_assign_stmt()?;
            let [Expr::Name(target)] = &assign.targets[..] else {
                return None;
            };
            Some((target.id.to_string(), read(&assign.value)?))
        })
        .collect()
}

/// The `NAME = "value"` assignments in the body of the class `name`, in
/// order.
fn class_strings(module: &ModModule, name: &str) -> Vec<(String, String)> {
    class_values(module, name, |value| literal(value).map(str::to_owned))
}

fn engine() -> ModModule {
    parse(&aye_engine::ENGINE_MODULE)
}

/// Every outcome, the compiler checking that none is missing.
fn outcomes() -> [Outcome; 6] {
    let all = [
        Outcome::Passed,
        Outcome::Failed,
        Outcome::Error,
        Outcome::Skipped,
        Outcome::ExpectedFailure,
        Outcome::UnexpectedPass,
    ];
    for outcome in all {
        match outcome {
            Outcome::Passed
            | Outcome::Failed
            | Outcome::Error
            | Outcome::Skipped
            | Outcome::ExpectedFailure
            | Outcome::UnexpectedPass => {}
        }
    }
    all
}

/// A unit variant's name on the wire, from its `json!` value.
fn wire(value: &Value) -> String {
    value.as_str().unwrap().to_owned()
}

#[test]
fn outcomes_are_the_same_on_both_sides() {
    let python = class_strings(&engine(), "Outcome");
    let rust: Vec<(String, String)> = outcomes()
        .into_iter()
        .map(|outcome| (outcome.as_str().to_ascii_uppercase(), wire(&json!(outcome))))
        .collect();
    assert_eq!(python, rust);
    for outcome in outcomes() {
        assert_eq!(wire(&json!(outcome)), outcome.as_str());
    }
}

/// Python's `_SEVERITY`, by which the worse outcome of a test's phases
/// wins, ranks the outcomes as `Outcome::severity` does.
#[test]
fn outcome_severities_are_the_same_on_both_sides() {
    let module = engine();
    let table = module
        .body
        .iter()
        .find_map(|stmt| match stmt {
            Stmt::Assign(assign)
                if matches!(&assign.targets[..], [Expr::Name(t)] if t.id.as_str() == "_SEVERITY") =>
            {
                assign.value.as_dict_expr()
            }
            _ => None,
        })
        .expect("_SEVERITY");
    let python: BTreeMap<String, u64> = table
        .items
        .iter()
        .map(|item| {
            let key = item.key.as_ref().and_then(Expr::as_attribute_expr).unwrap();
            (key.attr.to_string(), integer(&item.value).unwrap())
        })
        .collect();
    let rust: BTreeMap<String, u64> = outcomes()
        .into_iter()
        .map(|o| (o.as_str().to_ascii_uppercase(), u64::from(o.severity())))
        .collect();
    assert_eq!(python, rust);
}

#[test]
fn phases_are_the_same_on_both_sides() {
    let all = [Phase::Setup, Phase::Test, Phase::Teardown];
    for phase in all {
        match phase {
            Phase::Setup | Phase::Test | Phase::Teardown => {}
        }
    }
    let rust: Vec<(String, String)> = all
        .into_iter()
        .map(|phase| {
            (
                wire(&json!(phase)).to_ascii_uppercase(),
                wire(&json!(phase)),
            )
        })
        .collect();
    assert_eq!(class_strings(&engine(), "Phase"), rust);
}

/// The model's `Scope`, an `IntEnum` whose order is that of `Scope`:
/// widest first.
#[test]
fn scopes_are_the_same_on_both_sides() {
    let rust: Vec<(String, u64)> = Scope::ALL
        .iter()
        .zip(0..)
        .map(|(scope, at)| (scope.as_str().to_ascii_uppercase(), at))
        .collect();
    assert_eq!(
        class_values(&parse(&aye_engine::MODEL_MODULE), "Scope", integer),
        rust
    );
}

/// The statuses a batch ends with, which the runner reads as
/// `FrameworkStatus`es: pytest's, as the pytest adapter's executor names
/// them, and unittest's, as `python -m unittest` exits.
#[test]
fn framework_statuses_are_the_same_on_both_sides() {
    let module = |name: &str| {
        let module = embedded().into_iter().find(|m| m.name == name).unwrap();
        parse(&module)
    };
    let number = |status: FrameworkStatus| u64::try_from(status.get()).ok();
    let pytest = module("aye_pytest_native");
    for (name, status) in [
        ("OK", FrameworkStatus::OK),
        ("TESTS_FAILED", FrameworkStatus::TESTS_FAILED),
        ("INTERRUPTED", FrameworkStatus::INTERRUPTED),
        ("INTERNAL_ERROR", FrameworkStatus::INTERNAL_ERROR),
        ("USAGE_ERROR", FrameworkStatus::USAGE_ERROR),
        ("NO_TESTS_COLLECTED", FrameworkStatus::NO_TESTS),
    ] {
        assert_eq!(module_int(&pytest, name), number(status), "pytest's {name}");
    }
    let unittest = module("aye_engine_unittest");
    for (name, status) in [
        ("OK", FrameworkStatus::OK),
        ("FAILED", FrameworkStatus::TESTS_FAILED),
        ("INTERRUPTED", FrameworkStatus::INTERRUPTED),
        ("NO_TESTS", FrameworkStatus::NO_TESTS),
    ] {
        assert_eq!(
            module_int(&unittest, name),
            number(status),
            "unittest's {name}"
        );
    }
}

/// `aye_engine.Services` names every `ServiceName`, in order, as the wire
/// writes it.
#[test]
fn service_names_are_the_same_on_both_sides() {
    let rust: Vec<(String, String)> = ServiceName::ALL
        .into_iter()
        .map(|service| (service.as_str().to_ascii_uppercase(), wire(&json!(service))))
        .collect();
    assert_eq!(class_strings(&engine(), "Services"), rust);
}

/// The services the runner asks every worker for are the ones the engine
/// provides itself (`default_services`): the worker drops any other name
/// without a word.
#[test]
fn the_default_services_are_the_ones_the_engine_provides() {
    let module = engine();
    let names: BTreeMap<String, String> = class_strings(&module, "Services").into_iter().collect();
    let function = module
        .body
        .iter()
        .find_map(|stmt| {
            stmt.as_function_def_stmt()
                .filter(|f| f.name.as_str() == "default_services")
        })
        .expect("default_services");
    let mut provided = Vec::new();
    for stmt in &function.body {
        let Some(call) = stmt
            .as_return_stmt()
            .and_then(|r| r.value.as_deref())
            .and_then(Expr::as_call_expr)
        else {
            continue;
        };
        let dict = call.arguments.args[0].as_dict_expr().unwrap();
        for item in &dict.items {
            let key = item.key.as_ref().and_then(Expr::as_attribute_expr).unwrap();
            provided.push(names[key.attr.as_str()].clone());
        }
    }
    assert_eq!(provided, ServiceName::DEFAULT.map(ServiceName::as_str));
}

#[test]
fn the_engine_and_the_worker_speak_the_runners_protocol() {
    assert_eq!(
        module_int(&engine(), "PROTOCOL"),
        Some(u64::from(PROTOCOL_VERSION))
    );
    let worker = ModuleSource::embedded("aye_worker", aye_runner::WORKER_SOURCE);
    assert_eq!(
        module_int(&parse(&worker), "PROTOCOL"),
        Some(u64::from(PROTOCOL_VERSION))
    );
}

/// The operators of a rewritten `assert` (the rewriter's `_COMPARISONS`)
/// are the first `ComparisonOp`s, in order, and with those of unittest's
/// `assert*` methods (the unittest adapter's `_COMPARISONS`, each the first
/// of its tuple) they are all of them. The kinds `aye_engine.compare` tells
/// apart (its `_COMPARERS`) are `CompareKind`'s, in order.
#[test]
fn comparisons_are_the_same_on_both_sides() {
    let module = |name: &str| {
        let module = embedded().into_iter().find(|m| m.name == name).unwrap();
        parse(&module)
    };
    let rewriter = parse(&aye_engine::REWRITE_MODULE);
    let rewriter: Vec<&str> = module_dict(&rewriter, "_COMPARISONS")
        .iter()
        .map(|item| literal(&item.value).unwrap())
        .collect();
    let unittest = module("aye_engine_unittest");
    let unittest: BTreeSet<&str> = module_dict(&unittest, "_COMPARISONS")
        .iter()
        .map(|item| {
            let tuple = item.value.as_tuple_expr().unwrap();
            literal(&tuple.elts[0]).unwrap()
        })
        .collect();
    let known: Vec<&str> = ComparisonOp::ALL.iter().map(ComparisonOp::as_str).collect();
    assert_eq!(rewriter, known[..rewriter.len()]);
    let mut python: BTreeSet<&str> = rewriter.iter().copied().collect();
    python.extend(&unittest);
    assert_eq!(
        python,
        known.iter().copied().collect(),
        "unittest: {unittest:?}"
    );
    let engine = engine();
    let kinds: Vec<&str> = module_dict(&engine, "_COMPARERS")
        .iter()
        .map(|item| literal(item.key.as_ref().unwrap()).unwrap())
        .collect();
    let rust: Vec<&str> = CompareKind::ALL.iter().map(CompareKind::as_str).collect();
    assert_eq!(kinds, rust);
}

/// The diagnostics a module builds: a call whose first two arguments are a
/// severity and a code (`sink.diagnostic("error", "usage-error", …)`), or
/// one to something named for a diagnostic whose second argument is a code
/// (`_Diagnostic(severity, "plugin-not-loaded", …)`), and a dict with a
/// `"code"` (`{"severity": …, "code": "internal-error", …}`). Codes held in
/// variables are found where they are written.
#[derive(Default)]
struct Diagnostics {
    codes: BTreeSet<String>,
    severities: BTreeSet<String>,
}

const SEVERITIES: [&str; 3] = ["error", "warning", "info"];

impl<'a> Visitor<'a> for Diagnostics {
    fn visit_expr(&mut self, expr: &'a Expr) {
        match expr {
            Expr::Call(call) => {
                let callee = match &*call.func {
                    Expr::Name(name) => name.id.as_str(),
                    Expr::Attribute(attribute) => attribute.attr.as_str(),
                    _ => "",
                };
                let args = &call.arguments.args;
                let severity = args.first().and_then(literal);
                let code = args.get(1).and_then(literal);
                let named = callee.to_ascii_lowercase().contains("diagnos");
                if let Some(code) = code
                    && (named || severity.is_some_and(|s| SEVERITIES.contains(&s)))
                {
                    self.codes.insert(code.to_owned());
                    if let Some(severity) = severity {
                        self.severities.insert(severity.to_owned());
                    }
                }
                visitor::walk_expr(self, expr);
            }
            Expr::Dict(dict) => {
                for item in &dict.items {
                    match item.key.as_ref().and_then(literal) {
                        Some("code") => {
                            if let Some(code) = literal(&item.value) {
                                self.codes.insert(code.to_owned());
                            }
                        }
                        Some("severity") => {
                            if let Some(severity) = literal(&item.value) {
                                self.severities.insert(severity.to_owned());
                            }
                        }
                        _ => {}
                    }
                }
                visitor::walk_expr(self, expr);
            }
            _ => visitor::walk_expr(self, expr),
        }
    }
}

#[test]
fn python_emits_only_codes_and_severities_rust_knows() {
    let mut found = Diagnostics::default();
    for module in embedded() {
        found.visit_body(&parse(&module).body);
    }
    let codes: Vec<&str> = found.codes.iter().map(String::as_str).collect();
    assert_eq!(
        codes,
        [
            "batch-stopped",
            "collection-error",
            "duplicate-id",
            "ignored",
            "internal-error",
            "not-found",
            "plugin-not-loaded",
            "session-failed",
            "skipped",
            "unknown-option",
            "usage-error",
        ],
        "the codes Python emits changed: check that Rust knows each"
    );
    for code in &codes {
        assert!(
            DiagnosticCode::ALL.contains(&DiagnosticCode::from(*code)),
            "{code}"
        );
    }
    let rust_severities: BTreeSet<String> = [Severity::Error, Severity::Warning, Severity::Info]
        .into_iter()
        .map(|severity| wire(&json!(severity)))
        .collect();
    assert_eq!(found.severities, rust_severities);
}

/// The keys of a batch's context that don't start another session are the
/// pytest executor's `BATCH_KEYS` (a set of the strings): a key the
/// executor keeps out of its session key that Rust doesn't call neutral, or
/// the other way round, would start a session per batch, or keep one across
/// batches that differ. `selected_paths` is in neither: other paths start
/// another session.
#[test]
fn the_session_neutral_keys_are_the_pytest_executors_batch_keys() {
    let module = parse(&aye_adapter_pytest::embedded::NATIVE_MODULE);
    let value = module
        .body
        .iter()
        .find_map(|stmt| match stmt {
            Stmt::Assign(assign)
                if matches!(&assign.targets[..], [Expr::Name(target)] if target.id.as_str() == "BATCH_KEYS") =>
            {
                Some(&*assign.value)
            }
            _ => None,
        })
        .expect("BATCH_KEYS");
    let call = value.as_call_expr().expect("frozenset({…})");
    assert!(
        matches!(&*call.func, Expr::Name(name) if name.id.as_str() == "frozenset"),
        "BATCH_KEYS is a frozenset"
    );
    let set = call.arguments.args[0].as_set_expr().expect("a set");
    let python: BTreeSet<&str> = set
        .elts
        .iter()
        .map(|elt| literal(elt).expect("a string"))
        .collect();
    let rust: BTreeSet<&str> = context_keys::SESSION_NEUTRAL.into_iter().collect();
    assert_eq!(python, rust);
    assert_eq!(rust.len(), context_keys::SESSION_NEUTRAL.len());
    assert!(!rust.contains(context_keys::SELECTED_PATHS));
}
