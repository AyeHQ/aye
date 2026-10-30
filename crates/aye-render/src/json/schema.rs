//! Every line JSON lines writes, as it is serialized: each field in the
//! order it is written, and when it is left out.

use std::borrow::Cow;

use aye_events::{Local, Outcome, StopReason, TestId, WorkerIndex};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use super::limits::{ID_MAX, LOCAL_MAX};
use crate::words;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Line<'a> {
    Case {
        id: Cow<'a, str>,
        #[serde(skip_serializing_if = "is_false")]
        partial: bool,
    },
    Fail(Box<Fail<'a>>),
    LifetimeFail(Box<LifetimeFail<'a>>),
    CollectionError {
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        code: Cow<'a, str>,
        error: Cow<'a, str>,
    },
    WorkerError {
        #[serde(skip_serializing_if = "Option::is_none")]
        worker: Option<WorkerIndex>,
        error: Cow<'a, str>,
    },
    Test {
        #[serde(skip_serializing_if = "Option::is_none")]
        key: Option<String>,
        id: Cow<'a, str>,
        outcome: &'static str,
        ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<Cow<'a, str>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cause: Option<Cow<'a, str>>,
    },
    Diagnostic {
        severity: &'static str,
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
        code: Cow<'a, str>,
        message: Cow<'a, str>,
    },
    Stale {
        path: Cow<'a, str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        rerun: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        gone: bool,
    },
    Summary(Summary),
}

#[derive(Serialize)]
pub(super) struct Fail<'a> {
    pub(super) key: String,
    pub(super) id: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) sub: Option<Cow<'a, str>>,
    pub(super) outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cause: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) same_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) why: Option<String>,
    #[serde(flatten)]
    pub(super) shown: Option<Shown>,
    #[serde(flatten)]
    pub(super) detail: Detail<'a>,
}

impl<'a> Fail<'a> {
    pub(super) fn new(key: String, id: &'a TestId, outcome: Outcome) -> Self {
        Self {
            key,
            id: words::head(id.as_str(), ID_MAX),
            sub: None,
            outcome: outcome.as_str(),
            phase: None,
            cause: None,
            same_as: None,
            at: None,
            why: None,
            shown: None,
            detail: Detail::default(),
        }
    }
}

#[derive(Serialize)]
pub(super) struct LifetimeFail<'a> {
    pub(super) key: String,
    pub(super) lifetime: Cow<'a, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) phase: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) same_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) why: Option<String>,
    #[serde(flatten)]
    pub(super) shown: Option<Shown>,
    #[serde(flatten)]
    pub(super) detail: Detail<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) blocked: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) blocked_omitted: usize,
}

/// What `aye show` adds to a bad outcome's line.
#[derive(Serialize)]
pub(super) struct Shown {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) python: Option<String>,
    pub(super) stale: bool,
}

/// L2 and L3.
#[derive(Serialize, Default)]
pub(super) struct Detail<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) cols: Option<[u32; 2]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) before: Vec<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) diff: Option<Diff<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) left_source: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) right_source: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expected: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) actual: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) left: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) right: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) hint: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fixture: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) setup: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) history: Option<History>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) also: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) rerun: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ms: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) frames: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) output: Option<Output>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) locals: Option<Locals<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub(super) enum Diff<'a> {
    Compare(CompareLine<'a>),
    Lines(Vec<Cow<'a, str>>),
}

#[derive(Serialize)]
pub(super) struct CompareLine<'a> {
    pub(super) kind: Cow<'a, str>,
    #[serde(skip_serializing_if = "str::is_empty")]
    pub(super) summary: Cow<'a, str>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) same: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) differs: Vec<Entry<'a>>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) omitted: u32,
}

#[derive(Serialize)]
pub(super) struct Entry<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) at: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expected: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) actual: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) left: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) right: Option<Cow<'a, str>>,
}

#[derive(Serialize)]
pub(super) struct History {
    pub(super) previous: usize,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) passed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) failed: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) since: Option<String>,
}

/// The innermost frame's locals, as an object in their order.
pub(super) struct Locals<'a>(pub(super) &'a [Local]);

impl Serialize for Locals<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for local in self.0 {
            map.serialize_entry(
                &words::head(&local.name, LOCAL_MAX),
                &words::head(&local.value, LOCAL_MAX),
            )?;
        }
        map.end()
    }
}

#[derive(Serialize)]
pub(super) struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stdout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stderr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) log: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Summary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) run: Option<String>,
    pub(super) exit: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tests: Option<u32>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) failed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) errors: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) passed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) skipped: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) expected_failure: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) unexpected_pass: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) deselected: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) setup_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) teardown_error: u32,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) lifetime_error: u32,
    pub(super) s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) stopped: Option<StopReason>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) failures: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub(super) failures_omitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) more: Option<&'static str>,
}

pub(super) fn is_zero<T: Default + PartialEq>(n: &T) -> bool {
    *n == T::default()
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "signature fixed by serde"
)]
pub(super) const fn is_false(b: &bool) -> bool {
    !*b
}
