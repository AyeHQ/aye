//! Test keys: the short name the renderers print beside a test's id
//! (`3fae6`), and what `aye show 3fae6` takes.
//!
//! A key is the start of the id's [`TestKey`] (FNV-1a 64, 16 hex digits),
//! [`MIN`] digits long, longer only when needed: [`Keys`] folds over a log
//! in order, and every id, the first time it appears (a `case_collected`,
//! `test_started`, `sub_outcome` or `test_finished`; a lifetime's from its
//! `lifetime_started` or `lifetime_finished`), takes the shortest start of
//! at least [`MIN`] digits that no id before it starts with, and keeps it.
//! Every selected case is collected before any result, so a run's keys are
//! fixed before the first is printed, and a stored log gives back the keys
//! the run printed. A test keeps its key from run to run, unless another
//! id that starts the same way comes before it.

use std::collections::{BTreeMap, HashMap};

use aye_events::{Event, ProjectPath, Record, TestKey};

/// The fewest hex digits in a key.
pub(crate) const MIN: usize = 5;
/// The fewest hex digits `aye show` matches a key's start on.
const MIN_PREFIX: usize = 4;

/// The keys of a log's tests and lifetimes; see the module's docs.
#[derive(Debug, Clone, Default)]
pub struct Keys {
    /// Each id's key.
    keys: HashMap<String, String>,
    /// The full [`TestKey`] of each id, by its first [`MIN`] digits.
    by_start: HashMap<String, Vec<(String, String)>>,
}

impl Keys {
    /// No id yet: feed a run's records to it as they come ([`Self::note`]).
    pub fn new() -> Self {
        Self::default()
    }

    /// The keys of a whole log.
    pub fn of<'a>(records: impl IntoIterator<Item = &'a Record>) -> Self {
        let mut keys = Self::new();
        for record in records {
            keys.note(record);
        }
        keys
    }

    /// Give the ids `record` names that have none their key.
    pub fn note(&mut self, record: &Record) {
        match &record.event {
            Event::CaseCollected(case) => self.add(case.id.as_str()),
            Event::TestStarted { id } | Event::SubOutcome { id, .. } => self.add(id.as_str()),
            Event::TestFinished(result) => self.add(result.id.as_str()),
            Event::LifetimeStarted { lifetime, .. } | Event::LifetimeFinished { lifetime, .. } => {
                self.add(lifetime);
            }
            _ => {}
        }
    }

    /// The key of a test's or a lifetime's id, once a record named it.
    pub fn key(&self, id: &str) -> Option<&str> {
        self.keys.get(id).map(String::as_str)
    }

    /// The ids `wanted` names: the one whose key it is; else, for 4 to 16
    /// hex digits, every id whose full key starts with them. Sorted; empty
    /// when it names none (or isn't hex).
    pub fn resolve(&self, wanted: &str) -> Vec<String> {
        let wanted = wanted.to_ascii_lowercase();
        if !(MIN_PREFIX..=16).contains(&wanted.len())
            || !wanted.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Vec::new();
        }
        let exact: Vec<String> = self
            .candidates(&wanted)
            .filter(|(_, id)| self.keys.get(id.as_str()) == Some(&wanted))
            .map(|(_, id)| id.clone())
            .collect();
        let mut found = if exact.is_empty() {
            self.candidates(&wanted)
                .filter(|(full, _)| full.starts_with(&wanted))
                .map(|(_, id)| id.clone())
                .collect()
        } else {
            exact
        };
        found.sort();
        found.dedup();
        found
    }

    /// The (full key, id) pairs that may start with `wanted`.
    fn candidates<'a>(
        &'a self,
        wanted: &'a str,
    ) -> Box<dyn Iterator<Item = &'a (String, String)> + 'a> {
        if wanted.len() >= MIN {
            Box::new(self.by_start.get(&wanted[..MIN]).into_iter().flatten())
        } else {
            Box::new(
                self.by_start
                    .iter()
                    .filter(move |(start, _)| start.starts_with(wanted))
                    .flat_map(|(_, ids)| ids),
            )
        }
    }

    fn add(&mut self, id: &str) {
        if self.keys.contains_key(id) {
            return;
        }
        let full = TestKey::of(id).0;
        let same_start = self.by_start.entry(full[..MIN].to_string()).or_default();
        let len = same_start
            .iter()
            .map(|(other, _)| common(&full, other) + 1)
            .max()
            .unwrap_or(MIN)
            .clamp(MIN, full.len());
        same_start.push((full.clone(), id.to_string()));
        self.keys.insert(id.to_string(), full[..len].to_string());
    }
}

/// How many leading characters `a` and `b` share.
fn common(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

/// What `aye show` knows of a stored run beside its log, for the details'
/// headers: which run it is, how long ago it finished, and which of its
/// files changed since.
#[derive(Debug, Clone, Default)]
pub struct ShowContext {
    /// The run's number (`r81`).
    pub run: Option<u64>,
    /// How long ago the run finished: `2 min ago`.
    pub ago: Option<String>,
    /// The files, as ids spell them, that changed since the run, each with
    /// whether it is gone.
    pub stale: BTreeMap<ProjectPath, bool>,
}

#[cfg(test)]
mod tests {
    use aye_events::{Envelope, TestId, TestResult};

    use super::*;

    fn finished(id: &str) -> Record {
        Record {
            envelope: Envelope::default(),
            event: Event::TestFinished(Box::new(TestResult::new(
                TestId::new(id),
                aye_events::Outcome::Failed,
            ))),
        }
    }

    #[test]
    fn a_key_is_five_digits_of_the_ids_hash() {
        let keys = Keys::of(&[finished("t.py::test_a")]);
        let full = TestKey::of("t.py::test_a").0;
        assert_eq!(keys.key("t.py::test_a"), Some(&full[..5]));
        assert_eq!(keys.resolve(&full[..5]), ["t.py::test_a"]);
        assert_eq!(keys.resolve(&full[..4]), ["t.py::test_a"]);
        assert_eq!(keys.resolve(&full), ["t.py::test_a"]);
        assert_eq!(
            keys.resolve(&full[..5].to_ascii_uppercase()),
            ["t.py::test_a"]
        );
        assert_eq!(keys.resolve(&full[..3]), [] as [String; 0]);
        assert_eq!(keys.resolve("zzzzz"), [] as [String; 0]);
        assert_eq!(keys.key("t.py::other"), None);
    }

    #[test]
    fn an_id_that_starts_like_an_earlier_one_takes_a_longer_key() {
        // Find two ids whose keys share their first five digits.
        let mut seen: HashMap<String, String> = HashMap::new();
        let (first, second) = (0..)
            .find_map(|n| {
                let id = format!("t.py::test_{n}");
                let start = TestKey::of(&id).0[..MIN].to_string();
                seen.insert(start, id.clone()).map(|earlier| (earlier, id))
            })
            .unwrap();
        let keys = Keys::of(&[finished(&first), finished(&second), finished(&first)]);
        let (a, b) = (TestKey::of(&first).0, TestKey::of(&second).0);
        let shared = common(&a, &b);
        assert_eq!(keys.key(&first), Some(&a[..MIN]));
        assert_eq!(keys.key(&second), Some(&b[..=shared]));
        // The first's key names it alone; the shared start names both.
        assert_eq!(keys.resolve(&a[..MIN]), std::slice::from_ref(&first));
        assert_eq!(keys.resolve(&b[..=shared]), std::slice::from_ref(&second));
        let mut both = vec![first, second];
        both.sort();
        assert_eq!(keys.resolve(&a[..4]), both);
    }
}
