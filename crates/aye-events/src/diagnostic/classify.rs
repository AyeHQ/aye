//! What an error diagnostic means for a run: its [`ErrorClass`], from its
//! code and where it came from, and the three answers aye gives from the
//! class, which disagree on purpose: what it decides of the exit code, what
//! the summary counts it as, and whether the run store finds something to
//! show in it.

use crate::diagnostic::{Diagnostic, DiagnosticCode, Severity};
use crate::event::Envelope;
use crate::id::BatchId;
use crate::outcome::ExitCode;

/// Where a diagnostic came from, as its envelope says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticOrigin {
    /// No batch reported it: collection, the binary, or the runner outside
    /// any batch.
    BeforeBatches,
    /// The batch that reported it.
    Batch(BatchId),
}

impl DiagnosticOrigin {
    pub fn of(envelope: &Envelope) -> Self {
        envelope.batch.map_or(Self::BeforeBatches, Self::Batch)
    }
}

/// What an error diagnostic is, for a run. Six classes give the exit
/// code's, the summary's and the run store's answers for every code and
/// origin: [`ErrorClass::exit_code`], [`ErrorClass::summary`] and
/// [`ErrorClass::shows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// What the command line names isn't there: an explicit file no adapter
    /// collects (`unclaimed`), or, before any batch, a path, id, dotted
    /// name or key that names nothing (`not-found`).
    CommandLine,
    /// The framework rejected its arguments, or a conftest it loads at
    /// start-up (`usage-error`).
    Usage,
    /// The framework's own internal error (`internal-error`).
    Internal,
    /// The runner's: a protocol error, an adapter that raised outside any
    /// test, a case no executor handles (and the reserved
    /// `adapter-error`). Internal for the summary; for the exit code, the
    /// runner's own flag says so, not the code.
    RunnerInternal,
    /// The framework stopped a batch (`batch-stopped`), which the run's
    /// verdict and its stop notice tell.
    Stop,
    /// Every other error: a file that failed to collect, a requested file
    /// or id a batch can't find, a code this aye doesn't know.
    Collection,
}

/// What the summary line counts an error as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryKind {
    Collection,
    Usage,
    Internal,
}

impl DiagnosticCode {
    /// The class of an error with this code from `origin`.
    pub fn class(&self, origin: DiagnosticOrigin) -> ErrorClass {
        match self {
            Self::Unclaimed => ErrorClass::CommandLine,
            Self::NotFound => match origin {
                DiagnosticOrigin::BeforeBatches => ErrorClass::CommandLine,
                DiagnosticOrigin::Batch(_) => ErrorClass::Collection,
            },
            Self::UsageError => ErrorClass::Usage,
            Self::InternalError => ErrorClass::Internal,
            Self::AdapterError | Self::ProtocolError | Self::BatchFailed | Self::NoExecutor => {
                ErrorClass::RunnerInternal
            }
            Self::BatchStopped => ErrorClass::Stop,
            Self::SyntaxError
            | Self::Unreadable
            | Self::DuplicateId
            | Self::NoTests
            | Self::NotRefined
            | Self::RefineFailed
            | Self::ClassHasInit
            | Self::DuplicateDefinition
            | Self::CollectionError
            | Self::UnknownOption
            | Self::PluginNotLoaded
            | Self::Skipped
            | Self::Ignored
            | Self::SessionFailed
            | Self::ListingDiffers
            | Self::StandIn
            | Self::Other(_) => ErrorClass::Collection,
        }
    }
}

impl Diagnostic {
    /// Its class from `origin` when it is an error; a warning or an info
    /// has none.
    pub fn error_class(&self, origin: DiagnosticOrigin) -> Option<ErrorClass> {
        (self.severity == Severity::Error).then(|| self.code.class(origin))
    }
}

impl ErrorClass {
    /// The exit code an error of this class decides by itself: a usage
    /// error, or an internal one. `None` for one that fails the run as the
    /// rest of it says: interrupted, or failed with
    /// `--continue-on-collection-errors`.
    pub const fn exit_code(self) -> Option<ExitCode> {
        match self {
            Self::CommandLine | Self::Usage => Some(ExitCode::UsageError),
            Self::Internal => Some(ExitCode::InternalError),
            Self::RunnerInternal | Self::Stop | Self::Collection => None,
        }
    }

    /// What the summary counts an error of this class as; a stop it
    /// doesn't count.
    pub const fn summary(self) -> Option<SummaryKind> {
        match self {
            Self::CommandLine | Self::Usage => Some(SummaryKind::Usage),
            Self::Internal | Self::RunnerInternal => Some(SummaryKind::Internal),
            Self::Collection => Some(SummaryKind::Collection),
            Self::Stop => None,
        }
    }

    /// Whether an error of this class gives a stored run something to
    /// show: all but the command line's, which names what isn't there.
    pub const fn shows(self) -> bool {
        match self {
            Self::CommandLine => false,
            Self::Usage | Self::Internal | Self::RunnerInternal | Self::Stop | Self::Collection => {
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ErrorClass::{Collection, CommandLine, Internal, RunnerInternal, Stop, Usage};

    /// Every code, and one this aye doesn't know, with its class before any
    /// batch and from one.
    const CLASSES: [(&str, ErrorClass, ErrorClass); 26] = [
        ("syntax-error", Collection, Collection),
        ("unreadable", Collection, Collection),
        ("not-found", CommandLine, Collection),
        ("unclaimed", CommandLine, CommandLine),
        ("duplicate-id", Collection, Collection),
        ("no-tests", Collection, Collection),
        ("not-refined", Collection, Collection),
        ("refine-failed", Collection, Collection),
        ("class-has-init", Collection, Collection),
        ("duplicate-definition", Collection, Collection),
        ("collection-error", Collection, Collection),
        ("usage-error", Usage, Usage),
        ("unknown-option", Collection, Collection),
        ("plugin-not-loaded", Collection, Collection),
        ("internal-error", Internal, Internal),
        ("skipped", Collection, Collection),
        ("ignored", Collection, Collection),
        ("batch-stopped", Stop, Stop),
        ("session-failed", Collection, Collection),
        ("protocol-error", RunnerInternal, RunnerInternal),
        ("batch-failed", RunnerInternal, RunnerInternal),
        ("no-executor", RunnerInternal, RunnerInternal),
        ("listing-differs", Collection, Collection),
        ("stand-in", Collection, Collection),
        ("adapter-error", RunnerInternal, RunnerInternal),
        ("some-other-code", Collection, Collection),
    ];

    #[test]
    fn every_code_has_its_class_by_origin() {
        let known: Vec<&str> = CLASSES[..25].iter().map(|(code, ..)| *code).collect();
        let all: Vec<&str> = DiagnosticCode::ALL
            .iter()
            .map(DiagnosticCode::as_str)
            .collect();
        assert_eq!(known, all);
        for (code, before, from_batch) in CLASSES {
            let code = DiagnosticCode::from(code);
            assert_eq!(
                (
                    code.class(DiagnosticOrigin::BeforeBatches),
                    code.class(DiagnosticOrigin::Batch(3))
                ),
                (before, from_batch),
                "{code}"
            );
        }
    }

    /// docs/reorganisation.md §4.9's table: each class's exit code,
    /// summary and store answer.
    #[test]
    fn each_class_answers_the_three_rules() {
        for (class, exit_code, summary, shows) in [
            (
                CommandLine,
                Some(ExitCode::UsageError),
                Some(SummaryKind::Usage),
                false,
            ),
            (
                Usage,
                Some(ExitCode::UsageError),
                Some(SummaryKind::Usage),
                true,
            ),
            (
                Internal,
                Some(ExitCode::InternalError),
                Some(SummaryKind::Internal),
                true,
            ),
            (RunnerInternal, None, Some(SummaryKind::Internal), true),
            (Stop, None, None, true),
            (Collection, None, Some(SummaryKind::Collection), true),
        ] {
            assert_eq!(
                (class.exit_code(), class.summary(), class.shows()),
                (exit_code, summary, shows),
                "{class:?}"
            );
        }
    }

    #[test]
    fn only_an_error_has_a_class_and_its_origin_is_its_batch() {
        let mut diagnostic = Diagnostic {
            severity: Severity::Error,
            code: DiagnosticCode::NotFound,
            path: crate::DiagnosticPath::Nowhere,
            span: None,
            message: String::new(),
        };
        let mut envelope = Envelope::default();
        assert_eq!(
            DiagnosticOrigin::of(&envelope),
            DiagnosticOrigin::BeforeBatches
        );
        assert_eq!(
            diagnostic.error_class(DiagnosticOrigin::of(&envelope)),
            Some(CommandLine)
        );
        envelope.batch = Some(2);
        assert_eq!(DiagnosticOrigin::of(&envelope), DiagnosticOrigin::Batch(2));
        assert_eq!(
            diagnostic.error_class(DiagnosticOrigin::of(&envelope)),
            Some(Collection)
        );
        for severity in [Severity::Warning, Severity::Info] {
            diagnostic.severity = severity;
            assert_eq!(
                diagnostic.error_class(DiagnosticOrigin::BeforeBatches),
                None
            );
        }
    }
}
