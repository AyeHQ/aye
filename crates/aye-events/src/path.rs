//! Places in the project's files: spans and locations.

use serde::{Deserialize, Serialize};

/// A file and a byte range in it. Separate from ids: failure locations,
/// snapshots and the source rewriter all point at spans.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    /// Relative to the project root, `/` separators.
    pub path: String,
    pub start: u32,
    pub end: u32,
    /// 1-based line of `start`, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A file and a 1-based line, where only a line is known (traceback frames).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub path: String,
    pub line: u32,
}
