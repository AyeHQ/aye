//! `--junitxml`: a JUnit XML report for CI systems, written when the stream
//! ends.
//!
//! One `<testsuite>` inside `<testsuites name="aye">`, named `aye` unless the
//! framework's settings name it ([`JunitOptions`], pytest's
//! `junit_suite_name`), with the run's properties (pytest's
//! `record_testsuite_property`, each pair once) first. A test's `classname`
//! is its file as a dotted module plus its classes, and its `name` the test
//! name with its parameters, as pytest writes them; a prefix
//! ([`Junit::with_prefix`], pytest's `--junitprefix`) goes before every
//! `classname`, as `prefix.` or alone where it is empty. `file` and `line`
//! are written for xunit1 (aye's default, and pytest's `junit_family =
//! xunit1` or `legacy`), not for xunit2 (pytest's default family). A test
//! case starts with its properties (pytest's `record_property`). Each bad
//! sub-outcome (a failed subtest or doctest example) is a `<failure>` of its
//! test, before the test's own, as in pytest's report; the test's own
//! failures that repeat it are left out. Each test a failed lifetime kept
//! from running has an `<error>` naming the lifetime and its message. A
//! module skipped while collecting is a skipped test case named after it, as
//! in pytest's report. Captured output goes in last, as [`JunitLogging`]
//! says.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use aye_events::{
    Diagnostic, Event, Failure, LifetimeId, Outcome, Phase, Record, Severity, TestResult,
};
use aye_stream::{Consumer, DefinitionLines, HeldSub, ModuleSkips, Repeats, SubOutcomes};

use crate::{failure, words};

mod names;
mod settings;
mod xml;

use names::names;
use xml::{attr, body, element, properties, section};

pub use settings::{JunitDuration, JunitFamily, JunitLogging, JunitOptions};

impl From<&str> for JunitLogging {
    /// pytest's spelling; pytest writes no output for one it doesn't know.
    fn from(value: &str) -> Self {
        match value {
            "log" => Self::Log,
            "system-out" => Self::SystemOut,
            "system-err" => Self::SystemErr,
            "out-err" => Self::OutErr,
            "all" => Self::All,
            _ => Self::No,
        }
    }
}

impl From<&str> for JunitDuration {
    /// pytest's spelling: `call` is the body; anything else the whole test.
    fn from(value: &str) -> Self {
        if value == "call" {
            Self::Body
        } else {
            Self::Total
        }
    }
}

impl From<&str> for JunitFamily {
    /// pytest's spelling; one it doesn't know is taken as its default,
    /// `xunit2`.
    fn from(value: &str) -> Self {
        if matches!(value, "xunit1" | "legacy") {
            Self::Xunit1
        } else {
            Self::Xunit2
        }
    }
}

pub struct Junit {
    path: PathBuf,
    /// Put before every `classname` (pytest's `--junitprefix`).
    prefix: String,
    options: JunitOptions,
    /// The run's properties, each pair once, in the order first recorded.
    properties: Vec<(String, String)>,
    /// The 1-based line each collected test is defined on.
    lines: DefinitionLines,
    /// Test cases in the order they finished.
    cases: Vec<Testcase>,
    /// Failed lifetimes, for the tests they kept from running.
    lifetimes: HashMap<LifetimeId, Failure>,
    /// Bad sub-outcomes of tests that haven't finished yet.
    subs: SubOutcomes,
    totals: Totals,
    /// Modules skipped while collecting.
    module_skips: ModuleSkips,
    /// The run's own duration, once known.
    duration: Option<Duration>,
    /// The sum of the tests' durations.
    tests_time: Duration,
}

enum Testcase {
    Done(String),
    /// Kept from running by a lifetime; written once its failure is known.
    Blocked(Box<TestResult>),
    /// A failed lifetime, where its failure arrived: a test case of its own
    /// unless it kept a test from running, which is known only at the end
    /// (under `maxfail`, the engine reports it before the tests it blocks).
    Lifetime(LifetimeId),
}

#[derive(Default)]
struct Totals {
    tests: u32,
    failures: u32,
    errors: u32,
    skipped: u32,
}

impl Junit {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            prefix: String::new(),
            options: JunitOptions::default(),
            properties: Vec::new(),
            lines: DefinitionLines::new(),
            cases: Vec::new(),
            lifetimes: HashMap::new(),
            subs: SubOutcomes::new(),
            totals: Totals::default(),
            module_skips: ModuleSkips::default(),
            duration: None,
            tests_time: Duration::ZERO,
        }
    }

    /// Put `prefix` before every `classname`, as pytest's `--junitprefix`
    /// does; an empty one puts nothing.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Write the report as the framework's settings say.
    #[must_use]
    pub fn with_options(mut self, options: JunitOptions) -> Self {
        self.options = options;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// ` file="…" line="…"` for a test case, where the family has them.
    fn location(&self, path: &str, line: Option<u32>) -> String {
        if self.options.family == JunitFamily::Xunit2 {
            return String::new();
        }
        let mut attributes = format!(" file=\"{}\"", attr(path));
        if let Some(line) = line {
            let _ = write!(attributes, " line=\"{line}\"");
        }
        attributes
    }

    /// `classname` with the prefix before it.
    fn classname(&self, classname: &str) -> String {
        match (self.prefix.as_str(), classname) {
            ("", classname) => classname.to_string(),
            (prefix, "") => prefix.to_string(),
            (prefix, classname) => format!("{prefix}.{classname}"),
        }
    }

    /// The report as it stands.
    pub fn xml(&self) -> String {
        let causes: HashSet<&LifetimeId> = self
            .cases
            .iter()
            .filter_map(|case| match case {
                Testcase::Blocked(result) => result.cause.as_ref(),
                _ => None,
            })
            .collect();
        // A failed lifetime that kept no test from running is a test case,
        // and an error, of its own.
        let alone = |case: &Testcase| match case {
            Testcase::Lifetime(lifetime) => !causes.contains(lifetime),
            _ => false,
        };
        let lifetimes =
            u32::try_from(self.cases.iter().filter(|case| alone(case)).count()).unwrap_or(u32::MAX);
        let Totals {
            tests,
            failures,
            errors,
            skipped,
        } = self.totals;
        let tests = tests.saturating_add(lifetimes);
        let errors = errors.saturating_add(lifetimes);
        let time = self.duration.unwrap_or(self.tests_time).as_secs_f64();
        let counts = format!(
            "tests=\"{tests}\" failures=\"{failures}\" errors=\"{errors}\" skipped=\"{skipped}\" time=\"{time:.3}\""
        );
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        let _ = writeln!(xml, "<testsuites name=\"aye\" {counts}>");
        let _ = writeln!(
            xml,
            "  <testsuite name=\"{}\" {counts}>",
            attr(&self.options.suite_name)
        );
        properties(&mut xml, "    ", &self.properties);
        for case in &self.cases {
            match case {
                Testcase::Done(done) => xml.push_str(done),
                Testcase::Blocked(result) => {
                    let failure = result
                        .cause
                        .as_ref()
                        .and_then(|lifetime| self.lifetimes.get(lifetime));
                    xml.push_str(&self.testcase(result, failure, &[]));
                }
                Testcase::Lifetime(lifetime) => {
                    if alone(case)
                        && let Some(failure) = self.lifetimes.get(lifetime)
                    {
                        xml.push_str(&self.lifetime_case(lifetime, failure));
                    }
                }
            }
        }
        xml.push_str("  </testsuite>\n</testsuites>\n");
        xml
    }

    fn test(&mut self, result: &TestResult) {
        let subs = self.subs.take(&result.id);
        // The unittest and doctest adapters fail the test with the very
        // failures of its subtests or examples: each is reported once, on
        // its sub-outcome, which names it. A bare failure of its own stays.
        let own = aye_stream::own_failures(result, &subs, Repeats::Same)
            .cloned()
            .collect();
        let result = &TestResult {
            failures: own,
            ..result.clone()
        };
        self.totals.tests += 1;
        match result.outcome {
            Outcome::Failed => self.totals.failures += 1,
            Outcome::Error => self.totals.errors += 1,
            // The test case holds a `<failure>` all the same.
            _ if !subs.is_empty() => self.totals.failures += 1,
            Outcome::Skipped | Outcome::ExpectedFailure => self.totals.skipped += 1,
            Outcome::Passed | Outcome::UnexpectedPass => {}
        }
        self.tests_time += result.duration;
        if result.cause.is_some() {
            let mut blocked = result.clone();
            blocked.failures.clear();
            self.cases.push(Testcase::Blocked(Box::new(blocked)));
        } else {
            let done = self.testcase(result, None, &subs);
            self.cases.push(Testcase::Done(done));
        }
    }

    fn testcase(
        &self,
        result: &TestResult,
        lifetime: Option<&Failure>,
        subs: &[HeldSub],
    ) -> String {
        let id = result.id.as_str();
        let (classname, name) = names(id);
        let line = self.lines.line(&result.id);
        let time = match self.options.duration {
            JunitDuration::Body => result.body_duration.unwrap_or(result.duration),
            JunitDuration::Total => result.duration,
        };
        let mut xml = format!(
            "    <testcase classname=\"{}\" name=\"{}\"{} time=\"{:.3}\"",
            attr(&self.classname(&classname)),
            attr(&name),
            self.location(result.id.path().as_str(), line),
            time.as_secs_f64()
        );

        let mut children = String::new();
        properties(&mut children, "      ", &result.properties);
        for sub in subs {
            let tag = if sub.outcome == Outcome::Error {
                "error"
            } else {
                "failure"
            };
            let message = sub
                .failure
                .as_ref()
                .map_or(sub.outcome.as_str(), failure::message);
            element(
                &mut children,
                tag,
                Some(&format!("{}: {message}", sub.name)),
                sub.failure.as_ref().and_then(|f| f.exception.as_deref()),
                &sub.failure.as_ref().map(detail).unwrap_or_default(),
            );
        }
        let reason = result.reason.as_deref();
        match result.outcome {
            Outcome::Passed | Outcome::UnexpectedPass => {}
            Outcome::Skipped => element(&mut children, "skipped", reason, None, ""),
            Outcome::ExpectedFailure => element(
                &mut children,
                "skipped",
                reason,
                Some("expected_failure"),
                "",
            ),
            // A test failed only by its sub-outcomes: their elements say it.
            Outcome::Failed | Outcome::Error
                if !subs.is_empty() && result.failures.is_empty() && result.cause.is_none() => {}
            Outcome::Failed | Outcome::Error => failures(&mut children, result, lifetime),
        }
        self.output(&mut children, result);
        if children.is_empty() {
            xml.push_str("/>\n");
        } else {
            xml.push_str(">\n");
            xml.push_str(&children);
            xml.push_str("    </testcase>\n");
        }
        xml
    }

    /// The test's captured output, as [`JunitOptions::logging`] says. With
    /// pytest's layout, each stream is a section under a header, written
    /// even when empty. With [`JunitOptions::log_passing_tests`] off, only
    /// a test that failed carries its output: as pytest has it, one whose
    /// body or teardown failed, or that was skipped, but not one that only
    /// failed to set up or was expected to fail.
    fn output(&self, children: &mut String, result: &TestResult) {
        let output = &result.output;
        if !self.options.log_passing_tests {
            let failed = match result.outcome {
                Outcome::Failed | Outcome::Skipped => true,
                Outcome::Error => result
                    .failures
                    .iter()
                    .any(|failure| failure.phase != Some(Phase::Setup)),
                Outcome::Passed | Outcome::ExpectedFailure | Outcome::UnexpectedPass => false,
            };
            if !failed {
                return;
            }
        }
        let (out, err) = match self.options.logging {
            JunitLogging::Aye => {
                let stdout = match (output.stdout.trim_end(), output.log.trim_end()) {
                    ("", log) => log.to_owned(),
                    (stdout, "") => stdout.to_owned(),
                    (stdout, log) => format!("{stdout}\n----- log -----\n{log}"),
                };
                let stderr = output.stderr.trim_end().to_owned();
                (
                    (!stdout.is_empty()).then_some(stdout),
                    (!stderr.is_empty()).then_some(stderr),
                )
            }
            JunitLogging::No => (None, None),
            JunitLogging::Log => (Some(section("Captured Log", &output.log)), None),
            JunitLogging::SystemOut => (Some(section("Captured Out", &output.stdout)), None),
            JunitLogging::SystemErr => (None, Some(section("Captured Err", &output.stderr))),
            JunitLogging::OutErr => (
                Some(section("Captured Out", &output.stdout)),
                Some(section("Captured Err", &output.stderr)),
            ),
            JunitLogging::All => (
                Some(
                    section("Captured Log", &output.log) + &section("Captured Out", &output.stdout),
                ),
                Some(section("Captured Err", &output.stderr)),
            ),
        };
        if let Some(out) = out {
            let _ = writeln!(children, "      <system-out>{}</system-out>", body(&out));
        }
        if let Some(err) = err {
            let _ = writeln!(children, "      <system-err>{}</system-err>", body(&err));
        }
    }

    /// A failed lifetime is reported on the tests it kept from running; one
    /// that kept none from running (a failed teardown) is a test case of its
    /// own, where its failure arrived. Which it is is decided when the
    /// report is written ([`Testcase::Lifetime`]).
    fn lifetime(&mut self, lifetime: &LifetimeId, failure: &Failure) {
        if self.lifetimes.contains_key(lifetime) {
            return;
        }
        self.lifetimes.insert(lifetime.clone(), failure.clone());
        self.cases.push(Testcase::Lifetime(lifetime.clone()));
    }

    /// The test case of a failed lifetime that kept no test from running.
    fn lifetime_case(&self, lifetime: &LifetimeId, failure: &Failure) -> String {
        let phase = failure.phase.map_or("lifetime", words::phase);
        let mut xml = format!(
            "    <testcase classname=\"{}\" name=\"{}\"",
            attr(&self.classname("")),
            attr(&format!("{phase} of {lifetime}"))
        );
        if let Some((path, line)) = failure::origin(failure) {
            xml.push_str(&self.location(path.as_str(), Some(line)));
        }
        xml.push_str(" time=\"0.000\">\n");
        element(
            &mut xml,
            "error",
            Some(failure::message(failure)),
            failure.exception.as_deref(),
            &detail(failure),
        );
        xml.push_str("    </testcase>\n");
        xml
    }

    /// A module the framework skipped while collecting it: a skipped test
    /// case named after the module, as in pytest's report.
    fn module_skip(&mut self, diagnostic: &Diagnostic) {
        self.totals.tests += 1;
        self.totals.skipped += 1;
        let (classname, name) = names(diagnostic.path.as_str());
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        let mut xml = format!(
            "    <testcase classname=\"{}\" name=\"{}\"{} time=\"0.000\">\n",
            attr(&self.classname(&classname)),
            attr(&name),
            self.location(diagnostic.path.as_str(), line)
        );
        element(
            &mut xml,
            "skipped",
            Some(&diagnostic.message),
            Some("collection"),
            "",
        );
        xml.push_str("    </testcase>\n");
        self.cases.push(Testcase::Done(xml));
    }

    fn diagnostic(&mut self, diagnostic: &Diagnostic) {
        if diagnostic.severity != Severity::Error {
            return;
        }
        self.totals.tests += 1;
        self.totals.errors += 1;
        let (classname, name) = names(diagnostic.path.as_str());
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        let mut xml = format!(
            "    <testcase classname=\"{}\" name=\"{}\"{} time=\"0.000\">\n",
            attr(&self.classname(&classname)),
            attr(&name),
            self.location(diagnostic.path.as_str(), line)
        );
        let first = diagnostic.message.lines().next().unwrap_or_default();
        element(
            &mut xml,
            "error",
            Some(first),
            Some(diagnostic.code.as_str()),
            &diagnostic.message,
        );
        xml.push_str("    </testcase>\n");
        self.cases.push(Testcase::Done(xml));
    }
}

/// The `<failure>` and `<error>` elements of a bad outcome.
fn failures(children: &mut String, result: &TestResult, lifetime: Option<&Failure>) {
    let outcome = result.outcome;
    if let Some(cause) = &result.cause {
        // As pytest reports each test a failed fixture kept from running:
        // an error naming what failed and why.
        let message = match lifetime {
            Some(failure) => {
                let phase = match failure.phase {
                    Some(Phase::Teardown) => "teardown",
                    _ => "setup",
                };
                format!("{phase} of {cause} failed: {}", failure::message(failure))
            }
            None => format!("setup of {cause} failed"),
        };
        let exception = lifetime.and_then(|failure| failure.exception.as_deref());
        let detail = lifetime.map(detail).unwrap_or_default();
        element(children, "error", Some(&message), exception, &detail);
        return;
    }
    if result.failures.is_empty() {
        let tag = if outcome == Outcome::Failed {
            "failure"
        } else {
            "error"
        };
        element(children, tag, Some(outcome.as_str()), None, "");
        return;
    }
    // An error with no failed setup or teardown is an unexpected exception
    // in the test itself.
    let body_is_error = outcome == Outcome::Error
        && result
            .failures
            .iter()
            .all(|failure| !matches!(failure.phase, Some(Phase::Setup | Phase::Teardown)));
    for failure in &result.failures {
        let tag = match failure.phase {
            Some(Phase::Setup | Phase::Teardown) => "error",
            _ if body_is_error => "error",
            _ => "failure",
        };
        element(
            children,
            tag,
            Some(failure::message(failure)),
            failure.exception.as_deref(),
            &detail(failure),
        );
    }
}

/// The framework's full text, or one built from the frames and message.
fn detail(failure: &Failure) -> String {
    if let Some(detail) = &failure.detail {
        return detail.trim_end().to_owned();
    }
    let mut detail = String::new();
    for frame in &failure.frames {
        let _ = write!(detail, "{}:{}", frame.path, frame.line);
        if let Some(function) = &frame.function {
            let _ = write!(detail, " in {function}");
        }
        detail.push('\n');
        if let Some(source) = &frame.source {
            let _ = writeln!(detail, "    {source}");
        }
    }
    detail.push_str(failure::message(failure));
    if let Some(assertion) = &failure.assertion {
        for line in failure::diff_lines(&assertion.diff) {
            detail.push('\n');
            detail.push_str(line);
        }
    }
    detail
}

impl Consumer for Junit {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        if let Some(skipped) = self.module_skips.note(record) {
            self.module_skip(skipped);
        }
        self.lines.note(record);
        match &record.event {
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic),
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                ..
            } => self.lifetime(lifetime, failure),
            Event::SubOutcome { .. } => {
                self.subs.hold(record);
            }
            Event::TestFinished(result) => self.test(result),
            Event::RunProperty { name, value } => {
                let pair = (name.clone(), value.clone());
                if !self.properties.contains(&pair) {
                    self.properties.push(pair);
                }
            }
            Event::RunFinished { duration, .. } => self.duration = Some(*duration),
            _ => {}
        }
        Ok(())
    }

    /// Writes the report; an error names the file.
    fn finish(&mut self) -> io::Result<()> {
        let write = || {
            if let Some(parent) = self.path.parent()
                && !parent.as_os_str().is_empty()
            {
                fs::create_dir_all(parent)?;
            }
            fs::write(&self.path, self.xml())
        };
        write().map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "cannot write the JUnit XML report to {}: {err}",
                    self.path.display()
                ),
            )
        })
    }
}
