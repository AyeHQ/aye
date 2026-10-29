//! GitHub Actions annotations: `::error` workflow commands that GitHub shows
//! on the lines that failed.
//!
//! One annotation per failure signature, named after the first test (or
//! bad sub-outcome, `<id> › <name>`) that failed with it; one per failed
//! lifetime, counting the tests it kept from running; one per error
//! diagnostic and per worker that failed. A test's failures that repeat its
//! sub-outcomes' aren't annotated again. Failures are written when the run
//! finishes, so repeats can be counted, and capped: GitHub shows only the
//! first few of a step anyway.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{self, Write};

use aye_events::{
    Diagnostic, Event, Failure, LifetimeId, Outcome, Phase, Record, Severity, TestId, TestResult,
};

use aye_static::EnvVars;

use crate::{Reporter, shared};

/// Failure annotations written per run.
const ANNOTATIONS: usize = 50;
/// Explanation lines in an annotation.
const DIFF_LINES: usize = 10;
/// Lines of a worker's error in an annotation.
const WORKER_LINES: usize = 10;

/// Whether this process runs as a GitHub Actions step.
pub fn github_actions_enabled() -> bool {
    is_github_actions(std::env::var_os(EnvVars::GITHUB_ACTIONS).as_deref())
}

fn is_github_actions(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "true")
}

pub struct GithubAnnotations<W: Write> {
    out: W,
    prefix: String,
    /// The 1-based line each collected test is defined on.
    lines: HashMap<String, u32>,
    annotations: Vec<Annotation>,
    /// Index of the annotation for each failure signature.
    signatures: HashMap<String, usize>,
    /// Index of the annotation for each failed lifetime.
    lifetimes: HashMap<LifetimeId, usize>,
    /// Per lifetime: the first test it kept from running, and how many.
    blocked: BTreeMap<LifetimeId, (TestId, u32)>,
    /// The failures of each running test's bad sub-outcomes.
    subs: HashMap<TestId, Vec<Failure>>,
    written: bool,
    /// Error diagnostics annotated, so that one every batch repeats is
    /// annotated once.
    errors: HashSet<(String, String, String)>,
}

struct Annotation {
    title: String,
    file: Option<String>,
    line: Option<u32>,
    message: String,
    /// Tests that failed the same way after the first.
    repeats: u32,
    /// The lifetime this annotation is about, if any.
    lifetime: Option<LifetimeId>,
}

impl<W: Write> GithubAnnotations<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            prefix: String::new(),
            lines: HashMap::new(),
            annotations: Vec::new(),
            signatures: HashMap::new(),
            lifetimes: HashMap::new(),
            blocked: BTreeMap::new(),
            subs: HashMap::new(),
            written: false,
            errors: HashSet::new(),
        }
    }

    /// Where the project root lies inside the repository, for projects that
    /// are not at its top: annotation paths are relative to the repository.
    #[must_use]
    pub fn with_path_prefix(mut self, prefix: impl Into<String>) -> Self {
        let mut prefix = prefix.into();
        prefix.truncate(prefix.trim_end_matches('/').len());
        self.prefix = prefix;
        self
    }

    pub fn into_inner(self) -> W {
        self.out
    }

    fn file(&self, path: &str) -> String {
        if self.prefix.is_empty() {
            path.to_owned()
        } else {
            format!("{}/{path}", self.prefix)
        }
    }

    /// Where to annotate: where the failure was raised or the innermost
    /// project frame, else the test's definition.
    fn locate(&self, failure: Option<&Failure>, id: &TestId) -> (Option<String>, Option<u32>) {
        if let Some((path, line)) = failure
            .and_then(shared::origin)
            .filter(|(path, _)| shared::is_project_path(path))
        {
            return (Some(path.to_owned()), Some(line));
        }
        let line = self
            .lines
            .get(id.as_str())
            .or_else(|| self.lines.get(id.without_params()))
            .copied();
        (Some(id.path().as_str().to_owned()), line)
    }

    fn test(&mut self, result: &TestResult) {
        let subs = self.subs.remove(&result.id);
        if !result.outcome.is_bad() {
            return;
        }
        if let Some(lifetime) = &result.cause {
            self.blocked
                .entry(lifetime.clone())
                .or_insert_with(|| (result.id.clone(), 0))
                .1 += 1;
            return;
        }
        // What its bad sub-outcomes were annotated with isn't annotated
        // again: the unittest and doctest adapters fail the test with
        // their failures.
        let failure = match &subs {
            Some(subs) => match result
                .failures
                .iter()
                .find(|failure| !shared::repeats_subs(failure, subs))
            {
                Some(own) => Some(own),
                None => return,
            },
            None => result.failure(),
        };
        self.annotate(
            result.id.as_str().to_owned(),
            &result.id,
            failure,
            result.outcome,
        );
    }

    /// A bad sub-outcome (a failed subtest or doctest example), annotated
    /// where it failed and named after it.
    fn sub_outcome(
        &mut self,
        id: &TestId,
        name: &str,
        outcome: Outcome,
        failure: Option<&Failure>,
    ) {
        let subs = self.subs.entry(id.clone()).or_default();
        if let Some(failure) = failure {
            subs.push(failure.clone());
        }
        self.annotate(format!("{id} › {name}"), id, failure, outcome);
    }

    /// One annotation per failure signature: the first named `title`, the
    /// rest counted on it.
    fn annotate(
        &mut self,
        title: String,
        id: &TestId,
        failure: Option<&Failure>,
        outcome: Outcome,
    ) {
        let signature = failure
            .map(|failure| failure.signature.as_str())
            .filter(|signature| !signature.is_empty());
        if let Some(&at) = signature.and_then(|signature| self.signatures.get(signature)) {
            self.annotations[at].repeats += 1;
            return;
        }
        let (file, line) = self.locate(failure, id);
        let message = failure.map_or_else(|| outcome.as_str().to_owned(), message);
        if let Some(signature) = signature {
            self.signatures
                .insert(signature.to_owned(), self.annotations.len());
        }
        self.annotations.push(Annotation {
            title,
            file,
            line,
            message,
            repeats: 0,
            lifetime: None,
        });
    }

    fn lifetime(&mut self, lifetime: &LifetimeId, failure: &Failure) {
        if self.lifetimes.contains_key(lifetime) {
            return;
        }
        let (file, line) = match self.blocked.get(lifetime) {
            Some((first, _)) => self.locate(Some(failure), first),
            None => shared::origin(failure)
                .filter(|(path, _)| shared::is_project_path(path))
                .map_or((None, None), |(path, line)| {
                    (Some(path.to_owned()), Some(line))
                }),
        };
        let phase = match failure.phase {
            Some(Phase::Teardown) => "teardown",
            _ => "setup",
        };
        self.lifetimes
            .insert(lifetime.clone(), self.annotations.len());
        self.annotations.push(Annotation {
            title: format!("{phase} of {lifetime}"),
            file,
            line,
            message: message(failure),
            repeats: 0,
            lifetime: Some(lifetime.clone()),
        });
    }

    fn diagnostic(&mut self, diagnostic: &Diagnostic) -> io::Result<()> {
        if diagnostic.severity != Severity::Error
            || !self.errors.insert((
                diagnostic.code.clone(),
                diagnostic.path.clone(),
                diagnostic.message.clone(),
            ))
        {
            return Ok(());
        }
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        // A diagnostic about no file in particular (pytest rejecting its
        // arguments) is annotated on none.
        let file = (!diagnostic.path.is_empty()).then(|| self.file(&diagnostic.path));
        let command = command(file.as_deref(), line, &diagnostic.code, &diagnostic.message);
        writeln!(self.out, "{command}")
    }

    fn worker_error(&mut self, worker: Option<u32>, error: &str) -> io::Result<()> {
        let title = worker.map_or_else(
            || "a worker failed".to_owned(),
            |w| format!("worker {w} failed"),
        );
        let tail = shared::last_lines(error, WORKER_LINES).lines.join("\n");
        writeln!(self.out, "{}", command(None, None, &title, &tail))
    }

    fn write(&mut self) -> io::Result<()> {
        if self.written {
            return Ok(());
        }
        self.written = true;
        let mut annotations = std::mem::take(&mut self.annotations);
        // Tests blocked by a lifetime that never reported its failure.
        for (lifetime, (first, _)) in &self.blocked {
            if !self.lifetimes.contains_key(lifetime) {
                annotations.push(Annotation {
                    title: format!("setup of {lifetime}"),
                    file: Some(first.path().as_str().to_owned()),
                    line: None,
                    message: format!("setup of {lifetime} failed"),
                    repeats: 0,
                    lifetime: Some(lifetime.clone()),
                });
            }
        }
        for annotation in annotations.iter().take(ANNOTATIONS) {
            let mut message = annotation.message.clone();
            if annotation.repeats > 0 {
                let _ = write!(
                    message,
                    "\n{} failed the same way",
                    shared::plural(annotation.repeats, "more test", "more tests")
                );
            }
            if let Some((_, count)) = annotation
                .lifetime
                .as_ref()
                .and_then(|lifetime| self.blocked.get(lifetime))
            {
                let _ = write!(
                    message,
                    "\n{} not run",
                    shared::plural(*count, "test", "tests")
                );
            }
            let file = annotation.file.as_deref().map(|file| self.file(file));
            let command = command(
                file.as_deref(),
                annotation.line,
                &annotation.title,
                &message,
            );
            writeln!(self.out, "{command}")?;
        }
        if annotations.len() > ANNOTATIONS {
            let more = annotations.len() - ANNOTATIONS;
            let message = format!(
                "{} not annotated; see the log",
                shared::plural(more, "more failure", "more failures")
            );
            writeln!(self.out, "{}", command(None, None, "aye", &message))?;
        }
        self.out.flush()
    }
}

/// The annotation's text: the message, with the rest of a message that
/// spans lines, then the start of the explanation.
fn message(failure: &Failure) -> String {
    let mut message = shared::message(failure).to_owned();
    let explanation = failure
        .assertion
        .as_ref()
        .map(shared::explanation)
        .unwrap_or_default();
    for line in shared::message_rest(failure)
        .into_iter()
        .chain(explanation)
        .take(DIFF_LINES)
    {
        message.push('\n');
        message.push_str(line);
    }
    message
}

/// `::error file=…,line=…,title=…::message`.
fn command(file: Option<&str>, line: Option<u32>, title: &str, message: &str) -> String {
    let mut properties = Vec::new();
    if let Some(file) = file {
        properties.push(format!("file={}", property(file)));
        if let Some(line) = line.filter(|line| *line > 0) {
            properties.push(format!("line={line}"));
        }
    }
    properties.push(format!("title={}", property(title)));
    format!("::error {}::{}", properties.join(","), data(message))
}

fn data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

fn property(value: &str) -> String {
    data(value).replace(':', "%3A").replace(',', "%2C")
}

impl<W: Write> Reporter for GithubAnnotations<W> {
    fn report(&mut self, record: &Record) -> io::Result<()> {
        match &record.event {
            Event::CaseCollected(case) => {
                if let Some(line) = case.span.as_ref().and_then(|span| span.line) {
                    self.lines.insert(case.id.as_str().to_owned(), line);
                }
                Ok(())
            }
            Event::Diagnostic(diagnostic) => self.diagnostic(diagnostic),
            Event::LifetimeFinished {
                lifetime,
                failure: Some(failure),
                ..
            } => {
                self.lifetime(lifetime, failure);
                Ok(())
            }
            Event::SubOutcome {
                id,
                name,
                outcome,
                failure,
                ..
            } if outcome.is_bad() => {
                self.sub_outcome(id, name, *outcome, failure.as_ref());
                Ok(())
            }
            Event::TestFinished(result) => {
                self.test(result);
                Ok(())
            }
            Event::WorkerFinished {
                error: Some(error), ..
            } => self.worker_error(record.envelope.worker, error),
            Event::RunFinished { .. } => self.write(),
            _ => Ok(()),
        }
    }

    fn finish(&mut self) -> io::Result<()> {
        self.write()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_only_when_github_says_so() {
        assert!(is_github_actions(Some("true".as_ref())));
        assert!(!is_github_actions(Some("false".as_ref())));
        assert!(!is_github_actions(Some("".as_ref())));
        assert!(!is_github_actions(None));
    }

    #[test]
    fn commands_escape_like_the_actions_toolkit() {
        assert_eq!(
            command(
                Some("tests/a,b.py"),
                Some(3),
                "tests/a,b.py::test_x[50%]",
                "100% broken\r\nsecond: line"
            ),
            "::error file=tests/a%2Cb.py,line=3,title=tests/a%2Cb.py%3A%3Atest_x[50%25]::100%25 broken%0D%0Asecond: line"
        );
        assert_eq!(command(None, Some(3), "aye", "m"), "::error title=aye::m");
        assert_eq!(
            command(Some("t.py"), Some(0), "x", "m"),
            "::error file=t.py,title=x::m"
        );
    }
}
