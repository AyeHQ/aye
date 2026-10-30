//! A stored run's id ([`StoredRunId`]): its number, or the hex id of a run
//! stored before runs were numbered.

use std::fmt;

use aye_events::{RunId, RunNumber};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A stored run's id, which names its files: `r81` for the run numbered
/// 81, or, for a run stored before runs were numbered, its records' run id
/// (16 hex digits). Any text but a number's own spelling, `r` and its
/// digits without leading zeros, is such a legacy id, so an id reads back
/// as it was written, and two ids are equal when their texts are.
///
/// It prints and serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum StoredRunId {
    Numbered(RunNumber),
    Legacy(RunId),
}

impl StoredRunId {
    /// The run's number, when it has one.
    pub fn number(&self) -> Option<RunNumber> {
        match self {
            Self::Numbered(number) => Some(*number),
            Self::Legacy(_) => None,
        }
    }
}

/// The number `id` is the spelling of: `r` and its digits, without
/// leading zeros.
pub(super) fn numbered(id: &str) -> Option<RunNumber> {
    let digits = id.strip_prefix('r')?;
    if digits.starts_with('0') {
        return None;
    }
    id.parse().ok()
}

impl From<&str> for StoredRunId {
    fn from(id: &str) -> Self {
        numbered(id).map_or_else(|| Self::Legacy(RunId::from(id)), Self::Numbered)
    }
}

impl fmt::Display for StoredRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Numbered(number) => number.fmt(f),
            Self::Legacy(id) => id.fmt(f),
        }
    }
}

/// Whether `other` is its text.
impl PartialEq<str> for StoredRunId {
    fn eq(&self, other: &str) -> bool {
        match self {
            Self::Numbered(number) => numbered(other) == Some(*number),
            Self::Legacy(id) => numbered(other).is_none() && *id == *other,
        }
    }
}

impl PartialEq<&str> for StoredRunId {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl Serialize for StoredRunId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for StoredRunId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let id = Box::<str>::deserialize(deserializer)?;
        Ok(Self::from(&*id))
    }
}
