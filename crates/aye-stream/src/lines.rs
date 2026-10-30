//! The line each collected test is defined on ([`DefinitionLines`]), where
//! a report points at a test that failed without a place in the project.

use std::collections::HashMap;

use aye_events::{Event, Record, TestId};

/// The 1-based line each collected test is defined on, from its
/// `case_collected` (the last, for an id collected twice).
#[derive(Debug, Default)]
pub struct DefinitionLines {
    lines: HashMap<TestId, u32>,
}

impl DefinitionLines {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note(&mut self, record: &Record) {
        if let Event::CaseCollected(case) = &record.event
            && let Some(line) = case.span.as_ref().and_then(|span| span.line)
        {
            self.lines.insert(case.id.clone(), line);
        }
    }

    /// The line `id` is defined on: its own case's, else that of its id
    /// without parameters (a case collected without them finishes as one
    /// result per parameter).
    pub fn line(&self, id: &TestId) -> Option<u32> {
        self.lines
            .get(id)
            .or_else(|| self.lines.get(id.without_params()))
            .copied()
    }
}
