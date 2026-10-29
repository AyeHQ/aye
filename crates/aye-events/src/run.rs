//! A run's identities: its id, its number in the run store, the
//! environment it ran in, the lanes its workers run on and the batches they
//! are handed; and the stamper that writes the run's part of each record's
//! envelope.

use std::borrow::Borrow;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::num::NonZeroU64;
use std::str::FromStr;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use aye_index::newtype_index;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::event::Record;

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

/// A run's number in the run store, from 1: the store claims it before the
/// run's `run_started`, which says it (`run_number`). The history keeps the
/// number of the run a test's streak of bad outcomes began in, and a bad
/// outcome's `history` says it (`since`).
///
/// It prints as the store spells it, `r81`, reads from `r81` or `81` (as
/// `aye show --run` takes it), and serializes as the number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunNumber(NonZeroU64);

impl RunNumber {
    /// The run numbered `n`; `None` for 0, which numbers no run.
    pub const fn new(n: u64) -> Option<Self> {
        match NonZeroU64::new(n) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// `r81`.
impl fmt::Display for RunNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "r{}", self.0)
    }
}

/// `r81` or `81`: ASCII digits, with leading zeros or without, after an `r`
/// or not, of a number from 1.
impl FromStr for RunNumber {
    type Err = InvalidRunNumber;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let digits = text.strip_prefix('r').unwrap_or(text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(InvalidRunNumber(text.into()));
        }
        digits
            .parse()
            .ok()
            .and_then(Self::new)
            .ok_or_else(|| InvalidRunNumber(text.into()))
    }
}

impl Serialize for RunNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.get())
    }
}

/// A number from 1; 0 is refused.
impl<'de> Deserialize<'de> for RunNumber {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let n = u64::deserialize(deserializer)?;
        Self::new(n).ok_or_else(|| serde::de::Error::custom("run number 0"))
    }
}

/// A text that is not a run's number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidRunNumber(Box<str>);

impl fmt::Display for InvalidRunNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` is not a run's number", self.0)
    }
}

impl std::error::Error for InvalidRunNumber {}

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

/// Stamps the run's part of every record's envelope: the run id, the
/// environment, the run-wide `seq` and `t_ms` since the run started. The
/// binary stamps its own records (`run_started`, collection) with the same
/// stamper it hands to the runner, which stamps the records it merges from
/// its workers, so `seq` is one sequence.
#[derive(Debug)]
pub struct Stamper {
    run: RunId,
    environment: Option<EnvironmentId>,
    started: Instant,
    next: u64,
}

impl Default for Stamper {
    fn default() -> Self {
        Self::new()
    }
}

impl Stamper {
    /// A new run, with a fresh id, starting now.
    pub fn new() -> Self {
        Self::with_run(RunId::random())
    }

    pub fn with_run(run: impl Into<RunId>) -> Self {
        Self {
            run: run.into(),
            environment: None,
            started: Instant::now(),
            next: 0,
        }
    }

    #[must_use]
    pub fn environment(mut self, environment: impl Into<EnvironmentId>) -> Self {
        self.environment = Some(environment.into());
        self
    }

    pub fn run_id(&self) -> &RunId {
        &self.run
    }

    pub fn started(&self) -> Instant {
        self.started
    }

    /// The `seq` the next record gets.
    pub fn next_seq(&self) -> u64 {
        self.next
    }

    /// Set `run`, `environment` (when known), `seq` and `t_ms`; keep
    /// `worker` and `batch`.
    pub fn stamp(&mut self, mut record: Record) -> Record {
        let envelope = &mut record.envelope;
        envelope.run = Some(self.run.clone());
        if self.environment.is_some() {
            envelope.environment.clone_from(&self.environment);
        }
        envelope.seq = Some(self.next);
        envelope.t_ms = Some((self.started.elapsed().as_secs_f64() * 1_000_000.0).round() / 1000.0);
        self.next += 1;
        record
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::Event;

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

    #[test]
    fn stamps_run_seq_and_time_and_keeps_worker_and_batch() {
        let mut stamper = Stamper::with_run("r1").environment("py313");
        let mut record = Record::new(Event::CollectionStarted);
        record.envelope.worker = Some(WorkerIndex::from_u32(2));
        record.envelope.batch = Some(BatchIndex::from_u32(5));
        record.envelope.seq = Some(99);
        record.envelope.t_ms = Some(123_456.0);
        let first = stamper.stamp(record);
        let second = stamper.stamp(Record::new(Event::CollectionStarted));
        assert_eq!(first.envelope.run, Some("r1".into()));
        assert_eq!(first.envelope.environment, Some("py313".into()));
        assert_eq!(
            (first.envelope.worker, first.envelope.batch),
            (
                Some(WorkerIndex::from_u32(2)),
                Some(BatchIndex::from_u32(5))
            )
        );
        assert_eq!(
            (first.envelope.seq, second.envelope.seq),
            (Some(0), Some(1))
        );
        assert!(first.envelope.t_ms.unwrap() < 1000.0);
        assert!(second.envelope.t_ms >= first.envelope.t_ms);
        assert_eq!(stamper.next_seq(), 2);
    }

    /// A run's number prints as `r81`, reads from `r81` or `81`, and
    /// serializes as the number it was when it was a `u64`.
    #[test]
    fn a_run_number_is_r_and_its_digits() {
        let n = RunNumber::new(81).unwrap();
        assert_eq!(n.get(), 81);
        assert_eq!(n.to_string(), "r81");
        for text in ["r81", "81", "r081", "081"] {
            assert_eq!(text.parse::<RunNumber>(), Ok(n), "{text}");
        }
        for text in [
            "",
            "r",
            "rr81",
            "R81",
            "r0",
            "0",
            "r-1",
            "+81",
            "r8a",
            "r99999999999999999999",
        ] {
            assert!(text.parse::<RunNumber>().is_err(), "{text}");
        }
        assert_eq!(RunNumber::new(0), None);
        assert_eq!(serde_json::to_string(&n).unwrap(), "81");
        assert_eq!(serde_json::from_str::<RunNumber>("81").unwrap(), n);
        assert!(serde_json::from_str::<RunNumber>("0").is_err());
        assert!(RunNumber::new(9) < RunNumber::new(10));
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
