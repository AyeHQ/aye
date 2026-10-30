//! Why a test file is refined, reason by reason, and whether its static
//! cases may be exact ([`Refinement`]).

use aye_adapter::Recognition;
use aye_events::Certainty;

/// Why a test file's tests can only be known by importing it: the reasons,
/// in the order they hold, and whether none of its static cases may be
/// exact. The recognition says the reasons joined by `; ` (collection's
/// `not-refined` info gives them when refinement is off).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Refinement {
    reasons: Vec<String>,
    /// A reason holds for every case: each exact one becomes `Partial`.
    demote: bool,
}

impl Refinement {
    /// The reasons a module's collectors found, each once, in the order
    /// first found. They demote no case: the collectors make `Partial` the
    /// cases each concerns.
    pub(crate) fn of(found: Vec<String>) -> Self {
        let mut reasons: Vec<String> = Vec::new();
        for reason in found {
            if !reasons.contains(&reason) {
                reasons.push(reason);
            }
        }
        Self {
            reasons,
            demote: false,
        }
    }

    /// Whether it has no reason.
    pub(crate) fn is_empty(&self) -> bool {
        self.reasons.is_empty()
    }

    /// A reason that comes before those so far, and holds for every case.
    pub(crate) fn before(&mut self, reason: String) {
        self.reasons.insert(0, reason);
        self.demote = true;
    }

    /// A reason that comes after those so far, and holds for every case.
    pub(crate) fn after(&mut self, reason: String) {
        self.reasons.push(reason);
        self.demote = true;
    }

    /// No case may be exact, with no reason to refine the file: its ids
    /// stay, and pytest has the final word.
    pub(crate) fn demote(&mut self) {
        self.demote = true;
    }

    /// Make `recognition` say it: its reason, and its exact cases `Partial`
    /// when a reason holds for every case.
    pub(crate) fn apply(self, recognition: &mut Recognition) {
        if self.demote {
            for case in &mut recognition.cases {
                if case.certainty == Certainty::Exact {
                    case.certainty = Certainty::Partial;
                }
            }
        }
        recognition.refine = (!self.reasons.is_empty()).then(|| self.reasons.join("; ").into());
    }
}

#[cfg(test)]
mod tests {
    use aye_adapter::RefineReason;
    use aye_events::Case;

    use super::*;

    fn cases(certainties: &[Certainty]) -> Recognition {
        Recognition {
            cases: certainties
                .iter()
                .enumerate()
                .map(|(at, certainty)| Case {
                    certainty: *certainty,
                    ..Case::new(format!("t.py::test_{at}"), crate::NAME)
                })
                .collect(),
            ..Recognition::default()
        }
    }

    fn said(recognition: &Recognition) -> (Option<&str>, Vec<Certainty>) {
        (
            recognition.refine.as_ref().map(RefineReason::as_str),
            recognition.cases.iter().map(|c| c.certainty).collect(),
        )
    }

    /// The collectors' reasons once each, in the order found, demoting
    /// nothing; one before them and one after, as the file's own passes
    /// add them, demote every exact case.
    #[test]
    fn reasons_are_said_once_each_in_order() {
        use Certainty::{Exact, Partial};
        let found = ["b", "c", "b", "d", "c"].map(String::from).to_vec();
        let mut recognition = cases(&[Exact, Partial]);
        Refinement::of(found.clone()).apply(&mut recognition);
        assert_eq!(said(&recognition), (Some("b; c; d"), vec![Exact, Partial]));

        let mut refinement = Refinement::of(found);
        refinement.before("a".into());
        refinement.after("e".into());
        refinement.after("b".into());
        let mut recognition = cases(&[Exact, Partial]);
        refinement.apply(&mut recognition);
        assert_eq!(
            said(&recognition),
            (Some("a; b; c; d; e; b"), vec![Partial, Partial])
        );

        let mut refinement = Refinement::default();
        assert!(refinement.is_empty());
        refinement.demote();
        let mut recognition = cases(&[Exact]);
        refinement.apply(&mut recognition);
        assert_eq!(said(&recognition), (None, vec![Partial]));
    }
}
