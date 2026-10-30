//! What a batch's executor is told beside its ids: the adapter's own
//! context ([`AdapterContext`]), and the keys the binary and the runner
//! add to it ([`BatchContext`], named in [`context_keys`]).

use aye_events::TestId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// What an adapter's executor needs from its recognizer (pytest's rootdir
/// and options, unittest's start directory), sent with every batch and
/// refinement. The adapter builds it once, from its own typed context
/// ([`AdapterContext::of`]); only its Python reads it, so it is opaque to
/// Rust. `null` for an adapter that gives none.
///
/// It serializes as the JSON value it holds, an object's keys sorted:
/// `serde_json` is built without `preserve_order`, so a typed context
/// gives the bytes of the object built by hand before it, whatever the
/// order of its fields.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterContext(Value);

impl AdapterContext {
    /// The context `context` serializes to.
    ///
    /// Panics if it isn't JSON: a map whose keys aren't strings.
    pub fn of(context: &impl Serialize) -> Self {
        Self(serde_json::to_value(context).expect("an adapter's context is JSON"))
    }

    pub const fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

/// A context as written: read back from the wire, or a test's.
impl From<Value> for AdapterContext {
    fn from(value: Value) -> Self {
        Self(value)
    }
}

/// The keys of a batch's context that the binary and the runner add to
/// the adapter's (see [`BatchContext`]).
pub mod context_keys {
    /// [`BatchContext::requested_ids`](super::BatchContext::requested_ids).
    pub const REQUESTED_IDS: &str = "requested_ids";
    /// [`BatchContext::rerun_args`](super::BatchContext::rerun_args).
    pub const RERUN_ARGS: &str = "rerun_args";
    /// [`BatchContext::selected_paths`](super::BatchContext::selected_paths).
    pub const SELECTED_PATHS: &str = "selected_paths";
    /// [`BatchContext::ran_ids`](super::BatchContext::ran_ids).
    pub const RAN_IDS: &str = "ran_ids";

    /// The keys that don't change the session a batch runs in: an executor
    /// that lists the session keeps it for a batch whose context differs
    /// from the one it listed with only in these (the pytest executor's
    /// `BATCH_KEYS`). [`SELECTED_PATHS`] is deliberately not one: other
    /// paths are another session, since pytest's start-up conftests and
    /// initial paths are theirs.
    pub const SESSION_NEUTRAL: [&str; 3] = [REQUESTED_IDS, RAN_IDS, RERUN_ARGS];
}

/// A batch's context as the worker gets it: the adapter's, with what the
/// binary and the runner add. On the wire it is the adapter's object with
/// each of these that holds something among its keys (where an adapter's
/// key has the same name, the added one replaces it); the binary's go
/// only into an object, and [`Self::ran_ids`] makes a `null` context an
/// object to go in. Any other context is sent as the adapter gave it.
///
/// Read back, it is all the adapter's, as written.
#[derive(Debug, Clone, Default)]
pub struct BatchContext {
    pub adapter: AdapterContext,
    /// Added by the binary: the ids with parameters the command line names
    /// (`path::test[x]`), relative to the root, so the framework reports
    /// one it can't find instead of running its test's other parameters
    /// in its place.
    pub requested_ids: Vec<TestId>,
    /// Added by the binary: aye's own options that shape the run (`-c
    /// FILE`), as given, which a test's rerun command repeats before its
    /// id (`aye -c FILE <id>`), so that it reruns from where aye ran as
    /// aye ran it.
    pub rerun_args: Vec<String>,
    /// Added by the binary when a positional argument isn't a path (a
    /// dotted name, a test key): the paths the user's run starts from,
    /// absolute, in the order of the arguments, each once: each path
    /// argument (an id's file), and the files of the cases each other
    /// argument selects, as `pytest <these>` would be given them. The
    /// adapter's own context saw only the path arguments (with none, its
    /// framework's default), so an adapter whose framework starts from the
    /// paths it is given (pytest's start-up conftests and initial paths)
    /// starts from these instead.
    pub selected_paths: Vec<String>,
    /// Added by the runner to a batch it re-queued after its worker
    /// crashed: the tests that ran before, which the adapter doesn't run
    /// again.
    pub ran_ids: Vec<TestId>,
}

impl BatchContext {
    /// The adapter's context, with nothing added.
    pub fn new(adapter: AdapterContext) -> Self {
        Self {
            adapter,
            ..Self::default()
        }
    }

    /// Whether nothing is added to the adapter's context.
    fn adds_nothing(&self) -> bool {
        self.requested_ids.is_empty()
            && self.rerun_args.is_empty()
            && self.selected_paths.is_empty()
            && self.ran_ids.is_empty()
    }

    /// The context as the worker gets it (see [`BatchContext`]).
    pub fn to_wire(&self) -> Value {
        let mut context = self.adapter.as_value().clone();
        if let Some(object) = context.as_object_mut() {
            add(
                object,
                context_keys::REQUESTED_IDS,
                self.requested_ids.iter().map(TestId::as_str),
            );
            add(
                object,
                context_keys::SELECTED_PATHS,
                self.selected_paths.iter().map(String::as_str),
            );
            add(
                object,
                context_keys::RERUN_ARGS,
                self.rerun_args.iter().map(String::as_str),
            );
        }
        if !self.ran_ids.is_empty() && context.is_null() {
            context = Value::Object(Map::new());
        }
        if let Some(object) = context.as_object_mut() {
            add(
                object,
                context_keys::RAN_IDS,
                self.ran_ids.iter().map(TestId::as_str),
            );
        }
        context
    }
}

/// `key` in `object`, holding `values`, when there are some.
fn add<'a>(object: &mut Map<String, Value>, key: &str, values: impl Iterator<Item = &'a str>) {
    let values: Vec<Value> = values.map(Value::from).collect();
    if !values.is_empty() {
        object.insert(key.to_string(), Value::Array(values));
    }
}

/// Two contexts are equal when the worker gets the same.
impl PartialEq for BatchContext {
    fn eq(&self, other: &Self) -> bool {
        self.to_wire() == other.to_wire()
    }
}

impl From<AdapterContext> for BatchContext {
    fn from(adapter: AdapterContext) -> Self {
        Self::new(adapter)
    }
}

impl Serialize for BatchContext {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.adds_nothing() {
            return self.adapter.serialize(serializer);
        }
        self.to_wire().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for BatchContext {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        AdapterContext::deserialize(deserializer).map(Self::new)
    }
}
