//! Test keys: the short name the renderers print beside a test's id
//! (`3fae6`), and what `aye show 3fae6` takes.
//!
//! A key is the start of the id's [`TestKey`] (FNV-1a 64, 16 hex digits),
//! [`ShortKey::MIN`] digits long, longer only when needed: [`Keys`] folds
//! over a log in order, and every id, the first time it appears (a
//! `case_collected`, `test_started`, `sub_outcome` or `test_finished`; a
//! lifetime's from its `lifetime_started` or `lifetime_finished`), takes the
//! shortest start of at least [`ShortKey::MIN`] digits that no id before it
//! starts with, and keeps it. Every selected case is collected before any
//! result, so a run's keys are fixed before the first is printed, and a
//! stored log gives back the keys the run printed. A test keeps its key
//! from run to run, unless another id that starts the same way comes before
//! it.

use std::collections::HashMap;
use std::fmt;

use aye_events::{Event, KeyPrefix, LifetimeId, Record, TestId, TestKey};

/// What a key names: a test or a lifetime. Tests and lifetimes share one
/// key space, by their ids' text: a test and a lifetime spelled alike are
/// one subject, with one key.
#[derive(Debug, Clone, Copy)]
pub enum Subject<'a> {
    Test(&'a TestId),
    Lifetime(&'a LifetimeId),
}

impl<'a> Subject<'a> {
    /// The id's text, which its key is of.
    pub fn as_str(self) -> &'a str {
        match self {
            Self::Test(id) => id.as_str(),
            Self::Lifetime(lifetime) => lifetime.as_str(),
        }
    }
}

impl<'a> From<&'a TestId> for Subject<'a> {
    fn from(id: &'a TestId) -> Self {
        Self::Test(id)
    }
}

impl<'a> From<&'a LifetimeId> for Subject<'a> {
    fn from(lifetime: &'a LifetimeId) -> Self {
        Self::Lifetime(lifetime)
    }
}

/// A key as it is shown: the first digits of a [`TestKey`], from
/// [`ShortKey::MIN`] to all 16. It prints as them, padded as asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ShortKey {
    key: TestKey,
    digits: u8,
}

impl ShortKey {
    /// The fewest hex digits in a key.
    pub const MIN: usize = 5;

    /// The first `digits` of `key`, from [`Self::MIN`] to all of them.
    fn new(key: TestKey, digits: usize) -> Self {
        let digits = digits.clamp(Self::MIN, TestKey::DIGITS);
        Self {
            key,
            digits: u8::try_from(digits).unwrap_or(u8::MAX),
        }
    }

    /// The whole key it is the start of.
    pub const fn full(self) -> TestKey {
        self.key
    }

    /// How many digits it shows.
    pub fn digits(self) -> usize {
        usize::from(self.digits)
    }
}

impl fmt::Display for ShortKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.key.prefix(self.digits()))
    }
}

/// The keys of a log's tests and lifetimes; see the module's docs.
#[derive(Debug, Clone, Default)]
pub struct Keys {
    /// Each id's key.
    keys: HashMap<Box<str>, ShortKey>,
    /// The full [`TestKey`] of each id, by its first [`ShortKey::MIN`]
    /// digits.
    by_start: HashMap<Box<str>, Vec<(TestKey, Box<str>)>>,
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
                self.add(lifetime.as_str());
            }
            _ => {}
        }
    }

    /// The key of a test or a lifetime, once a record named it.
    pub fn key<'s>(&self, subject: impl Into<Subject<'s>>) -> Option<ShortKey> {
        self.keys.get(subject.into().as_str()).copied()
    }

    /// The key a test or a lifetime is shown by: its own, or for one no
    /// record named, the first [`ShortKey::MIN`] digits of its
    /// [`TestKey`].
    pub fn shown<'s>(&self, subject: impl Into<Subject<'s>>) -> ShortKey {
        let id = subject.into().as_str();
        self.keys
            .get(id)
            .copied()
            .unwrap_or_else(|| ShortKey::new(TestKey::of(id), ShortKey::MIN))
    }

    /// The ids `prefix` names, with their keys: the one whose key it is;
    /// else every id whose full key starts with it. Sorted by id; empty
    /// when it names none.
    pub fn resolve(&self, prefix: &KeyPrefix) -> Vec<(&str, ShortKey)> {
        let wanted = prefix.as_str();
        let exact: Vec<&str> = self
            .candidates(wanted)
            .filter(|(_, id)| {
                self.keys
                    .get(id)
                    .is_some_and(|key| key.to_string() == wanted)
            })
            .map(|(_, id)| &**id)
            .collect();
        let mut found = if exact.is_empty() {
            self.candidates(wanted)
                .filter(|(full, _)| full.starts_with(prefix))
                .map(|(_, id)| &**id)
                .collect()
        } else {
            exact
        };
        found.sort_unstable();
        found.dedup();
        found.into_iter().map(|id| (id, self.keys[id])).collect()
    }

    /// The (full key, id) pairs that may start with `wanted`.
    fn candidates<'a: 'w, 'w>(
        &'a self,
        wanted: &'w str,
    ) -> Box<dyn Iterator<Item = &'a (TestKey, Box<str>)> + 'w> {
        if wanted.len() >= ShortKey::MIN {
            Box::new(
                self.by_start
                    .get(&wanted[..ShortKey::MIN])
                    .into_iter()
                    .flatten(),
            )
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
        let key = TestKey::of(id);
        let full = key.to_string();
        let same_start = self
            .by_start
            .entry(full[..ShortKey::MIN].into())
            .or_default();
        let digits = same_start
            .iter()
            .map(|(other, _)| common(&full, &other.to_string()) + 1)
            .max()
            .unwrap_or(ShortKey::MIN);
        same_start.push((key, id.into()));
        self.keys.insert(id.into(), ShortKey::new(key, digits));
    }
}

/// How many leading characters `a` and `b` share.
fn common(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use aye_events::{Envelope, TestResult};

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

    /// The ids `wanted` names, as `aye show` reads it: none when it isn't
    /// 4 to 16 hex digits.
    fn resolve(keys: &Keys, wanted: &str) -> Vec<String> {
        wanted.parse().map_or_else(
            |_| Vec::new(),
            |prefix| {
                keys.resolve(&prefix)
                    .into_iter()
                    .map(|(id, _)| id.to_owned())
                    .collect()
            },
        )
    }

    fn key(keys: &Keys, id: &str) -> Option<String> {
        keys.key(&TestId::new(id)).map(|key| key.to_string())
    }

    #[test]
    fn a_key_is_five_digits_of_the_ids_hash() {
        let keys = Keys::of(&[finished("t.py::test_a")]);
        let full = TestKey::of("t.py::test_a").to_string();
        assert_eq!(key(&keys, "t.py::test_a").as_deref(), Some(&full[..5]));
        assert_eq!(resolve(&keys, &full[..5]), ["t.py::test_a"]);
        assert_eq!(resolve(&keys, &full[..4]), ["t.py::test_a"]);
        assert_eq!(resolve(&keys, &full), ["t.py::test_a"]);
        assert_eq!(
            resolve(&keys, &full[..5].to_ascii_uppercase()),
            ["t.py::test_a"]
        );
        assert_eq!(resolve(&keys, &full[..3]), [] as [String; 0]);
        assert_eq!(resolve(&keys, "zzzzz"), [] as [String; 0]);
        assert_eq!(key(&keys, "t.py::other"), None);
    }

    #[test]
    fn an_id_that_starts_like_an_earlier_one_takes_a_longer_key() {
        // Find two ids whose keys share their first five digits.
        let mut seen: HashMap<String, String> = HashMap::new();
        let (first, second) = (0..)
            .find_map(|n| {
                let id = format!("t.py::test_{n}");
                let start = TestKey::of(&id).to_string()[..ShortKey::MIN].to_string();
                seen.insert(start, id.clone()).map(|earlier| (earlier, id))
            })
            .unwrap();
        let keys = Keys::of(&[finished(&first), finished(&second), finished(&first)]);
        let (a, b) = (
            TestKey::of(&first).to_string(),
            TestKey::of(&second).to_string(),
        );
        let shared = common(&a, &b);
        assert_eq!(key(&keys, &first).as_deref(), Some(&a[..ShortKey::MIN]));
        assert_eq!(key(&keys, &second).as_deref(), Some(&b[..=shared]));
        // The first's key names it alone; the shared start names both.
        assert_eq!(
            resolve(&keys, &a[..ShortKey::MIN]),
            std::slice::from_ref(&first)
        );
        assert_eq!(resolve(&keys, &b[..=shared]), std::slice::from_ref(&second));
        let mut both = vec![first, second];
        both.sort();
        assert_eq!(resolve(&keys, &a[..4]), both);
    }

    #[test]
    fn a_test_and_a_lifetime_spelled_alike_share_a_key() {
        let lifetime = Record::new(Event::LifetimeFinished {
            lifetime: "README.rst".into(),
            failure: None,
            duration: std::time::Duration::ZERO,
        });
        let keys = Keys::of(&[lifetime, finished("README.rst")]);
        let as_lifetime = keys.key(&LifetimeId::from("README.rst")).unwrap();
        assert_eq!(keys.key(&TestId::new("README.rst")), Some(as_lifetime));
        assert_eq!(as_lifetime.digits(), ShortKey::MIN);
        // One no record named is shown by its first five digits.
        let other = TestId::new("t.py::test_other");
        assert_eq!(
            keys.shown(&other).to_string(),
            TestKey::of("t.py::test_other").prefix(ShortKey::MIN)
        );
    }
}
