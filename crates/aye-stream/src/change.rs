//! How a file of a stored run changed since ([`FileChange`]): what the run
//! store finds, and what `aye show` warns of.

/// How a file differs from what a stored run read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChange {
    /// Its content changed.
    Edited,
    /// It no longer exists.
    Gone,
}
