use aye_events::{BatchIndex, DiagnosticCode, DiagnosticPath, Severity};

use super::*;

#[test]
fn diagnostic_codes_choose_the_exit_code() {
    let diagnostic = |severity, code: &str| Diagnostic {
        severity,
        code: code.into(),
        path: DiagnosticPath::Nowhere,
        span: None,
        message: String::new(),
    };
    let before = DiagnosticOrigin::BeforeBatches;
    let mut errors = Errors::default();
    errors.see(&diagnostic(Severity::Warning, "usage-error"), before);
    assert!(!errors.any && !errors.usage);
    errors.see(&diagnostic(Severity::Error, "collection-error"), before);
    assert!(errors.any && !errors.usage && !errors.internal);
    errors.see(&diagnostic(Severity::Error, "usage-error"), before);
    assert!(errors.usage);
    errors.see(&diagnostic(Severity::Error, "internal-error"), before);
    assert!(errors.internal);
    let mut errors = Errors::default();
    errors.see(&diagnostic(Severity::Error, "unclaimed"), before);
    assert!(errors.usage);
    let mut errors = Errors::default();
    errors.see(&diagnostic(Severity::Error, "not-found"), before);
    assert!(errors.usage);
    let mut errors = Errors::default();
    errors.see(
        &diagnostic(Severity::Error, "not-found"),
        DiagnosticOrigin::Batch(BatchIndex::from_u32(0)),
    );
    assert!(errors.any && !errors.usage);
}

/// Every diagnostic code aye 0.0.1 knows, and one it doesn't, with what an
/// error with it decides of the exit code, as 0.0.1 decides it, before any
/// batch and from one: `usage` (4), `internal` (3), or `error`, which fails
/// the run as the rest of it says (2, or 1). Collection's diagnostics come
/// before any batch.
const EXIT: [(&str, &str, &str); 26] = [
    ("syntax-error", "error", "error"),
    ("unreadable", "error", "error"),
    ("not-found", "usage", "error"),
    ("unclaimed", "usage", "usage"),
    ("duplicate-id", "error", "error"),
    ("no-tests", "error", "error"),
    ("not-refined", "error", "error"),
    ("refine-failed", "error", "error"),
    ("class-has-init", "error", "error"),
    ("duplicate-definition", "error", "error"),
    ("collection-error", "error", "error"),
    ("usage-error", "usage", "usage"),
    ("unknown-option", "error", "error"),
    ("plugin-not-loaded", "error", "error"),
    ("internal-error", "internal", "internal"),
    ("skipped", "error", "error"),
    ("ignored", "error", "error"),
    ("batch-stopped", "error", "error"),
    ("session-failed", "error", "error"),
    ("protocol-error", "error", "error"),
    ("batch-failed", "error", "error"),
    ("no-executor", "error", "error"),
    ("listing-differs", "error", "error"),
    ("stand-in", "error", "error"),
    ("adapter-error", "error", "error"),
    ("some-other-code", "error", "error"),
];

/// An error diagnostic with each code decides the exit code as in 0.0.1:
/// collection's (before any batch, where a `not-found` names what the
/// command line asked for) as `run` decides on them before the run, and
/// a batch's as `run_exit_code` does after it.
#[test]
fn every_code_decides_the_exit_code_as_before() {
    let known: Vec<&str> = EXIT[..25].iter().map(|(code, ..)| *code).collect();
    let all: Vec<&str> = DiagnosticCode::ALL
        .iter()
        .map(DiagnosticCode::as_str)
        .collect();
    assert_eq!(known, all);
    let decides = |code: &str, origin| {
        let mut errors = Errors::default();
        let diagnostic = Diagnostic {
            severity: Severity::Error,
            code: code.into(),
            path: DiagnosticPath::Nowhere,
            span: None,
            message: String::new(),
        };
        errors.see(&diagnostic, origin);
        assert!(errors.any, "{code}");
        if errors.internal {
            "internal"
        } else if errors.usage {
            "usage"
        } else {
            "error"
        }
    };
    for (code, before, from_batch) in EXIT {
        assert_eq!(
            (
                decides(code, DiagnosticOrigin::BeforeBatches),
                decides(code, DiagnosticOrigin::Batch(BatchIndex::from_u32(0)))
            ),
            (before, from_batch),
            "{code}"
        );
    }
}

/// The decision when the run ends.
fn decided_after_the_run(facts: Facts) -> ExitCode {
    let summary = RunSummary {
        interrupted: facts.interrupted,
        internal_error: facts.internal,
        framework_failed: facts.framework_failed,
        ..RunSummary::default()
    };
    let errors = Errors {
        any: facts.errors,
        usage: facts.usage,
        internal: false,
    };
    let counts = Counts {
        failed: facts.bad,
        passed: facts.total - facts.bad,
        ..Counts::default()
    };
    run_exit_code(&summary, errors, &counts, facts.continuing)
}

/// The exit code of a run that ran, decided as `run` decides it: from
/// collection's verdict, here clean, with what the run adds.
fn run_exit_code(
    summary: &RunSummary,
    errors: Errors,
    counts: &Counts,
    continue_on_collection_errors: bool,
) -> ExitCode {
    let collected = RunVerdict {
        selected: 1,
        continue_on_collection_errors,
        ..RunVerdict::default()
    };
    collected
        .with_run(summary, errors, *counts)
        .exit_code(RunStage::Run)
}

#[test]
fn a_usage_error_a_batch_reports_exits_4() {
    let usage = Errors {
        any: true,
        usage: true,
        ..Errors::default()
    };
    let counts = Counts {
        passed: 3,
        failed: 1,
        ..Counts::default()
    };
    let ran = RunSummary::default();
    // pytest rejecting its arguments, or an initial conftest that fails to
    // load (pytest exits 4), whatever else the batches reported.
    assert_eq!(
        run_exit_code(&ran, usage, &counts, false),
        ExitCode::UsageError
    );
    assert_eq!(
        run_exit_code(&ran, usage, &Counts::default(), true),
        ExitCode::UsageError
    );
    // An interruption, then an internal error, decide first.
    let interrupted = RunSummary {
        interrupted: true,
        ..RunSummary::default()
    };
    assert_eq!(
        run_exit_code(&interrupted, usage, &counts, false),
        ExitCode::Interrupted
    );
    let internal = Errors {
        internal: true,
        ..usage
    };
    assert_eq!(
        run_exit_code(&ran, internal, &counts, false),
        ExitCode::InternalError
    );
    // Other errors during the run.
    let collection = Errors {
        any: true,
        ..Errors::default()
    };
    assert_eq!(
        run_exit_code(&ran, collection, &counts, false),
        ExitCode::Interrupted
    );
    assert_eq!(
        run_exit_code(&ran, collection, &Counts::default(), true),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&ran, Errors::default(), &counts, false),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&ran, Errors::default(), &Counts::default(), false),
        ExitCode::NoTests
    );
}

/// C22: a batch whose framework failed it although none of its tests did
/// (a conftest's `session.exitstatus = 1`, pytest-cov's
/// `--cov-fail-under`) fails the run, as pytest exits 1; the rest decides
/// first.
#[test]
fn a_framework_that_failed_a_clean_batch_fails_the_run() {
    let failed = RunSummary {
        framework_failed: true,
        ..RunSummary::default()
    };
    let passed = Counts {
        passed: 3,
        ..Counts::default()
    };
    assert_eq!(
        run_exit_code(&failed, Errors::default(), &passed, false),
        ExitCode::TestsFailed
    );
    assert_eq!(
        run_exit_code(&RunSummary::default(), Errors::default(), &passed, false),
        ExitCode::Ok
    );
    let interrupted = RunSummary {
        interrupted: true,
        ..failed.clone()
    };
    assert_eq!(
        run_exit_code(&interrupted, Errors::default(), &passed, false),
        ExitCode::Interrupted
    );
    let usage = Errors {
        any: true,
        usage: true,
        ..Errors::default()
    };
    assert_eq!(
        run_exit_code(&failed, usage, &passed, false),
        ExitCode::UsageError
    );
}

/// What a run's exit code is decided on, at the end of collection or of
/// the run ([`EXIT_CODES`]).
#[derive(Debug, Clone, Copy, Default)]
struct Facts {
    /// Collection was interrupted, or the run was (the runner's summary).
    interrupted: bool,
    /// An internal error: a diagnostic in collection, the runner's
    /// summary in the run.
    internal: bool,
    /// A usage error diagnostic (a `not-found` in collection).
    usage: bool,
    /// An error diagnostic: collection's in collection; collection's or
    /// the run's in the run.
    errors: bool,
    framework_failed: bool,
    /// Cases selected when collection ends.
    selected: u32,
    /// Bad outcomes, and outcomes, of the tests that ran.
    bad: u32,
    total: u32,
    collect_only: bool,
    continuing: bool,
}

/// Where a run's exit code is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// When collection ends: an exit code, or `None` when the run goes on.
    Collection,
    /// When the run ends.
    Run,
}

/// No fact holds.
const F: Facts = Facts {
    interrupted: false,
    internal: false,
    usage: false,
    errors: false,
    framework_failed: false,
    selected: 0,
    bad: 0,
    total: 0,
    collect_only: false,
    continuing: false,
};

/// Every way a run's exit code was decided in aye 0.0.1, at each stage, by
/// which facts hold, as it decided them (pinned before [`RunVerdict`]
/// decided it).
#[rustfmt::skip]
const EXIT_CODES: [(&str, Stage, Facts, Option<u8>); 31] = {
    use Stage::{Collection, Run};
    [
        ("an interrupted collection", Collection, Facts { interrupted: true, ..F }, Some(2)),
        ("interrupted, then internal", Collection, Facts { interrupted: true, internal: true, ..F }, Some(2)),
        ("an internal error", Collection, Facts { internal: true, selected: 3, ..F }, Some(3)),
        ("internal, then usage", Collection, Facts { internal: true, usage: true, ..F }, Some(3)),
        ("a usage error", Collection, Facts { usage: true, selected: 3, ..F }, Some(4)),
        ("usage, then collection errors", Collection, Facts { usage: true, errors: true, ..F }, Some(4)),
        ("--co, nothing selected", Collection, Facts { collect_only: true, ..F }, Some(5)),
        ("--co", Collection, Facts { collect_only: true, selected: 3, ..F }, Some(0)),
        ("--co, collection errors", Collection, Facts { collect_only: true, errors: true, selected: 3, ..F }, Some(2)),
        ("--co, collection errors, continuing", Collection, Facts { collect_only: true, errors: true, continuing: true, selected: 3, ..F }, Some(1)),
        ("--co, collection errors, continuing, nothing selected", Collection, Facts { collect_only: true, errors: true, continuing: true, ..F }, Some(1)),
        ("--co, an internal error", Collection, Facts { collect_only: true, internal: true, selected: 3, ..F }, Some(3)),
        ("--co, a usage error", Collection, Facts { collect_only: true, usage: true, selected: 3, ..F }, Some(4)),
        ("--co, interrupted", Collection, Facts { collect_only: true, interrupted: true, ..F }, Some(2)),
        ("collection errors", Collection, Facts { errors: true, selected: 3, ..F }, Some(2)),
        ("collection errors, nothing selected", Collection, Facts { errors: true, ..F }, Some(2)),
        ("collection errors, continuing, nothing selected", Collection, Facts { errors: true, continuing: true, ..F }, Some(1)),
        ("collection errors, continuing", Collection, Facts { errors: true, continuing: true, selected: 3, ..F }, None),
        ("nothing selected", Collection, Facts { continuing: true, ..F }, Some(5)),
        ("cases to run", Collection, Facts { selected: 3, ..F }, None),
        ("an interrupted run", Run, Facts { interrupted: true, internal: true, usage: true, errors: true, framework_failed: true, bad: 1, total: 2, ..F }, Some(2)),
        ("internal, then usage", Run, Facts { internal: true, usage: true, errors: true, bad: 1, total: 2, ..F }, Some(3)),
        ("a usage error", Run, Facts { usage: true, errors: true, bad: 1, total: 2, ..F }, Some(4)),
        ("a usage error, continuing, nothing ran", Run, Facts { usage: true, errors: true, continuing: true, ..F }, Some(4)),
        ("usage, then a failed framework", Run, Facts { usage: true, errors: true, framework_failed: true, total: 2, ..F }, Some(4)),
        ("errors", Run, Facts { errors: true, total: 3, ..F }, Some(2)),
        ("errors, continuing, nothing ran", Run, Facts { errors: true, continuing: true, ..F }, Some(1)),
        ("bad outcomes", Run, Facts { bad: 1, total: 4, ..F }, Some(1)),
        ("a framework that failed a clean batch", Run, Facts { framework_failed: true, total: 3, ..F }, Some(1)),
        ("nothing ran", Run, F, Some(5)),
        ("every test passed", Run, Facts { total: 3, ..F }, Some(0)),
    ]
};

/// The decision when collection ends: an exit code, or `None` when the
/// run goes on.
fn decided_after_collection(facts: Facts) -> Option<ExitCode> {
    let Facts {
        interrupted,
        internal,
        usage,
        errors,
        framework_failed: _,
        selected,
        bad: _,
        total: _,
        collect_only,
        continuing,
    } = facts;
    let verdict = RunVerdict {
        interrupted,
        internal,
        usage,
        errors,
        selected,
        collect_only,
        continue_on_collection_errors: continuing,
        ..RunVerdict::default()
    };
    verdict
        .ends_after_collection()
        .then(|| verdict.exit_code(RunStage::Collection))
}

/// The exit code is decided as aye 0.0.1 decided it, in every case of
/// [`EXIT_CODES`]: when collection ends (an interruption, then an internal
/// error, then a usage error, then collection errors, `--co`, nothing
/// selected) and when the run ends.
#[test]
fn the_exit_code_is_decided_as_before_at_both_stages() {
    for (case, stage, facts, expected) in EXIT_CODES {
        let decided = match stage {
            Stage::Collection => decided_after_collection(facts),
            Stage::Run => Some(decided_after_the_run(facts)),
        };
        assert_eq!(decided.map(u8::from), expected, "{stage:?}: {case}");
    }
}
