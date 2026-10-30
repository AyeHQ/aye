//! Structured failures: the exception, the assertion and how its operands
//! differ, the frames it was raised through, and the signature that groups
//! identical ones.

use serde::{Deserialize, Deserializer, Serialize};

use crate::outcome::Phase;
use crate::path::{FramePath, Location};
use crate::serde_ext::string_enum;

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
    /// Groups identical failures; `None` for a failure never grouped.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "FailureSignature::read"
    )]
    pub signature: Option<FailureSignature>,
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

/// What groups identical failures: the exception's type, its message with
/// numbers and addresses blanked out, and where it was raised (else the
/// adapter's own place for it, a doctest's example), hashed: the first 16
/// hex digits of a SHA-1 (`aye_engine.signature`). The renderers show the
/// first failure of a signature, and say of the others that they are the
/// same.
///
/// It is never empty: a failure whose `signature` is `""` on the wire has
/// none, and is written back without it. Any other text is kept as
/// written. It serializes as its text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct FailureSignature(Box<str>);

impl FailureSignature {
    /// The signature `text` spells; `None` for the empty text, which groups
    /// nothing.
    pub fn new(text: impl Into<Box<str>>) -> Option<Self> {
        let text = text.into();
        (!text.is_empty()).then_some(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// A failure's `signature`: a string, `""` being none.
    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Self>, D::Error> {
        Box::<str>::deserialize(deserializer).map(Self::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Assertion {
    /// How the operands were compared.
    pub op: ComparisonOp,
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

string_enum! {
    /// How an assertion compared its operands: an operator of a rewritten
    /// `assert` (the rewriter's `_COMPARISONS`, `aye_rewrite.py`), or the
    /// comparison a unittest `assert*` method makes (the unittest adapter's
    /// `_COMPARISONS`). One this aye doesn't know reads as
    /// [`ComparisonOp::Other`].
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum ComparisonOp {
        Eq = "==",
        NotEq = "!=",
        Lt = "<",
        LtE = "<=",
        Gt = ">",
        GtE = ">=",
        Is = "is",
        IsNot = "is not",
        In = "in",
        NotIn = "not in",
        /// unittest's `assertIsInstance`.
        IsInstance = "isinstance",
        /// unittest's `assertNotIsInstance`.
        NotIsInstance = "not isinstance",
    }
}

/// The empty operator, which names no comparison: a default
/// [`Assertion`]'s.
impl Default for ComparisonOp {
    fn default() -> Self {
        Self::Other(Box::default())
    }
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
    /// What was compared.
    pub kind: CompareKind,
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

string_enum! {
    /// What kind of values a [`Compare`] compared: one of the kinds
    /// `aye_engine.compare` knows (its `_COMPARERS`, `aye_engine.py`), both
    /// operands being of it. One this aye doesn't know reads as
    /// [`CompareKind::Other`].
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum CompareKind {
        /// Two mappings (any `Mapping`).
        Dict = "dict",
        List = "list",
        /// A tuple, or a namedtuple compared with another tuple or another
        /// type's namedtuple.
        Tuple = "tuple",
        /// Two sets (any `AbstractSet`).
        Set = "set",
        /// Two dataclasses of one type.
        Dataclass = "dataclass",
        /// Two namedtuples of one type.
        Namedtuple = "namedtuple",
        Str = "str",
        /// `bytes` or `bytearray`.
        Bytes = "bytes",
        Datetime = "datetime",
        Date = "date",
        Time = "time",
        /// Two numbers (`numbers.Number`), `bool`s aside.
        Number = "number",
    }
}

impl CompareKind {
    /// Whether the values were text, `str` or `bytes`, which differ at a
    /// position rather than by entries.
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Str | Self::Bytes)
    }
}

/// The empty kind, which names none: a default [`Compare`]'s.
impl Default for CompareKind {
    fn default() -> Self {
        Self::Other(Box::default())
    }
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
    /// The failing expression's span in `source`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<Columns>,
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
    pub locals: Vec<Local>,
}

/// A span of characters in a source line: 0-based character offsets, the
/// end excluded. `[start, end]` on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "[u32; 2]", into = "[u32; 2]")]
pub struct Columns {
    pub start: u32,
    pub end: u32,
}

impl From<[u32; 2]> for Columns {
    fn from([start, end]: [u32; 2]) -> Self {
        Self { start, end }
    }
}

impl From<Columns> for [u32; 2] {
    fn from(columns: Columns) -> Self {
        [columns.start, columns.end]
    }
}

/// A local variable of a frame, its value as Python shows it. `[name,
/// value]` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(String, String)", into = "(String, String)")]
pub struct Local {
    pub name: String,
    pub value: String,
}

impl From<(String, String)> for Local {
    fn from((name, value): (String, String)) -> Self {
        Self { name, value }
    }
}

impl From<Local> for (String, String) {
    fn from(local: Local) -> Self {
        (local.name, local.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each operator and kind is written as Python writes it, and one this
    /// aye doesn't know reads back as it was.
    #[test]
    fn operators_and_kinds_are_spelled_as_python_spells_them() {
        let ops: Vec<&str> = ComparisonOp::ALL.iter().map(ComparisonOp::as_str).collect();
        assert_eq!(
            ops,
            [
                "==",
                "!=",
                "<",
                "<=",
                ">",
                ">=",
                "is",
                "is not",
                "in",
                "not in",
                "isinstance",
                "not isinstance"
            ]
        );
        let kinds: Vec<&str> = CompareKind::ALL.iter().map(CompareKind::as_str).collect();
        assert_eq!(
            kinds,
            [
                "dict",
                "list",
                "tuple",
                "set",
                "dataclass",
                "namedtuple",
                "str",
                "bytes",
                "datetime",
                "date",
                "time",
                "number"
            ]
        );
        for text in ["is not", "~=", ""] {
            let op: ComparisonOp = serde_json::from_str(&format!("\"{text}\"")).unwrap();
            assert_eq!(serde_json::to_string(&op).unwrap(), format!("\"{text}\""));
        }
        assert_eq!(ComparisonOp::from("~="), ComparisonOp::Other("~=".into()));
        assert_eq!(
            CompareKind::from("Decimal"),
            CompareKind::Other("Decimal".into())
        );
        assert_eq!(Assertion::default().op.as_str(), "");
        assert_eq!(Compare::default().kind.as_str(), "");
    }

    /// An empty signature is none, and is left out when written; any other
    /// is kept as written.
    #[test]
    fn an_empty_signature_is_none() {
        let read = |json: &str| serde_json::from_str::<Failure>(json).unwrap();
        let failure = read(r#"{"phase":null,"message":"m","signature":""}"#);
        assert_eq!(failure.signature, None);
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"phase":null,"message":"m"}"#
        );
        assert_eq!(read(r#"{"phase":null,"message":"m"}"#).signature, None);
        let failure = read(r#"{"phase":null,"message":"m","signature":"not hex"}"#);
        assert_eq!(failure.signature, FailureSignature::new("not hex"));
        assert_eq!(
            serde_json::to_string(&failure).unwrap(),
            r#"{"phase":null,"message":"m","signature":"not hex"}"#
        );
        assert!(
            serde_json::from_str::<Failure>(r#"{"phase":null,"message":"m","signature":null}"#)
                .is_err()
        );
        assert_eq!(FailureSignature::new(""), None);
    }

    #[test]
    fn only_strings_and_bytes_are_text() {
        let text: Vec<&CompareKind> = CompareKind::ALL.iter().filter(|k| k.is_text()).collect();
        assert_eq!(text, [&CompareKind::Str, &CompareKind::Bytes]);
        assert!(!CompareKind::from("text").is_text());
    }
}
