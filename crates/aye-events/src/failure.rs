//! Structured failures: the exception, the assertion and how its operands
//! differ, and the frames it was raised through.

use serde::{Deserialize, Serialize};

use crate::outcome::Phase;
use crate::path::{FramePath, Location};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Failure {
    pub phase: Option<Phase>,
    /// The exception's type name, e.g. `AssertionError`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<String>,
    /// One line.
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assertion: Option<Assertion>,
    /// Where the exception was raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
    /// User frames, innermost last; runner, engine and framework internals
    /// are removed by the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<Frame>,
    /// Groups identical failures: exception type, normalised message and the
    /// innermost user frame, hashed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature: String,
    /// The framework's own full text, bounded, for renderers that want it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The resource (pytest's fixture) whose setup or teardown raised it,
    /// when it failed in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
    /// One line suggesting a fix, e.g. the names closest to an unknown
    /// resource (`did you mean cart?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Assertion {
    pub op: String,
    pub left: String,
    pub right: String,
    /// Explanation lines (the diff), without the leading assertion line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diff: Vec<String>,
    /// Which operand is the expected value: the one written as a literal or
    /// a named constant. Absent when neither or both are; their source then
    /// names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Side>,
    /// The left operand's source text, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_source: Option<String>,
    /// The right operand's source text, as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_source: Option<String>,
    /// How the operands differ, by their structure rather than their text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare: Option<Compare>,
}

/// One operand of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
}

/// How two compared values differ, entry by entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Compare {
    /// What was compared: `dict`, `list`, `tuple`, `set`, `str`, `bytes`,
    /// `dataclass`, `namedtuple`, `datetime`, `number`, or a type's name.
    pub kind: String,
    /// One line saying how they differ, e.g. `dict differs in 1 of 2 keys`.
    pub summary: String,
    /// Entries (keys, items, fields) that are the same.
    #[serde(default, skip_serializing_if = "crate::serde_ext::is_zero_u32")]
    pub same: u32,
    /// Entries that differ, in order, at most 50.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub differs: Vec<Difference>,
    /// Differing entries left out of `differs`.
    #[serde(default, skip_serializing_if = "crate::serde_ext::is_zero_u32")]
    pub omitted: u32,
}

/// One entry that differs between two compared values.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Difference {
    /// Where, written as an access on the value: `['pear']`, `[3]`,
    /// `.total`; for text, where it first differs (`line 2, column 5`).
    /// Absent when the values differ as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The left operand's entry, shown as Python shows it; absent when the
    /// left has none there (only the right has it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<String>,
    /// The right operand's entry; absent when only the left has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Frame {
    /// The frame's file: the project's, or outside it.
    pub path: FramePath,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The line, without its indentation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The failing expression's span in `source`: 0-based character
    /// offsets, the end excluded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<[u32; 2]>,
    /// The lines just before `line`, at most two, oldest first, with their
    /// indentation relative to the least indented of them and `line`. Only
    /// on the innermost project frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    /// How far `line` is indented beyond the least indented of `before`
    /// (`source` has no indentation of its own).
    #[serde(default, skip_serializing_if = "crate::serde_ext::is_zero_u32")]
    pub indent: u32,
    /// The frame's local variables when it failed, as Python shows them,
    /// bounded. Only on the innermost frame.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locals: Vec<(String, String)>,
}
