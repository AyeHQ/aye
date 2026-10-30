//! The unittest adapter's context ([`UnittestContext`]): what the executor
//! needs to discover as unittest would, built once by the recognizer and
//! sent with every batch and refinement.

use serde::Serialize;

use crate::UnittestRecognizer;

/// The unittest adapter's context. Its keys are written in order,
/// whatever the order of the fields ([`aye_protocol::AdapterContext`]).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UnittestContext {
    /// Discovery's start directory, absolute (`discover -s`).
    start_directory: String,
    /// Discovery's top-level directory, absolute (`discover -t`).
    top_level_directory: String,
    /// The file pattern (`discover -p`).
    pattern: String,
    /// The directories named on the command line outside the start
    /// directory, absolute, each its own start; left out when there are
    /// none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    starts: Vec<String>,
    /// The files named on the command line below a package that may define
    /// `load_tests`, absolute, each loaded alone; left out when there are
    /// none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    named: Vec<String>,
}

impl UnittestContext {
    pub(crate) fn new(recognizer: &UnittestRecognizer) -> Self {
        let path = |path: &std::path::Path| path.to_string_lossy().into_owned();
        let configured = &recognizer.starts.configured;
        Self {
            start_directory: path(&configured.start),
            top_level_directory: path(&configured.top),
            pattern: recognizer.config.pattern.value.clone(),
            starts: recognizer
                .starts
                .named
                .iter()
                .map(|named| path(&named.start))
                .collect(),
            named: recognizer
                .named_below_load_tests()
                .into_iter()
                .map(|named| path(named))
                .collect(),
        }
    }
}
