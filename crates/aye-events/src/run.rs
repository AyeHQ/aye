//! A run's identities: its id, the environment it ran in, the lanes its
//! workers run on and the batches they are handed.

use std::borrow::Borrow;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use aye_index::newtype_index;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A run's id: 16 random hex digits ([`RunId::random`]), on every record's
/// envelope (`run`), in the history as the run that last finished a test,
/// and given to the workers as pytest-xdist's `testrun_uid`. Not the run's
/// number in the run store, which its `run_started` says.
///
/// An id is opaque once written: one read back is kept as it was (the
/// tests' logs say `r1`). It orders, compares and serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Box<str>);

impl RunId {
    /// A new run's id: 16 hex digits, random per call.
    pub fn random() -> Self {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        hasher.write_u32(std::process::id());
        Self(format!("{:016x}", hasher.finish()).into_boxed_str())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An id as it was written.
impl From<&str> for RunId {
    fn from(id: &str) -> Self {
        Self(id.into())
    }
}

/// An id as it was written.
impl From<String> for RunId {
    fn from(id: String) -> Self {
        Self(id.into_boxed_str())
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RunId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for RunId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for RunId {
    fn eq(&self, other: &str) -> bool {
        *self.0 == *other
    }
}

impl PartialEq<&str> for RunId {
    fn eq(&self, other: &&str) -> bool {
        *self.0 == **other
    }
}

/// One environment (an interpreter and its dependencies) a run's tests ran
/// in: the envelope's `environment`. Reserved for the matrix: nothing sets
/// it yet, and one read back is kept as it was. It serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvironmentId(Box<str>);

impl EnvironmentId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An id as it was written.
impl From<&str> for EnvironmentId {
    fn from(id: &str) -> Self {
        Self(id.into())
    }
}

/// An id as it was written.
impl From<String> for EnvironmentId {
    fn from(id: String) -> Self {
        Self(id.into_boxed_str())
    }
}

impl fmt::Display for EnvironmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

newtype_index! {
    /// A worker's lane in a run, from 0: the slot that pulls batches, which
    /// a crashed worker's replacement takes over, so a lane may see more
    /// than one worker. The envelope's `worker`, and what the lane's
    /// workers are told in `AYE_WORKER` and, with several lanes,
    /// `PYTEST_XDIST_WORKER` ([`WorkerIndex::xdist_name`]).
    ///
    /// It prints and serializes as its number.
    pub struct WorkerIndex;
}

newtype_index! {
    /// A batch of a run, from 0, dense, in the order the batches were handed
    /// out: the envelope's `batch`, a protocol batch's `id` and the
    /// `batch_done` that ends it. A batch put back before it started keeps
    /// its index.
    ///
    /// It prints and serializes as its number.
    pub struct BatchIndex;
}

impl WorkerIndex {
    /// The first lane: the one a worker kept from refinement starts on, the
    /// one whose listing of the session the others compare theirs with,
    /// and the only one that runs the tests of a file another lane lists
    /// otherwise.
    pub const FIRST: Self = Self::from_u32(0);

    pub const fn is_first(self) -> bool {
        self.as_u32() == 0
    }

    /// How pytest-xdist names the lane's worker: `gw3`.
    pub fn xdist_name(self) -> String {
        format!("gw{self}")
    }
}

/// Writes an index as its number, and reads it back.
macro_rules! wire_index {
    ($name:ident) => {
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.as_u32(), f)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_u32(self.as_u32())
            }
        }

        /// A `u32`; `u32::MAX`, which no index can be, is refused.
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let index = u32::deserialize(deserializer)?;
                if index == u32::MAX {
                    return Err(serde::de::Error::custom(concat!(
                        stringify!($name),
                        " out of range"
                    )));
                }
                Ok(Self::from_u32(index))
            }
        }
    };
}

wire_index!(WorkerIndex);
wire_index!(BatchIndex);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_ids_are_sixteen_random_hex_digits() {
        let a = RunId::random();
        assert_eq!(a.as_str().len(), 16);
        assert!(a.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, RunId::random());
    }

    /// A run's id and an environment's are their text, kept as written.
    #[test]
    fn a_run_id_is_its_text() {
        let read: RunId = serde_json::from_str("\"r1\"").unwrap();
        assert_eq!(read, "r1");
        assert_eq!(read.to_string(), "r1");
        assert_eq!(serde_json::to_string(&read).unwrap(), "\"r1\"");
        assert_eq!(RunId::from(String::from("r1")), read);
        let environment: EnvironmentId = serde_json::from_str("\"py313\"").unwrap();
        assert_eq!(environment.as_str(), "py313");
        assert_eq!(serde_json::to_string(&environment).unwrap(), "\"py313\"");
    }

    /// An index prints and serializes as the number it was when it was a
    /// `u32`.
    #[test]
    fn an_index_is_its_number_on_the_wire() {
        for n in [0, 3, 41] {
            let worker = WorkerIndex::from_u32(n);
            let batch = BatchIndex::from_u32(n);
            assert_eq!(worker.to_string(), n.to_string());
            assert_eq!(serde_json::to_string(&worker).unwrap(), n.to_string());
            assert_eq!(serde_json::to_string(&batch).unwrap(), n.to_string());
            assert_eq!(
                serde_json::from_str::<WorkerIndex>(&n.to_string()).unwrap(),
                worker
            );
            assert_eq!(
                serde_json::from_str::<BatchIndex>(&n.to_string()).unwrap(),
                batch
            );
        }
        assert!(serde_json::from_str::<BatchIndex>("4294967295").is_err());
        assert!(serde_json::from_str::<BatchIndex>("-1").is_err());
    }

    #[test]
    fn the_first_lane_is_lane_0_and_pytest_xdist_calls_it_gw0() {
        assert!(WorkerIndex::FIRST.is_first());
        assert_eq!(WorkerIndex::FIRST, WorkerIndex::from_u32(0));
        assert!(!WorkerIndex::from_u32(1).is_first());
        assert_eq!(WorkerIndex::FIRST.xdist_name(), "gw0");
        assert_eq!(WorkerIndex::from_u32(3).xdist_name(), "gw3");
    }
}
