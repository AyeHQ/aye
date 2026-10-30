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

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{self, Write};

use aye_events::{
    Diagnostic, DiagnosticCode, Event, Failure, LifetimeId, Outcome, Phase, Record, Severity,
    TestId, TestResult, WorkerIndex,
};
use aye_static::{EnvVars, ProcessEnv};
use aye_stream::{
    Again, BlockedTests, Consumer, DefinitionLines, FirstBySignature, Repeats, SubOutcomes,
};

use crate::{failure, words};

mod command;

use command::command;

/// Failure annotations written per run.
const ANNOTATIONS: usize = 50;

/// Explanation lines in an annotation.
const DIFF_LINES: usize = 10;

/// Lines of a worker's error in an annotation.
const WORKER_LINES: usize = 10;

/// Whether aye runs as a GitHub Actions step, as its environment `env`
/// says.
pub fn github_actions_enabled(env: &ProcessEnv) -> bool {
    is_github_actions(env.get_os(EnvVars::GITHUB_ACTIONS))
}

fn is_github_actions(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|value| value == "true")
}

pub struct GithubAnnotations<W: Write> {
    out: W,
    prefix: String,
    /// The 1-based line each collected test is defined on.
    lines: DefinitionLines,
    annotations: Vec<Annotation>,
    /// Index of the annotation for each failure signature.
    signatures: FirstBySignature<usize>,
    /// The tests each lifetime kept from running (the first names where an
    /// annotation goes, and each is counted), and the lifetimes annotated,
    /// each once.
    blocked: BlockedTests<TestId>,
    /// Each running test's bad sub-outcomes.
    subs: SubOutcomes,
    written: bool,
    /// Error diagnostics annotated, so that one every batch repeats is
    /// annotated once.
    errors: HashSet<(DiagnosticCode, String, String)>,
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
            lines: DefinitionLines::new(),
            annotations: Vec::new(),
            signatures: FirstBySignature::new(),
            blocked: BlockedTests::new(),
            subs: SubOutcomes::new(),
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
            .and_then(failure::origin)
            .filter(|(path, _)| path.is_project())
        {
            return (Some(path.as_str().to_owned()), Some(line));
        }
        (Some(id.path().as_str().to_owned()), self.lines.line(id))
    }

    fn test(&mut self, result: &TestResult) {
        let subs = self.subs.take(&result.id);
        if !result.outcome.is_bad() {
            return;
        }
        if let Some(lifetime) = &result.cause {
            self.blocked.hold(lifetime, result.id.clone());
            return;
        }
        // What its bad sub-outcomes were annotated with isn't annotated
        // again: the unittest and doctest adapters fail the test with
        // their failures.
        let failure = aye_stream::own_failures(result, &subs, Repeats::SameOrBare).next();
        if failure.is_none() && !subs.is_empty() {
            return;
        }
        self.annotate(
            result.id.as_str().to_owned(),
            &result.id,
            failure,
            result.outcome,
        );
    }

    /// A bad sub-outcome (a failed subtest or doctest example), `record`,
    /// annotated where it failed and named after it.
    fn sub_outcome(
        &mut self,
        record: &Record,
        id: &TestId,
        name: &str,
        outcome: Outcome,
        failure: Option<&Failure>,
    ) {
        self.subs.hold(record);
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
        let signature = failure.and_then(|failure| failure.signature.as_ref());
        let next = self.annotations.len();
        if let Some(&at) =
            signature.and_then(|signature| self.signatures.first_or_keep(signature, || next))
        {
            self.annotations[at].repeats += 1;
            return;
        }
        let (file, line) = self.locate(failure, id);
        let message = failure.map_or_else(|| outcome.as_str().to_owned(), message);
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
        if !self.blocked.failed(lifetime, failure, Again::Never) {
            return;
        }
        let (file, line) = match self.blocked.held(lifetime).first() {
            Some(first) => self.locate(Some(failure), first),
            None => failure::origin(failure)
                .filter(|(path, _)| path.is_project())
                .map_or((None, None), |(path, line)| {
                    (Some(path.as_str().to_owned()), Some(line))
                }),
        };
        let phase = match failure.phase {
            Some(Phase::Teardown) => "teardown",
            _ => "setup",
        };
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
                diagnostic.path.to_string(),
                diagnostic.message.clone(),
            ))
        {
            return Ok(());
        }
        let line = diagnostic.span.as_ref().and_then(|span| span.line);
        // A diagnostic about no file in particular (pytest rejecting its
        // arguments) is annotated on none.
        let file = (!diagnostic.path.is_nowhere()).then(|| self.file(diagnostic.path.as_str()));
        let command = command(
            file.as_deref(),
            line,
            diagnostic.code.as_str(),
            &diagnostic.message,
        );
        writeln!(self.out, "{command}")
    }

    fn worker_error(&mut self, worker: Option<WorkerIndex>, error: &str) -> io::Result<()> {
        let title = worker.map_or_else(
            || "a worker failed".to_owned(),
            |w| format!("worker {w} failed"),
        );
        let tail = words::last_lines(error, WORKER_LINES).lines.join("\n");
        writeln!(self.out, "{}", command(None, None, &title, &tail))
    }

    fn write(&mut self) -> io::Result<()> {
        if self.written {
            return Ok(());
        }
        self.written = true;
        let mut annotations = std::mem::take(&mut self.annotations);
        // Tests blocked by a lifetime that never reported its failure.
        for (lifetime, blocked) in self.blocked.by_lifetime() {
            if let Some(first) = blocked.first()
                && !self.blocked.has_failed(lifetime)
            {
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
                    words::plural(annotation.repeats, "more test", "more tests")
                );
            }
            if let Some(count) = annotation
                .lifetime
                .as_ref()
                .map(|lifetime| self.blocked.held(lifetime).len())
                .filter(|count| *count > 0)
            {
                let _ = write!(
                    message,
                    "\n{} not run",
                    words::plural(count, "test", "tests")
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
                words::plural(more, "more failure", "more failures")
            );
            writeln!(self.out, "{}", command(None, None, "aye", &message))?;
        }
        self.out.flush()
    }
}

/// The annotation's text: the message, with the rest of a message that
/// spans lines, then the start of the explanation.
fn message(failure: &Failure) -> String {
    let mut message = failure::message(failure).to_owned();
    let explanation = failure
        .assertion
        .as_ref()
        .map(failure::explanation)
        .unwrap_or_default();
    for line in failure::message_rest(failure)
        .into_iter()
        .chain(explanation)
        .take(DIFF_LINES)
    {
        message.push('\n');
        message.push_str(line);
    }
    message
}

impl<W: Write> Consumer for GithubAnnotations<W> {
    fn consume(&mut self, record: &Record) -> io::Result<()> {
        self.lines.note(record);
        match &record.event {
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
                self.sub_outcome(record, id, name, *outcome, failure.as_ref());
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
}
