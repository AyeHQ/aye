//! A run's identities: the lanes its workers run on and the batches they
//! are handed.

use std::fmt;

use aye_index::newtype_index;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
